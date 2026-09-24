//! Timeline histogram: per-bucket counts per level, over time or over
//! position when a source has no timestamps.

use crate::level::Level;

/// Fixed internal resolution; the UI resamples to its height.
pub const BUCKETS: usize = 2048;

/// Slot index for lines without a level.
pub const NO_LEVEL: usize = Level::COUNT;

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

    /// Bucket index for a key (timestamp or row), folding the histogram if
    /// the key lies beyond the current range.
    fn bucket_for(&mut self, key: i64) -> usize {
        loop {
            let idx = match self.axis {
                Axis::Time { start, bucket_ms } => {
                    if key < start {
                        return 0;
                    }
                    ((key - start) / bucket_ms) as u64
                }
                Axis::Position { bucket_rows } => (key.max(0) as u64) / bucket_rows,
            };
            if (idx as usize) < BUCKETS {
                return idx as usize;
            }
            self.fold();
        }
    }

    /// Halve the resolution: merge bucket pairs so the range doubles.
    fn fold(&mut self) {
        for i in 0..BUCKETS / 2 {
            let a = self.counts[2 * i];
            let b = self.counts[2 * i + 1];
            let mut m = [0u32; Level::COUNT + 1];
            for k in 0..=Level::COUNT {
                m[k] = a[k].saturating_add(b[k]);
            }
            self.counts[i] = m;
        }
        self.counts[BUCKETS / 2..].fill([0; Level::COUNT + 1]);
        self.used /= 2;
        match &mut self.axis {
            Axis::Time { bucket_ms, .. } => *bucket_ms = bucket_ms.saturating_mul(2),
            Axis::Position { bucket_rows } => *bucket_rows = bucket_rows.saturating_mul(2),
        }
    }

    /// Record one line.
    pub fn add(&mut self, key: i64, level: Option<Level>) {
        let b = self.bucket_for(key);
        let slot = level.map(|l| l.index()).unwrap_or(NO_LEVEL);
        self.counts[b][slot] = self.counts[b][slot].saturating_add(1);
        if b >= self.used {
            self.used = b + 1;
        }
        self.total += 1;
    }

}
