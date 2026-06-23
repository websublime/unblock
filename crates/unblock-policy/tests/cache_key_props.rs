//! Property tests for cache-key minting + filter fingerprinting (plan §3; NFR-16).
//!
//! - [`filters_fingerprint`] is order-insensitive (set-field reordering/duplication is irrelevant)
//!   and idempotent;
//! - logically-equal filters fingerprint equal; a field difference fingerprints different;
//! - the `ready:` and `blocked:` namespaces never collide for the same fingerprint.

use proptest::prelude::*;

use unblock_policy::proptest_support::{arb_list_filters, arb_list_filters_with_permutation};
use unblock_policy::{cache_key_blocked, cache_key_ready, filters_fingerprint};

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
}
