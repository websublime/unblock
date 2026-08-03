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

    /// Establishing a usable database handle failed — [`unblock_storage::LibsqlStorage::open_local`]
    /// **or**, since D46 clause (10), the pre-migration [`unblock_storage::Storage::schema_version`]
    /// read that runs on the very next line.
    ///
    /// That read belongs to this same "establish a usable handle" step and runs BEFORE the ladder, so
    /// labelling it `MigrationFailed` would be wrong; a THIRD variant is deliberately not minted (it
    /// would need its own `code()` and `hint()` arms for no gain — see `src/context.rs`).
    ///
    /// Forwards the inner [`StorageError`]'s own code (typically
    /// [`StorageError::Backend`] → [`ErrorCode::DatabaseError`]) — config does **not** hardcode it,
    /// so a lock/backend cause keeps its honest code — and, since D46, its `hint()` too.
    #[snafu(display("failed to open the workspace database: {source}"))]
    DbOpenFailed {
        /// The underlying storage failure from `open_local` (or the pre-migration version read).
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
    /// resolves, so this is **effectively unreachable** in T1.3 — the variant is reserved for a
    /// future strict-actor mode.
    #[snafu(display("could not resolve an actor (UNBLOCK_ACTOR / $USER / default)"))]
    ActorUnresolved,

    /// A `.unblock/config.toml` could not be parsed as TOML (spine §2.1 additive set).
    ///
    /// Maps to [`ErrorCode::ConfigParseError`] (exit 7). The variant identifier is `Parse` (spine
    /// §2.1) even though it maps to the `ConfigParseError` code; the inner [`toml::de::Error`] is
    /// absorbed (its message is surfaced, the type never re-exposed past this boundary).
    #[snafu(display("failed to parse {}: {source}", path.display()))]
    Parse {
        /// The TOML deserialization failure.
        source: toml::de::Error,
        /// The config file that failed to parse.
        path: PathBuf,
    },

    /// Reading a config file from disk failed (spine §2.1 additive set).
    ///
    /// Maps to [`ErrorCode::IoError`] (exit 8). Wraps the underlying [`std::io::Error`] together
    /// with the offending path for a diagnostic message.
    #[snafu(display("failed to read {}: {source}", path.display()))]
    Io {
        /// The underlying I/O failure.
        source: std::io::Error,
        /// The path that could not be read.
        path: PathBuf,
    },

    /// A resolved config value violated a validation rule (spine §2.1 additive set).
    ///
    /// Maps to [`ErrorCode::ConfigError`] (exit 7). Covers: an out-of-policy actor (Seam A
    /// `validate_actor` — over-length / NUL / control char), a path-injecting or absolute
    /// `db_filename` / `jsonl_filename` / `--db` (Seam B), an unparseable `UNBLOCK_OUTPUT_FORMAT`,
    /// an unsupported `backend`, and a forbidden `[remote] auth_token` in `config.toml` (NFR-18).
    #[snafu(display("invalid config value for `{key}` = `{value}`: {reason}"))]
    InvalidValue {
        /// The config key whose value was rejected.
        key: String,
        /// The rejected value (terminal-safe; rendered as-supplied).
        value: String,
        /// Why the value was rejected.
        reason: String,
    },
}

impl CodedError for ConfigError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::WorkspaceNotFound { .. } => ErrorCode::NotInitialized,
            // Forward the inner StorageError's own code — do NOT hardcode (so a Backend cause stays
            // DatabaseError and a Migration/SchemaMismatch cause stays SchemaMismatch).
            Self::DbOpenFailed { source } | Self::MigrationFailed { source } => source.code(),
            Self::ActorUnresolved => ErrorCode::RequiredField,
            // Config-file additive set (spine §2.1): exit 7 (Parse/InvalidValue) + exit 8 (Io).
            Self::Parse { .. } => ErrorCode::ConfigParseError,
            Self::Io { .. } => ErrorCode::IoError,
            Self::InvalidValue { .. } => ErrorCode::ConfigError,
        }
    }

    /// **D46 (v1.0.1) — the ONE change this crate's error type carries, and it is plumbing, not
    /// policy:** forward the inner [`StorageError`]'s own hint on the two storage-wrapping variants,
    /// mirroring EXACTLY the `code()` forward those two already share.
    ///
    /// **Why it is REQUIRED rather than tidy.** D46 clause (5) makes the migration run IMPLICITLY ON
    /// OPEN, and that path is this crate's open facade (`src/context.rs`). With only `code()`
    /// implemented, the `CodedError` trait DEFAULT `hint() -> None` discards the stale-schema hint
    /// composed in `unblock-storage` before `StructuredError` is built — so the contract would
    /// publish `SchemaMismatch`'s `hint_shape: contextual_text` (paid for with the
    /// `unblock.mcp.v1.8` → `unblock.mcp.v1.9` bump) while the user on the normative path received no
    /// hint: this decision's own failure mode wearing the contract's clothes.
    ///
    /// Every other variant keeps the trait default `None`. No variant is added, no `ErrorCode` moves,
    /// and no exit code changes.
    fn hint(&self) -> Option<String> {
        match self {
            Self::DbOpenFailed { source } | Self::MigrationFailed { source } => source.hint(),
            // Enumerated rather than a catch-all `_`, for the same reason `code()` is: a future
            // variant must DECLARE whether it carries a hint.
            Self::WorkspaceNotFound { .. }
            | Self::ActorUnresolved
            | Self::Parse { .. }
            | Self::Io { .. }
            | Self::InvalidValue { .. } => None,
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

    /// **D46 — the two storage-wrapping variants forward the inner `StorageError`'s HINT, and this
    /// crate is the ONLY reason it survives the implicit-on-open boundary.**
    ///
    /// MUTANT KILLED: deleting the `hint()` impl (or its two-variant arm) — the `CodedError` trait
    /// default `hint() -> None` swallows the storage-composed text and the `StructuredError` the L7
    /// boundary builds arrives with `hint: None`. That is the contract publishing
    /// `SchemaMismatch: contextual_text` — bought with the `unblock.mcp.v1.9` bump — with nothing
    /// behind it on the normative path.
    #[test]
    fn db_open_and_migration_failures_forward_the_storage_hint() {
        use unblock_error::StructuredError;

        let inner = StorageError::Migration {
            from: 2,
            to: 2,
            reason: "the `comments` table is missing the column(s) updated_at, redacted_at that \
                     schema version 2 adds"
                .to_string(),
        };
        let expected = inner.hint().expect("storage composes the hint");

        for wrapped in [
            ConfigError::MigrationFailed {
                source: StorageError::Migration {
                    from: 2,
                    to: 2,
                    reason:
                        "the `comments` table is missing the column(s) updated_at, redacted_at \
                             that schema version 2 adds"
                            .to_string(),
                },
            },
            ConfigError::DbOpenFailed {
                source: StorageError::SchemaMismatch {
                    found: 3,
                    expected: 2,
                },
            },
        ] {
            assert!(
                wrapped.hint().is_some(),
                "{wrapped:?} must forward its source's hint"
            );
            // The forwarded text survives the bridge the L7 boundary actually builds.
            let structured: StructuredError = (&wrapped).into();
            assert_eq!(structured.hint, wrapped.hint());
        }

        assert_eq!(
            ConfigError::MigrationFailed { source: inner }
                .hint()
                .as_deref(),
            Some(expected.as_str()),
            "forwarded VERBATIM — config composes no text of its own"
        );

        // Every other variant keeps the trait default.
        assert_eq!(ConfigError::ActorUnresolved.hint(), None);
        assert_eq!(
            ConfigError::WorkspaceNotFound {
                start: PathBuf::from("/tmp/nowhere")
            }
            .hint(),
            None
        );
    }

    #[test]
    fn parse_maps_to_config_parse_error_exit_7() {
        // A genuine toml::de::Error (a number where a table is expected).
        let source = toml::from_str::<toml::Table>("not_a_table").expect_err("toml parse error");
        let err = ConfigError::Parse {
            source,
            path: PathBuf::from("/ws/.unblock/config.toml"),
        };
        assert_eq!(err.code(), ErrorCode::ConfigParseError);
        // spine §2.3: ConfigParseError is a Config-category code -> exit 7.
        assert_eq!(err.code().exit_code(), 7);
    }

    #[test]
    fn io_maps_to_io_error_exit_8() {
        let source = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = ConfigError::Io {
            source,
            path: PathBuf::from("/ws/.unblock/config.toml"),
        };
        assert_eq!(err.code(), ErrorCode::IoError);
        // spine §2.3: IoError is an I/O-category code -> exit 8.
        assert_eq!(err.code().exit_code(), 8);
    }

    #[test]
    fn invalid_value_maps_to_config_error_exit_7() {
        let err = ConfigError::InvalidValue {
            key: "actor".to_string(),
            value: "x".repeat(201),
            reason: "exceeds 200 characters".to_string(),
        };
        assert_eq!(err.code(), ErrorCode::ConfigError);
        // spine §2.3: ConfigError is a Config-category code -> exit 7.
        assert_eq!(err.code().exit_code(), 7);
    }

    /// Golden snapshot of the full `(variant -> code -> exit)` table (now 7 variants, spine §2.3 /
    /// FR-11). Drift in any variant's mapping or the addition/removal of a variant fails the check.
    #[test]
    fn error_variant_code_exit_table_golden() {
        let variants: Vec<ConfigError> = vec![
            ConfigError::WorkspaceNotFound {
                start: PathBuf::from("/ws"),
            },
            ConfigError::DbOpenFailed {
                source: StorageError::IntegrityFailed {
                    messages: vec!["x".to_string()],
                },
            },
            ConfigError::MigrationFailed {
                source: StorageError::Migration {
                    from: 1,
                    to: 2,
                    reason: "r".to_string(),
                },
            },
            ConfigError::ActorUnresolved,
            ConfigError::Parse {
                source: toml::from_str::<toml::Table>("x").expect_err("parse err"),
                path: PathBuf::from("/ws/.unblock/config.toml"),
            },
            ConfigError::Io {
                source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
                path: PathBuf::from("/ws/.unblock/config.toml"),
            },
            ConfigError::InvalidValue {
                key: "actor".to_string(),
                value: "v".to_string(),
                reason: "r".to_string(),
            },
        ];

        let table: Vec<(String, String, u8)> = variants
            .iter()
            .map(|e| {
                let variant = match e {
                    ConfigError::WorkspaceNotFound { .. } => "WorkspaceNotFound",
                    ConfigError::DbOpenFailed { .. } => "DbOpenFailed",
                    ConfigError::MigrationFailed { .. } => "MigrationFailed",
                    ConfigError::ActorUnresolved => "ActorUnresolved",
                    ConfigError::Parse { .. } => "Parse",
                    ConfigError::Io { .. } => "Io",
                    ConfigError::InvalidValue { .. } => "InvalidValue",
                };
                (
                    variant.to_string(),
                    e.code().as_str().to_string(),
                    e.code().exit_code(),
                )
            })
            .collect();

        insta::assert_json_snapshot!("config_error_variant_code_exit_table", table);
    }
}
