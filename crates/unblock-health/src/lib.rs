//! `unblock-health` (L3) — workspace health/integrity diagnostics.
//!
//! # Scope: v1-lite (T3.3 / D29) vs v1.1-full
//!
//! **v1-lite (this build)** ships the storage-free doctor: an [`integrity_check`] passthrough (the
//! engine runs the PRAGMA and passes the `Vec<String>` rows in), pure [`classify_file_state`]
//! file-state diagnostics, and the [`run_doctor`] aggregation into a [`DoctorReport`]. It holds NO
//! `Storage` handle and never names a libsql type (F3/D29, NFR-15); no git (NFR-6), no network.
//!
//! **v1.1-full** adds the active four-state taxonomy (`AnomalyClass`), the composite
//! `classify_workspace` pipeline (db-state probes via the reserved CF-E `Storage::diagnostic_probe(s)`
//! seams + optional JSONL drift via `unblock-sync`), the `unblock.reliability` audit record, and the
//! `.unblock/.recovery/` evidence writer. Those module seams are **reserved but not built** here (the
//! commented `mod` lines below) so the v1.1 layout needs no structural rewrite.
//!
//! The full four-variant [`HealthLevel`] ladder ships now (a stable contract from day one); v1-lite
//! only ever *produces* `Healthy`/`Recoverable`/`Unsafe`, with `Drifted` reachable in v1.1.
//!
//! See `docs/plans/crates/unblock-health.md`.
//!
//! [`integrity_check`]: run_doctor
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod doctor;
mod error;
mod file_state;
mod level;
mod paths;

// [v1.1] mod anomaly;   // the active file-state ∪ db-state ∪ drift taxonomy (supersedes FileAnomaly)
// [v1.1] mod classify;  // classify_workspace: db-state probes (CF-E seams) + optional JSONL drift
// [v1.1] mod audit;     // ReliabilityAuditRecord + tracing on `unblock.reliability` (NFR-13)
// [v1.1] mod recovery;  // `.unblock/.recovery/` evidence writer (FR-16 full)

pub use doctor::{DoctorReport, HealthSummary, run_doctor};
pub use error::HealthError;
pub use file_state::{
    FileAnomaly, classify_file_state, is_orphaned_lock_file, jsonl_has_conflict_markers,
};
pub use level::HealthLevel;
pub use paths::{JOURNAL_SUFFIX, SHM_SUFFIX, WAL_SUFFIX, WorkspacePaths, sidecar};
