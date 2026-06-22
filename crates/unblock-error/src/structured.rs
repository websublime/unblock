//! The structured JSON error payload ([`StructuredError`]) and the [`ExitCode`] newtype
//! (spine §2.4). This is the shape the L7 boundary serializes to stdout (CLI) or attaches as MCP
//! error data — always valid JSON even on error (FR-11).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::code::ErrorCode;
use crate::coded::CodedError;
use crate::sanitize::sanitize_message;

/// A machine-parseable, agent-friendly structured error (spine §2.4; FR-11).
///
/// Carries a stable [`ErrorCode`], a terminal-sanitized human message, an optional
/// (terminal-sanitized) self-correction `hint`, a `retryable` flag (mirrors
/// [`ErrorCode::is_retryable`]), and a free-form `context` object. Both `message` and `hint` are
/// always routed through [`crate::sanitize_message`] by every constructor/builder, so neither can
/// carry raw control bytes (spine §2.4 chokepoint). `context` values are terminal-safe only via
/// JSON encoding — a text/plain render of any context value must route through a sanitizer.
///
/// Serializes to valid JSON in every state: an empty `context` is omitted, and a `None` `hint`
/// is omitted, so the payload stays compact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StructuredError {
    /// The stable error code (serialized as `SCREAMING_SNAKE_CASE`).
    pub code: ErrorCode,
    /// Human-readable, terminal-sanitized message.
    pub message: String,
    /// Optional agent self-correction guidance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Whether the failing operation is potentially retryable (`== code.is_retryable()`).
    pub retryable: bool,
    /// Optional structured detail (e.g. `context["fields"]` for validation aggregates).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub context: Map<String, Value>,
}

impl StructuredError {
    /// Build a structured error from a code and a message.
    ///
    /// Sets `retryable = code.is_retryable()`, leaves `hint`/`context` empty, and routes the
    /// `message` through [`crate::sanitize_message`].
    ///
    /// # Examples
    ///
    /// ```
    /// use unblock_error::{ErrorCode, StructuredError};
    /// let err = StructuredError::from_code(ErrorCode::IssueNotFound, "Issue not found: ub-abc");
    /// assert_eq!(err.exit_code(), 3);
    /// assert!(!err.retryable);
    /// ```
    #[must_use]
    pub fn from_code(code: ErrorCode, message: impl Into<String>) -> Self {
        let message = sanitize_message(&message.into()).into_owned();
        Self {
            code,
            message,
            hint: None,
            retryable: code.is_retryable(),
            context: Map::new(),
        }
    }

    /// Build a structured error from any [`CodedError`] (dynamic-dispatch entry point).
    ///
    /// Pulls `code`/`hint`/`retryable`/`context` from the trait, uses [`std::fmt::Display`] for
    /// the message, and routes **both** the message and the `hint` through
    /// [`crate::sanitize_message`] (the §2.4 chokepoint covers message AND hint). This is the
    /// `&dyn CodedError` path the L7 boundary uses; the blanket `From<&E>` (see
    /// [`crate::CodedError`]) is the generic counterpart.
    #[must_use]
    pub fn from_coded<E>(err: &E) -> Self
    where
        E: CodedError + std::fmt::Display + ?Sized,
    {
        let code = err.code();
        Self {
            code,
            message: sanitize_message(&err.to_string()).into_owned(),
            hint: err.hint().map(|hint| sanitize_message(&hint).into_owned()),
            retryable: err.retryable(),
            context: err.context(),
        }
    }

    /// Attach a self-correction `hint` (builder).
    ///
    /// The `hint` is routed through [`crate::sanitize_message`] — `find_similar_ids` can fold an
    /// attacker-influenced not-found id into a suggestion, so the suggested text is untrusted and
    /// must be terminal-safe even for a plain-text TTY renderer (the §2.4 chokepoint, NFR-14).
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(sanitize_message(&hint.into()).into_owned());
        self
    }

    /// Insert a `context` key/value (builder).
    #[must_use]
    pub fn with_context(mut self, key: impl Into<String>, value: Value) -> Self {
        self.context.insert(key.into(), value);
        self
    }

    /// The 0–8 exit code (= [`ErrorCode::exit_code`]).
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        self.code.exit_code()
    }
}

/// A process exit code in the 0..=8 range (spine §2.2/§2.3).
///
/// The CLI casts this to the platform `i32` / [`std::process::ExitCode`] at the L7 boundary.
/// `#[must_use]` on the type means a constructed `ExitCode` (incl. via `From<ErrorCode>`) cannot
/// be silently dropped.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExitCode(pub u8);

impl ExitCode {
    /// Success (`0`) — emitted by no [`ErrorCode`], reserved for the no-error path.
    pub const EXIT_SUCCESS: Self = Self(0);
    /// Internal / unknown failure (`1`) — the fallback when no more specific code applies.
    pub const EXIT_INTERNAL: Self = Self(1);
}

impl From<ErrorCode> for ExitCode {
    fn from(code: ErrorCode) -> Self {
        Self(code.exit_code())
    }
}

#[cfg(test)]
mod tests {
    use super::{ExitCode, StructuredError};
    use crate::code::ErrorCode;
    use serde_json::json;

    #[test]
    fn from_code_sets_retryable_from_code() {
        let err = StructuredError::from_code(ErrorCode::AlreadyClaimed, "lost the race");
        assert!(err.retryable);
        assert_eq!(err.exit_code(), 3);

        let err = StructuredError::from_code(ErrorCode::PolicyViolation, "gate fired");
        assert!(!err.retryable);
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn from_code_sanitizes_message() {
        let err = StructuredError::from_code(ErrorCode::InternalError, "boom\x1b[2Jwipe");
        assert!(!err.message.contains('\x1b'));
        assert!(err.message.contains("\\u{1b}[2Jwipe"));
    }

    #[test]
    fn empty_context_and_none_hint_are_omitted() {
        let err = StructuredError::from_code(ErrorCode::IssueNotFound, "nope");
        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["code"], "ISSUE_NOT_FOUND");
        assert!(value.get("hint").is_none());
        assert!(value.get("context").is_none());
    }

    #[test]
    fn builders_populate_hint_and_context() {
        let err = StructuredError::from_code(ErrorCode::InvalidPriority, "bad")
            .with_hint("Priority must be 0-4")
            .with_context("provided", json!("high"));
        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["hint"], "Priority must be 0-4");
        assert_eq!(value["context"]["provided"], "high");
    }

    #[test]
    fn with_hint_sanitizes_control_chars() {
        let err = StructuredError::from_code(ErrorCode::IssueNotFound, "nope")
            .with_hint("Did you mean '\x1b[2Kub-x'?\x07");
        let hint = err.hint.as_deref().unwrap();
        assert!(!hint.contains('\x1b'));
        assert!(!hint.contains('\x07'));
        assert!(hint.contains("\\u{1b}[2Kub-x"));
        assert!(hint.contains("\\u{7}"));
    }

    #[test]
    fn exit_code_newtype_from_error_code() {
        assert_eq!(ExitCode::from(ErrorCode::CycleDetected), ExitCode(5));
        assert_eq!(ExitCode::EXIT_SUCCESS, ExitCode(0));
        assert_eq!(ExitCode::EXIT_INTERNAL, ExitCode(1));
    }

    #[test]
    fn round_trips_through_json() {
        let err = StructuredError::from_code(ErrorCode::ValidationFailed, "invalid")
            .with_hint("fix it")
            .with_context("field", json!("title"));
        let text = serde_json::to_string(&err).unwrap();
        let back: StructuredError = serde_json::from_str(&text).unwrap();
        assert_eq!(err, back);
    }
}
