//! Incremental background search over a view.
//!
//! Results are row indices, streamed in ascending order so that "next
//! match" is a binary search.  For identity views the scan runs the regex
//! over the raw byte range of each chunk (one `find_iter` per 32k lines
//! instead of one `is_match` per line), which is several times faster.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::filter::Matcher;
use crate::view::View;

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
    pub fn count(&self) -> usize {
        self.0.matches.read().len()
    }

    /// Whether row `row` is a match.
    pub fn contains(&self, row: usize) -> bool {
        self.0.matches.read().binary_search(&(row as u64)).is_ok()
    }

}

