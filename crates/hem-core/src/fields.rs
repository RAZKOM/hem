//! Structured field extraction for JSON, logfmt, nginx and syslog lines.

use crate::format::Format;

/// A flattened `(key, value)` pair.  Nested JSON keys are joined with `.`.
pub type Field = (String, String);

/// Extract fields from a line according to its format.  Plain lines yield
/// nothing.  Never panics on malformed input.
pub fn extract(line: &[u8], format: Format) -> Vec<Field> {
    match format {
        Format::Json => json_fields(line),
        Format::Logfmt => logfmt_fields(line),
        Format::Nginx => nginx_fields(line),
        Format::Syslog => syslog_fields(line),
        Format::Plain => Vec::new(),
    }
}

/// Fast path: fetch a single top-level value by key without building the
/// whole field list.  Works for JSON (string / number / bool values) and
/// logfmt.  Dotted JSON keys (`ctx.user`) that are not top-level keys fall
/// back to the flattened field list.
pub fn get<'a>(line: &'a [u8], format: Format, key: &str) -> Option<std::borrow::Cow<'a, str>> {
    match format {
        Format::Json => json_get(line, key).or_else(|| {
            if !key.contains('.') {
                return None;
            }
            json_fields(line)
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| std::borrow::Cow::Owned(v))
        }),
        Format::Logfmt => logfmt_get(line, key).map(String::from_utf8_lossy),
        _ => extract(line, format)
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| std::borrow::Cow::Owned(v)),
    }
}

pub fn json_fields(line: &[u8]) -> Vec<Field> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    flatten("", &v, &mut out, 0);
    out
}

fn flatten(prefix: &str, v: &serde_json::Value, out: &mut Vec<Field>, depth: usize) {
    use serde_json::Value;
    match v {
        Value::Object(map) if depth < 6 => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out, depth + 1);
            }
        }
        Value::String(s) => out.push((prefix.to_string(), s.clone())),
        Value::Null => out.push((prefix.to_string(), "null".into())),
        other => out.push((prefix.to_string(), other.to_string())),
    }
}

/// Find a top-level JSON key and return its scalar value.  This is a
/// lightweight scanner that understands string escapes and skips nested
/// containers; it is used for the hot paths (timestamp / level lookup).
pub fn json_get<'a>(line: &'a [u8], key: &str) -> Option<std::borrow::Cow<'a, str>> {
    let (s, e) = json_value_span(line, key)?;
    let raw = &line[s..e];
    if raw.first() == Some(&b'"') {
        let inner = &raw[1..raw.len().saturating_sub(1)];
        if inner.contains(&b'\\') {
            serde_json::from_slice::<String>(raw)
                .ok()
                .map(std::borrow::Cow::Owned)
        } else {
            Some(String::from_utf8_lossy(inner))
        }
    } else {
        Some(String::from_utf8_lossy(raw))
    }
}

/// Byte span of the raw value for top-level `key`.
pub fn json_value_span(line: &[u8], key: &str) -> Option<(usize, usize)> {
    let mut i = skip_ws(line, 0);
    if line.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    loop {
        i = skip_ws(line, i);
        match line.get(i)? {
            b'}' => return None,
            b',' => {
                i += 1;
                continue;
            }
            b'"' => {}
            _ => return None,
        }
        let (ks, ke) = string_span(line, i)?;
        i = skip_ws(line, ke);
        if line.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(line, i + 1);
        let vs = i;
        let ve = value_end(line, i)?;
        if &line[ks + 1..ke - 1] == key.as_bytes() {
            return Some((vs, ve));
        }
        i = ve;
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Span `[start, end)` of a JSON string starting at `i` (inclusive of
/// quotes).
fn string_span(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some((i, j + 1)),
            _ => j += 1,
        }
    }
    None
}

fn value_end(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        b'"' => string_span(b, i).map(|(_, e)| e),
        b'{' | b'[' => {
            let mut depth = 0i32;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = string_span(b, j)?.1;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && !b[j].is_ascii_whitespace()
            {
                j += 1;
            }
            Some(j)
        }
    }
}

/// Parse logfmt pairs.  Bare words without `=` are ignored.
pub fn logfmt_fields(line: &[u8]) -> Vec<Field> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        while i < line.len() && line[i].is_ascii_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < line.len() && line[i] != b'=' && !line[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= line.len() || line[i] != b'=' {
            continue;
        }
        let key = &line[ks..i];
        i += 1;
        let (val, next) = logfmt_value(line, i);
        i = next;
        if !key.is_empty() {
            out.push((String::from_utf8_lossy(key).into_owned(), val));
        }
    }
    out
}

fn logfmt_value(line: &[u8], i: usize) -> (String, usize) {
    if line.get(i) == Some(&b'"') {
        let mut j = i + 1;
        let mut s = Vec::new();
        while j < line.len() {
            match line[j] {
                b'\\' if j + 1 < line.len() => {
                    s.push(match line[j + 1] {
                        b'n' => b'\n',
                        b't' => b'\t',
                        c => c,
                    });
                    j += 2;
                }
                b'"' => {
                    j += 1;
                    break;
                }
                c => {
                    s.push(c);
                    j += 1;
                }
            }
        }
        (String::from_utf8_lossy(&s).into_owned(), j)
    } else {
        let mut j = i;
        while j < line.len() && !line[j].is_ascii_whitespace() {
            j += 1;
        }
        (String::from_utf8_lossy(&line[i..j]).into_owned(), j)
    }
}

/// Fetch the raw (unescaped, unquoted) bytes of a logfmt value.
pub fn logfmt_get<'a>(line: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let kb = key.as_bytes();
    let mut i = 0;
    while i + kb.len() < line.len() {
        let at_start = i == 0 || line[i - 1].is_ascii_whitespace();
        if at_start && &line[i..i + kb.len()] == kb && line.get(i + kb.len()) == Some(&b'=') {
            let vs = i + kb.len() + 1;
            if line.get(vs) == Some(&b'"') {
                let mut j = vs + 1;
                while j < line.len() && line[j] != b'"' {
                    if line[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                return Some(&line[vs + 1..j.min(line.len())]);
            }
            let mut j = vs;
            while j < line.len() && !line[j].is_ascii_whitespace() {
                j += 1;
            }
            return Some(&line[vs..j]);
        }
        i += 1;
    }
    None
}

/// nginx / Apache combined log format.
pub fn nginx_fields(line: &[u8]) -> Vec<Field> {
    let s = String::from_utf8_lossy(line);
    let mut out = Vec::new();
    let mut rest = s.as_ref();
    let take_word = |rest: &mut &str| -> Option<String> {
        let t = rest.trim_start();
        let end = t.find(' ').unwrap_or(t.len());
        let w = &t[..end];
        *rest = &t[end..];
        if w.is_empty() {
            None
        } else {
            Some(w.to_string())
        }
    };
    let take_delim = |rest: &mut &str, open: char, close: char| -> Option<String> {
        let t = rest.trim_start();
        if !t.starts_with(open) {
            return None;
        }
        let end = t[1..].find(close)? + 1;
        let w = &t[1..end];
        *rest = &t[end + 1..];
        Some(w.to_string())
    };
    if let Some(v) = take_word(&mut rest) {
        out.push(("remote_addr".into(), v));
    }
    if let Some(v) = take_word(&mut rest) {
        out.push(("ident".into(), v));
    }
    if let Some(v) = take_word(&mut rest) {
        out.push(("user".into(), v));
    }
    if let Some(v) = take_delim(&mut rest, '[', ']') {
        out.push(("time".into(), v));
    }
    if let Some(v) = take_delim(&mut rest, '"', '"') {
        let mut parts = v.splitn(3, ' ');
        if let Some(m) = parts.next() {
            out.push(("method".into(), m.to_string()));
        }
        if let Some(p) = parts.next() {
            out.push(("path".into(), p.to_string()));
        }
        if let Some(p) = parts.next() {
            out.push(("protocol".into(), p.to_string()));
        }
        out.push(("request".into(), v));
    }
    if let Some(v) = take_word(&mut rest) {
        out.push(("status".into(), v));
    }
    if let Some(v) = take_word(&mut rest) {
        out.push(("bytes".into(), v));
    }
    if let Some(v) = take_delim(&mut rest, '"', '"') {
        out.push(("referer".into(), v));
    }
    if let Some(v) = take_delim(&mut rest, '"', '"') {
        out.push(("user_agent".into(), v));
    }
    out
}

/// RFC 3164-ish syslog: `Oct 13 10:02:11 host app[pid]: message`.
pub fn syslog_fields(line: &[u8]) -> Vec<Field> {
    let s = String::from_utf8_lossy(line);
    let mut out = Vec::new();
    let mut rest: &str = &s;
    if rest.starts_with('<') {
        if let Some(end) = rest.find('>') {
            out.push(("pri".into(), rest[1..end].to_string()));
            rest = &rest[end + 1..];
        }
    }
    // Timestamp: "Mon DD HH:MM:SS" = 15 chars, or an ISO timestamp word.
    let t = rest.trim_start();
    let ts_len = if t.len() >= 15 && t.as_bytes()[3] == b' ' && t.as_bytes()[9] == b':' {
        15
    } else {
        t.find(' ').unwrap_or(t.len())
    };
    if ts_len > 0 {
        out.push(("time".into(), t[..ts_len].to_string()));
    }
    rest = &t[ts_len..];
    let mut words = rest.trim_start().splitn(3, ' ');
    if let Some(h) = words.next() {
        out.push(("host".into(), h.to_string()));
    }
    if let Some(tag) = words.next() {
        let tag = tag.trim_end_matches(':');
        if let Some(b) = tag.find('[') {
            out.push(("app".into(), tag[..b].to_string()));
            out.push(("pid".into(), tag[b + 1..].trim_end_matches(']').to_string()));
        } else {
            out.push(("app".into(), tag.to_string()));
        }
    }
    if let Some(msg) = words.next() {
        out.push(("msg".into(), msg.trim_start().to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_flatten_and_get() {
        let l = br#"{"level":"info","ts":1.5,"ctx":{"user":"bob","n":[1,2]},"msg":"a \"q\""}"#;
        let f = json_fields(l);
        assert!(f.contains(&("ctx.user".into(), "bob".into())));
        assert!(f.contains(&("ctx.n".into(), "[1,2]".into())));
        assert_eq!(json_get(l, "level").as_deref(), Some("info"));
        assert_eq!(json_get(l, "ts").as_deref(), Some("1.5"));
        assert_eq!(json_get(l, "msg").as_deref(), Some("a \"q\""));
        assert_eq!(json_get(l, "nope"), None);
        assert_eq!(json_get(b"not json", "x"), None);
        assert_eq!(json_get(br#"{"a":"#, "a"), None);
        // Nested keys go through the flattened fields.
        assert_eq!(get(l, Format::Json, "ctx.user").as_deref(), Some("bob"));
        assert_eq!(get(l, Format::Json, "ctx.nope"), None);
    }

    #[test]
    fn logfmt_pairs() {
        let l = br#"ts=2026-10-13T10:00:00Z level=warn msg="slow query (812ms)" dur=812 flag"#;
        let f = logfmt_fields(l);
        assert_eq!(f.len(), 4);
        assert_eq!(f[2], ("msg".into(), "slow query (812ms)".into()));
        assert_eq!(logfmt_get(l, "level"), Some(&b"warn"[..]));
        assert_eq!(logfmt_get(l, "msg"), Some(&b"slow query (812ms)"[..]));
        assert_eq!(logfmt_get(l, "s"), None);
    }

    #[test]
    fn nginx_combined() {
        let l = br#"1.2.3.4 - bob [13/Oct/2026:13:55:36 -0500] "GET /x?y=1 HTTP/1.1" 503 12 "-" "curl/8""#;
        let f = nginx_fields(l);
        let get = |k: &str| f.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());
        assert_eq!(get("status"), Some("503"));
        assert_eq!(get("path"), Some("/x?y=1"));
        assert_eq!(get("user_agent"), Some("curl/8"));
        assert_eq!(get("user"), Some("bob"));
    }

    #[test]
    fn syslog_parts() {
        let f = syslog_fields(b"Oct 13 10:02:11 web01 sshd[1234]: Accepted publickey for bob");
        let get = |k: &str| f.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());
        assert_eq!(get("host"), Some("web01"));
        assert_eq!(get("app"), Some("sshd"));
        assert_eq!(get("pid"), Some("1234"));
        assert_eq!(get("msg"), Some("Accepted publickey for bob"));
    }

    #[test]
    fn garbage_is_safe() {
        let junk: Vec<u8> = (0..=255u8).collect();
        for fmt in [
            Format::Json,
            Format::Logfmt,
            Format::Nginx,
            Format::Syslog,
            Format::Plain,
        ] {
            let _ = extract(&junk, fmt);
            let _ = get(&junk, fmt, "x");
            let _ = extract(b"", fmt);
        }
    }
}
