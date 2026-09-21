//! Line-offset index and the scanner that builds it.
//!
//! The index stores the byte offset of the *start* of every line.  After
//! every `\n` at byte `p` we push `p + 1`, so a buffer `"a\nb\n"` yields
//! `[0, 2, 4]`.  The number of lines is therefore `offsets.len()` unless the
//! final offset equals the buffer length (the file ended with a newline), in
//! which case it is `offsets.len() - 1`.  That convention lets the index grow
//! incrementally without special-casing the trailing line.

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

}

