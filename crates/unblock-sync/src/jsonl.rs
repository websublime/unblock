//! Line-oriented JSONL parse/validate + the deterministic export serializer (FR-7/FR-8).
//!
//! **Serialize (export):** one [`Issue`] per line, timestamps canonicalized to `unblock_model::
//! fmt_ts_secs` (CF-TS/D-OQ-B) via a RECURSIVE `Value`-tree rewrite (MF-6 — covers the 7 top-level
//! timestamps AND nested `dependencies[].created_at` / `comments[].created_at`, which have no
//! `skip_serializing_if`). No trailing `\n` here (the atomic writer adds it).
//!
//! **Parse/validate (import preflight):** each non-empty line → `Issue` → `normalize` (recompute
//! `content_hash`, spine §1.8) → `IssueValidator::validate` → in-file duplicate-id detection. ALL
//! failures are collected; the orchestrator aborts with ZERO DB writes if any survive. Uses the same
//! bounded per-line read + fd-metadata size guard as [`crate::conflict`] (MF-3).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use serde_json::Value;
use unblock_model::{Issue, IssueValidator, fmt_ts_secs};

use crate::conflict::{MAX_IMPORT_FILE_BYTES, read_line_bounded};
use crate::error::SyncError;
use crate::path::PathReject;

/// The result of validating every record in an import file (records carried so the apply pass does
/// not re-parse — a single parse pass).
#[derive(Debug, Default)]
pub struct JsonlValidationSummary {
    /// How many non-empty records were seen.
    pub record_count: usize,
    /// Per-line `(line_no, detail)` failures collected before any DB mutation.
    pub failures: Vec<(usize, String)>,
    /// The ids of the successfully-parsed records (for duplicate detection / reporting).
    pub ids: Vec<String>,
    /// The successfully-parsed, normalized records (carried to avoid a second parse pass).
    pub records: Vec<Issue>,
}

/// Serialize one [`Issue`] to a canonical JSONL line (no trailing newline).
///
/// Every emitted `DateTime<Utc>` is rewritten to `fmt_ts_secs` form (UTC, second precision, `Z`) so
/// export bytes are deterministic and byte-coherent with render (CF-TS/D-OQ-B).
///
/// # Errors
///
/// [`SyncError::JsonEncode`] if serde serialization fails.
pub fn serialize_issue_line(issue: &Issue) -> Result<String, SyncError> {
    let mut value =
        serde_json::to_value(issue).map_err(|source| SyncError::JsonEncode { source })?;
    canonicalize_ts_in_value(&mut value);
    serde_json::to_string(&value).map_err(|source| SyncError::JsonEncode { source })
}

/// Recursively rewrite every RFC-3339 timestamp string in `value` to canonical `fmt_ts_secs` form.
///
/// A field is treated as a timestamp iff its string value parses as RFC-3339 (MF-5 —
/// `parse_from_rfc3339` returns `FixedOffset`, so we re-anchor to `Utc`). The recursion covers the
/// whole tree so nested `dependencies[].created_at` / `comments[].created_at` (no
/// `skip_serializing_if`, MF-6) are canonicalized too. A non-timestamp string is left untouched
/// (only strings that fully parse as RFC-3339 are rewritten).
pub fn canonicalize_ts_in_value(value: &mut Value) {
    match value {
        Value::String(s) => {
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(s) {
                *s = fmt_ts_secs(parsed.with_timezone(&chrono::Utc));
            }
        }
        Value::Array(items) => {
            for item in items {
                canonicalize_ts_in_value(item);
            }
        }
        Value::Object(map) => {
            for (_k, v) in map.iter_mut() {
                canonicalize_ts_in_value(v);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Parse a single JSONL line into an [`Issue`] (trimming surrounding whitespace).
///
/// # Errors
///
/// [`SyncError::JsonlParse`] carrying the 1-based `line_no` on a serde failure.
pub fn parse_issue_line(line: &str, line_no: usize) -> Result<Issue, SyncError> {
    serde_json::from_str::<Issue>(line.trim()).map_err(|source| SyncError::JsonlParse {
        line: line_no,
        source,
    })
}

/// Normalize an issue for import (spine §1.8 order: normalize → hash → validate).
///
/// Dedups + sorts labels, recomputes `content_hash` (the `#[serde(skip)]` idempotency key), and
/// clamps `updated_at >= created_at`.
pub fn normalize(issue: &mut Issue) {
    issue.labels.sort_unstable();
    issue.labels.dedup();
    if issue.updated_at < issue.created_at {
        issue.updated_at = issue.created_at;
    }
    issue.content_hash = Some(issue.compute_content_hash());
}

/// Validate every record in `path` (parse → normalize → validate → in-file dup-id), collecting ALL
/// failures before any DB mutation. Uses the fd-metadata size guard + bounded per-line read (MF-3).
///
/// # Errors
///
/// Ingestion-guard errors ([`SyncError::Io`]/[`SyncError::FileTooLarge`]/[`SyncError::LineTooLong`]).
/// Per-record parse/validation failures are COLLECTED into the summary, not returned as `Err`.
pub fn validate_records(path: &Path) -> Result<JsonlValidationSummary, SyncError> {
    let meta = std::fs::metadata(path).map_err(|source| SyncError::Io {
        path: path.to_path_buf(),
        action: "reading metadata for",
        source,
    })?;
    if !meta.is_file() {
        return Err(SyncError::PathTraversal {
            path: path.to_path_buf(),
            reason: PathReject::NonRegularFile,
        });
    }
    if meta.len() > MAX_IMPORT_FILE_BYTES {
        return Err(SyncError::FileTooLarge {
            path: path.to_path_buf(),
            size: meta.len(),
            cap: MAX_IMPORT_FILE_BYTES,
        });
    }
    let file = File::open(path).map_err(|source| SyncError::Io {
        path: path.to_path_buf(),
        action: "opening",
        source,
    })?;
    let mut reader = BufReader::with_capacity(2 * 1024 * 1024, file);

    let mut summary = JsonlValidationSummary::default();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut line_no = 0usize;
    loop {
        line_no += 1;
        let read = read_line_bounded(&mut reader, &mut buf, line_no, path)?;
        if read == 0 {
            break;
        }
        // A non-UTF-8 line cannot be valid JSONL — record a parse failure rather than panicking.
        let Ok(text) = std::str::from_utf8(&buf) else {
            summary
                .failures
                .push((line_no, "line is not valid UTF-8".to_string()));
            continue;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue; // blank lines are skipped, not counted.
        }
        summary.record_count += 1;

        let mut issue = match parse_issue_line(trimmed, line_no) {
            Ok(issue) => issue,
            Err(err) => {
                summary.failures.push((line_no, err.to_string()));
                continue;
            }
        };
        normalize(&mut issue);
        if let Err(err) = IssueValidator::validate(&issue) {
            summary.failures.push((line_no, err.to_string()));
            continue;
        }
        if !seen_ids.insert(issue.id.clone()) {
            summary
                .failures
                .push((line_no, format!("duplicate id '{}'", issue.id)));
            continue;
        }
        summary.ids.push(issue.id.clone());
        summary.records.push(issue);
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::{
        canonicalize_ts_in_value, normalize, parse_issue_line, serialize_issue_line,
        validate_records,
    };
    use crate::error::SyncError;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::io::Write;
    use unblock_model::{Dependency, DependencyType, Issue, Status};

    fn issue(id: &str) -> Issue {
        Issue {
            id: id.to_string(),
            title: format!("issue {id}"),
            status: Status::Open,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    #[test]
    fn serialize_parse_round_trip_sync_equals() {
        let original = issue("ub-1");
        let line = serialize_issue_line(&original).unwrap();
        let mut back = parse_issue_line(&line, 1).unwrap();
        normalize(&mut back);
        assert!(original.sync_equals(&back));
    }

    #[test]
    fn serialize_canonicalizes_top_level_timestamps() {
        // A sub-second created_at renders WITHOUT a fractional component (second precision, `Z`).
        let mut i = issue("ub-1");
        i.created_at = Utc.timestamp_opt(1_000_000_000, 123_456_789).unwrap();
        i.updated_at = i.created_at;
        let line = serialize_issue_line(&i).unwrap();
        assert!(line.contains("2001-09-09T01:46:40Z"), "line: {line}");
        assert!(!line.contains(".123"), "no sub-second: {line}");
    }

    #[test]
    fn serialize_canonicalizes_nested_dependency_created_at() {
        // MF-6: nested `dependencies[].created_at` (no skip_serializing_if) must be canonicalized.
        let mut i = issue("ub-1");
        let sub = Utc.timestamp_opt(1_000_000_000, 987_654_321).unwrap();
        i.dependencies = vec![Dependency {
            issue_id: "ub-1".to_string(),
            depends_on_id: "ub-2".to_string(),
            dep_type: DependencyType::Blocks,
            created_at: sub,
            created_by: None,
            metadata: None,
            thread_id: None,
        }];
        let line = serialize_issue_line(&i).unwrap();
        assert!(line.contains("2001-09-09T01:46:40Z"), "line: {line}");
        assert!(!line.contains(".987"), "no sub-second nested ts: {line}");
    }

    #[test]
    fn canonicalize_leaves_non_timestamp_strings() {
        let mut v = json!({ "title": "not a date", "created_at": "2026-01-02T03:04:05.5Z" });
        canonicalize_ts_in_value(&mut v);
        assert_eq!(v["title"], "not a date");
        assert_eq!(v["created_at"], "2026-01-02T03:04:05Z");
    }

    #[test]
    fn parse_malformed_line_is_jsonl_parse_error() {
        let err = parse_issue_line("not json", 3).expect_err("bad");
        match err {
            SyncError::JsonlParse { line, .. } => assert_eq!(line, 3),
            other => panic!("expected JsonlParse, got {other:?}"),
        }
    }

    fn write_lines(dir: &tempfile::TempDir, lines: &[&str]) -> std::path::PathBuf {
        let path = dir.path().join("issues.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        path
    }

    #[test]
    fn validate_records_collects_all_failures() {
        let dir = tempfile::tempdir().unwrap();
        let good = serialize_issue_line(&issue("ub-1")).unwrap();
        let dup = serialize_issue_line(&issue("ub-1")).unwrap();
        let path = write_lines(&dir, &[&good, "not json", "", &dup]);
        let summary = validate_records(&path).unwrap();
        // 3 non-empty records (the blank line is skipped, not counted).
        assert_eq!(summary.record_count, 3);
        // One parse failure + one duplicate-id failure.
        assert_eq!(summary.failures.len(), 2);
        assert_eq!(summary.records.len(), 1);
        assert_eq!(summary.ids, vec!["ub-1".to_string()]);
    }

    #[test]
    fn validate_records_clean_file_no_failures() {
        let dir = tempfile::tempdir().unwrap();
        let a = serialize_issue_line(&issue("ub-1")).unwrap();
        let b = serialize_issue_line(&issue("ub-2")).unwrap();
        let path = write_lines(&dir, &[&a, &b]);
        let summary = validate_records(&path).unwrap();
        assert!(summary.failures.is_empty());
        assert_eq!(summary.records.len(), 2);
    }
}
