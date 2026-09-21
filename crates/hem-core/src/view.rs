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

/// Packed row reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Row(u64);

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

}

/// What a view is built over.
#[derive(Debug, Clone)]
pub enum Base {
    Single(Source),
    Merged(Arc<Merged>),
}

impl Base {
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

}

