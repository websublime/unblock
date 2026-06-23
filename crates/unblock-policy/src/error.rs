//! [`PolicyError`] — the per-crate error enum for `unblock-policy` (spine §2.1; plan §2 `error.rs`).
//!
//! **v1 variant set = [`PolicyError::Internal`] only.** It is a deliberate forward-compat *seam*:
//! no v1 policy primitive is fallible (ready-ranking, cache-key minting, and the contract helpers
//! return values directly, and the v1 inheritance helper is infallible). The seam is kept — rather
//! than shipping no error type at all — so the [`CodedError`] plumbing and the variant→[`ErrorCode`]
//! golden snapshot exist from v1, and the gate/evidence variants
//! (`InvalidPolicyDocument`/`MissingRequiredEvidence`/…) can be added **additively** in v1.1 where
//! the code that raises them lands (plan Q2; resolved with Miguel).
//!
//! It implements [`unblock_error::CodedError`] — only [`CodedError::code`] is overridden; `hint`,
//! `retryable`, and `context` keep their defaults — so the L7 boundary bridges it via the blanket
//! `From<&E>` impl in `unblock-error` with no per-type glue.

use snafu::Snafu;
use unblock_error::{CodedError, ErrorCode};

/// The error type returned by fallible `unblock-policy` operations (spine §2.1).
///
/// In v1 this carries a single [`PolicyError::Internal`] variant — a forward-compat seam that **no
/// v1 code path raises** (every v1 primitive is infallible). It maps to
/// [`ErrorCode::InternalError`] (exit 1). The gate/saved-query/evidence variants are introduced
/// additively in v1.1 at the call sites that raise them.
///
/// # Examples
///
/// ```
/// use unblock_policy::PolicyError;
/// use unblock_error::{CodedError, ErrorCode};
///
/// assert_eq!(PolicyError::Internal.code(), ErrorCode::InternalError);
/// // `Internal` is not retryable (it tracks `InternalError::is_retryable()`).
/// assert!(!PolicyError::Internal.retryable());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum PolicyError {
    /// An unexpected internal policy error (a forward-compat seam; unraised in v1).
    #[snafu(display("internal policy error"))]
    Internal,
}

impl CodedError for PolicyError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Internal => ErrorCode::InternalError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PolicyError;
    use unblock_error::{CodedError, ErrorCode, StructuredError};

    #[test]
    fn internal_maps_to_internal_error() {
        assert_eq!(PolicyError::Internal.code(), ErrorCode::InternalError);
    }

    #[test]
    fn internal_is_not_retryable_by_default() {
        // `retryable` defaults to `code().is_retryable()`; `InternalError` is not retryable.
        assert!(!PolicyError::Internal.retryable());
    }

    #[test]
    fn display_is_terminal_sanitized_and_non_empty() {
        // The message is a fixed literal with no control bytes, so bridging leaves it intact.
        let err = PolicyError::Internal;
        assert!(!err.to_string().is_empty());
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::InternalError);
        assert_eq!(structured.message, "internal policy error");
        assert!(!structured.retryable);
    }

    #[test]
    fn golden_variant_to_code_table() {
        // The full v1 variant->code table (just `Internal => INTERNAL_ERROR`), pinned so any
        // additive v1.1 variant forces a deliberate re-bless (mirrors the §2.3 discipline at L1).
        let table: Vec<(&'static str, &'static str)> = [PolicyError::Internal]
            .iter()
            .map(|e| {
                let name = match e {
                    PolicyError::Internal => "Internal",
                };
                (name, e.code().as_str())
            })
            .collect();
        insta::assert_json_snapshot!(table);
    }
}
