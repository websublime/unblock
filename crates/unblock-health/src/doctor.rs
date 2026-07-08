//! v1-lite doctor aggregation (FR-16 lite) — **pure and non-async** (F3/D29).
//!
//! [`run_doctor`] is **storage-free**: the engine calls `Session::integrity_check()` and passes the
//! resulting `Vec<String>` rows in; `run_doctor` runs [`classify_file_state`] over the workspace
//! paths and folds both signals into a [`DoctorReport`]. It holds no `Storage` handle and never names
//! a libsql type (NFR-15).

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::HealthError;
use crate::file_state::{FileAnomaly, classify_file_state};
use crate::level::HealthLevel;
use crate::paths::WorkspacePaths;

/// The aggregated v1-lite doctor result (flows into the engine's `DiagnosticReport` at the boundary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct DoctorReport {
    /// Whether `PRAGMA integrity_check` reported no problems (the rows were empty).
    pub integrity_ok: bool,
    /// The raw integrity problem rows (empty when `integrity_ok`).
    pub integrity_rows: Vec<String>,
    /// The pure file-state anomalies, in deterministic order.
    pub file_state: Vec<FileAnomaly>,
    /// The compact summary (composite severity + counts).
    pub summary: HealthSummary,
}

/// A compact summary of a [`DoctorReport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct HealthSummary {
    /// Whether the integrity check was clean.
    pub integrity_ok: bool,
    /// The number of file-state anomalies detected.
    pub anomaly_count: usize,
    /// The composite worst health level (`max` of the file-state severities and the
    /// integrity-derived severity).
    pub worst: HealthLevel,
}

/// Aggregate the integrity rows and the pure file-state classification into a [`DoctorReport`].
///
/// **Storage-free, non-async, pure** (F3/D29): the engine supplies `integrity_rows` (from
/// `Session::integrity_check()`); this function does no DB work. `integrity_ok =
/// integrity_rows.is_empty()` — the shipped `Storage::integrity_check()` normalizes libsql's `"ok"`
/// sentinel away and returns an EMPTY vec on a healthy DB, so a non-empty vec means integrity
/// problems. The composite `worst` is the `max` of the file-state severities and the
/// integrity-derived severity (`Healthy` when clean, else `Recoverable`).
///
/// # Errors
/// Infallible in v1-lite — always returns `Ok`. The `Result` matches the v1.1 contract (which does
/// evidence-dir I/O and report serialization); keeping it now means the v1.1 body is a purely
/// additive change with no signature churn at the engine boundary.
pub fn run_doctor(
    integrity_rows: &[String],
    paths: &WorkspacePaths,
) -> Result<DoctorReport, HealthError> {
    let file_state = classify_file_state(&paths.db, paths.jsonl.as_deref());
    let integrity_ok = integrity_rows.is_empty();

    let integrity_severity = if integrity_ok {
        HealthLevel::Healthy
    } else {
        HealthLevel::Recoverable
    };
    let worst = file_state
        .iter()
        .map(FileAnomaly::severity)
        .fold(integrity_severity, HealthLevel::max);

    let summary = HealthSummary {
        integrity_ok,
        anomaly_count: file_state.len(),
        worst,
    };

    Ok(DoctorReport {
        integrity_ok,
        integrity_rows: integrity_rows.to_vec(),
        file_state,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::{HealthSummary, run_doctor};
    use crate::level::HealthLevel;
    use crate::paths::WorkspacePaths;
    use std::io::Write as _;
    use tempfile::TempDir;

    /// A healthy on-disk workspace + its [`WorkspacePaths`] (db is valid `SQLite`, jsonl present, clean).
    fn healthy_workspace() -> (TempDir, WorkspacePaths) {
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
    fn clean_integrity_and_clean_files_is_healthy() {
        let (_dir, paths) = healthy_workspace();
        let report = run_doctor(&[], &paths).unwrap();
        assert!(report.integrity_ok);
        assert!(report.file_state.is_empty());
        assert_eq!(
            report.summary,
            HealthSummary {
                integrity_ok: true,
                anomaly_count: 0,
                worst: HealthLevel::Healthy,
            }
        );
    }

    #[test]
    fn nonempty_integrity_rows_are_recoverable() {
        let (_dir, paths) = healthy_workspace();
        let rows = vec!["*** in database main *** page 3 is never used".to_string()];
        let report = run_doctor(&rows, &paths).unwrap();
        assert!(!report.integrity_ok);
        assert_eq!(report.integrity_rows, rows);
        assert_eq!(report.summary.worst, HealthLevel::Recoverable);
    }

    #[test]
    fn a_conflict_marker_makes_the_worst_unsafe_even_with_clean_integrity() {
        let (_dir, paths) = healthy_workspace();
        // Overwrite the jsonl with a merge-conflict marker → JsonlConflictMarkers (Unsafe).
        std::fs::write(
            paths.jsonl.as_ref().unwrap(),
            "<<<<<<< HEAD\n{\"id\":\"a\"}\n=======\n{\"id\":\"b\"}\n>>>>>>> branch\n",
        )
        .unwrap();
        let report = run_doctor(&[], &paths).unwrap();
        assert!(report.integrity_ok, "integrity is clean");
        assert_eq!(report.summary.worst, HealthLevel::Unsafe);
        assert_eq!(report.summary.anomaly_count, 1);
    }

    #[test]
    fn worst_is_the_max_of_integrity_and_file_state() {
        // Corrupt integrity (Recoverable) + a conflict marker (Unsafe) → Unsafe.
        let (_dir, paths) = healthy_workspace();
        std::fs::write(
            paths.jsonl.as_ref().unwrap(),
            "<<<<<<< HEAD\n{\"id\":\"a\"}\n",
        )
        .unwrap();
        let rows = vec!["page 5 is never used".to_string()];
        let report = run_doctor(&rows, &paths).unwrap();
        assert_eq!(report.summary.worst, HealthLevel::Unsafe);
    }

    #[test]
    fn missing_db_with_jsonl_is_recoverable() {
        let (_dir, paths) = healthy_workspace();
        std::fs::remove_file(&paths.db).unwrap();
        let report = run_doctor(&[], &paths).unwrap();
        assert_eq!(report.summary.worst, HealthLevel::Recoverable);
        assert!(
            report
                .file_state
                .iter()
                .any(|a| a.code() == "database_missing")
        );
    }

    #[test]
    fn doctor_report_json_shape_is_pinned() {
        let (_dir, paths) = healthy_workspace();
        let rows = vec!["page 3 is never used".to_string()];
        let report = run_doctor(&rows, &paths).unwrap();
        insta::assert_json_snapshot!("doctor_report_corrupt_integrity", report);
    }
}
