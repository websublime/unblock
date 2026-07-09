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

pub use code::{ErrorCode, HintShape};
pub use coded::CodedError;
pub use hints::{
    MAX_SUGGESTION_DISTANCE, PRIORITY_DETAIL_HINT, PRIORITY_SHORT_HINT, VALID_STATUS_HINT,
    VALID_TYPE_HINT, detect_priority_intent, detect_status_intent, detect_type_intent,
    find_similar_ids, levenshtein_distance,
};
pub use model::{FieldError, ModelError};
pub use sanitize::sanitize_message;
pub use structured::{ExitCode, StructuredError};

/// The `tracing` target every reliability event/span is emitted on (NFR-13/D30).
///
/// Hoisted to this L0 crate (T3.4/D30) so `unblock-sync` (the L3 emit site), `unblock-engine`
/// (`init_tracing`'s `EnvFilter` directive, via a re-export) and `unblock-cli` all reference the
/// SAME one const — the subscriber's filter-target can never diverge from the emit-target (the
/// silent-drop hazard a by-value duplicate would create). A zero-dep string const with no natural
/// module home, so it lives at the crate root.
pub const RELIABILITY_TARGET: &str = "unblock.reliability";

#[cfg(test)]
mod tests {
    use super::RELIABILITY_TARGET;

    #[test]
    fn reliability_target_is_pinned() {
        // The single source of the NFR-13 tracing-target name (engine re-exports, sync/cli import).
        assert_eq!(RELIABILITY_TARGET, "unblock.reliability");
    }
}
