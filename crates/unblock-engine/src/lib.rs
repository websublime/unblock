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
//! - Scope is **in-process only**: the supported topology is exactly one MCP server (`unblock mcp`) per
//!   workspace.
//! - Permit acquisition is **uncancel-safe** across the tx boundary: a dropped future before commit
//!   releases the permit and leaves the DB committed-or-rolled-back (no partial state, NFR-5).
//!
//! Cooperative shutdown reads a flag the cli installs (OQ-4 — the engine only reads/flips it, never
//! installs an OS handler). No libsql/backend type ever crosses this boundary (spine §6 rule 2); no
//! git crate / network (NFR-6).
//!
//! # Runtime configuration (FR-13 full, PRD §4 D57)
//!
//! A [`Session`] holds its startup keys immutably and its runtime keys as a LIVE snapshot
//! ([`Session::runtime`]) that only `config_set`/`config_unset` swap, after re-resolving them through
//! the context's `ConfigReloader`. While the snapshot holds a BLOCKING DB config fault, every
//! fallible method except `open`, the four `config_*` methods and `shutdown` fail-closes with
//! `ConfigError::FaultyDbConfig` (→ [`EngineError::Config`], `CONFIG_ERROR`, the recovery hint).
//!
//! # Feature-gated methods (typed not-wired — never a faked success)
//!
//! `export_jsonl`/`import_jsonl`/`import_bd` delegate to `unblock-sync` (default-on `sync` feature;
//! `FeatureNotWired` only under `--no-default-features`).
//!
//! # Workspace health (FR-16 full, PRD §4 D58)
//!
//! Under the default-on `health` feature (which implies `sync`), [`preopen`] gathers and classifies
//! the file facts before any libsql connection exists and runs the exclusive repairs,
//! `Session::doctor` classifies an open workspace into a [`HealthReport`], and [`Session::recover`]
//! runs the online repairs with evidence under `.unblock/.recovery/`. A `--no-default-features`
//! build has neither `preopen` nor `doctor`, and `recover()` returns
//! [`EngineError::FeatureNotWired`] (`feature: "health"`).
//!
//! # Example — a happy-path round-trip against a temp workspace
//!
//! ```
//! use std::sync::Arc;
//! use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};
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
//!     // D57: the actor lives in `config.runtime`; the reloader reproduces it at the `$USER` tier.
//!     let mut config = ResolvedConfig::default();
//!     config.runtime.actor = "doctest".to_string();
//!     config.runtime.sources.actor = unblock_model::ConfigSource::OsUser;
//!     let paths = ConfigPaths {
//!         db_path: unblock_dir.join(&config.startup.db_filename),
//!         jsonl_path: unblock_dir.join(&config.startup.jsonl_filename),
//!         unblock_dir,
//!     };
//!     let ctx = WorkspaceContext {
//!         storage,
//!         workspace_dir,
//!         config,
//!         reloader: unblock_config::ConfigReloader::new(
//!             unblock_config::CliOverrides::default(),
//!             unblock_config::EnvOverrides::default(),
//!             Some("doctest".to_string()),
//!             None,
//!             None,
//!         ),
//!         paths,
//!         source: WorkspaceSource::WalkUp,
//!         // D46: this context is hand-built, not facade-opened.
//!         schema_version_before_migrate: 0,
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
mod epic;
mod error;
mod logging;
mod permit;
#[cfg(feature = "health")]
pub mod preopen;
mod report;
mod session;
mod shutdown;

// --- engine-owned public surface ---
pub use error::{EngineError, Result};
pub use logging::{RELIABILITY_TARGET, TracingOptions, init_tracing};
pub use session::write::{NewDep, NewIssue};
pub use session::{MigrateOutcome, Session, SessionConfig};
pub use shutdown::is_shutdown_requested;

// --- the D58 health surface, re-exported so the cli needs no `unblock-health` edge ---
#[cfg(feature = "health")]
pub use preopen::HealthReport;
#[cfg(feature = "health")]
pub use unblock_health::{
    DbFileFact, HealthLevel, PlannedAction, PlannedRepair, PreOpenFacts, RepairPath,
};

// --- the `SQLite` sidecar suffixes (owner `unblock-sync`, PRD §4 D5 amendment, `ub-c15`),
//     re-exported so the cli needs no `unblock-sync` edge (`init`'s orphan-sidecar check) ---
#[cfg(feature = "sync")]
pub use unblock_sync::{SIDECAR_SUFFIXES, sidecar};

// --- the report module owns the engine-local `ImportOptions` and re-exports the model-owned
//     report/outcome DTOs (CF-A) from one place ---
pub use report::{CloseOutcome, ExportReport, ImportOptions, ImportReport};

// --- re-exported model-owned DTOs (CF-B/CF-C, spine §1.10) — defined in unblock-model, never
//     redefined here, so unblock-render (model + error only) can format engine results. Since D56
//     the FR-6 organization results (`EpicStatus`/`LabelCount`/`LabelRename`/
//     `CloseEligibleOutcome`) ride the same re-export ---
pub use unblock_model::{
    CloseEligibleOutcome, CountBucket, CountGroupBy, DepTree, Dependency, DiagnosticFinding,
    DiagnosticKind, DiagnosticReport, EpicStatus, GraphEdge, LabelCount, LabelRename, ListFilters,
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
        CloseEligibleOutcome, CloseOutcome, CountBucket, CountGroupBy, DeleteMode, DeletePlan,
        DepTree, Dependency, DiagnosticFinding, DiagnosticKind, DiagnosticReport, EngineError,
        EpicStatus, ExportReport, GraphEdge, ImportOptions, ImportReport, IssuePatch, LabelCount,
        LabelRename, ListFilters, MigrateOutcome, Result, Session, SessionConfig, TracingOptions,
    };
    // The D58 health surface (`health` feature).
    #[cfg(feature = "health")]
    #[allow(unused_imports)]
    use crate::{
        DbFileFact, HealthLevel, HealthReport, PlannedAction, PlannedRepair, PreOpenFacts,
        RepairPath,
        preopen::{ActionOutcome, ActionResult, PreOpenRepairOutcome},
    };
    // The `SQLite` sidecar suffixes (`sync` feature; PRD §4 D5 amendment, `ub-c15`).
    #[cfg(feature = "sync")]
    #[allow(unused_imports)]
    use crate::{SIDECAR_SUFFIXES, sidecar};

    #[test]
    fn engine_owned_re_exports_construct() {
        let _cfg = SessionConfig::default();
        let _opts = ImportOptions::default();
        let _tracing = TracingOptions::default();
        // The engine-local `MigrateOutcome` return (D27/AF-2) is constructible from the crate root.
        let outcome = MigrateOutcome {
            from: 1,
            to: 1,
            applied: false,
        };
        assert!(!outcome.applied);
        assert_eq!(crate::RELIABILITY_TARGET, "unblock.reliability");
    }
}
