//! The `EngineError → ErrorCode → exit_code` golden contract (FR-11), insta-pinned.
//!
//! Mirrors the spine §2.3 discipline at L5: every constructible `EngineError` shape (the three
//! transparent sources + the engine-local variants, incl. `FeatureNotWired`) maps to a stable
//! `(code, exit_code)`; a change forces a deliberate re-bless.

use unblock_engine::EngineError;
use unblock_error::{CodedError, ErrorCode, FieldError, ModelError};
use unblock_policy::PolicyError;
use unblock_storage::StorageError;

/// Build the full `(variant-label -> (code, exit_code, retryable))` table.
fn contract_rows() -> Vec<(&'static str, String, u8, bool)> {
    let cases: Vec<(&'static str, EngineError)> = vec![
        // --- transparent storage sources (a representative spread of codes/exit classes) ---
        (
            "Storage::IssueNotFound",
            StorageError::IssueNotFound { id: "ub-1".into() }.into(),
        ),
        (
            "Storage::AlreadyClaimed",
            StorageError::AlreadyClaimed {
                id: "ub-1".into(),
                by: "alice".into(),
            }
            .into(),
        ),
        (
            "Storage::CycleDetected",
            StorageError::CycleDetected {
                path: "a -> b -> a".into(),
            }
            .into(),
        ),
        (
            // IntegrityFailed maps to the generic DatabaseError code (exit 2) and is publicly
            // constructible — a faithful stand-in for the "backend-class failure -> DatabaseError"
            // row (Backend's inner BackendOpaque ctor is crate-private).
            "Storage::IntegrityFailed",
            StorageError::IntegrityFailed {
                messages: vec!["corrupt page".into()],
            }
            .into(),
        ),
        (
            "Storage::SchemaMismatch",
            StorageError::SchemaMismatch {
                found: 1,
                expected: 2,
            }
            .into(),
        ),
        // --- transparent policy source ---
        ("Policy::Internal", PolicyError::Internal.into()),
        // --- transparent model sources ---
        (
            "Model::ValidationFailed",
            ModelError::ValidationFailed {
                fields: vec![FieldError::new("title", "cannot be empty")],
            }
            .into(),
        ),
        (
            "Model::InvalidPriority",
            ModelError::InvalidPriority { value: "9".into() }.into(),
        ),
        // --- engine-local variants ---
        ("WorkspaceNotOpen", EngineError::WorkspaceNotOpen),
        ("ShutdownInProgress", EngineError::ShutdownInProgress),
        ("WritePermitPoisoned", EngineError::WritePermitPoisoned),
        (
            "FeatureNotWired(sync)",
            EngineError::FeatureNotWired { feature: "sync" },
        ),
        (
            "FeatureNotWired(health)",
            EngineError::FeatureNotWired { feature: "health" },
        ),
    ];

    // The transparent `unblock-sync` source (T2.4/D23), cfg-gated behind the default-on `sync`
    // feature. It forwards `SyncError::code()` (exit-6 IMPORT_COLLISION here), non-retryable.
    #[cfg(feature = "sync")]
    let cases = {
        let mut cases = cases;
        cases.push((
            "Sync::ImportCollision",
            unblock_sync::SyncError::ImportCollision { id: "ub-1".into() }.into(),
        ));
        cases
    };

    // The transparent `unblock-health` source (T3.3/HEALTH-LITE, D29), cfg-gated behind the default-on
    // `health` feature. It forwards `HealthError::code()` (exit-2 DATABASE_ERROR here), non-retryable.
    #[cfg(feature = "health")]
    let cases = {
        let mut cases = cases;
        cases.push((
            "Health::IntegrityCheckFailed",
            unblock_health::HealthError::IntegrityCheckFailed {
                rows: vec!["corrupt page".into()],
            }
            .into(),
        ));
        cases
    };

    cases
        .into_iter()
        .map(|(label, err)| {
            let code = err.code();
            (
                label,
                code.as_str().to_string(),
                code.exit_code(),
                err.retryable(),
            )
        })
        .collect()
}

#[test]
fn engine_error_code_exit_golden() {
    insta::assert_json_snapshot!(contract_rows());
}

#[test]
fn engine_local_variants_map_to_existing_codes() {
    // Each engine-local variant must map to an EXISTING §2.2 code (never a new one).
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
}

#[test]
fn transparent_sources_pass_their_code_through_unchanged() {
    let storage: EngineError = StorageError::IssueNotFound { id: "ub-1".into() }.into();
    assert_eq!(storage.code(), ErrorCode::IssueNotFound);

    let model: EngineError = ModelError::ValidationFailed {
        fields: vec![FieldError::new("priority", "must be 0-4")],
    }
    .into();
    assert_eq!(model.code(), ErrorCode::ValidationFailed);

    let policy: EngineError = PolicyError::Internal.into();
    assert_eq!(policy.code(), ErrorCode::InternalError);
}
