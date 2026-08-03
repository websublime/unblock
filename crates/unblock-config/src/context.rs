//! The two workspace contexts and their open facades (CF-D, spine §4).
//!
//! - [`ResolvedContext`] — resolve-only, **no storage** (for `init` / doctor pre-checks / anything
//!   that must not open the DB); returned by [`open_workspace`] / [`open_workspace_with_cli`].
//! - [`WorkspaceContext`] — **storage-bearing** (`storage: Arc<dyn Storage>`, NON-OPTIONAL);
//!   returned by [`open_with_storage`] / [`open_with_storage_with_cli`], consumed by the engine's
//!   `Session::open`.
//!
//! Both bundle `config: ResolvedConfig` (config-owned VALUES) + `paths: ConfigPaths` (config-owned
//! PATHS) + `workspace_dir` (project root) + `actor` (authoritative, spine §4.1). **Config builds
//! storage; the engine never does** (CF-D).
//!
//! **Facade model (FORK-1).** The `&Path` facades ([`open_workspace`] / [`open_with_storage`]) are
//! PERMANENT; they **delegate** to the additive `_with_cli` overloads passing `start` as the WALK-UP
//! START parameter (`discover_unblock_dir(Some(start), &CliOverrides::default(), &ProcessEnv)`) — NOT as `cli.dir`
//! (MF-2). The full layered resolution (CLI > env `UNBLOCK_*` > project `config.toml` > defaults)
//! runs in both forms (it replaces the T1.3a defaulting internals).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use snafu::ResultExt;
use unblock_storage::{LibsqlStorage, Storage};

use crate::cli::CliOverrides;
use crate::config::{ResolvedConfig, WorkspaceConfig};
use crate::discovery::{WorkspaceSource, discover_workspace};
use crate::env::{EnvOverrides, ProcessEnv};
use crate::error::{ConfigError, DbOpenFailedSnafu, MigrationFailedSnafu};
use crate::paths::ConfigPaths;
use crate::schema::ProjectConfig;

/// The resolve-only context (no storage handle) — discovery + resolved config + derived paths
/// (spine §4).
///
/// Returned by [`open_workspace`] / [`open_workspace_with_cli`] for paths that must **not** open or
/// create the database (`init`, doctor pre-checks, completions).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// The project root — the directory that CONTAINS `.unblock/` (distinct from
    /// `paths.unblock_dir`).
    pub workspace_dir: PathBuf,
    /// The authoritative actor (spine §4.1) — NOT inside [`ResolvedConfig`].
    pub actor: String,
    /// The config-owned resolved values.
    pub config: ResolvedConfig,
    /// The config-owned resolved `.unblock/` + db/jsonl paths.
    pub paths: ConfigPaths,
    /// Which discovery tier bound the workspace dir (D39 — ADDITIVE). The CLI reports it at startup.
    pub source: WorkspaceSource,
}

/// The storage-bearing context (CF-D, spine §4.1) — discovery + open/migrate libsql + the built
/// `Arc<dyn Storage>`.
///
/// Returned by [`open_with_storage`] / [`open_with_storage_with_cli`] and consumed by the engine's
/// `Session::open(ctx, cfg)`. The `storage` field is **NON-OPTIONAL** (G-5): it is always present
/// once built, so `Session::open` never unwraps an `Option`.
#[derive(Clone)]
pub struct WorkspaceContext {
    /// The built backend handle (NON-OPTIONAL, G-5). Config constructs it; the engine consumes it.
    pub storage: Arc<dyn Storage>,
    /// The project root — the directory that CONTAINS `.unblock/`.
    pub workspace_dir: PathBuf,
    /// The authoritative actor (spine §4.1) — NOT inside [`ResolvedConfig`].
    pub actor: String,
    /// The config-owned resolved values.
    pub config: ResolvedConfig,
    /// The config-owned resolved `.unblock/` + db/jsonl paths.
    pub paths: ConfigPaths,
    /// Which discovery tier bound the workspace dir (D39 — ADDITIVE). The CLI reports it at startup.
    pub source: WorkspaceSource,
    /// The `PRAGMA user_version` this facade observed **BEFORE** it migrated (D46 clause (10) —
    /// ADDITIVE; `0` on a never-migrated database).
    ///
    /// **This is the only moment the pre-repair stamp still exists.** Because this facade migrates on
    /// open (FR-9 single open path), every later reader — `Session::migrate` included — sees the
    /// POST-repair stamp, which is exactly why `unblock migrate` could not report the delta it exists
    /// to report. The cli `migrate` command copies this value out before `Session::open` consumes the
    /// context; the engine IGNORES it (`Session::open` destructures it to `_`), so no L4→L5 contract
    /// moves — the same additive shape D39's `source` takes.
    pub schema_version_before_migrate: i64,
}

/// The fully-resolved per-workspace config: the merged [`WorkspaceConfig`], the resolved actor, the
/// project root, and the resolved [`ConfigPaths`]. Shared by both facades.
struct Resolution {
    workspace_dir: PathBuf,
    config: WorkspaceConfig,
    paths: ConfigPaths,
    source: WorkspaceSource,
}

/// Run the shared discovery + layered resolution + path resolution for a given `start`/`cli`.
///
/// `discover_unblock_dir` honors `cli.dir`/`cli.db` (explicit, no walk-up) else walks up from
/// `start`; the discovered `unblock_dir` is canonicalized (Seam C). The project config is loaded from
/// `<unblock_dir>/config.toml` (missing = defaults), the env layer is read via [`ProcessEnv`], and
/// the layered [`WorkspaceConfig::resolve`] produces the merged value (FORK-4 actor + Seam A
/// validation). [`ConfigPaths::resolve`] then confines the artifact paths (Seam B).
fn resolve_workspace(start: Option<&Path>, cli: &CliOverrides) -> Result<Resolution, ConfigError> {
    // `CLAUDE_PROJECT_DIR`/`$HOME` are read via the process-env seam INSIDE discovery (D39); the
    // `&Path`/`_with_cli` facades stay signature-stable. `source` is the winning discovery tier.
    let discovered = discover_workspace(start, cli, &ProcessEnv)?;
    let unblock_dir = discovered.unblock_dir;
    let source = discovered.source;
    // workspace_dir is the project root that CONTAINS the `.unblock`/`_unblock` dir.
    let workspace_dir = unblock_dir
        .parent()
        .map_or_else(|| unblock_dir.clone(), Path::to_path_buf);

    let project = ProjectConfig::load(&unblock_dir)?;
    let env = EnvOverrides::from_process_env()?;
    let config = WorkspaceConfig::resolve(cli, &env, &project, &ProcessEnv)?;
    let paths = ConfigPaths::resolve(&unblock_dir, &config, cli)?;

    Ok(Resolution {
        workspace_dir,
        config,
        paths,
        source,
    })
}

/// Resolve a workspace **without** opening the database — the permanent `&Path` facade (spine §4,
/// G-5 option b).
///
/// Delegates to [`open_workspace_with_cli`] passing `start` as the WALK-UP START (FORK-1/MF-2): the
/// full layered resolution still runs (env `UNBLOCK_*` + project `config.toml` + defaults), only the
/// CLI layer is empty. It **never** opens or creates the database file.
///
/// # Errors
///
/// - [`ConfigError::WorkspaceNotFound`] if no `.unblock/`/`_unblock/` is found at or above `start`.
/// - [`ConfigError::Parse`] / [`ConfigError::Io`] / [`ConfigError::InvalidValue`] from config
///   resolution (a malformed/credential-bearing `config.toml`, an over-bound actor, a path-injecting
///   filename).
pub async fn open_workspace(start: &Path) -> Result<ResolvedContext, ConfigError> {
    open_workspace_from(Some(start), &CliOverrides::default())
}

/// Open a workspace **with** storage — the permanent `&Path` facade (CF-D, spine §4.1).
///
/// Delegates to [`open_with_storage_with_cli`] passing `start` as the WALK-UP START (FORK-1/MF-2).
///
/// # Errors
///
/// As [`open_workspace`], plus:
/// - [`ConfigError::DbOpenFailed`] if `open_local` — or the D46 pre-migration `schema_version()`
///   read — fails (forwards the inner storage code, and since D46 its hint).
/// - [`ConfigError::MigrationFailed`] if `migrate()` fails (forwards the inner storage code + hint).
pub async fn open_with_storage(start: &Path) -> Result<WorkspaceContext, ConfigError> {
    open_with_storage_from(Some(start), &CliOverrides::default()).await
}

/// The additive CLI overload of [`open_workspace`] (FORK-1) — threads `--dir`/`--db`/`--actor`/
/// `--output-format` through resolution. `cli.dir`/`cli.db` are EXPLICIT (no walk-up, MF-2); when
/// both are unset, discovery walks up from the CWD.
///
/// # Errors
///
/// As [`open_workspace`].
pub async fn open_workspace_with_cli(cli: &CliOverrides) -> Result<ResolvedContext, ConfigError> {
    open_workspace_from(None, cli)
}

/// The additive CLI overload of [`open_with_storage`] (FORK-1).
///
/// # Errors
///
/// As [`open_with_storage`].
pub async fn open_with_storage_with_cli(
    cli: &CliOverrides,
) -> Result<WorkspaceContext, ConfigError> {
    open_with_storage_from(None, cli).await
}

/// Shared resolve-only body (no DB) for both the `&Path` facade and the `_with_cli` overload.
fn open_workspace_from(
    start: Option<&Path>,
    cli: &CliOverrides,
) -> Result<ResolvedContext, ConfigError> {
    let Resolution {
        workspace_dir,
        config,
        paths,
        source,
    } = resolve_workspace(start, cli)?;
    let actor = config.actor().to_string();

    Ok(ResolvedContext {
        workspace_dir,
        actor,
        config: config.into_resolved(),
        paths,
        source,
    })
}

/// Shared open-with-storage body for both the `&Path` facade and the `_with_cli` overload.
async fn open_with_storage_from(
    start: Option<&Path>,
    cli: &CliOverrides,
) -> Result<WorkspaceContext, ConfigError> {
    let Resolution {
        workspace_dir,
        config,
        paths,
        source,
    } = resolve_workspace(start, cli)?;
    let actor = config.actor().to_string();

    // open_local creates the db FILE inside the existing `.unblock/` but does NOT migrate. The
    // resolved `write_lock_timeout_ms` (D31) is threaded DOWN here (L4→L2); the `.write.lock` PATH is
    // derived inside storage from the db-file parent, so there is no L2→L4 back-edge.
    let storage = LibsqlStorage::open_local(&paths.db_path, config.write_lock_timeout_ms())
        .await
        .context(DbOpenFailedSnafu)?;
    // D46 clause (10) — RECORD THE PRE-REPAIR STAMP. This is the ONE place it is still observable:
    // the next line migrates, so every later reader (including `Session::migrate`) sees the
    // POST-repair value. It is the EXISTING pure `Storage::schema_version()` read (in the trait since
    // D27/AF-2, so no trait surface moves), wrapped with the SAME `DbOpenFailedSnafu` `open_local`
    // uses one line above: it belongs to that same "establish a usable handle" step and runs BEFORE
    // the ladder, so `MigrationFailed` would mis-label it — and no third `ConfigError` variant is
    // minted. A `.unwrap_or(0)`-style fallback is FORBIDDEN (clippy-legal and silently wrong): a read
    // failure laundered into `0` is indistinguishable from a genuinely never-migrated database, so
    // every open would report `0` -> CURRENT `applied: true` — a fabricated "applied" manufactured
    // out of an error, on precisely the report D46 exists to make honest.
    let schema_version_before_migrate =
        storage.schema_version().await.context(DbOpenFailedSnafu)?;
    // migrate() sets up the schema (two explicit calls — DbOpenFailed wraps open, MigrationFailed
    // wraps migrate).
    storage.migrate().await.context(MigrationFailedSnafu)?;

    let storage: Arc<dyn Storage> = Arc::new(storage);

    Ok(WorkspaceContext {
        storage,
        workspace_dir,
        actor,
        config: config.into_resolved(),
        paths,
        source,
        schema_version_before_migrate,
    })
}
