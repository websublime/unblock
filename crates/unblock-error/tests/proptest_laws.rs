//! Property tests for the crate invariants (NFR-16): `exit_code` totality, `sanitize_message`
//! laws, and the Levenshtein metric laws.

use proptest::prelude::*;
use unblock_error::{ErrorCode, levenshtein_distance, sanitize_message};

/// Strategy over any constructible `ErrorCode` (drawn from the exhaustive `ALL` table).
fn any_code() -> impl Strategy<Value = ErrorCode> {
    (0usize..ErrorCode::ALL.len()).prop_map(|i| ErrorCode::ALL[i])
}

proptest! {
    /// `exit_code()` is total and in-range (1..=8) for every constructible variant.
    #[test]
    fn exit_code_is_total_and_in_range(code in any_code()) {
        let exit = code.exit_code();
        prop_assert!((1..=8).contains(&exit));
        // `retryable` is also total and never panics.
        let _ = code.is_retryable();
        // `as_str` is non-empty.
        prop_assert!(!code.as_str().is_empty());
    }

    /// Sanitization never emits a raw control byte other than `\n`/`\t`, and is idempotent.
    #[test]
    fn sanitize_message_laws(text in ".{0,256}") {
        let once = sanitize_message(&text).into_owned();
        for ch in once.chars() {
            if ch.is_control() {
                prop_assert!(matches!(ch, '\n' | '\t'));
            }
        }
        let twice = sanitize_message(&once).into_owned();
        prop_assert_eq!(once, twice);
    }

    /// Levenshtein distance is symmetric, ≥ the length difference, and `0` iff the inputs are equal.
    #[test]
    fn levenshtein_metric_laws(a in ".{0,32}", b in ".{0,32}") {
        let d = levenshtein_distance(&a, &b);
        prop_assert_eq!(d, levenshtein_distance(&b, &a), "must be symmetric");

        let len_diff = a.chars().count().abs_diff(b.chars().count());
        prop_assert!(d >= len_diff, "distance must be at least the length difference");

        prop_assert_eq!(d == 0, a == b, "distance is 0 iff the strings are equal");
    }

    /// The triangle inequality holds: d(a, c) ≤ d(a, b) + d(b, c).
    #[test]
    fn levenshtein_triangle_inequality(a in ".{0,16}", b in ".{0,16}", c in ".{0,16}") {
        let ac = levenshtein_distance(&a, &c);
        let ab = levenshtein_distance(&a, &b);
        let bc = levenshtein_distance(&b, &c);
        prop_assert!(ac <= ab + bc);
    }
}
