//! `unblock init` (D27/AF-3) — scaffold a new workspace: `.unblock/config.toml` (hand-written TOML,
//! `normalize_prefix`-seeded) + a migrated empty `unblock.db` opened through the config facade (one
//! code path, FR-9 no-drift).
//!
//! NO `.gitignore`, NO `metadata.json`, NO seeded `issues.jsonl` (D13/NFR-6/model-B). Clobber guard:
//! refuse if `config.toml` OR `unblock.db` is already present under the target `.unblock/` without
//! `--force` → a CLI-local `CliError::AlreadyInitialized` (`ConfigError` has none) → exit 2.

use std::path::PathBuf;

use snafu::{ResultExt, ensure};
use unblock_config::{CliOverrides, open_with_storage_with_cli};
use unblock_model::normalize_prefix;

use crate::cli::{GlobalArgs, InitArgs};
use crate::exit::{AlreadyInitializedSnafu, CliError, IoSnafu};
use crate::output::{self, InitReport, ToDiagnosticReport};

/// The scaffolded config filename.
const CONFIG_FILENAME: &str = "config.toml";
/// The scaffolded database filename (default; matches `ResolvedConfig::db_filename`).
const DB_FILENAME: &str = "unblock.db";
/// The default issue-id prefix when `--prefix` is absent (D21).
const DEFAULT_PREFIX: &str = "ub";

/// Run `unblock init`.
///
/// # Errors
/// - [`CliError::AlreadyInitialized`] if the target `.unblock/` already holds a scaffold (no `--force`);
/// - [`CliError::Io`] if creating the directory or writing `config.toml` fails;
/// - [`CliError::Config`] if opening/migrating the fresh database fails;
/// - [`CliError::Render`]/[`CliError::Io`] if rendering / writing the report fails.
pub async fn run(args: &InitArgs, global: &GlobalArgs) -> Result<Option<u8>, CliError> {
    // 1. Resolve the target `.unblock` dir: the explicit `--dir` if set, else `<cwd>/.unblock`.
    let unblock_dir = target_unblock_dir(global)?;

    // 2. Clobber guard (AF-3): refuse if config.toml OR unblock.db already present without `--force`.
    let config_path = unblock_dir.join(CONFIG_FILENAME);
    let db_present = unblock_dir.join(DB_FILENAME).exists();
    ensure!(
        args.force || !(config_path.exists() || db_present),
        AlreadyInitializedSnafu {
            path: unblock_dir.clone(),
        }
    );

    // 3. Create `.unblock/` (mkdir -p).
    std::fs::create_dir_all(&unblock_dir).context(IoSnafu)?;

    // 4. Hand-write config.toml (`ProjectConfig` is Deserialize-only — DR-8). Seed the NORMALIZED prefix.
    let prefix = args
        .prefix
        .as_deref()
        .map_or_else(|| DEFAULT_PREFIX.to_string(), normalize_prefix);
    std::fs::write(&config_path, render_config_toml(&prefix)).context(IoSnafu)?;

    // 5. Open+migrate via the facade to create the migrated empty unblock.db (FR-9 no-drift).
    let overrides = CliOverrides::new().with_dir(&unblock_dir);
    let ctx = open_with_storage_with_cli(&overrides).await?;

    // 6. Report exactly what was scaffolded.
    let fmt = ctx.config.output_format;
    let report = InitReport {
        workspace_dir: ctx.workspace_dir,
        unblock_dir: ctx.paths.unblock_dir,
        db_path: ctx.paths.db_path,
        id_prefix: prefix,
        config_path,
    };
    output::emit_report(&report.to_report(), fmt).map(|()| None)
}

/// The target `.unblock` directory for `init`: the explicit `--dir` if set, else `<cwd>/.unblock`.
fn target_unblock_dir(global: &GlobalArgs) -> Result<PathBuf, CliError> {
    if let Some(dir) = &global.dir {
        return Ok(dir.clone());
    }
    let cwd = std::env::current_dir().context(IoSnafu)?;
    Ok(cwd.join(".unblock"))
}

/// Render the minimal `config.toml` text the resolver deserializes. Only `id_prefix` is seeded (every
/// other value defaults); a comment header records the scaffold provenance.
fn render_config_toml(id_prefix: &str) -> String {
    format!(
        "# unblock workspace config (scaffolded by `unblock init`).\n\
         # See `docs/plans/crates/unblock-config.md` for the full key set; every key not set here\n\
         # falls back to its default.\n\
         id_prefix = \"{id_prefix}\"\n"
    )
}

/// Whether the given `.unblock` dir already holds a scaffold (used by the clobber-guard test).
#[cfg(test)]
fn is_scaffolded(unblock_dir: &std::path::Path) -> bool {
    unblock_dir.join(CONFIG_FILENAME).exists() || unblock_dir.join(DB_FILENAME).exists()
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PREFIX, is_scaffolded, render_config_toml};
    use unblock_config::ProjectConfig;
    use unblock_model::normalize_prefix;

    #[test]
    fn rendered_config_deserializes_and_carries_normalized_prefix() {
        // `--prefix Weird!!` normalizes to lowercase-alnum-only ("weird").
        let prefix = normalize_prefix("Weird!!");
        assert_eq!(prefix, "weird");
        let toml_text = render_config_toml(&prefix);
        // The scaffold must round-trip through the real config deserializer with the seeded prefix.
        let parsed: ProjectConfig =
            toml::from_str(&toml_text).expect("scaffolded config.toml must deserialize");
        assert_eq!(parsed.id_prefix.as_deref(), Some(prefix.as_str()));
    }

    #[test]
    fn default_prefix_is_ub() {
        assert_eq!(DEFAULT_PREFIX, "ub");
        // The default scaffold also round-trips and carries "ub".
        let parsed: ProjectConfig = toml::from_str(&render_config_toml(DEFAULT_PREFIX)).unwrap();
        assert_eq!(parsed.id_prefix.as_deref(), Some("ub"));
    }

    #[test]
    fn is_scaffolded_detects_config_or_db() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        assert!(!is_scaffolded(dir));
        std::fs::write(dir.join("config.toml"), "id_prefix = \"ub\"\n").unwrap();
        assert!(is_scaffolded(dir));
    }
}
