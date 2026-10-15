//! Performance probe for merging many files.
//!
//! ```text
//! cargo run --release --example merge_perf -- <dir> [max-files]
//! ```
//!
//! Opens every `*.log` file in `dir` (up to `max-files`), then reports the
//! time until all are indexed, until the timestamp merge is complete, and
//! until a merged view (and a collapsed merged view) is built.  Generate
//! inputs with `genlog`, e.g. 300 files of 4 MB:
//!
//! ```text
//! for i in $(seq 1 300); do
//!   cargo run --release --example genlog -- --size 4M --seed $i --out many/svc$i.log
//! done
//! ```

use std::time::{Duration, Instant};

use hem_core::merge::Merged;
use hem_core::source::{Source, SourceOptions};
use hem_core::view::{Base, View, ViewSpec};

fn wait(cond: impl Fn() -> bool, max: Duration) -> Duration {
    let start = Instant::now();
    while !cond() {
        if start.elapsed() > max {
            eprintln!("timed out");
            break;
        }
        std::thread::sleep(Duration::from_micros(500));
    }
    start.elapsed()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: merge_perf <dir> [max-files]");
        std::process::exit(2);
    };
    let max: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .collect();
    paths.sort();
    paths.truncate(max);
    let bytes: u64 = paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    println!("{} files, {:.2} GB", paths.len(), bytes as f64 / 1e9);

    let t0 = Instant::now();
    let sources: Vec<Source> = paths
        .iter()
        .map(|p| {
            Source::open(
                p,
                SourceOptions {
                    mmap: std::env::var_os("MMAP").is_some(),
                    ..Default::default()
                },
            )
            .expect("open")
        })
        .collect();
    println!("open all            {:>8.2?}", t0.elapsed());
    wait(
        || sources.iter().all(|s| s.is_complete()),
        Duration::from_secs(600),
    );
    let lines: usize = sources.iter().map(|s| s.line_count()).sum();
    println!(
        "all indexed         {:>8.2?}  ({lines} lines)",
        t0.elapsed()
    );

    let t1 = Instant::now();
    let merged = Merged::start(sources.clone());
    let m2 = merged.clone();
    wait(|| m2.is_complete(), Duration::from_secs(600));
    println!(
        "merge complete      {:>8.2?}  ({} rows)",
        t1.elapsed(),
        merged.len()
    );

    for collapse in [false, true] {
        let t = Instant::now();
        let view = View::start(
            Base::Merged(merged.clone()),
            ViewSpec {
                filter: Default::default(),
                collapse,
            },
        );
        let v2 = view.clone();
        wait(|| v2.is_complete(), Duration::from_secs(600));
        println!(
            "{:<20}{:>8.2?}  ({} rows)",
            if collapse {
                "collapsed view"
            } else {
                "merged view"
            },
            t.elapsed(),
            view.len()
        );
    }
}
