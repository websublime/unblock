//! Reusable post-condition assertions shared by the fuzz cores.
//!
//! These check **invariants** (properties that must hold for *any* input): a breach is a real bug,
//! so they `assert!` (panic), which libFuzzer reports as a crash and the stable regression replay
//! reports as a test failure. They are distinct from the operational `?`-propagated
//! [`FuzzError`](crate::FuzzError) (a malformed input that simply does not reach the deep path is
//! **not** a bug — the core handles it and returns `Ok`).

// Every `assert_*` here is an invariant check that panics on a breach **by design** — that IS its
// contract (libFuzzer/the regression replay treat the panic as the bug report). A `# Panics` section
// on each would be noise, so the pedantic lint is scoped off for this assertion module.
#![allow(clippy::missing_panics_doc)]

use unblock_error::{CodedError, ErrorCode, StructuredError};
use unblock_model::Issue;

/// Assert a content hash is well-formed: exactly 64 lowercase-hex chars (spine §1.8).
pub fn assert_hash_well_formed(hash: &str) {
    assert_eq!(hash.len(), 64, "content_hash must be 64 chars: {hash:?}");
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "content_hash must be lowercase hex: {hash:?}"
    );
}

/// Assert the standard read-side surface of an `Issue` is panic-free + well-formed (mirrors the
/// `unblock-model` `proptest_panic_safety` surface): the hash is well-formed, `sync_equals` is
/// reflexive, and the tombstone TTL helper is total.
pub fn assert_issue_surface_well_formed(issue: &Issue) {
    assert_hash_well_formed(&issue.compute_content_hash());
    assert!(issue.sync_equals(issue), "sync_equals must be reflexive");
    // is_expired_tombstone is total for any retention, including absurd ones.
    let _ = issue.is_expired_tombstone(None);
    let _ = issue.is_expired_tombstone(Some(0));
    let _ = issue.is_expired_tombstone(Some(u64::MAX));
}

/// Assert a coded error bridges to a non-empty, well-formed [`StructuredError`]: the code round-trips
/// to a valid [`ErrorCode`], the (sanitized) message is non-empty, and `retryable` agrees with the
/// code (spine §2.2/§2.4).
pub fn assert_nonempty_structured_error<E: CodedError + std::fmt::Display + ?Sized>(error: &E) {
    let structured: StructuredError = StructuredError::from_coded(error);
    assert!(
        !structured.message.is_empty(),
        "a structured error must carry a non-empty message"
    );
    assert_eq!(
        structured.retryable,
        structured.code.is_retryable(),
        "retryable must equal code.is_retryable()"
    );
    // The exit code is in the 0..=8 table (a defined byte, never a panic).
    let exit = structured.exit_code();
    assert!(exit <= 8, "exit code {exit} out of the 0..=8 table");
    // The message + hint are terminal-sanitized: no raw control byte survives (NFR-14).
    assert_no_raw_control(&structured.message);
    if let Some(hint) = &structured.hint {
        assert_no_raw_control(hint);
    }
}

/// Assert text carries no raw terminal-control byte (only `\n`/`\t` layout is allowed through).
pub fn assert_no_raw_control(text: &str) {
    assert!(
        !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t')),
        "raw terminal-control byte leaked: {text:?}"
    );
}

/// Assert that a code maps to a defined `as_str` (never empty) — a cheap total-mapping check.
pub fn assert_code_total(code: ErrorCode) {
    assert!(
        !code.as_str().is_empty(),
        "ErrorCode::as_str must be defined"
    );
}
