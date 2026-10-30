//! Live reload of the RUNTIME keys (FR-13 full, PRD §4 D57 clause (8)) — [`ConfigReloader`] — and
//! the value helpers the engine's `config` action runs before any write
//! ([`classify_runtime_key`], [`canonicalize_runtime_value`]).
//!
//! A reload re-reads the project file, the user file and the database rows it is handed, and
//! re-applies the CLI and env layers the process captured at open (a process's arguments and
//! environment do not change). It decodes the rows TOLERANTLY (faults are RETURNED in
//! `RuntimeConfig::db_faults`, never raised), so it fails ONLY on a project- or user-file error, and
//! it ignores every startup key — a startup key edited into the project file mid-session takes effect
//! at the next open.

use std::path::PathBuf;

use unblock_model::{ConfigSource, OutputFormat, validate_actor};

use crate::cli::CliOverrides;
use crate::config::{MAX_SEARCH_CAP, RuntimeConfig, resolve_runtime};
use crate::env::{EnvOverrides, parse_output_format};
use crate::error::ConfigError;
use crate::keys::{KeyClass, RuntimeKey, classify};
use crate::schema::ProjectConfig;
use crate::user_config::load_user_file;

/// The reason every `jsonl_export` refusal carries (PRD §4 D57 clause (2)): the key is inert until
/// its consumer lands, so no user file and no database row may set it.
pub(crate) const JSONL_EXPORT_NOT_A_LAYER_KEY: &str = "`jsonl_export` is not a user-file, \
    database or `config`-action key: nothing reads it yet (its consumer is tracked as ub-p1h); set \
    it on the command line, with UNBLOCK_JSONL or in the project file .unblock/config.toml";

/// Re-resolves the RUNTIME keys for a LIVE session (PRD §4 D57 clause (8)).
///
/// Built once per storage-bearing open and moved into the engine `Session`; a reload REPLACES the
/// whole runtime snapshot with what the captured layers resolve. It CAPTURES what cannot change
/// during a process — the [`CliOverrides`], the parsed [`EnvOverrides`], the `$USER` value, the
/// project file path and the user file path — and re-reads both files on every call.
///
/// A context built by struct literal must hand it the layers that produced its snapshot: a harness
/// whose snapshot carries actor `"tester"` uses
/// `ConfigReloader::new(CliOverrides::default(), EnvOverrides::default(), Some("tester".into()),
/// None, None)` — the actor at the `$USER` (`os_user`) tier — so a reload re-resolves `"tester"`
/// while a DB `actor` row still overrides it, exactly as in production. [`ConfigReloader::default`]
/// is correct only for a snapshot of pure defaults.
#[derive(Debug, Clone, Default)]
pub struct ConfigReloader {
    cli: CliOverrides,
    env: EnvOverrides,
    os_user: Option<String>,
    project_file: Option<PathBuf>,
    user_file: Option<PathBuf>,
}

impl ConfigReloader {
    /// The injectable constructor; reads nothing (no `$HOME`, no file) until
    /// [`ConfigReloader::reload_runtime`] is called. A `None` file is an empty layer; so is a path
    /// whose file does not exist.
    #[must_use]
    pub fn new(
        cli: CliOverrides,
        env: EnvOverrides,
        os_user: Option<String>,
        project_file: Option<PathBuf>,
        user_file: Option<PathBuf>,
    ) -> Self {
        Self {
            cli,
            env,
            os_user,
            project_file,
            user_file,
        }
    }

    /// Re-read the project file and the user file, decode `db_rows` tolerantly (PRD §4 D57 clause
    /// (2a) — faults are RETURNED in [`RuntimeConfig::db_faults`], never raised), and re-resolve the
    /// four runtime keys over the six layers (the captured CLI/env layers re-applied). Pure apart
    /// from the two file reads; never touches storage, so the caller decides which rows (committed or
    /// prospective) it is resolved against.
    ///
    /// # Errors
    ///
    /// ONLY a project- or user-file error: [`ConfigError::Io`], [`ConfigError::Parse`],
    /// [`ConfigError::InvalidValue`] (a credential, a user `jsonl_export`, an out-of-policy actor or
    /// `search_cap`) or [`ConfigError::StartupKeyInRuntimeLayer`] (a startup key in the user file).
    /// Startup keys in the project file are ignored.
    pub fn reload_runtime(
        &self,
        db_rows: &[(String, String)],
    ) -> Result<RuntimeConfig, ConfigError> {
        let project = match &self.project_file {
            Some(path) => ProjectConfig::load_file(path)?,
            None => ProjectConfig::default(),
        };
        let user = load_user_file(self.user_file.as_deref())?;
        resolve_runtime(
            &self.cli,
            &self.env,
            &project,
            &user,
            db_rows,
            self.os_user.as_deref(),
        )
    }
}

/// Accept only the three LAYER keys (PRD §4 D57 clause (7)) — the validator the engine's `config`
/// action runs before any write.
///
/// # Errors
///
/// - `jsonl_export` → [`ConfigError::InvalidValue`] naming `ub-p1h`;
/// - a startup key (either `jsonl_filename` spelling) →
///   [`ConfigError::StartupKeyInRuntimeLayer`] with `layer: Db`;
/// - anything else unknown → [`ConfigError::UnknownKey`].
pub fn classify_runtime_key(key: &str) -> Result<RuntimeKey, ConfigError> {
    match classify(key) {
        Some(KeyClass::Runtime(RuntimeKey::JsonlExport)) => Err(jsonl_export_refusal("")),
        Some(KeyClass::Runtime(rk)) => Ok(rk),
        Some(KeyClass::Startup(_)) => Err(ConfigError::StartupKeyInRuntimeLayer {
            key: key.to_string(),
            layer: ConfigSource::Db,
        }),
        None => Err(ConfigError::UnknownKey {
            key: key.to_string(),
        }),
    }
}

/// Canonicalize `raw` for `key` — the string a `config` row stores (PRD §4 D57 clause (4)), produced
/// by the per-key parsers the env layer uses, so a row decodes exactly the way an `UNBLOCK_*` value
/// does and `value` always equals the `effective` it yields: `output_format` → its wire name,
/// `search_cap` → a decimal integer in `1..=i64::MAX`, `actor` → TRIMMED, an empty result refused,
/// then `validate_actor`.
///
/// # Errors
///
/// [`ConfigError::InvalidValue`] for an empty value, an undecodable value, an out-of-range
/// `search_cap`, an out-of-policy actor, or `RuntimeKey::JsonlExport` (naming `ub-p1h`, exactly as
/// [`classify_runtime_key`] does).
pub fn canonicalize_runtime_value(key: RuntimeKey, raw: &str) -> Result<String, ConfigError> {
    parse_runtime_value(key, raw).map(|value| value.canonical())
}

/// A decoded runtime LAYER value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeValue {
    /// A trimmed, bounded actor.
    Actor(String),
    /// An output format.
    OutputFormat(OutputFormat),
    /// A search cap in `1..=i64::MAX`.
    SearchCap(usize),
}

impl RuntimeValue {
    /// The canonical string this value stores as.
    pub(crate) fn canonical(&self) -> String {
        match self {
            Self::Actor(actor) => actor.clone(),
            Self::OutputFormat(format) => output_format_name(*format),
            Self::SearchCap(cap) => cap.to_string(),
        }
    }
}

/// The wire name of an [`OutputFormat`] (its serde `snake_case` spelling — the single string table).
pub(crate) fn output_format_name(format: OutputFormat) -> String {
    match serde_json::to_value(format) {
        Ok(serde_json::Value::String(name)) => name,
        // `OutputFormat` is a unit-variant enum with `rename_all = "snake_case"`, so serde always
        // yields a string; the debug spelling is an unreachable, still-deterministic fallback.
        _ => format!("{format:?}").to_ascii_lowercase(),
    }
}

/// Parse `raw` for a runtime LAYER key with the env layer's per-key parsers (PRD §4 D57 clause (4)).
///
/// # Errors
///
/// As [`canonicalize_runtime_value`].
pub(crate) fn parse_runtime_value(key: RuntimeKey, raw: &str) -> Result<RuntimeValue, ConfigError> {
    let name = key.name();
    let trimmed = raw.trim();
    if key == RuntimeKey::JsonlExport {
        return Err(jsonl_export_refusal(raw));
    }
    if trimmed.is_empty() {
        return Err(ConfigError::InvalidValue {
            key: name.to_string(),
            value: raw.to_string(),
            reason: "the value must not be empty or blank".to_string(),
        });
    }
    match key {
        RuntimeKey::Actor => {
            validate_actor(trimmed).map_err(|e| ConfigError::InvalidValue {
                key: name.to_string(),
                value: raw.to_string(),
                reason: e.reason,
            })?;
            Ok(RuntimeValue::Actor(trimmed.to_string()))
        }
        RuntimeKey::OutputFormat => {
            parse_output_format(name, trimmed).map(RuntimeValue::OutputFormat)
        }
        RuntimeKey::SearchCap => parse_search_cap(name, raw).map(RuntimeValue::SearchCap),
        RuntimeKey::JsonlExport => Err(jsonl_export_refusal(raw)),
    }
}

/// Parse a decimal `search_cap` BOUNDED to `1..=i64::MAX` (PRD §4 D57 clause (4)): a `0` would
/// silently empty every default search, and a larger value overflows `SQLite`'s `LIMIT`.
fn parse_search_cap(key: &str, raw: &str) -> Result<usize, ConfigError> {
    let out_of_range = || ConfigError::InvalidValue {
        key: key.to_string(),
        value: raw.to_string(),
        reason: format!("expected a decimal integer from 1 to {MAX_SEARCH_CAP}"),
    };
    let parsed: i64 = raw.trim().parse().map_err(|_| out_of_range())?;
    if parsed < 1 {
        return Err(out_of_range());
    }
    usize::try_from(parsed).map_err(|_| out_of_range())
}

/// The `jsonl_export` refusal (PRD §4 D57 clause (2)): the EXISTING `InvalidValue` naming `ub-p1h`.
pub(crate) fn jsonl_export_refusal(value: &str) -> ConfigError {
    ConfigError::InvalidValue {
        key: RuntimeKey::JsonlExport.name().to_string(),
        value: value.to_string(),
        reason: JSONL_EXPORT_NOT_A_LAYER_KEY.to_string(),
    }
}
