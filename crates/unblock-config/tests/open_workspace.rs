//! End-to-end facade tests (T1.3a): `open_workspace` (resolve-only, no DB) and `open_with_storage`
//! (discover + `open_local` + migrate + build `Arc<dyn Storage>`), exercised on real `tempfile`
//! `.unblock/` trees.

use std::fs;

use chrono::{TimeZone, Utc};
use std::path::Path;

use unblock_config::{
    CliOverrides, ConfigError, EnvSource, ResolvedContext, WorkspaceContext,
    open_with_storage_with_env, open_workspace_with_env,
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
