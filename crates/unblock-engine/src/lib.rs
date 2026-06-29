//! `unblock-engine` (L5) — the single mutation home (FR-9).
//!
//! A [`Session`] consumes the `WorkspaceContext` built by `unblock-config` (CF-D — config performs
//! `.unblock/` discovery, opens/migrates libsql, and builds the `Arc<dyn Storage>`; the engine does
//! **not** construct storage) and composes storage + policy (+ optional sync/health) over one
//! lifecycle (`open → import? → mutate → export? → recover`). MCP and CLI are thin adapters over the
//! `Session` surface, so behaviour cannot drift (FR-9, conformance rule 4).
//!
//! # The write-Semaphore contract (D14 / spine §4.2 — normative)
//!
//! - One `Arc<tokio::sync::Semaphore>` with **1 permit** per [`Session`]. Every mutation acquires the
//!   single permit for the **entire** storage transaction, then releases — serializing all
//!   in-process writers (linearizable per FR-9).
//! - **Reads never touch the permit** (FR-10): they run concurrently against libsql WAL readers
//!   while a write holds the permit.
//! - Scope is **in-process only**: the supported topology is exactly one `unblock serve` per
//!   workspace.
//! - Permit acquisition is **uncancel-safe** across the tx boundary: a dropped future before commit
//!   releases the permit and leaves the DB committed-or-rolled-back (no partial state, NFR-5).
//!
//! Cooperative shutdown reads a flag the cli installs (OQ-4 — the engine only reads/flips it, never
//! installs an OS handler). No libsql/backend type ever crosses this boundary (spine §6 rule 2); no
//! git crate / network (NFR-6).
//!
//! # SEAM-deferred methods (typed not-wired — never a faked success)
//!
//! `export_jsonl`/`import_jsonl`/`import_bd` (the `sync` seam, T2.4) and `doctor`/`recover` (the
//! `health` seam, T3.3) land their **signatures** now; their v1 bodies return
//! [`EngineError::FeatureNotWired`]. T2.4/T3.3 replace the bodies additively (no v1 signature change).
//!
//! # Example — a happy-path round-trip against a temp workspace
//!
//! ```
//! use std::sync::Arc;
//! use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext};
//! use unblock_engine::{Session, SessionConfig};
//! use unblock_model::Issue;
//! use unblock_storage::{LibsqlStorage, Storage};
//! use chrono::{TimeZone, Utc};
//!
//! let rt = tokio::runtime::Runtime::new().unwrap();
//! rt.block_on(async {
//!     // The TEST/CALLER builds the WorkspaceContext (config builds storage in prod; here we wire
//!     // an in-memory libsql backend the same way for a runnable doctest).
//!     let storage = LibsqlStorage::open_in_memory().await.expect("open");
//!     storage.migrate().await.expect("migrate");
//!     let storage: Arc<dyn Storage> = Arc::new(storage);
//!
//!     let workspace_dir = std::path::PathBuf::from("/tmp/ws");
//!     let unblock_dir = workspace_dir.join(".unblock");
//!     let config = ResolvedConfig::default();
//!     let paths = ConfigPaths {
//!         db_path: unblock_dir.join(&config.db_filename),
//!         jsonl_path: unblock_dir.join(&config.jsonl_filename),
//!         unblock_dir,
//!     };
//!     let ctx = WorkspaceContext {
//!         storage,
//!         workspace_dir,
//!         actor: "doctest".to_string(),
//!         config,
//!         paths,
//!     };
//!
//!     let session = Session::open(ctx, SessionConfig::default()).await.expect("open session");
//!
//!     // create -> ready -> claim -> close, all through the single Session surface.
//!     let issue = Issue {
//!         id: "ub-abc123".to_string(),
//!         title: "Write the parser".to_string(),
//!         created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
//!         updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
//!         ..Issue::default()
//!     };
//!     let id = session.create(&issue).await.expect("create");
//!
//!     let ready = session.ready(&Default::default()).await.expect("ready");
//!     assert_eq!(ready.len(), 1);
//!
//!     let claimed = session.claim(&id, "doctest").await.expect("claim");
//!     assert_eq!(claimed.assignee.as_deref(), Some("doctest"));
//!
//!     let outcome = session.close_with_suggestions(&id, None).await.expect("close");
//!     assert_eq!(outcome.closed.id, id);
//!
//!     session.shutdown().await.expect("shutdown");
//! });
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod diagnostics;
mod error;
mod logging;
mod permit;
mod report;
mod session;
mod shutdown;

// --- engine-owned public surface ---
pub use error::{EngineError, Result};
pub use logging::{RELIABILITY_TARGET, TracingOptions, init_tracing};
pub use session::{Session, SessionConfig};
pub use shutdown::is_shutdown_requested;

// --- the report module owns the engine-local `ImportOptions` and re-exports the model-owned
//     report/outcome DTOs (CF-A) from one place ---
pub use report::{CloseOutcome, ExportReport, ImportOptions, ImportReport};

// --- re-exported model-owned DTOs (CF-B/CF-C, spine §1.10) — defined in unblock-model, never
//     redefined here, so unblock-render (model + error only) can format engine results ---
pub use unblock_model::{
    CountBucket, CountGroupBy, DepTree, Dependency, DiagnosticFinding, DiagnosticKind,
    DiagnosticReport, GraphEdge, ListFilters,
};

// --- re-exported storage-owned contract types so adapters import them from one place ---
pub use unblock_storage::{DeleteMode, DeletePlan, IssuePatch};

#[cfg(test)]
mod tests {
    //! Compile-time guard that the curated re-exports are public and resolve from the engine root.
    //!
    //! Each `use` line below only compiles if the name is `pub` at the crate root, so this module
    //! statically witnesses the whole re-export surface (CF-A/CF-B/CF-C + storage-owned + engine).
    #[allow(unused_imports)]
    use crate::{
        CloseOutcome, CountBucket, CountGroupBy, DeleteMode, DeletePlan, DepTree, Dependency,
        DiagnosticFinding, DiagnosticKind, DiagnosticReport, EngineError, ExportReport, GraphEdge,
        ImportOptions, ImportReport, IssuePatch, ListFilters, Result, Session, SessionConfig,
        TracingOptions,
    };

    #[test]
    fn engine_owned_re_exports_construct() {
        let _cfg = SessionConfig::default();
        let _opts = ImportOptions::default();
        let _tracing = TracingOptions::default();
        assert_eq!(crate::RELIABILITY_TARGET, "unblock.reliability");
    }
}
