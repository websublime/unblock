//! End-to-end facade tests (T1.3a): `open_workspace` (resolve-only, no DB) and `open_with_storage`
//! (discover + `open_local` + migrate + build `Arc<dyn Storage>`), exercised on real `tempfile`
//! `.unblock/` trees.

use std::fs;

use chrono::{TimeZone, Utc};
use unblock_config::{ConfigError, open_with_storage, open_workspace};
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

    // workspace_dir is the project root that CONTAINS `.unblock/`.
    assert_eq!(
        ctx.workspace_dir.canonicalize().expect("canon ws"),
        workspace.path().canonicalize().expect("canon tmp")
    );
    // paths are derived from the discovered workspace + defaulted filenames.
    assert_eq!(ctx.paths.unblock_dir, workspace.path().join(".unblock"));
    assert_eq!(
        ctx.paths.db_path,
        workspace.path().join(".unblock").join("unblock.db")
    );
    assert_eq!(
        ctx.paths.jsonl_path,
        workspace.path().join(".unblock").join("issues.jsonl")
    );
    // resolve-only MUST NOT open or create the database file.
    assert!(
        !ctx.paths.db_path.exists(),
        "open_workspace must not create the db file"
    );
    // actor always resolves (UNBLOCK_ACTOR / $USER / "unblock").
    assert!(!ctx.actor.is_empty());
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
        .create_issue(&issue, &ctx.actor)
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
            .create_issue(&issue, &ctx.actor)
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
