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

    pub fn buckets(&self) -> &[Counts] {
        &self.counts
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

    /// Bucket index containing `key` (no folding; clamped).
    pub fn bucket_of(&self, key: i64) -> usize {
        let idx = match self.axis {
            Axis::Time { start, bucket_ms } => ((key - start).max(0) / bucket_ms) as usize,
            Axis::Position { bucket_rows } => ((key.max(0) as u64) / bucket_rows) as usize,
        };
        idx.min(BUCKETS - 1)
    }

    /// Resample the used part of the histogram into `n` display cells,
    /// summing counts.  Returns an empty vec if nothing was recorded.
    pub fn resample(&self, n: usize) -> Vec<Counts> {
        let used = self.used.max(1);
        if n == 0 || self.total == 0 {
            return Vec::new();
        }
        let mut out = vec![[0u32; Level::COUNT + 1]; n];
        for b in 0..used {
            let cell = (b * n / used).min(n - 1);
            for (k, slot) in out[cell].iter_mut().enumerate() {
                *slot = slot.saturating_add(self.counts[b][k]);
            }
        }
        out
    }

    /// Display cell for a key.
    pub fn cell_of(&self, key: i64, n: usize) -> usize {
        let used = self.used.max(1);
        let b = self.bucket_of(key).min(used - 1);
        (b * n / used).min(n.saturating_sub(1))
    }

    /// First bucket at or after `from` whose counts at level `>= min` are
    /// non-zero.  `forward=false` searches backwards.
    pub fn next_bucket_with(&self, from: usize, min: Level, forward: bool) -> Option<usize> {
        let hit = |c: &Counts| (min.index()..Level::COUNT).any(|k| c[k] > 0);
        if forward {
            (from..self.used).find(|&b| hit(&self.counts[b]))
        } else {
            (0..=from.min(self.used.saturating_sub(1)))
                .rev()
                .find(|&b| hit(&self.counts[b]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_axis_folds() {
        let mut h = Histogram::new_position(100);
        for i in 0..10_000 {
            h.add(
                i,
                Some(if i % 100 == 0 {
                    Level::Error
                } else {
                    Level::Info
                }),
            );
        }
        assert_eq!(h.total, 10_000);
        let cells = h.resample(10);
        let errs: u32 = cells.iter().map(|c| c[Level::Error.index()]).sum();
        assert_eq!(errs, 100);
        let infos: u32 = cells.iter().map(|c| c[Level::Info.index()]).sum();
        assert_eq!(infos, 9_900);
        assert!(matches!(h.axis, Axis::Position { bucket_rows } if bucket_rows >= 4));
    }

    #[test]
    fn time_axis_and_lookup() {
        let mut h = Histogram::new_time(1_000, 1_000 + 2048 * 10);
        h.add(1_000, Some(Level::Warn));
        h.add(1_000 + 1000 * 10, Some(Level::Error));
        h.add(500, None); // before range clamps to bucket 0
        assert_eq!(h.bucket_of(1_000 + 1000 * 10), 1000);
        assert_eq!(h.next_bucket_with(1, Level::Error, true), Some(1000));
        assert_eq!(h.next_bucket_with(999, Level::Warn, false), Some(0));
        assert_eq!(h.next_bucket_with(1001, Level::Error, true), None);
        // Beyond the range: folds.
        h.add(1_000 + 2048 * 10 * 3, Some(Level::Info));
        assert!(matches!(h.axis, Axis::Time { bucket_ms, .. } if bucket_ms == 40));
        assert_eq!(h.total, 4);
    }
}
