//! hem-core: the terminal-independent engine behind `hem`.
//!
//! Everything here is usable without a terminal: memory-mapped sources,
//! background line indexing, follow mode, lazy line parsing, search and
//! filters, repeat collapsing, template clustering, timelines and
//! multi-source merging.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod cluster;
pub mod collapse;
pub mod error;
pub mod fields;
pub mod filter;
pub mod format;
pub mod index;
pub mod level;
pub mod merge;
pub mod par;
pub mod parse;
pub mod search;
pub mod source;
pub mod timeline;
pub mod timestamp;
pub mod view;

pub use error::{Error, Result};
pub use level::Level;
pub use source::{Source, SourceGuard, SourceOptions};
