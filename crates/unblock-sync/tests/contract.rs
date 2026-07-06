//! Crate-level integration against REAL `unblock-storage` libsql (NFR-16): round-trip identity,
//! idempotency, reject-with-zero-writes, tombstone-non-resurrection (MF-9), and the MF-1 k-of-n
//! atomic-rollback AC.

use std::io::Write;
use std::path::Path;

use chrono::{TimeZone, Utc};
use unblock_model::{Issue, ListFilters, Status};
use unblock_sync::{
    CollisionPolicy, ExportOptions, ImportOptions, export_jsonl, import_jsonl, serialize_issue_line,
};

use unblock_storage::{LibsqlStorage, Storage};

async fn fresh_storage() -> LibsqlStorage {
    let storage = LibsqlStorage::open_in_memory().await.expect("open");
    storage.migrate().await.expect("migrate");
    storage
}

fn issue(id: &str) -> Issue {
    let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    Issue {
        id: id.to_string(),
        title: format!("issue {id}"),
        status: Status::Open,
        created_at: ts,
        updated_at: ts,
        ..Issue::default()
    }
}

fn unblock_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".unblock");
    std::fs::create_dir_all(&dir).unwrap();
    (tmp, dir)
}

fn write_lines(dir: &Path, name: &str, lines: &[String]) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    path
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
async fn export_then_import_round_trip_identity() {
    let (_tmp, dir) = unblock_dir();
    let source = fresh_storage().await;
    source.create_issue(&issue("ub-1"), "t").await.unwrap();
    source.create_issue(&issue("ub-2"), "t").await.unwrap();

    let target = dir.join("issues.jsonl");
    let report = export_jsonl(&source, &target, &dir, &ExportOptions::default())
        .await
        .expect("export");
    assert_eq!(report.written, 2);

    // Import into a fresh DB → both issues land, sync_equals identity.
    let dest = fresh_storage().await;
    let ir = import_jsonl(&dest, &target, &dir, "t", &ImportOptions::default())
        .await
        .expect("import");
    assert_eq!(ir.imported, 2);
    let a = dest.get_issue("ub-1").await.unwrap().unwrap();
    assert!(issue("ub-1").sync_equals(&a));
}

#[tokio::test]
async fn re_import_is_idempotent() {
    let (_tmp, dir) = unblock_dir();
    let storage = fresh_storage().await;
    storage.create_issue(&issue("ub-1"), "t").await.unwrap();
    let target = dir.join("issues.jsonl");
    export_jsonl(&storage, &target, &dir, &ExportOptions::default())
        .await
        .unwrap();

    // Re-import the same file into the SAME DB → imported == 0 (all skipped as identical).
    let ir = import_jsonl(&storage, &target, &dir, "t", &ImportOptions::default())
        .await
        .expect("import");
    assert_eq!(ir.imported, 0);
    assert_eq!(ir.skipped, 1);
    assert_eq!(count_rows(&storage).await, 1);
}

#[tokio::test]
async fn conflict_marker_file_rejected_zero_writes() {
    let (_tmp, dir) = unblock_dir();
    let storage = fresh_storage().await;
    let path = dir.join("issues.jsonl");
    std::fs::write(&path, "<<<<<<< HEAD\n=======\n").unwrap();
    let before = count_rows(&storage).await;
    let err = import_jsonl(&storage, &path, &dir, "t", &ImportOptions::default())
        .await
        .expect_err("markers");
    assert!(matches!(
        err,
        unblock_sync::SyncError::ConflictMarkers { .. }
    ));
    assert_eq!(count_rows(&storage).await, before, "zero writes on reject");
}

#[tokio::test]
async fn tombstone_non_resurrection_zero_writes() {
    // MF-9: a non-tombstone incoming line for a DB-tombstoned id is SKIPPED, ZERO writes.
    let (_tmp, dir) = unblock_dir();
    let storage = fresh_storage().await;
    storage.create_issue(&issue("ub-1"), "t").await.unwrap();
    // Tombstone ub-1 via a soft delete.
    storage
        .delete_issue(
            &unblock_storage::DeletePlan {
                mode: unblock_storage::DeleteMode::Tombstone,
                targets: vec!["ub-1".to_string()],
                cascade_children: Vec::new(),
            },
            "admin",
        )
        .await
        .unwrap();
    let existing = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(existing.status, Status::Tombstone);

    // Import a NON-tombstone line for ub-1 → skipped, still a tombstone.
    let line = serialize_issue_line(&issue("ub-1")).unwrap();
    let path = write_lines(&dir, "issues.jsonl", &[line]);
    let ir = import_jsonl(&storage, &path, &dir, "t", &ImportOptions::default())
        .await
        .expect("import");
    assert_eq!(ir.imported, 0, "must not resurrect");
    assert_eq!(ir.skipped, 1);
    let after = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(after.status, Status::Tombstone, "still tombstoned");
}

#[tokio::test]
async fn dry_run_over_resurrection_path_zero_writes() {
    // MF-9: dry_run over the tombstone path performs ZERO create/update.
    let (_tmp, dir) = unblock_dir();
    let storage = fresh_storage().await;
    storage.create_issue(&issue("ub-1"), "t").await.unwrap();
    storage
        .delete_issue(
            &unblock_storage::DeletePlan {
                mode: unblock_storage::DeleteMode::Tombstone,
                targets: vec!["ub-1".to_string()],
                cascade_children: Vec::new(),
            },
            "admin",
        )
        .await
        .unwrap();
    let line = serialize_issue_line(&issue("ub-1")).unwrap();
    let path = write_lines(&dir, "issues.jsonl", &[line]);
    let ir = import_jsonl(
        &storage,
        &path,
        &dir,
        "t",
        &ImportOptions {
            dry_run: true,
            ..ImportOptions::default()
        },
    )
    .await
    .expect("dry run");
    assert_eq!(ir.imported, 0);
    let after = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(after.status, Status::Tombstone);
}

#[tokio::test]
async fn tombstone_guard_wins_over_error_policy_on_real_libsql() {
    // SF-1 (NON-VACUOUS): the tombstone guard is distinguished from the production-default `Skip`
    // ONLY under a non-`Skip` policy. Under `CollisionPolicy::Error`, a DB-tombstoned id whose
    // incoming line DIFFERS (a non-tombstone resurrection attempt) would — WITHOUT the guard — fall
    // through to the collision branch and raise `ImportCollision`. The guard runs FIRST, so the
    // record is SKIPPED ("tombstone protection"): zero writes, the row stays tombstoned, and NO
    // `ImportCollision` is raised. Removing the `existing.is_tombstone() && !incoming.is_tombstone()`
    // guard flips this to `Err(ImportCollision)` — this test then FAILS (proven by mutation).
    let (_tmp, dir) = unblock_dir();
    let storage = fresh_storage().await;
    storage.create_issue(&issue("ub-1"), "t").await.unwrap();
    storage
        .delete_issue(
            &unblock_storage::DeletePlan {
                mode: unblock_storage::DeleteMode::Tombstone,
                targets: vec!["ub-1".to_string()],
                cascade_children: Vec::new(),
            },
            "admin",
        )
        .await
        .unwrap();
    let before = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(before.status, Status::Tombstone);

    // The incoming line is a DIFFERING non-tombstone record for the same id (so `sync_equals` is
    // false and, absent the guard, the `Error` policy would collide).
    let mut incoming = issue("ub-1");
    incoming.title = "resurrect me".to_string();
    let line = serialize_issue_line(&incoming).unwrap();
    let path = write_lines(&dir, "issues.jsonl", &[line]);

    let ir = import_jsonl(
        &storage,
        &path,
        &dir,
        "t",
        &ImportOptions {
            on_collision: CollisionPolicy::Error,
            ..ImportOptions::default()
        },
    )
    .await
    .expect("the tombstone guard must skip BEFORE the Error collision branch");
    assert_eq!(ir.imported, 0, "must not resurrect under Error policy");
    assert_eq!(
        ir.skipped, 1,
        "skipped via the tombstone guard, not collided"
    );

    // The DB row is UNTOUCHED — still a tombstone.
    let after = storage.get_issue("ub-1").await.unwrap().unwrap();
    assert_eq!(
        after.status,
        Status::Tombstone,
        "tombstone guard protects even under Error policy"
    );
}

#[tokio::test]
async fn k_of_n_mid_batch_failure_rolls_back_whole_batch() {
    // MF-1 AC (non-vacuous): an import whose ONE `create_issues` tx fails mid-batch leaves ZERO rows.
    //
    // A `RaceInjector` over real libsql commits a row that collides with a classified-NEW id JUST
    // before delegating the batch — the precise "an out-of-band writer races a row in between the
    // classify probe and the atomic commit" scenario. The real one-tx insert then hits the in-tx
    // `IdCollision` on that record and ROLLS BACK the whole batch → ub-1/ub-3 (which a per-record
    // loop would have committed before the colliding record) are NOT persisted.
    let (_tmp, dir) = unblock_dir();
    let inner = std::sync::Arc::new(fresh_storage().await);
    let storage = RaceInjector::new(inner.clone(), "ub-2");

    let lines = vec![
        serialize_issue_line(&issue("ub-1")).unwrap(),
        serialize_issue_line(&issue("ub-2")).unwrap(),
        serialize_issue_line(&issue("ub-3")).unwrap(),
    ];
    let path = write_lines(&dir, "issues.jsonl", &lines);

    let err = import_jsonl(&storage, &path, &dir, "t", &ImportOptions::default())
        .await
        .expect_err("mid-batch id collision must fail the atomic tx");
    assert!(
        matches!(err, unblock_sync::SyncError::Storage { .. }),
        "{err:?}"
    );

    // ONE atomic tx (never a per-record loop).
    assert_eq!(storage.create_issues_calls(), 1);
    // NON-VACUOUS: ZERO rows from the batch — the whole tx rolled back (only the injected racer row,
    // `ub-2`, exists; ub-1 and ub-3 do NOT).
    assert!(
        inner.get_issue("ub-1").await.unwrap().is_none(),
        "ub-1 rolled back"
    );
    assert!(
        inner.get_issue("ub-3").await.unwrap().is_none(),
        "ub-3 rolled back"
    );

    // Sanity: a per-record loop WOULD have committed ub-1 before the ub-2 collision — its absence is
    // the atomicity proof.
}

/// A `Storage` decorator that injects an out-of-band racing commit before the FIRST `create_issues`
/// delegation (mirrors the engine's `RaceInjector`), forcing the in-tx `IdCollision` → whole-batch
/// rollback. Every other call is a pure delegate.
struct RaceInjector {
    inner: std::sync::Arc<LibsqlStorage>,
    race_id: String,
    armed: std::sync::atomic::AtomicBool,
    create_issues_calls: std::sync::atomic::AtomicUsize,
}

impl RaceInjector {
    fn new(inner: std::sync::Arc<LibsqlStorage>, race_id: &str) -> Self {
        Self {
            inner,
            race_id: race_id.to_string(),
            armed: std::sync::atomic::AtomicBool::new(true),
            create_issues_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
    fn create_issues_calls(&self) -> usize {
        self.create_issues_calls
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Storage for RaceInjector {
    async fn create_issues(
        &self,
        issues: &[Issue],
        actor: &str,
    ) -> Result<(), unblock_storage::StorageError> {
        use std::sync::atomic::Ordering;
        self.create_issues_calls.fetch_add(1, Ordering::SeqCst);
        if self.armed.swap(false, Ordering::SeqCst) {
            // Commit a colliding row out-of-band, then delegate the batch → in-tx IdCollision.
            self.inner
                .create_issue(&issue(&self.race_id), actor)
                .await?;
        }
        self.inner.create_issues(issues, actor).await
    }
    async fn get_issue(&self, id: &str) -> Result<Option<Issue>, unblock_storage::StorageError> {
        self.inner.get_issue(id).await
    }
    async fn list_issues(
        &self,
        f: &ListFilters,
    ) -> Result<Vec<Issue>, unblock_storage::StorageError> {
        self.inner.list_issues(f).await
    }
    async fn migrate(&self) -> Result<(), unblock_storage::StorageError> {
        self.inner.migrate().await
    }
    async fn integrity_check(&self) -> Result<Vec<String>, unblock_storage::StorageError> {
        self.inner.integrity_check().await
    }
    async fn create_issue(
        &self,
        i: &Issue,
        a: &str,
    ) -> Result<String, unblock_storage::StorageError> {
        self.inner.create_issue(i, a).await
    }
    async fn get_issues(
        &self,
        ids: &[String],
    ) -> Result<Vec<Issue>, unblock_storage::StorageError> {
        self.inner.get_issues(ids).await
    }
    async fn update_issue(
        &self,
        id: &str,
        p: &unblock_storage::IssuePatch,
        a: &str,
    ) -> Result<Issue, unblock_storage::StorageError> {
        self.inner.update_issue(id, p, a).await
    }
    async fn delete_issue(
        &self,
        plan: &unblock_storage::DeletePlan,
        a: &str,
    ) -> Result<unblock_storage::DeletePlan, unblock_storage::StorageError> {
        self.inner.delete_issue(plan, a).await
    }
    async fn restore_issue(
        &self,
        id: &str,
        a: &str,
    ) -> Result<Issue, unblock_storage::StorageError> {
        self.inner.restore_issue(id, a).await
    }
    async fn claim_issue(
        &self,
        id: &str,
        s: &str,
        a: &str,
    ) -> Result<Issue, unblock_storage::StorageError> {
        self.inner.claim_issue(id, s, a).await
    }
    async fn defer_issue(
        &self,
        id: &str,
        u: chrono::DateTime<Utc>,
        a: &str,
    ) -> Result<Issue, unblock_storage::StorageError> {
        self.inner.defer_issue(id, u, a).await
    }
    async fn undefer_issue(
        &self,
        id: &str,
        a: &str,
    ) -> Result<Issue, unblock_storage::StorageError> {
        self.inner.undefer_issue(id, a).await
    }
    async fn ready_issues(
        &self,
        f: &ListFilters,
    ) -> Result<Vec<Issue>, unblock_storage::StorageError> {
        self.inner.ready_issues(f).await
    }
    async fn blocked_issues(
        &self,
        f: &ListFilters,
    ) -> Result<Vec<Issue>, unblock_storage::StorageError> {
        self.inner.blocked_issues(f).await
    }
    async fn search_issues(
        &self,
        q: &str,
        f: &ListFilters,
    ) -> Result<Vec<Issue>, unblock_storage::StorageError> {
        self.inner.search_issues(q, f).await
    }
    async fn count_issues(
        &self,
        f: &ListFilters,
        g: Option<unblock_model::CountGroupBy>,
    ) -> Result<Vec<unblock_model::CountBucket>, unblock_storage::StorageError> {
        self.inner.count_issues(f, g).await
    }
    async fn stale_issues(
        &self,
        o: chrono::DateTime<Utc>,
        f: &ListFilters,
    ) -> Result<Vec<Issue>, unblock_storage::StorageError> {
        self.inner.stale_issues(o, f).await
    }
    async fn add_dependency(
        &self,
        d: &unblock_model::Dependency,
        a: &str,
    ) -> Result<(), unblock_storage::StorageError> {
        self.inner.add_dependency(d, a).await
    }
    async fn remove_dependency(
        &self,
        i: &str,
        d: &str,
        t: &unblock_model::DependencyType,
        a: &str,
    ) -> Result<(), unblock_storage::StorageError> {
        self.inner.remove_dependency(i, d, t, a).await
    }
    async fn list_dependencies(
        &self,
        id: &str,
    ) -> Result<Vec<unblock_model::Dependency>, unblock_storage::StorageError> {
        self.inner.list_dependencies(id).await
    }
    async fn next_child_number(&self, p: &str) -> Result<u32, unblock_storage::StorageError> {
        self.inner.next_child_number(p).await
    }
    async fn dependency_tree(
        &self,
        id: &str,
    ) -> Result<unblock_model::DepTree, unblock_storage::StorageError> {
        self.inner.dependency_tree(id).await
    }
    async fn dependency_graph(
        &self,
        r: &[String],
    ) -> Result<unblock_model::DepTree, unblock_storage::StorageError> {
        self.inner.dependency_graph(r).await
    }
    async fn detect_cycles(
        &self,
        b: bool,
    ) -> Result<Vec<Vec<String>>, unblock_storage::StorageError> {
        self.inner.detect_cycles(b).await
    }
    async fn list_events(
        &self,
        id: &str,
    ) -> Result<Vec<unblock_model::Event>, unblock_storage::StorageError> {
        self.inner.list_events(id).await
    }
    async fn epic_child_rollup(
        &self,
    ) -> Result<Vec<(String, (usize, usize))>, unblock_storage::StorageError> {
        self.inner.epic_child_rollup().await
    }
    async fn closed_since(
        &self,
        s: Option<chrono::DateTime<Utc>>,
    ) -> Result<Vec<Issue>, unblock_storage::StorageError> {
        self.inner.closed_since(s).await
    }
    async fn orphan_candidates(&self) -> Result<Vec<Issue>, unblock_storage::StorageError> {
        self.inner.orphan_candidates().await
    }
}

#[tokio::test]
async fn malformed_line_rejected_zero_writes() {
    let (_tmp, dir) = unblock_dir();
    let storage = fresh_storage().await;
    let good = serialize_issue_line(&issue("ub-1")).unwrap();
    let path = write_lines(&dir, "issues.jsonl", &[good, "not json".to_string()]);
    let err = import_jsonl(&storage, &path, &dir, "t", &ImportOptions::default())
        .await
        .expect_err("malformed");
    assert!(matches!(
        err,
        unblock_sync::SyncError::ValidationFailed { .. }
    ));
    assert_eq!(count_rows(&storage).await, 0);
}
