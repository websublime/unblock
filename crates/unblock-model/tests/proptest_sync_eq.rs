//! Property tests for `sync_equals` algebra (NFR-16).
//!
//! - reflexive / symmetric;
//! - relation-order independence (shuffled deps/comments/labels);
//! - `id` and `estimated_minutes` flips break equality;
//! - volatile fields (`created_at`/`updated_at`/`content_hash`/`agent_context`) are ignored.

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use unblock_model::{Comment, Dependency, DependencyType, Issue};

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

fn arb_dep() -> impl Strategy<Value = Dependency> {
    ("ub-[a-z0-9]{1,6}", "ub-[a-z0-9]{1,6}").prop_map(|(issue_id, depends_on_id)| Dependency {
        issue_id,
        depends_on_id,
        dep_type: DependencyType::Blocks,
        created_at: ts(),
        created_by: None,
        metadata: None,
        thread_id: None,
    })
}

fn arb_comment() -> impl Strategy<Value = Comment> {
    (1i64..1000, "[a-z]{1,8}", "[a-z ]{0,20}").prop_map(|(id, author, body)| Comment {
        id,
        issue_id: "ub-root".to_string(),
        author,
        body,
        created_at: ts(),
    })
}

fn arb_issue() -> impl Strategy<Value = Issue> {
    (
        prop::collection::vec("[a-z0-9-]{1,10}", 0..6),
        prop::collection::vec(arb_dep(), 0..5),
        prop::collection::vec(arb_comment(), 0..5),
        prop::option::of(0i32..1000),
    )
        .prop_map(|(labels, dependencies, comments, est)| Issue {
            id: "ub-root".to_string(),
            title: "prop".to_string(),
            estimated_minutes: est,
            created_at: ts(),
            updated_at: ts(),
            labels,
            dependencies,
            comments,
            ..Issue::default()
        })
}

proptest! {
    #[test]
    fn reflexive(issue in arb_issue()) {
        prop_assert!(issue.sync_equals(&issue));
        prop_assert!(issue.sync_equals(&issue.clone()));
    }

    #[test]
    fn symmetric(a in arb_issue(), b in arb_issue()) {
        prop_assert_eq!(a.sync_equals(&b), b.sync_equals(&a));
    }

    #[test]
    fn relation_order_independent(issue in arb_issue()) {
        let mut shuffled = issue.clone();
        shuffled.labels.reverse();
        shuffled.dependencies.reverse();
        shuffled.comments.reverse();
        prop_assert!(issue.sync_equals(&shuffled));
    }

    #[test]
    fn volatile_fields_ignored(issue in arb_issue()) {
        let mut other = issue.clone();
        other.created_at = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
        other.updated_at = Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
        other.content_hash = Some("ignored".to_string());
        other.agent_context = Some("ignored".to_string());
        prop_assert!(issue.sync_equals(&other));
    }

    #[test]
    fn id_flip_breaks_equality(issue in arb_issue()) {
        let mut other = issue.clone();
        other.id = format!("{}-x", other.id);
        prop_assert!(!issue.sync_equals(&other));
    }

    #[test]
    fn estimated_minutes_flip_breaks_equality(issue in arb_issue()) {
        let mut other = issue.clone();
        other.estimated_minutes = Some(issue.estimated_minutes.unwrap_or(0) + 1);
        prop_assert!(!issue.sync_equals(&other));
    }
}
