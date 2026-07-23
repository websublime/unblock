//! `xtask` — workspace tooling library.
//!
//! Exposes the CI gates as library functions so they are unit- and integration-testable:
//! - [`layering::check_layering`] enforces the acyclic crate graph (NFR-15) from resolved
//!   `cargo metadata`.
//! - [`doc_lint::doc_lint`] runs the doc-corpus consistency lint (the six drift classes a..f;
//!   ci-cd §2.1); [`doc_lint::lint_at`] is the testable core used by the corpus-green integration
//!   test.
//! - [`knowledge_lint::knowledge_lint`] runs the knowledge-layer structural lint (the six checks
//!   k1..k6 over the dynamic knowledge corpus; ci-cd §2.3) — a sibling of `doc_lint` with a
//!   SEPARATE corpus; [`knowledge_lint::lint_at`] is its testable core.
//! - [`bench_gate::bench_gate`] enforces the NFR-1 HYBRID perf gate tier ii (the hard per-PR
//!   absolute-ms ceilings, D34) by reading criterion's `estimates.json`.
//! - [`bench_compare::bench_compare`] reports the NFR-1 tier-i ADVISORY relative-10% delta against a
//!   committed baseline (D34/F-1) — report-only, never fails a PR/nightly.
//! - [`verify_pins::verify_pins`] fails if any third-party `uses:` in `.github/workflows/*.yml` is
//!   not pinned to a 40-char commit SHA (NFR-9); [`verify_pins::scan_workflow`] is the testable core.
//! - [`no_network::no_network`] fails if a networking symbol appears un-gated anywhere in
//!   `crates/*/src` + `xtask/src` outside the whitelisted `self-update` axoupdater path (NFR-17/D5);
//!   [`no_network::scan_file`] is the testable core.
//! - [`release::run`] is the interactive `cargo xtask release` helper: pre-flight → prompt → compute
//!   the next version → bump `Cargo.toml` + `Cargo.lock` → commit → annotated tag → push, gated by
//!   two typed-tag confirmations and an offline `--dry-run` (ci-cd §3).
//!
//! The `xtask` binary (`src/main.rs`) is a thin dispatcher over this library.
#![forbid(unsafe_code)]

pub mod bench_compare;
pub mod bench_gate;
pub mod doc_lint;
pub mod knowledge_lint;
pub mod layering;
pub mod no_network;
pub mod release;
pub mod verify_pins;
