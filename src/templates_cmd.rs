//! `hem templates <file>`: print mined line templates as a table.

use std::io::Write;
use std::path::PathBuf;

use clap::Args;

#[derive(Args, Debug, Clone)]
pub struct TemplatesArgs {
    /// The log file to mine.
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// Number of templates to print.
    #[arg(short = 'n', long, default_value_t = 30)]
    top: usize,

    /// Sort rarest first instead of most common first.
    #[arg(long)]
    rare: bool,

    /// Maximum lines to sample (0 = all).
    #[arg(long, default_value_t = 2_000_000)]
    cap: usize,

    /// Line format: auto, json, logfmt, syslog, nginx, plain.
    #[arg(long, default_value = "auto")]
    format: String,

    /// Print as JSON lines instead of a table.
    #[arg(long)]
    json: bool,
}

