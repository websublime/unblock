//! Interchange tests (FR-7/8/26) — run under the DEFAULT (sync-on) build (MF-10).
//!
//! `export_jsonl`/`import_jsonl` now DELEGATE to `unblock-sync`: they SUCCEED over a confined path
//! under a real temp `.unblock/`, and positive-assert they do NOT return `FeatureNotWired` in the
//! same build. An external `/tmp/...` path returns `EngineError::Sync(PathTraversal)`. Only
//! `import_bd` stays seamed (`FeatureNotWired{"sync"}`, T2.5).

mod common;

use common::{issue, session_with_unblock_dir};
use unblock_engine::{EngineError, ImportOptions};
use unblock_model::{ListFilters, Priority};

#[tokio::test]
async fn export_succeeds_on_confined_path_and_is_not_feature_not_wired() {
    let (session, tmp) = session_with_unblock_dir().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");

    let target = tmp.path().join(".unblock").join("issues.jsonl");
    let report = session
        .export_jsonl(&target)
        .await
        .expect("export succeeds");
    assert_eq!(report.written, 1, "one issue exported");
    assert!(target.exists(), "the export file was written");

    // A confined export must NOT hit the seam — that is what MF-10 pins in the sync-on build.
    let content = std::fs::read_to_string(&target).unwrap();
    assert!(content.contains("ub-0001"), "content: {content}");
}

#[tokio::test]
async fn export_to_external_path_is_sync_path_traversal() {
    let (session, _tmp) = session_with_unblock_dir().await;
    let err = session
        .export_jsonl(std::path::Path::new("/tmp/should-not-be-written.jsonl"))
        .await
        .expect_err("external path rejected");
    // The seam is GONE (sync-on): an external path is now a real Sync(PathTraversal), not
    // FeatureNotWired.
    assert!(
        matches!(err, EngineError::Sync { .. }),
        "expected EngineError::Sync(PathTraversal), got {err:?}"
    );
    assert!(!matches!(err, EngineError::FeatureNotWired { .. }));
    assert!(!std::path::Path::new("/tmp/should-not-be-written.jsonl").exists());
}

#[tokio::test]
async fn import_round_trip_identity_and_idempotency() {
    let (session, tmp) = session_with_unblock_dir().await;
    session
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");
    session
        .create(&issue("ub-0002", Priority::HIGH, 2000))
        .await
        .expect("create");

    // Export, then import into a FRESH session — the two issues round-trip.
    let target = tmp.path().join(".unblock").join("issues.jsonl");
    session.export_jsonl(&target).await.expect("export");
    let exported = std::fs::read_to_string(&target).unwrap();

    let (fresh, tmp2) = session_with_unblock_dir().await;
    let fresh_target = tmp2.path().join(".unblock").join("issues.jsonl");
    std::fs::write(&fresh_target, &exported).unwrap();

    let report = fresh
        .import_jsonl(&fresh_target, ImportOptions::default())
        .await
        .expect("import succeeds");
    assert_eq!(report.imported, 2, "both issues imported");
    let all = fresh.list(&ListFilters::default()).await.expect("list");
    assert_eq!(all.len(), 2);

    // Re-import the SAME file → idempotent (imported == 0 on the second run).
    let again = fresh
        .import_jsonl(&fresh_target, ImportOptions::default())
        .await
        .expect("re-import");
    assert_eq!(again.imported, 0, "re-import is idempotent");
    assert_eq!(again.skipped, 2);
}

#[tokio::test]
async fn import_dry_run_plans_only() {
    let (source, tmp) = session_with_unblock_dir().await;
    source
        .create(&issue("ub-0001", Priority::MEDIUM, 1000))
        .await
        .expect("create");
    let target = tmp.path().join(".unblock").join("issues.jsonl");
    source.export_jsonl(&target).await.expect("export");
    let exported = std::fs::read_to_string(&target).unwrap();

    let (fresh, tmp2) = session_with_unblock_dir().await;
    let fresh_target = tmp2.path().join(".unblock").join("issues.jsonl");
    std::fs::write(&fresh_target, &exported).unwrap();

    let report = fresh
        .import_jsonl(&fresh_target, ImportOptions { dry_run: true })
        .await
        .expect("dry run");
    assert_eq!(report.imported, 1, "would-import count");
    // dry_run mutates nothing.
    let all = fresh.list(&ListFilters::default()).await.expect("list");
    assert!(all.is_empty(), "dry_run must not write any rows");
}

#[tokio::test]
async fn import_bd_stays_feature_not_wired() {
    let (session, _tmp) = session_with_unblock_dir().await;
    let err = session
        .import_bd(std::path::Path::new("/tmp/nonexistent.jsonl"))
        .await
        .expect_err("bd seam");
    // Only `import_bd` keeps the seam (T2.5) — pinned in the SAME sync-on build.
    assert!(matches!(
        err,
        EngineError::FeatureNotWired { feature: "sync" }
    ));
}
