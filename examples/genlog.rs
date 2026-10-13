//! Synthetic log generator for tests and benchmarks.
//!
//! ```text
//! cargo run --release --example genlog -- --size 1G --out testdata/big-1g.log
//! cargo run --release --example genlog -- --lines 2000 --out testdata/sample.log
//! cargo run --release --example genlog -- --size 5G --format json --out big.jsonl
//! ```
//!
//! Produces a realistic mix: a plain application log with timestamps and
//! levels, error bursts, long runs of repeated lines, multi-line stack
//! traces, the odd JSON / logfmt line from a sidecar, and occasional
//! nginx access-log lines.  Deterministic for a given seed.

use std::fs::File;
use std::io::{BufWriter, Write};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

fn parse_size(s: &str) -> u64 {
    let s = s.trim().to_ascii_uppercase();
    let (num, mult) = if let Some(n) = s.strip_suffix('G') {
        (n, 1u64 << 30)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, 1u64 << 20)
    } else if let Some(n) = s.strip_suffix('K') {
        (n, 1u64 << 10)
    } else {
        (s.as_str(), 1)
    };
    (num.parse::<f64>().unwrap_or(0.0) * mult as f64) as u64
}

const USERS: &[&str] = &[
    "bob", "alice", "carol", "dave", "erin", "frank", "grace", "heidi",
];
const SERVICES: &[&str] = &["api", "auth", "billing", "search", "worker", "scheduler"];
const PATHS: &[&str] = &[
    "/",
    "/api/v1/users",
    "/api/v1/orders",
    "/health",
    "/login",
    "/static/app.js",
    "/api/v1/search?q=hem",
];
const AGENTS: &[&str] = &[
    "curl/8.4.0",
    "Mozilla/5.0 (Windows NT 10.0) Chrome/128.0",
    "python-requests/2.32",
    "Go-http-client/2.0",
];
const INFO_MSGS: &[&str] = &[
    "request completed",
    "cache hit",
    "cache miss, fetching from origin",
    "user logged in",
    "session refreshed",
    "job scheduled",
    "job finished",
    "config reloaded",
    "connection pool resized",
    "health check ok",
];
const WARN_MSGS: &[&str] = &[
    "slow query",
    "retrying request",
    "rate limit approaching",
    "deprecated endpoint called",
    "certificate expires soon",
];
const ERROR_MSGS: &[&str] = &[
    "connection reset by peer",
    "upstream timeout",
    "database connection refused",
    "failed to acquire lock",
    "null pointer dereference in handler",
    "disk quota exceeded",
];

fn ts(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.{millis:03}",
        sod / 3600,
        (sod / 60) % 60,
        sod % 60
    )
}

fn clf_ts(ms: i64) -> String {
    let iso = ts(ms);
    // 2026-10-13 10:02:11.204 -> 13/Oct/2026:10:02:11 +0000
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m: usize = iso[5..7].parse().unwrap_or(1);
    format!(
        "{}/{}/{}:{} +0000",
        &iso[8..10],
        months[m - 1],
        &iso[0..4],
        &iso[11..19]
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut out = "testdata/generated.log".to_string();
    let mut size: u64 = 0;
    let mut lines: u64 = 0;
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut format = "mixed".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out" | "-o" => {
                out = args[i + 1].clone();
                i += 1;
            }
            "--size" => {
                size = parse_size(&args[i + 1]);
                i += 1;
            }
            "--lines" => {
                lines = args[i + 1].parse().unwrap_or(0);
                i += 1;
            }
            "--seed" => {
                seed = args[i + 1].parse().unwrap_or(seed);
                i += 1;
            }
            "--format" => {
                format = args[i + 1].clone();
                i += 1;
            }
            "-h" | "--help" => {
                eprintln!("usage: genlog [--size 1G | --lines N] [--format mixed|plain|json|logfmt|nginx] [--seed N] --out FILE");
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if size == 0 && lines == 0 {
        lines = 10_000;
    }
    let file = File::create(&out).unwrap_or_else(|e| {
        eprintln!("cannot create {out}: {e}");
        std::process::exit(1)
    });
    let mut w = BufWriter::with_capacity(4 << 20, file);
    let mut rng = Rng(seed | 1);
    let mut now_ms: i64 = 1_791_885_600_000; // 2026-10-13 09:00:00 UTC
    let mut written: u64 = 0;
    let mut count: u64 = 0;
    let mut burst_left = 0u32;
    let mut repeat_left = 0u32;
    let mut repeat_line = String::new();
    let start = std::time::Instant::now();

    let emit = |w: &mut BufWriter<File>, line: &str, written: &mut u64, count: &mut u64| {
        w.write_all(line.as_bytes()).expect("write");
        w.write_all(b"\n").expect("write");
        *written += line.len() as u64 + 1;
        *count += 1;
    };

    loop {
        if (size > 0 && written >= size) || (lines > 0 && count >= lines) {
            break;
        }
        now_ms += rng.below(900) as i64 + 1;
        let t = ts(now_ms);
        let svc = rng.pick(SERVICES);
        let user = rng.pick(USERS);
        let fmt = match format.as_str() {
            "mixed" => {
                let r = rng.below(100);
                if r < 80 {
                    "plain"
                } else if r < 90 {
                    "json"
                } else if r < 96 {
                    "logfmt"
                } else {
                    "nginx"
                }
            }
            f => f,
        };
        if repeat_left > 0 {
            repeat_left -= 1;
            let l = repeat_line
                .replace("{ts}", &t)
                .replace("{n}", &rng.below(9999).to_string());
            emit(&mut w, &l, &mut written, &mut count);
            continue;
        }
        if burst_left == 0 && rng.chance(1) {
            burst_left = 50 + rng.below(400) as u32;
        }
        let level = if burst_left > 0 {
            burst_left -= 1;
            if rng.chance(70) {
                "ERROR"
            } else {
                "WARN"
            }
        } else {
            let r = rng.below(1000);
            if r < 2 {
                "FATAL"
            } else if r < 20 {
                "ERROR"
            } else if r < 80 {
                "WARN"
            } else if r < 700 {
                "INFO"
            } else if r < 950 {
                "DEBUG"
            } else {
                "TRACE"
            }
        };
        let msg = match level {
            "ERROR" | "FATAL" => rng.pick(ERROR_MSGS),
            "WARN" => rng.pick(WARN_MSGS),
            _ => rng.pick(INFO_MSGS),
        };
        let dur = rng.below(2000);
        let req = format!("req-{:08x}", rng.next() as u32);
        let line = match fmt {
            "json" => format!(
                r#"{{"ts":"{}","level":"{}","service":"{}","msg":"{}","user":"{}","duration_ms":{},"request_id":"{}","status":{}}}"#,
                t.replace(' ', "T") + "Z",
                level.to_lowercase(),
                svc,
                msg,
                user,
                dur,
                req,
                if level == "ERROR" {
                    500 + rng.below(4)
                } else {
                    200
                }
            ),
            "logfmt" => format!(
                "ts={} level={} service={} msg=\"{}\" user={} duration_ms={} request_id={}",
                t.replace(' ', "T") + "Z",
                level.to_lowercase(),
                svc,
                msg,
                user,
                dur,
                req
            ),
            "nginx" => format!(
                "10.0.{}.{} - {} [{}] \"GET {} HTTP/1.1\" {} {} \"-\" \"{}\"",
                rng.below(255),
                rng.below(255),
                user,
                clf_ts(now_ms),
                rng.pick(PATHS),
                if level == "ERROR" {
                    502
                } else if level == "WARN" {
                    404
                } else {
                    200
                },
                rng.below(100_000),
                rng.pick(AGENTS)
            ),
            _ => format!(
                "{} {:<5} [{}] {} user={} duration={}ms request_id={}",
                t, level, svc, msg, user, dur, req
            ),
        };
        emit(&mut w, &line, &mut written, &mut count);

        // Stack trace after some errors.
        if (level == "ERROR" || level == "FATAL") && fmt == "plain" && rng.chance(30) {
            let frames = 3 + rng.below(12);
            emit(
                &mut w,
                "java.lang.IllegalStateException: upstream failure",
                &mut written,
                &mut count,
            );
            for f in 0..frames {
                let l = format!(
                    "\tat com.example.{}.{}Handler.handle({}Handler.java:{})",
                    svc,
                    svc,
                    svc,
                    100 + f * 7 + rng.below(50)
                );
                emit(&mut w, &l, &mut written, &mut count);
            }
            emit(&mut w, "\t... 12 more", &mut written, &mut count);
        }
        // Runs of repeats.
        if fmt == "plain" && rng.chance(2) {
            repeat_left = 5 + rng.below(400) as u32;
            repeat_line = format!("{{ts}} {:<5} [{}] {} (attempt {{n}})", level, svc, msg);
        }
    }
    w.flush().expect("flush");
    let el = start.elapsed();
    eprintln!(
        "wrote {} lines, {} bytes to {} in {:.1}s ({:.0} MB/s)",
        count,
        written,
        out,
        el.as_secs_f64(),
        written as f64 / 1e6 / el.as_secs_f64().max(0.001)
    );
}
