//! `unblock-config` (L4) — the layered configuration resolver, `.unblock/` workspace discovery,
//! config-owned value/path types, and the two workspace-open facades (CF-D, spine §4).
//!
//! This crate is the **full layered resolver**. Since D57 (v1.1) it merges configuration across SIX
//! layers in ONE strict precedence order — **CLI ([`CliOverrides`]) > env `UNBLOCK_*`
//! ([`EnvOverrides`]) > project `.unblock/config.toml` ([`ProjectConfig`]) > user `config.toml`
//! ([`load_user_config`]) > the database `config` table ([`decode_db_rows`]) > defaults** (highest
//! wins per field) — and ENFORCES the startup/runtime partition ([`keys`]: [`STARTUP_KEYS`] /
//! [`RUNTIME_KEYS`] / [`LAYER_KEYS`], [`classify`]): the user and DB layers carry the three LAYER
//! keys only. It delivers the config-owned value type [`ResolvedConfig`] (partitioned into
//! [`StartupConfig`] and [`RuntimeConfig`]) and path type [`ConfigPaths`], the two contexts
//! [`ResolvedContext`] (resolve-only, no storage) and [`WorkspaceContext`] (storage-bearing), the
//! live-session [`ConfigReloader`], and the per-crate [`ConfigError`].
//!
//! **Discovery** walks up for the nearest dir named `.unblock` **or** `_unblock` (the monorepo alias,
//! FORK-2/D8); the discovered dir is **canonicalized** so artifacts are confined to the canonical
//! subtree (FORK-3, NFR-18). An explicit `--dir`/`UNBLOCK_DIR` is used directly with no walk-up
//! (MF-2). [`UNBLOCK_DIR_NAMES`], [`has_unblock_dir_name`] and [`probe_workspace_root`] hold the
//! workspace-dir names and the child probe. Discovery and `unblock init` share them, so the two
//! agree on which directory a root holds (spine §4 D39).
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
//! Resolve a workspace without opening the database (the resolve-only facade; the injected
//! environment keeps the example independent of the caller's own `$HOME`/`UNBLOCK_*`, NFR-16):
//!
//! ```
//! use unblock_config::{CliOverrides, EnvSource, open_workspace_with_env};
//!
//! struct NoEnv;
//! impl EnvSource for NoEnv {
//!     fn get(&self, _key: &str) -> Option<String> {
//!         None
//!     }
//! }
//!
//! let rt = tokio::runtime::Runtime::new().unwrap();
//! rt.block_on(async {
//!     let workspace = tempfile::tempdir().unwrap();
//!     std::fs::create_dir(workspace.path().join(".unblock")).unwrap();
//!
//!     let ctx = open_workspace_with_env(Some(workspace.path()), &CliOverrides::default(), &NoEnv)
//!         .await
//!         .expect("resolve");
//!     // The discovered dir is canonicalized (FORK-3), so compare against the canonical tempdir.
//!     let canon = workspace.path().canonicalize().unwrap();
//!     assert_eq!(ctx.paths.unblock_dir, canon.join(".unblock"));
//!     // resolve-only never opens or creates the database file:
//!     assert!(!ctx.paths.db_path.exists());
//!     assert_eq!(ctx.config.runtime.actor, "unblock");
//! });
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod actor;
mod cli;
mod config;
mod context;
mod db_layer;
mod discovery;
mod env;
mod error;
pub mod keys;
mod merge;
mod paths;
mod reload;
mod schema;
mod user_config;

pub use cli::CliOverrides;
pub use config::{ResolvedConfig, RuntimeConfig, RuntimeSources, StartupConfig, WorkspaceConfig};
pub use context::{
    ResolvedContext, WorkspaceContext, open_with_storage, open_with_storage_with_cli,
    open_with_storage_with_env, open_workspace, open_workspace_with_cli, open_workspace_with_env,
};
pub use db_layer::decode_db_rows;
pub use discovery::{
    UNBLOCK_DIR_NAMES, WorkspaceSource, discover_optional_unblock_dir, discover_unblock_dir,
    has_unblock_dir_name, probe_workspace_root,
};
pub use env::{EnvOverrides, EnvSource, ProcessEnv};
pub use error::ConfigError;
pub use keys::{
    KeyClass, LAYER_KEYS, RUNTIME_KEYS, RuntimeKey, STARTUP_KEYS, StartupKey, classify,
};
pub use paths::ConfigPaths;
pub use reload::{ConfigReloader, canonicalize_runtime_value, classify_runtime_key};
pub use schema::{ProjectConfig, RemoteTable};
pub use user_config::{load_user_config, user_config_path, user_output_format};

// `OutputFormat` is owned once in `unblock-model` (G-7/CF-J) and re-exported here so consumers of
// `RuntimeConfig.output_format` reach it through config without a second definition.
pub use unblock_model::OutputFormat;
