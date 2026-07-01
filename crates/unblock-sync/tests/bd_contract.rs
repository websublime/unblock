//! bd one-shot import contract (FR-26/D16/D24) against REAL `unblock-storage` libsql.
//!
//! Drives `import_bd` over the synthesized byte-faithful `fixtures/bd_export.jsonl` (D24/F3 — NO
//! dev-dep on `temp/beads_rust-main`) and asserts:
//!
//! - **per-repair POST-import field values** (non-vacuity, SF-1): each assertion FAILS under a
//!   single-repair mutation — idempotency alone does NOT satisfy the gate (`content_hash` is
//!   `#[serde(skip)]`, so a repair skipped-in-both runs is still idempotent);
//! - the extended `ImportReport` counts (`imported`/`dependencies`/`comments`) + the `dropped_fields`
//!   list (unknown top-level keys, D24/F4);
//! - **idempotent rerun** (2nd import → `imported == 0`);
//! - **bd ids preserved verbatim** (the `bd-` prefix survives — no remap);
//! - FR-8 on the bd path: a conflict-marker file is rejected with ZERO writes; a `..`-traversal path
//!   is refused at preflight.

use std::io::Write;
use std::path::Path;

use unblock_model::{DependencyType, ListFilters, Status};
use unblock_storage::{LibsqlStorage, Storage};

use unblock_sync::import_bd;

async fn fresh_storage() -> LibsqlStorage {
    let storage = LibsqlStorage::open_in_memory().await.expect("open");
    storage.migrate().await.expect("migrate");
    storage
}

/// Copy the packaged fixture into a confined `.unblock/` dir and return `(tempdir, confine_root,
/// staged_path)`.
fn stage_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".unblock");
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/bd_export.jsonl"
    );
    let bytes = std::fs::read(fixture).expect("read fixture");
    let staged = dir.join("issues.jsonl");
    std::fs::write(&staged, &bytes).unwrap();
    (tmp, dir, staged)
}

async fn count_rows(storage: &LibsqlStorage) -> usize {
    storage
        .list_issues(&ListFilters {
            include_closed: true,
            include_deferred: true,
            include_tombstone: true,
            ..ListFilters::default()
        })
        .await
        .expect("list")
        .len()
}

#[tokio::test]
async fn bd_import_applies_every_repair_and_reports_counts() {
    let (_tmp, dir, staged) = stage_fixture();
    let storage = fresh_storage().await;

    let report = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("bd import");

    // ---- counts ----
    assert_eq!(report.imported, 5, "5 issues migrated");
    assert_eq!(report.skipped, 0);
    // POST-dedup deps on bd-1: parent-child + waits-for + blocks(kept-latest of 2) = 3.
    assert_eq!(report.dependencies, 3, "deps counted POST-dedup");
    assert_eq!(report.comments, 2, "two comments on bd-1");
    // dropped_fields = unknown TOP-LEVEL keys, deduped, first-seen order.
    assert_eq!(
        report.dropped_fields,
        vec!["legacy_field".to_string(), "deprecated_col".to_string()],
    );
    assert_eq!(count_rows(&storage).await, 5);

    // ---- repair 3 + 5: status `done` → Closed AND closed_at set to updated_at ----
    let bd1 = storage.get_issue("bd-1").await.unwrap().unwrap();
    assert_eq!(bd1.status, Status::Closed, "terminal-status alias → Closed");
    assert_eq!(
        bd1.closed_at,
        Some(bd1.updated_at),
        "terminal + closed_at:None → updated_at (repair runs AFTER the alias, SF-2)"
    );

    // ---- repair 1: general underscore dep-types adopt their kebab variants ----
    let dep_type_for = |target: &str| -> DependencyType {
        bd1.dependencies
            .iter()
            .find(|d| d.depends_on_id == target)
            .unwrap_or_else(|| panic!("dep to {target} present"))
            .dep_type
            .clone()
    };
    assert_eq!(
        dep_type_for("bd-2"),
        DependencyType::ParentChild,
        "parent_child → parent-child"
    );
    assert_eq!(
        dep_type_for("bd-3"),
        DependencyType::WaitsFor,
        "waits_for → waits-for (general rule, not just parent_child)"
    );
    assert_eq!(dep_type_for("bd-4"), DependencyType::Blocks);

    // ---- repair 2: dup (bd-1 → bd-4 blocks) deduped to the LATEST created_at ----
    assert_eq!(
        bd1.dependencies
            .iter()
            .filter(|d| d.depends_on_id == "bd-4")
            .count(),
        1,
        "duplicate (issue_id, depends_on_id, dep_type) collapsed to 1 (N-1)"
    );

    // ---- repair 6: non-terminal bd-2 has its stale closed_at cleared ----
    let bd2 = storage.get_issue("bd-2").await.unwrap().unwrap();
    assert_eq!(bd2.status, Status::Open);
    assert_eq!(bd2.closed_at, None, "non-terminal → closed_at cleared");

    // ---- repair 7a: blank external_ref → None ----
    let bd3 = storage.get_issue("bd-3").await.unwrap().unwrap();
    assert_eq!(bd3.external_ref, None, "blank external_ref → None");

    // ---- repair 7b (SF-3): whitespace-padded external_ref → trimmed ----
    let bd4 = storage.get_issue("bd-4").await.unwrap().unwrap();
    assert_eq!(
        bd4.external_ref.as_deref(),
        Some("GH-42"),
        "padded external_ref → trimmed (the trim sub-branch is non-vacuous)"
    );

    // ---- repair 4: `-wisp-` id → ephemeral ----
    let wisp = storage.get_issue("bd-wisp-x1").await.unwrap().unwrap();
    assert!(wisp.ephemeral, "wisp id → ephemeral = true");

    // ---- bd ids preserved verbatim (no remap) ----
    for id in ["bd-1", "bd-2", "bd-3", "bd-4", "bd-wisp-x1"] {
        assert!(
            storage.get_issue(id).await.unwrap().is_some(),
            "id {id} preserved verbatim"
        );
    }
}

#[tokio::test]
async fn bd_import_rerun_is_idempotent() {
    let (_tmp, dir, staged) = stage_fixture();
    let storage = fresh_storage().await;

    let first = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("first import");
    assert_eq!(first.imported, 5);

    // 2nd import into the SAME DB → every record already present → imported == 0 (all skipped).
    let second = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("re-import");
    assert_eq!(second.imported, 0, "rerun is idempotent");
    assert_eq!(second.skipped, 5, "all 5 records skipped on rerun");
    assert_eq!(count_rows(&storage).await, 5, "no duplicate rows");
}

#[tokio::test]
async fn bd_import_rejects_conflict_markers_zero_writes() {
    // FR-8 on the bd path: a merge-accident file is rejected at preflight, ZERO DB writes.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".unblock");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("issues.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "<<<<<<< HEAD").unwrap();
    writeln!(f, "=======").unwrap();
    writeln!(f, ">>>>>>> theirs").unwrap();
    drop(f);

    let storage = fresh_storage().await;
    let before = count_rows(&storage).await;
    let err = import_bd(&storage, &path, &dir, "importer")
        .await
        .expect_err("conflict markers");
    assert!(
        matches!(err, unblock_sync::SyncError::ConflictMarkers { .. }),
        "{err:?}"
    );
    assert_eq!(count_rows(&storage).await, before, "zero writes on reject");
}

#[tokio::test]
async fn bd_import_mapping_report_snapshot() {
    // insta (NFR-14): pin the mapping report (counts + dropped-field list) over the fixture.
    let (_tmp, dir, staged) = stage_fixture();
    let storage = fresh_storage().await;
    let report = import_bd(&storage, &staged, &dir, "importer")
        .await
        .expect("bd import");
    insta::assert_json_snapshot!(report);
}

#[tokio::test]
async fn bd_import_refuses_parent_traversal_path() {
    // FR-8 path confinement: a `..`-escaping path is refused at preflight (platform-independent —
    // rejected lexically, no /tmp-symlink dependence).
    let (_tmp, dir, _staged) = stage_fixture();
    let storage = fresh_storage().await;
    let escaping = Path::new("../../etc/issues.jsonl");
    let err = import_bd(&storage, escaping, &dir, "importer")
        .await
        .expect_err("parent traversal refused");
    assert!(
        matches!(err, unblock_sync::SyncError::PathTraversal { .. }),
        "{err:?}"
    );
}
