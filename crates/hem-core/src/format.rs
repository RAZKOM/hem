//! Line format detection.

use serde::{Deserialize, Serialize};

/// The structural format of a source's lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// JSON object per line.
    Json,
    /// `key=value key2="quoted value"`.
    Logfmt,
    /// nginx / Apache combined access log.
    Nginx,
    /// RFC 3164 / 5424 syslog.
    Syslog,
    /// Anything else.
    #[default]
    Plain,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Logfmt => "logfmt",
            Format::Nginx => "nginx",
            Format::Syslog => "syslog",
            Format::Plain => "plain",
        }
    }

    /// Whether this format exposes structured fields.
    pub fn has_fields(self) -> bool {
        matches!(
            self,
            Format::Json | Format::Logfmt | Format::Nginx | Format::Syslog
        )
    }

    /// Guess the format from a sample of lines.  Requires a clear majority
    /// so that a few JSON lines inside a plain log do not flip the whole
    /// source.
    pub fn detect(sample: &[&[u8]]) -> Format {
        let lines: Vec<&[u8]> = sample
            .iter()
            .copied()
            .filter(|l| !l.iter().all(|b| b.is_ascii_whitespace()))
            .collect();
        if lines.is_empty() {
            return Format::Plain;
        }
        let n = lines.len();
        let mut json = 0;
        let mut logfmt = 0;
        let mut nginx = 0;
        let mut syslog = 0;
        for l in &lines {
            if looks_json(l) {
                json += 1;
            } else if looks_nginx(l) {
                nginx += 1;
            } else if looks_syslog(l) {
                syslog += 1;
            } else if looks_logfmt(l) {
                logfmt += 1;
            }
        }
        let majority = |c: usize| c * 2 > n;
        if majority(json) {
            Format::Json
        } else if majority(nginx) {
            Format::Nginx
        } else if majority(syslog) {
            Format::Syslog
        } else if majority(logfmt) {
            Format::Logfmt
        } else {
            Format::Plain
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Format {
    type Err = crate::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "json" | "jsonl" | "ndjson" => Format::Json,
            "logfmt" => Format::Logfmt,
            "nginx" | "apache" | "clf" | "combined" => Format::Nginx,
            "syslog" => Format::Syslog,
            "plain" | "text" => Format::Plain,
            other => return Err(crate::Error::UnknownFormat(other.to_string())),
        })
    }
}

fn trim(l: &[u8]) -> &[u8] {
    let s = l
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(l.len());
    let e = l
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(s);
    &l[s..e.max(s)]
}

pub(crate) fn looks_json(l: &[u8]) -> bool {
    let t = trim(l);
    t.len() >= 2 && t[0] == b'{' && t[t.len() - 1] == b'}'
}

pub(crate) fn looks_logfmt(l: &[u8]) -> bool {
    // Count `key=` tokens at token starts; need at least two and the first
    // token must be one of them.
    let mut pairs = 0;
    let mut first_ok = false;
    let mut i = 0;
    let mut first = true;
    while i < l.len() {
        while i < l.len() && l[i] == b' ' {
            i += 1;
        }
        let start = i;
        let mut key_ok = false;
        while i < l.len() && l[i] != b' ' {
            if l[i] == b'=' && i > start && !key_ok {
                let key = &l[start..i];
                key_ok = key
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'@'));
                if key_ok {
                    pairs += 1;
                    if first {
                        first_ok = true;
                    }
                }
            }
            if l[i] == b'"' {
                // skip quoted value
                i += 1;
                while i < l.len() && l[i] != b'"' {
                    if l[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            i += 1;
        }
        first = false;
    }
    pairs >= 2 && first_ok
}

pub(crate) fn looks_nginx(l: &[u8]) -> bool {
    // <addr> <ident> <user> [<time>] "<request>" <status> ...
    let mut fields = 0;
    let mut i = 0;
    while fields < 3 {
        while i < l.len() && l[i] != b' ' {
            i += 1;
        }
        if i >= l.len() {
            return false;
        }
        fields += 1;
        i += 1;
    }
    l.get(i) == Some(&b'[')
        && l[i..]
            .iter()
            .position(|&b| b == b']')
            .map(|p| l.get(i + p + 2) == Some(&b'"'))
            .unwrap_or(false)
}

pub(crate) fn looks_syslog(l: &[u8]) -> bool {
    if l.first() == Some(&b'<') {
        // RFC 5424 / 3164 with PRI.
        if let Some(close) = l.iter().take(6).position(|&b| b == b'>') {
            return close >= 2 && l[1..close].iter().all(|b| b.is_ascii_digit());
        }
    }
    // "Oct 13 10:02:11 host app[pid]: msg"
    if l.len() < 16 {
        return false;
    }
    let month = l[0].is_ascii_uppercase() && l[1].is_ascii_lowercase() && l[2].is_ascii_lowercase();
    month
        && l[3] == b' '
        && (l[4].is_ascii_digit() || l[4] == b' ')
        && l[5].is_ascii_digit()
        && l[6] == b' '
        && l[7].is_ascii_digit()
        && l[9] == b':'
        && l[12] == b':'
        && l[15] == b' '
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        let json: Vec<&[u8]> = vec![br#"{"a":1}"#, br#"{"b":2}"#, b"garbage"];
        assert_eq!(Format::detect(&json), Format::Json);
        let logfmt: Vec<&[u8]> = vec![
            b"ts=1 level=info msg=\"hi there\"",
            b"ts=2 level=warn msg=x",
        ];
        assert_eq!(Format::detect(&logfmt), Format::Logfmt);
        let nginx: Vec<&[u8]> =
            vec![br#"1.2.3.4 - - [13/Oct/2026:13:55:36 -0500] "GET / HTTP/1.1" 200 12 "-" "curl""#];
        assert_eq!(Format::detect(&nginx), Format::Nginx);
        let syslog: Vec<&[u8]> = vec![
            b"Oct 13 10:02:11 host app[1]: hi",
            b"<34>Oct 11 22:14:15 x y: z",
        ];
        assert_eq!(Format::detect(&syslog), Format::Syslog);
        let plain: Vec<&[u8]> = vec![b"2026-10-13 10:02:11 INFO hello", b"x=1"];
        assert_eq!(Format::detect(&plain), Format::Plain);
        assert_eq!(Format::detect(&[]), Format::Plain);
    }

    #[test]
    fn parses_names() {
        assert_eq!("JSON".parse::<Format>().ok(), Some(Format::Json));
        assert!("xml".parse::<Format>().is_err());
    }
}
