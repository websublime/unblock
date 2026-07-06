//! Lifecycle integration tests: open / reopen / import-on-open seam / shutdown / doctor / recover.

mod common;

use common::{session, session_with};
use unblock_engine::{EngineError, Session, SessionConfig};

#[tokio::test]
async fn migrate_is_idempotent_no_op_post_open() {
    // The context's config already migrated on open (FR-9 single open path), so `migrate` is a
    // no-op: from == to (the stamped baseline), applied == false (D27/AF-2).
    let session = session().await;
    let outcome = session.migrate().await.expect("migrate");
    assert_eq!(
        outcome.from, outcome.to,
        "a facade-opened workspace is already at the baseline: from == to"
    );
    assert!(
        outcome.from >= 1,
        "migrated store reports its stamped baseline (>= 1), got {}",
        outcome.from
    );
    assert!(
        !outcome.applied,
        "no schema advance post-open (applied == false)"
    );

    // A second migrate is still idempotent (applied stays false, from == to unchanged).
    let again = session.migrate().await.expect("re-migrate");
    assert_eq!(
        (again.from, again.to, again.applied),
        (outcome.from, outcome.to, false)
    );
}

#[tokio::test]
async fn migrate_under_shutdown_flag_is_refused() {
    // A shutdown-in-progress session refuses the write permit up front (migrate is a write-path op),
    // so `migrate()` fails fast with ShutdownInProgress and never touches the DB.
    let session = session().await;
    session.shutdown().await.expect("shutdown");
    let err = session.migrate().await.expect_err("refused under shutdown");
    assert!(matches!(err, EngineError::ShutdownInProgress));
}

#[tokio::test]
async fn open_fresh_workspace_succeeds() {
    let session = session().await;
    // The context's config already migrated; a read works immediately.
    let ready = session
        .ready(&unblock_model::ListFilters::default())
        .await
        .expect("ready");
    assert!(ready.is_empty());
    assert_eq!(session.actor(), "tester");
}

#[tokio::test]
async fn import_on_open_returns_sync_seam_and_writes_nothing() {
    use unblock_storage::Storage;
    // open(import_on_open=true) must return the typed not-wired sync seam (until T2.4) and NOT have
    // applied any import (the seam fires before any DB write).
    let storage = unblock_storage::LibsqlStorage::open_in_memory()
        .await
        .expect("open");
    storage.migrate().await.expect("migrate");
    let storage: std::sync::Arc<dyn Storage> = std::sync::Arc::new(storage);

    // Seed one issue directly through storage so we can prove open() did not touch it.
    let issue = common::issue("ub-seed", unblock_model::Priority::MEDIUM, 1000);
    storage.create_issue(&issue, "tester").await.expect("seed");

    let cfg = SessionConfig {
        import_on_open: true,
        ..SessionConfig::default()
    };
    let ctx = make_ctx(storage.clone());
    // `Session` is not `Debug` (it holds `Arc<dyn Storage>`), so match rather than `expect_err`.
    match Session::open(ctx, cfg).await {
        Err(EngineError::FeatureNotWired { feature: "sync" }) => {}
        Ok(_) => panic!("import_on_open=true must return the sync seam, not succeed"),
        Err(other) => panic!("expected sync seam, got {other:?}"),
    }

    // The pre-existing issue is untouched (the failed open never mutated the DB).
    let still_there = storage.get_issue("ub-seed").await.expect("get");
    assert!(still_there.is_some());
}

#[tokio::test]
async fn shutdown_then_writes_are_refused_and_double_shutdown_is_noop() {
    let session = session().await;

    // A mutation succeeds before shutdown.
    let issue = common::issue("ub-0001", unblock_model::Priority::MEDIUM, 1000);
    session
        .create(&issue)
        .await
        .expect("create before shutdown");

    session.shutdown().await.expect("first shutdown");
    assert!(session.is_shutdown_requested());

    // After shutdown, a new write is refused (the permit acquire checks the flag first).
    let issue2 = common::issue("ub-0002", unblock_model::Priority::MEDIUM, 1001);
    let err = session.create(&issue2).await.expect_err("refused");
    assert!(matches!(err, EngineError::ShutdownInProgress));

    // Reads still work after shutdown (they never touch the permit).
    let got = session.get("ub-0001").await.expect("read after shutdown");
    assert!(got.is_some());

    // Double shutdown is a no-op Ok.
    session
        .shutdown()
        .await
        .expect("second shutdown is a no-op");
}

#[tokio::test]
async fn doctor_and_recover_return_health_seam_and_write_nothing() {
    let session = session().await;
    let issue = common::issue("ub-0001", unblock_model::Priority::MEDIUM, 1000);
    session.create(&issue).await.expect("create");
    let before = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");

    let doctor_err = session.doctor().await.expect_err("health seam");
    assert!(matches!(
        doctor_err,
        EngineError::FeatureNotWired { feature: "health" }
    ));
    let recover_err = session.recover().await.expect_err("health seam");
    assert!(matches!(
        recover_err,
        EngineError::FeatureNotWired { feature: "health" }
    ));

    // The DB is unchanged (the seam wrote nothing).
    let after = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");
    assert_eq!(before.len(), after.len());
}

#[tokio::test]
async fn open_with_jsonl_export_knob_is_accepted() {
    // The jsonl_export knob is wired (its export body is the T2.4 seam); open must still succeed.
    let session = session_with(SessionConfig {
        jsonl_export: true,
        ..SessionConfig::default()
    })
    .await;
    assert!(
        session
            .ready(&unblock_model::ListFilters::default())
            .await
            .is_ok()
    );
}

/// Build a synthetic `WorkspaceContext` over `storage` (mirrors the harness, for the explicit-open
/// test above).
fn make_ctx(
    storage: std::sync::Arc<dyn unblock_storage::Storage>,
) -> unblock_config::WorkspaceContext {
    use std::path::PathBuf;
    use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext};
    let workspace_dir = PathBuf::from("/tmp/unblock-test-ws");
    let unblock_dir = workspace_dir.join(".unblock");
    let config = ResolvedConfig::default();
    let paths = ConfigPaths {
        db_path: unblock_dir.join(&config.db_filename),
        jsonl_path: unblock_dir.join(&config.jsonl_filename),
        unblock_dir,
    };
    WorkspaceContext {
        storage,
        workspace_dir,
        actor: "tester".to_string(),
        config,
        paths,
    }
}
