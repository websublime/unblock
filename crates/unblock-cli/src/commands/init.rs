//! `unblock init` (D27/AF-3) — scaffold a new workspace: `.unblock/config.toml` (hand-written TOML,
//! `normalize_prefix`-seeded) + a migrated empty `unblock.db` opened through the config facade (one
//! code path, FR-9 no-drift).
//!
//! NO `.gitignore`, NO `metadata.json`, NO seeded `issues.jsonl` (D13/NFR-6/model-B). Clobber guard:
//! refuse if `config.toml` OR `unblock.db` is already present under the target without `--force` →
//! a CLI-local `CliError::AlreadyInitialized` (`ConfigError` has none) → exit 2.
//!
//! A `--dir` named `.unblock` or `_unblock` is the target as given. Any other `--dir`, or the cwd
//! without one, is a project root, which `init` probes with discovery's own child probe. The target
//! is the root's existing `.unblock/`, else its existing `_unblock/`, else a new `.unblock/`, so
//! discovery at that root later binds the directory `init` scaffolds. `init` never walks up and
//! never reads `CLAUDE_PROJECT_DIR`.
//!
//! The open forwards the global flags with the target as its `--dir`, so `-o` and `--actor` reach
//! config (the spine §5b forwarder rule). An actor that fails validation fails that open (exit 7)
//! after `config.toml` is written, and `init --force` recovers.

use std::path::PathBuf;

use snafu::{ResultExt, ensure};
use unblock_config::{
    UNBLOCK_DIR_NAMES, has_unblock_dir_name, open_with_storage_with_cli, probe_workspace_root,
};
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
/// `init` names a new workspace dir with the name discovery probes first.
const NEW_UNBLOCK_DIR: &str = UNBLOCK_DIR_NAMES[0];

/// Run `unblock init`.
///
/// # Errors
/// - [`CliError::AlreadyInitialized`] if the target already holds a scaffold (no `--force`);
/// - [`CliError::Io`] if reading the cwd, creating the directory or writing `config.toml` fails;
/// - [`CliError::Config`] if opening/migrating the fresh database fails;
/// - [`CliError::Render`]/[`CliError::Io`] if rendering / writing the report fails.
pub async fn run(args: &InitArgs, global: &GlobalArgs) -> Result<Option<u8>, CliError> {
    // 1. Resolve the target, probing a project root the way discovery does.
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

    // 3. Create the target (mkdir -p).
    std::fs::create_dir_all(&unblock_dir).context(IoSnafu)?;

    // 4. Hand-write config.toml (`ProjectConfig` is Deserialize-only — DR-8). Seed the NORMALIZED prefix.
    let prefix = args
        .prefix
        .as_deref()
        .map_or_else(|| DEFAULT_PREFIX.to_string(), normalize_prefix);
    std::fs::write(&config_path, render_config_toml(&prefix)).context(IoSnafu)?;

    // 5. Open+migrate via the facade to create the migrated empty unblock.db (FR-9 no-drift). The
    //    open forwards the global flags, and the target replaces any raw `--dir`.
    let overrides = global.to_overrides().with_dir(&unblock_dir);
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

/// Resolves the directory `init` scaffolds.
///
/// A `--dir` named `.unblock` or `_unblock` is the target as given. Any other `--dir`, or the cwd
/// without one, is a project root. Its target is the workspace dir discovery's child probe finds
/// there, or a new `.unblock` when the root holds none.
fn target_unblock_dir(global: &GlobalArgs) -> Result<PathBuf, CliError> {
    let root = match &global.dir {
        Some(dir) if has_unblock_dir_name(dir) => return Ok(dir.clone()),
        Some(root) => root.clone(),
        None => std::env::current_dir().context(IoSnafu)?,
    };
    Ok(probe_workspace_root(&root).unwrap_or_else(|| root.join(NEW_UNBLOCK_DIR)))
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
