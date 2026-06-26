//! The precedence engine (FR-13).
//!
//! v1 order (highest → lowest): **CLI > env (`UNBLOCK_*`) > project `config.toml` > defaults**.
//!
//! **MERGE CONVENTION (NORMATIVE):** the layer slice is folded **HIGHEST-PRECEDENCE-FIRST** —
//! [`merge_layers`] takes `&[ConfigLayer]` ordered highest→lowest and the **first non-`None`** field
//! value (scanning from the head) wins. This is the OPPOSITE of the original beads lowest-first fold;
//! do not flip it. The fold direction is pinned by the [`merge_layers`] doctest.
//!
//! Field-wise picking ([`pick`]) makes the result a fully-resolved [`WorkspaceConfig`]: every field
//! is the value of the highest-precedence layer that set it, with the `Defaults` layer guaranteeing
//! a value for each. The enum is intentionally `pub(crate)` (SF-5) — widening it to a general/public
//! surface in v1.1 (when the user/DB layers land) is a deliberate pre-1.0 widening, not a
//! frozen-surface violation.

use unblock_model::OutputFormat;

use crate::cli::CliOverrides;
use crate::config::{Defaults, WorkspaceConfig};
use crate::env::EnvOverrides;
use crate::schema::ProjectConfig;

/// One precedence layer (highest → lowest in the order they appear in a `merge_layers` slice).
///
/// v1.1 reserves `Db(ProjectConfig)` and `User(ProjectConfig)` slots between `Project` and
/// `Defaults`; v1.2 reserves a remote slot. They are intentionally omitted from this v1 enum (SF-5).
pub(crate) enum ConfigLayer<'a> {
    /// The CLI override layer (highest precedence).
    Cli(&'a CliOverrides),
    /// The `UNBLOCK_*` env layer.
    Env(&'a EnvOverrides),
    /// The project `.unblock/config.toml` layer.
    Project(&'a ProjectConfig),
    // v1.1: Db(ProjectConfig) and User(ProjectConfig) slot here, between Project and Defaults.
    /// The defaults layer (lowest precedence; guarantees a value for every field). The `actor`
    /// carried here is the already-resolved FORK-4 actor (cli/env/config/$USER/literal), so a
    /// higher layer's explicit actor still wins via the first-`Some` fold while the bottom always
    /// has a resolved value.
    Defaults(&'a Defaults),
}

/// The value a single layer contributes for one field — `None` means "this layer did not set it".
trait LayerView {
    fn actor(&self) -> Option<&str>;
    fn output_format(&self) -> Option<OutputFormat>;
    fn jsonl_export(&self) -> Option<bool>;
    fn search_cap(&self) -> Option<usize>;
    fn db_filename(&self) -> Option<&str>;
    fn jsonl_filename(&self) -> Option<&str>;
    fn deletions_retention_days(&self) -> Option<u64>;
    fn backend(&self) -> Option<&str>;
}

impl LayerView for ConfigLayer<'_> {
    fn actor(&self) -> Option<&str> {
        match self {
            // The actor is resolved ENTIRELY by the FORK-4 chain (`resolve_actor_layered`), whose
            // result is carried by the Defaults layer. No other layer contributes to the actor fold
            // here — doing so would mis-order env vs project (the chain already put env above
            // project). So only Defaults sets the actor, and it is the fully-resolved value.
            ConfigLayer::Cli(_) | ConfigLayer::Env(_) | ConfigLayer::Project(_) => None,
            ConfigLayer::Defaults(d) => Some(d.actor.as_str()),
        }
    }
    fn output_format(&self) -> Option<OutputFormat> {
        match self {
            ConfigLayer::Cli(c) => c.output_format,
            ConfigLayer::Env(e) => e.output_format,
            ConfigLayer::Project(p) => p.output_format,
            ConfigLayer::Defaults(d) => Some(d.output_format),
        }
    }
    fn jsonl_export(&self) -> Option<bool> {
        match self {
            ConfigLayer::Cli(c) => c.jsonl_export,
            ConfigLayer::Env(e) => e.jsonl_export,
            ConfigLayer::Project(p) => p.jsonl_export,
            ConfigLayer::Defaults(d) => Some(d.jsonl_export),
        }
    }
    fn search_cap(&self) -> Option<usize> {
        match self {
            // search_cap is not a CLI/env override key in v1.
            ConfigLayer::Cli(_) | ConfigLayer::Env(_) => None,
            ConfigLayer::Project(p) => p.search_cap,
            ConfigLayer::Defaults(d) => Some(d.search_cap),
        }
    }
    fn db_filename(&self) -> Option<&str> {
        match self {
            // db_filename is a startup key; --db (a full path) is handled in ConfigPaths, not here.
            ConfigLayer::Cli(_) | ConfigLayer::Env(_) => None,
            ConfigLayer::Project(p) => p.db_filename.as_deref(),
            ConfigLayer::Defaults(d) => Some(d.db_filename.as_str()),
        }
    }
    fn jsonl_filename(&self) -> Option<&str> {
        match self {
            ConfigLayer::Cli(_) | ConfigLayer::Env(_) => None,
            // In the raw ProjectConfig this key is `jsonl_export_filename`; it projects to the
            // merged `jsonl_filename`.
            ConfigLayer::Project(p) => p.jsonl_export_filename.as_deref(),
            ConfigLayer::Defaults(d) => Some(d.jsonl_filename.as_str()),
        }
    }
    fn deletions_retention_days(&self) -> Option<u64> {
        match self {
            ConfigLayer::Cli(_) | ConfigLayer::Env(_) => None,
            ConfigLayer::Project(p) => p.deletions_retention_days,
            // Defaults carries None (reserved; no default retention window in v1).
            ConfigLayer::Defaults(d) => d.deletions_retention_days,
        }
    }
    fn backend(&self) -> Option<&str> {
        match self {
            ConfigLayer::Cli(_) | ConfigLayer::Env(_) => None,
            ConfigLayer::Project(p) => p.backend.as_deref(),
            ConfigLayer::Defaults(d) => d.backend.as_deref(),
        }
    }
}

/// Pick the first `Some` value produced by `f`, scanning the layers **head-first** (highest
/// precedence first). Returns `None` only if no layer set the field — which cannot happen for a
/// field the `Defaults` layer always supplies.
fn pick<'a, T>(
    layers: &'a [ConfigLayer<'a>],
    f: impl Fn(&'a ConfigLayer<'a>) -> Option<T>,
) -> Option<T> {
    layers.iter().find_map(f)
}

/// Fold the precedence layers into a merged [`WorkspaceConfig`] (FR-13).
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
    WorkspaceConfig {
        actor: pick(layers, |l| l.actor().map(str::to_string))
            .expect("Defaults layer guarantees an actor"),
        output_format: pick(layers, LayerView::output_format)
            .expect("Defaults layer guarantees an output_format"),
        jsonl_export: pick(layers, LayerView::jsonl_export)
            .expect("Defaults layer guarantees a jsonl_export"),
        search_cap: pick(layers, LayerView::search_cap)
            .expect("Defaults layer guarantees a search_cap"),
        db_filename: pick(layers, |l| l.db_filename().map(str::to_string))
            .expect("Defaults layer guarantees a db_filename"),
        jsonl_filename: pick(layers, |l| l.jsonl_filename().map(str::to_string))
            .expect("Defaults layer guarantees a jsonl_filename"),
        // These two are reserved (Defaults supplies None) — not always set, so plain pick.
        deletions_retention_days: pick(layers, LayerView::deletions_retention_days),
        backend: pick(layers, |l| l.backend().map(str::to_string)),
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigLayer, merge_layers};
    use crate::cli::CliOverrides;
    use crate::config::Defaults;
    use crate::env::EnvOverrides;
    use crate::schema::ProjectConfig;
    use unblock_model::OutputFormat;

    fn defaults_with_actor(actor: &str) -> Defaults {
        Defaults {
            actor: actor.to_string(),
            ..Defaults::default()
        }
    }

    #[test]
    fn cli_beats_env_beats_project_beats_defaults_for_output_format() {
        let cli = CliOverrides::new().with_output_format(OutputFormat::Robot);
        let env = EnvOverrides {
            output_format: Some(OutputFormat::Plain),
            ..EnvOverrides::default()
        };
        let project = ProjectConfig {
            output_format: Some(OutputFormat::Csv),
            ..ProjectConfig::default()
        };
        let defaults = defaults_with_actor("unblock");

        // Full stack: cli wins.
        let merged = merge_layers(&[
            ConfigLayer::Cli(&cli),
            ConfigLayer::Env(&env),
            ConfigLayer::Project(&project),
            ConfigLayer::Defaults(&defaults),
        ]);
        assert_eq!(merged.output_format, OutputFormat::Robot);

        // Drop cli: env wins.
        let merged = merge_layers(&[
            ConfigLayer::Cli(&CliOverrides::default()),
            ConfigLayer::Env(&env),
            ConfigLayer::Project(&project),
            ConfigLayer::Defaults(&defaults),
        ]);
        assert_eq!(merged.output_format, OutputFormat::Plain);

        // Drop cli + env: project wins.
        let merged = merge_layers(&[
            ConfigLayer::Cli(&CliOverrides::default()),
            ConfigLayer::Env(&EnvOverrides::default()),
            ConfigLayer::Project(&project),
            ConfigLayer::Defaults(&defaults),
        ]);
        assert_eq!(merged.output_format, OutputFormat::Csv);

        // Drop all but defaults: defaults wins.
        let merged = merge_layers(&[
            ConfigLayer::Cli(&CliOverrides::default()),
            ConfigLayer::Env(&EnvOverrides::default()),
            ConfigLayer::Project(&ProjectConfig::default()),
            ConfigLayer::Defaults(&defaults),
        ]);
        assert_eq!(merged.output_format, OutputFormat::Json);
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
        assert_eq!(merged.deletions_retention_days, Some(99));
        assert_eq!(merged.backend.as_deref(), Some("libsql"));
    }

    #[test]
    fn defaults_actor_used_when_no_project_actor() {
        let defaults = defaults_with_actor("resolved-actor");
        let merged = merge_layers(&[
            ConfigLayer::Cli(&CliOverrides::default()),
            ConfigLayer::Env(&EnvOverrides::default()),
            ConfigLayer::Project(&ProjectConfig::default()),
            ConfigLayer::Defaults(&defaults),
        ]);
        assert_eq!(merged.actor, "resolved-actor");
    }
}
