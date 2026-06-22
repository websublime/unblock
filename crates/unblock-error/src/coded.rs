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
            hint: err.hint(),
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

    #[derive(Debug)]
    struct FakeError {
        code: ErrorCode,
        message: String,
    }

    impl fmt::Display for FakeError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for FakeError {}

    impl CodedError for FakeError {
        fn code(&self) -> ErrorCode {
            self.code
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
            code: ErrorCode::DatabaseLocked,
            message: "locked".to_string(),
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
            code: ErrorCode::IssueNotFound,
            message: "missing".to_string(),
        };
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::IssueNotFound);
        assert!(!structured.retryable);
    }

    #[test]
    fn display_with_control_chars_is_sanitized() {
        let err = FakeError {
            code: ErrorCode::InternalError,
            message: "alert\x07\x1b[31mred".to_string(),
        };
        let structured = StructuredError::from_coded(&err);
        assert!(!structured.message.contains('\x07'));
        assert!(!structured.message.contains('\x1b'));
        assert!(structured.message.contains("\\u{7}"));
        assert!(structured.message.contains("\\u{1b}[31mred"));

        let via_blanket: StructuredError = (&err).into();
        assert_eq!(structured.message, via_blanket.message);
    }
}
