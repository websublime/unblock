//! JSONL import orchestration (FR-8) — preflight → classify → ATOMIC apply.
//!
//! **Order is normative; ZERO DB writes before the tx (MF-1/D23 clause (5)):**
//! 1. `validate_sync_path` (confinement);
//! 2. `ensure_no_conflict_markers`;
//! 3. `validate_records` (normalize → recompute hash → validate → in-file dup-id; ALL failures
//!    collected → abort with ZERO writes if any survive);
//! 4. if `dry_run`, classify every record via read-only `get_issue` probes → build the planned
//!    report → STOP (mutate nothing);
//! 5. apply = classify (read-only `get_issue`) then ONE `Storage::create_issues` tx over the new-id
//!    subset (rollback-on-any → zero rows). The engine holds the D14 write permit across this whole
//!    call (MF-4), so the classify probes and the tx are race-free.
//!
//! Tombstone-non-resurrection is guarded FIRST, before any collision policy: a non-tombstone line
//! for a DB-tombstoned id is SKIPPED, never resurrected (spine §1.8, FR-8).

use std::path::Path;

use unblock_model::{ImportReport, Issue};
use unblock_storage::Storage;

use crate::conflict::ensure_no_conflict_markers;
use crate::error::SyncError;
use crate::jsonl::validate_records;
use crate::path::validate_sync_path;

/// How to handle an incoming record whose id already exists in the DB.
///
/// `Skip` (default, production) is idempotent. `Error`/`OverwriteIfNewer` are INTERNAL opt-ins.
/// `OverwriteIfNewer` is `#[cfg(test)]`-gated (SH-2) — it needs the `update` path (unsound for full
/// fidelity via `IssuePatch`) and is OUTSIDE the FR-8 atomic guarantee, so no production caller
/// reaches it (the engine hardwires `Skip`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CollisionPolicy {
    /// Skip an existing id (idempotent no-op via `sync_equals`); the production default.
    #[default]
    Skip,
    /// Error on an existing id → [`SyncError::ImportCollision`].
    Error,
    /// [`internal`] Overwrite when the incoming record is newer (test-gated; non-production, SH-2).
    #[cfg(test)]
    OverwriteIfNewer,
}

/// Options for [`import_jsonl`] (internal to sync — distinct from the engine's public `ImportOptions`).
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Validate + plan, mutate nothing (FR-8 AC; the MCP `sync.import` `dry_run`).
    pub dry_run: bool,
    /// Opt-in to read outside `confine_root` (NFR-7); default `false`.
    pub allow_external: bool,
    /// How to handle an existing id (default `Skip`).
    pub on_collision: CollisionPolicy,
}

/// The classification decision for one incoming record (read-only; no DB write yet).
#[derive(Debug)]
enum ApplyDecision {
    /// A new id → collect into the atomic `create_issues` subset.
    Import(Box<Issue>),
    /// A no-op / policy skip (surfaced in the report's `skipped` count).
    Skip {
        /// Why the record was skipped.
        reason: &'static str,
    },
    /// A collision under [`CollisionPolicy::Error`].
    Collision {
        /// The colliding id.
        id: String,
    },
}

/// Whether applying `incoming` over `existing` would resurrect a tombstone (spine §1.8).
///
/// A non-tombstone incoming line for a DB-tombstoned id is a resurrection attempt — always SKIPPED,
/// regardless of policy/force.
fn is_tombstone_resurrection(existing: &Issue, incoming: &Issue) -> bool {
    existing.is_tombstone() && !incoming.is_tombstone()
}

/// Classify one incoming record against its (optional) existing DB row (read-only).
///
/// Tombstone-guard FIRST, then the collision policy. `existing` is the `get_issue` probe result
/// (which RETURNS tombstoned rows).
fn classify(existing: Option<Issue>, incoming: Issue, policy: CollisionPolicy) -> ApplyDecision {
    let Some(existing) = existing else {
        return ApplyDecision::Import(Box::new(incoming));
    };
    if is_tombstone_resurrection(&existing, &incoming) {
        return ApplyDecision::Skip {
            reason: "tombstone protection",
        };
    }
    if existing.sync_equals(&incoming) {
        return ApplyDecision::Skip {
            reason: "identical",
        };
    }
    match policy {
        CollisionPolicy::Skip => ApplyDecision::Skip {
            reason: "exists (skip policy)",
        },
        CollisionPolicy::Error => ApplyDecision::Collision { id: incoming.id },
        #[cfg(test)]
        CollisionPolicy::OverwriteIfNewer => {
            // Non-production, test-gated: newer → best-effort overwrite is NOT part of the atomic
            // path; treated as a surfaced skip here (the atomic subset is create-only).
            if incoming.updated_at > existing.updated_at {
                ApplyDecision::Skip {
                    reason: "overwrite-if-newer (unsound; not applied atomically)",
                }
            } else {
                ApplyDecision::Skip {
                    reason: "existing newer/equal",
                }
            }
        }
    }
}

/// Import `path` into the store (FR-8), returning the [`ImportReport`].
///
/// The engine holds the D14 write permit across this whole call (MF-4). Production callers pass
/// `on_collision: Skip`; the apply is atomic (classify read-only, then ONE `create_issues` tx).
///
/// # Errors
///
/// [`SyncError::PathTraversal`]/[`SyncError::ConflictMarkers`]/[`SyncError::JsonlParse`]/
/// [`SyncError::ValidationFailed`]/[`SyncError::DuplicateId`] at preflight (ZERO DB writes);
/// [`SyncError::ImportCollision`] under `Error`; the transparent `Storage` source if the atomic
/// `create_issues` tx fails (rollback → ZERO rows).
pub async fn import_jsonl(
    storage: &dyn Storage,
    path: &Path,
    confine_root: &Path,
    actor: &str,
    opts: &ImportOptions,
) -> Result<ImportReport, SyncError> {
    // (1) path confinement.
    let canonical = validate_sync_path(path, confine_root, opts.allow_external)?;
    // (2) conflict markers.
    ensure_no_conflict_markers(&canonical)?;
    // (3) per-line validation — ALL failures collected; ANY survivor aborts with ZERO writes.
    let summary = validate_records(&canonical)?;
    if let Some((line, detail)) = summary.failures.first() {
        return Err(first_failure_to_error(*line, detail));
    }

    // (4)/(5) classify every record (read-only `get_issue` probes).
    let mut create_subset: Vec<Issue> = Vec::new();
    let mut skipped = 0usize;
    for incoming in summary.records {
        let id = incoming.id.clone();
        let existing = storage.get_issue(&incoming.id).await?;
        match classify(existing, incoming, opts.on_collision) {
            ApplyDecision::Import(issue) => create_subset.push(*issue),
            ApplyDecision::Skip { reason } => {
                tracing::debug!(target: "unblock.reliability", id = %id, reason, "import: skipping record");
                skipped += 1;
            }
            ApplyDecision::Collision { id } => return Err(SyncError::ImportCollision { id }),
        }
    }

    // (4) dry_run: report the plan, mutate nothing.
    if opts.dry_run {
        return Ok(ImportReport {
            imported: create_subset.len(),
            skipped,
            dropped_fields: Vec::new(),
        });
    }

    // (5b) ONE atomic tx over the new-id subset (rollback-on-any → ZERO rows). `imported` is set only
    //      after the tx COMMITS (a rollback propagates as Err, never a partial count).
    if !create_subset.is_empty() {
        storage.create_issues(&create_subset, actor).await?;
    }

    Ok(ImportReport {
        imported: create_subset.len(),
        skipped,
        dropped_fields: Vec::new(),
    })
}

/// Map the first collected validation failure to its typed [`SyncError`] (ZERO DB writes).
fn first_failure_to_error(line: usize, detail: &str) -> SyncError {
    if detail.starts_with("duplicate id") {
        // Extract the id from the `duplicate id '...'` detail for a precise error.
        let id = detail.split('\'').nth(1).unwrap_or_default().to_string();
        SyncError::DuplicateId { line, id }
    } else if detail.starts_with("JSONL parse error") || detail.contains("not valid UTF-8") {
        // Re-parse to reconstruct a JsonlParse is not possible without the line; surface as a
        // ValidationFailed carrying the detail so the exit code stays exit-6 (JSONL parse class).
        SyncError::ValidationFailed {
            line,
            detail: detail.to_string(),
        }
    } else {
        SyncError::ValidationFailed {
            line,
            detail: detail.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CollisionPolicy, ImportOptions, classify, import_jsonl};
    use crate::error::SyncError;
    use crate::jsonl::serialize_issue_line;
    use crate::testutil::{FakeStorage, sample_issue, tombstone_of};
    use std::io::Write;

    fn write_lines(dir: &tempfile::TempDir, lines: &[String]) -> std::path::PathBuf {
        let path = dir.path().join("issues.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        path
    }

    #[tokio::test]
    async fn clean_file_imports_all_via_one_create_issues() {
        let dir = tempfile::tempdir().unwrap();
        let a = serialize_issue_line(&sample_issue("ub-1")).unwrap();
        let b = serialize_issue_line(&sample_issue("ub-2")).unwrap();
        let path = write_lines(&dir, &[a, b]);
        let storage = FakeStorage::with_issues(vec![]);
        let report = import_jsonl(
            &storage,
            &path,
            dir.path(),
            "tester",
            &ImportOptions::default(),
        )
        .await
        .expect("import");
        assert_eq!(report.imported, 2);
        assert_eq!(report.skipped, 0);
        // Routed through ONE `create_issues` call (never a per-record loop).
        assert_eq!(storage.create_issues_calls(), 1);
        assert_eq!(storage.create_issue_calls(), 0);
    }

    #[tokio::test]
    async fn conflict_marker_file_rejected_zero_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("issues.jsonl");
        std::fs::write(&path, "<<<<<<< HEAD\n=======\n").unwrap();
        let storage = FakeStorage::with_issues(vec![]);
        let err = import_jsonl(&storage, &path, dir.path(), "t", &ImportOptions::default())
            .await
            .expect_err("markers");
        assert!(matches!(err, SyncError::ConflictMarkers { .. }));
        assert_eq!(storage.create_issues_calls(), 0);
    }

    #[tokio::test]
    async fn malformed_line_rejected_zero_writes() {
        let dir = tempfile::tempdir().unwrap();
        let good = serialize_issue_line(&sample_issue("ub-1")).unwrap();
        let path = write_lines(&dir, &[good, "not json".to_string()]);
        let storage = FakeStorage::with_issues(vec![]);
        let err = import_jsonl(&storage, &path, dir.path(), "t", &ImportOptions::default())
            .await
            .expect_err("malformed");
        assert!(matches!(err, SyncError::ValidationFailed { .. }));
        assert_eq!(storage.create_issues_calls(), 0);
    }

    #[tokio::test]
    async fn reimport_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let line = serialize_issue_line(&sample_issue("ub-1")).unwrap();
        let path = write_lines(&dir, &[line]);
        // Existing identical row → the second import is a no-op.
        let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
        let report = import_jsonl(&storage, &path, dir.path(), "t", &ImportOptions::default())
            .await
            .expect("import");
        assert_eq!(report.imported, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(storage.create_issues_calls(), 0);
    }

    #[tokio::test]
    async fn tombstone_not_resurrected() {
        let dir = tempfile::tempdir().unwrap();
        // Incoming NON-tombstone line for an id that is tombstoned in the DB.
        let line = serialize_issue_line(&sample_issue("ub-1")).unwrap();
        let path = write_lines(&dir, &[line]);
        let storage = FakeStorage::with_issues(vec![tombstone_of("ub-1")]);
        let report = import_jsonl(&storage, &path, dir.path(), "t", &ImportOptions::default())
            .await
            .expect("import");
        assert_eq!(report.imported, 0, "must not resurrect the tombstone");
        assert_eq!(report.skipped, 1);
        assert_eq!(storage.create_issues_calls(), 0);
    }

    #[tokio::test]
    async fn dry_run_plans_only() {
        let dir = tempfile::tempdir().unwrap();
        let line = serialize_issue_line(&sample_issue("ub-1")).unwrap();
        let path = write_lines(&dir, &[line]);
        let storage = FakeStorage::with_issues(vec![]);
        let report = import_jsonl(
            &storage,
            &path,
            dir.path(),
            "t",
            &ImportOptions {
                dry_run: true,
                ..ImportOptions::default()
            },
        )
        .await
        .expect("dry run");
        assert_eq!(report.imported, 1, "would-import count");
        assert_eq!(storage.create_issues_calls(), 0, "dry_run mutates nothing");
    }

    #[tokio::test]
    async fn collision_policy_error_on_existing_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut incoming = sample_issue("ub-1");
        incoming.title = "changed".to_string();
        let line = serialize_issue_line(&incoming).unwrap();
        let path = write_lines(&dir, &[line]);
        let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
        let err = import_jsonl(
            &storage,
            &path,
            dir.path(),
            "t",
            &ImportOptions {
                on_collision: CollisionPolicy::Error,
                ..ImportOptions::default()
            },
        )
        .await
        .expect_err("collision");
        assert!(matches!(err, SyncError::ImportCollision { .. }));
        assert_eq!(storage.create_issues_calls(), 0);
    }

    #[test]
    fn classify_tombstone_guard_is_first() {
        // Even a differing incoming record is skipped when the existing row is a tombstone.
        let existing = tombstone_of("ub-1");
        let mut incoming = sample_issue("ub-1");
        incoming.title = "resurrect me".to_string();
        let decision = classify(Some(existing), incoming, CollisionPolicy::Error);
        assert!(matches!(
            decision,
            super::ApplyDecision::Skip {
                reason: "tombstone protection"
            }
        ));
    }
}
