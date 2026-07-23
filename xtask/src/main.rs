//! `xtask` — workspace tooling (thin dispatcher over the `xtask` library).
//!
//! `check-layering` enforces the acyclic crate graph (NFR-15) from resolved `cargo metadata`
//! (`xtask::layering`). `doc-lint` runs the doc-corpus consistency lint (the six drift classes
//! a..f; `xtask::doc_lint`, ci-cd §2.1). `knowledge-lint` runs the knowledge-layer structural lint
//! (the six checks k1..k6 over its own dynamic corpus, separate from the 19-file normative one;
//! `xtask::knowledge_lint`, ci-cd §2.3). `bench-gate` enforces the NFR-1 tier-ii absolute-ms
//! ceilings from criterion's `estimates.json` (`xtask::bench_gate`, D34). `bench-compare` reports
//! the NFR-1 tier-i advisory relative-10% delta vs the committed baseline (`xtask::bench_compare`,
//! D34/F-1 — report-only, never fails). `verify-pins` fails if any third-party `uses:` in a
//! `.github/workflows/*.yml` is not pinned to a 40-char commit SHA (`xtask::verify_pins`, NFR-9/D4).
//! `no-network` fails if a networking symbol appears un-gated anywhere in `crates/*/src` + `xtask/src`
//! outside the whitelisted `self-update` axoupdater path (`xtask::no_network`, NFR-17/D5). `release` is
//! the interactive version-bump + tag + push helper for the `dist` release (`xtask::release`, ci-cd §3).
//!
//! Run: `cargo xtask check-layering` / `cargo xtask doc-lint` / `cargo xtask knowledge-lint` /
//! `cargo xtask bench-gate` / `cargo xtask bench-compare` / `cargo xtask verify-pins` /
//! `cargo xtask no-network` / `cargo xtask release [--dry-run]`. The CI `layering` / `doc-lint`
//! (which also runs `knowledge-lint`) / `bench-gate` / `verify-pins` / `no-network` jobs and the
//! nightly `fuzz-smoke` advisory leg wire these in (the `release` helper is developer-operated, not
//! a CI job). See `docs/plans/ci-cd-and-distribution.md` §2 (CI) + §3 (release).
#![forbid(unsafe_code)]

use std::process::ExitCode;

use xtask::{
    bench_compare, bench_gate, doc_lint, knowledge_lint, layering, no_network, release, verify_pins,
};

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check-layering") => layering::check_layering(),
        Some("doc-lint") => doc_lint::doc_lint(),
        Some("knowledge-lint") => knowledge_lint::knowledge_lint(),
        // Optional 2nd arg overrides the criterion dir (backs the non-vacuity proof + testing).
        Some("bench-gate") => bench_gate::bench_gate(std::env::args().nth(2)),
        // Optional 2nd arg = criterion dir, 3rd = baseline JSON (both back testing / manual runs).
        Some("bench-compare") => {
            bench_compare::bench_compare(std::env::args().nth(2), std::env::args().nth(3))
        }
        Some("verify-pins") => verify_pins::verify_pins(),
        Some("no-network") => no_network::no_network(),
        // Interactive release helper (`--dry-run` supported; parsed inside `release::run`).
        Some("release") => release::run(),
        Some(other) => {
            eprintln!(
                "unknown xtask {other:?}\n  available: check-layering, doc-lint, knowledge-lint, \
                 bench-gate, bench-compare, verify-pins, no-network, release"
            );
            ExitCode::from(2)
        }
        None => {
            eprintln!(
                "usage: cargo xtask <task>\n  available: check-layering, doc-lint, knowledge-lint, \
                 bench-gate, bench-compare, verify-pins, no-network, release"
            );
            ExitCode::from(2)
        }
    }
}
