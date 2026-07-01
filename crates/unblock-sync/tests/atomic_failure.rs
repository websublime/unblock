//! Atomic-write failure-injection integration (NFR-4/5, DRIFT-1).
//!
//! `write_atomic` is crate-private, so these tests drive it through the public `export_jsonl` path
//! and assert the observable durability guarantees: a rejected export leaves any existing target
//! byte-identical and removes every orphan temp; a successful export leaves NO `.tmp` sibling.

use std::path::Path;

use unblock_sync::{ExportOptions, export_jsonl};

mod fake;
use fake::{FakeStorage, sample_issue};

/// Count the `*.tmp` files in a directory (orphan-temp detector).
fn tmp_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .count()
}

#[tokio::test]
async fn rejected_export_leaves_original_intact_and_no_orphan_temp() {
    let dir = tempfile::tempdir().unwrap();
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).unwrap();
    let target = unblock.join("issues.jsonl");
    std::fs::write(&target, "ORIGINAL\n").unwrap();

    // An external (out-of-confine) target is rejected at preflight — BEFORE any temp is created.
    let external = dir.path().join("outside.jsonl");
    let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
    let err = export_jsonl(&storage, &external, &unblock, &ExportOptions::default())
        .await
        .expect_err("external rejected");
    assert!(
        !external.exists(),
        "no file written to the rejected path: {err:?}"
    );

    // The pre-existing confined target is byte-identical, and no orphan temp was left behind.
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ORIGINAL\n");
    assert_eq!(tmp_count(&unblock), 0, "no orphan temp file");
}

#[tokio::test]
async fn successful_export_leaves_no_temp_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).unwrap();
    let target = unblock.join("issues.jsonl");

    let storage = FakeStorage::with_issues(vec![sample_issue("ub-1"), sample_issue("ub-2")]);
    let report = export_jsonl(&storage, &target, &unblock, &ExportOptions::default())
        .await
        .expect("export");
    assert_eq!(report.written, 2);
    assert!(target.exists());
    // The temp was renamed over the target — no `.tmp` sibling remains.
    assert_eq!(
        tmp_count(&unblock),
        0,
        "temp persisted as target, no orphan"
    );
    let content = std::fs::read_to_string(&target).unwrap();
    assert_eq!(content.lines().count(), 2);
}

#[tokio::test]
async fn overwrite_preserves_content_and_no_partial() {
    let dir = tempfile::tempdir().unwrap();
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).unwrap();
    let target = unblock.join("issues.jsonl");
    std::fs::write(&target, "STALE LINE\n").unwrap();

    let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
    export_jsonl(&storage, &target, &unblock, &ExportOptions::default())
        .await
        .expect("export");
    let content = std::fs::read_to_string(&target).unwrap();
    // Atomic replace: the stale content is gone, exactly one fresh line, no partial concatenation.
    assert!(!content.contains("STALE LINE"), "content: {content}");
    assert_eq!(content.lines().count(), 1);
    assert_eq!(tmp_count(&unblock), 0);
}
