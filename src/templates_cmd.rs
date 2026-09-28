//! `hem templates <file>`: print mined line templates as a table.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::Args;

use hem_core::cluster::{self, Clusters};
use hem_core::source::{Source, SourceOptions};
use hem_core::view::Base;

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

pub fn run(args: TemplatesArgs, _config: Option<&Path>) -> Result<ExitCode> {
    let format = if args.format == "auto" {
        None
    } else {
        Some(
            args.format
                .parse::<hem_core::format::Format>()
                .map_err(|e| anyhow!("{e}"))?,
        )
    };
    let src = Source::open(
        &args.file,
        SourceOptions {
            follow: false,
            format,
            ..Default::default()
        },
    )
    .map_err(|e| anyhow!("{e}"))?;
    let base = Base::Single(src.clone());
    let cap = if args.cap == 0 {
        usize::MAX / 2
    } else {
        args.cap
    };
    let clusters = Clusters::start(base, cap);
    // Wait for indexing + clustering.
    while !src.is_complete() {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !cluster::wait_complete(&clusters, Duration::from_secs(3600)) {
        return Err(anyhow!("clustering timed out"));
    }
    let mut templates: Vec<_> = clusters.templates().iter().cloned().collect();
    templates.sort_by(|a, b| {
        if args.rare {
            a.count.cmp(&b.count)
        } else {
            b.count.cmp(&a.count)
        }
    });
    let stride = clusters.stride();
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let res = (|| -> io::Result<()> {
        if args.json {
            for t in templates.iter().take(args.top) {
                let levels: Vec<(String, u64)> = hem_core::Level::ALL
                    .iter()
                    .map(|l| (l.as_str().to_lowercase(), t.levels[l.index()]))
                    .filter(|(_, n)| *n > 0)
                    .collect();
                let obj = serde_json_line(
                    t.display().as_str(),
                    t.count * stride,
                    &levels,
                    t.first.line() + 1,
                    t.last.line() + 1,
                );
                writeln!(out, "{obj}")?;
            }
            return Ok(());
        }
        let width = templates
            .iter()
            .take(args.top)
            .map(|t| t.count.to_string().len())
            .max()
            .unwrap_or(5)
            .max(5)
            + 1;
        writeln!(
            out,
            "{:>w$}  {:<6} {:>9} {:>9}  template",
            if stride > 1 { "~count" } else { "count" },
            "level",
            "first",
            "last",
            w = width
        )?;
        for t in templates.iter().take(args.top) {
            let level = t.max_level().map(|l| l.as_str()).unwrap_or("-");
            writeln!(
                out,
                "{:>w$}  {:<6} {:>9} {:>9}  {}",
                t.count * stride,
                level,
                t.first.line() + 1,
                t.last.line() + 1,
                t.display(),
                w = width
            )?;
        }
        if stride > 1 {
            writeln!(
                out,
                "(sampled every {stride} lines of {}; counts are estimates)",
                src.line_count()
            )?;
        }
        Ok(())
    })();
    match res {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(ExitCode::SUCCESS),
        Err(e) => return Err(e.into()),
    }
    out.flush().ok();
    Ok(ExitCode::SUCCESS)
}

fn serde_json_line(
    template: &str,
    count: u64,
    levels: &[(String, u64)],
    first: u64,
    last: u64,
) -> String {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let lv: Vec<String> = levels
        .iter()
        .map(|(k, v)| format!("\"{}\":{v}", esc(k)))
        .collect();
    format!(
        "{{\"template\":\"{}\",\"count\":{count},\"levels\":{{{}}},\"first_line\":{first},\"last_line\":{last}}}",
        esc(template),
        lv.join(",")
    )
}
