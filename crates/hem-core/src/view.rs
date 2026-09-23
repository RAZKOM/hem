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
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::filter::Filter;
use crate::level::Level;
use crate::merge::Merged;
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

