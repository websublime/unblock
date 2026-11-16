//! NFR-5 gate (a) — failure replay against the D58 health classification.
//!
//! Corrupted-workspace fixtures are gathered with `preopen::gather_preopen` (`std::fs` only, no
//! connection) and classified by the pure `unblock_health::classify`, through
//! `preopen::classify_preopen` for the file classes and with `scan_export` for the export. A
//! healthy `Session::integrity_check()` is clean.
#![cfg(feature = "health")]

mod common;
use common::session;

use std::path::Path;

use unblock_config::ConfigPaths;
use unblock_engine::HealthLevel;
use unblock_engine::preopen::{classify_preopen, gather_preopen};
use unblock_health::{AdvisoryFacts, WAL_SUFFIX, classify, scan_export, sidecar};

/// A `.unblock/` in `dir` with its default database and export paths.
fn paths(dir: &Path) -> ConfigPaths {
    let unblock_dir = dir.join(".unblock");
    std::fs::create_dir_all(&unblock_dir).expect("create .unblock");
    ConfigPaths {
        db_path: unblock_dir.join("unblock.db"),
        jsonl_path: unblock_dir.join("issues.jsonl"),
        unblock_dir,
    }
}

/// Write the 16-byte `SQLite` magic header + padding — a valid-looking db file for the classifier.
fn write_sqlite_db(path: &Path) {
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend_from_slice(&[0_u8; 512]);
    std::fs::write(path, bytes).expect("write db");
}

/// The codes of the anomaly rows of a pre-open report (the rows between `health` and the first
/// `repair_planned`).
fn anomaly_codes(paths: &ConfigPaths) -> (HealthLevel, Vec<String>) {
    let report = classify_preopen(&gather_preopen(paths).expect("gather_preopen"));
    let codes = report
        .report
        .findings
        .iter()
        .skip(1)
        .take_while(|f| f.label != "repair_planned")
        .map(|f| f.label.clone())
        .collect();
    (report.level, codes)
}

#[test]
fn conflict_marker_jsonl_replays_as_recoverable() {
    // The export is never the source of truth, so the database regenerates it (D58 clause (1)).
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write_sqlite_db(&paths.db_path);
    std::fs::write(
        &paths.jsonl_path,
        "{\"id\":\"ub-1\"}\n<<<<<<< HEAD\n=======\n",
    )
    .unwrap();

    let facts = gather_preopen(&paths).expect("gather_preopen");
    let export = scan_export(Some(&paths.jsonl_path));
    let classification = classify(
        &facts,
        None,
        &export,
        &AdvisoryFacts {
            now_utc: std::time::SystemTime::now(),
        },
    );
    assert_eq!(classification.level, HealthLevel::Recoverable);
    assert!(
        classification
            .anomalies
            .iter()
            .any(|a| a.code() == "jsonl_conflict_markers"),
        "{:?}",
        classification.anomalies
    );
}

#[test]
fn non_sqlite_db_replays_as_unsafe() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    std::fs::write(
        &paths.db_path,
        b"this is not a sqlite database file at all\n",
    )
    .unwrap();

    let (level, codes) = anomaly_codes(&paths);
    assert_eq!(codes, ["database_not_sqlite"]);
    assert_eq!(level, HealthLevel::Unsafe);
}

#[test]
fn truncated_wal_replays_as_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write_sqlite_db(&paths.db_path);
    // A NON-EMPTY `-wal` smaller than its 32-byte header = a torn write (crash signature).
    std::fs::write(sidecar(&paths.db_path, WAL_SUFFIX), [0_u8; 16]).unwrap();

    let (level, codes) = anomaly_codes(&paths);
    assert_eq!(codes, ["truncated_wal"]);
    assert_eq!(level, HealthLevel::Recoverable);
}

#[test]
fn database_lost_beside_its_write_lock_replays_as_unsafe() {
    // The `.write.lock` marker records that a database existed, so its absence is a loss.
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    std::fs::write(paths.unblock_dir.join(".write.lock"), b"").unwrap();
    std::fs::write(&paths.jsonl_path, "{\"id\":\"ub-1\"}\n").unwrap();

    let (level, codes) = anomaly_codes(&paths);
    assert_eq!(codes, ["database_lost"]);
    assert_eq!(level, HealthLevel::Unsafe);
}

#[test]
fn database_absent_without_a_write_lock_replays_as_not_created() {
    // A fresh clone or worktree: no database and no marker is an advisory, not an anomaly.
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());

    let (level, codes) = anomaly_codes(&paths);
    assert_eq!(level, HealthLevel::Healthy);
    assert_eq!(codes, ["database_not_created"]);
    assert!(
        !paths.unblock_dir.join(".write.lock").exists(),
        "the probe never creates it"
    );
}

#[tokio::test]
async fn healthy_session_integrity_check_is_clean() {
    // A healthy real-libsql session's `integrity_check()` returns no problems (non-vacuous — a
    // corrupt db would return rows).
    let session = session().await;
    let problems = session.integrity_check().await.expect("integrity_check");
    assert!(
        problems.is_empty(),
        "a healthy workspace's integrity_check must be clean, got {problems:?}"
    );
}
