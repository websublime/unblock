//! Informational criterion micro-bench for the error-construction path
//! (`from_code` → sanitize → serialize). Not wired to the NFR-1 regression gate — the error path
//! is not in the budget table; this exists for visibility only.

// The `criterion_group!`/`criterion_main!` macros expand to undocumented items; benches are not a
// public API surface, so silence the workspace `missing_docs` warn here.
#![allow(missing_docs)]

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use unblock_error::{ErrorCode, StructuredError};

fn bench_from_code_and_serialize(c: &mut Criterion) {
    c.bench_function("from_code + serialize (clean message)", |b| {
        b.iter(|| {
            let err = StructuredError::from_code(
                black_box(ErrorCode::IssueNotFound),
                black_box("Issue not found: ub-abc123"),
            );
            black_box(serde_json::to_string(&err).expect("serializes"))
        });
    });

    c.bench_function("from_code + serialize (dirty message)", |b| {
        b.iter(|| {
            let err = StructuredError::from_code(
                black_box(ErrorCode::InternalError),
                black_box("boom\x1b[2K\x07bell\rcarriage"),
            );
            black_box(serde_json::to_string(&err).expect("serializes"))
        });
    });
}

criterion_group!(benches, bench_from_code_and_serialize);
criterion_main!(benches);
