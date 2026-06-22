//! `unblock-storage` (L2) — the backend-agnostic async [`Storage`] trait and its storage-owned
//! value types.
//!
//! This is the **interface** crate: it defines the contract every backend implements
//! ([`Storage`]), the storage-owned value types ([`DeletePlan`], [`DeleteMode`], [`IssuePatch`]),
//! and the backend-agnostic error ([`StorageError`], with backend failures absorbed into the opaque
//! [`BackendOpaque`]). The query/result contract types are **re-exported from `unblock-model`**
//! (§1.10 — CF-A/CF-B/CF-C) so this crate never redefines them.
//!
//! The only backend-aware implementation (libsql: schema/migrations, queries, transactional
//! mutate, WAL + native `busy_timeout` non-spin discipline, NFR-3) — and its `LibsqlStorage`
//! constructor, the `remote`/`testkit` features, and the reusable contract suite — land at **T0.6**.
//! No libsql type ever crosses the public API (spine §6 rule 2); remote/replica is behind the
//! non-default `remote` feature (D15). See `docs/plans/crates/unblock-storage.md`.
//!
//! # Example
//!
//! Constructing the storage-owned value types (this crate is interface-only at T0.5 — there is no
//! `open`/`migrate` yet):
//!
//! ```
//! use unblock_storage::{DeleteMode, DeletePlan, IssuePatch};
//! use unblock_model::Status;
//!
//! // An all-empty patch changes nothing.
//! let patch = IssuePatch::default();
//! assert!(patch.title.is_none());
//!
//! // A targeted patch: set the title, clear the description, move to in-progress.
//! let patch = IssuePatch {
//!     title: Some("Write the parser".to_string()),
//!     description: Some(None), // clear to NULL
//!     status: Some(Status::InProgress),
//!     ..IssuePatch::default()
//! };
//! assert_eq!(patch.status, Some(Status::InProgress));
//!
//! // A dry-run delete plan (mutates nothing when executed).
//! let plan = DeletePlan {
//!     mode: DeleteMode::DryRun,
//!     targets: vec!["ub-abc123".to_string()],
//!     cascade_children: Vec::new(),
//! };
//! assert_eq!(plan.mode, DeleteMode::DryRun);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod filters;
mod trait_def;

pub use error::{BackendOpaque, StorageError};
pub use filters::{DeleteMode, DeletePlan, IssuePatch};
pub use trait_def::Storage;

// Query/result contract types defined in `unblock-model` §1.10 (CF-A/CF-B/CF-C) and re-exported
// here so existing importers of `unblock_storage::ListFilters` keep compiling and CF-E
// diagnostic-seam consumers can reach `DiagnosticFinding` via storage (G-10). Storage does NOT
// redefine these.
pub use unblock_model::{
    CountBucket, CountGroupBy, DepTree, DiagnosticFinding, DiagnosticKind, DiagnosticReport,
    GraphEdge, ListFilters,
};
