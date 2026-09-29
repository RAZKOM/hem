//! `hem`: Human-readable Event Monitor.  *tail, but tailored.*

mod plain;
mod templates_cmd;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{ArgAction, Args, Parser, Subcommand};

use hem_core::Level;

/// hem: Human-readable Event Monitor.  tail, but tailored.
#[derive(Parser, Debug)]
#[command(name = "hem", version, about, long_about = None, arg_required_else_help = false)]
#[command(after_help = "EXAMPLES:
    hem app.log                       open a file in the TUI
    hem app.log nginx.log --merge     interleave several files by timestamp
    hem 'logs/*.log'                  globs are expanded by hem (quote them)
    cat app.log | hem                 read from stdin
    hem app.log --level error --plain | wc -l
    hem app.log --since 15m --filter timeout --follow
    hem templates app.log             print the most common line templates")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    view: ViewArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print the most common line templates of a file as a table.
    Templates(templates_cmd::TemplatesArgs),
}

#[derive(Args, Debug, Clone)]
struct ViewArgs {
    /// Files or globs to open.  Reads stdin when omitted and stdin is a pipe.
    #[arg(value_name = "FILES")]
    files: Vec<String>,

    /// Show only lines matching this regex (repeatable; all must match).
    #[arg(long = "filter", value_name = "REGEX", action = ArgAction::Append)]
    filter: Vec<String>,

    /// Hide lines matching this regex (repeatable).
    #[arg(short = 'x', long = "exclude", value_name = "REGEX", action = ArgAction::Append)]
    exclude: Vec<String>,

    /// Structured-field predicate, e.g. status>=500 or user=bob (repeatable).
    #[arg(long = "field", value_name = "EXPR", action = ArgAction::Append)]
    field: Vec<String>,

    /// Minimum level: trace, debug, info, warn, error, fatal.
    #[arg(short = 'l', long = "level", value_name = "LEVEL")]
    level: Option<Level>,

    /// Only lines at or after this time (timestamp, or relative like 15m).
    #[arg(long, value_name = "TIME")]
    since: Option<String>,

    /// Only lines at or before this time.
    #[arg(long, value_name = "TIME")]
    until: Option<String>,

    /// Treat --filter / --exclude patterns as literal text.
    #[arg(long)]
    literal: bool,

    /// Apply a filter preset from the config file.
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,

    /// Follow the file(s) as they grow (like tail -F).
    #[arg(short = 'f', long)]
    follow: bool,

    /// Collapse runs of repeated lines (default from config; on).
    #[arg(long, overrides_with = "no_collapse")]
    collapse: bool,

    /// Do not collapse repeated lines.
    #[arg(long, overrides_with = "collapse")]
    no_collapse: bool,

    /// Merge several files into one stream ordered by timestamp.
    #[arg(short = 'm', long)]
    merge: bool,

    /// Line format: auto, json, logfmt, syslog, nginx, plain, or a custom
    /// format name from the config.
    #[arg(long, value_name = "FORMAT", default_value = "auto")]
    format: String,

    /// Plain mode: print lines instead of starting the TUI (implied when
    /// stdout is not a terminal).
    #[arg(short = 'p', long)]
    plain: bool,

    /// Colour theme: dark or light.
    #[arg(long, value_name = "THEME")]
    theme: Option<String>,

    /// Use ASCII box drawing and symbols.
    #[arg(long)]
    ascii: bool,

    /// Disable mouse support.
    #[arg(long)]
    no_mouse: bool,

    /// Config file path (default: platform config dir).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Write hem's own debug log to this file (default: ./hem-debug.log).
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = "hem-debug.log")]
    debug: Option<PathBuf>,

    /// Force colour output in plain mode even when piped.
    #[arg(long)]
    color: bool,
}

/// Expand glob patterns (shells on Windows do not).  Plain paths pass
/// through unchanged so that a missing file produces a clear error later.
pub fn expand_globs(args: &[String]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for a in args {
        if a.contains(['*', '?', '[']) {
            let mut matched = 0;
            for entry in glob::glob(a).with_context(|| format!("bad glob {a:?}"))? {
                match entry {
                    Ok(p) if p.is_file() => {
                        out.push(p);
                        matched += 1;
                    }
                    Ok(_) => {}
                    Err(e) => return Err(anyhow!("{a}: {e}")),
                }
            }
            if matched == 0 {
                return Err(anyhow!("no files match {a:?}"));
            }
        } else {
            out.push(PathBuf::from(a));
        }
    }
    Ok(out)
}

fn main() {
    println!("Hello, world!");
}
