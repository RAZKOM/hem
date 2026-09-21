//! Sources: a memory-mapped file, stdin, a gzip stream or an in-memory
//! buffer, plus the background thread that indexes it and follows growth.
//!
//! All readers go through [`Source::read`], which returns a guard holding a
//! read lock on the current `(data, index)` pair.  The indexer thread only
//! takes the write lock for very short windows (to append a batch of
//! offsets or to swap in a bigger mapping), so the UI never stalls.

use std::fs::File;
use std::io::{Read, Write};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock, RwLockReadGuard};

use crate::error::{Error, Result};
use crate::format::Format;
use crate::index::LineIndex;

/// Raw bytes of a source.
pub enum Data {
    Empty,
    Mmap(memmap2::Mmap),
    Vec(Vec<u8>),
}

impl std::fmt::Debug for Data {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Data({} bytes)", self.len())
    }
}

impl Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Data::Empty => &[],
            Data::Mmap(m) => m,
            Data::Vec(v) => v,
        }
    }
}

/// What kind of thing a source is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    File,
    Stdin,
    Gzip,
    Memory,
}

/// Transient notices produced by the follower (shown in the status bar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Truncated,
    Rotated,
    Removed,
    Error(String),
}

/// Shared `(data, index)` state.
#[derive(Debug)]
pub struct State {
    pub data: Arc<Data>,
    pub index: LineIndex,
    /// Incremented every time the file is reset (truncation / rotation) so
    /// that cached per-line metadata can be invalidated.
    pub generation: u32,
}

#[derive(Debug, Default)]
struct Stats {
    indexed_bytes: AtomicU64,
    total_bytes: AtomicU64,
    line_count: AtomicUsize,
    complete: AtomicBool,
    generation: AtomicU32,
    eof: AtomicBool,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

struct Inner {
    name: String,
    path: Option<PathBuf>,
    kind: SourceKind,
    state: RwLock<State>,
    stats: Stats,
    notices: Mutex<Vec<Notice>>,
    stop: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
    poke: Mutex<Option<Sender<()>>>,
    format_override: Option<Format>,
    format: Mutex<Option<Format>>,
    follow: bool,
    _temp: Mutex<Option<tempfile::NamedTempFile>>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .finish()
    }
}

/// A log source.  Cheap to clone (it is an `Arc`).
#[derive(Debug, Clone)]
pub struct Source(Arc<Inner>);

/// Read guard over a source's data and index.
pub struct SourceGuard<'a> {
    guard: RwLockReadGuard<'a, State>,
}

impl std::fmt::Debug for SourceGuard<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SourceGuard({} lines)", self.line_count())
    }
}

impl SourceGuard<'_> {
    pub fn len(&self) -> u64 {
        self.guard.index.scanned()
    }

    pub fn line_count(&self) -> usize {
        self.guard.index.line_count()
    }

}

impl Source {
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// Acquire a read guard over the current data and index.
    pub fn read(&self) -> SourceGuard<'_> {
        SourceGuard {
            guard: self.0.state.read(),
        }
    }

    /// Lines indexed so far (lock-free).
    pub fn line_count(&self) -> usize {
        self.0.stats.line_count.load(Ordering::Acquire)
    }

}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

