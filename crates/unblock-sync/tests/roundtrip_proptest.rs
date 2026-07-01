//! Property suite (NFR-16): the REAL `serialize_issue_line` → `parse_issue_line` round-trip is a
//! `sync_equals`-identity (spine §1.8 — NOT `==`, since `content_hash` + volatile fields recompute
//! on load), and the serializer is deterministic across runs.
//!
//! MF-7: the strategy second-truncates EVERY `sync_equals`-compared timestamp (top-level
//! `closed_at`/`due_at`/`defer_until`/`deleted_at`/`compacted_at` + nested `dependencies[].created_at`)
//! AND the LHS `original` is itself second-truncated — `serialize_issue_line` renders every timestamp
//! at second precision, so a sub-second value on a compared field would break `sync_equals`.
//! `updated_at >= created_at`.

use chrono::{DateTime, TimeZone, Utc};
use proptest::prelude::*;
use unblock_model::{Dependency, DependencyType, Issue, Status};
use unblock_sync::{parse_issue_line, serialize_issue_line};

/// A second-precision UTC timestamp within a sane range.
fn arb_ts() -> impl Strategy<Value = DateTime<Utc>> {
    (946_684_800_i64..4_102_444_800_i64).prop_map(|secs| Utc.timestamp_opt(secs, 0).unwrap())
}

prop_compose! {
    fn arb_dependency(issue_id: String)(
        target in "ub-[a-z0-9]{1,6}",
        created_at in arb_ts(),
    ) -> Dependency {
        Dependency {
            issue_id: issue_id.clone(),
            depends_on_id: target,
            dep_type: DependencyType::Blocks,
            created_at, // second-truncated (MF-7): dependencies[].created_at is sync_equals-compared.
            created_by: None,
            metadata: None,
            thread_id: None,
        }
    }
}

prop_compose! {
    fn arb_issue()(
        id in "ub-[a-z0-9]{1,8}",
        title in "[a-zA-Z0-9 ]{1,40}",
        created_at in arb_ts(),
        updated_delta in 0_i64..1_000_000,
        due in proptest::option::of(arb_ts()),
        defer in proptest::option::of(arb_ts()),
        closed in proptest::option::of(arb_ts()),
        labels in proptest::collection::vec("[a-z]{1,6}", 0..4),
        n_deps in 0_usize..3,
    )(
        id in Just(id.clone()),
        title in Just(title),
        created_at in Just(created_at),
        updated_delta in Just(updated_delta),
        due in Just(due),
        defer in Just(defer),
        closed in Just(closed),
        labels in Just(labels),
        deps in proptest::collection::vec(arb_dependency(id.clone()), n_deps..=n_deps),
    ) -> Issue {
        let updated_at = created_at + chrono::Duration::seconds(updated_delta); // >= created_at.
        Issue {
            id,
            title,
            status: Status::Open,
            created_at,
            updated_at,
            closed_at: closed,  // sync_equals-compared → second-truncated.
            due_at: due,        // sync_equals-compared → second-truncated.
            defer_until: defer, // sync_equals-compared → second-truncated.
            labels,
            dependencies: deps,
            ..Issue::default()
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn parse_of_serialize_is_sync_equals(mut original in arb_issue()) {
        // The LHS is itself second-truncated by construction (arb_ts), so a serialize→parse round
        // trip is a `sync_equals` identity through the REAL crate helpers.
        original.content_hash = Some(original.compute_content_hash());
        let line = serialize_issue_line(&original).expect("serialize");
        let back = parse_issue_line(&line, 1).expect("parse");
        prop_assert!(
            original.sync_equals(&back),
            "sync_equals must hold:\n{original:?}\n{back:?}"
        );
    }

    #[test]
    fn serialize_is_deterministic(original in arb_issue()) {
        let a = serialize_issue_line(&original).expect("serialize");
        let b = serialize_issue_line(&original).expect("serialize");
        prop_assert_eq!(a, b);
    }
}
