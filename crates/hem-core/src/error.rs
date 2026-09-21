use std::path::PathBuf;

/// Errors produced by hem-core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is a directory")]
    IsDirectory { path: PathBuf },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid regex `{pattern}`: {message}")]
    Regex { pattern: String, message: String },
    #[error("invalid filter expression `{expr}`: {message}")]
    FilterExpr { expr: String, message: String },
    #[error("unknown level `{0}`")]
    UnknownLevel(String),
    #[error("unknown format `{0}` (expected auto, json, logfmt, syslog, nginx or plain)")]
    UnknownFormat(String),
    #[error("cannot parse time `{0}`")]
    BadTime(String),
}

pub type Result<T> = std::result::Result<T, Error>;
