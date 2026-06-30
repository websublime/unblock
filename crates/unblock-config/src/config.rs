//! [`ResolvedConfig`] — the resolved, validated config **VALUES** the engine/`Session` reads
//! (config-owned, spine §4 CF-D) — plus the T1.3 producer [`WorkspaceConfig`] that fills it.
//!
//! [`ResolvedConfig`] is **NOT** paths (those live in [`crate::ConfigPaths`]) and **NOT** the actor
//! (that is the top-level context field, spine §4.1). Its v1 shape is pinned by the crate plan
//! (`docs/plans/crates/unblock-config.md` §2/§3): **DEFAULTED** in the T1.3a minimal subset and
//! **RESOLVED** for real at T1.3 — the shape does not change.
//!
//! [`WorkspaceConfig`] is the merged, validated config value (startup + runtime keys) produced by
//! the layered resolver ([`WorkspaceConfig::resolve`]); it *fills* [`ResolvedConfig`] (and feeds
//! [`crate::ConfigPaths`]) via the infallible [`WorkspaceConfig::into_resolved`] (MF-5). `actor`
//! follows the global precedence FORK-4 (`--actor` > `UNBLOCK_ACTOR` > `config.toml [actor]` >
//! `$USER` > `"unblock"`) and is bounded via [`unblock_model::validate_actor`] (Seam A);
//! `deletions_retention_days` and `backend` are resolved-but-NOT-projected (reserved).

use unblock_model::{OutputFormat, normalize_prefix, validate_actor};

use crate::actor::resolve_actor_layered;
use crate::cli::CliOverrides;
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

/// The resolved, validated config values the engine/`Session` consumes (config-owned, spine §4
/// CF-D).
///
/// Embedded **by value** in both [`crate::ResolvedContext`] and [`crate::WorkspaceContext`]. In the
/// T1.3a minimal subset every field is the locked default ([`ResolvedConfig::default`]); the T1.3
/// layered resolver fills the values for real without changing this shape.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// The output format for rendered command output (re-exported from `unblock-model`, G-7/CF-J).
    /// T1.3a default = [`OutputFormat::Json`] (the model's own `Default`).
    pub output_format: OutputFormat,
    /// Auto-export JSONL after mutating ops (FR-7). T1.3a default = `false`.
    pub jsonl_export: bool,
    /// Search-result cap (FR-4). T1.3a default = `50`.
    pub search_cap: usize,
    /// The database filename inside `.unblock/`. T1.3a default = `"unblock.db"` (PRD §12.5).
    pub db_filename: String,
    /// The JSONL export filename inside `.unblock/`. T1.3a default = `"issues.jsonl"`
    /// (PRD §12.5).
    pub jsonl_filename: String,
    /// The issue-id prefix the engine allocator mints with (D21/T1.8 ADDITIVE). Default = `"ub"`,
    /// `normalize_prefix`-normalized. The engine reads `ctx.config.id_prefix` at mint time to render
    /// `ub-<hash>`/`ub-<slug>-<hash>` (config-derived, not a constant).
    pub id_prefix: String,
}

impl Default for ResolvedConfig {
    /// The T1.3a defaults: `Json` / `false` / `50` / `"unblock.db"` / `"issues.jsonl"` / `"ub"`.
    fn default() -> Self {
        Self {
            output_format: OutputFormat::default(),
            jsonl_export: false,
            search_cap: DEFAULT_SEARCH_CAP,
            db_filename: DB_FILENAME.to_string(),
            jsonl_filename: JSONL_FILENAME.to_string(),
            id_prefix: DEFAULT_ID_PREFIX.to_string(),
        }
    }
}

/// The lowest-precedence defaults layer (FR-13). Guarantees a value for every non-reserved field.
///
/// `actor` carries the **already-resolved** FORK-4 actor (the chain runs in
/// [`WorkspaceConfig::resolve`] before this layer is built), so the merge fold always has a resolved
/// actor at the bottom; `deletions_retention_days`/`backend` carry `None` (reserved — no v1 default).
#[derive(Debug, Clone)]
pub(crate) struct Defaults {
    /// The resolved actor (filled by the FORK-4 chain before merging).
    pub actor: String,
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
    /// Reserved (no v1 default retention window).
    pub deletions_retention_days: Option<u64>,
    /// Reserved (no v1 default backend value; `None` resolves to the libsql default downstream).
    pub backend: Option<String>,
}

impl Default for Defaults {
    /// The locked v1 defaults, with `actor` = the literal `"unblock"` (overwritten by the resolved
    /// FORK-4 actor in [`WorkspaceConfig::resolve`]).
    fn default() -> Self {
        Self {
            actor: crate::actor::DEFAULT_ACTOR.to_string(),
            output_format: OutputFormat::default(),
            jsonl_export: false,
            search_cap: DEFAULT_SEARCH_CAP,
            db_filename: DB_FILENAME.to_string(),
            jsonl_filename: JSONL_FILENAME.to_string(),
            id_prefix: DEFAULT_ID_PREFIX.to_string(),
            deletions_retention_days: None,
            backend: None,
        }
    }
}

/// The supported storage backend in v1 (MF-3) — any other `backend` value is rejected.
pub(crate) const SUPPORTED_BACKEND: &str = "libsql";

/// The merged, validated config value (startup + runtime keys) — the T1.3 producer that fills
/// [`ResolvedConfig`] and feeds [`crate::ConfigPaths`].
///
/// Built by [`WorkspaceConfig::resolve`] (the precedence engine) and validated by
/// [`WorkspaceConfig::validate`]. `deletions_retention_days` and `backend` are
/// **resolved-but-not-projected** (reserved — v1.1 / v1.2), not silently dropped (MF-3/MF-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceConfig {
    /// The resolved actor (FORK-4; bounded via [`unblock_model::validate_actor`]).
    pub(crate) actor: String,
    /// The resolved output format.
    pub(crate) output_format: OutputFormat,
    /// The resolved JSONL-export toggle (FR-7).
    pub(crate) jsonl_export: bool,
    /// The resolved search cap (FR-4).
    pub(crate) search_cap: usize,
    /// The resolved db filename inside `.unblock/` (startup key).
    pub(crate) db_filename: String,
    /// The resolved JSONL filename inside `.unblock/` (startup key).
    pub(crate) jsonl_filename: String,
    /// The resolved, `normalize_prefix`-normalized issue-id prefix (startup key, D21/T1.8). Projected
    /// to [`ResolvedConfig::id_prefix`].
    pub(crate) id_prefix: String,
    /// The resolved tombstone retention window (reserved for v1.1; NOT projected).
    pub(crate) deletions_retention_days: Option<u64>,
    /// The resolved backend selector (only `"libsql"` accepted in v1; reserved, NOT projected).
    pub(crate) backend: Option<String>,
}

impl WorkspaceConfig {
    /// Resolve the layered config (FR-13): **CLI > env `UNBLOCK_*` > project `config.toml` >
    /// defaults**, with the FORK-4 actor chain and full validation (Seam A + backend + paths-feed).
    ///
    /// The merge folds highest-precedence-first (`merge_layers`); the actor is resolved separately
    /// via the global FORK-4 chain and carried in the `Defaults` layer. The result is validated
    /// before it is returned, so [`WorkspaceConfig::into_resolved`] can stay infallible (MF-5).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidValue`] if the resolved actor violates the bounds
    /// ([`unblock_model::validate_actor`] — Seam A) or `backend` is an unsupported value (MF-3).
    ///
    /// `env_source` is the [`EnvSource`] used for the FORK-4 `$USER` lookup; pass the same source
    /// the `env` layer was parsed from. The facades in [`crate::context`] are the production
    /// entrypoints; this is also the public precedence-engine seam the FR-13 integration suite drives
    /// with injected layers (spine §2).
    pub fn resolve(
        cli: &CliOverrides,
        env: &EnvOverrides,
        project: &ProjectConfig,
        env_source: &dyn EnvSource,
    ) -> Result<Self, ConfigError> {
        // FORK-4 actor: --actor > UNBLOCK_ACTOR > config.toml [actor] > $USER > "unblock".
        let resolved_actor =
            resolve_actor_layered(cli.actor.as_deref(), project.actor.as_deref(), env_source)?;

        let defaults = Defaults {
            actor: resolved_actor,
            ..Defaults::default()
        };

        let mut merged = merge_layers(&[
            ConfigLayer::Cli(cli),
            ConfigLayer::Env(env),
            ConfigLayer::Project(project),
            ConfigLayer::Defaults(&defaults),
        ]);

        // D21: normalize the resolved id_prefix via the single-home model normalizer. `normalize_prefix`
        // is total — it strips unsupported chars/trailing separators and falls back to "ub" if nothing
        // usable remains — so the engine allocator always receives a valid, non-empty prefix.
        merged.id_prefix = normalize_prefix(&merged.id_prefix);

        merged.validate()?;
        Ok(merged)
    }

    /// Validate the resolved config (Seam A actor bound + supported backend).
    ///
    /// # Errors
    ///
    /// - [`ConfigError::InvalidValue`] (`key = "actor"`) if the actor violates
    ///   [`unblock_model::validate_actor`] (over-length / NUL / control char — Seam A).
    /// - [`ConfigError::InvalidValue`] (`key = "backend"`) if `backend` is set to anything other
    ///   than `"libsql"` (MF-3).
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Seam A: bound the resolved actor via the single-home model validator.
        if let Err(field_err) = validate_actor(&self.actor) {
            return Err(ConfigError::InvalidValue {
                key: "actor".to_string(),
                value: self.actor.clone(),
                reason: field_err.reason,
            });
        }
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

    /// Project the merged config into the engine-facing [`ResolvedConfig`] (INFALLIBLE, MF-5).
    ///
    /// All validation runs in [`WorkspaceConfig::validate`] (called by [`WorkspaceConfig::resolve`])
    /// **before** this projection, so it adds no new error path. `deletions_retention_days` and
    /// `backend` are **deliberately resolved-but-not-projected** (reserved — v1.1 / v1.2), not
    /// silently dropped.
    #[must_use]
    pub fn into_resolved(self) -> ResolvedConfig {
        ResolvedConfig {
            output_format: self.output_format,
            jsonl_export: self.jsonl_export,
            search_cap: self.search_cap,
            db_filename: self.db_filename,
            jsonl_filename: self.jsonl_filename,
            id_prefix: self.id_prefix,
        }
    }

    /// The resolved actor (top-level context field; spine §4.1).
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
    /// `ResolvedConfig.id_prefix` and read by the engine allocator at mint time).
    #[must_use]
    pub fn id_prefix(&self) -> &str {
        &self.id_prefix
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
    use super::{DB_FILENAME, DEFAULT_SEARCH_CAP, JSONL_FILENAME, ResolvedConfig, WorkspaceConfig};
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
        assert_eq!(cfg.output_format, OutputFormat::Json);
        assert!(!cfg.jsonl_export);
        assert_eq!(cfg.search_cap, DEFAULT_SEARCH_CAP);
        assert_eq!(cfg.search_cap, 50);
        assert_eq!(cfg.db_filename, DB_FILENAME);
        assert_eq!(cfg.db_filename, "unblock.db");
        assert_eq!(cfg.jsonl_filename, JSONL_FILENAME);
        assert_eq!(cfg.jsonl_filename, "issues.jsonl");
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
        assert_eq!(resolved.db_filename, "unblock.db");

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
        assert_eq!(resolved.search_cap, 123);
    }

    #[test]
    fn id_prefix_defaults_to_ub_and_resolves_through() {
        // No layer sets it → the "ub" default, projected to ResolvedConfig.
        let wc = resolve(&CliOverrides::default(), &ProjectConfig::default(), &[]).expect("resolve");
        assert_eq!(wc.id_prefix, "ub");
        assert_eq!(wc.into_resolved().id_prefix, "ub");
        // The default ResolvedConfig also carries "ub" (T1.3a default).
        assert_eq!(ResolvedConfig::default().id_prefix, "ub");
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
        assert_eq!(wc.into_resolved().id_prefix, "myproj");

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
