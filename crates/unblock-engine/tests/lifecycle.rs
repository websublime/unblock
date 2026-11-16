//! Lifecycle integration tests: open / reopen / import-on-open seam / shutdown / doctor / recover.

mod common;

use common::session;
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

/// D58: `recover()` on a healthy workspace returns no action row and writes nothing. The
/// file-backed recover cells (evidence, drift, export, failure, lock posture) live in `health.rs`.
#[cfg(feature = "health")]
#[tokio::test]
async fn recover_on_a_healthy_session_returns_no_action_rows_and_writes_nothing() {
    let session = session().await;
    let issue = common::issue("ub-0001", unblock_model::Priority::MEDIUM, 1000);
    session.create(&issue).await.expect("create");
    let before = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");

    let report = session.recover().await.expect("recover");
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.label == "repair" || f.label == "evidence"),
        "nothing to repair: {:?}",
        report.findings
    );
    assert!(!session.workspace_dir().join(".unblock/.recovery").exists());

    let after = session
        .list(&unblock_model::ListFilters::default())
        .await
        .expect("list");
    assert_eq!(before, after);
}

/// D58 clause (8): a build without the `health` feature keeps `recover()`'s signature and returns
/// the typed `FeatureNotWired { feature: "health" }`.
#[cfg(not(feature = "health"))]
#[tokio::test]
async fn no_default_features_recover_returns_feature_not_wired() {
    let session = session().await;
    let err = session
        .recover()
        .await
        .expect_err("health is not compiled in");
    assert!(matches!(
        err,
        EngineError::FeatureNotWired { feature: "health" }
    ));
}

/// D58: `doctor()` returns a typed `HealthReport` whose report reuses `DiagnosticKind::Info` (F2).
/// A fresh workspace is healthy, its first row is the `health` level row, and no integrity row
/// exists any more (the rows come from `health_facts`).
#[cfg(feature = "health")]
#[tokio::test]
async fn doctor_returns_a_healthy_info_report_on_a_fresh_workspace() {
    use unblock_engine::{DiagnosticKind, HealthLevel};

    let session = session().await;
    let health = session
        .doctor(&common::clean_preopen())
        .await
        .expect("doctor");

    assert_eq!(health.level, HealthLevel::Healthy);
    assert!(health.planned.is_empty());
    assert_eq!(health.report.kind, DiagnosticKind::Info);
    assert_eq!(health.report.findings[0].label, "health");
    assert_eq!(health.report.findings[0].detail, "healthy");
    assert!(
        !health
            .report
            .findings
            .iter()
            .any(|f| f.label.starts_with("integrity")),
        "{:?}",
        health.report.findings
    );
}

/// D58: a merge-conflict marker in the export is `jsonl_conflict_markers`/`Recoverable`, with its
/// `regenerate_export` repair planned online.
#[cfg(feature = "health")]
#[tokio::test]
async fn doctor_reports_conflict_markers_in_the_export_as_recoverable() {
    use unblock_engine::HealthLevel;

    let (session, tmp) = common::session_with_unblock_dir().await;
    std::fs::write(
        tmp.path().join(".unblock").join("issues.jsonl"),
        "<<<<<<< HEAD\n{\"id\":\"a\"}\n=======\n{\"id\":\"b\"}\n>>>>>>> branch\n",
    )
    .expect("write conflict jsonl");

    let health = session
        .doctor(&common::clean_preopen())
        .await
        .expect("doctor");
    assert_eq!(health.level, HealthLevel::Recoverable);
    let labels: Vec<&str> = health
        .report
        .findings
        .iter()
        .map(|f| f.label.as_str())
        .collect();
    assert_eq!(
        &labels[..3],
        ["health", "jsonl_conflict_markers", "repair_planned"]
    );
    assert_eq!(
        health.report.findings[2].detail,
        "regenerate_export (online)"
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
        let conn = common::raw_connection(&db_path).await;
        conn.query("PRAGMA user_version = 1", ())
            .await
            .expect("re-stamp to the baseline");
    }

    // 3. Gather the pre-open facts while no connection is open, then open read-only (no migrate),
    //    so the drifted stamp survives into `doctor()`.
    let facts = unblock_engine::preopen::gather_preopen(&unblock_config::ConfigPaths {
        unblock_dir: unblock_dir.clone(),
        db_path: db_path.clone(),
        jsonl_path: unblock_dir.join("issues.jsonl"),
    })
    .expect("gather_preopen");
    let storage = LibsqlStorage::open_local_read_only(&db_path)
        .await
        .expect("open read-only");
    let storage: Arc<dyn Storage> = Arc::new(storage);
    let session = common::session_over_in_dir(
        storage,
        SessionConfig::default(),
        workspace_dir,
        unblock_dir,
    )
    .await;

    let report = session.doctor(&facts).await.expect("doctor").report;
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

/// Build a synthetic `WorkspaceContext` over `storage` (mirrors the harness, for the explicit-open
/// test above).
fn make_ctx(
    storage: std::sync::Arc<dyn unblock_storage::Storage>,
) -> unblock_config::WorkspaceContext {
    use std::path::PathBuf;
    use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};
    let workspace_dir = PathBuf::from("/tmp/unblock-test-ws");
    let unblock_dir = workspace_dir.join(".unblock");
    let mut config = ResolvedConfig::default();
    config.runtime.actor = "tester".to_string();
    config.runtime.sources.actor = unblock_model::ConfigSource::OsUser;
    let paths = ConfigPaths {
        db_path: unblock_dir.join(&config.startup.db_filename),
        jsonl_path: unblock_dir.join(&config.startup.jsonl_filename),
        unblock_dir,
    };
    WorkspaceContext {
        storage,
        workspace_dir,
        config,
        reloader: unblock_config::ConfigReloader::new(
            unblock_config::CliOverrides::default(),
            unblock_config::EnvOverrides::default(),
            Some("tester".to_string()),
            None,
            None,
        ),
        paths,
        source: WorkspaceSource::WalkUp,
        schema_version_before_migrate: 0,
    }
}
