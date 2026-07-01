//! `unblock-sync` (L3) — light JSONL export/import (D5): atomic temp+fsync+rename incl. parent-dir
//! fsync (NFR-4/5), per-line validation, conflict-marker + path-confinement rejection
//! (FR-7/FR-8/NFR-7/NFR-8), bounded ingestion (FORK-3), canonical export timestamps (CF-TS/D-OQ-B),
//! and atomic classify-then-`create_issues` import apply (MF-1/D23). **No git, no merge, no
//! network** (D5/D13/NFR-6); consumes only the `Storage` trait + model types — no libsql type crosses
//! this boundary (NFR-15). See `docs/plans/crates/unblock-sync.md`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod atomic;
mod bd_import;
mod conflict;
mod error;
mod export;
mod import;
mod jsonl;
mod path;

#[cfg(test)]
mod testutil;

// Reports are model-owned (CF-A): re-export, never redefine.
pub use unblock_model::{ExportReport, ImportReport};

// The orchestration entry points the engine calls.
pub use bd_import::{import_bd, map_bd_record};
pub use export::{ExportOptions, export_jsonl};
pub use import::{CollisionPolicy, ImportOptions, import_jsonl};

// Preflight + scanning primitives (also consumed by `unblock-health` lite + the fuzz targets).
pub use conflict::{
    ConflictMarker, ConflictMarkerType, ensure_no_conflict_markers, scan_conflict_markers,
};
pub use jsonl::{parse_issue_line, serialize_issue_line};
pub use path::validate_sync_path;

// The per-crate error + its `PathReject` detail.
pub use error::SyncError;
pub use path::PathReject;
