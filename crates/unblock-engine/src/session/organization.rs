//! `impl Session` organization methods — the FR-6 remainder (PRD §4 D56, spine §4.1 organization
//! block): the label CATALOG, the label RENAME, the epic ROLLUP read and the explicit close of every
//! eligible epic.
//!
//! `rename_label` and BOTH `close_eligible` runs take the write permit (D14) AND the D31
//! `.write.lock` for the WHOLE operation through the shared `acquire()` (`session/write.rs`) — the
//! dry run too, for a consistent snapshot, as `import_jsonl{dry_run}` and `delete(DryRun)` already
//! do. `list_all_labels` and `epic_statuses` are reads and take neither (FR-10).
//!
//! Eligibility is the ONE predicate in [`crate::epic`]: `epic_statuses` applies it SINGLE-PASS (a
//! parent whose last open child is an eligible epic reads ineligible there, exactly as
//! `stats.epics_eligible` counts it), while `close_eligible` closes the CASCADE
//! ([`crate::epic::cascade_close_set`]) and its dry run is the cascade preview.
//!
//! D57 clause (2a): every method applies the fail-closed gate first — the two mutations on the
//! post-acquire snapshot whose actor they then write with (spine §4.1 actor discipline).

use unblock_error::{FieldError, ModelError};
use unblock_model::{
    CloseEligibleOutcome, EpicStatus, IssueType, LabelCount, LabelRename, LabelValidator,
    ListFilters,
};
use unblock_storage::StorageError;

use crate::epic;
use crate::error::{EngineError, Result};
use crate::session::Session;

/// The `close_reason` every epic closed by `close_eligible` carries (D56 clause (4), bd parity).
const CLOSE_ELIGIBLE_REASON: &str = "All children completed";

impl Session {
    /// The label CATALOG (`query kind: labels`, D56 clause (6)): one entry per distinct label
    /// carried by at least one non-tombstoned issue, `count` over such issues whatever their status,
    /// ordered by label in byte order. A read: no permit, no lock (FR-10).
    ///
    /// # Errors
    /// Forwards any storage failure as the transparent `EngineError` source.
    pub async fn list_all_labels(&self) -> Result<Vec<LabelCount>> {
        self.gated_runtime()?;
        Ok(self.storage.list_all_labels().await?)
    }

    /// RENAME the label `old` to `new` on every non-tombstoned issue (`issue action: relabel`,
    /// D56 clause (5)).
    ///
    /// Both names pass [`LabelValidator`] BEFORE anything else; the failures are re-keyed to the
    /// fields `old_name`/`new_name` in ONE `ValidationFailed` aggregate, with zero writes.
    /// `old == new` returns `affected_issues: 0` without taking the permit or opening a
    /// transaction. Otherwise the whole rename runs under the write permit + the `.write.lock` in
    /// ONE storage transaction ([`unblock_storage::Storage::rename_label`]).
    ///
    /// # Errors
    /// - [`EngineError::Model`] if either name fails validation.
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn rename_label(&self, old: &str, new: &str) -> Result<LabelRename> {
        let mut fields = Vec::new();
        for (field, name) in [("old_name", old), ("new_name", new)] {
            if let Err(err) = LabelValidator::validate(name) {
                fields.push(FieldError::new(field, err.reason));
            }
        }
        if !fields.is_empty() {
            return Err(EngineError::Model {
                source: ModelError::ValidationFailed { fields },
            });
        }

        let affected_issues = if old == new {
            // No permit and no transaction — but still a call on the workspace, so it is gated.
            self.gated_runtime()?;
            0
        } else {
            let (_guard, runtime) = self.acquire_mutation().await?;
            self.storage.rename_label(old, new, &runtime.actor).await?
        };
        Ok(LabelRename {
            old_name: old.to_string(),
            new_name: new.to_string(),
            affected_issues,
        })
    }

    /// The epic ROLLUP read (`query kind: epics`, D56 clause (3)) — SINGLE-PASS eligibility.
    ///
    /// Without `id`: every epic whose status is neither `closed` nor `tombstone` and that is not a
    /// template — deferred, blocked and ephemeral epics included, a childless epic included with
    /// `total_children: 0` — ordered by epic id ascending. With `id`: that one epic whatever its
    /// status except `tombstone`; a closed AND a template epic are both returned, each with
    /// `eligible_for_close: false` because the predicate excludes them. `eligible_only` keeps only
    /// the eligible entries. Counts come from `Storage::epic_child_rollup` (D26; absent = `(0, 0)`;
    /// the by-id read still runs the whole aggregate — disclosed, NFR-2 accepts it). A read: no
    /// permit, no lock (FR-10).
    ///
    /// # Errors
    /// - The transparent storage source `IssueNotFound` for an unknown or tombstoned `id` (this read
    ///   treats a tombstone as absent, as every mutation does through the tombstone-patch guard).
    /// - [`EngineError::Model`] (`ValidationFailed`, field `id`) for an `id` naming an issue of
    ///   another type.
    /// - Any other storage failure as the transparent source.
    pub async fn epic_statuses(
        &self,
        id: Option<&str>,
        eligible_only: bool,
    ) -> Result<Vec<EpicStatus>> {
        self.gated_runtime()?;
        let epics = match id {
            None => {
                // `include_deferred` without `include_closed`: closed + tombstone excluded, every
                // other status kept; templates are dropped in memory.
                let filters = ListFilters {
                    issue_type: vec![IssueType::Epic],
                    include_deferred: true,
                    ..ListFilters::default()
                };
                let mut epics = self.storage.list_issues(&filters).await?;
                epics.retain(|issue| !issue.is_template);
                epics.sort_unstable_by(|a, b| a.id.cmp(&b.id));
                epics
            }
            Some(id) => {
                let Some(issue) = self
                    .storage
                    .get_issue(id)
                    .await?
                    .filter(|issue| !issue.is_tombstone())
                else {
                    return Err(EngineError::Storage {
                        source: StorageError::IssueNotFound { id: id.to_string() },
                    });
                };
                if issue.issue_type != IssueType::Epic {
                    return Err(EngineError::Model {
                        source: ModelError::ValidationFailed {
                            fields: vec![FieldError::new(
                                "id",
                                format!(
                                    "issue {id} is a {}, not an epic",
                                    issue.issue_type.as_str()
                                ),
                            )],
                        },
                    });
                }
                vec![issue]
            }
        };

        // The rollup is sorted by epic id in SQL (byte order, the same order `str::cmp` gives), so
        // each epic finds its counts by binary search.
        let rollup = self.storage.epic_child_rollup().await?;
        Ok(epics
            .into_iter()
            .filter_map(|epic| {
                let (total_children, closed_children) = rollup
                    .binary_search_by(|(epic_id, _)| epic_id.as_str().cmp(epic.id.as_str()))
                    .map_or((0, 0), |i| rollup[i].1);
                let eligible_for_close =
                    epic::is_eligible_for_close(&epic, total_children, closed_children);
                (!eligible_only || eligible_for_close).then_some(EpicStatus {
                    epic,
                    total_children,
                    closed_children,
                    eligible_for_close,
                })
            })
            .collect())
    }

    /// CLOSE every eligible epic, nested epics CASCADING (`issue action: close_eligible`, D56
    /// clause (4)).
    ///
    /// BOTH runs take the write permit + the `.write.lock` for the whole operation and read ONE
    /// snapshot under that guard — `epic_statuses(None, false)` — from which
    /// [`epic::cascade_close_set`] computes the cascade close set (by round, then id ascending:
    /// child before parent). A dry run returns that set and writes nothing. A real run with a
    /// non-empty set closes it in ONE storage transaction
    /// ([`unblock_storage::Storage::close_issues`], the D44 one-transaction invariant) with
    /// `close_reason` "All children completed"; an empty set opens no transaction. `epic_ids` lists
    /// the ids actually closed, in close order. No `newly_unblocked` is computed (bd parity).
    ///
    /// REMOTE-mode residual (D51, no `.write.lock`; routed to the v1.3 lock): there the snapshot is
    /// guarded by the in-process permit only, so a child reopened by another process between
    /// computing the set and committing it can let an epic close with an open child, silently.
    ///
    /// # Errors
    /// - [`EngineError::ShutdownInProgress`] / transparent storage source.
    pub async fn close_eligible(&self, dry_run: bool) -> Result<CloseEligibleOutcome> {
        let (_guard, runtime) = self.acquire_mutation().await?;
        let candidates = self.epic_statuses(None, false).await?;
        let cascade = epic::cascade_close_set(&candidates);

        let epic_ids = if dry_run || cascade.is_empty() {
            cascade
        } else {
            self.storage
                .close_issues(&cascade, CLOSE_ELIGIBLE_REASON, &runtime.actor)
                .await?
        };
        Ok(CloseEligibleOutcome {
            count: epic_ids.len(),
            epic_ids,
            dry_run,
        })
    }
}
