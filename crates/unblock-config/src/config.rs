//! [`ResolvedConfig`] — the resolved, validated config **VALUES** the engine/`Session` reads
//! (config-owned, spine §4 CF-D) — plus the producer [`WorkspaceConfig`] that fills it.
//!
//! [`ResolvedConfig`] is **NOT** paths (those live in [`crate::ConfigPaths`]). Since D57 (v1.1) it is
//! PARTITIONED by key class — the FR-13 startup/runtime split is a TYPE, not a list:
//! `ResolvedConfig { startup: StartupConfig, runtime: RuntimeConfig }`. The STARTUP keys are read
//! ONCE at open and never reloaded; the RUNTIME keys (the actor included — it moved here from the
//! contexts' top level) are re-resolved by [`crate::ConfigReloader::reload_runtime`], and carry the
//! layer that supplied each ([`RuntimeSources`]) and the database layer's faulty rows
//! ([`RuntimeConfig::db_faults`], PRD §4 D57 clause (2a)).
//!
//! [`WorkspaceConfig`] is the merged, validated config value (startup + runtime keys) produced by
//! the layered resolver; it *fills* [`ResolvedConfig`] (and feeds [`crate::ConfigPaths`]). The ONE
//! order (PRD §4 D57 clause (1)) is **CLI > env `UNBLOCK_*` > project `.unblock/config.toml` > user
//! `config.toml` > DB `config` table > defaults**; startup keys resolve over CLI > env > project >
//! defaults only, and the actor's bottom is `$USER` > `"unblock"` (clause (5)), bounded via
//! [`unblock_model::validate_actor`] (Seam A). `deletions_retention_days` and `backend` are
//! resolved-but-NOT-projected (reserved).

use unblock_model::{ConfigSource, DbConfigFault, OutputFormat, normalize_prefix, validate_actor};

use crate::cli::CliOverrides;
use crate::db_layer::{DbLayer, db_layer};
use crate::env::{EnvOverrides, EnvSource};
use crate::error::ConfigError;
use crate::merge::{ConfigLayer, merge_layers};
use crate::schema::ProjectConfig;
/// The locked default database filename (PRD §12.5).
pub(crate) const DB_FILENAME: &str = "unblock.db";

/// The locked default JSONL export filename (PRD §12.5).
pub(crate) const JSONL_FILENAME: &str = "issues.jsonl";

/// The T1.3a default search-result cap (FR-4).
pub(crate) const DEFAULT_SEARCH_CAP: usize = 50;

/// The default issue-id prefix (D21/T1.8). Rendered as `ub-<hash>` by the engine allocator unless a
/// workspace overrides `id_prefix`.
pub(crate) const DEFAULT_ID_PREFIX: &str = "ub";

/// The default `.unblock/.write.lock` acquire timeout, in ms (D31/T3.4.1). Sourced from the L2
/// storage crate so config and storage share one value; threaded DOWN to `open_local` at open.
pub(crate) const DEFAULT_WRITE_LOCK_TIMEOUT_MS: u64 =
    unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS;

/// The documented lower bound (in ms) for a resolved `write_lock_timeout_ms` (D31/T3.4.1). Any
/// user-supplied value below this floor is clamped UP to it in [`WorkspaceConfig::resolve`].
///
/// Rationale: a `0` (or near-zero) acquire timeout makes **every** normal mutation fail-fast on the
/// slightest contention — a footgun. `0` is reserved for migrate's internal `Duration::ZERO`
/// fail-fast (an explicit L2 code path, MF2), and is never a user-facing acquire timeout. The floor
/// is deliberately small (`100` ms) so it only rules out the pathological "instant fail" values
/// while leaving every genuine tuning choice (short or long) untouched.
pub(crate) const MIN_WRITE_LOCK_TIMEOUT_MS: u64 = 100;

/// The largest resolved `search_cap` (PRD §4 D57 clause (4)): `SQLite`'s `LIMIT` is a signed 64-bit
/// integer, so a larger cap overflows it. The bound `1..=i64::MAX` holds on EVERY layer.
// `i64::MAX` is non-negative, so the cast is lossless; `usize` is 64-bit on every supported target.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) const MAX_SEARCH_CAP: usize = i64::MAX as usize;

/// The resolved, validated config values the engine/`Session` consumes (config-owned, spine §4
/// CF-D), PARTITIONED by key class since D57 (v1.1).
///
/// Embedded **by value** in both [`crate::ResolvedContext`] and [`crate::WorkspaceContext`].
/// [`ResolvedConfig::default`] is the T1.3a defaults with no faults and every source
/// [`ConfigSource::Default`], so struct-literal test contexts keep compiling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedConfig {
    /// The STARTUP keys — read ONCE at open, never reloaded.
    pub startup: StartupConfig,
    /// The RUNTIME keys — re-resolved by [`crate::ConfigReloader::reload_runtime`].
    pub runtime: RuntimeConfig,
}

/// The STARTUP keys (PRD §4 D57 clause (2)) — resolved over CLI > env > project > defaults only
/// and read ONCE at open. (`backend` and `deletions_retention_days` stay resolved-but-unprojected.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupConfig {
    /// The database filename inside `.unblock/`. Default = `"unblock.db"` (PRD §12.5).
    pub db_filename: String,
    /// The JSONL export filename inside `.unblock/`. Default = `"issues.jsonl"` (PRD §12.5).
    pub jsonl_filename: String,
    /// The issue-id prefix the engine allocator mints with (D21/T1.8). Default = `"ub"`,
    /// `normalize_prefix`-normalized. The engine reads `ctx.config.startup.id_prefix` at mint time.
    pub id_prefix: String,
    /// The `.unblock/.write.lock` acquire timeout in ms (D31/T3.4.1). Default = `30_000`; threaded
    /// DOWN to `LibsqlStorage::open_local` at open; clamped UP to [`MIN_WRITE_LOCK_TIMEOUT_MS`].
    pub write_lock_timeout_ms: u64,
}

impl Default for StartupConfig {
    /// The T1.3a defaults: `"unblock.db"` / `"issues.jsonl"` / `"ub"` / `30_000`.
    fn default() -> Self {
        Self {
            db_filename: DB_FILENAME.to_string(),
            jsonl_filename: JSONL_FILENAME.to_string(),
            id_prefix: DEFAULT_ID_PREFIX.to_string(),
            write_lock_timeout_ms: DEFAULT_WRITE_LOCK_TIMEOUT_MS,
        }
    }
}

/// The RUNTIME keys (PRD §4 D57 clause (2)) — re-resolved by a live session's reload — plus the
/// layer that supplied each and the database layer's faulty rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// The resolved actor: `--actor` > `UNBLOCK_ACTOR` > project `actor` > user `actor` > DB `actor`
    /// > `$USER` > `"unblock"` (D10 as amended by D57), trimmed and bounded by `validate_actor`.
    pub actor: String,
    /// The output format for rendered command output (re-exported from `unblock-model`, G-7/CF-J).
    pub output_format: OutputFormat,
    /// Auto-export JSONL after mutating ops (FR-7). NOT a layer key (consumer tracked as `ub-p1h`):
    /// resolved over CLI > env > project > defaults only.
    pub jsonl_export: bool,
    /// Search-result cap (FR-4), bounded to `1..=i64::MAX` on every layer. Default = `50`.
    pub search_cap: usize,
    /// The layer that supplied each runtime value.
    pub sources: RuntimeSources,
    /// The database `config` table's faulty rows (PRD §4 D57 clause (2a)): any whose
    /// `fault.kind.is_blocking()` fail-closes the engine `Session`.
    pub db_faults: Vec<DbConfigFault>,
}

impl Default for RuntimeConfig {
    /// The T1.3a defaults (`"unblock"` / `Json` / `false` / `50`), no faults, every source
    /// [`ConfigSource::Default`] — the actor's included, since the literal `"unblock"` IS the
    /// built-in tier.
    fn default() -> Self {
        Self {
            actor: crate::actor::DEFAULT_ACTOR.to_string(),
            output_format: OutputFormat::default(),
            jsonl_export: false,
            search_cap: DEFAULT_SEARCH_CAP,
            sources: RuntimeSources::default(),
            db_faults: Vec::new(),
        }
    }
}

impl RuntimeConfig {
    /// The fail-closed gate's error (PRD §4 D57 clause (2a)): `Some(ConfigError::FaultyDbConfig)`
    /// carrying every BLOCKING row ([`unblock_model::ConfigFaultKind::is_blocking`] — the ONE
    /// predicate) when at least one exists, else `None`. The engine gate, the cli `mcp` startup line
    /// and `unblock init`'s check all go through it.
    #[must_use]
    pub fn blocking_fault_error(&self) -> Option<ConfigError> {
        let faults: Vec<DbConfigFault> = self
            .db_faults
            .iter()
            .filter(|f| f.fault.kind.is_blocking())
            .cloned()
            .collect();
        if faults.is_empty() {
            None
        } else {
            Some(ConfigError::FaultyDbConfig { faults })
        }
    }
}

/// The layer that supplied each runtime value (spine §1.10 [`ConfigSource`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSources {
    /// The source of [`RuntimeConfig::actor`] (`os_user` for the `$USER` fallback).
    pub actor: ConfigSource,
    /// The source of [`RuntimeConfig::output_format`].
    pub output_format: ConfigSource,
    /// The source of [`RuntimeConfig::jsonl_export`].
    pub jsonl_export: ConfigSource,
    /// The source of [`RuntimeConfig::search_cap`].
    pub search_cap: ConfigSource,
}

impl Default for RuntimeSources {
    /// Every source [`ConfigSource::Default`].
    fn default() -> Self {
        Self {
            actor: ConfigSource::Default,
            output_format: ConfigSource::Default,
            jsonl_export: ConfigSource::Default,
            search_cap: ConfigSource::Default,
        }
    }
}

/// The lowest-precedence defaults layer (FR-13). Guarantees a value for every non-reserved field.
///
/// `actor` is the bottom of the actor chain (PRD §4 D57 clause (5)): the trimmed `$USER` when set
/// (`actor_source` [`ConfigSource::OsUser`]), else the literal `"unblock"` ([`ConfigSource::Default`]);
/// `deletions_retention_days`/`backend` carry `None` (reserved — no v1 default).
#[derive(Debug, Clone)]
pub(crate) struct Defaults {
    /// The actor chain's bottom: `$USER` > `"unblock"`.
    pub actor: String,
    /// Which tier supplied [`Defaults::actor`] — `OsUser` or `Default`.
    pub actor_source: ConfigSource,
    /// Default output format ([`OutputFormat::Json`]).
    pub output_format: OutputFormat,
    /// Default JSONL-export toggle (`false`).
    pub jsonl_export: bool,
    /// Default search cap (`50`, FR-4).
    pub search_cap: usize,
    /// Default db filename (`"unblock.db"`, PRD §12.5).
    pub db_filename: String,
    /// Default JSONL filename (`"issues.jsonl"`, PRD §12.5).
    pub jsonl_filename: String,
    /// Default issue-id prefix (`"ub"`, D21/T1.8).
    pub id_prefix: String,
    /// Default `.write.lock` acquire timeout (`30_000` ms, D31/T3.4.1).
    pub write_lock_timeout_ms: u64,
    /// Reserved (no v1 default retention window).
    pub deletions_retention_days: Option<u64>,
    /// Reserved (no v1 default backend value; `None` resolves to the libsql default downstream).
    pub backend: Option<String>,
}

impl Default for Defaults {
    /// The locked v1 defaults, with `actor` = the literal `"unblock"`.
    fn default() -> Self {
        Self {
            actor: crate::actor::DEFAULT_ACTOR.to_string(),
            actor_source: ConfigSource::Default,
            output_format: OutputFormat::default(),
            jsonl_export: false,
            search_cap: DEFAULT_SEARCH_CAP,
            db_filename: DB_FILENAME.to_string(),
            jsonl_filename: JSONL_FILENAME.to_string(),
            id_prefix: DEFAULT_ID_PREFIX.to_string(),
            write_lock_timeout_ms: DEFAULT_WRITE_LOCK_TIMEOUT_MS,
            deletions_retention_days: None,
            backend: None,
        }
    }
}

impl Defaults {
    /// The defaults with the actor chain's bottom resolved from `os_user` (`$USER`, trimmed; a blank
    /// value falls through to the literal `"unblock"`).
    pub(crate) fn with_os_user(os_user: Option<&str>) -> Self {
        match os_user.and_then(crate::actor::trimmed_non_empty) {
            Some(actor) => Self {
                actor,
                actor_source: ConfigSource::OsUser,
                ..Self::default()
            },
            None => Self::default(),
        }
    }
}

/// The supported storage backend in v1 (MF-3) — any other `backend` value is rejected.
pub(crate) const SUPPORTED_BACKEND: &str = "libsql";

/// The merged, validated config value (startup + runtime keys) — the producer that fills
/// [`ResolvedConfig`] and feeds [`crate::ConfigPaths`].
///
/// Built by [`WorkspaceConfig::resolve`] (the precedence engine) and validated by
/// [`WorkspaceConfig::validate`]. `deletions_retention_days` and `backend` are
/// **resolved-but-not-projected** (reserved — v1.1 / v1.3), not silently dropped (MF-3/MF-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceConfig {
    /// The resolved actor (D10 as amended by D57; bounded via [`unblock_model::validate_actor`]).
    pub(crate) actor: String,
    /// The resolved output format.
    pub(crate) output_format: OutputFormat,
    /// The resolved JSONL-export toggle (FR-7).
    pub(crate) jsonl_export: bool,
    /// The resolved search cap (FR-4), bounded to `1..=i64::MAX`.
    pub(crate) search_cap: usize,
    /// The layer that supplied each runtime value (D57).
    pub(crate) sources: RuntimeSources,
    /// The resolved db filename inside `.unblock/` (startup key).
    pub(crate) db_filename: String,
    /// The resolved JSONL filename inside `.unblock/` (startup key).
    pub(crate) jsonl_filename: String,
    /// The resolved, `normalize_prefix`-normalized issue-id prefix (startup key, D21/T1.8).
    pub(crate) id_prefix: String,
    /// The resolved `.write.lock` acquire timeout in ms (startup key, D31/T3.4.1). Clamped UP to
    /// [`MIN_WRITE_LOCK_TIMEOUT_MS`] by the merge.
    pub(crate) write_lock_timeout_ms: u64,
    /// The resolved tombstone retention window (reserved for v1.1; NOT projected).
    pub(crate) deletions_retention_days: Option<u64>,
    /// The resolved backend selector (only `"libsql"` accepted in v1; reserved, NOT projected).
    pub(crate) backend: Option<String>,
}

/// Fold all six layers (PRD §4 D57 clause (1)) highest-first, then normalize the startup values
/// (D21 prefix normalization, D31 write-lock floor). Validates NOTHING — the callers validate the
/// half they own.
fn merge_six(
    cli: &CliOverrides,
    env: &EnvOverrides,
    project: &ProjectConfig,
    user: &ProjectConfig,
    db: &DbLayer,
    os_user: Option<&str>,
) -> WorkspaceConfig {
    let defaults = Defaults::with_os_user(os_user);
    let mut merged = merge_layers(&[
        ConfigLayer::Cli(cli),
        ConfigLayer::Env(env),
        ConfigLayer::Project(project),
        ConfigLayer::User(user),
        ConfigLayer::Db(db),
        ConfigLayer::Defaults(&defaults),
    ]);
    // D21: normalize the resolved id_prefix via the single-home model normalizer. `normalize_prefix`
    // is total — it strips unsupported chars/trailing separators and falls back to "ub" if nothing
    // usable remains — so the engine allocator always receives a valid, non-empty prefix.
    merged.id_prefix = normalize_prefix(&merged.id_prefix);
    // D31/T3.4.1: clamp the resolved write-lock timeout UP to a documented floor. A user value of
    // 0 (or any sub-floor value) would make every normal mutation fail-fast on the slightest
    // contention — a footgun. `0` is reserved for migrate's internal `Duration::ZERO` fail-fast
    // (an explicit L2 code path, MF2), never a user-facing acquire timeout.
    merged.write_lock_timeout_ms = merged.write_lock_timeout_ms.max(MIN_WRITE_LOCK_TIMEOUT_MS);
    merged
}

/// Resolve the RUNTIME keys over the six layers (PRD §4 D57 clause (1)) — the ONE runtime
/// resolution, shared by the storage-bearing open (all six layers), the resolve-only open (`rows`
/// empty: five layers, the DB layer invisible) and [`crate::ConfigReloader::reload_runtime`].
///
/// `rows` are decoded TOLERANTLY (clause (2a)): their faults are RETURNED in
/// [`RuntimeConfig::db_faults`], never raised. Startup keys are ignored.
///
/// # Errors
///
/// [`ConfigError::InvalidValue`] when the resolved actor violates `validate_actor` or the resolved
/// `search_cap` is outside `1..=i64::MAX` (both possible only from a CLI/env/project/user value — a
/// DB row that fails either is a fault, never a contribution).
pub(crate) fn resolve_runtime(
    cli: &CliOverrides,
    env: &EnvOverrides,
    project: &ProjectConfig,
    user: &ProjectConfig,
    rows: &[(String, String)],
    os_user: Option<&str>,
) -> Result<RuntimeConfig, ConfigError> {
    let (db, db_faults) = db_layer(rows);
    let merged = merge_six(cli, env, project, user, &db, os_user);
    merged.validate_runtime()?;
    Ok(merged.into_runtime(db_faults))
}

/// Resolve the STARTUP keys (and the merged value that feeds [`crate::ConfigPaths`]) — phase (1) of
/// the two-phase open (PRD §4 D57 clause (6)). Startup keys resolve over CLI > env > project >
/// defaults only: the user layer is refused any startup key at load and the DB layer carries none.
///
/// # Errors
///
/// [`ConfigError::InvalidValue`] for an unsupported `backend` (MF-3).
pub(crate) fn resolve_startup(
    cli: &CliOverrides,
    env: &EnvOverrides,
    project: &ProjectConfig,
) -> Result<WorkspaceConfig, ConfigError> {
    let merged = merge_six(
        cli,
        env,
        project,
        &ProjectConfig::default(),
        &DbLayer::default(),
        None,
    );
    merged.validate_startup()?;
    Ok(merged)
}

impl WorkspaceConfig {
    /// Resolve the v1 layered config: **CLI > env `UNBLOCK_*` > project `config.toml` > defaults**,
    /// with the actor chain (`--actor` > `UNBLOCK_ACTOR` > project `actor` > `$USER` > `"unblock"`)
    /// and full validation (Seam A + `search_cap` bound + backend). The user and DB layers are EMPTY
    /// here: this is the public precedence-engine seam the FR-13 integration suite drives with
    /// injected layers; the six-layer runtime resolution is [`crate::ConfigReloader::reload_runtime`].
    ///
    /// `env_source` supplies the `$USER` tier; pass the same source the `env` layer was parsed from.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidValue`] if the resolved actor violates the bounds
    /// ([`unblock_model::validate_actor`] — Seam A), `search_cap` is outside `1..=i64::MAX`, or
    /// `backend` is an unsupported value (MF-3).
    pub fn resolve(
        cli: &CliOverrides,
        env: &EnvOverrides,
        project: &ProjectConfig,
        env_source: &dyn EnvSource,
    ) -> Result<Self, ConfigError> {
        let os_user = env_source.get("USER");
        let merged = merge_six(
            cli,
            env,
            project,
            &ProjectConfig::default(),
            &DbLayer::default(),
            os_user.as_deref(),
        );
        merged.validate()?;
        Ok(merged)
    }

    /// Validate the resolved config: the runtime half (Seam A actor bound, `search_cap` bound) and
    /// the startup half (supported backend).
    ///
    /// # Errors
    ///
    /// - [`ConfigError::InvalidValue`] (`key = "actor"`) if the actor violates
    ///   [`unblock_model::validate_actor`] (over-length / NUL / control char — Seam A).
    /// - [`ConfigError::InvalidValue`] (`key = "search_cap"`) if the cap is outside `1..=i64::MAX`
    ///   (D57 clause (4) — a disclosed tightening: a project `search_cap = 0` used to silently empty
    ///   every default search).
    /// - [`ConfigError::InvalidValue`] (`key = "backend"`) if `backend` is set to anything other
    ///   than `"libsql"` (MF-3).
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_runtime()?;
        self.validate_startup()
    }

    /// The runtime half of [`WorkspaceConfig::validate`].
    fn validate_runtime(&self) -> Result<(), ConfigError> {
        // Seam A: bound the resolved actor via the single-home model validator.
        if let Err(field_err) = validate_actor(&self.actor) {
            return Err(ConfigError::InvalidValue {
                key: "actor".to_string(),
                value: self.actor.clone(),
                reason: field_err.reason,
            });
        }
        // D57 clause (4): the search cap is bounded on EVERY layer.
        if !(1..=MAX_SEARCH_CAP).contains(&self.search_cap) {
            return Err(ConfigError::InvalidValue {
                key: "search_cap".to_string(),
                value: self.search_cap.to_string(),
                reason: format!("expected a decimal integer from 1 to {MAX_SEARCH_CAP}"),
            });
        }
        Ok(())
    }

    /// The startup half of [`WorkspaceConfig::validate`].
    fn validate_startup(&self) -> Result<(), ConfigError> {
        // MF-3: only "libsql" is accepted in v1.
        if let Some(backend) = &self.backend
            && backend != SUPPORTED_BACKEND
        {
            return Err(ConfigError::InvalidValue {
                key: "backend".to_string(),
                value: backend.clone(),
                reason: format!(
                    "unsupported backend (only \"{SUPPORTED_BACKEND}\" is supported in v1)"
                ),
            });
        }
        Ok(())
    }

    /// Project the merged config into the engine-facing [`ResolvedConfig`] (INFALLIBLE, MF-5), with
    /// no DB faults. `deletions_retention_days` and `backend` are **deliberately
    /// resolved-but-not-projected** (reserved — v1.1 / v1.3), not silently dropped.
    #[must_use]
    pub fn into_resolved(self) -> ResolvedConfig {
        let startup = self.startup();
        ResolvedConfig {
            startup,
            runtime: self.into_runtime(Vec::new()),
        }
    }

    /// The STARTUP half of this merged value.
    pub(crate) fn startup(&self) -> StartupConfig {
        StartupConfig {
            db_filename: self.db_filename.clone(),
            jsonl_filename: self.jsonl_filename.clone(),
            id_prefix: self.id_prefix.clone(),
            write_lock_timeout_ms: self.write_lock_timeout_ms,
        }
    }

    /// The RUNTIME half of this merged value, carrying `db_faults`.
    pub(crate) fn into_runtime(self, db_faults: Vec<DbConfigFault>) -> RuntimeConfig {
        RuntimeConfig {
            actor: self.actor,
            output_format: self.output_format,
            jsonl_export: self.jsonl_export,
            search_cap: self.search_cap,
            sources: self.sources,
            db_faults,
        }
    }

    /// The resolved actor.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// The resolved output format.
    #[must_use]
    pub fn output_format(&self) -> OutputFormat {
        self.output_format
    }

    /// The resolved JSONL-export toggle (FR-7).
    #[must_use]
    pub fn jsonl_export(&self) -> bool {
        self.jsonl_export
    }

    /// The resolved search-result cap (FR-4).
    #[must_use]
    pub fn search_cap(&self) -> usize {
        self.search_cap
    }

    /// The layer that supplied each runtime value (D57).
    #[must_use]
    pub fn sources(&self) -> RuntimeSources {
        self.sources
    }

    /// The resolved db filename inside `.unblock/` (startup key; consumed by `ConfigPaths::resolve`).
    #[must_use]
    pub fn db_filename(&self) -> &str {
        &self.db_filename
    }

    /// The resolved JSONL filename inside `.unblock/` (startup key; consumed by
    /// `ConfigPaths::resolve`).
    #[must_use]
    pub fn jsonl_filename(&self) -> &str {
        &self.jsonl_filename
    }

    /// The resolved, normalized issue-id prefix (startup key, D21/T1.8; projected to
    /// `ResolvedConfig.startup.id_prefix` and read by the engine allocator at mint time).
    #[must_use]
    pub fn id_prefix(&self) -> &str {
        &self.id_prefix
    }

    /// The resolved `.write.lock` acquire timeout in ms (startup key, D31/T3.4.1; projected to
    /// `ResolvedConfig.startup.write_lock_timeout_ms` and threaded into `open_local`).
    #[must_use]
    pub fn write_lock_timeout_ms(&self) -> u64 {
        self.write_lock_timeout_ms
    }

    /// The resolved tombstone retention window (reserved for v1.1; not projected to `ResolvedConfig`).
    #[must_use]
    pub fn deletions_retention_days(&self) -> Option<u64> {
        self.deletions_retention_days
    }

    /// The resolved backend selector (only `"libsql"` in v1; reserved, not projected).
    #[must_use]
    pub fn backend(&self) -> Option<&str> {
        self.backend.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DB_FILENAME, DEFAULT_SEARCH_CAP, DEFAULT_WRITE_LOCK_TIMEOUT_MS, JSONL_FILENAME,
        MIN_WRITE_LOCK_TIMEOUT_MS, ResolvedConfig, WorkspaceConfig,
    };
    use crate::cli::CliOverrides;
    use crate::env::{EnvOverrides, EnvSource};
    use crate::schema::ProjectConfig;
    use std::collections::HashMap;
    use unblock_model::OutputFormat;

    struct MapEnv(HashMap<String, String>);
    impl MapEnv {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            )
        }
    }
    impl EnvSource for MapEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    fn resolve(
        cli: &CliOverrides,
        project: &ProjectConfig,
        env_pairs: &[(&str, &str)],
    ) -> Result<WorkspaceConfig, crate::error::ConfigError> {
        let map = MapEnv::new(env_pairs);
        let env = EnvOverrides::from_source(&map).expect("env parse");
        WorkspaceConfig::resolve(cli, &env, project, &map)
    }

    #[test]
    fn defaults_match_the_t1_3a_pins() {
        let cfg = ResolvedConfig::default();
        assert_eq!(cfg.runtime.output_format, OutputFormat::Json);
        assert!(!cfg.runtime.jsonl_export);
        assert_eq!(cfg.runtime.search_cap, DEFAULT_SEARCH_CAP);
        assert_eq!(cfg.runtime.search_cap, 50);
        assert_eq!(cfg.runtime.actor, "unblock");
        assert_eq!(cfg.runtime.sources, super::RuntimeSources::default());
        assert!(cfg.runtime.db_faults.is_empty());
        assert_eq!(cfg.startup.db_filename, DB_FILENAME);
        assert_eq!(cfg.startup.db_filename, "unblock.db");
        assert_eq!(cfg.startup.jsonl_filename, JSONL_FILENAME);
        assert_eq!(cfg.startup.jsonl_filename, "issues.jsonl");
    }

    #[test]
    fn resolve_with_no_layers_uses_defaults() {
        let wc =
            resolve(&CliOverrides::default(), &ProjectConfig::default(), &[]).expect("resolve");
        assert_eq!(wc.output_format, OutputFormat::Json);
        assert!(!wc.jsonl_export);
        assert_eq!(wc.search_cap, 50);
        assert_eq!(wc.db_filename, "unblock.db");
        assert_eq!(wc.jsonl_filename, "issues.jsonl");
        assert_eq!(wc.actor, "unblock");
        assert!(wc.deletions_retention_days.is_none());
        assert!(wc.backend.is_none());
    }

    #[test]
    fn actor_precedence_is_the_fork4_global_order() {
        // cli > env > config > $USER > literal.
        let cli = CliOverrides::new().with_actor("cli");
        let project = ProjectConfig {
            actor: Some("cfg".to_string()),
            ..ProjectConfig::default()
        };
        let env = &[("UNBLOCK_ACTOR", "env"), ("USER", "user")];

        assert_eq!(resolve(&cli, &project, env).unwrap().actor, "cli");
        assert_eq!(
            resolve(&CliOverrides::default(), &project, env)
                .unwrap()
                .actor,
            "env"
        );
        assert_eq!(
            resolve(&CliOverrides::default(), &project, &[("USER", "user")])
                .unwrap()
                .actor,
            "cfg"
        );
        assert_eq!(
            resolve(
                &CliOverrides::default(),
                &ProjectConfig::default(),
                &[("USER", "user")]
            )
            .unwrap()
            .actor,
            "user"
        );
        assert_eq!(
            resolve(&CliOverrides::default(), &ProjectConfig::default(), &[])
                .unwrap()
                .actor,
            "unblock"
        );
    }

    #[test]
    fn empty_actor_at_any_layer_is_unset() {
        // Blank cli + blank UNBLOCK_ACTOR -> config wins.
        let cli = CliOverrides::new().with_actor("   ");
        let project = ProjectConfig {
            actor: Some("cfg".to_string()),
            ..ProjectConfig::default()
        };
        assert_eq!(
            resolve(&cli, &project, &[("UNBLOCK_ACTOR", "  ")])
                .unwrap()
                .actor,
            "cfg"
        );
    }

    #[test]
    fn over_long_actor_is_rejected_via_validate_actor() {
        let cli = CliOverrides::new().with_actor("x".repeat(201));
        let err = resolve(&cli, &ProjectConfig::default(), &[]).expect_err("over-long actor");
        match err {
            crate::error::ConfigError::InvalidValue { key, .. } => assert_eq!(key, "actor"),
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn nul_and_control_actor_rejected() {
        for bad in ["ali\0ce", "ali\tce", "ali\nce"] {
            let cli = CliOverrides::new().with_actor(bad);
            let err = resolve(&cli, &ProjectConfig::default(), &[]).expect_err("bad actor");
            assert!(matches!(
                err,
                crate::error::ConfigError::InvalidValue { .. }
            ));
        }
    }

    #[test]
    fn backend_accepts_only_libsql() {
        let ok = ProjectConfig {
            backend: Some("libsql".to_string()),
            ..ProjectConfig::default()
        };
        let wc = resolve(&CliOverrides::default(), &ok, &[]).expect("libsql ok");
        // backend is resolved-but-not-projected (reserved).
        assert_eq!(wc.backend.as_deref(), Some("libsql"));
        let resolved = wc.into_resolved();
        // ResolvedConfig has no backend field (reserved-not-projected, MF-3/MF-5).
        assert_eq!(resolved.startup.db_filename, "unblock.db");

        let bad = ProjectConfig {
            backend: Some("sqlite".to_string()),
            ..ProjectConfig::default()
        };
        let err = resolve(&CliOverrides::default(), &bad, &[]).expect_err("bad backend");
        match err {
            crate::error::ConfigError::InvalidValue { key, .. } => assert_eq!(key, "backend"),
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn into_resolved_is_infallible_and_reserves_fields() {
        let project = ProjectConfig {
            deletions_retention_days: Some(42),
            backend: Some("libsql".to_string()),
            search_cap: Some(123),
            ..ProjectConfig::default()
        };
        let wc = resolve(&CliOverrides::default(), &project, &[]).expect("resolve");
        assert_eq!(wc.deletions_retention_days, Some(42));
        let resolved = wc.into_resolved();
        // The reserved knobs are not on ResolvedConfig; the projected ones round-trip.
        assert_eq!(resolved.runtime.search_cap, 123);
    }

    #[test]
    fn write_lock_timeout_is_clamped_up_to_the_floor() {
        // D31/T3.4.1: a sub-floor user value (the `0` footgun, or any value below the floor) is
        // clamped UP to MIN_WRITE_LOCK_TIMEOUT_MS so it cannot make every mutation fail-fast.
        for sub_floor in [0_u64, 1, 50, MIN_WRITE_LOCK_TIMEOUT_MS - 1] {
            let project = ProjectConfig {
                write_lock_timeout_ms: Some(sub_floor),
                ..ProjectConfig::default()
            };
            let wc = resolve(&CliOverrides::default(), &project, &[]).expect("resolve");
            assert_eq!(
                wc.write_lock_timeout_ms, MIN_WRITE_LOCK_TIMEOUT_MS,
                "sub-floor value {sub_floor} must clamp up to the floor"
            );
            // The clamp round-trips through the infallible projection.
            assert_eq!(
                wc.into_resolved().startup.write_lock_timeout_ms,
                MIN_WRITE_LOCK_TIMEOUT_MS
            );
        }
    }

    #[test]
    fn write_lock_timeout_above_floor_passes_through() {
        // Any value at or above the floor is untouched by the clamp — genuine tuning is preserved.
        for ok in [
            MIN_WRITE_LOCK_TIMEOUT_MS,
            MIN_WRITE_LOCK_TIMEOUT_MS + 1,
            5_000,
            60_000,
        ] {
            let project = ProjectConfig {
                write_lock_timeout_ms: Some(ok),
                ..ProjectConfig::default()
            };
            let wc = resolve(&CliOverrides::default(), &project, &[]).expect("resolve");
            assert_eq!(wc.write_lock_timeout_ms, ok, "value {ok} must pass through");
        }
        // No project override → the default (well above the floor), unchanged.
        let wc =
            resolve(&CliOverrides::default(), &ProjectConfig::default(), &[]).expect("resolve");
        assert_eq!(wc.write_lock_timeout_ms, DEFAULT_WRITE_LOCK_TIMEOUT_MS);
    }

    #[test]
    fn id_prefix_defaults_to_ub_and_resolves_through() {
        // No layer sets it → the "ub" default, projected to ResolvedConfig.
        let wc =
            resolve(&CliOverrides::default(), &ProjectConfig::default(), &[]).expect("resolve");
        assert_eq!(wc.id_prefix, "ub");
        assert_eq!(wc.into_resolved().startup.id_prefix, "ub");
        // The default ResolvedConfig also carries "ub" (T1.3a default).
        assert_eq!(ResolvedConfig::default().startup.id_prefix, "ub");
    }

    #[test]
    fn id_prefix_from_project_is_normalized() {
        // A project override is honoured and normalize_prefix-normalized (lowercased, trimmed,
        // trailing separators stripped) by resolve().
        let project = ProjectConfig {
            id_prefix: Some("  MyProj-  ".to_string()),
            ..ProjectConfig::default()
        };
        let wc = resolve(&CliOverrides::default(), &project, &[]).expect("resolve");
        assert_eq!(wc.id_prefix, "myproj");
        assert_eq!(wc.into_resolved().startup.id_prefix, "myproj");

        // A prefix that normalizes to nothing falls back to the "ub" default (normalize_prefix is total).
        let empty = ProjectConfig {
            id_prefix: Some("!!!".to_string()),
            ..ProjectConfig::default()
        };
        let wc = resolve(&CliOverrides::default(), &empty, &[]).expect("resolve");
        assert_eq!(wc.id_prefix, "ub");
    }

    #[test]
    fn output_format_and_jsonl_export_precedence() {
        let cli = CliOverrides::new()
            .with_output_format(OutputFormat::Robot)
            .with_jsonl_export(true);
        let wc = resolve(&cli, &ProjectConfig::default(), &[]).expect("resolve");
        assert_eq!(wc.output_format, OutputFormat::Robot);
        assert!(wc.jsonl_export);
    }

    /// Golden snapshot of the fully-merged default `WorkspaceConfig` (no layer set; defaults only).
    /// Drift in any default value or a field add/remove fails the check (FR-13 contract pin).
    #[test]
    fn default_workspace_config_golden() {
        // Build with an empty env so $USER does not leak into the snapshot; force the literal actor.
        struct NoEnv;
        impl EnvSource for NoEnv {
            fn get(&self, _key: &str) -> Option<String> {
                None
            }
        }
        let env = EnvOverrides::from_source(&NoEnv).expect("env");
        let wc = WorkspaceConfig::resolve(
            &CliOverrides::default(),
            &env,
            &ProjectConfig::default(),
            &NoEnv,
        )
        .expect("resolve");
        insta::assert_debug_snapshot!("default_workspace_config", wc);
    }
}
