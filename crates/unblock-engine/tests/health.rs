//! FR-16 full through the engine (PRD §4 D58 criteria (c), (d), (i), (j), (k), (n) and the engine
//! half of (g)).
//!
//! Every cell runs on a file-backed workspace in a tempdir. A fixture is seeded through
//! `StorageTestkit::testkit_execute_batch`, every store is dropped before `gather_preopen`, and
//! `doctor` runs on a read-only open while `recover` runs on a non-migrating read-write open, the
//! way `unblock doctor` and `unblock doctor --repair` open them.
#![cfg(all(feature = "testkit", feature = "health"))]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use unblock_config::ConfigPaths;
use unblock_engine::preopen::{
    ActionResult, capture_sidecars, classify_preopen, gather_preopen, outcome_findings,
    repair_preopen,
};
use unblock_engine::{
    DiagnosticReport, EngineError, HealthLevel, HealthReport, NewIssue, PlannedAction, RepairPath,
    Session, SessionConfig,
};
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage, StorageTestkit};

const DAY: Duration = Duration::from_hours(24);

/// A migrated file-backed workspace whose stores are all closed between steps.
struct Workspace {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    paths: ConfigPaths,
}

impl Workspace {
    /// A workspace with a migrated database and its `.write.lock` marker.
    async fn new() -> Self {
        Self::with_names("unblock.db", "issues.jsonl").await
    }

    /// [`Workspace::new`] with explicit database and export file names inside `.unblock/` — a
    /// database named like the export (PRD §4 D5 amendment, `ub-c15`).
    async fn with_names(db: &str, jsonl: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let unblock_dir = root.join(".unblock");
        std::fs::create_dir_all(&unblock_dir).expect("create .unblock");
        let paths = ConfigPaths {
            db_path: unblock_dir.join(db),
            jsonl_path: unblock_dir.join(jsonl),
            unblock_dir,
        };
        let storage = LibsqlStorage::open_local(&paths.db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local");
        storage.migrate().await.expect("migrate");
        drop(storage);
        Self {
            _tmp: tmp,
            root,
            paths,
        }
    }

    fn db(&self) -> &Path {
        &self.paths.db_path
    }

    fn recovery_root(&self) -> PathBuf {
        self.paths.unblock_dir.join(".recovery")
    }

    /// The run directories under `.unblock/.recovery/`, sorted (none when the root is absent).
    fn run_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.recovery_root()) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries.map(|e| e.expect("entry").path()).collect();
        dirs.sort();
        dirs
    }

    /// A session on a read-write open that does not migrate (the `--repair` online posture).
    async fn open(&self) -> (Session, Arc<LibsqlStorage>) {
        let storage = LibsqlStorage::open_local(self.db(), DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local");
        self.session_over(Arc::new(storage)).await
    }

    /// A session on a read-only open (the plain `doctor` posture).
    async fn open_read_only(&self) -> (Session, Arc<LibsqlStorage>) {
        let storage = LibsqlStorage::open_local_read_only(self.db())
            .await
            .expect("open_local_read_only");
        self.session_over(Arc::new(storage)).await
    }

    async fn session_over(&self, storage: Arc<LibsqlStorage>) -> (Session, Arc<LibsqlStorage>) {
        let shared: Arc<dyn Storage> = storage.clone();
        (self.session_over_dyn(shared).await, storage)
    }

    /// A session over any store (a decorator over the real one), with this workspace's paths.
    async fn session_over_dyn(&self, storage: Arc<dyn Storage>) -> Session {
        common::session_over_paths(
            storage,
            SessionConfig::default(),
            self.root.clone(),
            self.paths.clone(),
        )
        .await
    }

    /// Run raw SQL on the database, then close it.
    async fn seed(&self, sql: &str) {
        let (_session, storage) = self.open().await;
        storage.testkit_execute_batch(sql).await.expect("seed sql");
    }

    /// Plain `doctor`: gather the facts with no store open, then classify on a read-only open.
    async fn doctor(&self) -> HealthReport {
        let facts = gather_preopen(&self.paths).expect("gather_preopen");
        let (session, _storage) = self.open_read_only().await;
        session.doctor(&facts).await.expect("doctor")
    }

    /// `recover()` on a fresh non-migrating open.
    async fn recover(&self) -> Result<DiagnosticReport, EngineError> {
        let (session, _storage) = self.open().await;
        session.recover().await
    }

    /// A parent with `children` direct children (`<parent>.1` .. `<parent>.<children>`).
    async fn parent_with_children(&self, children: usize) -> String {
        let (session, _storage) = self.open().await;
        let parent = session
            .create_issue(new_issue("parent", None))
            .await
            .expect("create parent")
            .id;
        for n in 0..children {
            session
                .create_issue(new_issue(&format!("child {n}"), Some(&parent)))
                .await
                .expect("create child");
        }
        parent
    }

    async fn child_high_water(&self, parent: &str) -> Option<u32> {
        let (_session, storage) = self.open().await;
        storage
            .testkit_child_high_water(parent)
            .await
            .expect("child high water")
    }
}

fn new_issue(title: &str, parent: Option<&str>) -> NewIssue {
    NewIssue {
        title: title.to_string(),
        parent: parent.map(str::to_string),
        ..NewIssue::default()
    }
}

fn details<'a>(report: &'a DiagnosticReport, label: &str) -> Vec<&'a str> {
    report
        .findings
        .iter()
        .filter(|f| f.label == label)
        .map(|f| f.detail.as_str())
        .collect()
}

fn manifest(run_dir: &Path) -> serde_json::Value {
    let bytes = std::fs::read(run_dir.join("manifest.json")).expect("read manifest.json");
    serde_json::from_slice(&bytes).expect("manifest is JSON")
}

/// `(action, target, outcome, reason)` of every manifest action.
fn manifest_actions(run_dir: &Path) -> Vec<(String, Option<String>, String, Option<String>)> {
    let text = |v: &serde_json::Value| v.as_str().map(str::to_string);
    manifest(run_dir)["actions"]
        .as_array()
        .expect("actions array")
        .iter()
        .map(|a| {
            (
                text(&a["action"]).expect("action"),
                text(&a["target"]),
                text(&a["outcome"]).expect("outcome"),
                text(&a["reason"]),
            )
        })
        .collect()
}

/// Every `(path, sha256)` the manifest lists, and a check that each matches its run-dir copy.
fn assert_manifest_hashes(run_dir: &Path) -> Vec<String> {
    let files = manifest(run_dir)["files"]
        .as_array()
        .expect("files array")
        .clone();
    files
        .iter()
        .map(|file| {
            let rel = file["path"].as_str().expect("path").to_string();
            let (bytes, sha) =
                unblock_health::recovery::sha256_file(&run_dir.join(&rel)).expect("hash the copy");
            assert_eq!(file["sha256"].as_str(), Some(sha.as_str()), "{rel}: sha256");
            assert_eq!(file["bytes"].as_u64(), Some(bytes), "{rel}: bytes");
            rel
        })
        .collect()
}

async fn query_strings(db: &Path, sql: &str) -> Vec<String> {
    let conn = common::raw_connection(db).await;
    let mut rows = conn.query(sql, ()).await.expect("query");
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        out.push(row.get::<String>(0).expect("text column"));
    }
    out
}

async fn query_i64(db: &Path, sql: &str) -> i64 {
    let conn = common::raw_connection(db).await;
    let mut rows = conn.query(sql, ()).await.expect("query");
    let row = rows.next().await.expect("row").expect("one row");
    row.get::<i64>(0).expect("integer column")
}

// ------------------------------------------------------------------------------------------------
// (c) DRIFT
// ------------------------------------------------------------------------------------------------

/// (c) A counter lowered below its greatest child is `Drifted`; `recover()` raises it to that child
/// number, and the next create under the parent mints the following number.
#[tokio::test]
async fn drift_child_counter_reports_drifted_and_recover_raises_it_then_next_create_mints_the_following_number()
 {
    let ws = Workspace::new().await;
    let parent = ws.parent_with_children(3).await;
    ws.seed(&format!(
        "UPDATE child_counters SET last_child = 1 WHERE parent_id = '{parent}'"
    ))
    .await;

    let report = ws.doctor().await;
    assert_eq!(report.level, HealthLevel::Drifted, "{:?}", report.report);
    assert_eq!(details(&report.report, "health"), ["drifted"]);
    assert_eq!(details(&report.report, "child_counter_drift").len(), 1);
    assert!(
        details(&report.report, "repair_planned")
            .contains(&format!("raise_child_counter {parent} (online)").as_str()),
        "{:?}",
        report.report
    );
    assert_eq!(
        ws.child_high_water(&parent).await,
        Some(1),
        "plain doctor wrote nothing"
    );

    let recovered = ws.recover().await.expect("recover");
    assert_eq!(
        details(&recovered, "repair"),
        [format!("raise_child_counter {parent}: applied").as_str()]
    );
    assert_eq!(ws.child_high_water(&parent).await, Some(3));

    let (session, _storage) = ws.open().await;
    let next = session
        .create_issue(new_issue("after repair", Some(&parent)))
        .await
        .expect("create after repair");
    assert_eq!(next.id, format!("{parent}.4"));
}

/// (c) A counter ABOVE the greatest child is not drift, and nothing lowers it.
#[tokio::test]
async fn counter_above_greatest_child_is_not_drift_and_never_lowered() {
    let ws = Workspace::new().await;
    let parent = ws.parent_with_children(3).await;
    ws.seed(&format!(
        "UPDATE child_counters SET last_child = 10 WHERE parent_id = '{parent}'"
    ))
    .await;

    let report = ws.doctor().await;
    assert_eq!(report.level, HealthLevel::Healthy, "{:?}", report.report);
    assert!(details(&report.report, "child_counter_drift").is_empty());

    let recovered = ws.recover().await.expect("recover");
    assert!(details(&recovered, "repair").is_empty(), "{recovered:?}");
    assert_eq!(ws.child_high_water(&parent).await, Some(10));
    assert!(ws.run_dirs().is_empty());
}

/// (c) Two `metadata` rows for one key are `Drifted`; `recover()` keeps the later one and the
/// snapshot still holds both.
#[tokio::test]
async fn duplicate_metadata_key_recover_keeps_the_latest_and_snapshot_holds_both() {
    let ws = Workspace::new().await;
    ws.seed(
        "INSERT INTO metadata (key, value) VALUES ('k', 'v1'); \
         INSERT INTO metadata (key, value) VALUES ('k', 'v2');",
    )
    .await;

    let report = ws.doctor().await;
    assert_eq!(report.level, HealthLevel::Drifted, "{:?}", report.report);
    assert_eq!(details(&report.report, "duplicate_metadata_key").len(), 1);
    assert!(details(&report.report, "repair_planned").contains(&"dedupe_metadata_key k (online)"));

    let recovered = ws.recover().await.expect("recover");
    assert_eq!(
        details(&recovered, "repair"),
        ["dedupe_metadata_key k: applied"]
    );

    let sql = "SELECT value FROM metadata WHERE key = 'k' ORDER BY rowid";
    assert_eq!(query_strings(ws.db(), sql).await, ["v2"]);
    let runs = ws.run_dirs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        query_strings(&runs[0].join("snapshot.db"), sql).await,
        ["v1", "v2"]
    );
}

/// (c) A stamp one step behind is `schema_stamp_behind`/`Drifted` and plain `doctor` leaves it;
/// `recover()` migrates AFTER the snapshot, which keeps the old stamp.
#[tokio::test]
async fn schema_stamp_behind_recover_migrates_after_the_snapshot_which_keeps_the_old_stamp() {
    let current = unblock_storage::CURRENT_SCHEMA_VERSION;
    let behind = current - 1;
    let ws = Workspace::new().await;
    ws.seed(&format!("PRAGMA user_version = {behind}")).await;

    let report = ws.doctor().await;
    assert_eq!(report.level, HealthLevel::Drifted, "{:?}", report.report);
    assert_eq!(details(&report.report, "schema_stamp_behind").len(), 1);
    assert!(details(&report.report, "repair_planned").contains(&"migrate (online)"));
    assert_eq!(
        details(&report.report, "schema_version"),
        [behind.to_string().as_str()]
    );
    assert_eq!(
        query_i64(ws.db(), "PRAGMA user_version").await,
        behind,
        "plain doctor never migrates"
    );

    let recovered = ws.recover().await.expect("recover");
    assert_eq!(details(&recovered, "repair"), ["migrate: applied"]);
    assert_eq!(query_i64(ws.db(), "PRAGMA user_version").await, current);
    let runs = ws.run_dirs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        query_i64(&runs[0].join("snapshot.db"), "PRAGMA user_version").await,
        behind,
        "the snapshot is taken before the migration"
    );
    assert_eq!(ws.doctor().await.level, HealthLevel::Healthy);
}

// ------------------------------------------------------------------------------------------------
// (d) ONE SCAN
// ------------------------------------------------------------------------------------------------

/// (d) One `doctor()` is one `health_facts` call and no `integrity_check` call.
#[tokio::test]
async fn doctor_makes_exactly_one_health_facts_call_and_no_integrity_check_call() {
    let ws = Workspace::new().await;
    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    let (session, storage) = ws.open_read_only().await;
    session.doctor(&facts).await.expect("doctor");
    assert_eq!(storage.testkit_health_facts_calls().await, 1);
    assert_eq!(storage.testkit_integrity_check_calls().await, 0);
}

/// (d) One `recover()` with work to do is one `health_facts` call.
#[tokio::test]
async fn recover_makes_exactly_one_health_facts_call() {
    let ws = Workspace::new().await;
    ws.seed("INSERT INTO metadata (key, value) VALUES ('k', 'a'), ('k', 'b');")
        .await;
    let (session, storage) = ws.open().await;
    session.recover().await.expect("recover");
    assert_eq!(storage.testkit_health_facts_calls().await, 1);
    assert_eq!(storage.testkit_integrity_check_calls().await, 0);
}

// ------------------------------------------------------------------------------------------------
// (i) EVIDENCE
// ------------------------------------------------------------------------------------------------

/// (i) A `recover()` phase creates exactly ONE run directory. Its manifest lists every file with
/// the SHA-256 of its run-directory copy and every action with its outcome, and `snapshot.db`
/// passes `integrity_check`.
#[tokio::test]
async fn recover_creates_exactly_one_run_dir_with_manifest_hashes_and_a_passing_snapshot() {
    let ws = Workspace::new().await;
    let parent = ws.parent_with_children(2).await;
    ws.seed(&format!(
        "UPDATE child_counters SET last_child = 0 WHERE parent_id = '{parent}'; \
         INSERT INTO metadata (key, value) VALUES ('k', 'a'), ('k', 'b');"
    ))
    .await;

    let recovered = ws.recover().await.expect("recover");
    let runs = ws.run_dirs();
    assert_eq!(runs.len(), 1, "exactly one run directory: {runs:?}");
    let run = &runs[0];
    assert_eq!(
        details(&recovered, "evidence"),
        [run.display().to_string().as_str()]
    );

    let doc = manifest(run);
    assert_eq!(doc["format"], "unblock.recovery.v1");
    assert_eq!(doc["phase"], "online");
    assert_eq!(doc["level_before"], "drifted");
    let name = run
        .file_name()
        .and_then(|n| n.to_str())
        .expect("utf-8 name");
    assert!(
        name.ends_with(doc["run_id"].as_str().expect("run_id")),
        "{name}"
    );
    assert_eq!(
        manifest_actions(run),
        [
            (
                "raise_child_counter".to_string(),
                Some(parent.clone()),
                "applied".to_string(),
                None
            ),
            (
                "dedupe_metadata_key".to_string(),
                Some("k".to_string()),
                "applied".to_string(),
                None
            ),
        ]
    );
    assert_eq!(assert_manifest_hashes(run), ["snapshot.db"]);
    assert_eq!(
        query_strings(&run.join("snapshot.db"), "PRAGMA integrity_check").await,
        ["ok"]
    );
}

/// (i)/(k) `recover()` with nothing to repair returns no action row and creates no directory.
#[tokio::test]
async fn recover_on_a_healthy_workspace_creates_no_directory() {
    let ws = Workspace::new().await;
    let recovered = ws.recover().await.expect("recover");
    assert!(details(&recovered, "repair").is_empty(), "{recovered:?}");
    assert!(details(&recovered, "evidence").is_empty(), "{recovered:?}");
    assert!(!ws.recovery_root().exists());
}

/// (i) A run directory 31 days old raises `stale_recovery_artifacts` without moving the level;
/// one 29 days old raises nothing.
#[tokio::test]
async fn stale_recovery_artifacts_31_days_raises_29_days_does_not() {
    for (days, stale) in [(31_u32, true), (29, false)] {
        let ws = Workspace::new().await;
        let stamp = SystemTime::now() - DAY * days;
        let name = unblock_health::recovery::run_dir_name(stamp, 1, 0);
        std::fs::create_dir_all(ws.recovery_root().join(name)).expect("old run dir");

        let report = classify_preopen(&gather_preopen(&ws.paths).expect("gather_preopen"));
        assert_eq!(report.level, HealthLevel::Healthy, "{days} days");
        assert_eq!(
            !details(&report.report, "stale_recovery_artifacts").is_empty(),
            stale,
            "{days} days: {:?}",
            report.report
        );
        assert_eq!(ws.doctor().await.level, HealthLevel::Healthy, "{days} days");
    }
}

// ------------------------------------------------------------------------------------------------
// (j) EXPORT
// ------------------------------------------------------------------------------------------------

/// Assert that `recover()` copied `poisoned` to `export/issues.jsonl`, regenerated the export from
/// the database, and named the rows it did not import.
async fn assert_export_regenerated(ws: &Workspace, poisoned: &[u8], issue_id: &str) {
    let recovered = ws.recover().await.expect("recover");
    assert_eq!(
        details(&recovered, "repair"),
        ["regenerate_export: applied"]
    );
    let runs = ws.run_dirs();
    assert_eq!(runs.len(), 1);
    let copy = runs[0].join("export").join("issues.jsonl");
    assert_eq!(std::fs::read(&copy).expect("export copy"), poisoned);
    let note = details(&recovered, "export_not_imported");
    assert_eq!(note.len(), 1, "{recovered:?}");
    assert!(note[0].contains("NOT imported"), "{}", note[0]);
    assert!(note[0].contains(&copy.display().to_string()), "{}", note[0]);
    assert!(note[0].contains("`sync`"), "{}", note[0]);
    assert!(
        assert_manifest_hashes(&runs[0]).contains(&"export/issues.jsonl".to_string()),
        "the export copy is hashed"
    );

    let regenerated = std::fs::read_to_string(&ws.paths.jsonl_path).expect("regenerated export");
    assert!(regenerated.contains(issue_id), "{regenerated}");
    assert!(!regenerated.contains("<<<<<<<"), "{regenerated}");
    assert_eq!(ws.doctor().await.level, HealthLevel::Healthy);
}

/// (j) Conflict markers are `Recoverable`; `recover()` copies the export, regenerates it through
/// the atomic exporter and reports the rows it did not import.
#[tokio::test]
async fn conflict_markers_recover_copies_regenerates_and_reports_not_imported_rows() {
    let ws = Workspace::new().await;
    let issue = {
        let (session, _storage) = ws.open().await;
        session
            .create_issue(new_issue("kept", None))
            .await
            .expect("create")
    };
    let poisoned =
        b"<<<<<<< HEAD\n{\"id\":\"ub-ours\"}\n=======\n{\"id\":\"ub-theirs\"}\n>>>>>>> b\n";
    std::fs::write(&ws.paths.jsonl_path, poisoned).expect("write poisoned export");

    let report = ws.doctor().await;
    assert_eq!(
        report.level,
        HealthLevel::Recoverable,
        "{:?}",
        report.report
    );
    assert_eq!(details(&report.report, "jsonl_conflict_markers").len(), 1);
    assert!(details(&report.report, "repair_planned").contains(&"regenerate_export (online)"));

    assert_export_regenerated(&ws, poisoned, &issue.id).await;
}

/// (j) An export the scanner refuses (a line past its length guard) is `jsonl_unscannable` with
/// the same repair.
#[tokio::test]
async fn unscannable_export_reports_jsonl_unscannable_with_the_same_repair() {
    let ws = Workspace::new().await;
    let issue = {
        let (session, _storage) = ws.open().await;
        session
            .create_issue(new_issue("kept", None))
            .await
            .expect("create")
    };
    // One byte past the scanner's 4 MiB line guard (`unblock-sync`'s `MAX_LINE_BYTES`).
    let poisoned = vec![b'a'; 4 * 1024 * 1024 + 1];
    std::fs::write(&ws.paths.jsonl_path, &poisoned).expect("write oversized export");

    let report = ws.doctor().await;
    assert_eq!(
        report.level,
        HealthLevel::Recoverable,
        "{:?}",
        report.report
    );
    assert_eq!(details(&report.report, "jsonl_unscannable").len(), 1);
    assert!(details(&report.report, "repair_planned").contains(&"regenerate_export (online)"));

    assert_export_regenerated(&ws, &poisoned, &issue.id).await;
}

// ------------------------------------------------------------------------------------------------
// (k) RECOVER
// ------------------------------------------------------------------------------------------------

/// (k) `recover()` holds the permit AND `.write.lock` from before classification to the end of
/// the repair. The double parks `health_facts` and then `repair`; at both points the D31 lock
/// reads as held and a writer on the same session is still waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recover_holds_permit_and_write_lock_through_classification_and_repair() {
    let ws = Workspace::new().await;
    ws.seed("INSERT INTO metadata (key, value) VALUES ('k', 'a'), ('k', 'b');")
        .await;
    let storage = LibsqlStorage::open_local(ws.db(), DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    let inner: Arc<dyn Storage> = Arc::new(storage);
    let parked = common::parked::ParkedStorage::new_gated_health(inner);
    let shared: Arc<dyn Storage> = parked.clone();
    let session = Arc::new(
        common::session_over_in_dir(
            shared,
            SessionConfig::default(),
            ws.root.clone(),
            ws.paths.unblock_dir.clone(),
        )
        .await,
    );

    let recovering = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.recover().await }
    });
    parked.wait_until_parked().await;
    assert!(
        unblock_storage::probe_write_lock(ws.db()).expect("probe"),
        "classification runs under .write.lock"
    );
    let writer = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.create_issue(new_issue("concurrent", None)).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !writer.is_finished(),
        "a writer waits during classification"
    );

    parked.release();
    parked.wait_until_parked().await;
    assert!(
        unblock_storage::probe_write_lock(ws.db()).expect("probe"),
        "repair runs under .write.lock"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!writer.is_finished(), "a writer waits during the repair");

    parked.release();
    let recovered = recovering.await.expect("join").expect("recover");
    assert_eq!(
        details(&recovered, "repair"),
        ["dedupe_metadata_key k: applied"]
    );
    writer
        .await
        .expect("join")
        .expect("the writer runs once recover ends");
}

/// (k) A failure in the SECOND SQL action leaves the first unapplied, records `failed` and
/// returns the error.
///
/// The failure is injected with a trigger seeded on the database: `BEFORE DELETE ON metadata`
/// raises `ABORT`, so `dedupe_metadata_key` (the second action, after `raise_child_counter` in
/// class order) fails inside the one action transaction. No storage double is needed, and the
/// real transaction is what rolls the first action back.
#[tokio::test]
async fn recover_failure_in_second_action_leaves_first_unapplied_records_failed_and_returns_the_error()
 {
    let ws = Workspace::new().await;
    let parent = ws.parent_with_children(3).await;
    ws.seed(&format!(
        "UPDATE child_counters SET last_child = 1 WHERE parent_id = '{parent}'; \
         INSERT INTO metadata (key, value) VALUES ('k', 'a'), ('k', 'b'); \
         CREATE TRIGGER fail_dedupe BEFORE DELETE ON metadata \
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;"
    ))
    .await;

    let err = ws
        .recover()
        .await
        .expect_err("the injected failure surfaces");
    assert!(matches!(err, EngineError::Storage { .. }), "{err:?}");
    let code = unblock_error::CodedError::code(&err).as_str().to_string();

    assert_eq!(
        ws.child_high_water(&parent).await,
        Some(1),
        "the first action rolled back"
    );
    let sql = "SELECT value FROM metadata WHERE key = 'k' ORDER BY rowid";
    assert_eq!(query_strings(ws.db(), sql).await, ["a", "b"]);

    let runs = ws.run_dirs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        manifest_actions(&runs[0]),
        [
            (
                "raise_child_counter".to_string(),
                Some(parent),
                "failed".to_string(),
                Some(code.clone())
            ),
            (
                "dedupe_metadata_key".to_string(),
                Some("k".to_string()),
                "failed".to_string(),
                Some(code)
            ),
        ]
    );
}

/// (k) A `repair` that fails after its migrate ladder committed records `migrate` as `applied`
/// and the failed action as `failed`, and returns the error.
///
/// The stamp is one step behind and the second action fails on the same `BEFORE DELETE ON
/// metadata` trigger as above, so the ladder commits and the action transaction then rolls back.
#[tokio::test]
async fn recover_failure_after_the_migrate_ladder_records_migrate_applied_and_the_action_failed() {
    let current = unblock_storage::CURRENT_SCHEMA_VERSION;
    let ws = Workspace::new().await;
    ws.seed(&format!(
        "INSERT INTO metadata (key, value) VALUES ('k', 'a'), ('k', 'b'); \
         CREATE TRIGGER fail_dedupe BEFORE DELETE ON metadata \
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END; \
         PRAGMA user_version = {};",
        current - 1
    ))
    .await;

    let err = ws
        .recover()
        .await
        .expect_err("the injected failure surfaces");
    assert!(matches!(err, EngineError::Storage { .. }), "{err:?}");
    let code = unblock_error::CodedError::code(&err).as_str().to_string();

    assert_eq!(
        query_i64(ws.db(), "PRAGMA user_version").await,
        current,
        "the ladder committed before the action transaction failed"
    );
    let sql = "SELECT value FROM metadata WHERE key = 'k' ORDER BY rowid";
    assert_eq!(query_strings(ws.db(), sql).await, ["a", "b"]);

    let runs = ws.run_dirs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        manifest_actions(&runs[0]),
        [
            (
                "dedupe_metadata_key".to_string(),
                Some("k".to_string()),
                "failed".to_string(),
                Some(code)
            ),
            ("migrate".to_string(), None, "applied".to_string(), None),
        ]
    );
}

// ------------------------------------------------------------------------------------------------
// THE UNSAFE GATE (tracker comment 389)
// ------------------------------------------------------------------------------------------------

/// The rows of a `recover()` stopped by the Unsafe gate: `code`'s anomaly row, its manual path, the
/// `recover` row naming it, and no action, evidence or drift row.
fn assert_unsafe_gate_report(report: &DiagnosticReport, code: &str) {
    assert_eq!(details(report, code).len(), 1, "{report:?}");
    let planned = details(report, "repair_planned");
    assert_eq!(planned.len(), 1, "{report:?}");
    assert!(planned[0].contains("(manual)"), "{planned:?}");
    let recover = details(report, "recover");
    assert_eq!(recover.len(), 1, "{report:?}");
    assert!(
        recover[0].contains("nothing repaired") && recover[0].contains(code),
        "{recover:?}"
    );
    assert!(details(report, "repair").is_empty(), "{report:?}");
    assert!(details(report, "evidence").is_empty(), "{report:?}");
    assert!(
        details(report, "child_counter_drift").is_empty(),
        "{report:?}"
    );
}

/// Page corruption beside a lowered counter: `recover()` repairs nothing, the counter stays
/// lowered and no run directory exists. The corrupt page is one more `integrity_check` row from a
/// double over the real store.
///
/// MUTANT KILLED: removing the gate raises the counter and creates a run directory.
#[tokio::test]
async fn recover_with_page_corruption_and_drift_repairs_nothing_and_creates_no_run_dir() {
    let ws = Workspace::new().await;
    let parent = ws.parent_with_children(3).await;
    ws.seed(&format!(
        "UPDATE child_counters SET last_child = 1 WHERE parent_id = '{parent}'"
    ))
    .await;

    let storage = LibsqlStorage::open_local(ws.db(), DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    let inner: Arc<dyn Storage> = Arc::new(storage);
    let shared: Arc<dyn Storage> =
        common::parked::ParkedStorage::new_with_integrity_row(inner, "Page 7: never used");
    let session = common::session_over_in_dir(
        shared,
        SessionConfig::default(),
        ws.root.clone(),
        ws.paths.unblock_dir.clone(),
    )
    .await;
    let report = session.recover().await.expect("recover");
    drop(session);

    assert_unsafe_gate_report(&report, "page_corruption");
    assert_eq!(
        ws.child_high_water(&parent).await,
        Some(1),
        "the counter is still lowered"
    );
    assert!(!ws.recovery_root().exists(), "no .recovery directory");
}

/// A schema stamp above this build's beside a lowered counter: `recover()` repairs nothing, the
/// stamp and the counter stay, and no run directory exists.
///
/// MUTANT KILLED: removing the gate raises the counter and creates a run directory.
#[tokio::test]
async fn recover_with_schema_stamp_ahead_and_drift_repairs_nothing_and_creates_no_run_dir() {
    let ahead = unblock_storage::CURRENT_SCHEMA_VERSION + 1;
    let ws = Workspace::new().await;
    let parent = ws.parent_with_children(3).await;
    ws.seed(&format!(
        "UPDATE child_counters SET last_child = 1 WHERE parent_id = '{parent}'; \
         PRAGMA user_version = {ahead};"
    ))
    .await;

    let report = ws.recover().await.expect("recover");

    assert_unsafe_gate_report(&report, "schema_stamp_ahead");
    assert_eq!(query_i64(ws.db(), "PRAGMA user_version").await, ahead);
    assert_eq!(
        ws.child_high_water(&parent).await,
        Some(1),
        "the counter is still lowered"
    );
    assert!(!ws.recovery_root().exists(), "no .recovery directory");
}

// ------------------------------------------------------------------------------------------------
// (n) NO SECOND DESCRIPTOR, and the pre-open repair
// ------------------------------------------------------------------------------------------------

/// (n) `gather_preopen` refuses while this process holds a connection to the database file.
#[tokio::test]
async fn gather_preopen_with_a_live_session_is_refused_pre_open_with_live_connection() {
    let ws = Workspace::new().await;
    let (session, storage) = ws.open().await;
    match gather_preopen(&ws.paths) {
        Err(EngineError::PreOpenWithLiveConnection { path }) => assert_eq!(path, ws.db()),
        other => panic!("expected PreOpenWithLiveConnection, got {other:?}"),
    }
    drop(session);
    drop(storage);
    gather_preopen(&ws.paths).expect("no live connection once the session is gone");
}

/// (n, G3, `ub-2vp`) A refused `gather_preopen` opens no descriptor on the database file, so this
/// process's store holds `SQLite`'s SHARED lock before AND after the refusal (`/proc/locks`). It
/// catches a reorder of `read_db_fact` above the live-connection check.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn gather_preopen_refusal_opens_no_descriptor_on_the_database() {
    use common::proc_locks::{LockKind, PROC_LOCKS_ATTEMPTS, SharedRangeReads, shared_range};

    let ws = Workspace::new().await;
    let (session, storage) = ws.open().await;
    storage.schema_version().await.expect("one read");
    let pid = i32::try_from(std::process::id()).expect("a pid fits i32");
    let before = shared_range(ws.db(), pid);
    match gather_preopen(&ws.paths) {
        Err(EngineError::PreOpenWithLiveConnection { path }) => assert_eq!(path, ws.db()),
        other => panic!("expected PreOpenWithLiveConnection, got {other:?}"),
    }
    let after = shared_range(ws.db(), pid);
    let leg = std::env::var("GITHUB_JOB").unwrap_or_else(|_| "local".to_owned());
    for (point, seen) in [("before-refusal", &before), ("after-refusal", &after)] {
        eprintln!(
            "UB2VP-MEASURE leg=linux/{leg} cell=G3 point={point} holder_pid={pid} sh={:?} \
             attempts={} proc_reads={} proc_bytes={}",
            seen.kind, seen.attempts, seen.proc_reads, seen.proc_bytes
        );
    }
    let explain = |seen: &SharedRangeReads| match seen.kind {
        None => format!("it was absent on all {PROC_LOCKS_ATTEMPTS} /proc/locks reads"),
        Some(kind) => format!(
            "expected Read, saw {kind:?} over {} /proc/locks reads",
            seen.attempts
        ),
    };
    assert_eq!(
        before.kind,
        Some(LockKind::Read),
        "the store holds SHARED before, yet {}; raw lines on the inode per read {:?}",
        explain(&before),
        before.raw_on_inode
    );
    assert_eq!(
        after.kind,
        Some(LockKind::Read),
        "a refused gather_preopen lost the store's SHARED read lock ({}), so a std::fs open/close \
         of the database ran before gather_preopen's `live > 0` check \
         (`PreOpenWithLiveConnection`), e.g. `read_db_fact` reordered above it; raw lines on the \
         inode per read {:?}",
        explain(&after),
        after.raw_on_inode
    );
    drop(session);
    drop(storage);
}

/// (n, G3p, `ub-2vp`) The engine's copy of the `/proc/locks` parser reads the same canned lines as
/// fdp, including the same inode on another device and the btrfs inode-only fallback.
#[cfg(target_os = "linux")]
#[test]
fn proc_locks_parser_reads_canned_lines() {
    use common::proc_locks::{
        HeldLock, LockKind, dev_major_minor, parse_proc_locks, raw_lines_on_inode,
    };

    let text = "\
1: POSIX  ADVISORY  READ  4242 08:01:123456 1073741826 1073742335
1: -> POSIX  ADVISORY  WRITE 4343 08:01:123456 1073741826 1073742335
2: POSIX  ADVISORY  WRITE 4242 fd:00:999 0 EOF
3: FLOCK  ADVISORY  WRITE 5151 08:01:777 0 EOF
4: POSIX  ADVISORY  WRITE 4242 08:01:123456 128 255
5: POSIX  ADVISORY  READ  4242 00:2a:123456 0 EOF
6: POSIX  ADVISORY  READ  5353 00:2c:4040 1073741826 1073742335
7: POSIX  ADVISORY  READ  5353 00:2c:4040 1073741826 1073742335
";
    assert_eq!(
        parse_proc_locks(text, (8, 1), 123_456, Some(4242)),
        [
            HeldLock {
                kind: LockKind::Read,
                pid: 4242,
                start: 1_073_741_826,
                len: 510,
                dev: Some("08:01".to_owned()),
            },
            HeldLock {
                kind: LockKind::Write,
                pid: 4242,
                start: 128,
                len: 128,
                dev: Some("08:01".to_owned()),
            },
        ],
        "a line carries the stat device 08:01, so the same inode on device 00:2a is excluded"
    );
    assert_eq!(
        parse_proc_locks(text, (0xfd, 0), 999, None),
        [HeldLock {
            kind: LockKind::Write,
            pid: 4242,
            start: 0,
            len: 0,
            dev: Some("fd:00".to_owned()),
        }],
        "the device is read as hex"
    );
    assert_eq!(
        parse_proc_locks(text, (0, 0x2a), 123_456, None),
        [HeldLock {
            kind: LockKind::Read,
            pid: 4242,
            start: 0,
            len: 0,
            dev: Some("00:2a".to_owned()),
        }],
        "a line carries the stat device 00:2a, so the lines on device 08:01 are excluded"
    );
    assert_eq!(
        parse_proc_locks(text, (0, 0x3a), 4040, Some(5353)),
        [HeldLock {
            kind: LockKind::Read,
            pid: 5353,
            start: 1_073_741_826,
            len: 510,
            dev: Some("00:2c".to_owned()),
        }],
        "no line carries the subvolume device 00:3a (btrfs), so the inode alone matches"
    );
    assert_eq!(
        parse_proc_locks(text, (8, 2), 123_456, None).len(),
        3,
        "no line carries device 08:02, so every line on the inode matches"
    );
    assert_eq!(
        parse_proc_locks(text, (0, 0x3a), 4040, None).len(),
        1,
        "a torn read that repeats a line yields one record"
    );
    assert_eq!(
        raw_lines_on_inode(text, 4040).len(),
        2,
        "the failure evidence keeps every raw line on the inode"
    );
    assert_eq!(
        raw_lines_on_inode(text, 123_456).len(),
        4,
        "the failure evidence keeps waiters and every device"
    );
    assert!(parse_proc_locks(text, (8, 1), 123_456, Some(1)).is_empty());
    assert!(parse_proc_locks(text, (8, 1), 31_337, None).is_empty());
    assert_eq!(dev_major_minor(0x0801), (8, 1));
    assert_eq!(dev_major_minor(0x1001_0305), (0x103, 0x1_0005));
}

/// (g, engine half) A lone `-shm` is renamed into `quarantine/` with its SHA-256 in the manifest,
/// its capture is the `original/` copy listed apart in `originals`, and the final level is
/// `Healthy`.
#[tokio::test]
async fn repair_preopen_quarantines_a_lone_shm_and_records_its_sha256() {
    let ws = Workspace::new().await;
    let shm = unblock_health::sidecar(ws.db(), unblock_health::SHM_SUFFIX);
    let bytes: Vec<u8> = (0..=255_u8).cycle().take(4096).collect();
    std::fs::write(&shm, &bytes).expect("write a lone -shm");
    let original = ws.root.join("shm-original");
    std::fs::write(&original, &bytes).expect("keep the original bytes");
    let (_, expected_sha) = unblock_health::recovery::sha256_file(&original).expect("hash");

    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    let capture = capture_sidecars(&ws.paths, &facts)
        .await
        .expect("capture_sidecars");
    assert_eq!(capture.files.len(), 1, "{capture:?}");
    assert_eq!(capture.files[0].name, "unblock.db-shm");
    assert_eq!(capture.files[0].bytes, bytes);
    assert_eq!(capture.files[0].sha256, expected_sha);
    let report = classify_preopen(&facts);
    assert_eq!(
        report.level,
        HealthLevel::Recoverable,
        "{:?}",
        report.report
    );
    assert!(
        report
            .planned
            .iter()
            .any(|r| r.action == PlannedAction::QuarantineSidecar
                && r.path == RepairPath::PreopenExclusive)
    );

    let outcome = repair_preopen(&ws.paths, &report.planned, capture)
        .await
        .expect("repair_preopen");
    assert!(!outcome.refused, "{outcome:?}");
    assert_eq!(outcome.actions.len(), 1);
    assert_eq!(outcome.actions[0].action, PlannedAction::QuarantineSidecar);
    assert_eq!(outcome.actions[0].outcome, ActionResult::Applied);
    let run = outcome.run_dir.clone().expect("a run directory");
    assert_eq!(ws.run_dirs(), std::slice::from_ref(&run));
    let findings = outcome_findings(&outcome);
    assert_eq!(findings[0].label, "repair");
    assert_eq!(findings[0].detail, "quarantine_sidecar: applied");
    assert_eq!(findings[1].label, "evidence");

    assert!(!shm.exists(), "the live -shm is gone");
    let quarantined = run.join("quarantine").join("unblock.db-shm");
    assert_eq!(
        std::fs::read(&quarantined).expect("quarantined -shm"),
        bytes
    );
    let doc = manifest(&run);
    assert_eq!(doc["phase"], "preopen");
    let listed = assert_manifest_hashes(&run);
    assert!(
        listed.contains(&"quarantine/unblock.db-shm".to_string()),
        "{listed:?}"
    );
    let entry = doc["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|f| f["path"] == "quarantine/unblock.db-shm")
        .expect("the quarantined -shm is listed");
    assert_eq!(entry["sha256"].as_str(), Some(expected_sha.as_str()));
    assert_eq!(
        manifest_actions(&run),
        [(
            "quarantine_sidecar".to_string(),
            None,
            "applied".to_string(),
            None
        )]
    );
    // No gate open ran, so the original carries no rebuilt note.
    assert_original_shm(&run, &bytes, &expected_sha, None);

    let after = classify_preopen(&gather_preopen(&ws.paths).expect("gather_preopen"));
    assert_eq!(after.level, HealthLevel::Healthy, "{:?}", after.report);
}

/// The `original/unblock.db-shm` capture and its `originals` row with `note`.
fn assert_original_shm(run: &Path, bytes: &[u8], sha256: &str, note: Option<&str>) {
    assert_eq!(
        std::fs::read(run.join("original").join("unblock.db-shm")).expect("the original -shm"),
        bytes
    );
    let doc = manifest(run);
    let originals = doc["originals"].as_array().expect("originals array");
    assert_eq!(originals.len(), 1, "{originals:?}");
    assert_eq!(originals[0]["path"], "original/unblock.db-shm");
    assert_eq!(originals[0]["sha256"].as_str(), Some(sha256));
    assert_eq!(originals[0]["bytes"].as_u64(), Some(bytes.len() as u64));
    assert_eq!(originals[0]["note"].as_str(), note, "{originals:?}");
    assert!(
        !assert_manifest_hashes(run)
            .iter()
            .any(|path| path.starts_with("original/")),
        "originals stay apart from files"
    );
}

/// (ruling 390) The `doctor --repair` order: the facts, the capture, the gate's read-only open,
/// then the exclusive window over the plan from the ORIGINAL facts. The window accepts what the
/// gate's open left beside the database, the live `-shm` still moves into `quarantine/`, and the
/// capture is the original evidence.
#[tokio::test]
async fn repair_preopen_after_the_gate_open_quarantines_the_shm_and_keeps_its_capture() {
    let ws = Workspace::new().await;
    let shm = unblock_health::sidecar(ws.db(), unblock_health::SHM_SUFFIX);
    let bytes: Vec<u8> = (0..=255_u8).cycle().take(4096).collect();
    std::fs::write(&shm, &bytes).expect("write a lone -shm");

    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    let mut capture = capture_sidecars(&ws.paths, &facts)
        .await
        .expect("capture_sidecars");
    let expected_sha = capture.files[0].sha256.clone();
    let gate = ws.doctor().await;
    assert_ne!(gate.level, HealthLevel::Unsafe, "{:?}", gate.report);
    capture.gate_opened = true;

    let planned = classify_preopen(&facts).planned;
    let outcome = repair_preopen(&ws.paths, &planned, capture)
        .await
        .expect("repair_preopen");
    assert!(!outcome.refused, "{outcome:?}");
    assert_eq!(outcome.actions.len(), 1);
    assert_eq!(outcome.actions[0].outcome, ActionResult::Applied);
    assert!(!shm.exists(), "the live -shm is gone");
    let run = outcome.run_dir.clone().expect("a run directory");
    assert_original_shm(
        &run,
        &bytes,
        &expected_sha,
        Some("rebuilt by SQLite's read-only open"),
    );
}

/// (tracker comment 393) A `-shm` above the 64 MiB capture cap is a capture refusal
/// (`DATABASE_ERROR`, exit 2) naming the cap. The file is sparse, so the cell writes no data.
#[tokio::test]
async fn capture_sidecars_refuses_a_shm_above_the_cap() {
    let ws = Workspace::new().await;
    let wal = unblock_health::sidecar(ws.db(), unblock_health::WAL_SUFFIX);
    if wal.exists() {
        std::fs::remove_file(&wal).expect("remove the -wal");
    }
    let shm = unblock_health::sidecar(ws.db(), unblock_health::SHM_SUFFIX);
    let cap = unblock_engine::preopen::CAPTURE_CAP_BYTES;
    assert_eq!(cap, 64 * 1024 * 1024);
    std::fs::File::create(&shm)
        .and_then(|file| file.set_len(cap + 1))
        .expect("a sparse -shm one byte above the cap");

    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    let err = capture_sidecars(&ws.paths, &facts)
        .await
        .expect_err("over the cap");
    match &err {
        EngineError::SidecarCaptureRefused { path, reason } => {
            assert_eq!(path, &shm);
            assert!(reason.contains(&cap.to_string()), "{reason}");
        }
        other => panic!("expected SidecarCaptureRefused, got {other:?}"),
    }
    assert_eq!(
        unblock_error::CodedError::code(&err),
        unblock_error::ErrorCode::DatabaseError
    );
    assert_eq!(unblock_error::CodedError::code(&err).exit_code(), 2);
}

/// (tracker comments 390 and 393) A 5-byte `-wal` (`truncated_wal`) is captured, and
/// `repair_preopen` writes it under `original/` with its SHA-256 in `originals` and no rebuilt
/// note, apart from the window's `raw/` copy.
#[tokio::test]
async fn capture_sidecars_keeps_a_truncated_wal_as_its_original() {
    let ws = Workspace::new().await;
    let shm = unblock_health::sidecar(ws.db(), unblock_health::SHM_SUFFIX);
    if shm.exists() {
        std::fs::remove_file(&shm).expect("remove the -shm");
    }
    let wal = unblock_health::sidecar(ws.db(), unblock_health::WAL_SUFFIX);
    let bytes = vec![1_u8, 2, 3, 4, 5];
    std::fs::write(&wal, &bytes).expect("write a 5-byte -wal");

    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    let mut capture = capture_sidecars(&ws.paths, &facts)
        .await
        .expect("capture_sidecars");
    assert_eq!(capture.files.len(), 1, "{capture:?}");
    assert_eq!(capture.files[0].name, "unblock.db-wal");
    assert_eq!(capture.files[0].bytes, bytes);
    let sha = unblock_health::recovery::sha256_bytes(&bytes);
    assert_eq!(capture.files[0].sha256, sha);
    capture.gate_opened = true;

    let outcome = repair_preopen(&ws.paths, &classify_preopen(&facts).planned, capture)
        .await
        .expect("repair_preopen");
    assert!(!outcome.refused, "{outcome:?}");
    let run = outcome.run_dir.clone().expect("a run directory");
    assert_eq!(
        std::fs::read(run.join("original").join("unblock.db-wal")).expect("the original -wal"),
        bytes
    );
    let doc = manifest(&run);
    let originals = doc["originals"].as_array().expect("originals array");
    assert_eq!(originals.len(), 1, "{originals:?}");
    assert_eq!(originals[0]["path"], "original/unblock.db-wal");
    assert_eq!(originals[0]["sha256"].as_str(), Some(sha.as_str()));
    assert!(
        originals[0]["note"].is_null(),
        "only a -shm is rebuilt by the gate's open: {originals:?}"
    );
    assert_eq!(
        std::fs::read(run.join("raw").join("unblock.db-wal")).expect("the raw -wal"),
        bytes
    );
}

/// (tracker comment 393) A hot `-journal` is NOT captured in memory: its evidence is the window's
/// `raw/` copy, taken before `SQLite` rolls it back, and the manifest lists no original for it.
#[tokio::test]
async fn capture_sidecars_leaves_a_hot_journal_to_the_raw_copy() {
    let ws = Workspace::new().await;
    let journal = hot_journal(ws.db()).await;

    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    assert!(facts.journal_present, "{facts:?}");
    let capture = capture_sidecars(&ws.paths, &facts)
        .await
        .expect("capture_sidecars");
    assert!(
        capture.files.is_empty(),
        "the -journal is not captured in memory: {capture:?}"
    );

    let outcome = repair_preopen(&ws.paths, &classify_preopen(&facts).planned, capture)
        .await
        .expect("repair_preopen");
    assert!(!outcome.refused, "{outcome:?}");
    let run = outcome.run_dir.clone().expect("a run directory");
    assert_eq!(
        std::fs::read(run.join("raw").join("unblock.db-journal")).expect("the raw -journal"),
        journal,
        "the raw/ copy holds the pre-rollback journal"
    );
    assert!(!run.join("original").exists(), "no in-memory original");
    let doc = manifest(&run);
    assert!(
        doc["originals"].as_array().is_some_and(Vec::is_empty),
        "{doc}"
    );
    assert!(
        !unblock_health::sidecar(ws.db(), unblock_health::JOURNAL_SUFFIX).exists(),
        "SQLite unlinked the rolled-back journal"
    );
}

/// A hot rollback `-journal` beside the database at `db`: the database leaves WAL mode, a write
/// transaction's journal is captured, the transaction rolls back, and the bytes are written back
/// once no connection remains. Returns the journal bytes.
async fn hot_journal(db: &Path) -> Vec<u8> {
    let path = unblock_health::sidecar(db, unblock_health::JOURNAL_SUFFIX);
    let bytes = {
        let conn = common::raw_connection(db).await;
        let mut rows = conn
            .query("PRAGMA journal_mode=DELETE", ())
            .await
            .expect("leave WAL mode");
        rows.next().await.expect("row").expect("present");
        drop(rows);
        conn.execute("PRAGMA synchronous=OFF", ())
            .await
            .expect("synchronous off");
        conn.execute("BEGIN IMMEDIATE", ()).await.expect("begin");
        conn.execute(
            "INSERT INTO metadata (key, value) VALUES ('hot', 'journal')",
            (),
        )
        .await
        .expect("write a page");
        let bytes = std::fs::read(&path).expect("the live journal");
        conn.execute("ROLLBACK", ()).await.expect("rollback");
        bytes
    };
    assert!(bytes.first().is_some_and(|b| *b != 0), "a hot header");
    std::fs::write(&path, &bytes).expect("write the hot journal back");
    bytes
}

/// (ruling 390) A symlinked `-shm` is a capture refusal. The capture never reads through it.
#[cfg(unix)]
#[tokio::test]
async fn capture_sidecars_refuses_a_symlinked_shm() {
    let ws = Workspace::new().await;
    let target = ws.root.join("elsewhere");
    std::fs::write(&target, b"not evidence").expect("symlink target");
    let shm = unblock_health::sidecar(ws.db(), unblock_health::SHM_SUFFIX);
    std::os::unix::fs::symlink(&target, &shm).expect("symlink the -shm");

    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    match capture_sidecars(&ws.paths, &facts).await {
        Err(EngineError::SidecarCaptureRefused { path, .. }) => assert_eq!(path, shm),
        other => panic!("expected SidecarCaptureRefused, got {other:?}"),
    }
}

/// (ruling 390) The capture refuses while this process holds a connection to the database file.
#[tokio::test]
async fn capture_sidecars_with_a_live_connection_is_refused() {
    let ws = Workspace::new().await;
    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    let (_session, _storage) = ws.open().await;
    match capture_sidecars(&ws.paths, &facts).await {
        Err(EngineError::PreOpenWithLiveConnection { path }) => assert_eq!(path, ws.db()),
        other => panic!("expected PreOpenWithLiveConnection, got {other:?}"),
    }
}

/// (clause (8)) The session-less report of a hot `-journal` says the database classes are read
/// only after `doctor --repair` lets `SQLite` roll the journal back.
#[tokio::test]
async fn classify_preopen_with_a_hot_journal_says_the_database_classes_wait_for_the_rollback() {
    let ws = Workspace::new().await;
    let journal = unblock_health::sidecar(ws.db(), unblock_health::JOURNAL_SUFFIX);
    std::fs::write(&journal, b"a hot journal").expect("write a -journal");

    let report = classify_preopen(&gather_preopen(&ws.paths).expect("gather_preopen"));
    let rows = details(&report.report, "journal_sidecar_present");
    assert_eq!(rows.len(), 1, "{:?}", report.report);
    assert!(
        rows[0].contains("read only after `unblock doctor --repair`"),
        "{rows:?}"
    );
}

/// A dangling `-shm` symlink is reported as present, so the window sees it and refuses it as
/// `not_a_regular_file` instead of the classifier missing it.
#[cfg(unix)]
#[tokio::test]
async fn gather_preopen_reports_a_dangling_sidecar_symlink() {
    let ws = Workspace::new().await;
    let shm = unblock_health::sidecar(ws.db(), unblock_health::SHM_SUFFIX);
    if shm.exists() {
        std::fs::remove_file(&shm).expect("remove the -shm");
    }
    std::os::unix::fs::symlink(ws.root.join("gone"), &shm).expect("a dangling -shm link");

    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    assert!(facts.shm_present, "{facts:?}");
}

/// A dangling `-wal` symlink is reported as present too, so plain `doctor` sees it.
#[cfg(unix)]
#[tokio::test]
async fn gather_preopen_reports_a_dangling_wal_symlink() {
    let ws = Workspace::new().await;
    let wal = unblock_health::sidecar(ws.db(), unblock_health::WAL_SUFFIX);
    if std::fs::symlink_metadata(&wal).is_ok() {
        std::fs::remove_file(&wal).expect("remove the -wal");
    }
    std::os::unix::fs::symlink(ws.root.join("gone"), &wal).expect("a dangling -wal link");

    let facts = gather_preopen(&ws.paths).expect("gather_preopen");
    assert!(facts.wal_len.is_some(), "{facts:?}");
}

// ------------------------------------------------------------------------------------------------
// PRD §4 D5 amendment and D58 amendment (`ub-c15`): an export that is a session file
// ------------------------------------------------------------------------------------------------

/// The conflict-marker line seeded into the database, so a scan that READ the database would report
/// `jsonl_conflict_markers` and recover would plan `regenerate_export`.
const MARKER_LINE: &str = "<<<<<<< HEAD";

/// Seed one issue whose description holds [`MARKER_LINE`] through `Session::create_issue`, drop
/// every store, and assert the precondition: the database FILE bytes carry `\n<<<<<<< `.
async fn seed_marker(ws: &Workspace) {
    {
        let (session, _storage) = ws.open().await;
        session
            .create_issue(NewIssue {
                title: "carries a marker line".to_string(),
                description: Some(format!("before\n{MARKER_LINE}\nafter")),
                ..NewIssue::default()
            })
            .await
            .expect("create the marker issue");
    }
    let bytes = std::fs::read(ws.db()).expect("read the database file");
    assert!(
        bytes.windows(9).any(|w| w == b"\n<<<<<<< "),
        "precondition: the database file carries a conflict-marker line"
    );
}

/// The database's inode (unix) and SHA-256.
fn db_identity(db: &Path) -> (Option<u64>, String) {
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt as _;
        Some(std::fs::metadata(db).expect("stat the database").ino())
    };
    #[cfg(not(unix))]
    let ino = None;
    let (_, sha) = unblock_health::recovery::sha256_file(db).expect("hash the database");
    (ino, sha)
}

/// Linux: this process's store holds `SQLite`'s SHARED lock on `db` (`/proc/locks`, the G3 witness).
/// Elsewhere a no-op: `F_GETLK` cannot see this process's own lock, so the macOS legs of the lock
/// claim are the holder cells.
#[cfg(target_os = "linux")]
fn assert_own_shared(cell: &str, point: &str, db: &Path) {
    use common::proc_locks::{LockKind, shared_range};

    let pid = i32::try_from(std::process::id()).expect("a pid fits i32");
    let seen = shared_range(db, pid);
    let leg = std::env::var("GITHUB_JOB").unwrap_or_else(|_| "local".to_owned());
    eprintln!(
        "UB2VP-MEASURE leg=linux/{leg} cell={cell} point={point} holder_pid={pid} sh={:?} \
         attempts={} proc_reads={} proc_bytes={}",
        seen.kind, seen.attempts, seen.proc_reads, seen.proc_bytes
    );
    assert_eq!(
        seen.kind,
        Some(LockKind::Read),
        "{cell} at {point}: the store lost SQLite's SHARED lock (a descriptor on a session file was \
         closed); raw lines on the inode per read {:?}",
        seen.raw_on_inode
    );
}

#[cfg(not(target_os = "linux"))]
fn assert_own_shared(_cell: &str, _point: &str, _db: &Path) {}

/// A `.tmp` left in `dir`, if any.
fn temp_in(dir: &Path) -> Option<String> {
    std::fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .find(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("tmp"))
        })
}

/// (E1, D58 amendment) `doctor` with the export a hard link or a symlink to the database reports
/// `export_is_session_file`, never `jsonl_conflict_markers` (the database carries a marker line, so
/// a scan that read it would) or `jsonl_unscannable`, and the session keeps SHARED.
#[cfg(unix)]
#[tokio::test]
async fn c15i0_doctor_reports_a_session_file_export_and_never_scans_it() {
    for shape in ["hard-link", "symlink"] {
        let ws = Workspace::new().await;
        seed_marker(&ws).await;
        if shape == "hard-link" {
            std::fs::hard_link(ws.db(), &ws.paths.jsonl_path).expect("hard link");
        } else {
            std::os::unix::fs::symlink(ws.db(), &ws.paths.jsonl_path).expect("symlink");
        }
        let facts = gather_preopen(&ws.paths).expect("gather_preopen");
        let (session, storage) = ws.open_read_only().await;
        storage.schema_version().await.expect("one read");
        assert_own_shared("c15i0", &format!("{shape}/before-doctor"), ws.db());
        let health = session.doctor(&facts).await.expect("doctor");
        assert_own_shared("c15i0", &format!("{shape}/after-doctor"), ws.db());
        assert_eq!(
            details(&health.report, "export_is_session_file").len(),
            1,
            "{shape}: {:?}",
            health.report
        );
        assert!(
            details(&health.report, "jsonl_conflict_markers").is_empty()
                && details(&health.report, "jsonl_unscannable").is_empty(),
            "{shape}: the session-file export was scanned: {:?}",
            health.report
        );
        drop(session);
        drop(storage);
    }
}

/// (E2, D58 amendment) `recover()` with the export resolving to the database plans no
/// `regenerate_export`: it reports `nothing to repair online`, creates no run directory, and leaves
/// the database's inode and SHA-256 unchanged and its integrity `ok`. Shape (i) is the data-loss
/// shape (`db_path == jsonl_path`): on unpatched main the copy passes the allowlist and
/// `write_export` renames a JSONL over the database. Shapes (ii) a hard link and (iii)
/// `jsonl_path = .unblock/unblock.db`.
#[tokio::test]
async fn c15i1_recover_skips_an_export_that_is_the_database_and_regenerates_nothing() {
    let mut shapes = vec![
        (
            "same-name",
            Workspace::with_names("issues.jsonl", "issues.jsonl").await,
        ),
        (
            "jsonl-is-db",
            Workspace::with_names("unblock.db", "unblock.db").await,
        ),
    ];
    if cfg!(unix) {
        shapes.push(("hard-link", Workspace::new().await));
    }
    for (shape, ws) in shapes {
        seed_marker(&ws).await;
        if shape == "hard-link" {
            std::fs::hard_link(ws.db(), &ws.paths.jsonl_path).expect("hard link");
        }
        let before = db_identity(ws.db());
        let (session, storage) = ws.open().await;
        storage.schema_version().await.expect("one read");
        assert_own_shared("c15i1", &format!("{shape}/before-recover"), ws.db());
        let report = session.recover().await.expect("recover");
        assert_own_shared("c15i1", &format!("{shape}/after-recover"), ws.db());
        assert_eq!(
            details(&report, "recover"),
            vec!["nothing to repair online"],
            "{shape}: {report:?}"
        );
        assert!(
            !format!("{report:?}").contains("regenerate_export"),
            "{shape}: a regeneration was planned: {report:?}"
        );
        assert!(ws.run_dirs().is_empty(), "{shape}: a run directory exists");
        drop(session);
        drop(storage);
        assert_eq!(
            db_identity(ws.db()),
            before,
            "{shape}: the database was replaced or rewritten"
        );
        let (session, _storage) = ws.open().await;
        // `Session::integrity_check` returns only the problem rows: empty is `ok`.
        let problems = session.integrity_check().await.expect("integrity_check");
        assert!(problems.is_empty(), "{shape}: {problems:?}");
    }
}

/// (E3, D5 amendment) A regeneration target that RACES into the database between recover's evidence
/// copy and `write_export` (a hard link to the database renamed over a distinct, marker-bearing
/// `issues.jsonl` by the repair hook) is refused `PATH_TRAVERSAL`/`SessionDatabase`: the manifest
/// records the regeneration `failed`, `issues.jsonl` is still the database's inode (no rename over
/// it), integrity stays `ok` and no temp is left.
#[cfg(unix)]
#[tokio::test]
async fn c15i2_recover_refuses_a_regeneration_target_that_raced_into_the_database() {
    use std::os::unix::fs::MetadataExt as _;
    use unblock_error::CodedError as _;

    let ws = Workspace::new().await;
    {
        let (session, _storage) = ws.open().await;
        session
            .create_issue(new_issue("kept", None))
            .await
            .expect("create");
    }
    std::fs::write(
        &ws.paths.jsonl_path,
        format!("{MARKER_LINE}\n{{\"id\":\"ub-ours\"}}\n=======\n"),
    )
    .expect("write the marker-bearing export");

    let db = ws.db().to_path_buf();
    let jsonl = ws.paths.jsonl_path.clone();
    let staged = ws.paths.unblock_dir.join("raced-link");
    let storage = Arc::new(
        LibsqlStorage::open_local(ws.db(), DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local"),
    );
    storage.schema_version().await.expect("one read");
    let raced: Arc<dyn Storage> =
        common::parked::ParkedStorage::with_before_repair(storage.clone(), move || {
            std::fs::hard_link(&db, &staged).expect("link the database");
            std::fs::rename(&staged, &jsonl).expect("race the link over the export");
        });
    let session = ws.session_over_dyn(raced).await;
    assert_own_shared("c15i2", "before-recover", ws.db());
    let err = session
        .recover()
        .await
        .expect_err("the raced regeneration is refused");
    assert_own_shared("c15i2", "after-recover", ws.db());
    assert_eq!(err.code(), unblock_error::ErrorCode::PathTraversal, "{err}");
    assert_eq!(err.code().exit_code(), 6);
    assert!(err.to_string().contains("SessionDatabase"), "{err}");

    let runs = ws.run_dirs();
    assert_eq!(runs.len(), 1, "one run directory");
    let regenerate: Vec<_> = manifest_actions(&runs[0])
        .into_iter()
        .filter(|(action, ..)| action == "regenerate_export")
        .collect();
    assert_eq!(regenerate.len(), 1, "{regenerate:?}");
    assert_eq!(regenerate[0].2, "failed", "{regenerate:?}");
    assert_eq!(
        std::fs::metadata(&ws.paths.jsonl_path)
            .expect("stat the export")
            .ino(),
        std::fs::metadata(ws.db()).expect("stat the database").ino(),
        "the export is still the database's link: nothing was renamed over it"
    );
    assert_eq!(
        temp_in(&ws.paths.unblock_dir),
        None,
        "a temp was left behind"
    );
    drop(session);
    drop(storage);
    let (session, _storage) = ws.open().await;
    let problems = session.integrity_check().await.expect("integrity_check");
    assert!(problems.is_empty(), "integrity_check: {problems:?}");
}
