//! Line-offset index and the scanner that builds it.
//!
//! The index stores the byte offset of the *start* of every line.  After
//! every `\n` at byte `p` we push `p + 1`, so a buffer `"a\nb\n"` yields
//! `[0, 2, 4]`.  The number of lines is therefore `offsets.len()` unless the
//! final offset equals the buffer length (the file ended with a newline), in
//! which case it is `offsets.len() - 1`.  That convention lets the index grow
//! incrementally without special-casing the trailing line.

use memchr::memchr_iter;

/// Growable line-start index.
#[derive(Debug, Default, Clone)]
pub struct LineIndex {
    /// Byte offset at which each line starts.  `offsets[0]` is always 0
    /// once at least one byte has been scanned.
    offsets: Vec<u64>,
    /// Number of bytes that have been scanned so far.
    scanned: u64,
}

impl LineIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes scanned so far.
    pub fn scanned(&self) -> u64 {
        self.scanned
    }

    /// Number of lines fully or partially scanned.
    pub fn line_count(&self) -> usize {
        match self.offsets.last() {
            None => 0,
            Some(&last) if last >= self.scanned => self.offsets.len() - 1,
            Some(_) => self.offsets.len(),
        }
    }

    /// Byte range `[start, end)` of line `i`, *including* its line terminator.
    pub fn raw_span(&self, i: usize) -> Option<(u64, u64)> {
        if i >= self.line_count() {
            return None;
        }
        let start = *self.offsets.get(i)?;
        let end = self.offsets.get(i + 1).copied().unwrap_or(self.scanned);
        Some((start, end))
    }

    /// Byte range `[start, end)` of line `i` *without* its `\n` / `\r\n`.
    pub fn span(&self, i: usize, data: &[u8]) -> Option<(u64, u64)> {
        let (start, mut end) = self.raw_span(i)?;
        if end > start && data.get(end as usize - 1) == Some(&b'\n') {
            end -= 1;
            if end > start && data.get(end as usize - 1) == Some(&b'\r') {
                end -= 1;
            }
        }
        Some((start, end))
    }

    /// Index of the line containing byte `offset` (clamped to the last line).
    pub fn line_at(&self, offset: u64) -> usize {
        if self.offsets.is_empty() {
            return 0;
        }
        let n = self.line_count();
        let i = match self.offsets.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        i.min(n.saturating_sub(1))
    }

    /// Scan `data[self.scanned..]` for newlines and append their offsets.
    /// `data` must be the complete buffer (not just the new tail) so that
    /// absolute offsets are correct.
    pub fn extend_from(&mut self, data: &[u8]) {
        let start = self.scanned as usize;
        if data.len() <= start {
            return;
        }
        self.scan_range(data, start, data.len());
    }

    /// Scan `data[from..to]` and append line starts.  Used for chunked,
    /// progress-reporting scans; ranges must be contiguous and ascending.
    pub fn scan_range(&mut self, data: &[u8], from: usize, to: usize) {
        debug_assert_eq!(from as u64, self.scanned);
        if self.offsets.is_empty() && to > from {
            self.offsets.push(0);
        }
        let slice = &data[from..to];
        self.offsets.reserve(slice.len() / 64);
        for p in memchr_iter(b'\n', slice) {
            self.offsets.push((from + p + 1) as u64);
        }
        self.scanned = to as u64;
    }

    pub fn offsets(&self) -> &[u64] {
        &self.offsets
    }

    /// Approximate heap memory used by the index, in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.offsets.capacity() * std::mem::size_of::<u64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(data: &[u8]) -> Vec<String> {
        let mut idx = LineIndex::new();
        idx.extend_from(data);
        (0..idx.line_count())
            .map(|i| {
                let (s, e) = idx.span(i, data).unwrap();
                String::from_utf8_lossy(&data[s as usize..e as usize]).into_owned()
            })
            .collect()
    }

    #[test]
    fn lf_lines() {
        assert_eq!(lines(b"a\nbb\nccc\n"), vec!["a", "bb", "ccc"]);
    }

    #[test]
    fn no_trailing_newline() {
        assert_eq!(lines(b"a\nbb\nccc"), vec!["a", "bb", "ccc"]);
    }

    #[test]
    fn crlf_lines() {
        assert_eq!(lines(b"a\r\nbb\r\nccc\r\n"), vec!["a", "bb", "ccc"]);
        assert_eq!(lines(b"a\r\nbb"), vec!["a", "bb"]);
    }

    #[test]
    fn blank_lines() {
        assert_eq!(lines(b"\n\na\n"), vec!["", "", "a"]);
        assert_eq!(lines(b"\n"), vec![""]);
    }

    #[test]
    fn giant_single_line() {
        let data = vec![b'x'; 10 * 1024 * 1024];
        let mut idx = LineIndex::new();
        idx.extend_from(&data);
        assert_eq!(idx.line_count(), 1);
        assert_eq!(idx.span(0, &data), Some((0, data.len() as u64)));
    }

    #[test]
    fn incremental_growth() {
        let mut data = b"a\nb".to_vec();
        let mut idx = LineIndex::new();
        idx.extend_from(&data);
        assert_eq!(idx.line_count(), 2);
        data.extend_from_slice(b"b\nc\n");
        idx.extend_from(&data);
        assert_eq!(idx.line_count(), 3);
        let (s, e) = idx.span(1, &data).unwrap();
        assert_eq!(&data[s as usize..e as usize], b"bb");
        data.extend_from_slice(b"d");
        idx.extend_from(&data);
        assert_eq!(idx.line_count(), 4);
    }

    #[test]
    fn line_at_offset() {
        let data = b"aa\nbb\ncc\n";
        let mut idx = LineIndex::new();
        idx.extend_from(data);
        assert_eq!(idx.line_at(0), 0);
        assert_eq!(idx.line_at(2), 0);
        assert_eq!(idx.line_at(3), 1);
        assert_eq!(idx.line_at(7), 2);
        assert_eq!(idx.line_at(100), 2);
    }
}
