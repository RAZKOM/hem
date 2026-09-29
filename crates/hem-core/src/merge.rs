//! Multi-source merge: interleave several sources by timestamp into one
//! ordered list of rows.  Lines without a timestamp inherit the previous
//! line's timestamp in the same source, so multi-line entries (stack
//! traces) stay together.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use crate::parse::parse_line;
use crate::source::Source;
use crate::timestamp;
use crate::view::{Base, Row, CHUNK_ROWS};

/// Maximum number of sources in a merge (rows carry an 8-bit source id).
pub const MAX_SOURCES: usize = 256;

/// A timestamp-merged set of sources.
pub struct Merged {
    pub sources: Vec<Source>,
    rows: RwLock<Vec<Row>>,
    generation: AtomicU32,
    complete: AtomicBool,
    cancel: AtomicBool,
    wakers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
}

impl std::fmt::Debug for Merged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Merged")
            .field("sources", &self.sources.len())
            .field("rows", &self.len())
            .finish()
    }
}

impl Drop for Merged {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn combined_generation(sources: &[Source]) -> u32 {
    sources.iter().fold(0u32, |acc, s| {
        acc.wrapping_mul(31).wrapping_add(s.generation())
    })
}

impl Merged {
    pub fn start(mut sources: Vec<Source>) -> Arc<Merged> {
        sources.truncate(MAX_SOURCES);
        let gen = combined_generation(&sources);
        let m = Arc::new(Merged {
            sources,
            rows: RwLock::new(Vec::new()),
            generation: AtomicU32::new(gen),
            complete: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            wakers: Mutex::new(Vec::new()),
        });
        let m2 = Arc::clone(&m);
        let _ = std::thread::Builder::new()
            .name("hem-merge".into())
            .spawn(move || merge_main(m2));
        m
    }

    pub fn len(&self) -> usize {
        self.rows.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn rows_range(&self, a: usize, b: usize) -> Vec<Row> {
        let rows = self.rows.read();
        let b = b.min(rows.len());
        rows[a.min(b)..b].to_vec()
    }

    pub fn row(&self, i: usize) -> Option<Row> {
        self.rows.read().get(i).copied()
    }

    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn add_waker(&self, w: Arc<dyn Fn() + Send + Sync>) {
        self.wakers.lock().push(w);
    }

    fn wake(&self) {
        for w in self.wakers.lock().iter() {
            w();
        }
    }
}

struct Cursor {
    next_line: usize,
    carry_ts: i64,
    pending: VecDeque<(i64, u64)>,
}

/// Parse timestamps for lines `a..b` of a source, with inheritance.
fn stamp_lines(
    src: &Source,
    a: usize,
    b: usize,
    carry: &mut i64,
    default_year: i64,
) -> Vec<(i64, u64)> {
    let base = Base::Single(src.clone());
    let chunk = base.chunk(a, b);
    let fmt = src.format();
    let mut out = Vec::with_capacity(chunk.len());
    for i in 0..chunk.len() {
        let m = parse_line(chunk.line(i), fmt, default_year);
        if let Some(t) = m.ts {
            *carry = t;
        }
        out.push((*carry, chunk.row(i).line()));
    }
    out
}

fn merge_main(m: Arc<Merged>) {
    let (tx, rx) = mpsc::channel::<()>();
    for s in &m.sources {
        let tx = tx.clone();
        s.add_waker(Arc::new(move || {
            let _ = tx.send(());
        }));
    }
    let default_year = timestamp::current_year();
    let mut cursors: Vec<Cursor> = m
        .sources
        .iter()
        .map(|_| Cursor {
            next_line: 0,
            carry_ts: i64::MIN,
            pending: VecDeque::new(),
        })
        .collect();
    let mut gen = combined_generation(&m.sources);
    let mut last_wake = Instant::now();
    const PENDING_TARGET: usize = CHUNK_ROWS * 4;

    loop {
        if m.cancel.load(Ordering::Acquire) {
            break;
        }
        let g = combined_generation(&m.sources);
        if g != gen {
            gen = g;
            for c in &mut cursors {
                c.next_line = 0;
                c.carry_ts = i64::MIN;
                c.pending.clear();
            }
            m.rows.write().clear();
            m.generation.store(g, Ordering::Release);
            m.wake();
        }

        // Refill pending queues.
        let mut refilled = false;
        for (i, src) in m.sources.iter().enumerate() {
            let c = &mut cursors[i];
            let avail = src.line_count();
            if c.pending.len() < PENDING_TARGET && avail > c.next_line {
                let b = (c.next_line + PENDING_TARGET).min(avail);
                let stamped = stamp_lines(src, c.next_line, b, &mut c.carry_ts, default_year);
                c.next_line += stamped.len();
                c.pending.extend(stamped);
                refilled = true;
            }
        }

        // Merge while every source can participate.
        let mut emitted = Vec::new();
        loop {
            let mut best: Option<(i64, usize)> = None;
            let mut blocked = false;
            for (i, c) in cursors.iter().enumerate() {
                match c.pending.front() {
                    Some(&(ts, _)) => {
                        if best.map(|(bt, _)| ts < bt).unwrap_or(true) {
                            best = Some((ts, i));
                        }
                    }
                    None => {
                        let src = &m.sources[i];
                        let more = src.line_count() > c.next_line;
                        if more || !src.is_complete() {
                            blocked = true;
                            break;
                        }
                    }
                }
            }
            if blocked {
                break;
            }
            let Some((_, i)) = best else { break };
            let (_, line) = cursors[i].pending.pop_front().unwrap_or((0, 0));
            emitted.push(Row::new(line, i as u8, 1));
            if emitted.len() >= CHUNK_ROWS {
                break;
            }
        }
        if !emitted.is_empty() {
            m.rows.write().extend_from_slice(&emitted);
            m.complete.store(false, Ordering::Release);
            if last_wake.elapsed() > Duration::from_millis(40) {
                m.wake();
                last_wake = Instant::now();
            }
            continue;
        }
        if refilled {
            continue;
        }

        let all_drained = cursors
            .iter()
            .zip(&m.sources)
            .all(|(c, s)| c.pending.is_empty() && s.line_count() <= c.next_line && s.is_complete());
        if all_drained {
            m.complete.store(true, Ordering::Release);
            m.wake();
            if !m.sources.iter().any(|s| s.is_live()) {
                break;
            }
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while rx.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(m: &Merged) {
        let start = Instant::now();
        while !m.is_complete() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn interleaves_by_timestamp() {
        let a = Source::from_bytes(
            "a",
            b"2026-10-13 10:00:00 A1\n2026-10-13 10:00:02 A2\n\tat trace (belongs to A2)\n2026-10-13 10:00:04 A3\n".to_vec(),
        );
        let b = Source::from_bytes(
            "b",
            b"2026-10-13 10:00:01 B1\n2026-10-13 10:00:03 B2\n".to_vec(),
        );
        a.stop();
        b.stop();
        let m = Merged::start(vec![a, b]);
        wait(&m);
        let order: Vec<(u8, u64)> = m
            .rows_range(0, 100)
            .iter()
            .map(|r| (r.source(), r.line()))
            .collect();
        assert_eq!(
            order,
            vec![(0, 0), (1, 0), (0, 1), (0, 2), (1, 1), (0, 3)],
            "trace line stays with its entry"
        );
    }

    #[test]
    fn lines_without_any_timestamp_go_first() {
        let a = Source::from_bytes("a", b"no ts\n2026-10-13 10:00:05 A\n".to_vec());
        let b = Source::from_bytes("b", b"2026-10-13 10:00:01 B\n".to_vec());
        a.stop();
        b.stop();
        let m = Merged::start(vec![a, b]);
        wait(&m);
        let order: Vec<(u8, u64)> = m
            .rows_range(0, 10)
            .iter()
            .map(|r| (r.source(), r.line()))
            .collect();
        assert_eq!(order, vec![(0, 0), (1, 0), (0, 1)]);
    }
}
