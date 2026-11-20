//! Criterion (j), scanner half: `scan_export` maps `unblock_sync::scan_conflict_markers` into
//! `ExportFacts`, and both export classes plan `regenerate_export` online. PRD §4 D5 amendment
//! (`ub-c15`): an export that is the session database or a sidecar is `ExportScan::SessionFile`,
//! never opened (the fd witness), and reported as the advisory `export_is_session_file`.

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "../../unblock-sync/tests/common/fd_witness.rs"]
mod fd_witness;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tempfile::TempDir;
use unblock_health::{
    Advisory, AdvisoryFacts, AnomalyClass, DbFileFact, ExportScan, HealthLevel, PlannedAction,
    PreOpenFacts, RepairPath, classify, scan_export,
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
    let facts = scan_export(None, &unblock_sync::SessionFiles::none());
    assert_eq!(facts.path, None);
    assert_eq!(facts.scan, ExportScan::Absent);
    assert!(classify_export(&facts).anomalies.is_empty());
}

#[test]
fn a_missing_file_is_absent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("issues.jsonl");
    let facts = scan_export(Some(&path), &unblock_sync::SessionFiles::none());
    assert_eq!(facts.path, Some(path));
    assert_eq!(facts.scan, ExportScan::Absent);
}

#[test]
fn a_clean_export_is_clean() {
    let dir = TempDir::new().unwrap();
    let path = write(&dir, b"{\"id\":\"ub-1\"}\n{\"id\":\"ub-2\"}\n");
    let facts = scan_export(Some(&path), &unblock_sync::SessionFiles::none());
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
    let facts = scan_export(Some(&path), &unblock_sync::SessionFiles::none());
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
    let facts = scan_export(Some(&path), &unblock_sync::SessionFiles::none());
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
    let facts = scan_export(Some(&path), &unblock_sync::SessionFiles::none());
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
    let facts = scan_export(Some(&path), &unblock_sync::SessionFiles::none());
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

// ---- PRD §4 D5 amendment (`ub-c15`): an export that is a session file is skipped and reported ----

/// The conflict-marker bytes every session file carries here, so a scan that READ one would report
/// `ConflictMarkers` instead of `SessionFile`.
const MARKED: &[u8] = b"{\"id\":\"ub-1\"}\n<<<<<<< HEAD\n{\"id\":\"ub-2\"}\n";

/// `<dir>/unblock.db` plus its three sidecars, each a plain file carrying [`MARKED`]; returns the db
/// and the four session files (db first).
fn session_files_in(dir: &Path) -> (PathBuf, Vec<PathBuf>) {
    let db = dir.join("unblock.db");
    std::fs::write(&db, MARKED).unwrap();
    let mut all = vec![db.clone()];
    for suffix in unblock_sync::SIDECAR_SUFFIXES {
        let path = unblock_sync::sidecar(&db, suffix);
        std::fs::write(&path, MARKED).unwrap();
        all.push(path);
    }
    (db, all)
}

/// Scan `export` with the session of `db` and assert the skip: `SessionFile`, its path the export
/// path, and no descriptor left on `watched` (fd witness +0, after the positive control).
fn assert_skipped(db: &Path, export: &Path, watched: &Path) {
    let session = unblock_sync::SessionFiles::of_database(db);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let fds = {
        fd_witness::assert_witness_sees(watched);
        fd_witness::descriptors_on(watched)
    };
    let facts = scan_export(Some(export), &session);
    assert_eq!(
        facts.scan,
        ExportScan::SessionFile,
        "{} must be skipped as a session file",
        export.display()
    );
    assert_eq!(facts.path.as_deref(), Some(export));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert_eq!(
        fd_witness::descriptors_on(watched),
        fds,
        "the scan left a descriptor on {} (a stat-refused export is never opened)",
        watched.display()
    );
}

/// (H1) Four shapes × four targets: the export path IS the session file (its own name), a hard link
/// to it, a symlink leaf to it, and a hard link to a symlinked database's TARGET files. Each is
/// skipped as `ExportScan::SessionFile`, never `ConflictMarkers` or `Unscannable`, and never opened.
#[test]
fn a_session_file_export_is_skipped_and_reported() {
    // Own name: the export path is the database or a sidecar itself.
    let dir = TempDir::new().unwrap();
    let (db, all) = session_files_in(dir.path());
    for target in &all {
        assert_skipped(&db, target, target);
    }

    #[cfg(unix)]
    {
        // A hard link and a symlink leaf named like the export.
        let dir = TempDir::new().unwrap();
        let (db, all) = session_files_in(dir.path());
        let alias = dir.path().join("issues.jsonl");
        for target in &all {
            std::fs::hard_link(target, &alias).unwrap();
            assert_skipped(&db, &alias, target);
            std::fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(target, &alias).unwrap();
            assert_skipped(&db, &alias, target);
            std::fs::remove_file(&alias).unwrap();
        }

        // A symlinked database: SQLite keeps the sidecars beside the TARGET.
        let dir = TempDir::new().unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir(&store).unwrap();
        let (real, all) = session_files_in(&store);
        let link = dir.path().join("unblock.db");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let alias = dir.path().join("issues.jsonl");
        for target in &all {
            std::fs::hard_link(target, &alias).unwrap();
            assert_skipped(&link, &alias, target);
            std::fs::remove_file(&alias).unwrap();
        }
    }
}

/// (H2) CONTROL: a distinct export carrying markers is still scanned, with the session set given.
#[test]
fn a_distinct_export_with_markers_is_still_scanned() {
    let dir = TempDir::new().unwrap();
    let (db, _) = session_files_in(dir.path());
    let path = write(&dir, MARKED);
    let facts = scan_export(Some(&path), &unblock_sync::SessionFiles::of_database(&db));
    assert_eq!(facts.scan, ExportScan::ConflictMarkers { first_line: 2 });
}

/// (H3) A skipped session-file export is the advisory `export_is_session_file`: no anomaly, no
/// planned `regenerate_export` (so recover never copies or regenerates it), and the level stays.
#[test]
fn a_skipped_session_file_export_plans_no_regeneration() {
    let dir = TempDir::new().unwrap();
    let (db, _) = session_files_in(dir.path());
    let facts = scan_export(Some(&db), &unblock_sync::SessionFiles::of_database(&db));
    assert_eq!(facts.scan, ExportScan::SessionFile);
    let class = classify_export(&facts);
    assert_eq!(class.level, HealthLevel::Healthy);
    assert!(class.anomalies.is_empty(), "{:?}", class.anomalies);
    assert!(
        class
            .planned
            .iter()
            .all(|planned| planned.action != PlannedAction::RegenerateExport),
        "{:?}",
        class.planned
    );
    assert_eq!(class.advisories, vec![Advisory::ExportIsSessionFile]);
    assert_eq!(class.advisories[0].code(), "export_is_session_file");
}
