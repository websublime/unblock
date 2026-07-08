//! The v1 health "contract suite" slice: [`run_doctor`] over canned integrity-row vectors crossed
//! with file-state fixtures. There is NO mock `Storage` (F3/D29 — `run_doctor` is storage-free and
//! non-async; the ENGINE, not health, runs `integrity_check`), so the contract is exercised by
//! passing the `Vec<String>` rows directly. Plus a proptest: the composite worst level is the `max`
//! over any set of severities.

use std::io::Write as _;
use std::path::PathBuf;

use proptest::prelude::*;
use tempfile::TempDir;
use unblock_health::{HealthLevel, WorkspacePaths, run_doctor};

/// A healthy on-disk workspace + its [`WorkspacePaths`].
fn healthy_paths() -> (TempDir, WorkspacePaths) {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("unblock.db");
    let jsonl = dir.path().join("issues.jsonl");
    let mut f = std::fs::File::create(&db).unwrap();
    f.write_all(b"SQLite format 3\0").unwrap();
    f.write_all(&[0_u8; 100]).unwrap();
    std::fs::write(&jsonl, "{\"id\":\"ub-1\"}\n").unwrap();
    let paths = WorkspacePaths {
        db,
        jsonl: Some(jsonl),
        recovery_dir: dir.path().join(".recovery"),
    };
    (dir, paths)
}

#[test]
fn empty_rows_and_clean_files_is_healthy() {
    let (_dir, paths) = healthy_paths();
    let report = run_doctor(&[], &paths).unwrap();
    assert!(report.integrity_ok);
    assert!(report.file_state.is_empty());
    assert_eq!(report.summary.worst, HealthLevel::Healthy);
    assert_eq!(report.summary.anomaly_count, 0);
}

#[test]
fn corrupt_rows_with_clean_files_is_recoverable() {
    let (_dir, paths) = healthy_paths();
    let rows = vec![
        "*** in database main ***".to_string(),
        "page 5 is never used".to_string(),
    ];
    let report = run_doctor(&rows, &paths).unwrap();
    assert!(!report.integrity_ok);
    assert_eq!(report.integrity_rows, rows);
    assert_eq!(report.summary.worst, HealthLevel::Recoverable);
}

#[test]
fn clean_rows_with_conflict_markers_is_unsafe() {
    let (_dir, paths) = healthy_paths();
    std::fs::write(
        paths.jsonl.as_ref().unwrap(),
        "<<<<<<< HEAD\n{\"id\":\"a\"}\n=======\n{\"id\":\"b\"}\n>>>>>>> branch\n",
    )
    .unwrap();
    let report = run_doctor(&[], &paths).unwrap();
    assert!(report.integrity_ok);
    assert_eq!(report.summary.worst, HealthLevel::Unsafe);
}

#[test]
fn doctor_report_json_shape_is_a_subset_of_diagnostic_findings() {
    // The report JSON must be stable (NFR-14) and carry integrity_ok / integrity_rows / file_state /
    // summary — the fields the engine folds into DiagnosticFinding rows.
    let (_dir, paths) = healthy_paths();
    let rows = vec!["page 3 is never used".to_string()];
    let report = run_doctor(&rows, &paths).unwrap();
    let value = serde_json::to_value(&report).unwrap();
    assert!(value.get("integrity_ok").is_some());
    assert!(value.get("integrity_rows").is_some());
    assert!(value.get("file_state").is_some());
    assert!(value.get("summary").is_some());
    assert_eq!(value["summary"]["worst"], "recoverable");
}

/// A canned `WorkspacePaths` whose db path does not exist (no filesystem effects needed for the
/// integrity-only assertions).
fn nonexistent_paths() -> WorkspacePaths {
    WorkspacePaths {
        db: PathBuf::from("/nonexistent/unblock.db"),
        jsonl: None,
        recovery_dir: PathBuf::from("/nonexistent/.recovery"),
    }
}

#[test]
fn integrity_ok_tracks_row_emptiness_exactly() {
    // The critical F3 fix: integrity_ok == rows.is_empty() (NOT `== ["ok"]`, which flagged EVERY
    // healthy DB). With db missing + jsonl None, no file-state anomaly can fire, so integrity is the
    // sole signal.
    let paths = nonexistent_paths();
    assert!(run_doctor(&[], &paths).unwrap().integrity_ok);
    assert!(
        !run_doctor(&["boom".to_string()], &paths)
            .unwrap()
            .integrity_ok
    );
}

proptest! {
    /// The composite `worst` equals the `max` over the integrity-derived severity and every
    /// file-state severity — verified here at the HealthLevel level: `max` of any set is its worst.
    #[test]
    fn health_level_composite_equals_max(levels in proptest::collection::vec(0_u8..4, 1..16)) {
        let to_level = |n: u8| match n {
            0 => HealthLevel::Healthy,
            1 => HealthLevel::Drifted,
            2 => HealthLevel::Recoverable,
            _ => HealthLevel::Unsafe,
        };
        let mapped: Vec<HealthLevel> = levels.iter().copied().map(to_level).collect();

        let folded = mapped.iter().copied().fold(HealthLevel::Healthy, HealthLevel::max);
        let via_max = mapped.iter().copied().max().unwrap_or(HealthLevel::Healthy);
        // The manual worst = the level with the highest severity rank (== the highest u8 code).
        let manual = to_level(levels.iter().copied().max().unwrap_or(0));

        prop_assert_eq!(folded, via_max);
        prop_assert_eq!(folded, manual);
    }
}
