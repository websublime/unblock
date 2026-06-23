//! `xtask` — workspace tooling (thin dispatcher over the `xtask` library).
//!
//! `check-layering` enforces the acyclic crate graph (NFR-15) from resolved `cargo metadata`
//! (`xtask::layering`). `doc-lint` runs the doc-corpus consistency lint (the six drift classes
//! a..f; `xtask::doc_lint`, ci-cd §2.1).
//!
//! Run: `cargo xtask check-layering` / `cargo xtask doc-lint`. T0.9 wires both into the CI
//! `layering` + `doc-lint` jobs. See `docs/plans/ci-cd-and-distribution.md` §2.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use xtask::{doc_lint, layering};

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check-layering") => layering::check_layering(),
        Some("doc-lint") => doc_lint::doc_lint(),
        Some(other) => {
            eprintln!("unknown xtask {other:?}\n  available: check-layering, doc-lint");
            ExitCode::from(2)
        }
        None => {
            eprintln!("usage: cargo xtask <task>\n  available: check-layering, doc-lint");
            ExitCode::from(2)
        }
    }
}
