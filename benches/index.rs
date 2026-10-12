//! Line indexing and per-line parsing throughput.
//!
//! Run with `cargo bench --bench index`.  Set `HEM_BENCH_FILE=path` to
//! benchmark indexing a real (large) file instead of the in-memory sample.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};

use hem_core::format::Format;
use hem_core::index::{scan_offsets, LineIndex};
use hem_core::parse::parse_line;
use hem_core::source::{Source, SourceOptions};

fn synthetic(bytes: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(bytes + 128);
    let mut i = 0u64;
    while v.len() < bytes {
        let level = match i % 97 {
            0 => "ERROR",
            1..=5 => "WARN",
            6..=20 => "DEBUG",
            _ => "INFO",
        };
        v.extend_from_slice(
            format!(
                "2026-10-13 10:{:02}:{:02}.{:03} {level:<5} [api] request completed user=bob duration={}ms request_id=req-{:08x}\n",
                (i / 60) % 60,
                i % 60,
                i % 1000,
                i % 2000,
                i.wrapping_mul(2654435761) as u32
            )
            .as_bytes(),
        );
        i += 1;
    }
    v
}

fn bench_scan(c: &mut Criterion) {
    let data = synthetic(64 << 20);
    let mut g = c.benchmark_group("index");
    g.throughput(Throughput::Bytes(data.len() as u64));
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(8));
    g.bench_function("scan_offsets_64MB", |b| {
        b.iter(|| scan_offsets(&data, 0).len());
    });
    g.bench_function("line_index_extend_64MB", |b| {
        b.iter_batched(
            LineIndex::new,
            |mut idx| {
                idx.extend_from(&data);
                idx.line_count()
            },
            BatchSize::LargeInput,
        );
    });
    g.finish();
}

fn bench_parse(c: &mut Criterion) {
    let data = synthetic(8 << 20);
    let lines: Vec<&[u8]> = data
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    let mut g = c.benchmark_group("parse");
    g.throughput(Throughput::Elements(lines.len() as u64));
    g.bench_function("parse_line_plain", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for l in &lines {
                let m = parse_line(l, Format::Plain, 2026);
                n += m.level.is_some() as usize + m.ts.is_some() as usize;
            }
            n
        });
    });
    let json: Vec<Vec<u8>> = (0..lines.len())
        .map(|i| {
            format!(
                r#"{{"ts":"2026-10-13T10:00:{:02}Z","level":"info","msg":"request completed","user":"bob","n":{i}}}"#,
                i % 60
            )
            .into_bytes()
        })
        .collect();
    g.bench_function("parse_line_json", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for l in &json {
                let m = parse_line(l, Format::Json, 2026);
                n += m.level.is_some() as usize + m.ts.is_some() as usize;
            }
            n
        });
    });
    g.bench_function("shape_hash", |b| {
        b.iter(|| {
            let mut h = 0u64;
            for l in &lines {
                h ^= hem_core::collapse::shape_hash(l);
            }
            h
        });
    });
    g.finish();
}

fn bench_file(c: &mut Criterion) {
    let Some(path) = std::env::var_os("HEM_BENCH_FILE") else {
        return;
    };
    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut g = c.benchmark_group("file");
    g.throughput(Throughput::Bytes(len));
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(30));
    g.bench_function("open_and_index_file", |b| {
        b.iter(|| {
            let src = Source::open(
                &path,
                SourceOptions {
                    follow: false,
                    ..Default::default()
                },
            )
            .expect("open");
            while !src.is_complete() {
                std::thread::sleep(Duration::from_millis(1));
            }
            src.line_count()
        });
    });
    g.finish();
}

criterion_group!(benches, bench_scan, bench_parse, bench_file);
criterion_main!(benches);
