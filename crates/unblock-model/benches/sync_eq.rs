//! Informational micro-bench for the `sync_equals` path (import no-op detection at scale).
//!
//! Not an NFR-1 CI regression gate.

// The `criterion_group!`/`criterion_main!` macros generate undocumented public items.
#![allow(missing_docs)]

use chrono::{TimeZone, Utc};
use criterion::{Criterion, criterion_group, criterion_main};
use unblock_model::{Dependency, DependencyType, Issue};

fn issue_with_relations(n: usize) -> Issue {
    let created = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let deps = (0..n)
        .map(|i| Dependency {
            issue_id: "ub-root".to_string(),
            depends_on_id: format!("ub-{i}"),
            dep_type: DependencyType::Blocks,
            created_at: created,
            created_by: None,
            metadata: None,
            thread_id: None,
        })
        .collect();
    Issue {
        id: "ub-root".to_string(),
        title: "Bench".to_string(),
        created_at: created,
        updated_at: created,
        dependencies: deps,
        labels: (0..n).map(|i| format!("l{i}")).collect(),
        ..Issue::default()
    }
}

fn bench_sync_equals(c: &mut Criterion) {
    for n in [0usize, 10, 100] {
        let a = issue_with_relations(n);
        let b = a.clone();
        c.bench_function(&format!("sync_equals/{n}"), |bencher| {
            bencher.iter(|| std::hint::black_box(&a).sync_equals(std::hint::black_box(&b)));
        });
    }
}

criterion_group!(benches, bench_sync_equals);
criterion_main!(benches);
