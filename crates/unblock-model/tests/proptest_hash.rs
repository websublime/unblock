//! Property tests for `content_hash` invariants (NFR-16).
//!
//! - deterministic for the same issue;
//! - mutating any **included** field changes the hash;
//! - mutating any **excluded** field leaves the hash unchanged;
//! - `compute_content_hash` == `content_hash_from_parts`.

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use unblock_model::{Issue, IssueType, Priority, Status, content_hash_from_parts};

fn arb_issue() -> impl Strategy<Value = Issue> {
    (
        "[a-z0-9-]{1,12}",
        ".{0,40}",
        prop::option::of(".{0,40}"),
        prop::option::of(".{0,40}"),
        0i32..=4,
        prop::bool::ANY,
        prop::bool::ANY,
        prop::option::of("[a-z@.]{1,20}"),
    )
        .prop_map(
            |(id, title, description, owner, prio, pinned, is_template, created_by)| Issue {
                id: format!("ub-{id}"),
                title,
                description,
                owner,
                created_by,
                priority: Priority(prio),
                pinned,
                is_template,
                created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                ..Issue::default()
            },
        )
}

proptest! {
    #[test]
    fn hash_is_deterministic(issue in arb_issue()) {
        prop_assert_eq!(issue.compute_content_hash(), issue.compute_content_hash());
        prop_assert_eq!(issue.compute_content_hash().len(), 64);
    }

    #[test]
    fn compute_equals_from_parts(issue in arb_issue()) {
        let from_parts = content_hash_from_parts(
            &issue.title,
            issue.description.as_deref(),
            issue.design.as_deref(),
            issue.acceptance_criteria.as_deref(),
            issue.notes.as_deref(),
            &issue.status,
            &issue.priority,
            &issue.issue_type,
            issue.assignee.as_deref(),
            issue.owner.as_deref(),
            issue.created_by.as_deref(),
            issue.external_ref.as_deref(),
            issue.source_system.as_deref(),
            issue.pinned,
            issue.is_template,
        );
        prop_assert_eq!(issue.compute_content_hash(), from_parts);
    }

    #[test]
    fn mutating_included_field_changes_hash(issue in arb_issue()) {
        let base = issue.compute_content_hash();

        // Title is always part of the hash; appending changes it.
        let mut t = issue.clone();
        t.title = format!("{}X", t.title);
        prop_assert_ne!(&base, &t.compute_content_hash());

        // Toggling `pinned` flips an included flag.
        let mut p = issue.clone();
        p.pinned = !p.pinned;
        prop_assert_ne!(&base, &p.compute_content_hash());

        // Changing the issue type (Task <-> a distinct custom) is included.
        let mut ty = issue.clone();
        ty.issue_type = IssueType::Custom("zzz-distinct".to_string());
        prop_assert_ne!(&base, &ty.compute_content_hash());
    }

    #[test]
    fn mutating_excluded_field_keeps_hash(issue in arb_issue()) {
        let base = issue.compute_content_hash();

        let mut updated = issue.clone();
        updated.updated_at = Utc.with_ymd_and_hms(2099, 12, 31, 23, 59, 59).unwrap();
        prop_assert_eq!(&base, &updated.compute_content_hash());

        let mut est = issue.clone();
        est.estimated_minutes = Some(12_345);
        prop_assert_eq!(&base, &est.compute_content_hash());

        let mut tomb = issue.clone();
        tomb.deleted_at = Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap());
        tomb.delete_reason = Some("gone".to_string());
        prop_assert_eq!(&base, &tomb.compute_content_hash());

        let mut rel = issue;
        rel.labels = vec!["a".to_string(), "b".to_string()];
        prop_assert_eq!(&base, &rel.compute_content_hash());
    }

    #[test]
    fn status_included(issue in arb_issue()) {
        let mut a = issue.clone();
        a.status = Status::Open;
        let mut b = issue;
        b.status = Status::Blocked;
        prop_assert_ne!(a.compute_content_hash(), b.compute_content_hash());
    }
}
