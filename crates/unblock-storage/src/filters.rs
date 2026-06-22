//! Storage-owned value types: the delete plan/mode and the partial-update patch (spine §3.1).
//!
//! The query/result contract types (`ListFilters`, `CountGroupBy`, `CountBucket`, `GraphEdge`,
//! `DepTree`, and the diagnostics DTOs) are **not** defined here — they live in `unblock-model`
//! §1.10 and are re-exported from [`crate`] (CF-A/CF-B/CF-C). This module owns only the three
//! storage-specific types that no other crate needs to define: [`DeletePlan`], [`DeleteMode`], and
//! [`IssuePatch`].

use chrono::{DateTime, Utc};
use unblock_model::{IssueType, Priority, Status};

/// How a delete should be carried out (spine §3.1).
///
/// `DryRun` is the planning mode: [`crate::Storage::delete_issue`] computes the resolved plan
/// (including `cascade_children`) and returns it **without mutating anything**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// Soft-delete: set `status = Tombstone` + the `deleted_*` fields, preserving `original_type`.
    Tombstone,
    /// Tombstone the targets **and** their children (resolved into `cascade_children`).
    Cascade,
    /// Permanently delete the rows.
    Hard,
    /// Compute and return the plan only; mutate nothing.
    DryRun,
}

/// A resolved delete operation (spine §3.1).
///
/// `targets` are the explicitly requested ids; `cascade_children` are the additional ids resolved
/// for the chosen [`DeleteMode`] (always populated by the storage layer so a `DryRun` plan shows
/// the full blast radius before any mutation).
#[derive(Debug, Clone)]
pub struct DeletePlan {
    /// The mode to execute.
    pub mode: DeleteMode,
    /// The explicitly requested target ids.
    pub targets: Vec<String>,
    /// The child ids that the chosen mode will also affect (resolved by storage).
    pub cascade_children: Vec<String>,
}

/// A partial update to an [`unblock_model::Issue`] (spine §3.1; field set = Option B).
///
/// This enumerates **every model-backed updatable column** that
/// [`crate::Storage::update_issue`] mutates (cross-checked against `unblock-model` `Issue`,
/// spine §1.6), plus the label-relation and reparent operations. It deliberately omits
/// `defer_until` — that field is owned by [`crate::Storage::defer_issue`] /
/// [`crate::Storage::undefer_issue`], not by a generic patch.
///
/// # Field semantics
///
/// - Outer `Option` everywhere = **"present in this patch / leave unchanged"**: `None` means do
///   not touch the column.
/// - Nullable text columns use `Option<Option<String>>`: `None` = leave; `Some(None)` = clear the
///   column to `NULL`; `Some(Some(_))` = set it.
/// - Non-nullable / scalar columns use a plain `Option<T>`: `Some(_)` sets, `None` leaves.
/// - `labels_set` (when `Some`) replaces the whole label set; `labels_add`/`labels_remove` are
///   applied additively/subtractively. `parent` reparents (cycle-checked); `Some(None)` detaches.
///
/// [`Default`] yields an all-`None` / empty patch (patch nothing) — useful for builder-style
/// construction and for the no-op-update path (a `Default` patch changes no field and writes no
/// `Event`).
#[derive(Debug, Clone, Default)]
// outer=present-in-patch; inner=clear-vs-set on nullable columns — this is an intentional,
// documented use of the nested Option, so the pedantic lint is scoped to the struct.
#[allow(clippy::option_option)]
pub struct IssuePatch {
    /// Title (`NOT NULL` column — plain `Option`, cannot be cleared to `NULL`).
    pub title: Option<String>,

    /// Description (nullable text: `None` leave / `Some(None)` clear / `Some(Some)` set).
    pub description: Option<Option<String>>,
    /// Technical design notes (nullable text).
    pub design: Option<Option<String>>,
    /// Acceptance criteria (nullable text).
    pub acceptance_criteria: Option<Option<String>>,
    /// Additional notes (nullable text).
    pub notes: Option<Option<String>>,
    /// Issue owner (nullable text).
    pub owner: Option<Option<String>>,
    /// External reference (nullable text; orphans derive from this, FR-15).
    pub external_ref: Option<Option<String>>,
    /// Assigned actor (nullable text).
    pub assignee: Option<Option<String>>,

    /// Workflow status (scalar; plain `Option`).
    pub status: Option<Status>,
    /// Priority (scalar; plain `Option`).
    pub priority: Option<Priority>,
    /// Issue type (scalar; plain `Option`).
    pub issue_type: Option<IssueType>,
    /// Estimated effort in minutes (scalar; plain `Option`).
    pub estimated_minutes: Option<i32>,
    /// Due date (scalar; plain `Option`).
    pub due_at: Option<DateTime<Utc>>,

    /// Labels to add (applied after `labels_set`).
    pub labels_add: Vec<String>,
    /// Labels to remove (applied after `labels_set`).
    pub labels_remove: Vec<String>,
    /// Replace the entire label set (when `Some`).
    pub labels_set: Option<Vec<String>>,

    /// Reparent the issue (cycle-checked). `Some(None)` detaches from any parent.
    pub parent: Option<Option<String>>,
}

#[cfg(test)]
mod tests {
    use super::{DeleteMode, DeletePlan, IssuePatch};
    use unblock_model::{Priority, Status};

    /// `DeleteMode` is `Copy` and its four variants are exhaustive & distinct.
    #[test]
    fn delete_mode_is_copy_and_exhaustive() {
        for mode in [
            DeleteMode::Tombstone,
            DeleteMode::Cascade,
            DeleteMode::Hard,
            DeleteMode::DryRun,
        ] {
            // `Copy`: using `mode` after passing it by value must still compile.
            let copied = mode;
            assert_eq!(copied, mode);
            // Exhaustive match (a new variant would force this to be updated).
            let label = match mode {
                DeleteMode::Tombstone => "tombstone",
                DeleteMode::Cascade => "cascade",
                DeleteMode::Hard => "hard",
                DeleteMode::DryRun => "dry_run",
            };
            assert!(!label.is_empty());
        }
    }

    /// `IssuePatch::default()` is an all-empty patch (patch nothing).
    #[test]
    fn default_patch_is_empty() {
        let patch = IssuePatch::default();
        assert!(patch.title.is_none());
        assert!(patch.description.is_none());
        assert!(patch.status.is_none());
        assert!(patch.priority.is_none());
        assert!(patch.assignee.is_none());
        assert!(patch.labels_add.is_empty());
        assert!(patch.labels_remove.is_empty());
        assert!(patch.labels_set.is_none());
        assert!(patch.parent.is_none());
    }

    /// Field-shape compile-test: the three Option flavours behave as documented (leave/clear/set).
    #[test]
    fn issue_patch_field_shapes() {
        let patch = IssuePatch {
            title: Some("New title".to_string()), // NOT NULL -> plain Option
            description: Some(None),              // clear to NULL
            notes: Some(Some("a note".to_string())), // set
            assignee: None,                       // leave unchanged
            status: Some(Status::InProgress),     // scalar set
            priority: Some(Priority::HIGH),
            labels_set: Some(vec!["urgent".to_string()]),
            labels_add: vec!["triage".to_string()],
            parent: Some(None), // detach from parent
            ..IssuePatch::default()
        };
        assert_eq!(patch.title.as_deref(), Some("New title"));
        assert_eq!(patch.description, Some(None));
        assert_eq!(patch.notes, Some(Some("a note".to_string())));
        assert!(patch.assignee.is_none());
        assert_eq!(patch.status, Some(Status::InProgress));
        assert_eq!(patch.priority, Some(Priority::HIGH));
        assert_eq!(
            patch.labels_set.as_deref(),
            Some(["urgent".to_string()].as_slice())
        );
        assert_eq!(patch.parent, Some(None));
    }

    /// A `DeletePlan` is constructed explicitly (it has no `Default` — there is no sensible
    /// all-empty default mode).
    #[test]
    fn delete_plan_construction() {
        let plan = DeletePlan {
            mode: DeleteMode::DryRun,
            targets: vec!["ub-1".to_string()],
            cascade_children: vec!["ub-2".to_string()],
        };
        assert_eq!(plan.mode, DeleteMode::DryRun);
        assert_eq!(plan.targets, ["ub-1".to_string()]);
        assert_eq!(plan.cascade_children, ["ub-2".to_string()]);
    }
}
