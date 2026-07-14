//! `xtask` — workspace tooling library.
//!
//! Exposes the CI gates as library functions so they are unit- and integration-testable:
//! - [`layering::check_layering`] enforces the acyclic crate graph (NFR-15) from resolved
//!   `cargo metadata`.
//! - [`doc_lint::doc_lint`] runs the doc-corpus consistency lint (the six drift classes a..f;
//!   ci-cd §2.1); [`doc_lint::lint_at`] is the testable core used by the corpus-green integration
//!   test.
//! - [`bench_gate::bench_gate`] enforces the NFR-1 HYBRID perf gate tier ii (the hard per-PR
//!   absolute-ms ceilings, D34) by reading criterion's `estimates.json`.
//! - [`bench_compare::bench_compare`] reports the NFR-1 tier-i ADVISORY relative-10% delta against a
//!   committed baseline (D34/F-1) — report-only, never fails a PR/nightly.
//!
//! The `xtask` binary (`src/main.rs`) is a thin dispatcher over this library.
#![forbid(unsafe_code)]

pub mod bench_compare;
pub mod bench_gate;
pub mod doc_lint;
pub mod layering;
