//! `impl Session` mutation methods — each acquires the single write permit for the **entire**
//! storage transaction (D14, spine §4.2), then releases on drop.
//!
//! `create`/`update` run `IssueValidator::validate` (→ `ModelError`) **in the engine before** the
//! storage delegation, so validation failures surface as the `EngineError::Model` source (never via
//! `StorageError`, which is validation-free). The permit is held across the whole tx and is
//! cancel-safe: a dropped future before commit releases the permit and leaves the DB
//! committed-or-rolled-back (no partial state — spine §4.2, NFR-5).
//!
//! `close_with_suggestions` closes the issue then computes the **newly-unblocked** set via the
//! `unblock_policy` free functions over caller-built `ReadyContext`s (OQ-1; no policy handle).

use chrono::{DateTime, Utc};
use unblock_model::{Dependency, DependencyType, Issue, IssueValidator, Status};
use unblock_policy::{BlockingEdge, ReadyContext, ReadyVerdict, is_ready};
use unblock_storage::{DeletePlan, IssuePatch};

use crate::error::{EngineError, Result};
use crate::permit::acquire_write;
use crate::report::CloseOutcome;
use crate::session::Session;

impl Session {
    /// Acquire the single write permit, shutdown-aware (D14, spine §4.2).
    async fn acquire(&self) -> Result<crate::permit::WriteGuard> {
        acquire_write(&self.write_permit, &self.shutdown).await
    }

    /// Create an issue, returning its allocated id (FR-1a).
    ///
    /// Runs `IssueValidator::validate` in the engine first (→ `ModelError`), then delegates to
    /// `storage.create_issue` under the write permit. The audit `Event(Created)` is written
    /// transactionally by storage.
    ///
    /// # Errors
    /// - [`EngineError::Model`] if validation fails (aggregate `ValidationFailed`).
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress.
    /// - The transparent storage source on any backend failure (id/external-ref collision, etc.).
    pub async fn create(&self, issue: &Issue) -> Result<String> {
        IssueValidator::validate(issue)?;
        let _guard = self.acquire().await?;
        Ok(self.storage.create_issue(issue, &self.actor).await?)
    }

    /// Apply an [`IssuePatch`] to an issue, returning the updated issue (FR-1c).
    ///
    /// Validates the *post-patch* issue the **same way `create` validates** (the storage
    /// `update_issue` is validation-free, spine §3.2.1): under the write permit it **loads the
    /// current issue**, merges every patch field into a candidate [`Issue`], runs the **full**
    /// [`IssueValidator::validate`] on that merged candidate, and only then delegates to
    /// `storage.update_issue`. So a patch that would set a blank/whitespace `title`, introduce a NUL
    /// byte, or push an over-length/whitespace `external_ref` surfaces [`EngineError::Model`] (the
    /// aggregate `ValidationFailed`) and leaves the DB unchanged — closing the update-path
    /// data-integrity hole. The load + validate + update all run under one permit (linearizable).
    ///
    /// # Errors
    /// - [`EngineError::Model`] if the merged candidate fails validation (aggregate `ValidationFailed`).
    /// - The transparent storage source `IssueNotFound` if the issue does not exist.
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn update(&self, id: &str, patch: &IssuePatch) -> Result<Issue> {
        let _guard = self.acquire().await?;
        // Load the current issue (under the permit, so the validate→update window is serialized).
        let Some(current) = self.storage.get_issue(id).await? else {
            return Err(EngineError::Storage {
                source: unblock_storage::StorageError::IssueNotFound { id: id.to_string() },
            });
        };
        // Merge the patch into a candidate and run the FULL validator (the same gate `create` runs);
        // storage trusts a validated row. A failure surfaces as the EngineError::Model aggregate.
        let candidate = apply_patch_for_validation(&current, patch);
        IssueValidator::validate(&candidate)?;
        Ok(self.storage.update_issue(id, patch, &self.actor).await?)
    }

    /// Execute (or, for `DeleteMode::DryRun`, plan) a delete (FR-1c).
    ///
    /// `DryRun` mutates nothing. Under the write permit (a non-`DryRun` mode writes rows + audit).
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn delete(&self, plan: &DeletePlan) -> Result<DeletePlan> {
        let _guard = self.acquire().await?;
        Ok(self.storage.delete_issue(plan, &self.actor).await?)
    }

    /// Atomically claim an issue for `assignee` (FR-2).
    ///
    /// A single conditional `UPDATE` so concurrent claimers cannot both win; the loser surfaces
    /// `StorageError::AlreadyClaimed` (→ the transparent source). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source (incl. `AlreadyClaimed`).
    pub async fn claim(&self, id: &str, assignee: &str) -> Result<Issue> {
        let _guard = self.acquire().await?;
        Ok(self.storage.claim_issue(id, assignee, &self.actor).await?)
    }

    /// Defer an issue until `until` (FR-3). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn defer(&self, id: &str, until: DateTime<Utc>) -> Result<Issue> {
        let _guard = self.acquire().await?;
        Ok(self.storage.defer_issue(id, until, &self.actor).await?)
    }

    /// Undefer an issue (FR-3). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn undefer(&self, id: &str) -> Result<Issue> {
        let _guard = self.acquire().await?;
        Ok(self.storage.undefer_issue(id, &self.actor).await?)
    }

    /// Add a dependency edge (FR-5). Rejects cycles with a path (→ transparent source). Under the
    /// write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source (incl. `CycleDetected`).
    pub async fn add_dep(&self, dep: &Dependency) -> Result<()> {
        let _guard = self.acquire().await?;
        self.storage.add_dependency(dep, &self.actor).await?;
        Ok(())
    }

    /// Remove a dependency edge (FR-5). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn remove_dep(&self, issue_id: &str, on: &str, ty: &DependencyType) -> Result<()> {
        let _guard = self.acquire().await?;
        self.storage
            .remove_dependency(issue_id, on, ty, &self.actor)
            .await?;
        Ok(())
    }

    /// Close an issue and report the issues it newly unblocked (FR-11).
    ///
    /// Closes by delegating a `status = Closed` patch (carrying the optional `reason`) to
    /// `storage.update_issue` (storage derives `closed_at`, **persists `close_reason`**, and writes
    /// the `StatusChanged`/`Closed` events transactionally), then computes the **newly-unblocked**
    /// set via the `unblock_policy` free functions: every issue that had a gating edge **to** the
    /// closed id is re-evaluated against its live incoming edges, and those that are now
    /// [`ReadyVerdict::Ready`] are returned (OQ-1 — no policy handle). The whole operation runs under
    /// one write permit.
    ///
    /// # `reason` persistence
    ///
    /// `reason` is persisted to the issue's `close_reason` column via the patch's
    /// `close_reason: Some(Some(reason))` field (spine §3.1/§4.1, T1.2 — no longer tracing-only). A
    /// `None` reason leaves `close_reason` unchanged (the patch field stays `None`). The
    /// `close_reason` column is not part of the frozen `content_hash` (spine §1.8), so persisting it
    /// does not perturb import idempotency (FR-26).
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn close_with_suggestions(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<CloseOutcome> {
        let _guard = self.acquire().await?;

        // Close: status -> Closed, persisting the reason (Some => set; None => leave unchanged).
        // Storage derives closed_at + the Closed/StatusChanged events transactionally.
        let patch = IssuePatch {
            status: Some(Status::Closed),
            close_reason: reason.map(Some),
            ..IssuePatch::default()
        };
        let closed = self.storage.update_issue(id, &patch, &self.actor).await?;

        // Compute newly-unblocked via the policy free fns over the live dependents (OQ-1).
        let newly_unblocked = self.newly_unblocked_after_close(id).await?;

        Ok(CloseOutcome {
            closed,
            newly_unblocked,
        })
    }

    /// Re-evaluate every issue that had a gating edge **to** `closed_id` and return those now ready
    /// (FR-11). Uses the `unblock_policy` free functions over caller-built `ReadyContext`s (OQ-1).
    ///
    /// Reads only (the caller already holds the write permit for the close; this just queries the
    /// post-close state through the same `Storage`).
    async fn newly_unblocked_after_close(&self, closed_id: &str) -> Result<Vec<Issue>> {
        // The whole graph: edges are (from = dependent issue_id, to = blocker depends_on_id).
        let graph = self.storage.dependency_graph(&[]).await?;

        // Candidates = the `from` of every edge whose `to` is the just-closed issue (its dependents),
        // de-duplicated and excluding the closed issue itself.
        let mut candidate_ids: Vec<String> = graph
            .edges
            .iter()
            .filter(|edge| edge.to == closed_id && edge.from != closed_id)
            .map(|edge| edge.from.clone())
            .collect();
        candidate_ids.sort();
        candidate_ids.dedup();
        if candidate_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Hydrate every candidate plus every blocker referenced by their incoming edges, so the
        // ReadyContext carries each blocker's LIVE status (post-close).
        let candidates = self.storage.get_issues(&candidate_ids).await?;

        // Blocker ids = the `to` of every edge whose `from` is a candidate.
        let candidate_set: std::collections::HashSet<&str> =
            candidate_ids.iter().map(String::as_str).collect();
        let mut blocker_ids: Vec<String> = graph
            .edges
            .iter()
            .filter(|edge| candidate_set.contains(edge.from.as_str()))
            .map(|edge| edge.to.clone())
            .collect();
        blocker_ids.sort();
        blocker_ids.dedup();
        let blockers = self.storage.get_issues(&blocker_ids).await?;
        let status_of: std::collections::HashMap<&str, Status> = blockers
            .iter()
            .map(|issue| (issue.id.as_str(), issue.status.clone()))
            .collect();

        let now = Utc::now();
        let mut newly_unblocked = Vec::new();
        for candidate in candidates {
            // Build the candidate's incoming gating edges with live source statuses.
            let incoming_blocking: Vec<BlockingEdge> = graph
                .edges
                .iter()
                .filter(|edge| edge.from == candidate.id)
                .map(|edge| BlockingEdge {
                    from_id: edge.to.clone(),
                    dep_type: edge.dep_type.clone(),
                    // A blocker not in the hydrated set (e.g. an `external:%` placeholder the storage
                    // graph omits) is treated as still-open (conservative — it would NOT mark the
                    // issue ready).
                    source_status: status_of
                        .get(edge.to.as_str())
                        .cloned()
                        .unwrap_or(Status::Open),
                })
                .collect();

            let ctx = ReadyContext {
                status: candidate.status.clone(),
                defer_until: candidate.defer_until,
                incoming_blocking,
                now,
            };
            if matches!(is_ready(&ctx), ReadyVerdict::Ready) {
                newly_unblocked.push(candidate);
            }
        }

        // Stable, deterministic order (id ASC) for snapshot-friendliness.
        newly_unblocked.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(newly_unblocked)
    }
}

/// Merge an [`IssuePatch`] into a clone of `current`, producing the candidate [`Issue`] that storage
/// would persist — so the engine can run the **full** [`IssueValidator::validate`] on it before
/// delegating (storage's `update_issue` is validation-free). It mirrors the storage apply rules for
/// every field the validator inspects, including the `status`-derived `closed_at` transition (set on
/// →`Closed`, cleared on reopen) so a legitimate close-via-update does not spuriously fail the
/// closed-state coherence rule.
///
/// `due_at`/`close_reason` and the `parent` reparent are not validated by `IssueValidator`, so they
/// are applied where they affect a validated field (none) and otherwise omitted from the candidate —
/// they cannot make a row invalid. Label ops (`labels_set`/`labels_add`/`labels_remove`) ARE merged
/// (the validator bounds the label count + charset).
fn apply_patch_for_validation(current: &Issue, patch: &IssuePatch) -> Issue {
    use std::collections::BTreeSet;

    let mut candidate = current.clone();

    if let Some(title) = &patch.title {
        candidate.title.clone_from(title);
    }
    apply_opt_text(&patch.description, &mut candidate.description);
    apply_opt_text(&patch.design, &mut candidate.design);
    apply_opt_text(
        &patch.acceptance_criteria,
        &mut candidate.acceptance_criteria,
    );
    apply_opt_text(&patch.notes, &mut candidate.notes);
    apply_opt_text(&patch.owner, &mut candidate.owner);
    apply_opt_text(&patch.external_ref, &mut candidate.external_ref);
    apply_opt_text(&patch.assignee, &mut candidate.assignee);
    apply_opt_text(&patch.close_reason, &mut candidate.close_reason);

    if let Some(priority) = patch.priority {
        candidate.priority = priority;
    }
    if let Some(issue_type) = &patch.issue_type {
        candidate.issue_type = issue_type.clone();
    }
    if let Some(minutes) = patch.estimated_minutes {
        candidate.estimated_minutes = Some(minutes);
    }
    if let Some(due) = patch.due_at {
        candidate.due_at = Some(due);
    }

    // Status transition: replicate storage's closed_at derivation so the closed-state coherence rule
    // matches what would actually be persisted (Closed sets closed_at; a non-terminal status clears
    // it; Tombstone leaves closed_at as-is — the validator permits a tombstone with any closed_at).
    if let Some(status) = &patch.status
        && status.as_str() != candidate.status.as_str()
    {
        match status {
            Status::Closed => {
                if candidate.closed_at.is_none() {
                    candidate.closed_at = Some(Utc::now());
                }
            }
            Status::Tombstone => {}
            _ => candidate.closed_at = None,
        }
        candidate.status = status.clone();
    }

    // Labels: set replaces, add inserts, remove deletes — order-independent, deduped (mirrors
    // storage's apply_labels reconciliation), so the count/charset validation sees the final set.
    let mut labels: BTreeSet<String> = candidate.labels.iter().cloned().collect();
    if let Some(set_labels) = &patch.labels_set {
        labels = set_labels.iter().cloned().collect();
    }
    for add in &patch.labels_add {
        labels.insert(add.clone());
    }
    for remove in &patch.labels_remove {
        labels.remove(remove);
    }
    candidate.labels = labels.into_iter().collect();

    candidate
}

/// Apply a nullable-text patch field (`None` leave / `Some(None)` clear / `Some(Some)` set) onto a
/// candidate field, mirroring the storage `push_opt_text` semantics. The nested `Option` and the
/// borrow mirror the [`IssuePatch`](unblock_storage::IssuePatch) field shape (outer=present-in-patch,
/// inner=clear-vs-set), so both pedantic lints are intentionally scoped here.
#[allow(clippy::option_option, clippy::ref_option)]
fn apply_opt_text(patch: &Option<Option<String>>, target: &mut Option<String>) {
    if let Some(new) = patch {
        target.clone_from(new);
    }
}
