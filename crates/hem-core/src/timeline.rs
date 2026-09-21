//! Timeline histogram: per-bucket counts per level, over time or over
//! position when a source has no timestamps.

use crate::level::Level;

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

