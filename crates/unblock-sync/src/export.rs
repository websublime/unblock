//! JSONL export orchestration (FR-7).
//!
//! Preflight the path → pull the FULL non-ephemeral corpus (incl. closed + tombstones via
//! `ListFilters.include_tombstone`, FORK-1/D23) → exclude ephemeral / `-wisp-` rows in-crate →
//! order `id ASC` → serialize each line (canonical timestamps, CF-TS) → atomic durable write.

use std::path::{Path, PathBuf};

use unblock_model::{ExportReport, ListFilters};
use unblock_storage::Storage;

use crate::atomic::write_atomic;
use crate::error::SyncError;
use crate::jsonl::serialize_issue_line;
use crate::path::validate_sync_path;

/// Options for [`export_jsonl`].
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Opt-in to write outside `confine_root` (NFR-7); default `false`.
    pub allow_external: bool,
    /// A written reason REQUIRED when `allow_external` is set (NFR-5/D30 forward seam). An
    /// `allow_external` override WITHOUT a reason is [`SyncError::ExternalOverrideWithoutReason`];
    /// when present, it rides the NFR-13 force-override INFO. Unreachable in v1 (`allow_external` is
    /// forced `false` by the engine).
    pub external_reason: Option<String>,
}

/// Export the store to `path` atomically (FR-7), returning the [`ExportReport`].
///
/// Pulls the full corpus (`include_closed=true, include_deferred=true, include_tombstone=true`),
/// excludes ephemeral rows + `-wisp-` ids, orders `id ASC`, and serializes each with canonical
/// timestamps. This is a READ + atomic write; the engine acquires no write permit for it.
///
/// # Errors
///
/// [`SyncError::ExternalOverrideWithoutReason`] when `allow_external` is set without a written
/// reason (NFR-5/D30); [`SyncError::PathTraversal`] on a rejected path; [`SyncError::JsonEncode`] on
/// serialization; [`SyncError::Io`] on the atomic write; the transparent `Storage` source on a
/// backend read failure.
pub async fn export_jsonl(
    storage: &dyn Storage,
    path: &Path,
    confine_root: &Path,
    opts: &ExportOptions,
) -> Result<ExportReport, SyncError> {
    // NFR-5/D30 (forward seam): an `allow_external` override MUST carry a written reason. The reject
    // lives in the orchestrator (it holds the opts + knows the operation label).
    crate::path::reject_external_without_reason(
        path,
        opts.allow_external,
        opts.external_reason.as_deref(),
    )?;
    let canonical: PathBuf = validate_sync_path(path, confine_root, opts.allow_external)?;
    if opts.allow_external && !canonical.starts_with(confine_root) {
        // NFR-13 (D30): ONE INFO covering BOTH external-path use AND force-override (they coincide in
        // v1 — the only external-path mechanism IS the reason-gated `allow_external`). The reject
        // above guarantees `external_reason` is `Some` here.
        crate::reliability::reliability_guard!(
            operation = "export",
            path = canonical.display(),
            result = "external-path-force-override",
            reason = opts.external_reason.as_deref().unwrap_or_default(),
        );
    }

    // Full non-ephemeral corpus incl. closed + tombstones (FORK-1/D23).
    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        include_tombstone: true,
        ..ListFilters::default()
    };
    let mut issues = storage.list_issues(&filters).await?;

    // Exclude ephemeral + `-wisp-` rows IN-CRATE (storage has no ephemeral filter on `list_issues`).
    issues.retain(|issue| !issue.ephemeral && !issue.id.contains("-wisp-"));
    // Deterministic byte order.
    issues.sort_by(|a, b| a.id.cmp(&b.id));

    let mut lines = Vec::with_capacity(issues.len());
    for issue in &issues {
        lines.push(serialize_issue_line(issue)?);
    }

    let written = write_atomic(&canonical, confine_root, lines.into_iter()).await?;
    Ok(ExportReport {
        written,
        path: canonical,
    })
}

#[cfg(test)]
mod tests {
    use super::{ExportOptions, export_jsonl};
    use crate::error::SyncError;
    use crate::testutil::FakeStorage;
    use crate::testutil::sample_issue;

    #[tokio::test]
    async fn exports_n_lines_and_report_matches() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FakeStorage::with_issues(vec![
            sample_issue("ub-2"),
            sample_issue("ub-1"),
            sample_issue("ub-3"),
        ]);
        let target = dir.path().join("issues.jsonl");
        let report = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect("export");
        assert_eq!(report.written, 3);
        let content = std::fs::read_to_string(&target).unwrap();
        // 3 lines, id ASC order (ub-1, ub-2, ub-3).
        let ids: Vec<String> = content
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["id"].as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(ids, vec!["ub-1", "ub-2", "ub-3"]);
    }

    #[tokio::test]
    async fn excludes_ephemeral_and_wisp() {
        let dir = tempfile::tempdir().unwrap();
        let mut ephemeral = sample_issue("ub-eph");
        ephemeral.ephemeral = true;
        let storage = FakeStorage::with_issues(vec![
            sample_issue("ub-1"),
            ephemeral,
            sample_issue("ub-wisp-x"),
        ]);
        let target = dir.path().join("issues.jsonl");
        let report = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect("export");
        assert_eq!(report.written, 1, "only the non-ephemeral non-wisp row");
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.contains("ub-1"));
        assert!(!content.contains("ub-eph"));
        assert!(!content.contains("ub-wisp-x"));
    }

    #[tokio::test]
    async fn empty_db_exports_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FakeStorage::with_issues(vec![]);
        let target = dir.path().join("issues.jsonl");
        let report = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect("export");
        assert_eq!(report.written, 0);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "");
    }

    #[tokio::test]
    async fn external_path_without_allow_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
        let target = other.path().join("issues.jsonl");
        let err = export_jsonl(&storage, &target, dir.path(), &ExportOptions::default())
            .await
            .expect_err("external rejected");
        assert!(matches!(err, SyncError::PathTraversal { .. }));
    }
}
