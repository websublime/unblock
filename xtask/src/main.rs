//! `xtask` — workspace tooling (thin dispatcher over the `xtask` library).
//!
//! `check-layering` enforces the acyclic crate graph (NFR-15) from resolved `cargo metadata`
//! (`xtask::layering`). `doc-lint` runs the doc-corpus consistency lint (the six drift classes
//! a..f; `xtask::doc_lint`, ci-cd §2.1). `bench-gate` enforces the NFR-1 tier-ii absolute-ms
//! ceilings from criterion's `estimates.json` (`xtask::bench_gate`, D34).
//!
//! Run: `cargo xtask check-layering` / `cargo xtask doc-lint` / `cargo xtask bench-gate`. The CI
//! `layering` / `doc-lint` / `bench-gate` jobs wire these in. See
//! `docs/plans/ci-cd-and-distribution.md` §2.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use xtask::{bench_gate, doc_lint, layering};

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check-layering") => layering::check_layering(),
        Some("doc-lint") => doc_lint::doc_lint(),
        // Optional 2nd arg overrides the criterion dir (backs the non-vacuity proof + testing).
        Some("bench-gate") => bench_gate::bench_gate(std::env::args().nth(2)),
        Some(other) => {
            eprintln!("unknown xtask {other:?}\n  available: check-layering, doc-lint, bench-gate");
            ExitCode::from(2)
        }
        None => {
            eprintln!(
                "usage: cargo xtask <task>\n  available: check-layering, doc-lint, bench-gate"
            );
            ExitCode::from(2)
        }
    }
}
