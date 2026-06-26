//! `unblock-config` (L4) — `.unblock/` workspace discovery, config-owned value/path types, and the
//! two workspace-open facades (CF-D, spine §4).
//!
//! This crate is the **T1.3a minimal subset**: the interface the engine (L5) consumes. It delivers
//! the config-owned value type [`ResolvedConfig`] and path type [`ConfigPaths`] (with
//! hardcoded/defaulted values), the two contexts [`ResolvedContext`] (resolve-only) and
//! [`WorkspaceContext`] (storage-bearing), the per-crate [`ConfigError`], and the facades
//! [`open_workspace`] (no DB) / [`open_with_storage`] (open + migrate libsql, build the
//! `Arc<dyn Storage>`).
//!
//! **Config owns workspace-open** (CF-D): it discovers `.unblock/`, resolves paths, opens/migrates
//! libsql, and constructs the `Arc<dyn Storage>` carried by [`WorkspaceContext`]; the engine
//! consumes the context via `Session::open` — it does not construct storage. The full layered
//! TOML/env/CLI resolver (CLI > env `UNBLOCK_*` > project `.unblock/config.toml` > defaults) lands
//! **additively at T1.3** without changing any type or facade signature pinned here.
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
pub use discovery::{discover_optional_unblock_dir, discover_unblock_dir};
pub use env::{EnvOverrides, EnvSource};
pub use error::ConfigError;
pub use keys::{KeyClass, RUNTIME_KEYS, RuntimeKey, STARTUP_KEYS, StartupKey, classify};
pub use paths::ConfigPaths;
pub use schema::{ProjectConfig, RemoteTable};

// `OutputFormat` is owned once in `unblock-model` (G-7/CF-J) and re-exported here so consumers of
// `ResolvedConfig.output_format` reach it through config without a second definition.
pub use unblock_model::OutputFormat;
