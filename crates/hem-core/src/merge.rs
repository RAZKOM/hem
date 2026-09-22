//! Multi-source merge: interleave several sources by timestamp into one
//! ordered list of rows.  Lines without a timestamp inherit the previous
//! line's timestamp in the same source, so multi-line entries (stack
//! traces) stay together.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::source::Source;
use crate::view::Row;

/// Maximum number of sources in a merge (rows carry an 8-bit source id).
pub const MAX_SOURCES: usize = 256;

/// A timestamp-merged set of sources.
pub struct Merged {
    pub sources: Vec<Source>,
    rows: RwLock<Vec<Row>>,
    generation: AtomicU32,
    complete: AtomicBool,
    cancel: AtomicBool,
    wakers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
}

impl std::fmt::Debug for Merged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Merged")
            .field("sources", &self.sources.len())
            .field("rows", &self.len())
            .finish()
    }
}

impl Drop for Merged {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Merged {
    pub fn len(&self) -> usize {
        self.rows.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn rows_range(&self, a: usize, b: usize) -> Vec<Row> {
        let rows = self.rows.read();
        let b = b.min(rows.len());
        rows[a.min(b)..b].to_vec()
    }

    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub fn add_waker(&self, w: Arc<dyn Fn() + Send + Sync>) {
        self.wakers.lock().push(w);
    }
}

