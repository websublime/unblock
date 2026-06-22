//! `unblock-error` (L0) — the shared error boundary vocabulary.
//!
//! This is the deepest leaf crate: it has **no** in-workspace dependencies. It owns the stable
//! [`ErrorCode`] enum, the 0–8 exit-code table, the [`StructuredError`] JSON payload, the
//! [`CodedError`] bridge every per-crate error enum implements, the concrete [`ModelError`] that
//! `unblock-model` returns, terminal-message sanitization ([`sanitize_message`]), and the agent
//! self-correction hint helpers. Per the spine §2, mapping a composed error to an `ErrorCode` and
//! a 0–8 exit code happens only at the L7 boundary; this crate just provides the vocabulary.
//!
//! See `docs/plans/crates/unblock-error.md` for the per-file plan and `docs/plans/01-design-spine.md`
//! §2 for the interface contract.
//!
//! # Example
//!
//! ```
//! use unblock_error::{ErrorCode, StructuredError};
//!
//! let err = StructuredError::from_code(ErrorCode::IssueNotFound, "Issue not found: ub-abc")
//!     .with_hint("Run a list query to see available issues.");
//!
//! assert_eq!(err.code, ErrorCode::IssueNotFound);
//! assert_eq!(err.exit_code(), 3);
//! assert!(!err.retryable);
//!
//! // The payload is always valid JSON, even on error (FR-11).
//! let json = serde_json::to_string(&err).expect("serializes");
//! assert!(json.contains("\"code\":\"ISSUE_NOT_FOUND\""));
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod code;
mod coded;
mod hints;
mod model;
mod sanitize;
mod structured;

pub use code::ErrorCode;
pub use coded::CodedError;
pub use hints::{
    MAX_SUGGESTION_DISTANCE, PRIORITY_DETAIL_HINT, PRIORITY_SHORT_HINT, VALID_STATUS_HINT,
    VALID_TYPE_HINT, detect_priority_intent, detect_status_intent, detect_type_intent,
    find_similar_ids, levenshtein_distance,
};
pub use model::{FieldError, ModelError};
pub use sanitize::sanitize_message;
pub use structured::{ExitCode, StructuredError};
