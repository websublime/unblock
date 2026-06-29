//! Criterion benches for the hot render paths (NFR-1 budget visibility).
//!
//! Renders 1k / 10k issues across every v1 format. Render is pure CPU (no async), so these are
//! synchronous benches. Baseline + 10% regression gate per the crate plan §4.

// The `criterion_group!`/`criterion_main!` macros generate undocumented public items.
#![allow(missing_docs)]

use chrono::{TimeZone, Utc};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use unblock_model::{Issue, OutputFormat};
use unblock_render::{RenderOptions, renderer_for};

fn make_issues(n: usize) -> Vec<Issue> {
    let created = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    (0..n)
        .map(|i| Issue {
            id: format!("ub-{i:06}"),
            title: format!("Issue number {i} with a reasonably long title"),
            created_at: created,
            updated_at: created,
            ..Issue::default()
        })
        .collect()
}

fn bench_issues(c: &mut Criterion) {
    let formats = [
        ("json", OutputFormat::Json),
        ("robot", OutputFormat::Robot),
        ("plain", OutputFormat::Plain),
        ("csv", OutputFormat::Csv),
        ("markdown", OutputFormat::Markdown),
    ];

    let mut group = c.benchmark_group("render_issues");
    for &count in &[1_000usize, 10_000] {
        let issues = make_issues(count);
        for (name, fmt) in formats {
            let renderer = renderer_for(fmt, RenderOptions::default());
            group.bench_with_input(BenchmarkId::new(name, count), &issues, |b, issues| {
                b.iter(|| {
                    let out = renderer
                        .issues(black_box(issues), &RenderOptions::default())
                        .expect("bench fixtures render");
                    black_box(out);
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_issues);
criterion_main!(benches);
