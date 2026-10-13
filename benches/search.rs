//! Search and view-building throughput over an in-memory source.
//!
//! Run with `cargo bench --bench search`.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

use hem_core::filter::{Filter, FilterSpec, Matcher};
use hem_core::search::{self, Search};
use hem_core::source::Source;
use hem_core::view::{self, Base, View, ViewSpec};
use hem_core::Level;

fn synthetic(bytes: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(bytes + 128);
    let mut i = 0u64;
    while v.len() < bytes {
        let (level, msg) = match i % 97 {
            0 => ("ERROR", "connection reset by peer"),
            1..=5 => ("WARN", "slow query"),
            6..=20 => ("DEBUG", "cache miss"),
            _ => ("INFO", "request completed"),
        };
        v.extend_from_slice(
            format!(
                "2026-10-13 10:{:02}:{:02}.{:03} {level:<5} [api] {msg} user=bob duration={}ms request_id=req-{:08x}\n",
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

fn bench_search(c: &mut Criterion) {
    let data = synthetic(256 << 20);
    let len = data.len() as u64;
    let src = Source::from_bytes("bench", data);
    src.stop();
    let base = Base::Single(src);
    let identity = View::start(base.clone(), ViewSpec::default());
    assert!(view::wait_complete(&identity, Duration::from_secs(120)));

    let mut g = c.benchmark_group("search");
    g.throughput(Throughput::Bytes(len));
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(15));
    for (name, pat) in [
        ("literal", "connection reset"),
        ("regex", r"req-[0-9a-f]{4}00"),
        ("case_insensitive", "slow query"),
    ] {
        g.bench_function(format!("identity_{name}"), |b| {
            b.iter(|| {
                let s = Search::start(identity.clone(), Matcher::new(pat, false).unwrap());
                assert!(search::wait_complete(&s, Duration::from_secs(120)));
                s.count()
            });
        });
    }
    g.finish();

    let mut g = c.benchmark_group("view");
    g.throughput(Throughput::Bytes(len));
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(20));
    g.bench_function("build_identity_256MB", |b| {
        b.iter(|| {
            let v = View::start(base.clone(), ViewSpec::default());
            assert!(view::wait_complete(&v, Duration::from_secs(120)));
            v.len()
        });
    });
    g.bench_function("build_level_filter_256MB", |b| {
        b.iter(|| {
            let v = View::start(
                base.clone(),
                ViewSpec {
                    filter: Filter::from_spec(&FilterSpec {
                        level: Some(Level::Warn),
                        ..Default::default()
                    })
                    .unwrap(),
                    collapse: false,
                },
            );
            assert!(view::wait_complete(&v, Duration::from_secs(120)));
            v.len()
        });
    });
    g.bench_function("build_collapse_256MB", |b| {
        b.iter(|| {
            let v = View::start(
                base.clone(),
                ViewSpec {
                    filter: Filter::default(),
                    collapse: true,
                },
            );
            assert!(view::wait_complete(&v, Duration::from_secs(120)));
            v.len()
        });
    });
    g.finish();
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
