//! Golden contract suite for the 0–8 exit-code table (spine §2.3; FR-11) + the static hint-shape
//! taxonomy (spine §2.2; D25/FORK-4B).
//!
//! Asserted at the crate boundary (independent of the unit tests in `code.rs`): every
//! `ErrorCode` maps to an exact `(as_str, exit_code, is_retryable, hint_shape)` quadruple, the emitted
//! exit codes cover exactly {1,2,3,4,5,6,7,8}, `0` is emitted by no code, and `AlreadyClaimed` is the
//! explicit `(exit 3, retryable)` carrier (FR-2). The full 36-quadruple table is insta-pinned, so any
//! unintentional change to the vocabulary fails the snapshot gate.

use std::collections::{BTreeSet, HashSet};
use unblock_error::{
    ErrorCode, HintShape, PRIORITY_DETAIL_HINT, VALID_STATUS_HINT, VALID_TYPE_HINT,
};

#[test]
fn all_array_has_36_unique_variants() {
    assert_eq!(ErrorCode::ALL.len(), 36, "the table is pinned at 36 codes");
    let unique: HashSet<_> = ErrorCode::ALL.iter().copied().collect();
    assert_eq!(unique.len(), 36, "ALL must contain no duplicates");
}

#[test]
fn every_exit_code_is_in_range_and_nonzero() {
    for code in ErrorCode::ALL {
        let exit = code.exit_code();
        assert!(
            (1..=8).contains(&exit),
            "{} has out-of-range exit {exit}",
            code.as_str()
        );
    }
}

#[test]
fn exit_codes_cover_one_through_eight_and_never_zero() {
    let emitted: BTreeSet<u8> = ErrorCode::ALL.iter().map(|c| c.exit_code()).collect();
    let expected: BTreeSet<u8> = (1..=8).collect();
    assert_eq!(emitted, expected, "exit codes must cover exactly 1..=8");
    assert!(!emitted.contains(&0), "0 is reserved for success");
}

#[test]
fn already_claimed_is_exit_three_and_retryable() {
    assert_eq!(ErrorCode::AlreadyClaimed.exit_code(), 3);
    assert!(ErrorCode::AlreadyClaimed.is_retryable());
}

#[test]
fn rate_limited_is_exit_two_and_retryable() {
    // NFR-18/D34 (OQ-2 ratified): the MCP concurrency-cap reject is exit 2 (the only {1..8} bucket
    // carrying "resource busy, retry" — a 9th exit code would break the pinned coverage) and retryable.
    assert_eq!(ErrorCode::RateLimited.exit_code(), 2);
    assert!(ErrorCode::RateLimited.is_retryable());
}

#[test]
fn as_str_matches_serde_string_for_every_variant() {
    for code in ErrorCode::ALL {
        let serialized = serde_json::to_string(&code).expect("code serializes");
        // serde emits a JSON string literal, e.g. "\"ISSUE_NOT_FOUND\"".
        let expected = format!("\"{}\"", code.as_str());
        assert_eq!(serialized, expected, "as_str must match serde for {code:?}");
    }
}

#[test]
fn golden_exit_code_table() {
    let table: Vec<(&'static str, u8, bool, &'static str)> = ErrorCode::ALL
        .iter()
        .map(|c| {
            (
                c.as_str(),
                c.exit_code(),
                c.is_retryable(),
                c.hint_shape().as_str(),
            )
        })
        .collect();
    insta::assert_json_snapshot!(table);
}

#[test]
fn hint_shape_counts_are_the_honest_map() {
    // The D25/FORK-4B honest map: exactly 3 StaticText + 1 ContextualText + 1 SimilarIds; the rest None.
    let mut static_text = 0;
    let mut contextual = 0;
    let mut similar = 0;
    for code in ErrorCode::ALL {
        match code.hint_shape() {
            HintShape::StaticText => static_text += 1,
            HintShape::ContextualText => contextual += 1,
            HintShape::SimilarIds => similar += 1,
            HintShape::None => {}
        }
    }
    assert_eq!(static_text, 3, "exactly 3 StaticText codes");
    assert_eq!(contextual, 1, "exactly 1 ContextualText code");
    assert_eq!(similar, 1, "exactly 1 SimilarIds code");
}

#[test]
fn static_hint_texts_equal_the_exported_consts() {
    assert_eq!(
        ErrorCode::InvalidStatus.static_hint(),
        Some(VALID_STATUS_HINT)
    );
    assert_eq!(ErrorCode::InvalidType.static_hint(), Some(VALID_TYPE_HINT));
    assert_eq!(
        ErrorCode::InvalidPriority.static_hint(),
        Some(PRIORITY_DETAIL_HINT)
    );
}

#[test]
fn static_hint_is_some_iff_hint_shape_is_static_text() {
    // The D25 coherence invariant, exhaustively over ALL (holds by construction).
    for code in ErrorCode::ALL {
        assert_eq!(
            code.hint_shape() == HintShape::StaticText,
            code.static_hint().is_some(),
            "hint_shape==StaticText must iff static_hint().is_some() for {}",
            code.as_str()
        );
    }
}
