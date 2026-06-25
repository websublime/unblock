//! [`EngineError`] — the engine's composed error, the union surfaced to L7 (spine §2, §4.1).
//!
//! `#[derive(Snafu)]` over the **three v1 source enums the engine actually produces** plus a small
//! set of engine-local variants. It implements [`unblock_error::CodedError`] (the §2.1 L0 bridge —
//! `code()` required; `hint`/`retryable`/`context` defaulted, `retryable()` tracking
//! `code().is_retryable()`) so the L7 boundary builds a `StructuredError` uniformly via the blanket
//! `From<&E>`.
//!
//! # v1 source set (precise — only enums that EXIST and the engine produces)
//!
//! - [`unblock_storage::StorageError`] — every read/mutation delegates to `Storage`.
//! - [`unblock_policy::PolicyError`] — the policy free-fn path (infallible in v1; the seam is
//!   wrapped so future fallible policy paths compose without a breaking change).
//! - [`unblock_error::ModelError`] — `IssueValidator::validate` runs **in the engine** on
//!   `create`/`update` *before* the storage delegation, so `ModelError` is unambiguously
//!   engine-produced and `StorageError` stays validation-free (it has no `ModelError` variant).
//!
//! **`ConfigError` is NOT wrapped** (CF-D): the engine never calls `unblock-config`; the caller
//! (cli/mcp/test) runs the open facade, handles `ConfigError`, and hands an already-built
//! `WorkspaceContext` to `Session::open`. A `ConfigError` is therefore never produced inside the
//! engine.
//!
//! # Engine-local variant → existing `ErrorCode` (spine §2.2 — never a NEW code)
//!
//! - `WorkspaceNotOpen`   → [`ErrorCode::NotInitialized`]
//! - `ShutdownInProgress` → [`ErrorCode::InternalError`]
//! - `WritePermitPoisoned`→ [`ErrorCode::InternalError`]
//! - `FeatureNotWired`    → [`ErrorCode::InternalError`]
//!
//! # Additive growth (NO v1 signature change)
//!
//! `Sync { source: SyncError }` lands with **T2.4** and `Health { source: HealthError }` with
//! **T3.3** (each forwarding `source.code()`); at that point `FeatureNotWired` is removed for those
//! methods. No backend type ever leaks (spine §6 rule 2).

use snafu::Snafu;
use unblock_error::{CodedError, ErrorCode, ModelError};
use unblock_policy::PolicyError;
use unblock_storage::StorageError;

/// The engine's composed error — the union surfaced to L7 (spine §2.1, §4.1).
///
/// The three source variants are `#[snafu(transparent)]` so the inner enum's `Display` and
/// [`CodedError::code`] pass straight through; the engine-local variants map to existing
/// [`ErrorCode`] values (never a new code).
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum EngineError {
    /// A storage operation failed (every read/mutation delegates to `Storage`).
    #[snafu(transparent)]
    Storage {
        /// The underlying storage failure.
        source: StorageError,
    },

    /// A policy decision failed (a forward-compat seam — infallible in v1).
    #[snafu(transparent)]
    Policy {
        /// The underlying policy failure.
        source: PolicyError,
    },

    /// Issue validation failed in the engine (run on `create`/`update` before storage).
    #[snafu(transparent)]
    Model {
        /// The underlying model/validation failure (aggregate or scalar).
        source: ModelError,
    },

    /// An operation was attempted against a workspace that is not open.
    ///
    /// Maps to [`ErrorCode::NotInitialized`] (exit 2).
    #[snafu(display("workspace is not open"))]
    WorkspaceNotOpen,

    /// A mutation was attempted while a cooperative shutdown is in progress (FR-17).
    ///
    /// Raised by `acquire_write` when the shutdown flag is set. Maps to
    /// [`ErrorCode::InternalError`] (exit 1).
    #[snafu(display("shutdown in progress; no new writes accepted"))]
    ShutdownInProgress,

    /// The write `Semaphore` was closed (poisoned) — no further writes can be serialized.
    ///
    /// Maps to [`ErrorCode::InternalError`] (exit 1). This is an internal invariant breach (the
    /// permit is never closed in v1 except by `shutdown`, which routes through `ShutdownInProgress`).
    #[snafu(display("write permit poisoned (semaphore closed)"))]
    WritePermitPoisoned,

    /// A method whose backing crate is not yet wired returned its typed seam error.
    ///
    /// `feature` is `"sync"` (the `export_jsonl`/`import_jsonl`/`import_bd` seam, T2.4) or
    /// `"health"` (the `doctor`/`recover` seam, T3.3). It is **never** a faked success and **never**
    /// inline L3 logic. Maps to [`ErrorCode::InternalError`] (exit 1) until the dep crate lands.
    #[snafu(display("feature not wired: {feature}"))]
    FeatureNotWired {
        /// The not-yet-wired feature: `"sync"` or `"health"`.
        feature: &'static str,
    },
}

impl CodedError for EngineError {
    fn code(&self) -> ErrorCode {
        match self {
            // Transparent sources forward their own code (so a Backend cause stays DatabaseError,
            // a ValidationFailed aggregate stays VALIDATION_FAILED, etc.).
            Self::Storage { source } => source.code(),
            Self::Policy { source } => source.code(),
            Self::Model { source } => source.code(),
            // Engine-local variants map to EXISTING §2.2 codes — never a new code.
            Self::WorkspaceNotOpen => ErrorCode::NotInitialized,
            Self::ShutdownInProgress | Self::WritePermitPoisoned | Self::FeatureNotWired { .. } => {
                ErrorCode::InternalError
            }
        }
    }

    fn hint(&self) -> Option<String> {
        // Forward the inner source's agent self-correction hint (e.g. ModelError's priority/status
        // hints) so it survives the engine boundary. Engine-local variants have no hint.
        match self {
            Self::Storage { source } => source.hint(),
            Self::Policy { source } => source.hint(),
            Self::Model { source } => source.hint(),
            Self::WorkspaceNotOpen
            | Self::ShutdownInProgress
            | Self::WritePermitPoisoned
            | Self::FeatureNotWired { .. } => None,
        }
    }

    fn context(&self) -> serde_json::Map<String, serde_json::Value> {
        // Forward the inner source's structured context (holder/cycle_path/fields/...) so the agent
        // self-correction surface survives the engine boundary. Engine-local variants carry no
        // structured payload beyond their (already sanitized) Display message.
        match self {
            Self::Storage { source } => source.context(),
            Self::Policy { source } => source.context(),
            Self::Model { source } => source.context(),
            Self::WorkspaceNotOpen
            | Self::ShutdownInProgress
            | Self::WritePermitPoisoned
            | Self::FeatureNotWired { .. } => serde_json::Map::new(),
        }
    }
    // `retryable()` is intentionally the default (`code().is_retryable()`): a transparent source's
    // retryability follows its forwarded code; the engine-local InternalError/NotInitialized codes
    // are non-retryable.
}

/// The engine result alias (spine §4.1: `Result<T, EngineError>`).
pub type Result<T> = core::result::Result<T, EngineError>;

#[cfg(test)]
mod tests {
    use super::EngineError;
    use unblock_error::{CodedError, ErrorCode, FieldError, ModelError, StructuredError};
    use unblock_policy::PolicyError;
    use unblock_storage::StorageError;

    #[test]
    fn engine_local_variants_map_to_pinned_codes() {
        assert_eq!(
            EngineError::WorkspaceNotOpen.code(),
            ErrorCode::NotInitialized
        );
        assert_eq!(
            EngineError::ShutdownInProgress.code(),
            ErrorCode::InternalError
        );
        assert_eq!(
            EngineError::WritePermitPoisoned.code(),
            ErrorCode::InternalError
        );
        assert_eq!(
            EngineError::FeatureNotWired { feature: "sync" }.code(),
            ErrorCode::InternalError
        );
        assert_eq!(
            EngineError::FeatureNotWired { feature: "health" }.code(),
            ErrorCode::InternalError
        );
    }

    #[test]
    fn transparent_storage_code_passes_through() {
        let err: EngineError = StorageError::IssueNotFound { id: "ub-1".into() }.into();
        assert_eq!(err.code(), ErrorCode::IssueNotFound);
        assert_eq!(err.code().exit_code(), 3);
    }

    #[test]
    fn transparent_policy_code_passes_through() {
        let err: EngineError = PolicyError::Internal.into();
        assert_eq!(err.code(), ErrorCode::InternalError);
    }

    #[test]
    fn transparent_model_aggregate_code_passes_through() {
        let err: EngineError = ModelError::ValidationFailed {
            fields: vec![FieldError::new("title", "cannot be empty")],
        }
        .into();
        assert_eq!(err.code(), ErrorCode::ValidationFailed);
        assert_eq!(err.code().exit_code(), 4);
        // retryable tracks code().is_retryable(); ValidationFailed is retryable.
        assert!(err.retryable());
    }

    #[test]
    fn engine_local_variants_are_not_retryable() {
        for err in [
            EngineError::WorkspaceNotOpen,
            EngineError::ShutdownInProgress,
            EngineError::WritePermitPoisoned,
            EngineError::FeatureNotWired { feature: "sync" },
        ] {
            assert!(!err.retryable(), "{err:?} must not be retryable");
        }
    }

    #[test]
    fn storage_context_survives_the_engine_boundary() {
        let err: EngineError = StorageError::AlreadyClaimed {
            id: "ub-42".into(),
            by: "winner".into(),
        }
        .into();
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::AlreadyClaimed);
        assert_eq!(structured.context["holder"], "winner");
        assert_eq!(structured.context["id"], "ub-42");
    }

    #[test]
    fn model_field_context_survives_the_engine_boundary() {
        let err: EngineError = ModelError::ValidationFailed {
            fields: vec![FieldError::new("priority", "must be 0-4")],
        }
        .into();
        let structured: StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::ValidationFailed);
        // The per-field detail surfaces under context["fields"] (D-E1).
        assert!(structured.context.contains_key("fields"));
    }

    #[test]
    fn feature_not_wired_display_carries_the_feature() {
        let err = EngineError::FeatureNotWired { feature: "health" };
        assert!(err.to_string().contains("health"));
    }
}
