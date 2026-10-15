//! Positional reads through a small page cache.
//!
//! This is how hem reads real files by default instead of memory-mapping
//! them. A mapping keeps a section object alive, and on Windows that stops
//! any other process from truncating the file (`copytruncate` rotation
//! fails with "user-mapped section open"). On Unix, a file truncated under a
//! mapping can make the reader fault with SIGBUS. Plain reads have neither
//! problem: the writer can truncate, rename or delete the file at any time,
//! and reads past the new end simply come back short.
//!
//! Reads go through `pread` / `seek_read`, so they never move a shared file
//! cursor and can run from several threads at once. Small reads (drawing a
//! screen, fetching scattered filtered rows) are served from 64 KiB pages
//! cached in an LRU of bounded size. Large sequential reads (indexing,
//! building views, searching) bypass the cache.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

/// Page size for cached reads.
pub const PAGE: u64 = 64 * 1024;
/// Pages kept in the cache (256 × 64 KiB = 16 MiB per open file).
const CACHE_PAGES: usize = 256;
/// Pages cached across all open files (4096 × 64 KiB = 256 MiB).  With
/// hundreds of files open, per-file caps alone would let the caches grow
/// to gigabytes.
const GLOBAL_PAGES: usize = 4096;
/// A file may always cache this many pages, whatever the global total, so
/// a newly busy file is never starved by idle ones.
const MIN_PAGES: usize = 16;
/// Pages currently cached by all files.
static CACHED: AtomicUsize = AtomicUsize::new(0);
/// Reads at least this large go straight to the file.
const DIRECT_READ: u64 = 256 * 1024;
/// Pages read at once when access is sequential.
const READ_AHEAD: u64 = 4;

/// Read exactly `buf.len()` bytes at `offset`, stopping early only at end
/// of file. Returns how many bytes were filled.
pub fn read_full_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match pread(file, &mut buf[filled..], offset + filled as u64) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(unix)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

#[cfg(windows)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

#[cfg(not(any(unix, windows)))]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    // Fallback for other targets: clone the handle so the shared cursor of
    // the original is untouched.
    let mut f = file.try_clone()?;
    f.seek(SeekFrom::Start(offset))?;
    f.read(buf)
}

#[derive(Default)]
struct PageCache {
    pages: HashMap<u64, (Arc<[u8]>, u64)>,
    clock: u64,
}

impl PageCache {
    fn get(&mut self, page: u64) -> Option<Arc<[u8]>> {
        self.clock += 1;
        let clock = self.clock;
        self.pages.get_mut(&page).map(|(bytes, stamp)| {
            *stamp = clock;
            Arc::clone(bytes)
        })
    }

    /// Insert a page, evicting this file's least recently used page when
    /// the file is at its cap, or when the global budget is spent and the
    /// file holds more than its minimum.
    fn insert(&mut self, page: u64, bytes: Arc<[u8]>) {
        if self.pages.contains_key(&page) {
            self.clock += 1;
            self.pages.insert(page, (bytes, self.clock));
            return;
        }
        let full = self.pages.len() >= CACHE_PAGES
            || (self.pages.len() >= MIN_PAGES && CACHED.load(Ordering::Relaxed) >= GLOBAL_PAGES);
        if full {
            if let Some(oldest) = self
                .pages
                .iter()
                .min_by_key(|(_, (_, stamp))| *stamp)
                .map(|(k, _)| *k)
            {
                self.pages.remove(&oldest);
                CACHED.fetch_sub(1, Ordering::Relaxed);
            }
        }
        self.clock += 1;
        self.pages.insert(page, (bytes, self.clock));
        CACHED.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for PageCache {
    fn drop(&mut self) {
        CACHED.fetch_sub(self.pages.len(), Ordering::Relaxed);
    }
}

/// A read-only view of the first `len` bytes of a file.
///
/// Cheap to clone. When the file grows, [`Paged::with_len`] makes a longer
/// view that shares the same handle and cache: cached pages are always
/// complete 64 KiB pages, which appends never change.
#[derive(Clone)]
pub struct Paged {
    file: Arc<File>,
    len: u64,
    cache: Arc<Mutex<PageCache>>,
}

impl std::fmt::Debug for Paged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Paged({} bytes)", self.len)
    }
}

impl Paged {
    /// A view of `len` bytes of `file` with a fresh, empty cache.
    pub fn new(file: Arc<File>, len: u64) -> Paged {
        Paged {
            file,
            len,
            cache: Arc::new(Mutex::new(PageCache::default())),
        }
    }

    /// The same file and cache, viewed up to `len` bytes.
    pub fn with_len(&self, len: u64) -> Paged {
        Paged {
            file: Arc::clone(&self.file),
            len,
            cache: Arc::clone(&self.cache),
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Read `[start, end)`, clamped to the view. Always returns exactly
    /// `end - start` bytes (after clamping): if the file was truncated by
    /// another process, the missing tail is zero-filled. The follower
    /// notices the truncation on its next poll and re-indexes.
    pub fn read(&self, start: u64, end: u64) -> Vec<u8> {
        let end = end.min(self.len);
        if start >= end {
            return Vec::new();
        }
        let n = (end - start) as usize;
        if end - start >= DIRECT_READ {
            let mut out = vec![0u8; n];
            let _ = read_full_at(&self.file, &mut out, start);
            return out;
        }
        let mut out = Vec::with_capacity(n);
        let mut pos = start;
        while pos < end {
            let page_no = pos / PAGE;
            let page_start = page_no * PAGE;
            let page_end = (page_start + PAGE).min(self.len);
            let page = self.page(page_no, page_start, page_end);
            let off = (pos - page_start) as usize;
            let want = (end.min(page_end) - pos) as usize;
            let have = page.len().saturating_sub(off).min(want);
            out.extend_from_slice(&page[off..off + have]);
            out.resize(out.len() + (want - have), 0);
            pos += want as u64;
        }
        out
    }

    /// Fill `buf` from `offset` without touching the cache (used for big
    /// sequential scans). Returns the number of bytes actually read.
    pub fn read_into(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let max = self.len.saturating_sub(offset).min(buf.len() as u64) as usize;
        read_full_at(&self.file, &mut buf[..max], offset)
    }

    fn page(&self, page_no: u64, page_start: u64, page_end: u64) -> Arc<[u8]> {
        let full = page_end - page_start == PAGE;
        let mut sequential = false;
        if full {
            let mut cache = self.cache.lock();
            if let Some(p) = cache.get(page_no) {
                return p;
            }
            sequential = page_no > 0 && cache.pages.contains_key(&(page_no - 1));
        }
        if sequential {
            return self.read_ahead(page_no, page_start);
        }
        let mut buf = vec![0u8; (page_end - page_start) as usize];
        let got = read_full_at(&self.file, &mut buf, page_start).unwrap_or(0);
        buf.truncate(got);
        let page: Arc<[u8]> = buf.into();
        // Only complete pages are cached: the partial last page can still
        // grow, and a short read means the file shrank under us.
        if full && got as u64 == PAGE {
            self.cache.lock().insert(page_no, Arc::clone(&page));
        }
        page
    }

    /// Sequential access (the previous page is cached): read the next
    /// `READ_AHEAD` complete pages with one call.  Building a merged view
    /// walks every source a few lines at a time, and one 64 KiB read per
    /// page made that several times slower than a mapping.
    fn read_ahead(&self, page_no: u64, page_start: u64) -> Arc<[u8]> {
        let pages = ((self.len - page_start) / PAGE).clamp(1, READ_AHEAD);
        let mut buf = vec![0u8; (pages * PAGE) as usize];
        let got = read_full_at(&self.file, &mut buf, page_start).unwrap_or(0) as u64;
        let mut first: Option<Arc<[u8]>> = None;
        let mut cache = self.cache.lock();
        for i in 0..pages {
            let (a, b) = (i * PAGE, (i + 1) * PAGE);
            if b > got {
                break;
            }
            let page: Arc<[u8]> = buf[a as usize..b as usize].into();
            cache.insert(page_no + i, Arc::clone(&page));
            first.get_or_insert(page);
        }
        drop(cache);
        // A short read means the file shrank under us: return what there is.
        first.unwrap_or_else(|| buf[..(got.min(PAGE)) as usize].into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn file_with(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.bin");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn reads_match_file_contents_across_pages() {
        let data = pattern(3 * PAGE as usize + 123);
        let (_d, path) = file_with(&data);
        let p = Paged::new(Arc::new(File::open(&path).unwrap()), data.len() as u64);
        for (s, e) in [
            (0u64, 10u64),
            (PAGE - 5, PAGE + 5),
            (PAGE * 2 - 1, PAGE * 3 + 100),
            (0, data.len() as u64),
            (data.len() as u64 - 3, data.len() as u64 + 50),
            (5, 5),
        ] {
            let got = p.read(s, e);
            let e = e.min(data.len() as u64);
            assert_eq!(got, data[s as usize..e.max(s) as usize], "range {s}..{e}");
        }
        // Second pass is served from the cache and must be identical.
        assert_eq!(
            p.read(PAGE - 5, PAGE + 5),
            data[PAGE as usize - 5..PAGE as usize + 5]
        );
    }

    #[test]
    fn growth_shares_cache_and_sees_new_bytes() {
        let (_d, path) = file_with(b"hello\n");
        let file = Arc::new(File::open(&path).unwrap());
        let p = Paged::new(Arc::clone(&file), 6);
        assert_eq!(p.read(0, 6), b"hello\n");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"world\n")
            .unwrap();
        let p2 = p.with_len(12);
        assert_eq!(p2.read(0, 12), b"hello\nworld\n");
        assert_eq!(p.read(0, 100), b"hello\n", "old view stays clamped");
    }

    #[test]
    fn truncated_file_reads_zero_filled() {
        let data = pattern(2 * PAGE as usize);
        let (_d, path) = file_with(&data);
        let p = Paged::new(Arc::new(File::open(&path).unwrap()), data.len() as u64);
        // Another process truncates the file while we hold it open. This is
        // exactly what a memory mapping would forbid on Windows.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(10)
            .unwrap();
        let got = p.read(5, PAGE + 5);
        assert_eq!(got.len(), PAGE as usize);
        assert_eq!(&got[..5], &data[5..10]);
        assert!(got[5..].iter().all(|b| *b == 0));
        let big = p.read(0, data.len() as u64);
        assert_eq!(big.len(), data.len());
    }

    #[test]
    fn cache_is_bounded() {
        let data = pattern((CACHE_PAGES + 20) * PAGE as usize);
        let (_d, path) = file_with(&data);
        let p = Paged::new(Arc::new(File::open(&path).unwrap()), data.len() as u64);
        for page in 0..(CACHE_PAGES as u64 + 20) {
            let s = page * PAGE + 7;
            assert_eq!(p.read(s, s + 3), data[s as usize..s as usize + 3]);
        }
        assert!(p.cache.lock().pages.len() <= CACHE_PAGES);
    }
}
