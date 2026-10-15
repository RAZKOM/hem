//! Plain mode: print (optionally coloured) lines to stdout, honouring the
//! same filters as the TUI.  Used when stdout is not a terminal or with
//! `--plain`.

use std::io::{self, BufWriter, Write};
use std::process::ExitCode;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use hem_core::filter::{Filter, FilterSpec};
use hem_core::merge::Merged;
use hem_core::parse::parse_line;
use hem_core::source::Source;
use hem_core::view::{Base, View, ViewSpec};
use hem_core::Level;
use hem_tui::ui::line::{segments, Kind};

pub struct PlainOptions {
    pub sources: Vec<Source>,
    pub spec: FilterSpec,
    pub collapse: bool,
    pub follow: bool,
    pub merge: bool,
    pub color: bool,
}

const RESET: &str = "\x1b[0m";

fn ansi(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "",
        Kind::Timestamp => "\x1b[2m",
        Kind::Level(Level::Trace) | Kind::Level(Level::Debug) => "\x1b[90m",
        Kind::Level(Level::Info) => "\x1b[36m",
        Kind::Level(Level::Warn) => "\x1b[33m",
        Kind::Level(Level::Error) => "\x1b[1;31m",
        Kind::Level(Level::Fatal) => "\x1b[1;35m",
        Kind::Key => "\x1b[34m",
        Kind::Str => "\x1b[32m",
        Kind::Number => "\x1b[33m",
        Kind::Url => "\x1b[4;36m",
        Kind::Punct => "\x1b[2m",
        Kind::Match | Kind::CurrentMatch => "\x1b[7m",
        Kind::Highlight => "\x1b[42m",
    }
}

fn write_colored(
    out: &mut impl Write,
    line: &[u8],
    fmt: hem_core::format::Format,
    year: i64,
) -> io::Result<()> {
    let meta = parse_line(line, fmt, year);
    for s in segments(line, &meta, fmt, &[], false, &[]) {
        let code = ansi(s.kind);
        if code.is_empty() {
            out.write_all(&line[s.start..s.end])?;
        } else {
            out.write_all(code.as_bytes())?;
            out.write_all(&line[s.start..s.end])?;
            out.write_all(RESET.as_bytes())?;
        }
    }
    out.write_all(b"\n")
}

pub fn run(opts: PlainOptions) -> Result<ExitCode> {
    match run_inner(opts) {
        Ok(code) => Ok(code),
        Err(e) => {
            // `hem … | head` closes the pipe early; that is not an error.
            if e.downcast_ref::<io::Error>()
                .map(|io| io.kind() == io::ErrorKind::BrokenPipe)
                .unwrap_or(false)
            {
                Ok(ExitCode::SUCCESS)
            } else {
                Err(e)
            }
        }
    }
}

fn run_inner(opts: PlainOptions) -> Result<ExitCode> {
    let filter = Filter::from_spec(&opts.spec)?;
    let bases: Vec<Base> = if opts.merge && opts.sources.len() > 1 {
        vec![Base::Merged(Merged::start(opts.sources.clone()))]
    } else {
        opts.sources
            .iter()
            .map(|s| Base::Single(s.clone()))
            .collect()
    };
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(256 * 1024, stdout.lock());
    let year = hem_core::timestamp::current_year();

    for base in bases {
        let view = View::start(
            base.clone(),
            ViewSpec {
                filter: filter.clone(),
                collapse: opts.collapse,
            },
        );
        let (tx, rx) = mpsc::channel::<()>();
        {
            let tx = tx.clone();
            view.add_waker(Arc::new(move || {
                let _ = tx.send(());
            }));
        }
        let tag_sources = base.is_merged();
        let names: Vec<String> = base
            .sources()
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        let mut printed = 0usize;
        loop {
            // "Complete" before length, so nothing appended in between is
            // skipped when we stop.
            let done = view.is_complete() && base.is_complete();
            let n = view.len();
            while printed < n {
                let end = (printed + 4096).min(n);
                let chunk = view.chunk(printed, end);
                let formats = base.formats();
                for i in 0..chunk.len() {
                    let row = chunk.row(i);
                    let line = chunk.line(i);
                    let fmt = formats
                        .get(row.source() as usize)
                        .copied()
                        .unwrap_or_default();
                    let res = (|| -> io::Result<()> {
                        if tag_sources {
                            if opts.color {
                                write!(
                                    out,
                                    "\x1b[1m{:<10}\x1b[0m ",
                                    names
                                        .get(row.source() as usize)
                                        .map(String::as_str)
                                        .unwrap_or("?")
                                )?;
                            } else {
                                write!(
                                    out,
                                    "{:<10} ",
                                    names
                                        .get(row.source() as usize)
                                        .map(String::as_str)
                                        .unwrap_or("?")
                                )?;
                            }
                        }
                        if row.count() > 1 {
                            if opts.color {
                                write!(out, "\x1b[1;35m×{}\x1b[0m ", row.count())?;
                            } else {
                                write!(out, "×{} ", row.count())?;
                            }
                        }
                        if opts.color {
                            write_colored(&mut out, line, fmt, year)
                        } else {
                            out.write_all(line)?;
                            out.write_all(b"\n")
                        }
                    })();
                    if let Err(e) = res {
                        if e.kind() == io::ErrorKind::BrokenPipe {
                            view.cancel();
                            return Ok(ExitCode::SUCCESS);
                        }
                        return Err(e.into());
                    }
                }
                printed = end.max(printed + chunk.len());
                if chunk.is_empty() {
                    break;
                }
            }
            if done && printed >= n {
                out.flush()?;
                if !opts.follow || !base.is_live() {
                    break;
                }
            }
            if view.is_finished() && printed >= view.len() {
                break;
            }
            let _ = rx.recv_timeout(Duration::from_millis(100));
            while rx.try_recv().is_ok() {}
        }
        view.cancel();
    }
    out.flush().or_else(|e| {
        if e.kind() == io::ErrorKind::BrokenPipe {
            Ok(())
        } else {
            Err(e)
        }
    })?;
    Ok(ExitCode::SUCCESS)
}
