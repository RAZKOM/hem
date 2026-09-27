//! Timestamp detection and parsing.
//!
//! All timestamps are normalised to **milliseconds since the Unix epoch**.
//! Timestamps without a zone are treated as UTC; hem only needs them to be
//! mutually consistent for ordering, bucketing and `--since/--until`, and
//! the original text is what gets displayed.

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone, Utc};

/// A detected timestamp: epoch milliseconds plus the byte span in the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ts {
    pub ms: i64,
    pub start: usize,
    pub end: usize,
}

/// Days since 1970-01-01 for a proleptic Gregorian civil date.
/// (Howard Hinnant's `days_from_civil`.)
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn valid_date(y: i64, m: u32, d: u32) -> bool {
    (1970..=2200).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d)
}

fn valid_time(h: u32, m: u32, s: u32) -> bool {
    h < 24 && m < 60 && s < 61
}

#[inline]
fn digit(b: u8) -> Option<u32> {
    if b.is_ascii_digit() {
        Some((b - b'0') as u32)
    } else {
        None
    }
}

#[inline]
fn num(buf: &[u8], at: usize, len: usize) -> Option<u32> {
    let mut v = 0u32;
    for i in 0..len {
        v = v * 10 + digit(*buf.get(at + i)?)?;
    }
    Some(v)
}

fn month_abbr(b: &[u8]) -> Option<u32> {
    if b.len() < 3 {
        return None;
    }
    let m = [
        b[0].to_ascii_lowercase(),
        b[1].to_ascii_lowercase(),
        b[2].to_ascii_lowercase(),
    ];
    Some(match &m {
        b"jan" => 1,
        b"feb" => 2,
        b"mar" => 3,
        b"apr" => 4,
        b"may" => 5,
        b"jun" => 6,
        b"jul" => 7,
        b"aug" => 8,
        b"sep" => 9,
        b"oct" => 10,
        b"nov" => 11,
        b"dec" => 12,
        _ => return None,
    })
}

/// Parse `HH:MM:SS[.fff...]` at `at`.  Returns (ms_of_day, end).
fn parse_time(buf: &[u8], at: usize) -> Option<(i64, usize)> {
    let h = num(buf, at, 2)?;
    if buf.get(at + 2) != Some(&b':') {
        return None;
    }
    let mi = num(buf, at + 3, 2)?;
    if buf.get(at + 5) != Some(&b':') {
        return None;
    }
    let s = num(buf, at + 6, 2)?;
    if !valid_time(h, mi, s) {
        return None;
    }
    let mut end = at + 8;
    let mut frac_ms = 0i64;
    if matches!(buf.get(end), Some(b'.') | Some(b',')) {
        let mut i = end + 1;
        let mut scale = 100i64;
        let mut any = false;
        while let Some(d) = buf.get(i).and_then(|b| digit(*b)) {
            if scale > 0 {
                frac_ms += d as i64 * scale;
                scale /= 10;
            }
            i += 1;
            any = true;
        }
        if any {
            end = i;
        }
    }
    let ms = ((h as i64 * 3600 + mi as i64 * 60 + s as i64) * 1000) + frac_ms;
    Some((ms, end))
}

/// Parse an optional zone suffix (`Z`, `+02:00`, `-0500`, ` +0000`).
/// Returns (offset_ms, end).
fn parse_zone(buf: &[u8], at: usize) -> (i64, usize) {
    let mut i = at;
    if buf.get(i) == Some(&b' ') && matches!(buf.get(i + 1), Some(b'+') | Some(b'-')) {
        i += 1;
    }
    match buf.get(i) {
        Some(b'Z') | Some(b'z') => (0, i + 1),
        Some(sign @ (b'+' | b'-')) => {
            let Some(h) = num(buf, i + 1, 2) else {
                return (0, at);
            };
            let mut j = i + 3;
            if buf.get(j) == Some(&b':') {
                j += 1;
            }
            let m = num(buf, j, 2).unwrap_or(0);
            if num(buf, j, 2).is_some() {
                j += 2;
            }
            if h > 14 || m > 59 {
                return (0, at);
            }
            let off = (h as i64 * 3600 + m as i64 * 60) * 1000;
            (if *sign == b'+' { -off } else { off }, j)
        }
        _ => (0, at),
    }
}

/// ISO 8601 / RFC 3339 / `YYYY-MM-DD HH:MM:SS[,fff]` / `YYYY/MM/DD`.
fn parse_iso(buf: &[u8], at: usize) -> Option<Ts> {
    let y = num(buf, at, 4)? as i64;
    let sep = *buf.get(at + 4)?;
    if sep != b'-' && sep != b'/' {
        return None;
    }
    let mo = num(buf, at + 5, 2)?;
    if buf.get(at + 7) != Some(&sep) {
        return None;
    }
    let d = num(buf, at + 8, 2)?;
    if !valid_date(y, mo, d) {
        return None;
    }
    let mut i = at + 10;
    let date_ms = days_from_civil(y, mo, d) * 86_400_000;
    match buf.get(i) {
        Some(b'T') | Some(b't') | Some(b' ') | Some(b'_') => i += 1,
        _ => {
            return Some(Ts {
                ms: date_ms,
                start: at,
                end: i,
            })
        }
    }
    let Some((tod, end)) = parse_time(buf, i) else {
        return Some(Ts {
            ms: date_ms,
            start: at,
            end: at + 10,
        });
    };
    let (off, end) = parse_zone(buf, end);
    Some(Ts {
        ms: date_ms + tod + off,
        start: at,
        end,
    })
}

/// Apache / nginx: `10/Oct/2026:13:55:36 -0500` (with or without brackets).
fn parse_clf(buf: &[u8], at: usize) -> Option<Ts> {
    let d = num(buf, at, 2)?;
    if buf.get(at + 2) != Some(&b'/') {
        return None;
    }
    let mo = month_abbr(buf.get(at + 3..at + 6)?)?;
    if buf.get(at + 6) != Some(&b'/') {
        return None;
    }
    let y = num(buf, at + 7, 4)? as i64;
    if buf.get(at + 11) != Some(&b':') || !valid_date(y, mo, d) {
        return None;
    }
    let (tod, end) = parse_time(buf, at + 12)?;
    let (off, end) = parse_zone(buf, end);
    Some(Ts {
        ms: days_from_civil(y, mo, d) * 86_400_000 + tod + off,
        start: at,
        end,
    })
}

/// Syslog / ctime-ish: `Oct 13 10:02:11`, `Oct  3 10:02:11`,
/// `Oct 13, 2026 10:02:11`, `Oct 13 2026 10:02:11`.  A missing year is
/// taken from `default_year`.
fn parse_syslog(buf: &[u8], at: usize, default_year: i64) -> Option<Ts> {
    let mo = month_abbr(buf.get(at..at + 3)?)?;
    if buf.get(at + 3) != Some(&b' ') {
        return None;
    }
    let mut i = at + 4;
    while buf.get(i) == Some(&b' ') {
        i += 1;
    }
    let (d, dl) = match (num(buf, i, 2), num(buf, i, 1)) {
        (Some(d), _) if buf.get(i + 2).map(|b| !b.is_ascii_digit()).unwrap_or(true) => (d, 2),
        (_, Some(d)) => (d, 1),
        _ => return None,
    };
    i += dl;
    if buf.get(i) == Some(&b',') {
        i += 1;
    }
    if buf.get(i) != Some(&b' ') {
        return None;
    }
    i += 1;
    let mut y = default_year;
    if let Some(yy) = num(buf, i, 4) {
        if buf.get(i + 4) == Some(&b' ') {
            y = yy as i64;
            i += 5;
        }
    }
    if !valid_date(y, mo, d) {
        return None;
    }
    let (tod, end) = parse_time(buf, i)?;
    let (off, end) = parse_zone(buf, end);
    Some(Ts {
        ms: days_from_civil(y, mo, d) * 86_400_000 + tod + off,
        start: at,
        end,
    })
}

/// Epoch seconds / millis / micros / nanos, optionally with a fraction.
fn parse_epoch(buf: &[u8], at: usize) -> Option<Ts> {
    let mut i = at;
    let mut int: i64 = 0;
    let mut digits = 0;
    while let Some(d) = buf.get(i).and_then(|b| digit(*b)) {
        int = int.checked_mul(10)?.checked_add(d as i64)?;
        i += 1;
        digits += 1;
    }
    if digits == 0 {
        return None;
    }
    let mut frac = 0f64;
    let mut end = i;
    if buf.get(i) == Some(&b'.') {
        let mut j = i + 1;
        let mut scale = 0.1;
        while let Some(d) = buf.get(j).and_then(|b| digit(*b)) {
            frac += d as f64 * scale;
            scale /= 10.0;
            j += 1;
        }
        if j > i + 1 {
            end = j;
        }
    }
    if end < buf.len() && (buf[end].is_ascii_alphanumeric() || buf[end] == b'_') {
        return None;
    }
    // Plausible ranges: 2001-09-09 .. 2286-11-20 for seconds.
    let ms = match digits {
        9..=10 => int * 1000 + (frac * 1000.0) as i64,
        12..=13 => int,
        15..=16 => int / 1000,
        18..=19 => int / 1_000_000,
        _ => return None,
    };
    if !(1_000_000_000_000..10_000_000_000_000).contains(&ms) {
        return None;
    }
    Some(Ts { ms, start: at, end })
}

/// Try every textual format at position `at`.
pub fn parse_at(buf: &[u8], at: usize, default_year: i64) -> Option<Ts> {
    let b = *buf.get(at)?;
    if b.is_ascii_digit() {
        parse_iso(buf, at)
            .or_else(|| parse_clf(buf, at))
            .or_else(|| parse_epoch(buf, at))
    } else if b.is_ascii_alphabetic() {
        parse_syslog(buf, at, default_year)
    } else {
        None
    }
}

/// How many leading bytes of a line are scanned for a timestamp.
const SCAN_LIMIT: usize = 160;

/// Scan the beginning of a line for a timestamp in any supported format.
pub fn find(line: &[u8], default_year: i64) -> Option<Ts> {
    let limit = line.len().min(SCAN_LIMIT);
    let mut i = 0;
    while i < limit {
        let b = line[i];
        let word_start = i == 0 || !(line[i - 1].is_ascii_alphanumeric());
        if word_start && (b.is_ascii_digit() || b.is_ascii_uppercase()) {
            if let Some(ts) = parse_at(line, i, default_year) {
                return Some(ts);
            }
        }
        i += 1;
    }
    None
}

/// Human-friendly short form for axis labels: picks a precision suited to
/// the span being labelled.
pub fn format_axis(ms: i64, span_ms: i64) -> String {
    let Some(dt) = DateTime::from_timestamp_millis(ms) else {
        return String::new();
    };
    if span_ms >= 2 * 86_400_000 {
        dt.format("%m-%d %H:%M").to_string()
    } else if span_ms >= 60_000 {
        dt.format("%H:%M:%S").to_string()
    } else {
        dt.format("%H:%M:%S%.3f").to_string()
    }
}

/// Current year, used for year-less syslog timestamps.
pub fn current_year() -> i64 {
    Local::now().year() as i64
}

/// Convert a `NaiveDateTime` (treated as UTC) to epoch ms.
pub fn naive_to_ms(n: NaiveDateTime) -> i64 {
    Utc.from_utc_datetime(&n).timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(s: &str) -> Option<i64> {
        find(s.as_bytes(), 2026).map(|t| t.ms)
    }

    #[test]
    fn epoch_variants() {
        assert_eq!(ms("1791885731 x"), Some(1_791_885_731_000));
        assert_eq!(ms("1791885731.204 x"), Some(1_791_885_731_204));
        assert_eq!(ms("1791885731204 x"), Some(1_791_885_731_204));
        assert_eq!(ms("1791885731204567 x"), Some(1_791_885_731_204));
        assert_eq!(ms("1791885731204567890 x"), Some(1_791_885_731_204));
        assert_eq!(ms("12345 x"), None);
    }

    #[test]
    fn no_timestamp() {
        assert_eq!(ms("hello world 42"), None);
        assert_eq!(ms("   at com.example.Foo(Foo.java:12)"), None);
        assert_eq!(ms(""), None);
    }

    #[test]
    fn find_span() {
        let t = find(b"[2026-10-13T10:02:11Z] x", 2026).unwrap();
        assert_eq!((t.start, t.end), (1, 21));
    }
}
