//! Criterion benches for the perf-sensitive ready-sort comparator (plan §3; NFR-1).
//!
//! Sorts 10k and 250k synthetic candidates by [`cmp_ready`]. The comparator must be a negligible
//! fraction of the storage `ready` budget (ready 1k <5ms / 10k <50ms), so this is the policy-layer
//! signal that the hybrid re-rank is cheap. Fully **synchronous** (no async helpers) — policy is
//! pure CPU.

// The `criterion_group!`/`criterion_main!` macros generate undocumented public items.
#![allow(missing_docs)]

use chrono::{TimeZone, Utc};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use unblock_model::{Issue, Priority};
use unblock_policy::cmp_ready;

/// Build `n` deterministic candidates spread across all five priorities and a wide `created_at`
/// range, with unique ids — exercising both buckets, age ordering, and the id tie-break.
fn candidates(n: usize) -> Vec<Issue> {
    let base = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    (0..n)
        .map(|i| {
            // A multiplicative step + modulus de-correlates priority from id order, so the input is
            // genuinely unsorted (worst case for the comparator-driven sort).
            let prio = i32::try_from((i * 7) % 5).unwrap_or(0);
            let secs = i64::try_from((i * 31) % 1_000_000).unwrap_or(0);
            Issue {
                id: format!("ub-{i:08}"),
                priority: Priority(prio),
                created_at: base + chrono::Duration::seconds(secs),
                ..Issue::default()
            }
        })
        .collect()
}

fn bench_cmp_ready_sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("cmp_ready_sort");
    for &n in &[10_000_usize, 250_000] {
        let data = candidates(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &data, |b, data| {
            b.iter_batched(
                || data.clone(),
                |mut candidates| {
                    candidates.sort_by(cmp_ready);
                    std::hint::black_box(candidates)
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_cmp_ready_sort);
criterion_main!(benches);
