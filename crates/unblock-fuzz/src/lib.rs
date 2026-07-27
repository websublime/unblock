//! `unblock-fuzz` (unpublished member) — the **stable** fuzz cores + shared harness (NFR-16).
//!
//! All target logic lives here in `src/` as plain `pub fn run_<t>_case(&[u8]) -> Result<(),
//! FuzzError>` cores, so it **builds and replays on stable Rust** (NFR-12). The nested
//! `fuzz/fuzz_targets/<t>.rs` files (in the workspace-excluded cargo-fuzz package) are 5-line
//! `fuzz_target!` wrappers over these cores, and `tests/regression.rs` replays the committed corpus
//! through them under plain `cargo test`.
//!
//! # Landed targets (T0.7)
//!
//! - **model + error:** `content_hash`, `issue_ingest`, `parse_id`, `enum_deserialize`, `sanitize`
//!   ([`run_content_hash_case`], [`run_issue_ingest_case`], [`run_parse_id_case`],
//!   [`run_enum_deserialize_case`], [`run_sanitize_case`]).
//! - **storage:** `query_filters`, `cycle_detect`, `id_alloc` ([`run_query_filters_case`],
//!   [`run_cycle_detect_case`], [`run_id_alloc_case`]).
//!
//! # Later targets
//!
//! - **error (D43):** `dup_scan` ([`run_dup_scan_case`]) — the DIFFERENTIAL duplicate-JSON-key
//!   target: the scanner's verdict is compared against an independent duplicate-preserving walker
//!   ([`RawJson`]) for under-rejection, and against rmcp's own frame parse for over-rejection.
//!
//! The JSONL/`bd`/sync targets are **post-T0.7** (they need `unblock-sync`, which this member does
//! not yet depend on). See `docs/plans/crates/unblock-fuzz.md`.
//!
//! # Async-in-libFuzzer (OQ-1 RESOLVED)
//!
//! libFuzzer's entry is synchronous; `Storage` is `async_trait`. [`tokio_block_on`] builds a
//! **fresh** `current_thread` runtime per call and blocks on the future — corpus determinism over
//! throughput.

#![forbid(unsafe_code)]

mod arbitraries;
mod cursor;
mod dup_scan_target;
pub mod invariants;
mod model_targets;
mod storage_targets;
mod workspace;

use snafu::Snafu;

pub use arbitraries::{arbitrary_issue, arbitrary_issues, normalize_issue};
pub use cursor::{ByteCursor, CursorExt};
pub use dup_scan_target::{RawJson, run_dup_scan_case};
pub use model_targets::{
    run_content_hash_case, run_enum_deserialize_case, run_issue_ingest_case, run_parse_id_case,
    run_sanitize_case,
};
pub use storage_targets::{run_cycle_detect_case, run_id_alloc_case, run_query_filters_case};
pub use workspace::FuzzWorkspace;

/// The local error the fuzz cores propagate with `?` for **operational** failures (a serialize/parse
/// step, a storage setup) — distinct from an **invariant** breach, which the cores `assert!` (so
/// libFuzzer reports it as a crash). A core returning `Err` means "this input did not reach the deep
/// path", not "the code under test is buggy".
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum FuzzError {
    /// A `serde_json` serialize/deserialize step failed.
    #[snafu(display("json error: {source}"), context(false))]
    Json {
        /// The underlying `serde_json` error.
        source: serde_json::Error,
    },

    /// A storage operation failed.
    #[snafu(display("storage error: {source}"), context(false))]
    Storage {
        /// The underlying `StorageError`.
        source: unblock_storage::StorageError,
    },

    /// An I/O step failed (e.g. creating the temp workspace).
    #[snafu(display("io error: {source}"), context(false))]
    Io {
        /// The underlying I/O error.
        source: std::io::Error,
    },
}

/// Drive an async future to completion on a **fresh** `current_thread` tokio runtime (OQ-1).
///
/// A new runtime is built per call (corpus determinism over throughput): the same corpus byte stream
/// always produces the same scheduling, so a replay is reproducible.
///
/// # Panics
///
/// Panics if the runtime cannot be constructed — an environment failure, not an input bug. (The
/// alternative would swallow a genuine setup failure.)
pub fn tokio_block_on<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a fresh current-thread tokio runtime");
    runtime.block_on(future)
}

#[cfg(test)]
mod tests {
    use super::tokio_block_on;

    #[test]
    fn block_on_returns_value() {
        assert_eq!(tokio_block_on(async { 21 * 2 }), 42);
    }

    #[test]
    fn block_on_drives_nested_futures() {
        let out = tokio_block_on(async {
            let a = async { 1 }.await;
            let b = async { 2 }.await;
            a + b
        });
        assert_eq!(out, 3);
    }
}
