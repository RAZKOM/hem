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
    pub fn data(&self) -> &[u8] {
        &self.guard.data
    }

    /// A reference-counted handle to the current data buffer, for background
    /// jobs that want to scan without holding the lock.
    pub fn data_arc(&self) -> Arc<Data> {
        Arc::clone(&self.guard.data)
    }

    pub fn index(&self) -> &LineIndex {
        &self.guard.index
    }

    pub fn generation(&self) -> u32 {
        self.guard.generation
    }

    pub fn len(&self) -> u64 {
        self.guard.index.scanned()
    }

    pub fn is_empty(&self) -> bool {
        self.line_count() == 0
    }

    pub fn line_count(&self) -> usize {
        self.guard.index.line_count()
    }

    /// Raw bytes of line `i` without its terminator.
    pub fn line(&self, i: usize) -> Option<&[u8]> {
        let data = &self.guard.data;
        let (s, e) = self.guard.index.span(i, data)?;
        data.get(s as usize..e as usize)
    }

    /// Byte span of line `i` (without terminator).
    pub fn span(&self, i: usize) -> Option<(u64, u64)> {
        self.guard.index.span(i, &self.guard.data)
    }

    pub fn line_at_offset(&self, offset: u64) -> usize {
        self.guard.index.line_at(offset)
    }

    /// Push `(start, len)` spans (without terminators) for lines `a..b`.
    pub fn spans_into(&self, a: usize, b: usize, out: &mut Vec<(u64, u32)>) {
        let data = &self.guard.data;
        let idx = &self.guard.index;
        let b = b.min(idx.line_count());
        out.reserve(b.saturating_sub(a));
        for i in a..b {
            if let Some((s, e)) = idx.span(i, data) {
                out.push((s, (e - s) as u32));
            }
        }
    }
}

impl Source {
    /// Append bytes to an in-memory source (tests / pasted input).
    pub fn append_bytes(&self, bytes: &[u8]) {
        let mut st = self.0.state.write();
        let mut v = match Arc::try_unwrap(std::mem::replace(&mut st.data, Arc::new(Data::Empty))) {
            Ok(Data::Vec(v)) => v,
            Ok(other) => other.to_vec(),
            Err(arc) => arc.to_vec(),
        };
        v.extend_from_slice(bytes);
        st.index.extend_from(&v);
        let len = v.len() as u64;
        st.data = Arc::new(Data::Vec(v));
        self.0.stats.indexed_bytes.store(len, Ordering::Release);
        self.0.stats.total_bytes.store(len, Ordering::Release);
        self.0
            .stats
            .line_count
            .store(st.index.line_count(), Ordering::Release);
        drop(st);
        self.0.wake();
    }

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

    /// Bytes indexed so far (lock-free).
    pub fn indexed_bytes(&self) -> u64 {
        self.0.stats.indexed_bytes.load(Ordering::Acquire)
    }

    /// Size of the underlying file as last observed (lock-free).
    pub fn total_bytes(&self) -> u64 {
        self.0.stats.total_bytes.load(Ordering::Acquire)
    }

    /// True once everything currently in the file has been indexed.
    pub fn is_complete(&self) -> bool {
        self.0.stats.complete.load(Ordering::Acquire)
    }

    /// True once a finite input (stdin, gzip, memory) has been fully read.
    pub fn is_eof(&self) -> bool {
        self.0.stats.eof.load(Ordering::Acquire)
    }

    pub fn generation(&self) -> u32 {
        self.0.stats.generation.load(Ordering::Acquire)
    }

    /// Install a callback invoked whenever new data becomes available.
    pub fn add_waker(&self, waker: Waker) {
        self.0.wakers.lock().push(waker);
    }

    /// Does this source keep growing (file being followed / stdin / gzip
    /// still decompressing)?  Finite inputs report `false` once at EOF.
    pub fn is_live(&self) -> bool {
        match self.0.kind {
            SourceKind::File => self.0.follow,
            SourceKind::Memory => !self.0.stop.load(Ordering::Acquire),
            SourceKind::Stdin | SourceKind::Gzip => !self.is_eof(),
        }
    }

    /// Ask the follower to re-check the file immediately.
    pub fn poke(&self) {
        self.0.poke();
    }

    /// Stop background threads.  Called automatically on drop of the last
    /// handle, but can be called earlier.
    pub fn stop(&self) {
        self.0.stop.store(true, Ordering::Release);
        self.0.poke();
    }

    /// Override the format at runtime.
    pub fn set_format(&self, format: Option<Format>) {
        *self.0.format.lock() = format;
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Inner {
    fn wake(&self) {
        for w in self.wakers.lock().iter() {
            w();
        }
    }

    fn poke(&self) {
        if let Some(tx) = self.poke.lock().as_ref() {
            let _ = tx.send(());
        }
    }
}

