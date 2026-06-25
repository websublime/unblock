//! End-to-end facade tests (T1.3a): `open_workspace` (resolve-only, no DB) and `open_with_storage`
//! (discover + `open_local` + migrate + build `Arc<dyn Storage>`), exercised on real `tempfile`
//! `.unblock/` trees.

use std::fs;

use chrono::{TimeZone, Utc};
use unblock_config::{open_with_storage, open_workspace};
use unblock_model::Issue;

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
    let issue = Issue {
        id: "ub-abc123".to_string(),
        title: "Resolve the workspace".to_string(),
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        ..Issue::default()
    };
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
