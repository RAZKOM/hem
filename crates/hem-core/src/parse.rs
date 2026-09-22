//! Lazy per-line metadata: timestamp and level, with a bounded cache.

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::level::Level;

/// Metadata extracted from a single line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineMeta {
    /// Epoch milliseconds.
    pub ts: Option<i64>,
    /// Byte span of the timestamp text.
    pub ts_span: Option<(u32, u32)>,
    pub level: Option<Level>,
    /// Byte span of the level keyword / value.
    pub level_span: Option<(u32, u32)>,
}

/// User-supplied regexes (from a custom format in the config) consulted
/// before the built-in detectors.  The first capture group is used.
#[derive(Debug, Default, Clone)]
pub struct CustomPatterns {
    pub level: Option<regex::bytes::Regex>,
    pub timestamp: Option<regex::bytes::Regex>,
}

/// Bounded cache of line metadata keyed by `(generation, line index)`.
#[derive(Debug)]
pub struct MetaCache {
    map: Mutex<HashMap<(u32, u64), LineMeta>>,
    cap: usize,
}

impl Default for MetaCache {
    fn default() -> Self {
        Self::new(200_000)
    }
}

impl MetaCache {
    pub fn new(cap: usize) -> Self {
        Self {
            map: Mutex::new(HashMap::with_capacity(cap.min(65_536))),
            cap,
        }
    }

    pub fn get(&self, generation: u32, line: u64) -> Option<LineMeta> {
        self.map.lock().get(&(generation, line)).copied()
    }

}
