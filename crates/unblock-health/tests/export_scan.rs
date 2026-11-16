//! Criterion (j), scanner half: `scan_export` maps `unblock_sync::scan_conflict_markers` into
//! `ExportFacts`, and both export classes plan `regenerate_export` online.

use std::path::PathBuf;
use std::time::SystemTime;

use tempfile::TempDir;
use unblock_health::{
    AdvisoryFacts, AnomalyClass, DbFileFact, ExportScan, HealthLevel, PlannedAction, PreOpenFacts,
    RepairPath, classify, scan_export,
};

fn write(dir: &TempDir, body: &[u8]) -> PathBuf {
    let path = dir.path().join("issues.jsonl");
    std::fs::write(&path, body).unwrap();
    path
}

fn classify_export(scan: &unblock_health::ExportFacts) -> unblock_health::WorkspaceClassification {
    let pre = PreOpenFacts {
        db: DbFileFact::SqliteHeader,
        lost: false,
        wal_len: None,
        shm_present: false,
        journal_present: false,
        write_lock_held: false,
        recovery_entries: Vec::new(),
    };
    classify(
        &pre,
        None,
        scan,
        &AdvisoryFacts {
            now_utc: SystemTime::now(),
        },
    )
}

#[test]
fn no_configured_export_is_absent() {
    let facts = scan_export(None);
    assert_eq!(facts.path, None);
    assert_eq!(facts.scan, ExportScan::Absent);
    assert!(classify_export(&facts).anomalies.is_empty());
}

#[test]
fn a_missing_file_is_absent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("issues.jsonl");
    let facts = scan_export(Some(&path));
    assert_eq!(facts.path, Some(path));
    assert_eq!(facts.scan, ExportScan::Absent);
}

#[test]
fn a_clean_export_is_clean() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, b"{\"id\":\"ub-1\"}\n{\"id\":\"ub-2\"}\n");
    let facts = scan_export(Some(&path));
    assert_eq!(facts.scan, ExportScan::Clean);
    assert_eq!(classify_export(&facts).level, HealthLevel::Healthy);
}

#[test]
fn conflict_markers_are_recoverable_with_regenerate_export() {
    let dir = TempDir::new().unwrap();
    let path = write(
        &dir,
        b"{\"id\":\"ub-1\"}\n<<<<<<< HEAD\n{\"id\":\"ub-2\"}\n=======\n{\"id\":\"ub-3\"}\n>>>>>>> theirs\n",
    );
    let facts = scan_export(Some(&path));
    assert_eq!(facts.scan, ExportScan::ConflictMarkers { first_line: 2 });
    let class = classify_export(&facts);
    assert_eq!(class.level, HealthLevel::Recoverable);
    assert_eq!(
        class.anomalies,
        vec![AnomalyClass::JsonlConflictMarkers { first_line: 2 }]
    );
    assert_eq!(class.planned[0].action, PlannedAction::RegenerateExport);
    assert_eq!(class.planned[0].path, RepairPath::Online);
}

#[test]
fn an_over_long_line_is_unscannable_with_the_same_repair() {
    let dir = TempDir::new().unwrap();
    // One line past `unblock-sync`'s 4 MiB per-line cap, with no newline.
    let body = vec![b'x'; 4 * 1024 * 1024 + 2];
    let path = write(&dir, &body);
    let facts = scan_export(Some(&path));
    assert!(
        matches!(facts.scan, ExportScan::Unscannable { .. }),
        "{:?}",
        facts.scan
    );
    let class = classify_export(&facts);
    assert_eq!(class.level, HealthLevel::Recoverable);
    assert_eq!(class.anomalies[0].code(), "jsonl_unscannable");
    assert_eq!(class.planned[0].action, PlannedAction::RegenerateExport);
    assert_eq!(class.planned[0].path, RepairPath::Online);
}

#[test]
fn a_directory_in_place_of_the_export_is_unscannable() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("issues.jsonl");
    std::fs::create_dir(&path).unwrap();
    let facts = scan_export(Some(&path));
    assert!(
        matches!(facts.scan, ExportScan::Unscannable { .. }),
        "{:?}",
        facts.scan
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_export_is_unscannable() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = TempDir::new().unwrap();
    let path = write(&dir, b"{}\n");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let facts = scan_export(Some(&path));
    if std::fs::File::open(&path).is_ok() {
        // A privileged user reads through mode 000, so the file scans clean. The I/O-error route
        // to `Unscannable` stays covered for root by
        // `a_directory_in_place_of_the_export_is_unscannable`.
        assert!(matches!(facts.scan, ExportScan::Clean), "{:?}", facts.scan);
        return;
    }
    assert!(
        matches!(facts.scan, ExportScan::Unscannable { .. }),
        "{:?}",
        facts.scan
    );
}
