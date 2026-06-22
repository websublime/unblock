//! `sync_equals` — semantic equality for import/export boundaries (spine §1.8).

use super::Issue;

impl Issue {
    /// Compare two issues using **sync semantics** instead of derived `PartialEq` (spine §1.8).
    ///
    /// This is the import "is this line a no-op?" predicate. It compares the full synced payload
    /// (scalars + `due_at`/`defer_until` + tombstone/compaction fields + relations, the last
    /// **order-independent**), treats `compaction_level == None` as `0`, and **ignores** the
    /// volatile audit-only / recomputed fields `created_at`, `updated_at`, `content_hash`, and
    /// `agent_context`.
    ///
    /// `id` is compared first; `estimated_minutes` is part of the compared payload.
    #[must_use]
    #[allow(clippy::too_many_lines)] // the field-by-field comparison mirrors the frozen spec.
    pub fn sync_equals(&self, other: &Self) -> bool {
        if self.id != other.id
            || self.title != other.title
            || self.description != other.description
            || self.design != other.design
            || self.acceptance_criteria != other.acceptance_criteria
            || self.notes != other.notes
            || self.status != other.status
            || self.priority != other.priority
            || self.issue_type != other.issue_type
            || self.assignee != other.assignee
            || self.owner != other.owner
            || self.estimated_minutes != other.estimated_minutes
            || self.created_by != other.created_by
            || self.closed_at != other.closed_at
            || self.close_reason != other.close_reason
            || self.closed_by_session != other.closed_by_session
            || self.due_at != other.due_at
            || self.defer_until != other.defer_until
            || self.external_ref != other.external_ref
            || self.source_system != other.source_system
            || self.source_repo != other.source_repo
            || self.source_repo_path != other.source_repo_path
            || self.deleted_at != other.deleted_at
            || self.deleted_by != other.deleted_by
            || self.delete_reason != other.delete_reason
            || self.original_type != other.original_type
            || self.compacted_at != other.compacted_at
            || self.compacted_at_commit != other.compacted_at_commit
            || self.original_size != other.original_size
            || self.sender != other.sender
            || self.ephemeral != other.ephemeral
            || self.pinned != other.pinned
            || self.is_template != other.is_template
        {
            return false;
        }

        // compaction_level serialization quirk: None == 0.
        if self.compaction_level.unwrap_or(0) != other.compaction_level.unwrap_or(0) {
            return false;
        }

        // Fast path: differing relation counts can never be equal.
        if self.dependencies.len() != other.dependencies.len()
            || self.comments.len() != other.comments.len()
        {
            return false;
        }

        // Labels: order-independent + dedup.
        let mut self_labels = self.labels.clone();
        self_labels.sort_unstable();
        self_labels.dedup();
        let mut other_labels = other.labels.clone();
        other_labels.sort_unstable();
        other_labels.dedup();
        if self_labels != other_labels {
            return false;
        }

        // Dependencies: order-independent (full 7-key sort).
        let mut self_deps = self.dependencies.clone();
        self_deps.sort_by(dep_sort_key);
        let mut other_deps = other.dependencies.clone();
        other_deps.sort_by(dep_sort_key);
        if self_deps != other_deps {
            return false;
        }

        // Comments: order-independent (5-key sort).
        let mut self_comments = self.comments.clone();
        self_comments.sort_by(comment_sort_key);
        let mut other_comments = other.comments.clone();
        other_comments.sort_by(comment_sort_key);
        if self_comments != other_comments {
            return false;
        }

        true
    }
}

fn dep_sort_key(
    left: &crate::relations::Dependency,
    right: &crate::relations::Dependency,
) -> std::cmp::Ordering {
    left.issue_id
        .cmp(&right.issue_id)
        .then_with(|| left.depends_on_id.cmp(&right.depends_on_id))
        .then_with(|| left.dep_type.as_str().cmp(right.dep_type.as_str()))
        .then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left.created_by.cmp(&right.created_by))
        .then_with(|| left.metadata.cmp(&right.metadata))
        .then_with(|| left.thread_id.cmp(&right.thread_id))
}

fn comment_sort_key(
    left: &crate::relations::Comment,
    right: &crate::relations::Comment,
) -> std::cmp::Ordering {
    left.issue_id
        .cmp(&right.issue_id)
        .then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left.author.cmp(&right.author))
        .then_with(|| left.body.cmp(&right.body))
        .then_with(|| left.id.cmp(&right.id))
}

#[cfg(test)]
mod tests {
    use crate::enums::DependencyType;
    use crate::issue::Issue;
    use crate::relations::Dependency;
    use chrono::{TimeZone, Utc};

    fn base() -> Issue {
        Issue {
            id: "ub-abc123".to_string(),
            title: "Test".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    fn dep(target: &str) -> Dependency {
        Dependency {
            issue_id: "ub-abc123".to_string(),
            depends_on_id: target.to_string(),
            dep_type: DependencyType::Blocks,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            created_by: None,
            metadata: None,
            thread_id: None,
        }
    }

    #[test]
    fn identical_issues_equal() {
        assert!(base().sync_equals(&base()));
    }

    #[test]
    fn id_difference_unequal() {
        let mut other = base();
        other.id = "ub-zzz999".to_string();
        assert!(!base().sync_equals(&other));
    }

    #[test]
    fn estimated_minutes_difference_unequal() {
        let mut a = base();
        a.estimated_minutes = Some(10);
        let mut b = base();
        b.estimated_minutes = Some(20);
        assert!(!a.sync_equals(&b));
    }

    #[test]
    fn volatile_fields_ignored() {
        let mut other = base();
        other.updated_at = Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
        other.created_at = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
        other.content_hash = Some("ignored".to_string());
        other.agent_context = Some("ignored too".to_string());
        assert!(base().sync_equals(&other));
    }

    #[test]
    fn compaction_level_none_equals_some_zero() {
        let mut a = base();
        a.compaction_level = None;
        let mut b = base();
        b.compaction_level = Some(0);
        assert!(a.sync_equals(&b));
    }

    #[test]
    fn dependency_order_independent() {
        let mut a = base();
        a.dependencies = vec![dep("ub-x"), dep("ub-y")];
        let mut b = base();
        b.dependencies = vec![dep("ub-y"), dep("ub-x")];
        assert!(a.sync_equals(&b));
    }

    #[test]
    fn label_order_and_dedup_independent() {
        let mut a = base();
        a.labels = vec!["b".to_string(), "a".to_string(), "a".to_string()];
        let mut b = base();
        b.labels = vec!["a".to_string(), "b".to_string()];
        assert!(a.sync_equals(&b));
    }
}
