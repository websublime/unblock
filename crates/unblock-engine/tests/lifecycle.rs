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
async fn recover_returns_health_seam_and_writes_nothing() {
    // recover() STAYS the typed FeatureNotWired health seam through v1 (F1/D29) — its --repair /
    // evidence body is v1.1. It must write nothing.
    let session = session().await;
    let issue = common::issue("ub-0001", unblock_model::Priority::MEDIUM, 1000);
    session.create(&issue).await.expect("create");
    let before = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");

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

/// T3.3 (HEALTH-LITE, D29): `doctor()` is WIRED — it returns a `DiagnosticReport` of the reused
/// `DiagnosticKind::Info` (F2). On a fresh in-memory workspace (clean integrity, no on-disk file-state
/// anomalies) the composite is healthy.
#[cfg(feature = "health")]
#[tokio::test]
async fn doctor_wired_returns_healthy_info_report() {
    use unblock_engine::DiagnosticKind;

    let session = session().await;
    let report = session
        .doctor()
        .await
        .expect("doctor is wired (health-lite)");

    assert_eq!(
        report.kind,
        DiagnosticKind::Info,
        "doctor reuses DiagnosticKind::Info (F2 — no new model variant)"
    );
    let detail = |label: &str| {
        report
            .findings
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.detail.as_str())
    };
    assert_eq!(
        detail("health"),
        Some("healthy"),
        "clean workspace is healthy; findings: {:?}",
        report.findings
    );
    assert_eq!(detail("integrity"), Some("ok"), "clean integrity");
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.label == "integrity_problem"),
        "a clean DB has no integrity problems"
    );
}

/// T3.3: a pure file-state anomaly (a merge-conflict marker in the JSONL) FOLDS INTO the wired
/// `doctor()` report — surfaced as an advisory `jsonl_conflict_markers` finding, lifting the composite
/// `health` to `unsafe` (the F5 severity for conflict markers).
#[cfg(feature = "health")]
#[tokio::test]
async fn doctor_folds_in_a_file_state_anomaly() {
    use std::io::Write as _;

    let (session, tmp) = common::session_with_unblock_dir().await;
    let unblock_dir = tmp.path().join(".unblock");
    // A valid magic-header db file so DatabaseMissing/NotSqlite do NOT fire (the live storage is
    // in-memory; this is only the on-disk file-state the classifier inspects).
    let mut db = std::fs::File::create(unblock_dir.join("unblock.db")).expect("create db file");
    db.write_all(b"SQLite format 3\0").expect("magic");
    db.write_all(&[0_u8; 100]).expect("body");
    // A JSONL with an unresolved merge-conflict marker.
    std::fs::write(
        unblock_dir.join("issues.jsonl"),
        "<<<<<<< HEAD\n{\"id\":\"a\"}\n=======\n{\"id\":\"b\"}\n>>>>>>> branch\n",
    )
    .expect("write conflict jsonl");

    let report = session.doctor().await.expect("doctor is wired");
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.label == "jsonl_conflict_markers"),
        "the file-state anomaly must fold into the doctor report; findings: {:?}",
        report.findings
    );
    assert_eq!(
        report
            .findings
            .iter()
            .find(|f| f.label == "health")
            .map(|f| f.detail.as_str()),
        Some("unsafe"),
        "a conflict marker lifts the composite health to unsafe (F5)"
    );
}

/// **D46 — the two schema findings REPORT WHAT THEY CLAIM TO REPORT, proven where the two numbers
/// genuinely DIFFER (Verify gate, 2026-08-03).**
///
/// The shipped cli cell (`crates/unblock-cli/tests/migrate_doctor.rs`) asserts both findings on a
/// freshly-`init`ed workspace, where the observed stamp and `CURRENT_SCHEMA_VERSION` are BOTH the
/// current version — so it pins their PRESENCE and their VALUE, but it cannot tell the two SOURCES
/// apart: swapping `schema_version`'s source for `schema_expected`'s (or the reverse) leaves it
/// green. Both source mutants were MEASURED alive against it. This cell is the pin that makes the
/// pair mean something: it drives the two apart and asserts each against its own source.
///
/// The state is built the only way it can be — a MIGRATED file-backed workspace whose stamp is then
/// reset to the BASELINE through a raw libsql open. It is deliberately NOT reachable through the
/// config facade (which migrates on open, closing the gap before anything can observe it), which is
/// why this cell owns a `Session` built directly over `LibsqlStorage` rather than over the facade.
///
/// MUTANT KILLED: sourcing `schema_version` from `unblock_storage::CURRENT_SCHEMA_VERSION` (the
/// observed-stamp finding then reports 2 on a database stamped 1 — a `doctor` that cannot see the
/// very drift D46 exists to expose, while every shipped cell stays green).
///
/// MUTANT KILLED: sourcing `schema_expected` from `storage.schema_version()` (the two findings
/// collapse onto the on-disk value, so `doctor` reports the stale stamp TWICE and can never
/// contradict itself — which is precisely the false green clause (4) forbids).
#[cfg(feature = "health")]
#[tokio::test]
async fn doctor_schema_findings_report_the_stamp_and_the_build_version_separately() {
    use std::sync::Arc;
    use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage};

    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace_dir = tmp.path().to_path_buf();
    let unblock_dir = workspace_dir.join(".unblock");
    std::fs::create_dir_all(&unblock_dir).expect("create .unblock");
    let db_path = unblock_dir.join("unblock.db");

    // 1. A genuinely migrated file-backed workspace (stamp == CURRENT, current shape).
    {
        let storage = LibsqlStorage::open_local(&db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local");
        storage.migrate().await.expect("migrate");
        assert_eq!(
            storage.schema_version().await.expect("schema_version"),
            unblock_storage::CURRENT_SCHEMA_VERSION,
            "the fixture starts AT the current version, so the reset below is the only difference"
        );
    }

    // 2. Re-stamp it to the BASELINE out of band — the drift `doctor` must be able to see. The
    //    shape is untouched, so this is a stamp-only disagreement: exactly the observable the two
    //    findings exist to surface.
    {
        let database = libsql::Builder::new_local(&db_path)
            .build()
            .await
            .expect("raw open");
        let conn = database.connect().expect("connect");
        conn.query("PRAGMA user_version = 1", ())
            .await
            .expect("re-stamp to the baseline");
    }

    // 3. `Session::open` does NOT migrate (the config facade does, and this cell deliberately
    //    bypasses it), so the drifted stamp survives into `doctor()`.
    let storage = LibsqlStorage::open_local(&db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("reopen_local");
    let storage: Arc<dyn Storage> = Arc::new(storage);
    let session = common::session_over_in_dir(
        storage,
        SessionConfig::default(),
        workspace_dir,
        unblock_dir,
    )
    .await;

    let report = session.doctor().await.expect("doctor is wired");
    let detail = |label: &str| {
        report
            .findings
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.detail.as_str())
    };
    assert_eq!(
        detail("schema_version"),
        Some("1"),
        "the OBSERVED stamp is read off the database, not taken from this build; findings: {:?}",
        report.findings
    );
    assert_eq!(
        detail("schema_expected"),
        Some(unblock_storage::CURRENT_SCHEMA_VERSION.to_string().as_str()),
        "the EXPECTED version comes from this build's constant, not from the database; findings: \
         {:?}",
        report.findings
    );
    assert_ne!(
        detail("schema_version"),
        detail("schema_expected"),
        "the whole point: on a drifted database the two findings must DISAGREE — a pair that cannot \
         disagree cannot stop `doctor` printing `healthy` beside a contradicting number"
    );
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
    use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};
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
        source: WorkspaceSource::WalkUp,
        schema_version_before_migrate: 0,
    }
}
