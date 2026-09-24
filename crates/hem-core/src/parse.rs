//! Lazy per-line metadata: timestamp and level, with a bounded cache.

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::{Mutex, RwLock};

use crate::fields;
use crate::format::Format;
use crate::level::{self, Level};
use crate::timestamp::{self, Ts};

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

const JSON_TS_KEYS: &[&str] = &[
    "time",
    "ts",
    "timestamp",
    "@timestamp",
    "t",
    "datetime",
    "date",
    "_time",
    "eventTime",
];
const JSON_LEVEL_KEYS: &[&str] = &[
    "level",
    "lvl",
    "severity",
    "levelname",
    "log.level",
    "loglevel",
    "priority",
    "status",
];
const LOGFMT_TS_KEYS: &[&str] = &["ts", "time", "t", "timestamp"];
const LOGFMT_LEVEL_KEYS: &[&str] = &["level", "lvl", "severity"];

/// User-supplied regexes (from a custom format in the config) consulted
/// before the built-in detectors.  The first capture group is used.
#[derive(Debug, Default, Clone)]
pub struct CustomPatterns {
    pub level: Option<regex::bytes::Regex>,
    pub timestamp: Option<regex::bytes::Regex>,
}

fn custom() -> &'static RwLock<Option<CustomPatterns>> {
    static C: OnceLock<RwLock<Option<CustomPatterns>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(None))
}

fn apply_custom(line: &[u8], meta: &mut LineMeta, default_year: i64) {
    let guard = custom().read();
    let Some(c) = guard.as_ref() else { return };
    if let Some(re) = &c.timestamp {
        if let Some(cap) = re.captures(line) {
            if let Some(m) = cap.get(1).or_else(|| cap.get(0)) {
                if let Some(ts) = timestamp::parse_at(line, m.start(), default_year) {
                    meta.ts = Some(ts.ms);
                    meta.ts_span = Some((ts.start as u32, ts.end as u32));
                }
            }
        }
    }
    if let Some(re) = &c.level {
        if let Some(cap) = re.captures(line) {
            if let Some(m) = cap.get(1).or_else(|| cap.get(0)) {
                if let Some(l) = Level::from_bytes(m.as_bytes()) {
                    meta.level = Some(l);
                    meta.level_span = Some((m.start() as u32, m.end() as u32));
                }
            }
        }
    }
}

/// Parse a line's timestamp and level.
pub fn parse_line(line: &[u8], format: Format, default_year: i64) -> LineMeta {
    let mut meta = LineMeta::default();
    apply_custom(line, &mut meta, default_year);
    if meta.ts.is_some() && meta.level.is_some() {
        return meta;
    }
    match format {
        Format::Json => {
            for key in JSON_TS_KEYS {
                if meta.ts.is_some() {
                    break;
                }
                if let Some((s, e)) = fields::json_value_span(line, key) {
                    let raw = &line[s..e];
                    let inner = if raw.first() == Some(&b'"') && raw.len() >= 2 {
                        &raw[1..raw.len() - 1]
                    } else {
                        raw
                    };
                    if let Some(ts) = timestamp::parse_at(inner, 0, default_year) {
                        let off = s + (raw.len() - inner.len()) / 2;
                        meta.ts = Some(ts.ms);
                        meta.ts_span = Some(((off + ts.start) as u32, (off + ts.end) as u32));
                        break;
                    }
                }
            }
            for key in JSON_LEVEL_KEYS {
                if meta.level.is_some() {
                    break;
                }
                if let Some((s, e)) = fields::json_value_span(line, key) {
                    let raw = &line[s..e];
                    let inner = if raw.first() == Some(&b'"') && raw.len() >= 2 {
                        &raw[1..raw.len() - 1]
                    } else {
                        raw
                    };
                    if let Some(l) = Level::from_bytes(inner) {
                        meta.level = Some(l);
                        meta.level_span = Some((s as u32, e as u32));
                        break;
                    }
                }
            }
        }
        Format::Logfmt => {
            for key in LOGFMT_TS_KEYS {
                if meta.ts.is_some() {
                    break;
                }
                if let Some(v) = fields::logfmt_get(line, key) {
                    if let Some(ts) = timestamp::parse_at(v, 0, default_year) {
                        let off = v.as_ptr() as usize - line.as_ptr() as usize;
                        meta.ts = Some(ts.ms);
                        meta.ts_span = Some(((off + ts.start) as u32, (off + ts.end) as u32));
                        break;
                    }
                }
            }
            for key in LOGFMT_LEVEL_KEYS {
                if meta.level.is_some() {
                    break;
                }
                if let Some(v) = fields::logfmt_get(line, key) {
                    if let Some(l) = Level::from_bytes(v) {
                        let off = v.as_ptr() as usize - line.as_ptr() as usize;
                        meta.level = Some(l);
                        meta.level_span = Some((off as u32, (off + v.len()) as u32));
                        break;
                    }
                }
            }
        }
        Format::Nginx => {
            // Status class as level: 5xx error, 4xx warn, else info.
            if let Some(ts) = timestamp::find(line, default_year) {
                if meta.ts.is_none() {
                    set_ts(&mut meta, ts);
                }
                if let Some(q) = memchr::memchr(b'"', &line[ts.end..]) {
                    let after = ts.end + q + 1;
                    if let Some(q2) = memchr::memchr(b'"', &line[after..]) {
                        let st = after + q2 + 2;
                        if let Some(code) = line.get(st..st + 3) {
                            if code.iter().all(|b| b.is_ascii_digit()) && meta.level.is_none() {
                                meta.level = Some(match code[0] {
                                    b'5' => Level::Error,
                                    b'4' => Level::Warn,
                                    _ => Level::Info,
                                });
                                meta.level_span = Some((st as u32, (st + 3) as u32));
                            }
                        }
                    }
                }
            }
        }
        Format::Syslog | Format::Plain => {}
    }
    if meta.ts.is_none() {
        if let Some(ts) = timestamp::find(line, default_year) {
            set_ts(&mut meta, ts);
        }
    }
    if meta.level.is_none() {
        if let Some((l, s, e)) = level::detect(line) {
            // Avoid treating a level keyword inside the timestamp (e.g. month
            // names never collide, but guard anyway).
            if meta
                .ts_span
                .map(|(ts, te)| (e as u32) <= ts || (s as u32) >= te)
                .unwrap_or(true)
            {
                meta.level = Some(l);
                meta.level_span = Some((s as u32, e as u32));
            }
        }
    }
    meta
}

fn set_ts(meta: &mut LineMeta, ts: Ts) {
    meta.ts = Some(ts.ms);
    meta.ts_span = Some((ts.start as u32, ts.end as u32));
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

    pub fn insert(&self, generation: u32, line: u64, meta: LineMeta) {
        let mut m = self.map.lock();
        if m.len() >= self.cap {
            m.clear();
        }
        m.insert((generation, line), meta);
    }

    /// Get or compute.
    pub fn get_or_parse(
        &self,
        generation: u32,
        line_idx: u64,
        line: &[u8],
        format: Format,
        default_year: i64,
    ) -> LineMeta {
        if let Some(m) = self.get(generation, line_idx) {
            return m;
        }
        let m = parse_line(line, format, default_year);
        self.insert(generation, line_idx, m);
        m
    }

    pub fn clear(&self) {
        self.map.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip() {
        let c = MetaCache::new(2);
        let m = c.get_or_parse(0, 1, b"INFO x", Format::Plain, 2026);
        assert_eq!(m.level, Some(Level::Info));
        assert_eq!(c.get(0, 1), Some(m));
        c.get_or_parse(0, 2, b"WARN", Format::Plain, 2026);
        c.get_or_parse(0, 3, b"WARN", Format::Plain, 2026);
        assert!(c.get(0, 1).is_none(), "cache evicts when full");
    }
}
