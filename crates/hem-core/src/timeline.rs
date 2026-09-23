//! Timeline histogram: per-bucket counts per level, over time or over
//! position when a source has no timestamps.

use crate::level::Level;

/// Fixed internal resolution; the UI resamples to its height.
pub const BUCKETS: usize = 2048;

/// Counts per level (plus "no level") in one bucket.
pub type Counts = [u32; Level::COUNT + 1];

/// What the histogram's x axis measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Epoch milliseconds; `start` is the left edge, each bucket spans
    /// `bucket_ms`.
    Time { start: i64, bucket_ms: i64 },
    /// Row / line position; each bucket spans `bucket_rows` rows.
    Position { bucket_rows: u64 },
}

/// Histogram with automatic range extension (buckets fold when the range
/// grows, so memory stays constant).
#[derive(Debug, Clone)]
pub struct Histogram {
    pub axis: Axis,
    counts: Vec<Counts>,
    /// Highest bucket index that has been touched.
    pub used: usize,
    pub total: u64,
}

impl Histogram {
    pub fn new_time(start: i64, end_hint: i64) -> Self {
        let span = (end_hint - start).max(1);
        let bucket_ms = (span / BUCKETS as i64).max(1);
        Self {
            axis: Axis::Time { start, bucket_ms },
            counts: vec![[0; Level::COUNT + 1]; BUCKETS],
            used: 0,
            total: 0,
        }
    }

    pub fn new_position(rows_hint: u64) -> Self {
        let bucket_rows = (rows_hint / BUCKETS as u64).max(1);
        Self {
            axis: Axis::Position { bucket_rows },
            counts: vec![[0; Level::COUNT + 1]; BUCKETS],
            used: 0,
            total: 0,
        }
    }

    pub fn is_time(&self) -> bool {
        matches!(self.axis, Axis::Time { .. })
    }

    /// Reset counts (keep the axis).
    pub fn clear(&mut self) {
        self.counts.fill([0; Level::COUNT + 1]);
        self.used = 0;
        self.total = 0;
    }

}
