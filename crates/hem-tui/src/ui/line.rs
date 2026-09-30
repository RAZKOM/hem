//! Syntax segmentation of a single log line.  Produces byte ranges tagged
//! with a [`Kind`]; the TUI maps kinds to theme styles and plain mode maps
//! them to ANSI colours.

use hem_core::format::Format;
use hem_core::parse::LineMeta;
use hem_core::Level;

/// What a run of bytes is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Timestamp,
    Level(Level),
    Key,
    Str,
    Number,
    Url,
    Punct,
    Match,
    CurrentMatch,
    Highlight,
}

/// A tagged byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// Maximum bytes segmented per line (the rest is plain text).
pub const MAX_SEGMENT_BYTES: usize = 8192;

/// Segment `line` into non-overlapping runs covering `0..line.len()`.
///
/// `matches` / `highlights` are byte ranges painted on top of the syntax
/// (search matches and value highlights); `current` marks the match that
/// should stand out.
pub fn segments(
    line: &[u8],
    meta: &LineMeta,
    format: Format,
    matches: &[(usize, usize)],
    current: bool,
    highlights: &[(usize, usize)],
) -> Vec<Segment> {
    let n = line.len();
    if n == 0 {
        return Vec::new();
    }
    let lim = n.min(MAX_SEGMENT_BYTES);
    let mut map: Vec<Kind> = vec![Kind::Text; lim];
    let paint = |map: &mut Vec<Kind>, s: usize, e: usize, k: Kind| {
        let e = e.min(lim);
        for slot in map.iter_mut().take(e).skip(s.min(e)) {
            *slot = k;
        }
    };

    // Numbers (standalone digit runs, optional decimal / sign / unit).
    let mut i = 0;
    while i < lim {
        let b = line[i];
        if b.is_ascii_digit() && (i == 0 || !is_word(line[i - 1])) {
            let mut j = i;
            while j < lim && (line[j].is_ascii_digit() || line[j] == b'.' || line[j] == b'_') {
                j += 1;
            }
            // Trailing unit like ms / s / % / KB.
            let mut k = j;
            while k < lim && k - j < 3 && (line[k].is_ascii_alphabetic() || line[k] == b'%') {
                k += 1;
            }
            if k >= lim || !is_word(line[k]) {
                j = k;
            }
            if j >= lim || !is_word(line[j]) {
                paint(&mut map, i, j, Kind::Number);
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }

    // Keys: `word=` (logfmt) and `"word":` (JSON).
    match format {
        Format::Logfmt | Format::Plain | Format::Syslog => {
            let mut i = 0;
            while i < lim {
                if line[i] == b'=' && i > 0 {
                    let mut s = i;
                    while s > 0 && is_key_byte(line[s - 1]) {
                        s -= 1;
                    }
                    if s < i
                        && (s == 0
                            || line[s - 1].is_ascii_whitespace()
                            || line[s - 1] == b'['
                            || line[s - 1] == b'('
                            || line[s - 1] == b',')
                    {
                        paint(&mut map, s, i, Kind::Key);
                        paint(&mut map, i, i + 1, Kind::Punct);
                    }
                }
                i += 1;
            }
        }
        _ => {}
    }

    // Quoted strings and JSON keys.
    let mut i = 0;
    while i < lim {
        let b = line[i];
        if b == b'"' || (b == b'\'' && format == Format::Plain) {
            let q = b;
            let mut j = i + 1;
            while j < lim && line[j] != q {
                if line[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            if j < lim {
                // Key if followed by ':' (JSON).
                let mut k = j + 1;
                while k < lim && line[k] == b' ' {
                    k += 1;
                }
                let is_key = format == Format::Json && k < lim && line[k] == b':';
                paint(
                    &mut map,
                    i,
                    j + 1,
                    if is_key { Kind::Key } else { Kind::Str },
                );
                if is_key {
                    paint(&mut map, k, k + 1, Kind::Punct);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }

    // Structural punctuation for JSON.
    if format == Format::Json {
        for (i, &b) in line.iter().enumerate().take(lim) {
            if matches!(b, b'{' | b'}' | b'[' | b']' | b',') && map[i] == Kind::Text {
                map[i] = Kind::Punct;
            }
        }
    }

    // URLs.
    for pat in [&b"http://"[..], &b"https://"[..]] {
        let mut from = 0;
        while let Some(p) = memchr::memmem::find(&line[from..lim], pat) {
            let s = from + p;
            let mut e = s;
            while e < lim
                && !line[e].is_ascii_whitespace()
                && line[e] != b'"'
                && line[e] != b'\''
                && line[e] != b'>'
            {
                e += 1;
            }
            while e > s && matches!(line[e - 1], b'.' | b',' | b')' | b';') {
                e -= 1;
            }
            paint(&mut map, s, e, Kind::Url);
            from = e.max(s + 1);
        }
    }

    // Level and timestamp win over everything syntactic.
    if let (Some(l), Some((s, e))) = (meta.level, meta.level_span) {
        paint(&mut map, s as usize, e as usize, Kind::Level(l));
    }
    if let Some((s, e)) = meta.ts_span {
        paint(&mut map, s as usize, e as usize, Kind::Timestamp);
    }
    // Overlays.
    for &(s, e) in highlights {
        paint(&mut map, s, e, Kind::Highlight);
    }
    for &(s, e) in matches {
        paint(
            &mut map,
            s,
            e,
            if current {
                Kind::CurrentMatch
            } else {
                Kind::Match
            },
        );
    }

    // Coalesce.
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=lim {
        if i == lim || map[i] != map[start] {
            out.push(Segment {
                start,
                end: i,
                kind: map[start],
            });
            start = i;
        }
    }
    if lim < n {
        out.push(Segment {
            start: lim,
            end: n,
            kind: Kind::Text,
        });
    }
    out
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'@')
}

#[cfg(test)]
mod tests {
    use super::*;
    use hem_core::parse::parse_line;

    fn kinds(line: &[u8], fmt: Format) -> Vec<(String, Kind)> {
        let meta = parse_line(line, fmt, 2026);
        segments(line, &meta, fmt, &[], false, &[])
            .into_iter()
            .map(|s| {
                (
                    String::from_utf8_lossy(&line[s.start..s.end]).into_owned(),
                    s.kind,
                )
            })
            .collect()
    }

    #[test]
    fn plain_line_segments() {
        let k = kinds(
            b"2026-10-13 10:02:14 WARN slow query (812ms) https://x.io/a, user=\"bob\"",
            Format::Plain,
        );
        assert_eq!(k[0], ("2026-10-13 10:02:14".into(), Kind::Timestamp));
        assert!(k.contains(&("WARN".into(), Kind::Level(Level::Warn))));
        assert!(k.contains(&("812ms".into(), Kind::Number)));
        assert!(k.contains(&("https://x.io/a".into(), Kind::Url)));
        assert!(k.contains(&("user".into(), Kind::Key)));
        assert!(k.contains(&("\"bob\"".into(), Kind::Str)));
    }

    #[test]
    fn json_segments() {
        let k = kinds(br#"{"level":"error","n":42,"msg":"x"}"#, Format::Json);
        assert!(k.contains(&("\"level\"".into(), Kind::Key)));
        assert!(k.contains(&("\"error\"".into(), Kind::Level(Level::Error))));
        assert!(k.contains(&("42".into(), Kind::Number)));
        assert!(k.contains(&("\"x\"".into(), Kind::Str)));
    }

    #[test]
    fn overlays_and_coverage() {
        let line = b"abc def ghi";
        let segs = segments(
            line,
            &LineMeta::default(),
            Format::Plain,
            &[(4, 7)],
            true,
            &[(0, 11)],
        );
        assert_eq!(segs.first().map(|s| s.start), Some(0));
        assert_eq!(segs.last().map(|s| s.end), Some(11));
        assert!(segs
            .iter()
            .any(|s| s.kind == Kind::CurrentMatch && s.start == 4 && s.end == 7));
        for w in segs.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        let long = vec![b'x'; MAX_SEGMENT_BYTES + 10];
        let segs = segments(&long, &LineMeta::default(), Format::Plain, &[], false, &[]);
        assert_eq!(segs.last().map(|s| s.end), Some(long.len()));
    }
}
