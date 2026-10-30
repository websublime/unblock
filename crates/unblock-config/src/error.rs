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

use serde_json::{Map, Value};
use snafu::Snafu;
use unblock_error::{CodedError, ErrorCode};
use unblock_model::{ConfigSource, DbConfigFault};
use unblock_storage::StorageError;

use crate::keys::{KeyClass, classify};

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
    /// absorbed (its message is surfaced, the type never re-exposed past this boundary). The
    /// display carries the parser's MESSAGE and byte offset only — never `toml`'s source-line
    /// excerpt, which would echo a malformed line verbatim (a credential typed unquoted is refused by
    /// the parse before the NFR-18 deny could redact it, and since D57 a reload failure is reported
    /// in-band to an MCP client).
    #[snafu(display("failed to parse {}: {}", path.display(), parse_summary(source)))]
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
    /// The display ECHOES `key` and `value` through [`shown`] (clipped to
    /// [`unblock_error::MAX_ECHOED_BYTES`], control characters escaped), because both can be
    /// attacker-controlled (an MCP `config` argument, a hand-written database row); an EMPTY `value`
    /// is not echoed.
    #[snafu(display(
        "invalid config value for `{}`{}: {reason}",
        shown(key),
        shown_value(value)
    ))]
    InvalidValue {
        /// The config key whose value was rejected.
        key: String,
        /// The rejected value (echoed through [`shown`]; empty when no value was supplied).
        value: String,
        /// Why the value was rejected.
        reason: String,
    },

    /// A STARTUP key appeared in a runtime layer (PRD §4 D57 clause (2)) — in the user file
    /// (`layer: User`), or named by the `issue` tool's `config` action (`layer: Db`).
    ///
    /// Maps to [`ErrorCode::ConfigError`] (exit 7, not retryable); `context` carries `key` and
    /// `layer` (the §1.10 [`ConfigSource`], only ever `User` or `Db`, wire `"user"`/`"db"`).
    #[snafu(display(
        "`{}` is a startup-only key and cannot be set in the {} configuration layer; set it on \
         the command line, in the environment or in the project file .unblock/config.toml",
        shown(key),
        source_name(*layer)
    ))]
    StartupKeyInRuntimeLayer {
        /// The startup key that was named.
        key: String,
        /// The runtime layer that carried it (`User` or `Db`).
        layer: ConfigSource,
    },

    /// The `issue` tool's `config` action named a key this build does not know (PRD §4 D57 clause
    /// (7)). Maps to [`ErrorCode::ConfigError`] (exit 7); `context` carries `key` (clipped).
    #[snafu(display(
        "unknown config key `{}` (the runtime layer keys are actor, output_format and \
         search_cap)",
        shown(key)
    ))]
    UnknownKey {
        /// The unknown key.
        key: String,
    },

    /// The workspace is FAIL-CLOSED: the database `config` table holds at least one BLOCKING row
    /// (PRD §4 D57 clause (2a), [`unblock_model::ConfigFaultKind::is_blocking`]).
    ///
    /// Maps to [`ErrorCode::ConfigError`] (exit 7); `context` = `{ "keys": [the blocking keys] }`.
    /// The FIRST `ConfigError` variant with a production [`CodedError::hint`]: one recovery call per
    /// blocking key — `issue {"action":"config","op":"unset","key":"<key>"}` for a NON-layer key (a
    /// startup key or a `jsonl_export` row), and for a LAYER key holding an undecodable value
    /// `op:"get"` first, then `op:"set"` with a valid value (or `op:"unset"`).
    #[snafu(display("{}", faulty_db_config_message(faults)))]
    FaultyDbConfig {
        /// The blocking faulty rows.
        faults: Vec<DbConfigFault>,
    },
}

/// The wire spelling of a [`ConfigSource`] (its serde `snake_case` name) — `"user"`, `"db"`, … — so
/// the layer has ONE name in messages, `context` and the published schema.
#[must_use]
pub(crate) fn source_name(source: ConfigSource) -> &'static str {
    match source {
        ConfigSource::Cli => "cli",
        ConfigSource::Env => "env",
        ConfigSource::Project => "project",
        ConfigSource::User => "user",
        ConfigSource::Db => "db",
        ConfigSource::OsUser => "os_user",
        ConfigSource::Default => "default",
    }
}

/// Echo attacker-controllable text (an MCP `config` argument, a hand-written database row, a user
/// file value) into a message: CLIPPED to [`unblock_error::MAX_ECHOED_BYTES`] (spine §2.4 — each
/// origin clips its own attacker-controlled text) and with every control character ESCAPED
/// (`\n` → `\\n`, ESC → `\\u{1b}`), so the echo is always one printable line. Printable text,
/// non-ASCII included, is unchanged.
#[must_use]
pub(crate) fn shown(text: &str) -> String {
    let clipped = unblock_error::clip(text);
    let mut out = String::with_capacity(clipped.len());
    for c in clipped.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// The echoed-value part of `InvalidValue`'s message (a space, `=`, a space and the quoted value), or
/// nothing for an empty value (a refusal that read no value, e.g. `config get jsonl_export`, never
/// prints a misleading empty echo).
fn shown_value(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!(" = `{}`", shown(value))
    }
}

/// A TOML parse failure without the parser's source excerpt: its message and, when known, the byte
/// offset it points at (see [`ConfigError::Parse`]).
fn parse_summary(err: &toml::de::Error) -> String {
    let message = shown(err.message());
    match err.span() {
        Some(span) => format!("{message} (at byte {})", span.start),
        None => message,
    }
}

/// [`unblock_error::clip`] as an owned `String` — the `context` echo of a key (JSON escapes control
/// characters itself, so only the size is bounded there).
fn clipped(text: &str) -> String {
    unblock_error::clip(text).into_owned()
}

/// The `FaultyDbConfig` message: the blocking keys and why each row blocks.
fn faulty_db_config_message(faults: &[DbConfigFault]) -> String {
    let rows: Vec<String> = faults
        .iter()
        .map(|f| format!("`{}` ({})", shown(&f.key), shown(&f.fault.reason)))
        .collect();
    format!(
        "the workspace is fail-closed: the database config table holds a blocking row for {}",
        rows.join("; ")
    )
}

/// Whether `key` is a D57 LAYER key (`actor`, `output_format`, `search_cap`).
fn is_layer_key(key: &str) -> bool {
    matches!(classify(key), Some(KeyClass::Runtime(rk)) if rk.is_layer_key())
}

/// The `FaultyDbConfig` recovery hint: one line per blocking key naming the exact call (PRD §4 D57
/// clause (2a)). The key is spelled as a JSON string so the call is copy-pasteable whatever bytes the
/// row holds.
fn faulty_db_config_hint(faults: &[DbConfigFault]) -> String {
    let lines: Vec<String> = faults
        .iter()
        .map(|f| {
            let key = Value::String(f.key.clone()).to_string();
            if is_layer_key(&f.key) {
                format!(
                    "`{}` holds a value this build cannot decode: read it with \
                     issue {{\"action\":\"config\",\"op\":\"get\",\"key\":{key}}} first, then \
                     replace it with issue {{\"action\":\"config\",\"op\":\"set\",\"key\":{key},\
                     \"value\":\"<valid value>\"}} (or remove it with \
                     issue {{\"action\":\"config\",\"op\":\"unset\",\"key\":{key}}})",
                    shown(&f.key)
                )
            } else {
                format!(
                    "`{}` cannot be set in the database: remove it with \
                     issue {{\"action\":\"config\",\"op\":\"unset\",\"key\":{key}}}",
                    shown(&f.key)
                )
            }
        })
        .collect();
    lines.join("\n")
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
            // D57: the three FR-13-full variants ride the EXISTING `CONFIG_ERROR` (exit 7); no code
            // is minted (`ErrorCode::ALL` stays 36).
            Self::InvalidValue { .. }
            | Self::StartupKeyInRuntimeLayer { .. }
            | Self::UnknownKey { .. }
            | Self::FaultyDbConfig { .. } => ErrorCode::ConfigError,
        }
    }

    /// **D46 (v1.0.1)** — forward the inner [`StorageError`]'s own hint on the two storage-wrapping
    /// variants, mirroring EXACTLY the `code()` forward those two already share.
    ///
    /// **Why it is REQUIRED rather than tidy.** D46 clause (5) makes the migration run IMPLICITLY ON
    /// OPEN, and that path is this crate's open facade (`src/context.rs`). With only `code()`
    /// implemented, the `CodedError` trait DEFAULT `hint() -> None` discards the stale-schema hint
    /// composed in `unblock-storage` before `StructuredError` is built — so the contract would
    /// publish `SchemaMismatch`'s `hint_shape: contextual_text` (paid for with the
    /// `unblock.mcp.v1.8` → `unblock.mcp.v1.9` bump) while the user on the normative path received no
    /// hint: this decision's own failure mode wearing the contract's clothes.
    ///
    /// **D57 (v1.1)** — `FaultyDbConfig` composes the recovery hint (one call per blocking key),
    /// `CONFIG_ERROR`'s first production hint site, which is why its published `hint_shape` moves
    /// `none` → `contextual_text`. `StartupKeyInRuntimeLayer` and `UnknownKey` keep `None`.
    fn hint(&self) -> Option<String> {
        match self {
            Self::DbOpenFailed { source } | Self::MigrationFailed { source } => source.hint(),
            Self::FaultyDbConfig { faults } => Some(faulty_db_config_hint(faults)),
            // Enumerated rather than a catch-all `_`, for the same reason `code()` is: a future
            // variant must DECLARE whether it carries a hint.
            Self::WorkspaceNotFound { .. }
            | Self::ActorUnresolved
            | Self::Parse { .. }
            | Self::Io { .. }
            | Self::InvalidValue { .. }
            | Self::StartupKeyInRuntimeLayer { .. }
            | Self::UnknownKey { .. } => None,
        }
    }

    /// D57 — the structured payload of the three FR-13-full variants: `key`/`layer` on
    /// `StartupKeyInRuntimeLayer`, `key` on `UnknownKey`, `keys` (the blocking keys) on
    /// `FaultyDbConfig`. Every other variant keeps the empty map it carried before D57.
    fn context(&self) -> Map<String, Value> {
        let mut map = Map::new();
        match self {
            Self::StartupKeyInRuntimeLayer { key, layer } => {
                map.insert("key".to_string(), Value::String(clipped(key)));
                map.insert(
                    "layer".to_string(),
                    Value::String(source_name(*layer).to_string()),
                );
            }
            Self::UnknownKey { key } => {
                map.insert("key".to_string(), Value::String(clipped(key)));
            }
            Self::FaultyDbConfig { faults } => {
                let keys = faults
                    .iter()
                    .map(|f| Value::String(clipped(&f.key)))
                    .collect();
                map.insert("keys".to_string(), Value::Array(keys));
            }
            Self::WorkspaceNotFound { .. }
            | Self::DbOpenFailed { .. }
            | Self::MigrationFailed { .. }
            | Self::ActorUnresolved
            | Self::Parse { .. }
            | Self::Io { .. }
            | Self::InvalidValue { .. } => {}
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigError;
    use std::path::PathBuf;
    use unblock_error::{CodedError, ErrorCode};
    use unblock_storage::{SchemaGap, StorageError};

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
            gaps: Vec::new(),
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

        let gaps = || {
            vec![
                SchemaGap::CommentsColumn("updated_at"),
                SchemaGap::CommentsColumn("redacted_at"),
            ]
        };
        let reason = "the `comments` table is missing the column(s) updated_at, redacted_at that \
                      schema version 2 adds";
        let inner = StorageError::Migration {
            from: 2,
            to: 3,
            reason: reason.to_string(),
            gaps: gaps(),
        };
        let expected = inner.hint().expect("storage composes the hint");

        for wrapped in [
            ConfigError::MigrationFailed {
                source: StorageError::Migration {
                    from: 2,
                    to: 3,
                    reason: reason.to_string(),
                    gaps: gaps(),
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
                    gaps: Vec::new(),
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
            ConfigError::StartupKeyInRuntimeLayer {
                key: "db_filename".to_string(),
                layer: unblock_model::ConfigSource::User,
            },
            ConfigError::UnknownKey {
                key: "nope".to_string(),
            },
            ConfigError::FaultyDbConfig { faults: Vec::new() },
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
                    ConfigError::StartupKeyInRuntimeLayer { .. } => "StartupKeyInRuntimeLayer",
                    ConfigError::UnknownKey { .. } => "UnknownKey",
                    ConfigError::FaultyDbConfig { .. } => "FaultyDbConfig",
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

    /// **D57 clause (2a) — the `FaultyDbConfig` recovery hint.** A NON-layer key (a startup key or
    /// a `jsonl_export` row) is told `op:"unset"`; a LAYER key with an undecodable value is told
    /// `op:"get"` FIRST, then `op:"set"` (or `op:"unset"`); `context.keys` lists the blocking keys.
    ///
    /// MUTANT KILLED: one `unset` text for every key (an agent whose gate is stale would delete a
    /// valid value another process already wrote).
    #[test]
    fn faulty_db_config_hint_names_the_exact_recovery_per_key() {
        use unblock_model::{ConfigFault, ConfigFaultKind, DbConfigFault};
        let fault = |key: &str, kind| DbConfigFault {
            key: key.to_string(),
            value: "v".to_string(),
            fault: ConfigFault {
                kind,
                reason: "r".to_string(),
            },
        };
        let err = ConfigError::FaultyDbConfig {
            faults: vec![
                fault("db_filename", ConfigFaultKind::StartupKey),
                fault("search_cap", ConfigFaultKind::InvalidValue),
            ],
        };
        assert_eq!(err.code(), ErrorCode::ConfigError);
        let hint = err.hint().expect("FaultyDbConfig carries a hint");
        let mut lines = hint.lines();
        let first = lines.next().unwrap();
        assert!(
            first.contains(r#"issue {"action":"config","op":"unset","key":"db_filename"}"#),
            "{first}"
        );
        let second = lines.next().unwrap();
        let get = second
            .find(r#"issue {"action":"config","op":"get","key":"search_cap"}"#)
            .expect("get first");
        let set = second
            .find(r#""op":"set","key":"search_cap""#)
            .expect("then set");
        assert!(get < set, "{second}");
        let structured: unblock_error::StructuredError = (&err).into();
        assert_eq!(
            structured.context["keys"],
            serde_json::json!(["db_filename", "search_cap"])
        );
        assert_eq!(structured.hint.as_deref(), Some(hint.as_str()));
    }

    #[test]
    fn d57_refusals_carry_key_and_layer_context() {
        let err = ConfigError::StartupKeyInRuntimeLayer {
            key: "id_prefix".to_string(),
            layer: unblock_model::ConfigSource::Db,
        };
        let structured: unblock_error::StructuredError = (&err).into();
        assert_eq!(structured.code, ErrorCode::ConfigError);
        assert_eq!(structured.context["layer"], "db");
        assert_eq!(structured.context["key"], "id_prefix");
        assert_eq!(err.hint(), None);
        let err = ConfigError::UnknownKey {
            key: "zz".to_string(),
        };
        assert_eq!(err.code(), ErrorCode::ConfigError);
        assert_eq!(err.hint(), None);
    }
}
