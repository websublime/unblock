//! `unblock-config` (L4) — the layered configuration resolver, `.unblock/` workspace discovery,
//! config-owned value/path types, and the two workspace-open facades (CF-D, spine §4).
//!
//! This crate is the **full T1.3 layered resolver**. It merges configuration across four layers in
//! strict precedence — **CLI ([`CliOverrides`]) > env `UNBLOCK_*` ([`EnvOverrides`]) > project
//! `.unblock/config.toml` ([`ProjectConfig`]) > defaults** (highest wins per field) — and partitions
//! keys into startup-only vs runtime-reloadable ([`keys`]: [`STARTUP_KEYS`] / [`RUNTIME_KEYS`],
//! [`classify`]). It delivers the config-owned value type [`ResolvedConfig`] and path type
//! [`ConfigPaths`] (resolved for real, not defaulted), the two contexts [`ResolvedContext`]
//! (resolve-only, no storage) and [`WorkspaceContext`] (storage-bearing), and the per-crate
//! [`ConfigError`].
//!
//! **Discovery** walks up for the nearest dir named `.unblock` **or** `_unblock` (the monorepo alias,
//! FORK-2/D8); the discovered dir is **canonicalized** so artifacts are confined to the canonical
//! subtree (FORK-3, NFR-18). An explicit `--dir`/`UNBLOCK_DIR` is used directly with no walk-up
//! (MF-2).
//!
//! **Two facade pairs** front the resolver. The permanent `&Path` facades — [`open_workspace`]
//! (resolve-only, no DB) / [`open_with_storage`] (open + migrate libsql, build the
//! `Arc<dyn Storage>`) — pass `start` as the walk-up start; their additive `_with_cli` overloads —
//! [`open_workspace_with_cli`] / [`open_with_storage_with_cli`] — thread a full [`CliOverrides`]
//! (`--dir`/`--db`/`--actor`/`--output-format`) through resolution (FORK-1 OVERLOAD model).
//!
//! **Config owns workspace-open** (CF-D): it discovers `.unblock/`, resolves paths, opens/migrates
//! libsql, and constructs the `Arc<dyn Storage>` carried by [`WorkspaceContext`]; the engine
//! consumes the context via `Session::open` — it does not construct storage.
//!
//! See `docs/plans/crates/unblock-config.md` and `docs/plans/01-design-spine.md` §4.
//!
//! # Example
//!
//! Resolve a workspace without opening the database (the resolve-only facade):
//!
//! ```
//! use unblock_config::open_workspace;
//!
//! let rt = tokio::runtime::Runtime::new().unwrap();
//! rt.block_on(async {
//!     let workspace = tempfile::tempdir().unwrap();
//!     std::fs::create_dir(workspace.path().join(".unblock")).unwrap();
//!
//!     let ctx = open_workspace(workspace.path()).await.expect("resolve");
//!     // The discovered dir is canonicalized (FORK-3), so compare against the canonical tempdir.
//!     let canon = workspace.path().canonicalize().unwrap();
//!     assert_eq!(ctx.paths.unblock_dir, canon.join(".unblock"));
//!     // resolve-only never opens or creates the database file:
//!     assert!(!ctx.paths.db_path.exists());
//! });
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod actor;
mod cli;
mod config;
mod context;
mod discovery;
mod env;
mod error;
pub mod keys;
mod merge;
mod paths;
mod schema;

pub use cli::CliOverrides;
pub use config::{ResolvedConfig, WorkspaceConfig};
pub use context::{
    ResolvedContext, WorkspaceContext, open_with_storage, open_with_storage_with_cli,
    open_workspace, open_workspace_with_cli,
};
pub use discovery::{WorkspaceSource, discover_optional_unblock_dir, discover_unblock_dir};
pub use env::{EnvOverrides, EnvSource};
pub use error::ConfigError;
pub use keys::{KeyClass, RUNTIME_KEYS, RuntimeKey, STARTUP_KEYS, StartupKey, classify};
pub use paths::ConfigPaths;
pub use schema::{ProjectConfig, RemoteTable};

// `OutputFormat` is owned once in `unblock-model` (G-7/CF-J) and re-exported here so consumers of
// `ResolvedConfig.output_format` reach it through config without a second definition.
pub use unblock_model::OutputFormat;
