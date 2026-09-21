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

