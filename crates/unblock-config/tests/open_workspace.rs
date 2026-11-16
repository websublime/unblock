//! End-to-end facade tests (T1.3a): `open_workspace` (resolve-only, no DB) and `open_with_storage`
//! (discover + `open_local` + migrate + build `Arc<dyn Storage>`), exercised on real `tempfile`
//! `.unblock/` trees.

use std::fs;

use chrono::{TimeZone, Utc};
use std::path::Path;

use unblock_config::{
    CliOverrides, ConfigError, EnvSource, OpenedContext, ResolvedContext, StorageOpenMode,
    WorkspaceContext, open_with_cli_with_env, open_with_storage_with_env, open_workspace_with_env,
};

// D57 / NFR-16: every cell opens through the `#[doc(hidden)]` `_with_env` entries over an EMPTY
// environment, so no developer's or runner's own user file (`$HOME`/`XDG_CONFIG_HOME`) or
// `UNBLOCK_*` can change a cell's actor, format or snapshot. The helpers below keep the production
// facades' names and argument shapes (`start` is the walk-up seed; the `_with_cli` forms walk up
// from the CWD unless `cli.dir`/`cli.db` is explicit).
struct NoEnv;
impl EnvSource for NoEnv {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
}

async fn open_workspace(start: &Path) -> Result<ResolvedContext, ConfigError> {
    open_workspace_with_env(Some(start), &CliOverrides::default(), &NoEnv).await
}

async fn open_with_storage(start: &Path) -> Result<WorkspaceContext, ConfigError> {
    open_with_storage_with_env(Some(start), &CliOverrides::default(), &NoEnv).await
}

async fn open_workspace_with_cli(cli: &CliOverrides) -> Result<ResolvedContext, ConfigError> {
    open_workspace_with_env(None, cli, &NoEnv).await
}

async fn open_with_storage_with_cli(cli: &CliOverrides) -> Result<WorkspaceContext, ConfigError> {
    open_with_storage_with_env(None, cli, &NoEnv).await
}

async fn open_with_cli(cli: &CliOverrides) -> Result<OpenedContext, ConfigError> {
    open_with_cli_with_env(None, cli, &NoEnv).await
}
use unblock_error::{CodedError, ErrorCode};
use unblock_model::Issue;

/// Build a deterministic `Issue` for the facade round-trip / reopen tests (mirrors the existing
/// round-trip test's chrono-timestamp pattern).
fn sample_issue(id: &str, title: &str) -> Issue {
    Issue {
        id: id.to_string(),
        title: title.to_string(),
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        ..Issue::default()
    }
}

/// Create a fresh `.unblock/` workspace under a tempdir and return the guard + the workspace root.
fn fresh_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join(".unblock")).expect("mkdir .unblock");
    dir
}

#[tokio::test]
async fn open_workspace_resolves_paths_without_creating_the_db() {
    let workspace = fresh_workspace();

    let ctx = open_workspace(workspace.path()).await.expect("resolve");

    // workspace_dir is the project root that CONTAINS `.unblock/`. The discovered dir is now
    // CANONICALIZED (FORK-3/Seam C), so compare against the canonicalized tempdir (macOS maps
    // /var -> /private/var). workspace_dir is the parent of the canonicalized `.unblock`.
    let canon_ws = workspace.path().canonicalize().expect("canon tmp");
    assert_eq!(ctx.workspace_dir, canon_ws);
    // paths derive from the canonicalized discovered unblock_dir + defaulted filenames.
    assert_eq!(ctx.paths.unblock_dir, canon_ws.join(".unblock"));
    assert_eq!(
        ctx.paths.db_path,
        canon_ws.join(".unblock").join("unblock.db")
    );
    assert_eq!(
        ctx.paths.jsonl_path,
        canon_ws.join(".unblock").join("issues.jsonl")
    );
    // resolve-only MUST NOT open or create the database file.
    assert!(
        !ctx.paths.db_path.exists(),
        "open_workspace must not create the db file"
    );
    // actor always resolves (UNBLOCK_ACTOR / $USER / "unblock").
    assert!(!ctx.config.runtime.actor.is_empty());
}

#[tokio::test]
async fn open_with_storage_opens_migrates_and_yields_a_usable_storage() {
    let workspace = fresh_workspace();

    let ctx = open_with_storage(workspace.path())
        .await
        .expect("open with storage");

    // The db file now exists inside `.unblock/` (open_local created it).
    assert!(
        ctx.paths.db_path.exists(),
        "open_local must create the db file"
    );

    // The returned Arc<dyn Storage> is USABLE: a real create + get round-trip through the trait,
    // proving migrate() set up the schema and the handle is live.
    let issue = sample_issue("ub-abc123", "Resolve the workspace");
    let id = ctx
        .storage
        .create_issue(&issue, &ctx.config.runtime.actor)
        .await
        .expect("create");
    assert_eq!(id, "ub-abc123");

    let fetched = ctx
        .storage
        .get_issue(&id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(fetched.title, "Resolve the workspace");

    // integrity_check on the freshly migrated DB reports no problems.
    let problems = ctx.storage.integrity_check().await.expect("integrity");
    assert!(
        problems.is_empty(),
        "fresh db must be healthy: {problems:?}"
    );
}

/// **D46 clause (10) — the facade records the PRE-MIGRATION stamp, and this is the ONE place it is
/// still observable.**
///
/// `0` on a never-migrated directory (the file does not exist yet, so `open_local` creates it
/// unstamped); the current version on a RE-open, because the first open already migrated it. The
/// storage itself is at the current version in BOTH cases — which is exactly why the cli `migrate`
/// command cannot source its delta from `Session::migrate`.
///
/// MUTANT KILLED: moving the `schema_version()` read to AFTER `storage.migrate()` — the first open
/// then reports the post-repair stamp instead of `0`, and the two halves become indistinguishable.
///
/// MUTANT KILLED: `.unwrap_or(0)`-ing a failing read — not observable here, but the re-open half
/// pins the non-zero value a default-to-zero fallback would destroy.
#[tokio::test]
async fn open_with_storage_records_the_stamp_observed_before_migrating() {
    let workspace = fresh_workspace();

    let first = open_with_storage(workspace.path())
        .await
        .expect("first open");
    assert_eq!(
        first.schema_version_before_migrate, 0,
        "a never-migrated workspace is unstamped BEFORE the facade migrates it"
    );
    let migrated = first
        .storage
        .schema_version()
        .await
        .expect("schema_version after the facade migrated");
    assert!(
        migrated > first.schema_version_before_migrate,
        "the facade genuinely advanced it ({} -> {migrated})",
        first.schema_version_before_migrate
    );
    drop(first);

    let second = open_with_storage(workspace.path()).await.expect("re-open");
    assert_eq!(
        second.schema_version_before_migrate, migrated,
        "on a re-open the pre-migration stamp is already current — nothing moves"
    );
}

#[tokio::test]
async fn open_workspace_errors_when_no_workspace_exists() {
    let root = tempfile::tempdir().expect("tempdir");
    let nested = root.path().join("a").join("b");
    fs::create_dir_all(&nested).expect("mkdir nested");

    let err = open_workspace(&nested)
        .await
        .expect_err("must not resolve without a .unblock/");
    // The error code is NotInitialized (asserted via the CodedError bridge in the unit tests);
    // here we assert the public Display surfaces the missing-workspace condition.
    let msg = err.to_string();
    assert!(msg.contains(".unblock"), "unexpected error message: {msg}");
}

#[tokio::test]
async fn open_with_storage_errors_when_no_workspace_exists() {
    let root = tempfile::tempdir().expect("tempdir");
    let nested = root.path().join("x");
    fs::create_dir_all(&nested).expect("mkdir nested");

    // `WorkspaceContext` is not `Debug` (it holds `Arc<dyn Storage>`), so match instead of
    // `expect_err` (which would require `T: Debug`).
    match open_with_storage(&nested).await {
        Ok(_) => panic!("must not open without a .unblock/"),
        Err(err) => {
            let msg = err.to_string();
            assert!(msg.contains(".unblock"), "unexpected error message: {msg}");
        }
    }
}

#[tokio::test]
async fn open_with_storage_surfaces_db_open_failure_as_db_open_failed() {
    // Exercise the REAL `open_with_storage` failing at `open_local` (not a synthetic StorageError):
    // a `.unblock/` exists, but a DIRECTORY sits where the db FILE must be created, so libsql cannot
    // open it as a SQLite file. This is the deterministic, portable forced failure (a dir is never a
    // valid SQLite database on any platform).
    let workspace = fresh_workspace();
    let db_path = workspace.path().join(".unblock").join("unblock.db");
    fs::create_dir(&db_path).expect("place a directory at the db_path");

    match open_with_storage(workspace.path()).await {
        Ok(_) => panic!("open_with_storage must fail when the db_path is a directory"),
        Err(err) => {
            // The failure surfaces from `open_local` (before any migrate), so it is `DbOpenFailed`,
            // and the forwarded inner storage code is the generic `DatabaseError` (a backend-class
            // open failure) -> exit 2 (spine §2.3).
            assert!(
                matches!(err, ConfigError::DbOpenFailed { .. }),
                "expected DbOpenFailed, got: {err:?}"
            );
            assert_eq!(err.code(), ErrorCode::DatabaseError);
            assert_eq!(err.code().exit_code(), 2);
        }
    }
}

#[tokio::test]
async fn open_with_storage_reopen_is_idempotent_and_preserves_data() {
    // The data-integrity reopen guarantee: a second `open_with_storage` on an already-migrated
    // workspace must succeed (migrate is idempotent) and still see the previously-created issue.
    let workspace = fresh_workspace();

    // First open: create + migrate a fresh DB, write one issue, then drop the context (closing the
    // handle) so the second open is a genuine reopen of the on-disk DB.
    {
        let ctx = open_with_storage(workspace.path())
            .await
            .expect("first open");
        let issue = sample_issue("ub-reopen1", "Survive a reopen");
        let id = ctx
            .storage
            .create_issue(&issue, &ctx.config.runtime.actor)
            .await
            .expect("create on first open");
        assert_eq!(id, "ub-reopen1");
        // ctx (and its Arc<dyn Storage>) drops here.
    }

    // Second open on the SAME workspace: migrate must be idempotent on an already-migrated DB.
    let reopened = match open_with_storage(workspace.path()).await {
        Ok(ctx) => ctx,
        Err(err) => panic!("reopen must succeed (migrate idempotent), got: {err:?}"),
    };

    // (a) the second open succeeded; (b) the previously-created issue is still retrievable.
    let fetched = reopened
        .storage
        .get_issue("ub-reopen1")
        .await
        .expect("get after reopen")
        .expect("issue must survive the reopen");
    assert_eq!(fetched.id, "ub-reopen1");
    assert_eq!(fetched.title, "Survive a reopen");

    // The reopened DB is still healthy.
    let problems = reopened
        .storage
        .integrity_check()
        .await
        .expect("integrity after reopen");
    assert!(
        problems.is_empty(),
        "reopened db must be healthy: {problems:?}"
    );
}

#[tokio::test]
async fn open_workspace_with_cli_explicit_dir_resolves_without_db() {
    // FORK-1 overload: the explicit `--dir` points straight at the workspace (no walk-up, MF-2).
    let workspace = fresh_workspace();
    let cli = CliOverrides::new().with_dir(workspace.path());

    let ctx = open_workspace_with_cli(&cli)
        .await
        .expect("resolve via cli");

    let canon_ws = workspace.path().canonicalize().expect("canon");
    assert_eq!(ctx.paths.unblock_dir, canon_ws.join(".unblock"));
    assert!(
        !ctx.paths.db_path.exists(),
        "resolve-only must not create the db"
    );
}

#[tokio::test]
async fn open_with_storage_with_cli_threads_actor_override() {
    // FORK-4: the `--actor` override is the authoritative actor in the resulting context.
    let workspace = fresh_workspace();
    let cli = CliOverrides::new()
        .with_dir(workspace.path())
        .with_actor("alice-cli");

    let ctx = open_with_storage_with_cli(&cli)
        .await
        .expect("open via cli");
    assert_eq!(ctx.config.runtime.actor, "alice-cli");
    assert_eq!(
        ctx.config.runtime.sources.actor,
        unblock_model::ConfigSource::Cli
    );
    assert!(ctx.paths.db_path.exists(), "open_local must create the db");
}

#[tokio::test]
async fn open_with_storage_with_cli_rejects_over_long_actor() {
    let workspace = fresh_workspace();
    let cli = CliOverrides::new()
        .with_dir(workspace.path())
        .with_actor("x".repeat(201));

    match open_with_storage_with_cli(&cli).await {
        Ok(_) => panic!("must reject an over-long actor"),
        Err(err) => {
            assert!(
                matches!(err, ConfigError::InvalidValue { .. }),
                "expected InvalidValue, got {err:?}"
            );
            assert_eq!(err.code(), ErrorCode::ConfigError);
            assert_eq!(err.code().exit_code(), 7);
        }
    }
}

/// **D57 criterion (n), the resolve-only half (the facade `unblock agents` uses)**: over a workspace
/// whose `config` table holds `actor` and `output_format` rows, the STORAGE-BEARING open resolves
/// both from the DB layer, while the RESOLVE-ONLY open resolves them over FIVE layers — the DB
/// layer invisible, its rows contributing nothing and no fault recorded.
#[tokio::test]
async fn the_resolve_only_open_never_reads_the_db_layer() {
    use unblock_config::OutputFormat;
    use unblock_model::ConfigSource;

    let workspace = fresh_workspace();
    {
        let ctx = open_with_storage(workspace.path()).await.expect("open");
        ctx.storage.set_config("actor", "db-actor").await.unwrap();
        ctx.storage
            .set_config("output_format", "csv")
            .await
            .unwrap();
        ctx.storage.set_config("db_filename", "x.db").await.unwrap();
    }

    let with_db = open_with_storage(workspace.path()).await.expect("open");
    let runtime = &with_db.config.runtime;
    assert_eq!(
        (runtime.actor.as_str(), runtime.sources.actor),
        ("db-actor", ConfigSource::Db)
    );
    assert_eq!(
        (runtime.output_format, runtime.sources.output_format),
        (OutputFormat::Csv, ConfigSource::Db)
    );
    assert_eq!(runtime.db_faults.len(), 1, "the startup-key row is a fault");
    drop(with_db);

    let resolved = open_workspace(workspace.path()).await.expect("resolve");
    let runtime = &resolved.config.runtime;
    assert_eq!(
        (runtime.actor.as_str(), runtime.sources.actor),
        ("unblock", ConfigSource::Default),
        "no DB actor, and (empty env) no $USER either"
    );
    assert_eq!(
        (runtime.output_format, runtime.sources.output_format),
        (OutputFormat::Json, ConfigSource::Default)
    );
    assert!(runtime.db_faults.is_empty(), "no row is read, so no fault");
}

// ------------------------------------------------------------------------------------------------
// D58 — the lost-database refusal (clause (7)) and the open modes (clause (8)).
// ------------------------------------------------------------------------------------------------

/// The open modes `open_with_cli` serves with storage.
const STORAGE_MODES: [StorageOpenMode; 3] = [
    StorageOpenMode::Migrate,
    StorageOpenMode::ReadOnly,
    StorageOpenMode::NoMigrate,
];

/// A `.unblock/` holding `config.toml` (a non-default `id_prefix`) and the `.write.lock` marker but
/// no database — the LOST state once a database file is absent or empty.
fn workspace_with_marker() -> tempfile::TempDir {
    let workspace = fresh_workspace();
    let unblock_dir = workspace.path().join(".unblock");
    fs::write(unblock_dir.join("config.toml"), "id_prefix = \"lost\"\n").expect("config.toml");
    fs::write(unblock_dir.join(".write.lock"), b"").expect(".write.lock");
    workspace
}

/// Every file under `root` with its bytes (directories by name only), for byte-identity checks.
fn tree(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("dir entry").path();
            let rel = path.strip_prefix(root).expect("under root").to_path_buf();
            if path.is_dir() {
                out.insert(rel, None);
                stack.push(path);
            } else {
                out.insert(rel, Some(fs::read(&path).expect("read file")));
            }
        }
    }
    out
}

/// The `DatabaseMissing` refusal: `DATABASE_NOT_FOUND`, exit 2, naming the configured path.
fn assert_database_missing(err: &ConfigError, db_path: &Path, open: &str) {
    match err {
        ConfigError::DatabaseMissing { path } => assert_eq!(
            path.file_name(),
            db_path.file_name(),
            "{open}: the refusal names the configured database"
        ),
        other => panic!("{open}: expected DatabaseMissing, got {other:?}"),
    }
    assert_eq!(err.code(), ErrorCode::DatabaseNotFound, "{open}");
    assert_eq!(err.code().exit_code(), 2, "{open}");
    let hint = err.hint().expect("DatabaseMissing carries a hint");
    assert!(
        hint.contains("unblock doctor") && hint.contains("unblock init"),
        "{open}: {hint}"
    );
}

/// Run every storage-bearing open over `workspace` and return each one's error (the `&Path`
/// facade, its CLI overload, and `open_with_cli` in each of the three modes).
async fn every_storage_open_err(workspace: &Path) -> Vec<(String, ConfigError)> {
    let mut out = Vec::new();
    match open_with_storage(workspace).await {
        Ok(_) => panic!("open_with_storage must refuse"),
        Err(err) => out.push(("open_with_storage".to_string(), err)),
    }
    let cli = CliOverrides::new().with_dir(workspace);
    match open_with_storage_with_cli(&cli).await {
        Ok(_) => panic!("open_with_storage_with_cli must refuse"),
        Err(err) => out.push(("open_with_storage_with_cli".to_string(), err)),
    }
    for mode in STORAGE_MODES {
        match open_with_cli(&cli.clone().with_storage_open(mode)).await {
            Ok(_) => panic!("open_with_cli({mode:?}) must refuse"),
            Err(err) => out.push((format!("open_with_cli({mode:?})"), err)),
        }
    }
    out
}

/// **D58 clause (7), criterion (h) config half** — `config.toml` and `.write.lock` present with the
/// database ABSENT, and again with a 0-BYTE database: every storage-bearing open, in every mode,
/// refuses with `DatabaseMissing` (`DATABASE_NOT_FOUND`, exit 2) and the workspace is
/// byte-identical afterwards — no database, no sidecar, the 0-byte file still 0 bytes.
///
/// MUTANT KILLED: dropping the refusal from any one open (the facades or a mode of `open_with_cli`)
/// — that open recreates an empty database over the lost one.
#[tokio::test]
async fn lost_database_refuses_database_missing_and_creates_nothing() {
    for zero_byte in [false, true] {
        let workspace = workspace_with_marker();
        let db_path = workspace.path().join(".unblock").join("unblock.db");
        if zero_byte {
            fs::write(&db_path, b"").expect("0-byte database");
        }
        let before = tree(workspace.path());

        let errors = every_storage_open_err(workspace.path()).await;
        assert_eq!(errors.len(), 5);
        for (open, err) in &errors {
            assert_database_missing(err, &db_path, &format!("{open} (zero_byte={zero_byte})"));
        }
        assert_eq!(
            tree(workspace.path()),
            before,
            "a refused open writes nothing (zero_byte={zero_byte})"
        );
    }
}

/// **D58 clause (7)** — the lost-database check names the file it failed on. With the database
/// absent and `.write.lock` a symlink to itself, the marker check fails with `ELOOP`, and the
/// `IO_ERROR` names `.write.lock`, not the database.
///
/// MUTANT KILLED: labelling every `lost_database` failure with the database path.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_marker_check_names_the_write_lock() {
    let workspace = fresh_workspace();
    let unblock_dir = workspace.path().join(".unblock");
    std::os::unix::fs::symlink(".write.lock", unblock_dir.join(".write.lock"))
        .expect("a self-referencing .write.lock");

    match open_with_storage(workspace.path()).await {
        Err(ConfigError::Io { path, .. }) => assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".write.lock"),
            "{path:?}"
        ),
        Err(other) => panic!("expected an Io error naming .write.lock, got {other:?}"),
        Ok(_) => panic!("a failed marker check must refuse the open"),
    }
}

/// **D58 clause (7)** — a FRESH clone (`config.toml` only: no database, no marker) is not lost: the
/// storage-bearing open creates and migrates the database exactly as before, and the migrate writes
/// the `.write.lock` marker that later tells a lost database from this state.
#[tokio::test]
async fn fresh_clone_without_marker_creates_the_database() {
    let workspace = fresh_workspace();
    let unblock_dir = workspace.path().join(".unblock");
    fs::write(unblock_dir.join("config.toml"), "id_prefix = \"fresh\"\n").expect("config.toml");
    assert!(!unblock_dir.join(".write.lock").exists());

    let ctx = open_with_storage(workspace.path())
        .await
        .expect("a fresh clone opens");
    assert!(ctx.paths.db_path.exists(), "the database is created");
    assert_eq!(ctx.schema_version_before_migrate, 0);
    assert_eq!(
        ctx.storage.schema_version().await.expect("stamp"),
        unblock_storage::CURRENT_SCHEMA_VERSION
    );
    assert!(
        unblock_dir.join(".write.lock").exists(),
        "the first migrating open writes the marker"
    );
}

/// **D58 clause (7)** — `create_db` (set by `unblock init` alone) creates and migrates the database
/// over the marker, absent or 0-byte, instead of refusing it; a following plain open succeeds.
#[tokio::test]
async fn create_db_creates_over_the_marker() {
    for zero_byte in [false, true] {
        let workspace = workspace_with_marker();
        let db_path = workspace.path().join(".unblock").join("unblock.db");
        if zero_byte {
            fs::write(&db_path, b"").expect("0-byte database");
        }
        let cli = CliOverrides::new()
            .with_dir(workspace.path())
            .with_create_db(true);
        let ctx = open_with_storage_with_cli(&cli)
            .await
            .expect("create_db opens over the marker");
        assert_eq!(
            ctx.storage.schema_version().await.expect("stamp"),
            unblock_storage::CURRENT_SCHEMA_VERSION
        );
        drop(ctx);
        assert!(fs::metadata(&db_path).expect("db").len() > 0);

        if let Err(err) = open_with_storage(workspace.path()).await {
            panic!("the recreated database opens without create_db: {err:?}");
        }
    }
}

/// **D58 clause (7)** — the refusal runs AFTER the config files are parsed and validated: over a
/// lost database, a malformed `config.toml` still fails `CONFIG_PARSE_ERROR` (exit 7), and an
/// invalid runtime value (`search_cap = 0`) still fails `CONFIG_ERROR` (exit 7), in every mode,
/// creating nothing.
///
/// MUTANT KILLED: moving the refusal before the file loads (or before the runtime validation) —
/// these opens then exit 2 with `DATABASE_NOT_FOUND`.
#[tokio::test]
async fn malformed_config_exits_7_before_the_lost_refusal() {
    for (body, code) in [
        ("id_prefix = [unterminated\n", ErrorCode::ConfigParseError),
        ("search_cap = 0\n", ErrorCode::ConfigError),
    ] {
        let workspace = workspace_with_marker();
        fs::write(workspace.path().join(".unblock").join("config.toml"), body)
            .expect("config.toml");
        let before = tree(workspace.path());

        for (open, err) in every_storage_open_err(workspace.path()).await {
            assert_eq!(err.code(), code, "{open} over {body:?}: {err:?}");
            assert_eq!(err.code().exit_code(), 7, "{open}");
        }
        assert_eq!(tree(workspace.path()), before, "nothing written ({body:?})");
    }
}

/// **D58 clause (8)** — `no_db` (its FIRST reader) returns `Resolved` and never touches the
/// database: over a fresh workspace no file appears; over a LOST database there is no refusal; over
/// a migrated workspace every byte — database, sidecars, `.write.lock` — is unchanged, whatever
/// `storage_open` says.
#[tokio::test]
async fn no_db_returns_resolved_and_never_touches_the_database() {
    let expect_resolved = |opened: OpenedContext, what: &str| match opened {
        OpenedContext::Resolved(ctx) => ctx,
        OpenedContext::Storage(_) => panic!("{what}: no_db must return Resolved"),
    };

    // Fresh: nothing is created.
    let fresh = fresh_workspace();
    let before = tree(fresh.path());
    let cli = CliOverrides::new().with_dir(fresh.path()).with_no_db(true);
    let ctx = expect_resolved(
        open_with_cli(&cli)
            .await
            .expect("no_db over a fresh workspace"),
        "fresh",
    );
    assert!(!ctx.paths.db_path.exists());
    assert_eq!(tree(fresh.path()), before);

    // Lost: the resolve-only open does not refuse (the `doctor` pre-open phase classifies it).
    let lost = workspace_with_marker();
    let before = tree(lost.path());
    let cli = CliOverrides::new().with_dir(lost.path()).with_no_db(true);
    let ctx = expect_resolved(
        open_with_cli(&cli)
            .await
            .expect("no_db over a lost database"),
        "lost",
    );
    assert_eq!(ctx.config.startup.id_prefix, "lost");
    assert_eq!(tree(lost.path()), before);

    // Migrated: every mode with no_db leaves every byte as it was.
    let migrated = fresh_workspace();
    drop(open_with_storage(migrated.path()).await.expect("migrate"));
    let before = tree(migrated.path());
    for mode in STORAGE_MODES {
        let cli = CliOverrides::new()
            .with_dir(migrated.path())
            .with_no_db(true)
            .with_storage_open(mode);
        expect_resolved(
            open_with_cli(&cli).await.expect("no_db over a migrated db"),
            "migrated",
        );
        assert_eq!(tree(migrated.path()), before, "{mode:?}");
    }
}

/// A raw libsql connection (the bundled `SQLite` the backend uses) that closes its handle exactly
/// once: `_close_guard`, a prepared never-stepped statement declared AFTER `conn`, drops after it
/// (libsql 0.9.30 closes a dropped connection's handle twice, ub-q1u).
struct RawConnection {
    conn: libsql::Connection,
    _close_guard: libsql::Statement,
}

/// Stamp `PRAGMA user_version = stamp` on the database at `db` out of band.
async fn restamp(db: &Path, stamp: i64) {
    let database = libsql::Builder::new_local(db)
        .build()
        .await
        .expect("raw open");
    let conn = database.connect().expect("connect");
    let close_guard = conn.prepare("SELECT 1").await.expect("close guard");
    let raw = RawConnection {
        conn,
        _close_guard: close_guard,
    };
    raw.conn
        .execute(&format!("PRAGMA user_version = {stamp}"), ())
        .await
        .expect("re-stamp");
}

/// A migrated workspace holding an `actor` row in its `config` table, re-stamped to `stamp`.
async fn migrated_workspace_stamped(stamp: i64) -> tempfile::TempDir {
    let workspace = fresh_workspace();
    {
        let ctx = open_with_storage(workspace.path()).await.expect("migrate");
        ctx.storage
            .set_config("actor", "db-actor")
            .await
            .expect("actor row");
    }
    restamp(&workspace.path().join(".unblock").join("unblock.db"), stamp).await;
    workspace
}

/// The storage-bearing context `open_with_cli` returned (panics on `Resolved` or an error).
async fn open_storage_mode(workspace: &Path, mode: StorageOpenMode) -> WorkspaceContext {
    let cli = CliOverrides::new()
        .with_dir(workspace)
        .with_storage_open(mode);
    match open_with_cli(&cli).await {
        Ok(OpenedContext::Storage(ctx)) => ctx,
        Ok(OpenedContext::Resolved(_)) => panic!("{mode:?}: expected Storage"),
        Err(err) => panic!("{mode:?}: open failed: {err:?}"),
    }
}

/// **D58 clause (8), criterion (i)** — a `ReadOnly` open of a database stamped one step BEHIND
/// leaves its bytes, its stamp and `.write.lock` untouched: no migrate (the stamp read before and
/// after is the stale one), no `.write.lock` (absent before, absent after), and the database file's
/// bytes are identical. Below stamp 3 the DB layer is empty: the `actor` row is not read.
///
/// MUTANT KILLED: migrating in `ReadOnly` mode — the stamp advances and the bytes move.
/// MUTANT KILLED: opening through `open_local` — the marker or a write appears.
#[tokio::test]
async fn read_only_open_leaves_db_bytes_stamp_and_write_lock_untouched_on_a_stale_stamp() {
    let stale = unblock_storage::CURRENT_SCHEMA_VERSION - 1;
    let workspace = migrated_workspace_stamped(stale).await;
    let unblock_dir = workspace.path().join(".unblock");
    let db_path = unblock_dir.join("unblock.db");
    // A database without the marker is not lost; with it gone, any marker write is observable.
    fs::remove_file(unblock_dir.join(".write.lock")).expect("remove the marker");
    let db_before = fs::read(&db_path).expect("db bytes");

    let ctx = open_storage_mode(workspace.path(), StorageOpenMode::ReadOnly).await;
    assert_eq!(ctx.schema_version_before_migrate, stale);
    assert_eq!(ctx.storage.schema_version().await.expect("stamp"), stale);
    assert_eq!(
        (
            ctx.config.runtime.actor.as_str(),
            ctx.config.runtime.sources.actor
        ),
        ("unblock", unblock_model::ConfigSource::Default),
        "below stamp 3 the config rows are not read"
    );
    drop(ctx);

    assert_eq!(fs::read(&db_path).expect("db bytes"), db_before);
    assert!(
        !unblock_dir.join(".write.lock").exists(),
        "a read-only open never writes .write.lock"
    );
    assert_stamp_on_disk(&db_path, stale).await;
}

/// The on-disk stamp, read through a fresh read-only open, equals `expected`.
async fn assert_stamp_on_disk(db_path: &Path, expected: i64) {
    use unblock_storage::Storage as _;
    let storage = unblock_storage::LibsqlStorage::open_local_read_only(db_path)
        .await
        .expect("read-only reopen");
    assert_eq!(storage.schema_version().await.expect("stamp"), expected);
}

/// **D58 clause (8)** — a `NoMigrate` open (read-write) of a stale database does not migrate: the
/// stamp before and after is the stale one, and the database stays at it on disk.
///
/// MUTANT KILLED: `NoMigrate` falling through to the migrating open.
#[tokio::test]
async fn no_migrate_open_leaves_the_stamp_unchanged() {
    let stale = unblock_storage::CURRENT_SCHEMA_VERSION - 1;
    let workspace = migrated_workspace_stamped(stale).await;
    let db_path = workspace.path().join(".unblock").join("unblock.db");

    let ctx = open_storage_mode(workspace.path(), StorageOpenMode::NoMigrate).await;
    assert_eq!(ctx.schema_version_before_migrate, stale);
    assert_eq!(ctx.storage.schema_version().await.expect("stamp"), stale);
    assert_eq!(
        ctx.config.runtime.sources.actor,
        unblock_model::ConfigSource::Default,
        "below stamp 3 the config rows are not read"
    );
    drop(ctx);
    assert_stamp_on_disk(&db_path, stale).await;

    // Contrast: the migrating open of the same database advances it.
    let ctx = open_storage_mode(workspace.path(), StorageOpenMode::Migrate).await;
    assert_eq!(ctx.schema_version_before_migrate, stale);
    assert_eq!(
        ctx.storage.schema_version().await.expect("stamp"),
        unblock_storage::CURRENT_SCHEMA_VERSION
    );
}

/// **D58 clause (8)** — a stamp ABOVE this build's opens in both non-migrating modes (so `doctor`
/// can classify it `schema_stamp_ahead`), reporting that stamp and reading the `config` rows (stamp
/// >= 3); the migrating open of the same database still refuses with `SCHEMA_MISMATCH`.
#[tokio::test]
async fn read_only_open_of_a_newer_stamp_opens() {
    let ahead = unblock_storage::CURRENT_SCHEMA_VERSION + 1;
    let workspace = migrated_workspace_stamped(ahead).await;

    for mode in [StorageOpenMode::ReadOnly, StorageOpenMode::NoMigrate] {
        let ctx = open_storage_mode(workspace.path(), mode).await;
        assert_eq!(ctx.schema_version_before_migrate, ahead, "{mode:?}");
        assert_eq!(
            ctx.storage.schema_version().await.expect("stamp"),
            ahead,
            "{mode:?}"
        );
        assert_eq!(
            (
                ctx.config.runtime.actor.as_str(),
                ctx.config.runtime.sources.actor
            ),
            ("db-actor", unblock_model::ConfigSource::Db),
            "{mode:?}: at stamp >= 3 the config rows are read"
        );
    }

    let cli = CliOverrides::new()
        .with_dir(workspace.path())
        .with_storage_open(StorageOpenMode::Migrate);
    match open_with_cli(&cli).await {
        Ok(_) => panic!("the migrating open refuses a newer stamp"),
        Err(err) => {
            assert!(
                matches!(err, ConfigError::MigrationFailed { .. }),
                "expected MigrationFailed, got {err:?}"
            );
            assert_eq!(err.code(), ErrorCode::SchemaMismatch);
        }
    }
}
