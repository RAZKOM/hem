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
use hem_core::source::{Notice, Source};
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
    let mut ctx = Ctx {
        out: BufWriter::with_capacity(256 * 1024, stdout.lock()),
        color: opts.color,
        collapse: opts.collapse,
        year: hem_core::timestamp::current_year(),
        // Like `tail -f a b`: when several files are followed without
        // merging, a header says which file the next lines come from.
        headers: opts.follow && bases.len() > 1,
        last_writer: None,
        had_error: false,
    };
    let (tx, rx) = mpsc::channel::<()>();
    let mut streams: Vec<Stream> = bases
        .into_iter()
        .map(|base| Stream::new(base, &filter, opts.collapse, &tx))
        .collect();

    // Print each input in turn.  Inputs that are followed stay open and are
    // then followed together, so a second file is not starved by the first.
    let mut following = Vec::new();
    for idx in 0..streams.len() {
        loop {
            let caught_up = streams[idx].pump(idx, &mut ctx)?;
            check_background(&mut ctx)?;
            collect_notices(&streams, &mut ctx, opts.follow);
            let s = &streams[idx];
            if caught_up {
                ctx.out.flush()?;
                if opts.follow && s.base.is_live() {
                    following.push(idx);
                }
                break;
            }
            if s.view.is_finished() && s.printed >= s.view.len() {
                break;
            }
            let _ = rx.recv_timeout(Duration::from_millis(100));
            while rx.try_recv().is_ok() {}
        }
        if !following.contains(&idx) {
            streams[idx].view.cancel();
        }
    }
    while !following.is_empty() {
        let mut wrote = false;
        for &idx in &following {
            let before = ctx.last_writer;
            streams[idx].pump(idx, &mut ctx)?;
            wrote |= ctx.last_writer != before || ctx.last_writer == Some(idx);
        }
        check_background(&mut ctx)?;
        collect_notices(&streams, &mut ctx, true);
        if wrote {
            ctx.out.flush()?;
        }
        following.retain(|&idx| streams[idx].base.is_live());
        let _ = rx.recv_timeout(Duration::from_millis(100));
        while rx.try_recv().is_ok() {}
    }
    for s in &streams {
        s.view.cancel();
    }
    flush_ignoring_broken_pipe(&mut ctx.out)?;
    collect_notices(&streams, &mut ctx, opts.follow);
    Ok(if ctx.had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

struct Ctx<W: Write> {
    out: W,
    color: bool,
    collapse: bool,
    year: i64,
    headers: bool,
    /// The stream whose lines were printed last (for headers).
    last_writer: Option<usize>,
    /// A source reported a read error (a corrupt `.gz`, …).
    had_error: bool,
}

/// One printed input: a file, or the merge of several.
struct Stream {
    base: Base,
    view: View,
    names: Vec<String>,
    tag_sources: bool,
    /// Rows printed so far.
    printed: usize,
    /// The repeat count already printed for row `printed - 1`.
    printed_count: u32,
    /// View generation the counters above refer to.
    generation: u32,
}

impl Stream {
    fn new(base: Base, filter: &Filter, collapse: bool, tx: &mpsc::Sender<()>) -> Stream {
        let view = View::start(
            base.clone(),
            ViewSpec {
                filter: filter.clone(),
                collapse,
            },
        );
        let tx = tx.clone();
        view.add_waker(Arc::new(move || {
            let _ = tx.send(());
        }));
        let names = base
            .sources()
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        Stream {
            tag_sources: base.is_merged(),
            generation: view.generation(),
            base,
            view,
            names,
            printed: 0,
            printed_count: 0,
        }
    }

    /// Print every row that is ready.  Returns true once the view has caught
    /// up with its input and everything in it has been printed.
    fn pump<W: Write>(&mut self, idx: usize, ctx: &mut Ctx<W>) -> io::Result<bool> {
        // "Complete" before length, so nothing appended in between is
        // skipped when we stop.
        let done = self.view.is_complete() && self.base.is_complete();
        let gen = self.view.generation();
        if gen != self.generation {
            // The input was truncated or rotated and the view rebuilt from
            // the new content: start again from its first row.
            self.generation = gen;
            self.printed = 0;
            self.printed_count = 0;
        }
        let n = self.view.len();
        if ctx.collapse && self.printed > 0 && self.printed <= n {
            // Repeats that joined the last printed row after it was printed
            // (follow mode) go out as a row of their own.
            if let Some(row) = self.view.row(self.printed - 1) {
                if row.count() > self.printed_count {
                    let chunk = self.view.chunk(self.printed - 1, self.printed);
                    if self.view.generation() != gen || chunk.is_empty() {
                        return Ok(false);
                    }
                    let extra = row.count() - self.printed_count;
                    self.header(idx, ctx)?;
                    self.write_row(ctx, row.source(), extra, chunk.line(0))?;
                    self.printed_count = row.count();
                }
            }
        }
        // While a collapsing view is still being built its last row can
        // still grow, so it is held back until the view has caught up.
        let ready = if ctx.collapse && !done {
            n.saturating_sub(1)
        } else {
            n
        };
        while self.printed < ready {
            let end = (self.printed + 4096).min(ready);
            let chunk = self.view.chunk(self.printed, end);
            if self.view.generation() != gen {
                // Rebuilt while reading: these rows may be stale.
                return Ok(false);
            }
            if chunk.is_empty() {
                break;
            }
            self.header(idx, ctx)?;
            for i in 0..chunk.len() {
                let row = chunk.row(i);
                self.write_row(ctx, row.source(), row.count(), chunk.line(i))?;
            }
            self.printed += chunk.len();
            self.printed_count = chunk.row(chunk.len() - 1).count();
        }
        Ok(done && self.printed >= n)
    }

    fn header<W: Write>(&self, idx: usize, ctx: &mut Ctx<W>) -> io::Result<()> {
        if ctx.headers && ctx.last_writer != Some(idx) {
            if ctx.last_writer.is_some() {
                ctx.out.write_all(b"\n")?;
            }
            writeln!(ctx.out, "==> {} <==", self.base.name())?;
        }
        ctx.last_writer = Some(idx);
        Ok(())
    }

    fn write_row<W: Write>(
        &self,
        ctx: &mut Ctx<W>,
        source: hem_core::view::SourceId,
        count: u32,
        line: &[u8],
    ) -> io::Result<()> {
        let out = &mut ctx.out;
        if self.tag_sources {
            let name = self
                .names
                .get(source as usize)
                .map(String::as_str)
                .unwrap_or("?");
            if ctx.color {
                write!(out, "\x1b[1m{name:<10}\x1b[0m ")?;
            } else {
                write!(out, "{name:<10} ")?;
            }
        }
        if count > 1 {
            if ctx.color {
                write!(out, "\x1b[1;35m×{count}\x1b[0m ")?;
            } else {
                write!(out, "×{count} ")?;
            }
        }
        if ctx.color {
            let fmt = self.view.format_for(source);
            write_colored(out, line, fmt, ctx.year)
        } else {
            out.write_all(line)?;
            out.write_all(b"\n")
        }
    }
}

/// Fail instead of printing partial output when a background job crashed
/// (see `hem_core::crash`).
fn check_background<W: Write>(ctx: &mut Ctx<W>) -> Result<()> {
    if let Some(msg) = hem_core::crash::failure() {
        flush_ignoring_broken_pipe(&mut ctx.out)?;
        anyhow::bail!("internal error in {msg}; the output above is incomplete");
    }
    Ok(())
}

/// Report read errors from the sources on stderr (they make hem exit 1).
/// With `report_rotation`, also report truncations and rotations, as
/// `tail -F` does.
fn collect_notices<W: Write>(streams: &[Stream], ctx: &mut Ctx<W>, report_rotation: bool) {
    for s in streams {
        for src in s.base.sources() {
            for n in src.take_notices() {
                match n {
                    Notice::Error(e) => {
                        ctx.had_error = true;
                        let _ = ctx.out.flush();
                        eprintln!("hem: {}: {e}", src.name());
                    }
                    Notice::Truncated if report_rotation => {
                        eprintln!("hem: {}: file truncated", src.name())
                    }
                    Notice::Rotated if report_rotation => {
                        eprintln!("hem: {}: file rotated, following the new file", src.name())
                    }
                    Notice::Removed if report_rotation => {
                        eprintln!("hem: {}: file removed", src.name())
                    }
                    _ => {}
                }
            }
        }
    }
}

fn flush_ignoring_broken_pipe(out: &mut impl Write) -> io::Result<()> {
    out.flush().or_else(|e| {
        if e.kind() == io::ErrorKind::BrokenPipe {
            Ok(())
        } else {
            Err(e)
        }
    })
}
