//! Incremental background search over a view.
//!
//! Results are row indices, streamed in ascending order so that "next
//! match" is a binary search.  For identity views the scan runs the regex
//! over the raw byte range of each chunk (one `find_iter` per 32k lines
//! instead of one `is_match` per line), which is several times faster.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use crate::filter::Matcher;
use crate::view::{View, CHUNK_ROWS};

struct Inner {
    view: View,
    matcher: Matcher,
    matches: RwLock<Vec<u64>>,
    scanned: AtomicU64,
    complete: AtomicBool,
    cancel: AtomicBool,
    wakers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
}

/// Handle to a running (or finished) search.
#[derive(Clone)]
pub struct Search(Arc<Inner>);

impl std::fmt::Debug for Search {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Search")
            .field("pattern", &self.0.matcher.text)
            .field("matches", &self.count())
            .finish()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Search {
    pub fn start(view: View, matcher: Matcher) -> Search {
        let inner = Arc::new(Inner {
            view,
            matcher,
            matches: RwLock::new(Vec::new()),
            scanned: AtomicU64::new(0),
            complete: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            wakers: Mutex::new(Vec::new()),
        });
        let s = Search(inner);
        let s2 = s.clone();
        let _ = std::thread::Builder::new()
            .name("hem-search".into())
            .spawn(move || search_main(s2));
        s
    }

    pub fn matcher(&self) -> &Matcher {
        &self.0.matcher
    }

    pub fn pattern(&self) -> &str {
        &self.0.matcher.text
    }

    pub fn count(&self) -> usize {
        self.0.matches.read().len()
    }

    pub fn is_complete(&self) -> bool {
        self.0.complete.load(Ordering::Acquire)
    }

    pub fn progress(&self) -> f64 {
        let n = self.0.view.len() as f64;
        if n == 0.0 {
            1.0
        } else {
            (self.0.scanned.load(Ordering::Acquire) as f64 / n).clamp(0.0, 1.0)
        }
    }

    pub fn add_waker(&self, w: Arc<dyn Fn() + Send + Sync>) {
        self.0.wakers.lock().push(w);
    }

    pub fn matches(&self) -> Vec<u64> {
        self.0.matches.read().clone()
    }

    /// Whether row `row` is a match.
    pub fn contains(&self, row: usize) -> bool {
        self.0.matches.read().binary_search(&(row as u64)).is_ok()
    }

    /// Index (within the match list) and row of the first match after
    /// `row`, wrapping around.
    pub fn next_after(&self, row: usize) -> Option<(usize, usize)> {
        let m = self.0.matches.read();
        if m.is_empty() {
            return None;
        }
        let p = m.partition_point(|&r| r <= row as u64);
        let i = if p >= m.len() { 0 } else { p };
        Some((i, m[i] as usize))
    }

    /// Index and row of the last match before `row`, wrapping around.
    pub fn prev_before(&self, row: usize) -> Option<(usize, usize)> {
        let m = self.0.matches.read();
        if m.is_empty() {
            return None;
        }
        let p = m.partition_point(|&r| r < row as u64);
        let i = if p == 0 { m.len() - 1 } else { p - 1 };
        Some((i, m[i] as usize))
    }

    /// 1-based ordinal of the match at `row`, if any.
    pub fn ordinal_of(&self, row: usize) -> Option<usize> {
        self.0
            .matches
            .read()
            .binary_search(&(row as u64))
            .ok()
            .map(|i| i + 1)
    }

    fn wake(&self) {
        for w in self.0.wakers.lock().iter() {
            w();
        }
    }
}

fn scan_chunk(s: &Inner, a: usize, b: usize) -> Vec<u64> {
    let chunk = s.view.chunk(a, b);
    let mut out = Vec::new();
    if chunk.is_empty() {
        return out;
    }
    let re = &s.matcher.regex;
    if s.view.is_identity() && chunk.datas.len() == 1 {
        // Fast path: one regex pass over the contiguous byte range.
        let data = &chunk.datas[0];
        let start = chunk.spans[0].0 as usize;
        let (ls, ll) = chunk.spans[chunk.len() - 1];
        let end = (ls + ll as u64) as usize;
        let Some(hay) = data.get(start..end) else {
            return out;
        };
        let mut last_line: Option<usize> = None;
        for m in re.find_iter(hay) {
            let abs = (start + m.start()) as u64;
            let i = chunk
                .spans
                .partition_point(|(s, _)| *s <= abs)
                .saturating_sub(1);
            // The match must start inside the line (not in its terminator).
            let (ls, ll) = chunk.spans[i];
            if abs >= ls + ll as u64 {
                continue;
            }
            if last_line != Some(i) {
                out.push((a + i) as u64);
                last_line = Some(i);
            }
        }
    } else {
        for i in 0..chunk.len() {
            if re.is_match(chunk.line(i)) {
                out.push((a + i) as u64);
            }
        }
    }
    out
}

fn search_main(search: Search) {
    let s = &search.0;
    let (tx, rx) = mpsc::channel::<()>();
    {
        let tx = tx.clone();
        s.view.add_waker(Arc::new(move || {
            let _ = tx.send(());
        }));
    }
    let threads = crate::par::worker_threads();
    let mut gen = s.view.generation();
    let mut scanned: usize = 0;
    let mut last_wake = Instant::now();
    loop {
        if s.cancel.load(Ordering::Acquire) {
            break;
        }
        let g = s.view.generation();
        if g != gen {
            gen = g;
            scanned = 0;
            s.matches.write().clear();
            s.scanned.store(0, Ordering::Release);
        }
        let avail = s.view.len();
        if avail > scanned {
            s.complete.store(false, Ordering::Release);
            let start = scanned;
            let n_chunks = (avail - start).div_ceil(CHUNK_ROWS);
            crate::par::ordered_scan(
                n_chunks,
                threads,
                &s.cancel,
                |i| {
                    let a = start + i * CHUNK_ROWS;
                    let b = (a + CHUNK_ROWS).min(avail);
                    scan_chunk(s, a, b)
                },
                |i, found| {
                    let b = (start + (i + 1) * CHUNK_ROWS).min(avail);
                    if !found.is_empty() {
                        s.matches.write().extend_from_slice(&found);
                    }
                    scanned = b;
                    s.scanned.store(b as u64, Ordering::Release);
                    if last_wake.elapsed() > Duration::from_millis(40) {
                        search.wake();
                        last_wake = Instant::now();
                    }
                    !s.cancel.load(Ordering::Relaxed)
                },
            );
            search.wake();
            continue;
        }
        if s.view.is_complete() {
            s.complete.store(true, Ordering::Release);
            search.wake();
            if s.view.is_finished() {
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

/// Block until the search is complete (tests / plain mode).
pub fn wait_complete(search: &Search, timeout: Duration) -> bool {
    let start = Instant::now();
    while !search.is_complete() {
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{Filter, FilterSpec};
    use crate::source::Source;
    use crate::view::{Base, ViewSpec};

    fn view(text: &str, spec: ViewSpec) -> View {
        let v = View::start(
            Base::Single(Source::from_bytes("t", text.as_bytes().to_vec())),
            spec,
        );
        assert!(crate::view::wait_complete(&v, Duration::from_secs(5)));
        v
    }

    #[test]
    fn finds_rows_identity() {
        let mut text = String::new();
        for i in 0..100_000 {
            text.push_str(if i % 1000 == 7 {
                "needle here\n"
            } else {
                "hay\n"
            });
        }
        let v = view(&text, ViewSpec::default());
        let s = Search::start(v, Matcher::new("needle", false).unwrap());
        assert!(wait_complete(&s, Duration::from_secs(10)));
        assert_eq!(s.count(), 100);
        assert_eq!(s.next_after(0), Some((0, 7)));
        assert_eq!(s.next_after(7), Some((1, 1007)));
        assert_eq!(s.prev_before(7), Some((99, 99007)), "wraps");
        assert_eq!(s.ordinal_of(1007), Some(2));
        assert!(s.contains(2007));
        assert!(!s.contains(2008));
    }

    #[test]
    fn finds_rows_filtered() {
        let text = "a 1\nb 2\na 3\nb needle 4\na needle 5\n";
        let spec = ViewSpec {
            filter: Filter::from_spec(&FilterSpec {
                include: vec!["^a".into()],
                ..Default::default()
            })
            .unwrap(),
            collapse: false,
        };
        let v = view(text, spec);
        let s = Search::start(v, Matcher::new("needle", false).unwrap());
        assert!(wait_complete(&s, Duration::from_secs(5)));
        assert_eq!(s.matches(), vec![2]);
    }

    #[test]
    fn multiple_matches_per_line_count_once() {
        let v = view("xx xx\nyy\nxx\n", ViewSpec::default());
        let s = Search::start(v, Matcher::new("xx", false).unwrap());
        assert!(wait_complete(&s, Duration::from_secs(5)));
        assert_eq!(s.matches(), vec![0, 2]);
    }
}
