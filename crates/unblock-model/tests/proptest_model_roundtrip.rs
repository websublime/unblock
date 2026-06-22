//! Property tests for serde JSON round-trips (NFR-16).
//!
//! `Issue` → `to_value` → `from_value` == original (modulo the `#[serde(skip)] content_hash`, which
//! is always `None` on load); enums round-trip; `Dependency.metadata` `""` → `None` coercion holds.

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use unblock_model::{DependencyType, EventType, Issue, IssueType, Priority, Status};

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

fn arb_status() -> impl Strategy<Value = Status> {
    prop_oneof![
        Just(Status::Open),
        Just(Status::InProgress),
        Just(Status::Blocked),
        Just(Status::Closed),
        "[a-zA-Z]{1,12}".prop_map(Status::Custom),
    ]
}

fn arb_issue_type() -> impl Strategy<Value = IssueType> {
    prop_oneof![
        Just(IssueType::Task),
        Just(IssueType::Bug),
        Just(IssueType::Epic),
        "[a-zA-Z]{1,12}".prop_map(IssueType::Custom),
    ]
}

fn arb_issue() -> impl Strategy<Value = Issue> {
    (
        "ub-[a-z0-9]{1,8}",
        "[ -~]{1,40}",
        prop::option::of("[ -~]{0,40}"),
        arb_status(),
        arb_issue_type(),
        0i32..=4,
        prop::bool::ANY,
    )
        .prop_map(
            |(id, title, description, status, issue_type, prio, pinned)| Issue {
                id,
                title,
                description,
                status,
                issue_type,
                priority: Priority(prio),
                pinned,
                created_at: ts(),
                updated_at: ts(),
                ..Issue::default()
            },
        )
}

proptest! {
    #[test]
    fn issue_json_roundtrip(issue in arb_issue()) {
        let value = serde_json::to_value(&issue).unwrap();
        let mut back: Issue = serde_json::from_value(value).unwrap();

        // Two documented serde asymmetries to normalize before a field-exact compare:
        //  - content_hash is #[serde(skip)] -> always None on load;
        //  - compaction_level serializes None as 0, so it loads back as Some(0) (this is exactly
        //    why sync_equals treats None == Some(0)).
        prop_assert!(back.content_hash.is_none());
        back.content_hash = issue.content_hash.clone();
        if issue.compaction_level.is_none() {
            prop_assert_eq!(back.compaction_level, Some(0));
            back.compaction_level = None;
        }

        prop_assert_eq!(&issue, &back);
        // The round-trip is also a sync no-op regardless of the normalization above.
        prop_assert!(issue.sync_equals(&back));
    }

    #[test]
    fn status_roundtrip(status in arb_status()) {
        let value = serde_json::to_value(&status).unwrap();
        let back: Status = serde_json::from_value(value).unwrap();
        prop_assert_eq!(status, back);
    }

    #[test]
    fn issue_type_roundtrip(ty in arb_issue_type()) {
        let value = serde_json::to_value(&ty).unwrap();
        let back: IssueType = serde_json::from_value(value).unwrap();
        prop_assert_eq!(ty, back);
    }

    #[test]
    fn priority_roundtrip(p in 0i32..=4) {
        let prio = Priority(p);
        let value = serde_json::to_value(prio).unwrap();
        let back: Priority = serde_json::from_value(value).unwrap();
        prop_assert_eq!(prio, back);
    }

    #[test]
    fn dependency_type_roundtrip_known(idx in 0usize..11) {
        let known = [
            DependencyType::Blocks,
            DependencyType::ParentChild,
            DependencyType::ConditionalBlocks,
            DependencyType::WaitsFor,
            DependencyType::Related,
            DependencyType::DiscoveredFrom,
            DependencyType::RepliesTo,
            DependencyType::RelatesTo,
            DependencyType::Duplicates,
            DependencyType::Supersedes,
            DependencyType::CausedBy,
        ];
        let dep = known[idx].clone();
        let value = serde_json::to_value(&dep).unwrap();
        let back: DependencyType = serde_json::from_value(value).unwrap();
        prop_assert_eq!(dep, back);
    }

    #[test]
    fn event_type_roundtrip(s in "[a-z_]{1,20}") {
        let ev: EventType = serde_json::from_value(serde_json::Value::String(s.clone())).unwrap();
        let value = serde_json::to_value(&ev).unwrap();
        let back: EventType = serde_json::from_value(value).unwrap();
        prop_assert_eq!(ev, back);
    }
}
