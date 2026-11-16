//! D58 clause (8) / criterion (l): `LibsqlStorage::open_local_read_only` — the open plain `doctor`
//! uses. It runs no migrate, takes no `.write.lock`, writes no byte of the database file, refuses
//! every mutation with `StorageError::ReadOnlyStore`, and turns a hot rollback journal met on its
//! first read into `StorageError::ReadOnlyRollback`.
//!
//! Gated on `feature = "testkit"`: the fixtures re-stamp the database through
//! `StorageTestkit::testkit_execute_batch`.

#![cfg(feature = "testkit")]

use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use unblock_model::{Dependency, DependencyType, Issue};
use unblock_storage::{
    CURRENT_SCHEMA_VERSION, DEFAULT_WRITE_LOCK_TIMEOUT_MS, DeleteMode, DeletePlan, IssuePatch,
    LibsqlStorage, RepairRequest, Storage, StorageError, StorageTestkit,
};

fn issue(id: &str) -> Issue {
    let at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    Issue {
        id: id.to_string(),
        title: format!("issue {id}"),
        created_at: at,
        updated_at: at,
        ..Issue::default()
    }
}

/// A closed file database stamped `stamp`, holding `ub-1` and `ub-2`, with its `.write.lock`
/// removed — a workspace whose marker is absent, so any lock taken would show.
async fn fixture(stamp: i64) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("unblock.db");
    {
        let storage = LibsqlStorage::open_local(&path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open");
        storage.migrate().await.expect("migrate");
        storage
            .create_issue(&issue("ub-1"), "seed")
            .await
            .expect("ub-1");
        storage
            .create_issue(&issue("ub-2"), "seed")
            .await
            .expect("ub-2");
        storage
            .testkit_execute_batch(&format!("PRAGMA user_version = {stamp};"))
            .await
            .expect("stamp");
    }
    let _ = std::fs::remove_file(lock_path(&path));
    assert!(!lock_path(&path).exists());
    (dir, path)
}

fn lock_path(db: &Path) -> PathBuf {
    db.with_file_name(".write.lock")
}

/// A read-only open of a database stamped one step behind REPORTS that stamp, reads its rows and
/// health facts, and afterwards the stamp is unchanged, `.write.lock` is still absent, and the
/// database file's bytes are identical.
#[tokio::test]
async fn read_only_open_runs_no_migrate_and_takes_no_write_lock() {
    let behind = CURRENT_SCHEMA_VERSION - 1;
    let (dir, path) = fixture(behind).await;
    let before = std::fs::read(&path).expect("read the database");

    {
        let storage = LibsqlStorage::open_local_read_only(&path)
            .await
            .expect("read-only open");
        assert_eq!(storage.schema_version().await.unwrap(), behind);
        assert!(storage.get_issue("ub-1").await.unwrap().is_some());
        let facts = storage.health_facts().await.expect("health facts");
        assert_eq!(facts.schema_version, behind);
        assert!(facts.integrity_rows.is_empty(), "{facts:?}");
        assert_eq!(
            storage.testkit_recovery_root().await,
            Some(dir.path().join(".recovery"))
        );
    }

    assert!(
        !lock_path(&path).exists(),
        "a read-only open never creates the .write.lock marker"
    );
    assert_eq!(
        std::fs::read(&path).expect("re-read the database"),
        before,
        "a read-only open writes no byte of the database"
    );
    let reopened = LibsqlStorage::open_local_read_only(&path).await.unwrap();
    assert_eq!(
        reopened.schema_version().await.unwrap(),
        behind,
        "the stamp is unchanged: nothing migrated"
    );
}

/// Every mutating method, `migrate`, `acquire_write_lock` and `repair` return `ReadOnlyStore`, and
/// nothing changed afterwards.
#[tokio::test]
#[allow(clippy::too_many_lines)] // one row per mutating method: the list IS the assertion.
async fn read_only_store_refuses_every_mutation_with_read_only_store() {
    let (dir, path) = fixture(CURRENT_SCHEMA_VERSION).await;
    let before = std::fs::read(&path).expect("read the database");
    let storage = LibsqlStorage::open_local_read_only(&path)
        .await
        .expect("read-only open");
    let at = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    let dep = Dependency {
        issue_id: "ub-2".to_string(),
        depends_on_id: "ub-1".to_string(),
        dep_type: DependencyType::Blocks,
        created_at: at,
        created_by: None,
        metadata: None,
        thread_id: None,
    };
    let tombstone = DeletePlan {
        mode: DeleteMode::Tombstone,
        targets: vec!["ub-1".to_string()],
        cascade_children: Vec::new(),
    };
    let repair = RepairRequest {
        snapshot: dir.path().join(".recovery").join("run").join("snapshot.db"),
        migrate: true,
        actions: Vec::new(),
    };

    let outcomes: Vec<(&str, Result<(), StorageError>)> = vec![
        ("migrate", storage.migrate().await),
        (
            "acquire_write_lock",
            storage.acquire_write_lock().await.map(drop),
        ),
        (
            "create_issue",
            storage.create_issue(&issue("ub-3"), "a").await.map(drop),
        ),
        (
            "create_issues",
            storage.create_issues(&[issue("ub-4")], "a").await,
        ),
        (
            "update_issue",
            storage
                .update_issue("ub-1", &IssuePatch::default(), "a")
                .await
                .map(drop),
        ),
        (
            "delete_issue",
            storage.delete_issue(&tombstone, "a").await.map(drop),
        ),
        (
            "restore_issue",
            storage.restore_issue("ub-1", "a").await.map(drop),
        ),
        (
            "claim_issue",
            storage.claim_issue("ub-1", "bob", "a").await.map(drop),
        ),
        (
            "defer_issue",
            storage.defer_issue("ub-1", at, "a").await.map(drop),
        ),
        (
            "undefer_issue",
            storage.undefer_issue("ub-1", "a").await.map(drop),
        ),
        ("add_dependency", storage.add_dependency(&dep, "a").await),
        (
            "remove_dependency",
            storage
                .remove_dependency("ub-2", "ub-1", &DependencyType::Blocks, "a")
                .await,
        ),
        (
            "add_comment",
            storage
                .add_comment("ub-1", "bob", "hi", "a")
                .await
                .map(drop),
        ),
        (
            "update_comment",
            storage.update_comment(1, "edit", "a").await.map(drop),
        ),
        (
            "delete_comment",
            storage.delete_comment(1, "a").await.map(drop),
        ),
        (
            "rename_label",
            storage.rename_label("old", "new", "a").await.map(drop),
        ),
        (
            "close_issues",
            storage
                .close_issues(&["ub-1".to_string()], "done", "a")
                .await
                .map(drop),
        ),
        ("set_config", storage.set_config("actor", "bob").await),
        (
            "unset_config",
            storage.unset_config("actor").await.map(drop),
        ),
        ("repair", storage.repair(&repair).await.map(drop)),
    ];
    for (method, outcome) in outcomes {
        assert!(
            matches!(outcome, Err(StorageError::ReadOnlyStore)),
            "{method} must refuse with ReadOnlyStore, got {outcome:?}"
        );
    }

    // Reads still serve, and nothing moved.
    assert!(storage.get_issue("ub-1").await.unwrap().is_some());
    assert!(storage.get_issue("ub-3").await.unwrap().is_none());
    drop(storage);
    assert!(!lock_path(&path).exists());
    assert!(!dir.path().join(".recovery").exists());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// A HOT rollback journal beside the database (a `-journal` whose first byte is non-zero, with no
/// writer holding the RESERVED lock) makes the read-only open's first read fail with the extended
/// `SQLITE_READONLY_ROLLBACK`, surfaced as `ReadOnlyRollback` — never laundered into `Backend` —
/// and the open leaves the journal and the database bytes untouched.
#[tokio::test]
async fn read_only_open_meeting_a_hot_journal_is_read_only_rollback() {
    let (_dir, path) = fixture(CURRENT_SCHEMA_VERSION).await;
    let before = std::fs::read(&path).expect("read the database");
    let journal = path.with_file_name("unblock.db-journal");
    // The rollback-journal magic header, then zeros: SQLite's hot-journal test reads only the
    // first byte, and a read-only connection refuses before parsing the rest.
    let mut bytes = vec![0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];
    bytes.resize(512, 0);
    std::fs::write(&journal, &bytes).expect("plant the hot journal");

    let result = LibsqlStorage::open_local_read_only(&path).await;
    assert!(
        matches!(result, Err(StorageError::ReadOnlyRollback)),
        "expected ReadOnlyRollback, got {:?}",
        result.err()
    );
    assert_eq!(
        std::fs::read(&journal).unwrap(),
        bytes,
        "the journal is not rolled back"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
