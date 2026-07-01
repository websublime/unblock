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

/// Shared FR-8 preflight for BOTH import paths (`import_jsonl` and `bd_import::import_bd`) — MF-4.
///
/// Runs the path-confinement + conflict-marker guards so FR-8 cannot be bypassed by construction: no
/// import can reach the classify+tx tail ([`apply_records`]) without confining the path and rejecting
/// a conflict-marker file first. The bounded fd-metadata size guard is enforced inside
/// [`ensure_no_conflict_markers`] (and again in `validate_records` / the bd `map_bd_record` reader).
/// The per-line validation (`IssueValidator` + in-file dup-id) is caller-specific: `import_jsonl`
/// runs [`validate_records`]; `bd_import` runs the equivalent after `map_bd_record`.
///
/// Returns the canonicalized, confined path (the caller then reads records from it).
///
/// # Errors
///
/// [`SyncError::PathTraversal`] (confinement) / [`SyncError::ConflictMarkers`] (a merge accident) /
/// the ingestion-guard errors from the bounded read — all BEFORE any DB write.
pub(crate) fn preflight_source(
    path: &Path,
    confine_root: &Path,
    allow_external: bool,
) -> Result<std::path::PathBuf, SyncError> {
    // (1) path confinement.
    let canonical = validate_sync_path(path, confine_root, allow_external)?;
    // (2) conflict markers (also enforces the fd-metadata size guard + bounded per-line read, MF-3).
    ensure_no_conflict_markers(&canonical)?;
    Ok(canonical)
}

/// The relation/comment sums over the APPLIED subset (the records `apply_records` actually inserts).
///
/// Faithful to bd's `record_imported_relation_counts` (`temp/beads_rust-main/src/sync/mod.rs:4611-4614`),
/// which is invoked ONLY on an applied Insert/Update record (mod.rs:4563/4579), NEVER on a Skip
/// (mod.rs:4581) — so these sums range over the applied subset, not every mapped record. On an
/// idempotent rerun (all records Skipped) both sums are `0`. Returned so the CALLER finalizes the two
/// relation counts on the report (MF-2): `import_jsonl` ignores them (stays `0`); `import_bd` copies
/// them onto its report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AppliedRelationSums {
    /// Dependency edges on the applied subset (POST-repair/POST-dedup at the `import_bd` call site).
    pub dependencies: usize,
    /// Comments on the applied subset.
    pub comments: usize,
}

/// The classify + atomic-`create_issues` tail shared by BOTH import paths (D24/F5, steps 5a-5c).
///
/// Classifies every record (READ-ONLY `get_issue` probes) under the SAME tombstone-guard-first →
/// `sync_equals`-Skip → exists-Skip matrix, then applies the new-id subset through ONE
/// `Storage::create_issues` tx (rollback-on-any → ZERO rows). The engine holds the D14 write permit
/// across the whole call (MF-4), so no concurrent writer races a classified-new id between probe and
/// tx. `imported` is set only AFTER the tx commits.
///
/// Returns `(ImportReport, AppliedRelationSums)`. The report carries `imported`/`skipped`/
/// `dropped_fields` with `dependencies: 0, comments: 0`; the second tuple element carries the
/// relation/comment sums over the APPLIED subset (the inserted records — NEVER the Skipped ones,
/// faithful to bd's applied-subset scoping). EACH CALLER finalizes the two relation counts on the
/// report (MF-2): `import_jsonl` DISCARDS the sums (leaves them `0`); `import_bd` copies them onto its
/// report.
///
/// `opts.dry_run` short-circuits AFTER classify (report the plan, mutate nothing — SF-5); `import_bd`
/// synthesizes `dry_run: false` at its call site, so it always applies. The applied-subset sums are
/// computed over the same `create_subset` in both the dry-run and commit branches (symmetric with
/// `imported = create_subset.len()`).
///
/// # Errors
///
/// [`SyncError::ImportCollision`] under `Error`; the transparent `Storage` source if the atomic
/// `create_issues` tx fails (rollback → ZERO rows).
pub(crate) async fn apply_records(
    storage: &dyn Storage,
    records: Vec<Issue>,
    dropped: Vec<String>,
    actor: &str,
    opts: &ImportOptions,
) -> Result<(ImportReport, AppliedRelationSums), SyncError> {
    // (5a) classify every record (read-only `get_issue` probes).
    let mut create_subset: Vec<Issue> = Vec::new();
    let mut skipped = 0usize;
    for incoming in records {
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

    // Relation/comment sums over the APPLIED subset ONLY (never the Skipped records) — faithful to
    // bd's applied-subset scoping (mod.rs:4563/4579/4581). On a full-Skip rerun `create_subset` is
    // empty, so both sums are `0`.
    let applied = AppliedRelationSums {
        dependencies: create_subset.iter().map(|i| i.dependencies.len()).sum(),
        comments: create_subset.iter().map(|i| i.comments.len()).sum(),
    };

    // dry_run: report the plan (would-import = new-id subset size), mutate nothing (SF-5).
    if opts.dry_run {
        let report = ImportReport {
            imported: create_subset.len(),
            skipped,
            dependencies: 0,
            comments: 0,
            dropped_fields: dropped,
        };
        return Ok((report, applied));
    }

    // (5b) ONE atomic tx over the new-id subset (rollback-on-any → ZERO rows). `imported` is set only
    //      after the tx COMMITS (a rollback propagates as Err, never a partial count).
    if !create_subset.is_empty() {
        storage.create_issues(&create_subset, actor).await?;
    }

    // (5c) imported set only after commit; relation counts default to 0 (each caller finalizes).
    let report = ImportReport {
        imported: create_subset.len(),
        skipped,
        dependencies: 0,
        comments: 0,
        dropped_fields: dropped,
    };
    Ok((report, applied))
}

/// Import `path` into the store (FR-8), returning the [`ImportReport`].
///
/// The engine holds the D14 write permit across this whole call (MF-4). Production callers pass
/// `on_collision: Skip`; the apply is atomic (classify read-only, then ONE `create_issues` tx). This
/// generic path imports only unblock's own canonical exports, so it applies NO bd repairs and leaves
/// `dependencies`/`comments` at `0` (it never tallies relations).
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
    // (1)/(2) shared FR-8 preflight: path confinement + conflict-marker rejection (ZERO DB writes).
    let canonical = preflight_source(path, confine_root, opts.allow_external)?;
    // (3) per-line validation — ALL failures collected; ANY survivor aborts with ZERO writes.
    let summary = validate_records(&canonical)?;
    if let Some((line, detail)) = summary.failures.first() {
        return Err(first_failure_to_error(*line, detail));
    }

    // (4)/(5) classify + atomic apply via the shared tail — the generic path drops no fields and
    //         never tallies relations: DISCARD the applied-subset sums so `dropped`/`dependencies`/
    //         `comments` stay empty/`0` (D24/F1 — only the bd path tallies relations).
    let (report, _applied) =
        apply_records(storage, summary.records, Vec::new(), actor, opts).await?;
    Ok(report)
}

/// Map the first collected validation failure to its typed [`SyncError`] (ZERO DB writes).
fn first_failure_to_error(line: usize, detail: &str) -> SyncError {
    if detail.starts_with("duplicate id") {
        // Extract the id from the `duplicate id '...'` detail for a precise error.
        let id = detail.split('\'').nth(1).unwrap_or_default().to_string();
        SyncError::DuplicateId { line, id }
    } else {
        // Everything else (a JSONL parse error, a non-UTF-8 line, or a semantic validation failure)
        // surfaces as a `ValidationFailed` carrying the detail, so the exit code stays exit-6 (the
        // JSONL/validation class). Reconstructing a `JsonlParse` from the detail alone is not
        // possible without the raw line, so a single arm is exact here.
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
