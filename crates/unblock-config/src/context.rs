//! The two workspace contexts and their open facades (CF-D, spine §4).
//!
//! - [`ResolvedContext`] — resolve-only, **no storage**; returned by [`open_workspace`] /
//!   [`open_workspace_with_cli`] (used by `unblock agents`). The DB layer is INVISIBLE here: the
//!   runtime keys resolve over FIVE layers, a documented divergence (PRD §4 D57 clause (6)).
//! - [`WorkspaceContext`] — **storage-bearing** (`storage: Arc<dyn Storage>`, NON-OPTIONAL);
//!   returned by [`open_with_storage`] / [`open_with_storage_with_cli`], consumed by the engine's
//!   `Session::open`. The runtime keys resolve over all SIX layers.
//! - [`OpenedContext`] — either of the two, returned by [`open_with_cli`], the one entry that reads
//!   the open-mode knobs `no_db` and `storage_open` (PRD §4 D58 clause (8)).
//!
//! Both bundle `config: ResolvedConfig` (config-owned VALUES, PARTITIONED since D57 into
//! `startup`/`runtime` — the actor lives in `config.runtime.actor`) + `paths: ConfigPaths`
//! (config-owned PATHS) + `workspace_dir` (project root). **Config builds storage; the engine never
//! does** (CF-D).
//!
//! **TWO-PHASE OPEN (PRD §4 D57 clause (6)).** The storage-bearing facade (1) discovers, loads the
//! project and user files and the env layer, resolves the STARTUP keys and `ConfigPaths`; (2) opens
//! libsql with `db_path` + `write_lock_timeout_ms`, records `schema_version_before_migrate`,
//! migrates (schema step 3 runs here on a stamp-2 database); (3) reads `read_config()` — a failure
//! is [`ConfigError::DbOpenFailed`], forwarding the storage code and hint — and resolves the RUNTIME
//! keys over all six layers with the clause (2a) faults (a bad row never fails the open); (4) builds
//! the [`ConfigReloader`] and returns the context.
//!
//! **THE LOST-DATABASE REFUSAL (PRD §4 D58 clause (7)).** Between phases (1) and (2) of every
//! storage-bearing open, a database that [`unblock_storage::lost_database`] reports lost (absent or
//! 0 bytes beside the `.write.lock` marker) is refused with [`ConfigError::DatabaseMissing`] and
//! nothing is created, unless [`CliOverrides::create_db`] is set. The runtime keys are validated
//! first over the empty DB layer a lost database has, so a malformed or invalid config file still
//! fails exit 7 first.
//!
//! **OPEN MODES (PRD §4 D58 clause (8)).** [`StorageOpenMode::Migrate`] is phase (2) as above.
//! [`StorageOpenMode::ReadOnly`] opens through [`LibsqlStorage::open_local_read_only`] and
//! [`StorageOpenMode::NoMigrate`] through [`LibsqlStorage::open_local`]; neither migrates, both read
//! the `config` rows only on a stamp of 3 or more (an older database
//! has no `config` table, so its DB layer is empty), and both open a stamp above
//! [`unblock_storage::CURRENT_SCHEMA_VERSION`].
//!
//! **Facade model (FORK-1).** The `&Path` facades are PERMANENT; they pass `start` as the WALK-UP
//! START — NOT as `cli.dir` (MF-2). The shared body takes its [`EnvSource`] as an internal parameter
//! (the D39 `discover_unblock_dir` precedent); production passes [`ProcessEnv`], and the
//! `#[doc(hidden)]` `_with_env` entries let in-process tests in other crates inject one (NFR-16).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use snafu::ResultExt;
use unblock_storage::{LibsqlStorage, Storage, lost_database};

use crate::cli::{CliOverrides, StorageOpenMode};
use crate::config::{ResolvedConfig, WorkspaceConfig, resolve_runtime, resolve_startup};
use crate::discovery::{WorkspaceSource, discover_workspace};
use crate::env::{EnvOverrides, EnvSource, ProcessEnv};
use crate::error::{ConfigError, DbOpenFailedSnafu, IoSnafu, MigrationFailedSnafu};
use crate::paths::ConfigPaths;
use crate::reload::ConfigReloader;
use crate::schema::{CONFIG_FILENAME, ProjectConfig};
use crate::user_config::{load_user_config, user_config_path};

/// The resolve-only context (no storage handle) — discovery + resolved config + derived paths
/// (spine §4).
///
/// Returned by [`open_workspace`] / [`open_workspace_with_cli`] for paths that must **not** open or
/// create the database. Its runtime keys resolve over FIVE layers — the DB layer is invisible (PRD
/// §4 D57 clause (6)).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// The project root — the directory that CONTAINS `.unblock/` (distinct from
    /// `paths.unblock_dir`).
    pub workspace_dir: PathBuf,
    /// The config-owned resolved values (the actor is `config.runtime.actor`).
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
    /// The config-owned resolved values over all SIX layers (the actor is `config.runtime.actor`;
    /// the DB layer's faulty rows are `config.runtime.db_faults`).
    pub config: ResolvedConfig,
    /// Re-resolves the runtime keys for a LIVE session (D57 clause (8)); moved into the `Session`.
    /// A context built by struct literal passes `ConfigReloader::default()` for a snapshot of pure
    /// defaults, or a reloader that reproduces its snapshot (see [`ConfigReloader`]).
    pub reloader: ConfigReloader,
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

/// The schema stamp at which the D57 `config` table exists (schema step 3). A non-migrating open
/// reads the DB layer only at or above it.
const CONFIG_TABLE_SCHEMA_VERSION: i64 = 3;

/// The D31 advisory lock marker beside the database file, the one storage's `lost_database` checks.
const WRITE_LOCK_FILE_NAME: &str = ".write.lock";

/// What [`open_with_cli`] opened: the resolve-only context when `cli.no_db` is set, else the
/// storage-bearing one (PRD §4 D58 clause (8)).
#[derive(Clone)]
#[allow(clippy::large_enum_variant)] // the spine §4 shape; one value per open, never stored in bulk.
pub enum OpenedContext {
    /// Discovery and the resolved config only; the database is untouched (`cli.no_db`).
    Resolved(ResolvedContext),
    /// The storage-bearing context, opened per `cli.storage_open`.
    Storage(WorkspaceContext),
}

/// Phase (1) of every open, fully resolved and OWNED (no borrow of the [`EnvSource`] survives it,
/// so the storage-bearing body below holds no `&dyn EnvSource` across an `.await`).
struct Resolution {
    workspace_dir: PathBuf,
    startup: WorkspaceConfig,
    paths: ConfigPaths,
    source: WorkspaceSource,
    project: ProjectConfig,
    user: ProjectConfig,
    env: EnvOverrides,
    os_user: Option<String>,
    user_file: Option<PathBuf>,
}

/// Phase (1): discovery, the project file, the user file (through [`load_user_config`], the single
/// home of every user-file refusal), the env layer, the STARTUP keys and the confined paths.
///
/// `discover_workspace` honors `cli.dir`/`cli.db` (explicit, no walk-up) else walks up from `start`;
/// the discovered `unblock_dir` is canonicalized (Seam C). [`ConfigPaths::resolve`] confines the
/// artifact paths (Seam B).
fn resolve_workspace(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<Resolution, ConfigError> {
    // `CLAUDE_PROJECT_DIR`/`$HOME` are read via the env seam INSIDE discovery (D39). `source` is the
    // winning discovery tier.
    let discovered = discover_workspace(start, cli, env)?;
    let unblock_dir = discovered.unblock_dir;
    let source = discovered.source;
    // workspace_dir is the project root that CONTAINS the `.unblock`/`_unblock` dir.
    let workspace_dir = unblock_dir
        .parent()
        .map_or_else(|| unblock_dir.clone(), Path::to_path_buf);

    let project = ProjectConfig::load(&unblock_dir)?;
    let user = load_user_config(env)?;
    let env_overrides = EnvOverrides::from_source(env)?;
    let startup = resolve_startup(cli, &env_overrides, &project)?;
    let paths = ConfigPaths::resolve(&unblock_dir, &startup, cli)?;

    Ok(Resolution {
        workspace_dir,
        startup,
        paths,
        source,
        project,
        user,
        env: env_overrides,
        os_user: env.get("USER"),
        user_file: user_config_path(env),
    })
}

/// Resolve a workspace **without** opening the database — the permanent `&Path` facade (spine §4,
/// G-5 option b).
///
/// Passes `start` as the WALK-UP START (FORK-1/MF-2): the full layered resolution still runs, only
/// the CLI layer is empty. It **never** opens or creates the database file, so the runtime keys
/// resolve over five layers (the DB layer is invisible, PRD §4 D57 clause (6)).
///
/// # Errors
///
/// - [`ConfigError::WorkspaceNotFound`] if no `.unblock/`/`_unblock/` is found at or above `start`.
/// - [`ConfigError::Parse`] / [`ConfigError::Io`] / [`ConfigError::InvalidValue`] /
///   [`ConfigError::StartupKeyInRuntimeLayer`] from config resolution (a malformed or
///   credential-bearing project or user file, a startup key or `jsonl_export` in the user file, an
///   over-bound actor or `search_cap`, a path-injecting filename).
pub async fn open_workspace(start: &Path) -> Result<ResolvedContext, ConfigError> {
    open_workspace_from(Some(start), &CliOverrides::default(), &ProcessEnv)
}

/// Open a workspace **with** storage — the permanent `&Path` facade (CF-D, spine §4.1).
///
/// Passes `start` as the WALK-UP START (FORK-1/MF-2).
///
/// # Errors
///
/// As [`open_workspace`], plus:
/// - [`ConfigError::DatabaseMissing`] if the database is lost (absent or 0 bytes beside the
///   `.write.lock` marker; PRD §4 D58 clause (7)); nothing is created.
/// - [`ConfigError::Io`] if the lost-database check cannot stat the database or the marker.
/// - [`ConfigError::DbOpenFailed`] if `open_local` — or the D46 pre-migration `schema_version()`
///   read, or the D57 `read_config()` read — fails (forwards the inner storage code and hint).
/// - [`ConfigError::MigrationFailed`] if `migrate()` fails (forwards the inner storage code + hint).
///
/// A faulty database `config` row NEVER fails the open (D57 clause (2a)): it is recorded in
/// `config.runtime.db_faults`, and a blocking one fail-closes the engine `Session` instead.
pub async fn open_with_storage(start: &Path) -> Result<WorkspaceContext, ConfigError> {
    let resolution = resolve_workspace(Some(start), &CliOverrides::default(), &ProcessEnv)?;
    open_resolved(
        resolution,
        &CliOverrides::default(),
        StorageOpenMode::Migrate,
    )
    .await
}

/// The additive CLI overload of [`open_workspace`] (FORK-1) — threads `--dir`/`--db`/`--actor`/
/// `--output-format` through resolution. `cli.dir`/`cli.db` are EXPLICIT (no walk-up, MF-2); when
/// both are unset, discovery walks up from the CWD.
///
/// # Errors
///
/// As [`open_workspace`].
pub async fn open_workspace_with_cli(cli: &CliOverrides) -> Result<ResolvedContext, ConfigError> {
    open_workspace_from(None, cli, &ProcessEnv)
}

/// The additive CLI overload of [`open_with_storage`] (FORK-1). It always migrates; it ignores
/// `cli.no_db` and `cli.storage_open`, which only [`open_with_cli`] reads.
///
/// # Errors
///
/// As [`open_with_storage`].
pub async fn open_with_storage_with_cli(
    cli: &CliOverrides,
) -> Result<WorkspaceContext, ConfigError> {
    let resolution = resolve_workspace(None, cli, &ProcessEnv)?;
    open_resolved(resolution, cli, StorageOpenMode::Migrate).await
}

/// Open a workspace per the open-mode knobs of `cli` (PRD §4 D58 clause (8)).
///
/// - `cli.no_db` → [`OpenedContext::Resolved`], exactly [`open_workspace_with_cli`]: discovery and
///   the resolved config; the database is never touched (the `doctor` pre-open phase).
/// - Otherwise [`OpenedContext::Storage`], opened per `cli.storage_open`:
///   [`StorageOpenMode::Migrate`] is exactly [`open_with_storage_with_cli`];
///   [`StorageOpenMode::ReadOnly`] opens read-only (no migrate, no `.write.lock`, no write);
///   [`StorageOpenMode::NoMigrate`] opens read-write without migrating. In the two non-migrating
///   modes `schema_version_before_migrate` is the stamp as found, the `config` rows are read only on
///   a stamp of 3 or more, and a stamp above this build's opens instead of failing.
///
/// # Errors
///
/// `cli.no_db`: as [`open_workspace`]. Otherwise as [`open_with_storage`]; `MigrationFailed` only in
/// `Migrate` mode. A read-only open that meets a hot journal fails with [`ConfigError::DbOpenFailed`]
/// for which [`ConfigError::is_read_only_rollback`] is true.
pub async fn open_with_cli(cli: &CliOverrides) -> Result<OpenedContext, ConfigError> {
    open_with_cli_from(None, cli, &ProcessEnv).await
}

/// **Test entry (NFR-16) — production code never calls it.** The resolve-only open with an
/// injected [`EnvSource`]: `start` is the walk-up seed exactly as in [`open_workspace`] (`None` walks
/// up from the CWD), so in-process tests in other crates can open a workspace without reading the
/// process environment's `$HOME`, `XDG_CONFIG_HOME` or `UNBLOCK_*`.
///
/// # Errors
///
/// As [`open_workspace`].
#[doc(hidden)]
pub async fn open_workspace_with_env(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<ResolvedContext, ConfigError> {
    open_workspace_from(start, cli, env)
}

/// **Test entry (NFR-16) — production code never calls it.** The storage-bearing open with an
/// injected [`EnvSource`]; `start` as in [`open_workspace_with_env`].
///
/// # Errors
///
/// As [`open_with_storage`].
#[doc(hidden)]
pub async fn open_with_storage_with_env(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<WorkspaceContext, ConfigError> {
    let resolution = resolve_workspace(start, cli, env)?;
    open_resolved(resolution, cli, StorageOpenMode::Migrate).await
}

/// **Test entry (NFR-16) — production code never calls it.** [`open_with_cli`] with an injected
/// [`EnvSource`]; `start` as in [`open_workspace_with_env`].
///
/// # Errors
///
/// As [`open_with_cli`].
#[doc(hidden)]
pub async fn open_with_cli_with_env(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<OpenedContext, ConfigError> {
    open_with_cli_from(start, cli, env).await
}

/// Shared body of [`open_with_cli`]: `no_db` selects the resolve-only open, else the storage-bearing
/// one in `cli.storage_open` mode.
async fn open_with_cli_from(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<OpenedContext, ConfigError> {
    if cli.no_db {
        return open_workspace_from(start, cli, env).map(OpenedContext::Resolved);
    }
    let resolution = resolve_workspace(start, cli, env)?;
    open_resolved(resolution, cli, cli.storage_open)
        .await
        .map(OpenedContext::Storage)
}

/// Shared resolve-only body (no DB): phase (1), then the runtime keys over FIVE layers.
fn open_workspace_from(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<ResolvedContext, ConfigError> {
    let Resolution {
        workspace_dir,
        startup,
        paths,
        source,
        project,
        user,
        env,
        os_user,
        user_file: _,
    } = resolve_workspace(start, cli, env)?;
    let runtime = resolve_runtime(cli, &env, &project, &user, &[], os_user.as_deref())?;

    Ok(ResolvedContext {
        workspace_dir,
        config: ResolvedConfig {
            startup: startup.startup(),
            runtime,
        },
        paths,
        source,
    })
}

/// The lost-database refusal (PRD §4 D58 clause (7)): `DatabaseMissing` when `db_path` is lost and
/// `cli.create_db` is unset. A lost database has an empty DB layer, so the runtime keys are first
/// validated over exactly that layer — a config file that would fail the open with exit 7 still does.
fn refuse_lost_database(
    db_path: &Path,
    cli: &CliOverrides,
    env: &EnvOverrides,
    project: &ProjectConfig,
    user: &ProjectConfig,
    os_user: Option<&str>,
) -> Result<(), ConfigError> {
    if cli.create_db {
        return Ok(());
    }
    // A database `stat` failure names the database. A later `lost_database` failure is the
    // `.write.lock` check beside it, so that error names the marker.
    if let Err(source) = std::fs::metadata(db_path)
        && source.kind() != std::io::ErrorKind::NotFound
    {
        return Err(source).context(IoSnafu { path: db_path });
    }
    let marker = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(WRITE_LOCK_FILE_NAME);
    let lost = lost_database(db_path).context(IoSnafu { path: marker })?;
    if !lost {
        return Ok(());
    }
    resolve_runtime(cli, env, project, user, &[], os_user)?;
    Err(ConfigError::DatabaseMissing {
        path: db_path.to_path_buf(),
    })
}

/// Shared storage-bearing body: the lost-database refusal, then phases (2)–(4) over an
/// already-resolved phase (1), opening the database in `mode`.
async fn open_resolved(
    resolution: Resolution,
    cli: &CliOverrides,
    mode: StorageOpenMode,
) -> Result<WorkspaceContext, ConfigError> {
    let Resolution {
        workspace_dir,
        startup,
        paths,
        source,
        project,
        user,
        env,
        os_user,
        user_file,
    } = resolution;

    refuse_lost_database(
        &paths.db_path,
        cli,
        &env,
        &project,
        &user,
        os_user.as_deref(),
    )?;

    // open_local creates the db FILE inside the existing `.unblock/` but does NOT migrate. The
    // resolved `write_lock_timeout_ms` (D31) is threaded DOWN here (L4→L2); the `.write.lock` PATH is
    // derived inside storage from the db-file parent, so there is no L2→L4 back-edge. The read-only
    // open takes no `.write.lock` and so needs no timeout.
    let storage = match mode {
        StorageOpenMode::Migrate | StorageOpenMode::NoMigrate => {
            LibsqlStorage::open_local(&paths.db_path, startup.write_lock_timeout_ms()).await
        }
        StorageOpenMode::ReadOnly => LibsqlStorage::open_local_read_only(&paths.db_path).await,
    }
    .context(DbOpenFailedSnafu)?;
    // D46 clause (10) — RECORD THE PRE-REPAIR STAMP. This is the ONE place it is still observable:
    // the `Migrate` mode migrates next, so every later reader (including `Session::migrate`) sees
    // the POST-repair value. It is the EXISTING pure `Storage::schema_version()` read, wrapped with the
    // SAME `DbOpenFailedSnafu` `open_local` uses one line above: it belongs to that same "establish
    // a usable handle" step and runs BEFORE the ladder, so `MigrationFailed` would mis-label it. A
    // `.unwrap_or(0)`-style fallback is FORBIDDEN: a read failure laundered into `0` is
    // indistinguishable from a genuinely never-migrated database.
    let schema_version_before_migrate =
        storage.schema_version().await.context(DbOpenFailedSnafu)?;
    // migrate() sets up the schema (two explicit calls — DbOpenFailed wraps open, MigrationFailed
    // wraps migrate). Schema step 3 (the D57 `config` table) runs here on a stamp-2 database. The
    // non-migrating modes leave the stamp as found, so below step 3 there is no `config` table.
    let has_config_table = match mode {
        StorageOpenMode::Migrate => {
            storage.migrate().await.context(MigrationFailedSnafu)?;
            true
        }
        StorageOpenMode::ReadOnly | StorageOpenMode::NoMigrate => {
            schema_version_before_migrate >= CONFIG_TABLE_SCHEMA_VERSION
        }
    };
    // D57 clause (6) phase (3): the DB layer. A read failure forwards the storage code and hint as
    // `DbOpenFailed` (no new variant); the rows themselves are decoded TOLERANTLY, so a faulty row
    // never fails the open — it becomes a `db_faults` entry.
    let rows = if has_config_table {
        storage.read_config().await.context(DbOpenFailedSnafu)?
    } else {
        Vec::new()
    };
    let runtime = resolve_runtime(cli, &env, &project, &user, &rows, os_user.as_deref())?;
    let reloader = ConfigReloader::new(
        cli.clone(),
        env,
        os_user,
        Some(paths.unblock_dir.join(CONFIG_FILENAME)),
        user_file,
    );

    let storage: Arc<dyn Storage> = Arc::new(storage);

    Ok(WorkspaceContext {
        storage,
        workspace_dir,
        config: ResolvedConfig {
            startup: startup.startup(),
            runtime,
        },
        reloader,
        paths,
        source,
        schema_version_before_migrate,
    })
}
