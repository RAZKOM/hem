//! Views: the ordered list of rows the UI scrolls through.
//!
//! A view is built in the background from a [`Base`] (a single source or a
//! timestamp-merged set of sources) by applying a [`Filter`] and optional
//! repeat collapsing.  It also accumulates the timeline histogram and
//! level statistics in the same pass, so the minimap always reflects what
//! is being shown.
//!
//! Rows are packed into 8 bytes: 40 bits line index, 8 bits source index,
//! 16 bits repeat count.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use crate::collapse::shape_hash;
use crate::filter::Filter;
use crate::format::Format;
use crate::level::Level;
use crate::merge::Merged;
use crate::parse::{parse_line, LineMeta};
use crate::source::{Data, Source};
use crate::timeline::Histogram;
use crate::timestamp;

/// Rows processed per work unit.
pub const CHUNK_ROWS: usize = 32 * 1024;
/// How far back to look for an inheritable level / timestamp.
const CARRY_LOOKBACK: usize = 64;
const MAX_COUNT: u32 = 0xFFFF;

/// Packed row reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Row(u64);

impl Row {
    pub const LINE_BITS: u32 = 40;
    pub const LINE_MASK: u64 = (1 << Self::LINE_BITS) - 1;

    pub fn new(line: u64, source: u8, count: u32) -> Self {
        let count = count.min(MAX_COUNT) as u64;
        Row((line & Self::LINE_MASK) | ((source as u64) << 40) | (count << 48))
    }

    pub fn line(self) -> u64 {
        self.0 & Self::LINE_MASK
    }

    pub fn source(self) -> u8 {
        ((self.0 >> 40) & 0xFF) as u8
    }

    pub fn count(self) -> u32 {
        ((self.0 >> 48) & 0xFFFF) as u32
    }

    pub fn with_count(self, count: u32) -> Self {
        Row::new(self.line(), self.source(), count)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A batch of rows with their bytes, detached from the source locks.
#[derive(Debug)]
pub struct Chunk {
    pub datas: Vec<Arc<Data>>,
    pub rows: Vec<Row>,
    /// `(start, len)` of each row's line within `datas[row.source()]`.
    pub spans: Vec<(u64, u32)>,
    /// Index of the first row within the base.
    pub first: usize,
}

impl Chunk {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn line(&self, i: usize) -> &[u8] {
        let (s, l) = self.spans[i];
        let data = &self.datas[self.rows[i].source() as usize];
        data.get(s as usize..(s + l as u64) as usize).unwrap_or(&[])
    }

    pub fn row(&self, i: usize) -> Row {
        self.rows[i]
    }
}

/// What a view is built over.
#[derive(Debug, Clone)]
pub enum Base {
    Single(Source),
    Merged(Arc<Merged>),
}

impl Base {
    pub fn sources(&self) -> Vec<Source> {
        match self {
            Base::Single(s) => vec![s.clone()],
            Base::Merged(m) => m.sources.clone(),
        }
    }

    pub fn source(&self, idx: u8) -> Option<Source> {
        match self {
            Base::Single(s) if idx == 0 => Some(s.clone()),
            Base::Single(_) => None,
            Base::Merged(m) => m.sources.get(idx as usize).cloned(),
        }
    }

    pub fn is_merged(&self) -> bool {
        matches!(self, Base::Merged(_))
    }

    pub fn name(&self) -> String {
        match self {
            Base::Single(s) => s.name().to_string(),
            Base::Merged(_) => "merged".to_string(),
        }
    }

    /// Number of base rows available right now.
    pub fn len(&self) -> usize {
        match self {
            Base::Single(s) => s.line_count(),
            Base::Merged(m) => m.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Changes whenever the underlying data is reset (rotation/truncation).
    pub fn generation(&self) -> u32 {
        match self {
            Base::Single(s) => s.generation(),
            Base::Merged(m) => m.generation(),
        }
    }

    /// Everything currently on disk has been processed.
    pub fn is_complete(&self) -> bool {
        match self {
            Base::Single(s) => s.is_complete(),
            Base::Merged(m) => m.is_complete(),
        }
    }

    /// Will more rows ever arrive?
    pub fn is_live(&self) -> bool {
        match self {
            Base::Single(s) => s.is_live(),
            Base::Merged(m) => m.sources.iter().any(|s| s.is_live()),
        }
    }

    pub fn formats(&self) -> Vec<Format> {
        self.sources().iter().map(|s| s.format()).collect()
    }

    pub fn add_waker(&self, w: Arc<dyn Fn() + Send + Sync>) {
        match self {
            Base::Single(s) => s.add_waker(w),
            Base::Merged(m) => m.add_waker(w),
        }
    }

    /// Fetch base rows `a..b` with their bytes.
    pub fn chunk(&self, a: usize, b: usize) -> Chunk {
        match self {
            Base::Single(s) => {
                let g = s.read();
                let b = b.min(g.line_count());
                let a = a.min(b);
                let mut spans = Vec::with_capacity(b - a);
                g.spans_into(a, b, &mut spans);
                let rows = (a..a + spans.len())
                    .map(|i| Row::new(i as u64, 0, 1))
                    .collect();
                Chunk {
                    datas: vec![g.data_arc()],
                    rows,
                    spans,
                    first: a,
                }
            }
            Base::Merged(m) => {
                let rows = m.rows_range(a, b);
                let mut c = self.fetch(&rows);
                c.first = a;
                c
            }
        }
    }

    /// Fetch arbitrary rows (e.g. a view's rows) with their bytes.
    pub fn fetch(&self, rows: &[Row]) -> Chunk {
        let sources = self.sources();
        let guards: Vec<_> = sources.iter().map(|s| s.read()).collect();
        let datas: Vec<Arc<Data>> = guards.iter().map(|g| g.data_arc()).collect();
        let mut spans = Vec::with_capacity(rows.len());
        let mut kept = Vec::with_capacity(rows.len());
        for r in rows {
            let Some(g) = guards.get(r.source() as usize) else {
                continue;
            };
            match g.span(r.line() as usize) {
                Some((s, e)) => {
                    spans.push((s, (e - s) as u32));
                    kept.push(*r);
                }
                None => {
                    spans.push((0, 0));
                    kept.push(*r);
                }
            }
        }
        Chunk {
            datas,
            rows: kept,
            spans,
            first: 0,
        }
    }

    /// Scan the last bytes of the (first) source for a timestamp, used to
    /// size the timeline before indexing completes.
    fn last_timestamp_hint(&self, default_year: i64) -> Option<i64> {
        let src = self.sources().into_iter().last()?;
        let g = src.read();
        let data = g.data();
        let tail_start = data.len().saturating_sub(64 * 1024);
        let tail = &data[tail_start..];
        let mut best = None;
        for line in tail.split(|&b| b == b'\n').rev().take(200) {
            if let Some(ts) = timestamp::find(line, default_year) {
                best = Some(ts.ms);
                break;
            }
        }
        best
    }

    fn first_timestamp_hint(&self, default_year: i64) -> Option<i64> {
        for src in self.sources() {
            let g = src.read();
            for i in 0..g.line_count().min(200) {
                if let Some(l) = g.line(i) {
                    if let Some(ts) = timestamp::find(l, default_year) {
                        return Some(ts.ms);
                    }
                }
            }
        }
        None
    }
}

/// How a view should be built.
#[derive(Debug, Clone, Default)]
pub struct ViewSpec {
    pub filter: Filter,
    pub collapse: bool,
}

/// Aggregate statistics for a view.
#[derive(Debug, Clone, Default)]
pub struct ViewStats {
    /// Lines kept (before collapsing).
    pub lines: u64,
    pub levels: [u64; Level::COUNT + 1],
    pub ts_min: Option<i64>,
    pub ts_max: Option<i64>,
}

struct Inner {
    base: Base,
    spec: ViewSpec,
    identity: bool,
    rows: RwLock<Vec<Row>>,
    hist: RwLock<Histogram>,
    stats: RwLock<ViewStats>,
    consumed: AtomicU64,
    complete: AtomicBool,
    finished: AtomicBool,
    cancel: AtomicBool,
    generation: AtomicU32,
    wakers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    default_year: i64,
}

/// A background-built, scrollable list of rows.
#[derive(Clone)]
pub struct View(Arc<Inner>);

impl Drop for Inner {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl View {
    /// Start building a view.  Returns immediately; rows stream in.
    pub fn start(base: Base, spec: ViewSpec) -> View {
        let identity = spec.filter.is_empty() && !spec.collapse && !base.is_merged();
        let default_year = timestamp::current_year();
        let hist = initial_histogram(&base, default_year);
        let inner = Arc::new(Inner {
            generation: AtomicU32::new(base.generation()),
            base,
            spec,
            identity,
            rows: RwLock::new(Vec::new()),
            hist: RwLock::new(hist),
            stats: RwLock::new(ViewStats::default()),
            consumed: AtomicU64::new(0),
            complete: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            wakers: Mutex::new(Vec::new()),
            default_year,
        });
        let view = View(inner);
        let v = view.clone();
        let _ = std::thread::Builder::new()
            .name("hem-view".into())
            .spawn(move || builder_main(v));
        view
    }

    pub fn base(&self) -> &Base {
        &self.0.base
    }

    pub fn spec(&self) -> &ViewSpec {
        &self.0.spec
    }

    pub fn is_identity(&self) -> bool {
        self.0.identity
    }

    pub fn len(&self) -> usize {
        if self.0.identity {
            self.0.base.len()
        } else {
            self.0.rows.read().len()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn row(&self, i: usize) -> Option<Row> {
        if self.0.identity {
            (i < self.0.base.len()).then(|| Row::new(i as u64, 0, 1))
        } else {
            self.0.rows.read().get(i).copied()
        }
    }

    /// Rows `a..b` (clamped).
    pub fn rows(&self, a: usize, b: usize) -> Vec<Row> {
        if self.0.identity {
            let n = self.0.base.len();
            let b = b.min(n);
            (a.min(b)..b).map(|i| Row::new(i as u64, 0, 1)).collect()
        } else {
            let rows = self.0.rows.read();
            let b = b.min(rows.len());
            rows[a.min(b)..b].to_vec()
        }
    }

    /// Fetch rows `a..b` with bytes.
    pub fn chunk(&self, a: usize, b: usize) -> Chunk {
        if self.0.identity {
            self.0.base.chunk(a, b)
        } else {
            let rows = self.rows(a, b);
            let mut c = self.0.base.fetch(&rows);
            c.first = a;
            c
        }
    }

    /// Position of the row showing `line` of `source` (or the row just
    /// after it, if it was filtered out / collapsed).
    pub fn row_for_line(&self, source: u8, line: u64) -> usize {
        if self.0.identity {
            return (line as usize).min(self.len().saturating_sub(1));
        }
        let rows = self.0.rows.read();
        if !self.0.base.is_merged() {
            let p = rows.partition_point(|r| r.line() < line);
            // If the line is inside a collapsed run, point at that run.
            if p > 0 {
                let prev = rows[p - 1];
                if prev.line() + prev.count() as u64 > line {
                    return p - 1;
                }
            }
            return p.min(rows.len().saturating_sub(1));
        }
        rows.iter()
            .position(|r| r.source() == source && r.line() >= line)
            .unwrap_or(rows.len().saturating_sub(1))
    }

    pub fn stats(&self) -> ViewStats {
        self.0.stats.read().clone()
    }

    /// True when every base row currently available has been processed.
    pub fn is_complete(&self) -> bool {
        self.0.complete.load(Ordering::Acquire)
    }

    /// True once the builder has exited (finite input fully processed).
    pub fn is_finished(&self) -> bool {
        self.0.finished.load(Ordering::Acquire)
    }

    pub fn generation(&self) -> u32 {
        self.0.generation.load(Ordering::Acquire)
    }

    pub fn add_waker(&self, w: Arc<dyn Fn() + Send + Sync>) {
        self.0.wakers.lock().push(w);
    }

    fn wake(&self) {
        for w in self.0.wakers.lock().iter() {
            w();
        }
    }
}

fn initial_histogram(base: &Base, default_year: i64) -> Histogram {
    match base.first_timestamp_hint(default_year) {
        Some(first) => {
            let last = base
                .last_timestamp_hint(default_year)
                .filter(|l| *l > first)
                .unwrap_or(first + 60_000);
            Histogram::new_time(first, last)
        }
        None => Histogram::new_position(base.len() as u64),
    }
}

struct ChunkOut {
    rows: Vec<Row>,
    /// `(level slot, key)` per kept line, in order.
    marks: Vec<(u8, i64)>,
    first_key: u64,
    last_key: u64,
    ts_min: Option<i64>,
    ts_max: Option<i64>,
    saw_ts: bool,
}

/// Per-chunk work: filter + collapse + metadata for the histogram.
fn process_chunk(v: &Inner, a: usize, b: usize, time_axis: bool) -> ChunkOut {
    let chunk = v.base.chunk(a, b);
    let formats = v.base.formats();
    let filter = &v.spec.filter;
    let collapse = v.spec.collapse;
    let needs_meta = true;
    let mut out = ChunkOut {
        rows: Vec::with_capacity(if filter.is_empty() { chunk.len() } else { 64 }),
        marks: Vec::with_capacity(chunk.len()),
        first_key: 0,
        last_key: 0,
        ts_min: None,
        ts_max: None,
        saw_ts: false,
    };
    // Carry-in: inherit level/ts from preceding lines.
    let (mut carry_level, mut carry_ts) = (None, None);
    if a > 0 {
        let back = v.base.chunk(a.saturating_sub(CARRY_LOOKBACK), a);
        for k in (0..back.len()).rev() {
            let fmt = formats
                .get(back.row(k).source() as usize)
                .copied()
                .unwrap_or_default();
            let m = parse_line(back.line(k), fmt, v.default_year);
            if carry_level.is_none() {
                carry_level = m.level;
            }
            if carry_ts.is_none() {
                carry_ts = m.ts;
            }
            if carry_level.is_some() && carry_ts.is_some() {
                break;
            }
        }
    }
    let mut prev_key: Option<u64> = None;
    for i in 0..chunk.len() {
        let line = chunk.line(i);
        let row = chunk.row(i);
        let fmt = formats
            .get(row.source() as usize)
            .copied()
            .unwrap_or_default();
        let meta = if needs_meta {
            parse_line(line, fmt, v.default_year)
        } else {
            LineMeta::default()
        };
        if meta.level.is_some() {
            carry_level = meta.level;
        }
        if meta.ts.is_some() {
            carry_ts = meta.ts;
        }
        let effective = LineMeta {
            level: carry_level,
            ts: carry_ts,
            ..meta
        };
        if !filter.matches(line, &effective, fmt) {
            continue;
        }
        let slot = meta.level.map(|l| l.index()).unwrap_or(Level::COUNT) as u8;
        let key = if time_axis {
            carry_ts.unwrap_or(i64::MIN)
        } else {
            (a + i) as i64
        };
        out.marks.push((slot, key));
        if let Some(t) = meta.ts {
            out.saw_ts = true;
            out.ts_min = Some(out.ts_min.map_or(t, |m: i64| m.min(t)));
            out.ts_max = Some(out.ts_max.map_or(t, |m: i64| m.max(t)));
        }
        if collapse {
            let k = shape_hash(line);
            if prev_key == Some(k) {
                if let Some(last) = out.rows.last_mut() {
                    if last.count() < MAX_COUNT {
                        *last = last.with_count(last.count() + 1);
                        continue;
                    }
                }
            }
            if out.rows.is_empty() {
                out.first_key = k;
            }
            prev_key = Some(k);
            out.last_key = k;
        }
        out.rows.push(row.with_count(1));
    }
    out
}

fn builder_main(view: View) {
    let v = &view.0;
    let (tx, rx) = mpsc::channel::<()>();
    {
        let tx = tx.clone();
        v.base.add_waker(Arc::new(move || {
            let _ = tx.send(());
        }));
    }
    let threads = crate::par::worker_threads();
    let mut seen_gen = v.base.generation();
    let mut consumed: usize = 0;
    let mut last_key: Option<u64> = None;
    let time_axis = v.hist.read().is_time();
    let mut last_wake = Instant::now();

    loop {
        if v.cancel.load(Ordering::Acquire) {
            break;
        }
        let gen = v.base.generation();
        if gen != seen_gen {
            seen_gen = gen;
            consumed = 0;
            last_key = None;
            v.rows.write().clear();
            v.hist.write().clear();
            *v.stats.write() = ViewStats::default();
            v.consumed.store(0, Ordering::Release);
            v.generation.store(gen, Ordering::Release);
            view.wake();
        }
        let avail = v.base.len();
        if avail > consumed {
            v.complete.store(false, Ordering::Release);
            let start = consumed;
            let n_chunks = (avail - start).div_ceil(CHUNK_ROWS);
            let collapse = v.spec.collapse;
            crate::par::ordered_scan(
                n_chunks,
                threads,
                &v.cancel,
                |i| {
                    let a = start + i * CHUNK_ROWS;
                    let b = (a + CHUNK_ROWS).min(avail);
                    process_chunk(v, a, b, time_axis)
                },
                |i, out: ChunkOut| {
                    let a = start + i * CHUNK_ROWS;
                    let b = (a + CHUNK_ROWS).min(avail);
                    if !v.identity {
                        let mut rows = v.rows.write();
                        let mut incoming = out.rows;
                        if collapse
                            && !incoming.is_empty()
                            && last_key.is_some()
                            && last_key == Some(out.first_key)
                        {
                            if let Some(last) = rows.last_mut() {
                                let add = incoming[0].count();
                                if last.count() + add <= MAX_COUNT {
                                    *last = last.with_count(last.count() + add);
                                    incoming.remove(0);
                                }
                            }
                        }
                        if collapse && !incoming.is_empty() {
                            last_key = Some(out.last_key);
                        } else if collapse && incoming.is_empty() && !out.marks.is_empty() {
                            // merged entirely into previous row; key unchanged
                        }
                        rows.extend_from_slice(&incoming);
                    }
                    {
                        let mut hist = v.hist.write();
                        for (slot, key) in &out.marks {
                            let level = Level::from_index(*slot as usize);
                            let key = if time_axis && *key == i64::MIN {
                                match hist.axis {
                                    crate::timeline::Axis::Time { start, .. } => start,
                                    _ => 0,
                                }
                            } else {
                                *key
                            };
                            hist.add(key, level);
                        }
                    }
                    {
                        let mut st = v.stats.write();
                        st.lines += out.marks.len() as u64;
                        for (slot, _) in &out.marks {
                            st.levels[*slot as usize] += 1;
                        }
                        if let Some(t) = out.ts_min {
                            st.ts_min = Some(st.ts_min.map_or(t, |m| m.min(t)));
                        }
                        if let Some(t) = out.ts_max {
                            st.ts_max = Some(st.ts_max.map_or(t, |m| m.max(t)));
                        }
                    }
                    consumed = b;
                    v.consumed.store(b as u64, Ordering::Release);
                    if last_wake.elapsed() > Duration::from_millis(40) {
                        view.wake();
                        last_wake = Instant::now();
                    }
                    !v.cancel.load(Ordering::Relaxed)
                },
            );
            if v.cancel.load(Ordering::Acquire) {
                break;
            }
            view.wake();
            continue;
        }
        if v.base.is_complete() {
            v.complete.store(true, Ordering::Release);
            view.wake();
            if !v.base.is_live() {
                break;
            }
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while rx.try_recv().is_ok() {}
    }
    v.finished.store(true, Ordering::Release);
    view.wake();
}

/// Wait (with timeout) until the view has finished building.  For tests
/// and plain mode.
pub fn wait_complete(view: &View, timeout: Duration) -> bool {
    let start = Instant::now();
    while !(view.is_finished() || (view.is_complete() && view.base().is_complete())) {
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

    fn mem(text: &str) -> Base {
        Base::Single(Source::from_bytes("t", text.as_bytes().to_vec()))
    }

    fn built(base: Base, spec: ViewSpec) -> View {
        let v = View::start(base, spec);
        assert!(wait_complete(&v, Duration::from_secs(5)));
        v
    }

    #[test]
    fn row_packing() {
        let r = Row::new(123_456_789_012, 7, 70_000);
        assert_eq!(r.line(), 123_456_789_012);
        assert_eq!(r.source(), 7);
        assert_eq!(r.count(), 0xFFFF);
        assert_eq!(r.with_count(3).count(), 3);
    }

    #[test]
    fn identity_view() {
        let v = built(mem("a\nb\nc\n"), ViewSpec::default());
        assert!(v.is_identity());
        assert_eq!(v.len(), 3);
        assert_eq!(v.row(1), Some(Row::new(1, 0, 1)));
        let st = v.stats();
        assert_eq!(st.lines, 3);
    }

}
