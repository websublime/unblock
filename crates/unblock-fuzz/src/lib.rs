//! `unblock-fuzz` (unpublished) — fuzz harness scaffold over model/sync/storage ingestion (NFR-16).
//! The libFuzzer targets live in a separate nested `fuzz/` package added at the fuzzing task (T0.7+);
//! this member currently holds only the (future) shared harness. See `docs/plans/crates/unblock-fuzz.md`.
#![forbid(unsafe_code)]
