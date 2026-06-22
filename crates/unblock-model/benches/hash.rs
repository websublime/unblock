//! Informational micro-bench for the content-hash path (hot on import/dedup, FR-26).
//!
//! Not an NFR-1 CI regression gate — just a quick signal on `compute_content_hash` throughput.

// The `criterion_group!`/`criterion_main!` macros generate undocumented public items.
#![allow(missing_docs)]

use chrono::{TimeZone, Utc};
use criterion::{Criterion, criterion_group, criterion_main};
use unblock_model::Issue;

fn small_issue() -> Issue {
    Issue {
        id: "ub-abc123".to_string(),
        title: "Bench issue".to_string(),
        description: Some("a short description".to_string()),
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        ..Issue::default()
    }
}

fn large_issue() -> Issue {
    Issue {
        description: Some("x".repeat(50_000)),
        notes: Some("y".repeat(50_000)),
        ..small_issue()
    }
}

fn bench_content_hash(c: &mut Criterion) {
    let small = small_issue();
    let large = large_issue();

    c.bench_function("content_hash/small", |b| {
        b.iter(|| std::hint::black_box(&small).compute_content_hash());
    });
    c.bench_function("content_hash/large", |b| {
        b.iter(|| std::hint::black_box(&large).compute_content_hash());
    });
}

criterion_group!(benches, bench_content_hash);
criterion_main!(benches);
