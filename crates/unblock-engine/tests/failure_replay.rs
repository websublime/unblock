//! NFR-5 gate (a) — failure-replay against the LITE doctor posture (T3.4/D30).
//!
//! Corrupted-workspace fixtures classify to the v1 `FileAnomaly` severities via
//! `unblock_health::classify_file_state`, and a healthy `Session::integrity_check()` is clean. This
//! is the v1 LITE posture (`classify_file_state` + `integrity_check`) — NOT the v1.1
//! audit-record/full-taxonomy posture (`WorkspaceClassification`/`--repair`/`.recovery/`), which is
//! out of T3.4 scope.

mod common;
use common::session;

use unblock_health::{FileAnomaly, HealthLevel, WAL_SUFFIX, classify_file_state, sidecar};

/// Write the 16-byte `SQLite` magic header + padding — a valid-looking db file for the classifier.
fn write_sqlite_db(path: &std::path::Path) {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path).expect("create db");
    file.write_all(b"SQLite format 3\0").expect("magic");
    file.write_all(&[0_u8; 512]).expect("body");
}

#[test]
fn conflict_marker_jsonl_replays_as_unsafe() {
    // A merge conflict left in the JSONL export is the ONLY v1 `Unsafe` file-state anomaly.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unblock.db");
    let jsonl = dir.path().join("issues.jsonl");
    write_sqlite_db(&db);
    std::fs::write(&jsonl, "{\"id\":\"ub-1\"}\n<<<<<<< HEAD\n=======\n").unwrap();

    let anomalies = classify_file_state(&db, Some(&jsonl));
    assert!(
        anomalies.contains(&FileAnomaly::JsonlConflictMarkers),
        "expected JsonlConflictMarkers, got {anomalies:?}"
    );
    assert_eq!(
        FileAnomaly::JsonlConflictMarkers.severity(),
        HealthLevel::Unsafe
    );
}

#[test]
fn non_sqlite_db_replays_as_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unblock.db");
    std::fs::write(&db, b"this is not a sqlite database file at all\n").unwrap();

    let anomalies = classify_file_state(&db, None);
    assert!(
        anomalies.contains(&FileAnomaly::DatabaseNotSqlite),
        "expected DatabaseNotSqlite, got {anomalies:?}"
    );
    assert_eq!(
        FileAnomaly::DatabaseNotSqlite.severity(),
        HealthLevel::Recoverable
    );
}

#[test]
fn truncated_wal_replays_as_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unblock.db");
    write_sqlite_db(&db);
    // A NON-EMPTY `-wal` smaller than its 32-byte header = a torn write (crash signature).
    std::fs::write(sidecar(&db, WAL_SUFFIX), [0_u8; 16]).unwrap();

    let anomalies = classify_file_state(&db, None);
    assert!(
        anomalies.contains(&FileAnomaly::TruncatedWal),
        "expected TruncatedWal, got {anomalies:?}"
    );
    assert_eq!(
        FileAnomaly::TruncatedWal.severity(),
        HealthLevel::Recoverable
    );
}

#[test]
fn database_missing_with_surviving_jsonl_replays_as_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("unblock.db"); // absent
    let jsonl = dir.path().join("issues.jsonl");
    std::fs::write(&jsonl, "{\"id\":\"ub-1\"}\n").unwrap();

    let anomalies = classify_file_state(&db, Some(&jsonl));
    assert!(
        anomalies.contains(&FileAnomaly::DatabaseMissing),
        "expected DatabaseMissing, got {anomalies:?}"
    );
    assert_eq!(
        FileAnomaly::DatabaseMissing.severity(),
        HealthLevel::Recoverable
    );
}

#[tokio::test]
async fn healthy_session_integrity_check_is_clean() {
    // The db-integrity half of the LITE doctor posture: a healthy real-libsql session's
    // `integrity_check()` returns no problems (non-vacuous — a corrupt db would return rows).
    let session = session().await;
    let problems = session.integrity_check().await.expect("integrity_check");
    assert!(
        problems.is_empty(),
        "a healthy workspace's integrity_check must be clean, got {problems:?}"
    );
}
