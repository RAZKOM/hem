//! End-to-end performance probe used to fill in the README numbers.
//!
//! ```text
//! cargo run --release --example perf -- path/to/big.log [pattern]
//! ```
//!
//! Reports: time to first screen (100 lines indexed), time to a complete
//! line index, index memory, time to first search hit and to a complete
//! search, and time to build a collapsed view (filter + collapse +
//! timeline in one pass).

use std::time::{Duration, Instant};

use hem_core::filter::Matcher;
use hem_core::search::Search;
use hem_core::source::{Source, SourceOptions};
use hem_core::view::{Base, View, ViewSpec};

fn wait(cond: impl Fn() -> bool, max: Duration) -> Duration {
    let start = Instant::now();
    while !cond() {
        if start.elapsed() > max {
            break;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    start.elapsed()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: perf <file> [pattern]");
        std::process::exit(2);
    };
    let pattern = args
        .next()
        .unwrap_or_else(|| "connection reset".to_string());
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("file: {path} ({:.2} GB)", size as f64 / 1e9);

    let t0 = Instant::now();
    let src = Source::open(
        &path,
        SourceOptions {
            follow: false,
            ..Default::default()
        },
    )
    .expect("open");
    let open = t0.elapsed();
    let first_screen = wait(|| src.line_count() >= 100, Duration::from_secs(60));
    println!(
        "open: {:.1} ms · first screen (100 lines indexed): {:.1} ms",
        open.as_secs_f64() * 1e3,
        (open + first_screen).as_secs_f64() * 1e3
    );
    let full = wait(|| src.is_complete(), Duration::from_secs(600));
    let lines = src.line_count();
    let total = open + first_screen + full;
    println!(
        "full index: {:.2} s · {} lines · {:.0} MB/s · index memory {:.0} MB ({:.1} bytes/line)",
        total.as_secs_f64(),
        lines,
        size as f64 / 1e6 / total.as_secs_f64(),
        src.read().index().memory_bytes() as f64 / 1e6,
        src.read().index().memory_bytes() as f64 / lines.max(1) as f64
    );

    // Identity view (timeline + stats pass).
    let base = Base::Single(src.clone());
    let t1 = Instant::now();
    let view = View::start(base.clone(), ViewSpec::default());
    wait(|| view.is_complete(), Duration::from_secs(600));
    println!(
        "identity view (timeline/stats pass): {:.2} s",
        t1.elapsed().as_secs_f64()
    );

    // Search.
    let t2 = Instant::now();
    let search = Search::start(view.clone(), Matcher::new(&pattern, false).unwrap());
    let first_hit = wait(|| search.count() > 0, Duration::from_secs(600));
    wait(|| search.is_complete(), Duration::from_secs(600));
    println!(
        "search {pattern:?}: first hit after {:.1} ms · complete in {:.2} s · {} matching lines · {:.0} MB/s",
        first_hit.as_secs_f64() * 1e3,
        t2.elapsed().as_secs_f64(),
        search.count(),
        size as f64 / 1e6 / t2.elapsed().as_secs_f64()
    );

    // Collapsed view.
    let t3 = Instant::now();
    let collapsed = View::start(
        base.clone(),
        ViewSpec {
            filter: Default::default(),
            collapse: true,
        },
    );
    wait(|| collapsed.is_complete(), Duration::from_secs(600));
    println!(
        "collapsed view: {:.2} s · {} rows from {} lines",
        t3.elapsed().as_secs_f64(),
        collapsed.len(),
        lines
    );

    // Random access: jump anywhere and fetch a screen.
    let t4 = Instant::now();
    let n = view.len();
    let mut total_fetch = Duration::ZERO;
    let mut worst = Duration::ZERO;
    for i in 0..200u64 {
        let row = ((i * 7_919_191) % n.max(1) as u64) as usize;
        let s = Instant::now();
        let chunk = view.chunk(row, row + 60);
        let mut bytes = 0usize;
        for k in 0..chunk.len() {
            bytes += chunk.line(k).len();
        }
        std::hint::black_box(bytes);
        let d = s.elapsed();
        total_fetch += d;
        worst = worst.max(d);
    }
    println!(
        "random screen fetch: avg {:.3} ms · worst {:.3} ms ({} jumps in {:.1} ms)",
        total_fetch.as_secs_f64() * 1e3 / 200.0,
        worst.as_secs_f64() * 1e3,
        200,
        t4.elapsed().as_secs_f64() * 1e3
    );
}
