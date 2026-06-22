//! The [`CodedError`] bridge — the uniform `code()` view every per-crate error enum implements,
//! plus the blanket conversion into [`StructuredError`] (spine §2.1).
//!
//! There are two distinct entry points and **no hand-written `From<&Concrete>` impls**:
//!
//! 1. The inherent [`StructuredError::from_coded`] takes any `&E: CodedError + Display`
//!    (the `&dyn CodedError` path the L7 boundary holds).
//! 2. The blanket [`From`]`<&E>` below covers every `E: CodedError + std::error::Error`, giving
//!    ergonomic `?` / `.into()` at call sites.
//!
//! Keeping these as the only two paths means a new per-crate enum gets the bridge for free by
//! implementing `CodedError` — no per-type glue to forget.

use serde_json::{Map, Value};

use crate::code::ErrorCode;
use crate::sanitize::sanitize_message;
use crate::structured::StructuredError;

/// The uniform error view that lets the L7 boundary build a [`StructuredError`] from any composed
/// per-crate error (spine §2.1).
///
/// The trait is **object-safe**: every method takes `&self` and returns an owned value, so the
/// boundary can hold a `&dyn CodedError`. Only [`CodedError::code`] is required; `hint`,
/// `retryable`, and `context` have sensible defaults (`retryable` tracks
/// [`ErrorCode::is_retryable`]).
pub trait CodedError {
    /// The stable [`ErrorCode`] this error maps to.
    fn code(&self) -> ErrorCode;

    /// Optional agent self-correction hint (default: none).
    fn hint(&self) -> Option<String> {
        None
    }

    /// Whether the failing operation is retryable (default: `self.code().is_retryable()`).
    fn retryable(&self) -> bool {
        self.code().is_retryable()
    }

    /// Optional structured context detail (default: empty).
    fn context(&self) -> Map<String, Value> {
        Map::new()
    }
}

impl<E> From<&E> for StructuredError
where
    E: CodedError + std::error::Error,
{
    fn from(err: &E) -> Self {
        let code = err.code();
        Self {
            code,
            message: sanitize_message(&err.to_string()).into_owned(),
            hint: err.hint().map(|hint| sanitize_message(&hint).into_owned()),
            retryable: err.retryable(),
            context: err.context(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CodedError;
    use crate::code::ErrorCode;
    use crate::structured::StructuredError;
    use serde_json::{Map, Value, json};
    use std::fmt;

    // Object-safety guard: this signature only compiles if `CodedError` is object-safe.
    fn _object_safe(_: &dyn CodedError) {}

    #[derive(Debug, Default)]
    struct FakeError {
        code: Option<ErrorCode>,
        message: String,
        hint: Option<String>,
    }

    impl fmt::Display for FakeError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for FakeError {}

    impl CodedError for FakeError {
        fn code(&self) -> ErrorCode {
            self.code.unwrap_or(ErrorCode::InternalError)
        }

        fn hint(&self) -> Option<String> {
            self.hint.clone()
        }

        fn context(&self) -> Map<String, Value> {
            let mut map = Map::new();
            map.insert("detail".to_string(), json!("extra"));
            map
        }
    }

    #[test]
    fn inherent_from_coded_path() {
        let err = FakeError {
            code: Some(ErrorCode::DatabaseLocked),
            message: "locked".to_string(),
            ..FakeError::default()
        };
        let structured = StructuredError::from_coded(&err);
        assert_eq!(structured.code, ErrorCode::DatabaseLocked);
        assert!(structured.retryable); // default tracks code.is_retryable()
        assert_eq!(structured.context["detail"], "extra");
        assert_eq!(structured.message, "locked");
    }

    #[test]
    fn blanket_from_path() {
        let err = FakeError {
            code: Some(ErrorCode::IssueNotFound),
            message: "missing".to_string(),
            ..FakeError::default()
        };
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::IssueNotFound);
        assert!(!structured.retryable);
    }

    #[test]
    fn display_with_control_chars_is_sanitized() {
        let err = FakeError {
            code: Some(ErrorCode::InternalError),
            message: "alert\x07\x1b[31mred".to_string(),
            ..FakeError::default()
        };
        let structured = StructuredError::from_coded(&err);
        assert!(!structured.message.contains('\x07'));
        assert!(!structured.message.contains('\x1b'));
        assert!(structured.message.contains("\\u{7}"));
        assert!(structured.message.contains("\\u{1b}[31mred"));

        let via_blanket: StructuredError = (&err).into();
        assert_eq!(structured.message, via_blanket.message);
    }

    #[test]
    fn hint_with_esc_and_bel_is_sanitized_on_both_paths() {
        let err = FakeError {
            code: Some(ErrorCode::IssueNotFound),
            message: "not found".to_string(),
            hint: Some("Did you mean '\x1b[2Kub-evil'?\x07\nsecond line".to_string()),
        };

        let via_inherent = StructuredError::from_coded(&err);
        let via_blanket: StructuredError = (&err).into();

        for structured in [&via_inherent, &via_blanket] {
            let hint = structured.hint.as_deref().expect("hint present");
            // No raw control byte except the preserved layout characters \n / \t.
            assert!(
                !hint
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\t')),
                "raw control char leaked into hint: {hint:?}"
            );
            assert!(hint.contains("\\u{1b}[2Kub-evil"));
            assert!(hint.contains("\\u{7}"));
            assert!(hint.contains('\n'), "newline must be preserved");
        }
        assert_eq!(via_inherent.hint, via_blanket.hint);
    }
}
