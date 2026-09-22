//! Log levels and keyword detection.

use serde::{Deserialize, Serialize};

/// Severity level, ordered from least to most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Level {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
    Fatal = 5,
}

impl Level {
    pub const COUNT: usize = 6;

    /// Canonical upper-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Fatal => "FATAL",
        }
    }

    /// Parse a level name (case-insensitive, accepts common aliases and
    /// numeric syslog / pino levels).
    pub fn parse(s: &str) -> Option<Level> {
        Self::from_bytes(s.trim().as_bytes())
    }

    pub fn from_bytes(b: &[u8]) -> Option<Level> {
        let mut buf = [0u8; 12];
        if b.is_empty() || b.len() > buf.len() {
            return None;
        }
        for (i, c) in b.iter().enumerate() {
            buf[i] = c.to_ascii_lowercase();
        }
        Some(match &buf[..b.len()] {
            b"trace" | b"trc" | b"verbose" | b"v" | b"t" | b"10" | b"7" => Level::Trace,
            b"debug" | b"dbg" | b"d" | b"20" | b"fine" | b"finer" | b"finest" => Level::Debug,
            b"info" | b"inf" | b"information" | b"notice" | b"i" | b"30" | b"6" | b"5" => {
                Level::Info
            }
            b"warn" | b"warning" | b"wrn" | b"w" | b"40" | b"4" => Level::Warn,
            b"error" | b"err" | b"e" | b"50" | b"3" | b"severe" => Level::Error,
            b"fatal" | b"critical" | b"crit" | b"panic" | b"emerg" | b"emergency" | b"alert"
            | b"f" | b"60" | b"2" | b"1" | b"0" => Level::Fatal,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names() {
        assert_eq!(Level::parse("warn"), Some(Level::Warn));
        assert_eq!(Level::parse("WARNING"), Some(Level::Warn));
        assert_eq!(Level::parse("critical"), Some(Level::Fatal));
        assert_eq!(Level::parse("30"), Some(Level::Info));
        assert_eq!(Level::parse("bogus"), None);
    }

}
