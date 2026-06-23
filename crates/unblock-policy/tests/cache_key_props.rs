//! Property tests for cache-key minting + filter fingerprinting (plan §3; NFR-16).
//!
//! - [`filters_fingerprint`] is order-insensitive (set-field reordering/duplication is irrelevant)
//!   and idempotent;
//! - logically-equal filters fingerprint equal; a field difference fingerprints different;
//! - the `ready:` and `blocked:` namespaces never collide for the same fingerprint.

use proptest::prelude::*;

use unblock_model::ListFilters;
use unblock_policy::proptest_support::{arb_list_filters, arb_list_filters_with_permutation};
use unblock_policy::{cache_key_blocked, cache_key_ready, filters_fingerprint};

/// An INDEPENDENT canonical key capturing the fingerprint's notion of logical equality (set fields
/// sorted + deduped via `as_str`; scalars structural). Built structurally — no string delimiters —
/// so it cannot share a delimiter-forging bug with `filters_fingerprint`: a genuine differential
/// oracle. Two filters share this key iff they are logically equal.
#[allow(clippy::type_complexity)]
fn canon_key(
    f: &ListFilters,
) -> (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Option<String>,
    Option<String>,
    Option<i32>,
    Option<i32>,
    bool,
    bool,
    Option<usize>,
    Option<usize>,
) {
    fn norm(mut v: Vec<String>) -> Vec<String> {
        v.sort_unstable();
        v.dedup();
        v
    }
    (
        norm(f.status.iter().map(|s| s.as_str().to_string()).collect()),
        norm(
            f.issue_type
                .iter()
                .map(|t| t.as_str().to_string())
                .collect(),
        ),
        norm(f.labels_all.clone()),
        norm(f.labels_any.clone()),
        f.assignee.clone(),
        f.text_contains.clone(),
        f.priority_min.map(|p| p.0),
        f.priority_max.map(|p| p.0),
        f.include_deferred,
        f.include_closed,
        f.limit,
        f.offset,
    )
}

proptest! {
    /// Reordering + duplicating the set fields does not change the fingerprint.
    #[test]
    fn fingerprint_is_order_insensitive((a, b) in arb_list_filters_with_permutation()) {
        prop_assert_eq!(filters_fingerprint(&a), filters_fingerprint(&b));
    }

    /// Fingerprinting the same filters twice yields the same string.
    #[test]
    fn fingerprint_is_idempotent(filters in arb_list_filters()) {
        prop_assert_eq!(filters_fingerprint(&filters), filters_fingerprint(&filters));
    }

    /// `ready` and `blocked` keys built from the same fingerprint never collide.
    #[test]
    fn ready_and_blocked_never_collide(filters in arb_list_filters()) {
        let fp = filters_fingerprint(&filters);
        prop_assert_ne!(cache_key_ready(&fp), cache_key_blocked(&fp));
    }

    /// A `ready` key is deterministic for a fixed fingerprint.
    #[test]
    fn cache_key_is_deterministic(filters in arb_list_filters()) {
        let fp = filters_fingerprint(&filters);
        prop_assert_eq!(cache_key_ready(&fp), cache_key_ready(&fp));
    }

    /// INJECTIVITY (the anti-collision contract): two filters fingerprint equal **iff** they are
    /// logically equal (per the independent `canon_key` oracle). This catches a future field-section
    /// drop/conflation in `filters_fingerprint` (e.g. forgetting to emit `offset`) that the
    /// hand-picked unit cases would miss — the highest-risk correctness area, pinned generatively.
    #[test]
    fn fingerprint_is_injective(a in arb_list_filters(), b in arb_list_filters()) {
        prop_assert_eq!(
            filters_fingerprint(&a) == filters_fingerprint(&b),
            canon_key(&a) == canon_key(&b)
        );
    }
}
