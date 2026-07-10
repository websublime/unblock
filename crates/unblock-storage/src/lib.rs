//! `unblock-storage` (L2) — the backend-agnostic async [`Storage`] trait and its storage-owned
//! value types.
//!
//! This is the **interface** crate: it defines the contract every backend implements
//! ([`Storage`]), the storage-owned value types ([`DeletePlan`], [`DeleteMode`], [`IssuePatch`]),
//! and the backend-agnostic error ([`StorageError`], with backend failures absorbed into the opaque
//! [`BackendOpaque`]). The query/result contract types are **re-exported from `unblock-model`**
//! (§1.10 — CF-A/CF-B/CF-C) so this crate never redefines them.
//!
//! The only backend-aware implementation ([`LibsqlStorage`]: schema/migrations, queries,
//! transactional mutate, WAL + native `busy_timeout` non-spin discipline, NFR-3) lives in the
//! `libsql` module. No libsql type ever crosses the public API (spine §6 rule 2); remote/replica is
//! behind the non-default `remote` feature (D15). See `docs/plans/crates/unblock-storage.md`.
//!
//! # Example
//!
//! Open an in-memory database, migrate it, then create and read back an issue:
//!
//! ```
//! use unblock_storage::{LibsqlStorage, Storage};
//! use unblock_model::Issue;
//! use chrono::{TimeZone, Utc};
//!
//! let rt = tokio::runtime::Runtime::new().unwrap();
//! rt.block_on(async {
//!     let storage = LibsqlStorage::open_in_memory().await.expect("open");
//!     storage.migrate().await.expect("migrate");
//!
//!     let issue = Issue {
//!         id: "ub-abc123".to_string(),
//!         title: "Write the parser".to_string(),
//!         created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
//!         updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
//!         ..Issue::default()
//!     };
//!     let id = storage.create_issue(&issue, "tester").await.expect("create");
//!     assert_eq!(id, "ub-abc123");
//!
//!     let fetched = storage.get_issue(&id).await.expect("get").expect("present");
//!     assert_eq!(fetched.title, "Write the parser");
//! });
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod filters;
mod libsql;
mod trait_def;

/// Backend-independent storage **contract suite** (NFR-16) + the gated `StorageTestkit` seam.
///
/// Gated behind `#[cfg(any(test, feature = "testkit"))]`: it is compiled for the crate's own
/// `tests/contract.rs` (which needs no flag) and, when the `testkit` feature is enabled, exported so
/// out-of-crate consumers (the `unblock-fuzz` storage targets, a future backend's self-test) can run
/// the same suite. It is **never** part of the default public surface (no extra deps, no production
/// code path).
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use error::{BackendOpaque, StorageError};
pub use filters::{DeleteMode, DeletePlan, IssuePatch};
pub use libsql::{LibsqlStorage, WriteLockGuard};
pub use trait_def::Storage;

/// The default `.unblock/.write.lock` acquire timeout, in milliseconds (D31).
///
/// The `unblock-config` `write_lock_timeout_ms` key defaults to this value and threads it DOWN into
/// [`LibsqlStorage::open_local`] at open; a bounded timeout turns a stuck holder into a retryable
/// [`StorageError::DatabaseLocked`] rather than an unbounded park. Faithful to beads' 30s
/// `--lock-timeout` default.
pub const DEFAULT_WRITE_LOCK_TIMEOUT_MS: u64 = 30_000;

// The contract-suite entry + the gated seam trait, re-exported so a future backend (and the fuzz
// crate) reuses them without reaching into the `testkit` module path.
#[cfg(any(test, feature = "testkit"))]
pub use testkit::{StorageTestkit, run_storage_contract_suite};

// Query/result contract types defined in `unblock-model` §1.10 (CF-A/CF-B/CF-C) and re-exported
// here so existing importers of `unblock_storage::ListFilters` keep compiling and CF-E
// diagnostic-seam consumers can reach `DiagnosticFinding` via storage (G-10). Storage does NOT
// redefine these.
pub use unblock_model::{
    CountBucket, CountGroupBy, DepTree, DiagnosticFinding, DiagnosticKind, DiagnosticReport,
    GraphEdge, ListFilters,
};
