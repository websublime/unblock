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
//! The target's sibling is the other of `.unblock` and `_unblock` in the same parent, and two
//! checks read the pair. Before the clobber guard, the binds-first check refuses the target with
//! `CliError::SiblingBindsFirst` when the pair's `.unblock` is a directory holding no scaffold and
//! its `_unblock` is the target or holds a scaffold, because discovery at their root binds that
//! `.unblock/` first. After the clobber guard, the sibling guard refuses a target whose sibling
//! holds a scaffold, naming the sibling. Both checks refuse with or without `--force`, and neither
//! fires when the sibling resolves to the target itself.
//!
//! The open forwards the global flags with the target as its `--dir`, so `-o` and `--actor` reach
//! config (the spine §5b forwarder rule). An actor that fails validation fails that open (exit 7)
//! after `config.toml` is written, and `init --force` recovers.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use snafu::{ResultExt, ensure};
use unblock_config::{
    UNBLOCK_DIR_NAMES, has_unblock_dir_name, open_with_storage_with_cli, probe_workspace_root,
};
use unblock_model::normalize_prefix;

use crate::cli::{GlobalArgs, InitArgs};
use crate::exit::{AlreadyInitializedSnafu, CliError, IoSnafu, SiblingBindsFirstSnafu};
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
/// - [`CliError::SiblingBindsFirst`] if discovery at the target's root binds a `.unblock` directory
///   holding no scaffold before an `_unblock` that is the target or holds a scaffold, with or
///   without `--force`;
/// - [`CliError::AlreadyInitialized`] if the target already holds a scaffold (no `--force`), or if
///   a distinct sibling of the target holds one, with or without `--force`;
/// - [`CliError::Io`] if reading the cwd, creating the directory or writing `config.toml` fails;
/// - [`CliError::Config`] if opening/migrating the fresh database fails;
/// - [`CliError::Render`]/[`CliError::Io`] if rendering / writing the report fails.
pub async fn run(args: &InitArgs, global: &GlobalArgs) -> Result<Option<u8>, CliError> {
    // 1. Resolve the target, probing a project root the way discovery does.
    let unblock_dir = target_unblock_dir(global)?;

    // 2. The binds-first check refuses the target when discovery at its root binds a `.unblock`
    //    holding no scaffold before an `_unblock` that is the target or holds one.
    check_binds_first(&unblock_dir)?;

    // 3. The clobber guard (AF-3) refuses a target that already holds a scaffold, unless `--force`.
    ensure!(
        args.force || !is_scaffolded(&unblock_dir),
        AlreadyInitializedSnafu {
            path: unblock_dir.clone(),
        }
    );

    // 4. The sibling guard refuses a target beside a distinct sibling that holds a scaffold.
    check_sibling(&unblock_dir)?;

    // 5. Create the target (mkdir -p).
    std::fs::create_dir_all(&unblock_dir).context(IoSnafu)?;

    // 6. Hand-write config.toml (`ProjectConfig` is Deserialize-only — DR-8). Seed the NORMALIZED prefix.
    let prefix = args
        .prefix
        .as_deref()
        .map_or_else(|| DEFAULT_PREFIX.to_string(), normalize_prefix);
    let config_path = unblock_dir.join(CONFIG_FILENAME);
    std::fs::write(&config_path, render_config_toml(&prefix)).context(IoSnafu)?;

    // 7. Open+migrate via the facade to create the migrated empty unblock.db (FR-9 no-drift). The
    //    open forwards the global flags, and the target replaces any raw `--dir`.
    let overrides = global.to_overrides().with_dir(&unblock_dir);
    let ctx = open_with_storage_with_cli(&overrides).await?;

    // 8. Report exactly what was scaffolded.
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

/// Refuses `target` through `SiblingBindsFirst` when discovery at its root binds a `.unblock`
/// directory that holds no scaffold before the `_unblock` of the pair.
///
/// An `_unblock` target is refused beside such a `.unblock`. A `.unblock` target that is such a
/// directory is refused when the `_unblock` beside it holds a scaffold. A pair that resolves to one
/// directory never blocks.
fn check_binds_first(target: &Path) -> Result<(), CliError> {
    let Some(sibling) = sibling_unblock_dir(target) else {
        return Ok(());
    };
    let target_is_dot = target.file_name() == Some(OsStr::new(NEW_UNBLOCK_DIR));
    if target_is_dot && !is_scaffolded(&sibling) {
        return Ok(());
    }
    let (bound, hidden) = if target_is_dot {
        (target.to_path_buf(), sibling)
    } else {
        (sibling, target.to_path_buf())
    };
    if !bound.is_dir() || is_scaffolded(&bound) || is_same_dir(&bound, &hidden) {
        return Ok(());
    }
    SiblingBindsFirstSnafu { bound, hidden }.fail()
}

/// Refuses `target` through `AlreadyInitialized` naming a distinct sibling that holds a scaffold.
fn check_sibling(target: &Path) -> Result<(), CliError> {
    let Some(sibling) = sibling_unblock_dir(target) else {
        return Ok(());
    };
    if !is_scaffolded(&sibling) || is_same_dir(&sibling, target) {
        return Ok(());
    }
    AlreadyInitializedSnafu { path: sibling }.fail()
}

/// Returns `target` with its workspace-dir name swapped for the other one.
fn sibling_unblock_dir(target: &Path) -> Option<PathBuf> {
    let name = target.file_name()?;
    UNBLOCK_DIR_NAMES
        .iter()
        .find(|other| OsStr::new(other) != name)
        .map(|other| target.with_file_name(other))
}

/// Reports whether `a` and `b` canonicalize to the same directory. A path that fails to
/// canonicalize matches nothing.
fn is_same_dir(a: &Path, b: &Path) -> bool {
    matches!(
        (std::fs::canonicalize(a), std::fs::canonicalize(b)),
        (Ok(canon_a), Ok(canon_b)) if canon_a == canon_b
    )
}

/// Reports whether `unblock_dir` already holds `config.toml` or `unblock.db`.
fn is_scaffolded(unblock_dir: &Path) -> bool {
    unblock_dir.join(CONFIG_FILENAME).exists() || unblock_dir.join(DB_FILENAME).exists()
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{DEFAULT_PREFIX, is_scaffolded, render_config_toml, sibling_unblock_dir};
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

        let db_only = tempfile::tempdir().expect("tempdir");
        std::fs::write(db_only.path().join("unblock.db"), b"").expect("write unblock.db");
        assert!(
            is_scaffolded(db_only.path()),
            "a directory holding only unblock.db holds a scaffold"
        );
    }

    #[test]
    fn sibling_unblock_dir_swaps_the_workspace_dir_name() {
        let cases = [
            ("/p/.unblock", "/p/_unblock"),
            ("/p/_unblock", "/p/.unblock"),
            ("/p/.unblock/", "/p/_unblock"),
            (".unblock", "_unblock"),
        ];
        for (target, sibling) in cases {
            assert_eq!(
                sibling_unblock_dir(Path::new(target)),
                Some(PathBuf::from(sibling)),
                "the sibling of {target}"
            );
        }
    }
}
