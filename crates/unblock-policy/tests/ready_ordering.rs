//! End-to-end ready-ranking contract (plan §2 `tests/ready_ordering.rs`; NFR-16).
//!
//! A golden `insta` snapshot of a ranked fixture (behaviour fidelity to the original `e2e_ready`
//! ordering), plus the `proptest` total-order suite over [`cmp_ready`] / [`ready_sort_key`] using
//! the shared generators in `unblock_policy::proptest_support` (reachable because the crate's
//! dev-build enables the `proptest-support` feature).

use std::cmp::Ordering;

use chrono::{DateTime, TimeZone, Utc};
use proptest::prelude::*;

use unblock_model::{Issue, Priority};
use unblock_policy::proptest_support::arb_ready_issue;
use unblock_policy::{cmp_ready, ready_sort_key};

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().expect("valid ts")
}

fn issue(id: &str, priority: Priority, created_secs: i64) -> Issue {
    Issue {
        id: id.to_string(),
        priority,
        created_at: at(created_secs),
        ..Issue::default()
    }
}

#[test]
fn golden_ranked_fixture() {
    // A deliberately shuffled candidate set spanning both buckets, ties on bucket+age, and an
    // id-only tie-break. After `cmp_ready`, the order must be:
    //   bucket 0 (P0/P1): oldest-first then id, then bucket 1 (P2..P4): oldest-first then id.
    let mut candidates = [
        issue("ub-p2-old", Priority::MEDIUM, 100),
        issue("ub-p0-new", Priority::CRITICAL, 300),
        issue("ub-p1-old-b", Priority::HIGH, 200),
        issue("ub-p1-old-a", Priority::HIGH, 200), // same bucket+age as -b: id breaks the tie
        issue("ub-p0-age200", Priority::CRITICAL, 200), // P0 shares bucket 0 + age 200 with the P1s:
        // cross-priority bucket-collapse, broken only by id
        issue("ub-p4-mid", Priority::BACKLOG, 150),
        issue("ub-p0-old", Priority::CRITICAL, 50),
    ];
    candidates.sort_by(cmp_ready);

    let ranked: Vec<&str> = candidates.iter().map(|i| i.id.as_str()).collect();
    insta::assert_json_snapshot!(ranked);
}

proptest! {
    /// Totality: any two issues compare to exactly one of Less/Equal/Greater (always true for a
    /// total `Ordering`, but assert the comparator is callable on arbitrary input without panic).
    #[test]
    fn totality(a in arb_ready_issue(), b in arb_ready_issue()) {
        let ord = cmp_ready(&a, &b);
        prop_assert!(matches!(ord, Ordering::Less | Ordering::Equal | Ordering::Greater));
    }

    /// Antisymmetry: `cmp(a,b)` is the reverse of `cmp(b,a)`.
    #[test]
    fn antisymmetry(a in arb_ready_issue(), b in arb_ready_issue()) {
        prop_assert_eq!(cmp_ready(&a, &b), cmp_ready(&b, &a).reverse());
    }

    /// Reflexivity: an issue compared to itself is Equal.
    #[test]
    fn reflexivity(a in arb_ready_issue()) {
        prop_assert_eq!(cmp_ready(&a, &a), Ordering::Equal);
    }

    /// Transitivity: if a<=b and b<=c then a<=c.
    #[test]
    fn transitivity(a in arb_ready_issue(), b in arb_ready_issue(), c in arb_ready_issue()) {
        if cmp_ready(&a, &b) != Ordering::Greater && cmp_ready(&b, &c) != Ordering::Greater {
            prop_assert_ne!(cmp_ready(&a, &c), Ordering::Greater);
        }
    }

    /// `cmp_ready` agrees with comparing the two `ReadySortKey`s.
    #[test]
    fn consistent_with_sort_key(a in arb_ready_issue(), b in arb_ready_issue()) {
        prop_assert_eq!(cmp_ready(&a, &b), ready_sort_key(&a).cmp(&ready_sort_key(&b)));
    }

    /// Determinism: sorting the same multiset twice yields the same order.
    #[test]
    fn deterministic_sort(items in proptest::collection::vec(arb_ready_issue(), 0..20)) {
        let mut first = items.clone();
        first.sort_by(cmp_ready);
        let mut second = items;
        second.sort_by(cmp_ready);
        let first_ids: Vec<&str> = first.iter().map(|i| i.id.as_str()).collect();
        let second_ids: Vec<&str> = second.iter().map(|i| i.id.as_str()).collect();
        prop_assert_eq!(first_ids, second_ids);
    }

    /// A unique-id input has a strict total order: no two distinct elements compare Equal.
    #[test]
    fn unique_ids_break_all_ties(items in proptest::collection::vec(arb_ready_issue(), 1..15)) {
        // Force unique ids so the `id` tie-break must resolve every pair.
        let unique: Vec<Issue> = items
            .into_iter()
            .enumerate()
            .map(|(idx, mut issue)| {
                issue.id = format!("ub-uniq-{idx:04}");
                issue
            })
            .collect();
        for (i, a) in unique.iter().enumerate() {
            for b in &unique[i + 1..] {
                prop_assert_ne!(cmp_ready(a, b), Ordering::Equal);
            }
        }
    }
}
