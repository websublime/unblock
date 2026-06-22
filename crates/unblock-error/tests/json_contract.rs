//! Boundary JSON-shape contract for [`StructuredError`] (spine §2.4; FR-11 "always valid JSON even
//! on error").

use proptest::prelude::*;
use serde_json::json;
use unblock_error::{ErrorCode, StructuredError, sanitize_message};

#[test]
fn code_is_screaming_snake_string() {
    let err = StructuredError::from_code(ErrorCode::IssueNotFound, "missing");
    let value = serde_json::to_value(&err).unwrap();
    assert_eq!(value["code"], "ISSUE_NOT_FOUND");
}

#[test]
fn empty_context_and_none_hint_are_omitted() {
    let err = StructuredError::from_code(ErrorCode::NothingToDo, "nothing");
    let value = serde_json::to_value(&err).unwrap();
    assert!(value.get("hint").is_none());
    assert!(value.get("context").is_none());
}

#[test]
fn full_payload_round_trips() {
    let err = StructuredError::from_code(ErrorCode::ValidationFailed, "invalid")
        .with_hint("fix the title")
        .with_context("fields", json!([{ "field": "title", "reason": "cannot be empty" }]));
    let text = serde_json::to_string(&err).unwrap();
    let back: StructuredError = serde_json::from_str(&text).unwrap();
    assert_eq!(err, back);
}

#[test]
fn one_payload_per_category_round_trips() {
    let samples = [
        ErrorCode::DatabaseLocked,
        ErrorCode::IssueNotFound,
        ErrorCode::ValidationFailed,
        ErrorCode::CycleDetected,
        ErrorCode::JsonlParseError,
        ErrorCode::ConfigError,
        ErrorCode::IoError,
        ErrorCode::InternalError,
    ];
    for code in samples {
        let err = StructuredError::from_code(code, "sample message");
        let text = serde_json::to_string(&err).unwrap();
        let back: StructuredError = serde_json::from_str(&text).unwrap();
        assert_eq!(err, back);
        assert_eq!(back.retryable, code.is_retryable());
    }
}

#[test]
fn golden_serialized_error() {
    let err = StructuredError::from_code(ErrorCode::IssueNotFound, "Issue not found: ub-abc")
        .with_hint("Did you mean 'ub-abd'?")
        .with_context("searched_id", json!("ub-abc"));
    insta::assert_json_snapshot!(err);
}

proptest! {
    /// Building and serializing a `StructuredError` never panics, even for messages full of
    /// control characters, and the serialized message carries no raw control byte except the
    /// layout characters `\n`/`\t`.
    #[test]
    fn never_panics_and_message_is_sanitized(message in ".{0,256}") {
        let err = StructuredError::from_code(ErrorCode::InternalError, message.as_str());
        let text = serde_json::to_string(&err).expect("serializes");
        let back: StructuredError = serde_json::from_str(&text).expect("round-trips");
        prop_assert_eq!(&err, &back);

        for ch in err.message.chars() {
            if ch.is_control() {
                prop_assert!(matches!(ch, '\n' | '\t'), "raw control char leaked: {:?}", ch);
            }
        }

        // The constructor's sanitization must be idempotent with the public sanitizer.
        prop_assert_eq!(sanitize_message(&err.message).into_owned(), err.message);
    }
}
