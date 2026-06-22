//! Contract that the `CodedError → StructuredError` bridge is uniform and object-safe (spine
//! §2.1) — the seam the L7 boundary relies on. Exercises both entry points (the inherent
//! `from_coded(&dyn CodedError)` and the blanket `From<&E>`), the message-sanitization chokepoint,
//! and the char-vs-byte boundary.

use serde_json::{Map, Value, json};
use std::fmt;
use unblock_error::{CodedError, ErrorCode, StructuredError};

// Object-safety guard: only compiles if `CodedError` is object-safe.
fn _accepts_dyn(_: &dyn CodedError) {}

/// A stand-in for a downstream per-crate error (mirrors a `StorageError`/`EngineError` shape).
#[derive(Debug)]
struct StorageLikeError {
    code: ErrorCode,
    display: String,
    context: Map<String, Value>,
}

impl fmt::Display for StorageLikeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display)
    }
}

impl std::error::Error for StorageLikeError {}

impl CodedError for StorageLikeError {
    fn code(&self) -> ErrorCode {
        self.code
    }

    fn context(&self) -> Map<String, Value> {
        self.context.clone()
    }
}

fn sample(code: ErrorCode, display: &str) -> StorageLikeError {
    let mut context = Map::new();
    context.insert("origin".to_string(), json!("storage"));
    StorageLikeError {
        code,
        display: display.to_string(),
        context,
    }
}

#[test]
fn both_paths_produce_the_same_structured_error() {
    let err = sample(ErrorCode::DatabaseLocked, "database locked");

    let via_inherent = StructuredError::from_coded(&err);
    let via_blanket: StructuredError = (&err).into();

    assert_eq!(via_inherent, via_blanket);
    assert_eq!(via_inherent.code, ErrorCode::DatabaseLocked);
    assert_eq!(via_inherent.message, "database locked");
    assert_eq!(via_inherent.context["origin"], "storage");
}

#[test]
fn default_retryable_tracks_code() {
    let retryable = StructuredError::from_coded(&sample(ErrorCode::AlreadyClaimed, "lost claim"));
    assert!(retryable.retryable);

    let non_retryable = StructuredError::from_coded(&sample(ErrorCode::PolicyViolation, "gate"));
    assert!(!non_retryable.retryable);
}

#[test]
fn display_with_esc_and_bel_is_sanitized_on_both_paths() {
    let err = sample(ErrorCode::InternalError, "alert\x07then\x1b[31mred");

    let via_inherent = StructuredError::from_coded(&err);
    let via_blanket: StructuredError = (&err).into();

    for structured in [&via_inherent, &via_blanket] {
        assert!(!structured.message.contains('\x07'));
        assert!(!structured.message.contains('\x1b'));
        assert!(structured.message.contains("\\u{7}"));
        assert!(structured.message.contains("\\u{1b}[31mred"));
    }
    assert_eq!(via_inherent.message, via_blanket.message);
}

#[test]
fn four_byte_emoji_title_counts_chars_not_bytes() {
    // A title built from a 4-byte UTF-8 emoji is not a control sequence; sanitization must leave
    // it intact (the boundary counts chars, not bytes). 4 emojis = 16 bytes but 4 chars.
    let emoji_title = "\u{1f980}".repeat(4);
    assert_eq!(emoji_title.len(), 16, "4 emojis are 16 UTF-8 bytes");
    assert_eq!(emoji_title.chars().count(), 4, "but only 4 chars");

    let err = sample(ErrorCode::ValidationFailed, &emoji_title);
    let structured = StructuredError::from_coded(&err);
    assert_eq!(structured.message, emoji_title);
    assert!(!structured.message.contains('\\'));
}
