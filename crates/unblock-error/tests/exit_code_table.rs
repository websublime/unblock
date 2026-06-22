//! Golden contract suite for the 0–8 exit-code table (spine §2.3; FR-11).
//!
//! Asserted at the crate boundary (independent of the unit tests in `code.rs`): every
//! `ErrorCode` maps to an exact `(as_str, exit_code, is_retryable)` triple, the emitted exit
//! codes cover exactly {1,2,3,4,5,6,7,8}, `0` is emitted by no code, and `AlreadyClaimed` is the
//! explicit `(exit 3, retryable)` carrier (FR-2). The full 35-triple table is insta-pinned, so any
//! unintentional change to the vocabulary fails the snapshot gate.

use std::collections::{BTreeSet, HashSet};
use unblock_error::ErrorCode;

#[test]
fn all_array_has_35_unique_variants() {
    assert_eq!(ErrorCode::ALL.len(), 35, "the table is pinned at 35 codes");
    let unique: HashSet<_> = ErrorCode::ALL.iter().copied().collect();
    assert_eq!(unique.len(), 35, "ALL must contain no duplicates");
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
    let table: Vec<(&'static str, u8, bool)> = ErrorCode::ALL
        .iter()
        .map(|c| (c.as_str(), c.exit_code(), c.is_retryable()))
        .collect();
    insta::assert_json_snapshot!(table);
}
