//! The precedence engine (FR-13).
//!
//! The ONE order since D57 (v1.1), highest → lowest (PRD §4 D57 clause (1)): **CLI > env
//! (`UNBLOCK_*`) > project `.unblock/config.toml` > user `config.toml` > DB `config` table >
//! defaults**. Startup keys resolve over CLI > env > project > defaults only (the user and DB layers
//! never contribute one — clause (2)); each runtime key over every layer that carries it
//! (`search_cap` has no CLI or env slot; `jsonl_export` is not a user/DB layer key; the actor's
//! bottom is the defaults layer's `$USER` > `"unblock"`).
//!
//! **MERGE CONVENTION (NORMATIVE):** the layer slice is folded **HIGHEST-PRECEDENCE-FIRST** —
//! [`merge_layers`] takes `&[ConfigLayer]` ordered highest→lowest and the **first non-`None`** field
//! value (scanning from the head) wins. This is the OPPOSITE of the original beads lowest-first fold;
//! do not flip it. The fold also records WHICH layer supplied each runtime key ([`RuntimeSources`]).
//!
//! [`ConfigLayer`] STAYS `pub(crate)` (D57): SF-5 permits widening it, but nothing outside the crate
//! needs it, and its `Defaults` arm carries the crate-private [`Defaults`].

use unblock_model::{ConfigSource, OutputFormat};

use crate::actor::trimmed_non_empty;
use crate::cli::CliOverrides;
use crate::config::{Defaults, RuntimeSources, WorkspaceConfig};
use crate::db_layer::DbLayer;
use crate::env::EnvOverrides;
use crate::schema::ProjectConfig;

/// One precedence layer (highest → lowest in the order they appear in a `merge_layers` slice).
///
/// v1.3 reserves a remote slot; it is intentionally omitted (SF-5).
pub(crate) enum ConfigLayer<'a> {
    /// The CLI override layer (highest precedence).
    Cli(&'a CliOverrides),
    /// The `UNBLOCK_*` env layer.
    Env(&'a EnvOverrides),
    /// The project `.unblock/config.toml` layer.
    Project(&'a ProjectConfig),
    /// The user `config.toml` layer (D57) — the three LAYER keys only: its loader refuses every
    /// startup key and `jsonl_export`, and this view never reads one from it.
    User(&'a ProjectConfig),
    /// The database `config` table layer (D57) — the sound LAYER-key rows only (faulty rows never
    /// contribute; they travel as `RuntimeConfig::db_faults`).
    Db(&'a DbLayer),
    /// The defaults layer (lowest precedence; guarantees a value for every field). Its `actor` is
    /// the chain's two-tier bottom, `$USER` > `"unblock"`.
    Defaults(&'a Defaults),
}

impl ConfigLayer<'_> {
    /// The [`ConfigSource`] this layer reports for a value it supplied.
    fn source(&self) -> ConfigSource {
        match self {
            ConfigLayer::Cli(_) => ConfigSource::Cli,
            ConfigLayer::Env(_) => ConfigSource::Env,
            ConfigLayer::Project(_) => ConfigSource::Project,
            ConfigLayer::User(_) => ConfigSource::User,
            ConfigLayer::Db(_) => ConfigSource::Db,
            ConfigLayer::Defaults(_) => ConfigSource::Default,
        }
    }

    /// The actor this layer contributes, TRIMMED (a blank value is unset), with its source — the
    /// defaults layer reports `OsUser` for the `$USER` tier.
    fn actor(&self) -> Option<(String, ConfigSource)> {
        let raw = match self {
            ConfigLayer::Cli(c) => c.actor.as_deref(),
            ConfigLayer::Env(e) => e.actor.as_deref(),
            ConfigLayer::Project(p) | ConfigLayer::User(p) => p.actor.as_deref(),
            ConfigLayer::Db(d) => d.actor.as_deref(),
            ConfigLayer::Defaults(d) => return Some((d.actor.clone(), d.actor_source)),
        };
        raw.and_then(trimmed_non_empty)
            .map(|actor| (actor, self.source()))
    }

    fn output_format(&self) -> Option<OutputFormat> {
        match self {
            ConfigLayer::Cli(c) => c.output_format,
            ConfigLayer::Env(e) => e.output_format,
            ConfigLayer::Project(p) | ConfigLayer::User(p) => p.output_format,
            ConfigLayer::Db(d) => d.output_format,
            ConfigLayer::Defaults(d) => Some(d.output_format),
        }
    }

    fn jsonl_export(&self) -> Option<bool> {
        match self {
            ConfigLayer::Cli(c) => c.jsonl_export,
            ConfigLayer::Env(e) => e.jsonl_export,
            ConfigLayer::Project(p) => p.jsonl_export,
            // D57 clause (2): NOT a layer key — never read from the user file or the database.
            ConfigLayer::User(_) | ConfigLayer::Db(_) => None,
            ConfigLayer::Defaults(d) => Some(d.jsonl_export),
        }
    }

    fn search_cap(&self) -> Option<usize> {
        match self {
            // search_cap has no CLI flag and no `UNBLOCK_*` variable.
            ConfigLayer::Cli(_) | ConfigLayer::Env(_) => None,
            ConfigLayer::Project(p) | ConfigLayer::User(p) => p.search_cap,
            ConfigLayer::Db(d) => d.search_cap,
            ConfigLayer::Defaults(d) => Some(d.search_cap),
        }
    }

    // --- STARTUP keys: CLI > env > project > defaults ONLY. The user and DB layers never
    //     contribute one (D57 clause (2)); none has a CLI/env slot in v1. ---

    /// The project layer's startup view; `None` for every other non-defaults layer.
    fn startup_project(&self) -> Option<&ProjectConfig> {
        match self {
            ConfigLayer::Project(p) => Some(p),
            ConfigLayer::Cli(_)
            | ConfigLayer::Env(_)
            | ConfigLayer::User(_)
            | ConfigLayer::Db(_)
            | ConfigLayer::Defaults(_) => None,
        }
    }

    fn write_lock_timeout_ms(&self) -> Option<u64> {
        match self {
            ConfigLayer::Defaults(d) => Some(d.write_lock_timeout_ms),
            other => other
                .startup_project()
                .and_then(|p| p.write_lock_timeout_ms),
        }
    }

    fn db_filename(&self) -> Option<&str> {
        match self {
            // --db (a full path) is handled in ConfigPaths, not here.
            ConfigLayer::Defaults(d) => Some(d.db_filename.as_str()),
            other => other
                .startup_project()
                .and_then(|p| p.db_filename.as_deref()),
        }
    }

    fn jsonl_filename(&self) -> Option<&str> {
        match self {
            ConfigLayer::Defaults(d) => Some(d.jsonl_filename.as_str()),
            // In the raw ProjectConfig this key is `jsonl_export_filename`; it projects to the
            // merged `jsonl_filename`.
            other => other
                .startup_project()
                .and_then(|p| p.jsonl_export_filename.as_deref()),
        }
    }

    fn id_prefix(&self) -> Option<&str> {
        match self {
            ConfigLayer::Defaults(d) => Some(d.id_prefix.as_str()),
            other => other.startup_project().and_then(|p| p.id_prefix.as_deref()),
        }
    }

    fn deletions_retention_days(&self) -> Option<u64> {
        match self {
            // Defaults carries None (reserved; no default retention window in v1).
            ConfigLayer::Defaults(d) => d.deletions_retention_days,
            other => other
                .startup_project()
                .and_then(|p| p.deletions_retention_days),
        }
    }

    fn backend(&self) -> Option<&str> {
        match self {
            ConfigLayer::Defaults(d) => d.backend.as_deref(),
            other => other.startup_project().and_then(|p| p.backend.as_deref()),
        }
    }
}

/// Pick the first `Some` value produced by `f`, scanning the layers **head-first** (highest
/// precedence first), with the source of the layer that supplied it.
fn pick<'a, T>(
    layers: &'a [ConfigLayer<'a>],
    f: impl Fn(&'a ConfigLayer<'a>) -> Option<T>,
) -> Option<(T, ConfigSource)> {
    layers
        .iter()
        .find_map(|layer| f(layer).map(|value| (value, layer.source())))
}

/// Fold the precedence layers into a merged [`WorkspaceConfig`] (FR-13), recording the source of
/// each runtime value.
///
/// The slice MUST be ordered **highest precedence first**; the first layer that sets a field wins.
/// The `Defaults` layer (last) guarantees a value for every field, so the merged config is total.
///
/// # Panics
///
/// Never panics in practice: every field is guaranteed by the `Defaults` layer. The `expect`s below
/// document that invariant — a missing `Defaults` layer is a programmer error, not an input error.
///
/// # Example (pins the highest-first fold direction)
///
/// ```ignore
/// // (internal — `ConfigLayer`/`Defaults` are pub(crate)). Highest-first: the head layer wins.
/// // cli sets output_format = Robot; project sets it = Plain; cli (the head) wins -> Robot.
/// ```
pub(crate) fn merge_layers(layers: &[ConfigLayer<'_>]) -> WorkspaceConfig {
    let (actor, actor_source) = layers
        .iter()
        .find_map(ConfigLayer::actor)
        .expect("Defaults layer guarantees an actor");
    let (output_format, output_format_source) = pick(layers, ConfigLayer::output_format)
        .expect("Defaults layer guarantees an output_format");
    let (jsonl_export, jsonl_export_source) =
        pick(layers, ConfigLayer::jsonl_export).expect("Defaults layer guarantees a jsonl_export");
    let (search_cap, search_cap_source) =
        pick(layers, ConfigLayer::search_cap).expect("Defaults layer guarantees a search_cap");
    WorkspaceConfig {
        actor,
        output_format,
        jsonl_export,
        search_cap,
        sources: RuntimeSources {
            actor: actor_source,
            output_format: output_format_source,
            jsonl_export: jsonl_export_source,
            search_cap: search_cap_source,
        },
        write_lock_timeout_ms: pick(layers, ConfigLayer::write_lock_timeout_ms)
            .expect("Defaults layer guarantees a write_lock_timeout_ms")
            .0,
        db_filename: pick(layers, |l| l.db_filename().map(str::to_string))
            .expect("Defaults layer guarantees a db_filename")
            .0,
        jsonl_filename: pick(layers, |l| l.jsonl_filename().map(str::to_string))
            .expect("Defaults layer guarantees a jsonl_filename")
            .0,
        id_prefix: pick(layers, |l| l.id_prefix().map(str::to_string))
            .expect("Defaults layer guarantees an id_prefix")
            .0,
        // These two are reserved (Defaults supplies None) — not always set, so plain pick.
        deletions_retention_days: pick(layers, ConfigLayer::deletions_retention_days).map(|v| v.0),
        backend: pick(layers, |l| l.backend().map(str::to_string)).map(|v| v.0),
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigLayer, merge_layers};
    use crate::cli::CliOverrides;
    use crate::config::Defaults;
    use crate::db_layer::DbLayer;
    use crate::env::EnvOverrides;
    use crate::schema::ProjectConfig;
    use unblock_model::{ConfigSource, OutputFormat};

    fn defaults_with_actor(actor: &str) -> Defaults {
        Defaults {
            actor: actor.to_string(),
            ..Defaults::default()
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one row per layer of the six-layer order, side by side.
    fn cli_beats_env_beats_project_beats_user_beats_db_beats_defaults_for_output_format() {
        let cli = CliOverrides::new().with_output_format(OutputFormat::Robot);
        let env = EnvOverrides {
            output_format: Some(OutputFormat::Plain),
            ..EnvOverrides::default()
        };
        let project = ProjectConfig {
            output_format: Some(OutputFormat::Csv),
            ..ProjectConfig::default()
        };
        let user = ProjectConfig {
            output_format: Some(OutputFormat::Markdown),
            ..ProjectConfig::default()
        };
        let db = DbLayer {
            output_format: Some(OutputFormat::Plain),
            ..DbLayer::default()
        };
        let defaults = defaults_with_actor("unblock");
        let none_cli = CliOverrides::default();
        let none_env = EnvOverrides::default();
        let none_file = ProjectConfig::default();
        let none_db = DbLayer::default();

        let cases: [(&[ConfigLayer<'_>], OutputFormat, ConfigSource); 6] = [
            (
                &[
                    ConfigLayer::Cli(&cli),
                    ConfigLayer::Env(&env),
                    ConfigLayer::Project(&project),
                    ConfigLayer::User(&user),
                    ConfigLayer::Db(&db),
                    ConfigLayer::Defaults(&defaults),
                ],
                OutputFormat::Robot,
                ConfigSource::Cli,
            ),
            (
                &[
                    ConfigLayer::Cli(&none_cli),
                    ConfigLayer::Env(&env),
                    ConfigLayer::Project(&project),
                    ConfigLayer::User(&user),
                    ConfigLayer::Db(&db),
                    ConfigLayer::Defaults(&defaults),
                ],
                OutputFormat::Plain,
                ConfigSource::Env,
            ),
            (
                &[
                    ConfigLayer::Cli(&none_cli),
                    ConfigLayer::Env(&none_env),
                    ConfigLayer::Project(&project),
                    ConfigLayer::User(&user),
                    ConfigLayer::Db(&db),
                    ConfigLayer::Defaults(&defaults),
                ],
                OutputFormat::Csv,
                ConfigSource::Project,
            ),
            (
                &[
                    ConfigLayer::Cli(&none_cli),
                    ConfigLayer::Env(&none_env),
                    ConfigLayer::Project(&none_file),
                    ConfigLayer::User(&user),
                    ConfigLayer::Db(&db),
                    ConfigLayer::Defaults(&defaults),
                ],
                OutputFormat::Markdown,
                ConfigSource::User,
            ),
            (
                &[
                    ConfigLayer::Cli(&none_cli),
                    ConfigLayer::Env(&none_env),
                    ConfigLayer::Project(&none_file),
                    ConfigLayer::User(&none_file),
                    ConfigLayer::Db(&db),
                    ConfigLayer::Defaults(&defaults),
                ],
                OutputFormat::Plain,
                ConfigSource::Db,
            ),
            (
                &[
                    ConfigLayer::Cli(&none_cli),
                    ConfigLayer::Env(&none_env),
                    ConfigLayer::Project(&none_file),
                    ConfigLayer::User(&none_file),
                    ConfigLayer::Db(&none_db),
                    ConfigLayer::Defaults(&defaults),
                ],
                OutputFormat::Json,
                ConfigSource::Default,
            ),
        ];
        for (layers, format, source) in cases {
            let merged = merge_layers(layers);
            assert_eq!(merged.output_format, format);
            assert_eq!(merged.sources.output_format, source);
        }
    }

    #[test]
    fn project_db_filename_overrides_default() {
        let project = ProjectConfig {
            db_filename: Some("custom.db".to_string()),
            jsonl_export_filename: Some("export.jsonl".to_string()),
            search_cap: Some(7),
            deletions_retention_days: Some(99),
            backend: Some("libsql".to_string()),
            ..ProjectConfig::default()
        };
        let defaults = defaults_with_actor("unblock");
        let merged = merge_layers(&[
            ConfigLayer::Cli(&CliOverrides::default()),
            ConfigLayer::Env(&EnvOverrides::default()),
            ConfigLayer::Project(&project),
            ConfigLayer::Defaults(&defaults),
        ]);
        assert_eq!(merged.db_filename, "custom.db");
        assert_eq!(merged.jsonl_filename, "export.jsonl");
        assert_eq!(merged.search_cap, 7);
        assert_eq!(merged.sources.search_cap, ConfigSource::Project);
        assert_eq!(merged.deletions_retention_days, Some(99));
        assert_eq!(merged.backend.as_deref(), Some("libsql"));
    }

    /// **D57 criterion (b) — STARTUP keys are never read from the user layer.** Even a user layer
    /// that (bypassing its loader) carried startup values contributes none of them, nor
    /// `jsonl_export`.
    ///
    /// MUTANT KILLED: folding `User(p)` into the `Project(p)` arm of a startup view.
    #[test]
    fn the_user_layer_never_supplies_a_startup_key_or_jsonl_export() {
        let user = ProjectConfig {
            db_filename: Some("user.db".to_string()),
            jsonl_export_filename: Some("user.jsonl".to_string()),
            id_prefix: Some("usr".to_string()),
            write_lock_timeout_ms: Some(5_000),
            jsonl_export: Some(true),
            backend: Some("other".to_string()),
            deletions_retention_days: Some(3),
            ..ProjectConfig::default()
        };
        let defaults = Defaults::default();
        let merged = merge_layers(&[
            ConfigLayer::User(&user),
            ConfigLayer::Db(&DbLayer::default()),
            ConfigLayer::Defaults(&defaults),
        ]);
        assert_eq!(merged.db_filename, defaults.db_filename);
        assert_eq!(merged.jsonl_filename, defaults.jsonl_filename);
        assert_eq!(merged.id_prefix, defaults.id_prefix);
        assert_eq!(merged.write_lock_timeout_ms, defaults.write_lock_timeout_ms);
        assert!(!merged.jsonl_export);
        assert_eq!(merged.sources.jsonl_export, ConfigSource::Default);
        assert_eq!(merged.backend, None);
        assert_eq!(merged.deletions_retention_days, None);
    }

    #[test]
    fn defaults_actor_used_when_no_higher_actor() {
        let defaults = defaults_with_actor("resolved-actor");
        let merged = merge_layers(&[
            ConfigLayer::Cli(&CliOverrides::default()),
            ConfigLayer::Env(&EnvOverrides::default()),
            ConfigLayer::Project(&ProjectConfig::default()),
            ConfigLayer::Defaults(&defaults),
        ]);
        assert_eq!(merged.actor, "resolved-actor");
        assert_eq!(merged.sources.actor, ConfigSource::Default);
    }
}
