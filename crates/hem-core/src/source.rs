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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock, RwLockReadGuard};

use crate::error::{Error, Result};
use crate::format::Format;
use crate::index::{scan_offsets, LineIndex};

/// Chunk size for background scanning; each chunk becomes one batch of
/// offsets appended under the write lock.
const SCAN_CHUNK: usize = 32 * 1024 * 1024;

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

/// Options controlling how a source is opened and followed.
#[derive(Debug, Clone)]
pub struct SourceOptions {
    /// Keep watching the file for growth / rotation after the initial index.
    pub follow: bool,
    /// Polling interval used as a fallback when filesystem events are
    /// unreliable (and as the heartbeat for stdin / gzip temp files).
    pub poll_interval: Duration,
    /// Force a line format instead of auto-detecting.
    pub format: Option<Format>,
}

impl Default for SourceOptions {
    fn default() -> Self {
        Self {
            follow: true,
            poll_interval: Duration::from_millis(250),
            format: None,
        }
    }
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
    /// Open a file (or a `.gz` file, which is transparently decompressed
    /// into a temp file in the background).
    pub fn open(path: impl AsRef<Path>, opts: SourceOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let meta = std::fs::metadata(&path).map_err(|e| Error::Open {
            path: path.clone(),
            source: e,
        })?;
        if meta.is_dir() {
            return Err(Error::IsDirectory { path });
        }
        let is_gz = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("gz"))
            .unwrap_or(false);
        if is_gz {
            return Self::open_gzip(path, opts);
        }
        let name = display_name(&path);
        let inner = Arc::new(Inner::new(
            name,
            Some(path.clone()),
            SourceKind::File,
            &opts,
        ));
        let src = Source(inner);
        src.spawn_follower(path, opts, None);
        Ok(src)
    }

    fn open_gzip(path: PathBuf, opts: SourceOptions) -> Result<Self> {
        let file = File::open(&path).map_err(|e| Error::Open {
            path: path.clone(),
            source: e,
        })?;
        let temp = tempfile::Builder::new()
            .prefix("hem-gz-")
            .tempfile()
            .map_err(Error::Io)?;
        let temp_path = temp.path().to_path_buf();
        let name = display_name(&path);
        let inner = Arc::new(Inner::new(name, Some(path), SourceKind::Gzip, &opts));
        *inner._temp.lock() = Some(temp);
        let src = Source(inner);
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let writer_path = temp_path.clone();
        let src2 = src.clone();
        std::thread::Builder::new()
            .name("hem-gunzip".into())
            .spawn(move || {
                let res = (|| -> std::io::Result<()> {
                    let mut out = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&writer_path)?;
                    let mut dec = flate2::read::MultiGzDecoder::new(std::io::BufReader::new(file));
                    let mut buf = vec![0u8; 1 << 20];
                    loop {
                        if src2.0.stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let n = dec.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        out.write_all(&buf[..n])?;
                    }
                    out.flush()
                })();
                if let Err(e) = res {
                    src2.0.push_notice(Notice::Error(format!("gunzip: {e}")));
                }
                src2.0.poke();
                let _ = done_tx.send(());
            })
            .map_err(Error::Io)?;
        src.spawn_follower(temp_path, opts, Some(done_rx));
        Ok(src)
    }

    /// Read stdin into a temp file and follow it.
    pub fn stdin(opts: SourceOptions) -> Result<Self> {
        let temp = tempfile::Builder::new()
            .prefix("hem-stdin-")
            .tempfile()
            .map_err(Error::Io)?;
        let temp_path = temp.path().to_path_buf();
        let inner = Arc::new(Inner::new(
            "stdin".to_string(),
            None,
            SourceKind::Stdin,
            &opts,
        ));
        *inner._temp.lock() = Some(temp);
        let src = Source(inner);
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let writer_path = temp_path.clone();
        let src2 = src.clone();
        std::thread::Builder::new()
            .name("hem-stdin".into())
            .spawn(move || {
                let res = (|| -> std::io::Result<()> {
                    let mut out = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&writer_path)?;
                    let stdin = std::io::stdin();
                    let mut lock = stdin.lock();
                    let mut buf = vec![0u8; 256 * 1024];
                    loop {
                        if src2.0.stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let n = lock.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        out.write_all(&buf[..n])?;
                        out.flush()?;
                        src2.0.poke();
                    }
                    Ok(())
                })();
                if let Err(e) = res {
                    src2.0.push_notice(Notice::Error(format!("stdin: {e}")));
                }
                src2.0.poke();
                let _ = done_tx.send(());
            })
            .map_err(Error::Io)?;
        src.spawn_follower(temp_path, opts, Some(done_rx));
        Ok(src)
    }

    /// An in-memory source, indexed synchronously.  Handy for tests and for
    /// small pasted inputs.
    pub fn from_bytes(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        let opts = SourceOptions::default();
        let inner = Inner::new(name.into(), None, SourceKind::Memory, &opts);
        {
            let mut st = inner.state.write();
            let mut index = LineIndex::new();
            index.extend_from(&bytes);
            let len = bytes.len() as u64;
            st.data = Arc::new(Data::Vec(bytes));
            st.index = index;
            inner.stats.indexed_bytes.store(len, Ordering::Release);
            inner.stats.total_bytes.store(len, Ordering::Release);
            inner
                .stats
                .line_count
                .store(st.index.line_count(), Ordering::Release);
            inner.stats.complete.store(true, Ordering::Release);
            inner.stats.eof.store(true, Ordering::Release);
        }
        Source(Arc::new(inner))
    }

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

    pub fn path(&self) -> Option<&Path> {
        self.0.path.as_deref()
    }

    pub fn kind(&self) -> SourceKind {
        self.0.kind
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

    /// Drain pending notices.
    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.0.notices.lock())
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

    /// The detected (or overridden) line format.  Detection samples the
    /// first lines once at least a handful are available and then caches
    /// the result.
    pub fn format(&self) -> Format {
        if let Some(f) = self.0.format_override {
            return f;
        }
        if let Some(f) = *self.0.format.lock() {
            return f;
        }
        let guard = self.read();
        let n = guard.line_count();
        if n == 0 {
            return Format::Plain;
        }
        let sample: Vec<&[u8]> = (0..n.min(200)).filter_map(|i| guard.line(i)).collect();
        let f = Format::detect(&sample);
        // Only cache once we have a decent sample or the input is finished.
        if n >= 20 || self.is_complete() {
            *self.0.format.lock() = Some(f);
        }
        f
    }

    /// Override the format at runtime.
    pub fn set_format(&self, format: Option<Format>) {
        *self.0.format.lock() = format;
    }

    fn spawn_follower(&self, path: PathBuf, opts: SourceOptions, eof_rx: Option<Receiver<()>>) {
        let (tx, rx) = mpsc::channel::<()>();
        *self.0.poke.lock() = Some(tx.clone());
        let inner = Arc::clone(&self.0);
        let _ = std::thread::Builder::new()
            .name(format!("hem-index-{}", inner.name))
            .spawn(move || follower_main(inner, path, opts, tx, rx, eof_rx));
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Inner {
    fn new(name: String, path: Option<PathBuf>, kind: SourceKind, opts: &SourceOptions) -> Self {
        Self {
            name,
            path,
            kind,
            state: RwLock::new(State {
                data: Arc::new(Data::Empty),
                index: LineIndex::new(),
                generation: 0,
            }),
            stats: Stats::default(),
            notices: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            wakers: Mutex::new(Vec::new()),
            poke: Mutex::new(None),
            format_override: opts.format,
            format: Mutex::new(None),
            follow: opts.follow,
            _temp: Mutex::new(None),
        }
    }

    fn push_notice(&self, n: Notice) {
        self.notices.lock().push(n);
        self.wake();
    }

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

    fn reset(&self, notice: Notice) {
        {
            let mut st = self.state.write();
            st.data = Arc::new(Data::Empty);
            st.index.clear();
            st.generation = st.generation.wrapping_add(1);
            self.stats
                .generation
                .store(st.generation, Ordering::Release);
        }
        self.stats.indexed_bytes.store(0, Ordering::Release);
        self.stats.total_bytes.store(0, Ordering::Release);
        self.stats.line_count.store(0, Ordering::Release);
        self.stats.complete.store(false, Ordering::Release);
        *self.format.lock() = None;
        self.push_notice(notice);
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Identity of a file (device+inode on Unix, volume+file index on
/// Windows), used to detect rotation: the path now refers to a different
/// file than the one we have open.
fn identity_of_path(path: &Path) -> Option<same_file::Handle> {
    same_file::Handle::from_path(path).ok()
}

/// Map the whole file.  Returns `Data::Empty` for zero-length files (an
/// empty mapping is an error on most platforms).
fn map_file(file: &File, len: u64) -> std::io::Result<Data> {
    if len == 0 {
        return Ok(Data::Empty);
    }
    // SAFETY: the mapping is read-only.  hem never writes to the files it
    // opens.  If another process truncates the file while it is mapped the
    // OS may fault on access; this is documented in DECISIONS.md and is the
    // standard trade-off every mmap-based viewer makes.
    let map = unsafe { memmap2::MmapOptions::new().len(len as usize).map(file)? };
    #[cfg(unix)]
    {
        let _ = map.advise(memmap2::Advice::Sequential);
    }
    Ok(Data::Mmap(map))
}

fn start_watcher(path: &Path, tx: Sender<()>) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty())?;
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })
    .ok()?;
    watcher
        .watch(dir, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

/// The follower / indexer thread.
fn follower_main(
    inner: Arc<Inner>,
    path: PathBuf,
    opts: SourceOptions,
    tx: Sender<()>,
    rx: Receiver<()>,
    eof_rx: Option<Receiver<()>>,
) {
    let _watcher = if opts.follow {
        start_watcher(&path, tx.clone())
    } else {
        None
    };
    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            inner.push_notice(Notice::Error(format!("open {}: {e}", path.display())));
            inner.stats.complete.store(true, Ordering::Release);
            return;
        }
    };
    let mut identity = identity_of_path(&path);
    let mut mapped_len: u64 = 0;
    let mut missing_reported = false;
    let mut shrunk = false;
    let mut last_progress = Instant::now();
    let finite = eof_rx.is_some();
    let mut writer_done = false;

    loop {
        if inner.stop.load(Ordering::Acquire) {
            break;
        }
        if let Some(rx) = &eof_rx {
            if !writer_done && rx.try_recv().is_ok() {
                writer_done = true;
            }
        }

        // Rotation / removal detection (real files only).
        if inner.kind == SourceKind::File {
            match identity_of_path(&path) {
                Some(now) => {
                    if identity.as_ref() != Some(&now) {
                        if let Ok(nf) = File::open(&path) {
                            file = nf;
                            identity = Some(now);
                            mapped_len = 0;
                            inner.reset(Notice::Rotated);
                            continue;
                        }
                    }
                    missing_reported = false;
                }
                None => {
                    if !missing_reported {
                        inner.push_notice(Notice::Removed);
                        missing_reported = true;
                    }
                }
            }
        }

        let len = match file.metadata() {
            Ok(m) => m.len(),
            Err(e) => {
                inner.push_notice(Notice::Error(format!("stat: {e}")));
                inner.stats.complete.store(true, Ordering::Release);
                break;
            }
        };
        inner.stats.total_bytes.store(len, Ordering::Release);
        let scanned = inner.state.read().index.scanned();

        if len < scanned {
            mapped_len = 0;
            inner.reset(Notice::Truncated);
            continue;
        }

        if len > scanned {
            inner.stats.complete.store(false, Ordering::Release);
            if len > mapped_len {
                match map_file(&file, len) {
                    Ok(data) => {
                        let mut st = inner.state.write();
                        st.data = Arc::new(data);
                        mapped_len = len;
                    }
                    Err(e) => {
                        inner.push_notice(Notice::Error(format!("mmap: {e}")));
                        inner.stats.complete.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            let data = inner.state.read().data.clone();
            let mut from = scanned as usize;
            let to = mapped_len as usize;
            while from < to {
                if inner.stop.load(Ordering::Acquire) {
                    return;
                }
                let end = (from + SCAN_CHUNK).min(to);
                let offsets = scan_offsets(&data[from..end], from as u64);
                {
                    let mut st = inner.state.write();
                    st.index.append_offsets(&offsets, end as u64);
                    inner
                        .stats
                        .line_count
                        .store(st.index.line_count(), Ordering::Release);
                }
                inner
                    .stats
                    .indexed_bytes
                    .store(end as u64, Ordering::Release);
                from = end;
                if last_progress.elapsed() > Duration::from_millis(30) {
                    inner.wake();
                    last_progress = Instant::now();
                }
            }
            inner.wake();
            continue;
        }

        if finite && !writer_done {
            // Still being written (stdin / gunzip): not complete yet.
            match rx.recv_timeout(opts.poll_interval) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            while rx.try_recv().is_ok() {}
            continue;
        }
        if !shrunk {
            inner.state.write().index.shrink_to_fit();
            shrunk = true;
        }
        inner.stats.complete.store(true, Ordering::Release);
        inner.wake();

        if finite && writer_done {
            inner.stats.eof.store(true, Ordering::Release);
            break;
        }
        if !opts.follow && !finite {
            break;
        }

        match rx.recv_timeout(opts.poll_interval) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Coalesce bursts of events.
        while rx.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wait_for(src: &Source, pred: impl Fn(&Source) -> bool) {
        let start = Instant::now();
        while !pred(src) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out waiting"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn memory_source_lines() {
        let src = Source::from_bytes("mem", b"one\ntwo\r\nthree".to_vec());
        let g = src.read();
        assert_eq!(g.line_count(), 3);
        assert_eq!(g.line(0), Some(&b"one"[..]));
        assert_eq!(g.line(1), Some(&b"two"[..]));
        assert_eq!(g.line(2), Some(&b"three"[..]));
        assert_eq!(g.line(3), None);
    }

    #[test]
    fn file_source_indexes_and_follows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        {
            let mut f = File::create(&path).unwrap();
            writeln!(f, "first").unwrap();
            writeln!(f, "second").unwrap();
        }
        let opts = SourceOptions {
            poll_interval: Duration::from_millis(30),
            ..Default::default()
        };
        let src = Source::open(&path, opts).unwrap();
        wait_for(&src, |s| s.is_complete() && s.line_count() == 2);
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "third").unwrap();
        }
        src.poke();
        wait_for(&src, |s| s.line_count() == 3);
        assert_eq!(src.read().line(2), Some(&b"third"[..]));
        src.stop();
    }

    /// Windows refuses to truncate a file that has a user-mapped section
    /// open, so in-place truncation can only be exercised on Unix.
    #[test]
    #[cfg(unix)]
    fn truncation_resets_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.log");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let opts = SourceOptions {
            poll_interval: Duration::from_millis(30),
            ..Default::default()
        };
        let src = Source::open(&path, opts).unwrap();
        wait_for(&src, |s| s.is_complete() && s.line_count() == 3);
        let gen0 = src.generation();
        // Truncate by rewriting a shorter file in place.
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            f.write_all(b"x\n").unwrap();
        }
        src.poke();
        wait_for(&src, |s| s.generation() != gen0 && s.line_count() == 1);
        assert_eq!(src.read().line(0), Some(&b"x"[..]));
        assert!(src.take_notices().contains(&Notice::Truncated));
        src.stop();
    }

    #[test]
    fn rotation_reopens_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.log");
        std::fs::write(&path, "old1\nold2\n").unwrap();
        let opts = SourceOptions {
            poll_interval: Duration::from_millis(30),
            ..Default::default()
        };
        let src = Source::open(&path, opts).unwrap();
        wait_for(&src, |s| s.is_complete() && s.line_count() == 2);
        let gen0 = src.generation();
        // Rotate: rename the old file away and create a fresh one.
        std::fs::rename(&path, dir.path().join("r.log.1")).unwrap();
        std::fs::write(&path, "new1\n").unwrap();
        src.poke();
        wait_for(&src, |s| s.generation() != gen0 && s.line_count() == 1);
        assert_eq!(src.read().line(0), Some(&b"new1"[..]));
        let notices = src.take_notices();
        assert!(notices.contains(&Notice::Rotated), "{notices:?}");
        src.stop();
    }

    #[test]
    fn empty_file_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.log");
        std::fs::write(&path, "").unwrap();
        let src = Source::open(&path, SourceOptions::default()).unwrap();
        wait_for(&src, |s| s.is_complete());
        assert_eq!(src.line_count(), 0);
        assert!(src.read().line(0).is_none());
        src.stop();
    }
}
