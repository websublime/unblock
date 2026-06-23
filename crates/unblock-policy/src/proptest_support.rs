//! Shared `proptest` strategies reused across this crate's unit tests, integration tests, and
//! benches (plan §2 `proptest_support.rs`; NFR-16).
//!
//! Gated `#[cfg(any(test, feature = "proptest-support"))]`: it is `#[cfg(test)]`-active for the
//! crate's own unit tests, and the `proptest-support` feature (which pulls in the optional
//! `proptest` dependency) lets the out-of-module integration tests + benches reach the same
//! generators. It is never part of the default/release surface.
//!
//! The strategies build the v1 ready-ranking domain: arbitrary [`Priority`], [`Status`],
//! [`DependencyType`], [`Issue`] (only the fields the comparator/predicate read), [`BlockingEdge`],
//! and [`ReadyContext`].

use chrono::{DateTime, TimeZone, Utc};
use proptest::collection::vec;
use proptest::prelude::*;

use unblock_model::{DependencyType, Issue, IssueType, ListFilters, Priority, Status};

use crate::inheritance::{AncestorNode, InheritanceConfig};
use crate::ready::{BlockingEdge, ReadyContext};

/// A bounded, deterministic epoch-second timestamp strategy (UTC).
///
/// Bounded so generated timestamps stay valid and comparisons exercise both `<`/`=`/`>` paths.
pub fn arb_timestamp() -> impl Strategy<Value = DateTime<Utc>> {
    // ~1970..2065, dense enough to hit ties on `created_at`.
    (0_i64..3_000_000_000_i64)
        .prop_map(|secs| Utc.timestamp_opt(secs, 0).single().unwrap_or_else(Utc::now))
}

/// Arbitrary [`Priority`] across the full valid `0..=4` range (P0..P4).
pub fn arb_priority() -> impl Strategy<Value = Priority> {
    (0_i32..=4).prop_map(Priority)
}

/// Arbitrary [`Status`], covering every known variant plus a small space of `Custom` strings.
pub fn arb_status() -> impl Strategy<Value = Status> {
    prop_oneof![
        Just(Status::Open),
        Just(Status::InProgress),
        Just(Status::Blocked),
        Just(Status::Deferred),
        Just(Status::Draft),
        Just(Status::Closed),
        Just(Status::Tombstone),
        Just(Status::Pinned),
        "[a-z]{1,8}".prop_map(Status::Custom),
    ]
}

/// Arbitrary [`DependencyType`], covering every known variant plus a `Custom` tail.
pub fn arb_dependency_type() -> impl Strategy<Value = DependencyType> {
    prop_oneof![
        Just(DependencyType::Blocks),
        Just(DependencyType::ParentChild),
        Just(DependencyType::ConditionalBlocks),
        Just(DependencyType::WaitsFor),
        Just(DependencyType::Related),
        Just(DependencyType::DiscoveredFrom),
        Just(DependencyType::RepliesTo),
        Just(DependencyType::RelatesTo),
        Just(DependencyType::Duplicates),
        Just(DependencyType::Supersedes),
        Just(DependencyType::CausedBy),
        "[a-z]{1,8}".prop_map(DependencyType::Custom),
    ]
}

/// Arbitrary [`Issue`] populated only with the fields the ready comparator/predicate read
/// (`id`, `priority`, `created_at`); everything else takes its default.
///
/// `id` is drawn from a small alphabet so id ties are reachable (exercising the `id`-tie-break and
/// the comparator's reflexivity-on-tie property).
pub fn arb_ready_issue() -> impl Strategy<Value = Issue> {
    ("ub-[a-c]{1,3}", arb_priority(), arb_timestamp()).prop_map(|(id, priority, created_at)| {
        Issue {
            id,
            priority,
            created_at,
            ..Issue::default()
        }
    })
}

/// Arbitrary [`BlockingEdge`] (a `from_id`, a dependency type, and a source status).
pub fn arb_blocking_edge() -> impl Strategy<Value = BlockingEdge> {
    ("ub-[a-z]{1,5}", arb_dependency_type(), arb_status()).prop_map(
        |(from_id, dep_type, source_status)| BlockingEdge {
            from_id,
            dep_type,
            source_status,
        },
    )
}

/// Arbitrary [`ReadyContext`] (status, optional deferral, up to four incoming edges, and `now`).
pub fn arb_ready_context() -> impl Strategy<Value = ReadyContext> {
    (
        arb_status(),
        prop::option::of(arb_timestamp()),
        vec(arb_blocking_edge(), 0..4),
        arb_timestamp(),
    )
        .prop_map(
            |(status, defer_until, incoming_blocking, now)| ReadyContext {
                status,
                defer_until,
                incoming_blocking,
                now,
            },
        )
}

/// Arbitrary [`IssueType`], covering every known variant plus a `Custom` tail.
pub fn arb_issue_type() -> impl Strategy<Value = IssueType> {
    prop_oneof![
        Just(IssueType::Task),
        Just(IssueType::Bug),
        Just(IssueType::Feature),
        Just(IssueType::Epic),
        Just(IssueType::Chore),
        Just(IssueType::Docs),
        Just(IssueType::Question),
        "[a-z]{1,8}".prop_map(IssueType::Custom),
    ]
}

/// A small label-string strategy (drawn from a tiny alphabet so set membership/dedup is reachable).
fn arb_label() -> impl Strategy<Value = String> {
    "[a-c]{1,3}"
}

/// Arbitrary [`ListFilters`] exercising every field the fingerprint reads (the set fields use a
/// small alphabet so reordering/duplication collisions are reachable).
pub fn arb_list_filters() -> impl Strategy<Value = ListFilters> {
    (
        vec(arb_status(), 0..4),
        vec(arb_issue_type(), 0..4),
        prop::option::of("[a-z]{1,6}"),
        vec(arb_label(), 0..4),
        vec(arb_label(), 0..4),
        prop::option::of(arb_priority()),
        prop::option::of(arb_priority()),
        prop::option::of("[a-z ]{0,8}"),
        prop::bool::ANY,
        prop::bool::ANY,
        prop::option::of(0_usize..1000),
        prop::option::of(0_usize..1000),
    )
        .prop_map(
            |(
                status,
                issue_type,
                assignee,
                labels_all,
                labels_any,
                priority_min,
                priority_max,
                text_contains,
                include_deferred,
                include_closed,
                limit,
                offset,
            )| ListFilters {
                status,
                issue_type,
                assignee,
                labels_all,
                labels_any,
                priority_min,
                priority_max,
                text_contains,
                include_deferred,
                include_closed,
                limit,
                offset,
            },
        )
}

/// A strategy that returns a [`ListFilters`] together with a logically-equal permutation of its
/// set fields (reversed + each set's first entry duplicated once), for order-insensitivity tests.
///
/// The permutation is deterministic (it ignores the RNG), so the pair is reproducible from the
/// seed: order + duplication of the set fields must not change the fingerprint.
pub fn arb_list_filters_with_permutation() -> impl Strategy<Value = (ListFilters, ListFilters)> {
    arb_list_filters().prop_map(|filters| {
        let permuted = permute_sets(&filters);
        (filters, permuted)
    })
}

/// Build a logically-equal `ListFilters`: reverse each set field and append a duplicate of its
/// first element (order + duplication must not affect the fingerprint).
fn permute_sets(filters: &ListFilters) -> ListFilters {
    fn perturb<T: Clone>(items: &[T]) -> Vec<T> {
        let mut out: Vec<T> = items.iter().rev().cloned().collect();
        if let Some(first) = items.first() {
            out.push(first.clone());
        }
        out
    }
    ListFilters {
        status: perturb(&filters.status),
        issue_type: perturb(&filters.issue_type),
        labels_all: perturb(&filters.labels_all),
        labels_any: perturb(&filters.labels_any),
        ..filters.clone()
    }
}

/// Arbitrary [`AncestorNode`] (id, type, title, optional `agent_context`, tombstone flag).
pub fn arb_ancestor_node() -> impl Strategy<Value = AncestorNode> {
    (
        "ub-[a-z]{1,5}",
        arb_issue_type(),
        "[a-z ]{0,12}",
        prop::option::of("[a-z]{1,10}"),
        prop::bool::ANY,
    )
        .prop_map(
            |(id, issue_type, title, agent_context, is_tombstone)| AncestorNode {
                id,
                issue_type,
                title,
                agent_context,
                is_tombstone,
            },
        )
}

/// Arbitrary ancestor chain (0..6 nodes, nearest-first).
pub fn arb_ancestor_chain() -> impl Strategy<Value = Vec<AncestorNode>> {
    vec(arb_ancestor_node(), 0..6)
}

/// Arbitrary [`InheritanceConfig`] (enabled flag + a field list drawn from plausible field names).
pub fn arb_inheritance_config() -> impl Strategy<Value = InheritanceConfig> {
    let field = prop_oneof![
        Just("agent_context".to_string()),
        Just("design".to_string()),
        Just("notes".to_string()),
    ];
    (prop::bool::ANY, vec(field, 0..3))
        .prop_map(|(enabled, fields)| InheritanceConfig { enabled, fields })
}
