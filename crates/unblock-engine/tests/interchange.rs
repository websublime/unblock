//! Interchange seam tests: `export_jsonl`/`import_jsonl`/`import_bd` are typed not-wired (the sync
//! seam, T2.4) — each returns `FeatureNotWired{"sync"}` and writes nothing (never a faked success).

mod common;

use common::{issue, session};
use unblock_engine::{EngineError, ImportOptions};
use unblock_model::Priority;

#[tokio::test]
async fn export_jsonl_returns_sync_seam_and_writes_nothing() {
    let session = session().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");
    let before = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");

    let err = session
        .export_jsonl(std::path::Path::new("/tmp/should-not-be-written.jsonl"))
        .await
        .expect_err("sync seam");
    assert!(matches!(
        err,
        EngineError::FeatureNotWired { feature: "sync" }
    ));

    // The DB is unchanged and no file was written (export is a read-snapshot + atomic write — the
    // seam fires before any of it).
    let after = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");
    assert_eq!(before.len(), after.len());
    assert!(!std::path::Path::new("/tmp/should-not-be-written.jsonl").exists());
}

#[tokio::test]
async fn import_jsonl_returns_sync_seam_and_writes_nothing() {
    let session = session().await;
    let before = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");

    let err = session
        .import_jsonl(
            std::path::Path::new("/tmp/nonexistent.jsonl"),
            ImportOptions::default(),
        )
        .await
        .expect_err("sync seam");
    assert!(matches!(
        err,
        EngineError::FeatureNotWired { feature: "sync" }
    ));

    let after = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");
    assert_eq!(before.len(), after.len());
}

#[tokio::test]
async fn import_jsonl_dry_run_also_returns_sync_seam() {
    let session = session().await;
    let err = session
        .import_jsonl(
            std::path::Path::new("/tmp/nonexistent.jsonl"),
            ImportOptions { dry_run: true },
        )
        .await
        .expect_err("sync seam");
    assert!(matches!(
        err,
        EngineError::FeatureNotWired { feature: "sync" }
    ));
}

#[tokio::test]
async fn import_bd_returns_sync_seam_and_writes_nothing() {
    let session = session().await;
    let before = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");

    let err = session
        .import_bd(std::path::Path::new("/tmp/nonexistent.jsonl"))
        .await
        .expect_err("sync seam");
    assert!(matches!(
        err,
        EngineError::FeatureNotWired { feature: "sync" }
    ));

    let after = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");
    assert_eq!(before.len(), after.len());
}
