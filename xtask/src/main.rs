//! `xtask` — workspace tooling (thin dispatcher over the `xtask` library).
//!
//! `check-layering` enforces the acyclic crate graph (NFR-15) from resolved `cargo metadata`
//! (`xtask::layering`). `doc-lint` runs the doc-corpus consistency lint (the six drift classes
//! a..f; `xtask::doc_lint`, ci-cd §2.1). `bench-gate` enforces the NFR-1 tier-ii absolute-ms
//! ceilings from criterion's `estimates.json` (`xtask::bench_gate`, D34). `bench-compare` reports
//! the NFR-1 tier-i advisory relative-10% delta vs the committed baseline (`xtask::bench_compare`,
//! D34/F-1 — report-only, never fails).
//!
//! Run: `cargo xtask check-layering` / `cargo xtask doc-lint` / `cargo xtask bench-gate` /
//! `cargo xtask bench-compare`. The CI `layering` / `doc-lint` / `bench-gate` jobs and the nightly
//! `fuzz-smoke` advisory leg wire these in. See `docs/plans/ci-cd-and-distribution.md` §2.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use xtask::{bench_compare, bench_gate, doc_lint, layering};

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check-layering") => layering::check_layering(),
        Some("doc-lint") => doc_lint::doc_lint(),
        // Optional 2nd arg overrides the criterion dir (backs the non-vacuity proof + testing).
        Some("bench-gate") => bench_gate::bench_gate(std::env::args().nth(2)),
        // Optional 2nd arg = criterion dir, 3rd = baseline JSON (both back testing / manual runs).
        Some("bench-compare") => {
            bench_compare::bench_compare(std::env::args().nth(2), std::env::args().nth(3))
        }
        Some(other) => {
            eprintln!(
                "unknown xtask {other:?}\n  available: check-layering, doc-lint, bench-gate, \
                 bench-compare"
            );
            ExitCode::from(2)
        }
        None => {
            eprintln!(
                "usage: cargo xtask <task>\n  available: check-layering, doc-lint, bench-gate, \
                 bench-compare"
            );
            ExitCode::from(2)
        }
    }
}
