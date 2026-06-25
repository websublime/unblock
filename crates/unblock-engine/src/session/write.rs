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
    /// Validates the *post-patch* coherence the engine owns — currently the patch is applied by
    /// storage (per-field events, no-op skip); the engine's pre-validation guards the model
    /// invariants storage trusts (e.g. a `priority` out of range surfaces `ModelError`, not a
    /// flattened backend error). Under the write permit.
    ///
    /// # Errors
    /// - [`EngineError::Model`] if the patch carries an out-of-range scalar (priority).
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn update(&self, id: &str, patch: &IssuePatch) -> Result<Issue> {
        validate_patch(patch)?;
        let _guard = self.acquire().await?;
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
    /// Closes by delegating a `status = Closed` patch to `storage.update_issue` (storage derives
    /// `closed_at` + writes the `StatusChanged`/`Closed` events transactionally), then computes the
    /// **newly-unblocked** set via the `unblock_policy` free functions: every issue that had a gating
    /// edge **to** the closed id is re-evaluated against its live incoming edges, and those that are
    /// now [`ReadyVerdict::Ready`] are returned (OQ-1 — no policy handle). The whole operation runs
    /// under one write permit.
    ///
    /// # `reason` — KNOWN v1 contract gap (surfaced, not simplified)
    ///
    /// The spine §4.1 signature takes `reason: Option<String>`, but the storage `IssuePatch` (spine
    /// §3.1, normative) has **no `close_reason` field** and there is **no dedicated close storage
    /// method** — so in v1 the `reason` **cannot be persisted** through the `Storage` trait. It is
    /// accepted (the public signature stays spine-exact) and surfaced as a tracing event, but NOT
    /// written to the DB. Persisting it needs a spine amendment (an `IssuePatch.close_reason` field
    /// or a `close_issue` storage method) — a cross-crate contract decision for Miguel, NOT a thing
    /// to drop silently or work around by reaching past the `Storage` boundary.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn close_with_suggestions(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<CloseOutcome> {
        let _guard = self.acquire().await?;

        // KNOWN GAP: `reason` is not persistable through the v1 Storage contract (no close_reason in
        // IssuePatch, no close_issue method). Capture it for observability so it is not silently
        // lost, and surface the gap for the spine decision (see the method doc).
        if let Some(ref reason) = reason {
            tracing::info!(
                target: crate::logging::RELIABILITY_TARGET,
                issue = id,
                reason = reason.as_str(),
                "close reason captured but NOT persisted (v1 contract gap — no IssuePatch.close_reason)"
            );
        }

        // Close: status -> Closed. Storage derives closed_at + the Closed/StatusChanged events.
        let patch = IssuePatch {
            status: Some(Status::Closed),
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

/// Validate the scalar fields an `IssuePatch` carries that the engine owns (priority range), so an
/// out-of-range value surfaces as `ModelError` (not a flattened backend error). Nullable text /
/// label ops are validated by storage's `update_issue` against the loaded row.
fn validate_patch(patch: &IssuePatch) -> Result<()> {
    use unblock_error::{FieldError, ModelError};
    use unblock_model::Priority;

    let mut fields = Vec::new();
    if let Some(priority) = patch.priority
        && (priority.0 < Priority::CRITICAL.0 || priority.0 > Priority::BACKLOG.0)
    {
        fields.push(FieldError::new("priority", "must be 0-4"));
    }
    if let Some(minutes) = patch.estimated_minutes {
        if minutes < 0 {
            fields.push(FieldError::new("estimated_minutes", "cannot be negative"));
        } else if minutes > unblock_model::ESTIMATED_MINUTES_MAX {
            fields.push(FieldError::new(
                "estimated_minutes",
                "exceeds maximum (525960 minutes / ~1 year)",
            ));
        }
    }
    if fields.is_empty() {
        Ok(())
    } else {
        Err(EngineError::Model {
            source: ModelError::ValidationFailed { fields },
        })
    }
}
