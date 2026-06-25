//! [`ConfigError`] — the per-crate snafu error enum (spine §2.1, §4) and its
//! [`unblock_error::CodedError`] bridge.
//!
//! This is the **T1.3a minimal** variant set (the full layered-resolution variants —
//! parse / unknown-key / invalid-value / I/O / credential paths — are added **additively at
//! T1.3**, mapping to the same exit-7/exit-8 codes). Following the §2.1 pattern, `unblock-config`
//! defines its own enum implementing the L0 [`unblock_error::CodedError`] bridge trait (NOT a
//! bespoke inherent `code()`), matching the landed [`unblock_storage::StorageError`] convention so
//! the L7 blanket `From<&E: CodedError>` bridges it uniformly.

use std::path::PathBuf;

use snafu::Snafu;
use unblock_error::{CodedError, ErrorCode};
use unblock_storage::StorageError;

/// The per-crate error returned by the `unblock-config` workspace facades (spine §2.1, §4).
///
/// Each variant maps to exactly one **existing** [`ErrorCode`] (§2.2) via the [`CodedError`] impl
/// below — it never introduces a new code. The set grows **additively at T1.3** (config-file
/// parse / I/O / invalid-value variants), so no T1.3a variant is renumbered or removed.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum ConfigError {
    /// No `.unblock/` workspace was found by upward discovery from `start`.
    ///
    /// Maps to [`ErrorCode::NotInitialized`]: an un-discovered workspace is the "run `init` first"
    /// condition. Workspace **creation** belongs to `init` (T3.1); both T1.3a facades require an
    /// existing `.unblock/`.
    #[snafu(display("no .unblock/ workspace found at or above {}", start.display()))]
    WorkspaceNotFound {
        /// The start path the upward discovery walk began from.
        start: PathBuf,
    },

    /// Opening the libsql database failed (wraps [`unblock_storage::LibsqlStorage::open_local`]).
    ///
    /// Forwards the inner [`StorageError`]'s own code (typically
    /// [`StorageError::Backend`] → [`ErrorCode::DatabaseError`]) — config does **not** hardcode it,
    /// so a lock/backend cause keeps its honest code.
    #[snafu(display("failed to open the workspace database: {source}"))]
    DbOpenFailed {
        /// The underlying storage failure from `open_local`.
        source: StorageError,
    },

    /// Applying schema migrations failed (wraps [`unblock_storage::Storage::migrate`]).
    ///
    /// Forwards the inner [`StorageError`]'s own code (a genuine
    /// [`StorageError::Migration`]/[`StorageError::SchemaMismatch`] cause →
    /// [`ErrorCode::SchemaMismatch`]; a [`StorageError::Backend`] cause stays
    /// [`ErrorCode::DatabaseError`]) — avoiding mis-labelling a backend failure as a schema problem.
    #[snafu(display("failed to migrate the workspace database: {source}"))]
    MigrationFailed {
        /// The underlying storage failure from `migrate()`.
        source: StorageError,
    },

    /// No actor could be resolved from `UNBLOCK_ACTOR` / `$USER` / the literal default.
    ///
    /// Maps to [`ErrorCode::RequiredField`] (the engine requires a non-empty `actor`, spine §4).
    /// With the `UNBLOCK_ACTOR → $USER → "unblock"` chain the final literal default always
    /// resolves, so this is **effectively unreachable** in T1.3a — the variant is reserved for a
    /// future strict-actor mode (T1.3).
    #[snafu(display("could not resolve an actor (UNBLOCK_ACTOR / $USER / default)"))]
    ActorUnresolved,
}

impl CodedError for ConfigError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::WorkspaceNotFound { .. } => ErrorCode::NotInitialized,
            // Forward the inner StorageError's own code — do NOT hardcode (so a Backend cause stays
            // DatabaseError and a Migration/SchemaMismatch cause stays SchemaMismatch).
            Self::DbOpenFailed { source } | Self::MigrationFailed { source } => source.code(),
            Self::ActorUnresolved => ErrorCode::RequiredField,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigError;
    use std::path::PathBuf;
    use unblock_error::{CodedError, ErrorCode};
    use unblock_storage::StorageError;

    #[test]
    fn workspace_not_found_maps_to_not_initialized() {
        let err = ConfigError::WorkspaceNotFound {
            start: PathBuf::from("/tmp/nowhere"),
        };
        assert_eq!(err.code(), ErrorCode::NotInitialized);
        // spine §2.3: NotInitialized is a Database-category code -> exit 2.
        assert_eq!(err.code().exit_code(), 2);
    }

    #[test]
    fn actor_unresolved_maps_to_required_field() {
        assert_eq!(
            ConfigError::ActorUnresolved.code(),
            ErrorCode::RequiredField
        );
        // spine §2.3: RequiredField is a Validation/policy-category code -> exit 4.
        assert_eq!(ConfigError::ActorUnresolved.code().exit_code(), 4);
    }

    #[test]
    fn db_open_failed_forwards_backend_as_database_error() {
        // A Migration cause forwards to SchemaMismatch; a non-schema storage failure forwards to
        // its own code. `IntegrityFailed` carries the generic `DatabaseError` code in storage, so
        // it is a faithful stand-in for "a backend-class open failure forwards to DatabaseError".
        let inner = StorageError::IntegrityFailed {
            messages: vec!["corrupt page".to_string()],
        };
        assert_eq!(inner.code(), ErrorCode::DatabaseError);
        let err = ConfigError::DbOpenFailed { source: inner };
        assert_eq!(err.code(), ErrorCode::DatabaseError);
        // spine §2.3: DatabaseError is a Database-category code -> exit 2.
        assert_eq!(err.code().exit_code(), 2);
    }

    #[test]
    fn migration_failed_forwards_migration_as_schema_mismatch() {
        let inner = StorageError::Migration {
            from: 1,
            to: 2,
            reason: "step failed".to_string(),
        };
        assert_eq!(inner.code(), ErrorCode::SchemaMismatch);
        let err = ConfigError::MigrationFailed { source: inner };
        assert_eq!(err.code(), ErrorCode::SchemaMismatch);
        // spine §2.3: SchemaMismatch is a Database-category code -> exit 2.
        assert_eq!(err.code().exit_code(), 2);
    }

    #[test]
    fn migration_failed_forwards_schema_mismatch_as_schema_mismatch() {
        let inner = StorageError::SchemaMismatch {
            found: 3,
            expected: 2,
        };
        assert_eq!(inner.code(), ErrorCode::SchemaMismatch);
        let err = ConfigError::MigrationFailed { source: inner };
        assert_eq!(err.code(), ErrorCode::SchemaMismatch);
        // spine §2.3: SchemaMismatch is a Database-category code -> exit 2.
        assert_eq!(err.code().exit_code(), 2);
    }
}
