//! The two workspace contexts and their open facades (CF-D, spine §4).
//!
//! - [`ResolvedContext`] — resolve-only, **no storage** (for `init` / doctor pre-checks / anything
//!   that must not open the DB); returned by [`open_workspace`].
//! - [`WorkspaceContext`] — **storage-bearing** (`storage: Arc<dyn Storage>`, NON-OPTIONAL);
//!   returned by [`open_with_storage`], consumed by the engine's `Session::open`.
//!
//! Both bundle `config: ResolvedConfig` (config-owned VALUES) + `paths: ConfigPaths` (config-owned
//! PATHS) + `workspace_dir` (project root) + `actor` (authoritative, spine §4.1). **Config builds
//! storage; the engine never does** (CF-D).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use unblock_storage::{LibsqlStorage, Storage};

use crate::actor::{ProcessEnv, resolve_actor};
use crate::config::ResolvedConfig;
use crate::discovery::discover_workspace_dir;
use crate::error::{ConfigError, DbOpenFailedSnafu, MigrationFailedSnafu};
use crate::paths::ConfigPaths;

use snafu::ResultExt;

/// The resolve-only context (no storage handle) — discovery + resolved config + derived paths
/// (spine §4).
///
/// Returned by [`open_workspace`] for paths that must **not** open or create the database
/// (`init`, doctor pre-checks, completions).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// The project root — the directory that CONTAINS `.unblock/` (distinct from
    /// `paths.unblock_dir`).
    pub workspace_dir: PathBuf,
    /// The authoritative actor (spine §4.1) — NOT inside [`ResolvedConfig`].
    pub actor: String,
    /// The config-owned resolved values (DEFAULTED at T1.3a).
    pub config: ResolvedConfig,
    /// The config-owned resolved `.unblock/` + db/jsonl paths.
    pub paths: ConfigPaths,
}

/// The storage-bearing context (CF-D, spine §4.1) — discovery + open/migrate libsql + the built
/// `Arc<dyn Storage>`.
///
/// Returned by [`open_with_storage`] and consumed by the engine's `Session::open(ctx, cfg)`. The
/// `storage` field is **NON-OPTIONAL** (G-5): it is always present once built, so `Session::open`
/// never unwraps an `Option`.
#[derive(Clone)]
pub struct WorkspaceContext {
    /// The built backend handle (NON-OPTIONAL, G-5). Config constructs it; the engine consumes it.
    pub storage: Arc<dyn Storage>,
    /// The project root — the directory that CONTAINS `.unblock/`.
    pub workspace_dir: PathBuf,
    /// The authoritative actor (spine §4.1) — NOT inside [`ResolvedConfig`].
    pub actor: String,
    /// The config-owned resolved values (DEFAULTED at T1.3a).
    pub config: ResolvedConfig,
    /// The config-owned resolved `.unblock/` + db/jsonl paths.
    pub paths: ConfigPaths,
}

/// Resolve a workspace **without** opening the database (spine §4, G-5 option b).
///
/// Performs `.unblock/` upward discovery from `start`, builds the defaulted [`ResolvedConfig`],
/// derives the [`ConfigPaths`], resolves the actor (`UNBLOCK_ACTOR` → `$USER` → `"unblock"`), and
/// returns a [`ResolvedContext`]. It **never** opens or creates the database file.
///
/// # Errors
///
/// - [`ConfigError::WorkspaceNotFound`] if no `.unblock/` is found at or above `start`.
/// - [`ConfigError::ActorUnresolved`] if no actor resolves (unreachable in T1.3a — the literal
///   default always resolves).
pub async fn open_workspace(start: &Path) -> Result<ResolvedContext, ConfigError> {
    let workspace_dir = discover_workspace_dir(start)?;
    let config = ResolvedConfig::default();
    let paths = ConfigPaths::derive(&workspace_dir, &config);
    let actor = resolve_actor(&ProcessEnv)?;

    Ok(ResolvedContext {
        workspace_dir,
        actor,
        config,
        paths,
    })
}

/// Open a workspace **with** storage (CF-D, spine §4.1) — the canonical workspace-open facade.
///
/// Performs `.unblock/` upward discovery from `start`, derives the [`ConfigPaths`], opens libsql via
/// [`LibsqlStorage::open_local`] (which creates the db file inside the existing `.unblock/` but does
/// **not** migrate), then applies migrations via [`Storage::migrate`] (two explicit calls), coerces
/// the backend to `Arc<dyn Storage>`, resolves the actor, and returns a [`WorkspaceContext`]. The
/// engine consumes it via `Session::open(ctx, cfg)`; **config builds storage, the engine never
/// does**.
///
/// # Errors
///
/// - [`ConfigError::WorkspaceNotFound`] if no `.unblock/` is found at or above `start`.
/// - [`ConfigError::DbOpenFailed`] if `open_local` fails (forwards the inner storage code).
/// - [`ConfigError::MigrationFailed`] if `migrate()` fails (forwards the inner storage code).
/// - [`ConfigError::ActorUnresolved`] if no actor resolves (unreachable in T1.3a).
pub async fn open_with_storage(start: &Path) -> Result<WorkspaceContext, ConfigError> {
    let workspace_dir = discover_workspace_dir(start)?;
    let config = ResolvedConfig::default();
    let paths = ConfigPaths::derive(&workspace_dir, &config);

    // open_local creates the db FILE inside the existing `.unblock/` but does NOT migrate.
    let storage = LibsqlStorage::open_local(&paths.db_path)
        .await
        .context(DbOpenFailedSnafu)?;
    // migrate() sets up the schema (two explicit calls — DbOpenFailed wraps open, MigrationFailed
    // wraps migrate).
    storage.migrate().await.context(MigrationFailedSnafu)?;

    let storage: Arc<dyn Storage> = Arc::new(storage);
    let actor = resolve_actor(&ProcessEnv)?;

    Ok(WorkspaceContext {
        storage,
        workspace_dir,
        actor,
        config,
        paths,
    })
}
