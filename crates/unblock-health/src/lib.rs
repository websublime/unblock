//! `unblock-health` (L3) — workspace health classification and the recovery evidence layout.
//!
//! The four [`HealthLevel`]s are defined by repair path (PRD §4 D58). `Drifted` means derived state
//! inside the database disagrees with its base rows and an online recompute fixes it. `Recoverable`
//! means an in-product repair exists. `Unsafe` means none does, and the report names the manual
//! path.
//!
//! The crate is pure, non-async and storage-free (D29 clause F3, kept by D58 clause (4)). The engine
//! gathers every fact and passes it in as data: [`PreOpenFacts`] from `std::fs` reads made before
//! any libsql connection exists, [`DbFacts`] mapped from `Storage::health_facts()`, [`ExportFacts`]
//! from [`scan_export`] and the clock in [`AdvisoryFacts`]. [`classify`] returns a
//! [`WorkspaceClassification`] with the anomalies, the advisories, one [`PlannedRepair`] per
//! anomaly and the composite level. The [`recovery`] module names, creates and fills the
//! `.unblock/.recovery/` run directories; it never opens the database. Health holds no `Storage`
//! handle, never names a libsql type (NFR-15) and uses no git (NFR-6) or network.
//!
//! See `docs/plans/crates/unblock-health.md`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod anomaly;
mod audit;
mod classify;
mod error;
mod export;
mod facts;
mod integrity;
mod level;
mod paths;
pub mod recovery;

pub use anomaly::{Advisory, AnomalyClass, PlannedAction, PlannedRepair, RepairPath};
pub use audit::ReliabilityAuditRecord;
pub use classify::{WorkspaceClassification, classify};
pub use error::HealthError;
pub use export::scan_export;
pub use facts::{
    AdvisoryFacts, DbFacts, DbFileFact, ExportFacts, ExportScan, PreOpenFacts, RecoveryEntry,
};
pub use integrity::classify_integrity_row;
pub use level::HealthLevel;
pub use paths::{JOURNAL_SUFFIX, SHM_SUFFIX, WAL_SUFFIX, WorkspacePaths, sidecar};
