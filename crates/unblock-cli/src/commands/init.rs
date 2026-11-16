//! `unblock init` (D27/AF-3) scaffolds a new workspace. It hand-writes `config.toml`, seeded with
//! the `normalize_prefix`-normalized prefix, and creates a migrated empty `unblock.db` through the
//! config facade, the same open path `mcp`, `migrate` and `doctor` take (FR-9). It writes no
//! `.gitignore`, `metadata.json` or `issues.jsonl` (D13/NFR-6/model-B).
//!
//! A `--dir` named `.unblock` or `_unblock` is the target as given. Any other `--dir`, or the cwd
//! without one, is a project root, which `init` probes with discovery's own child probe. The target
//! is the root's existing `.unblock/`, else its existing `_unblock/`, else a new `.unblock/`, so
//! discovery at that root later binds the directory `init` scaffolds. `init` never walks up and
//! never reads `CLAUDE_PROJECT_DIR`.
//!
//! Three checks run before anything is written, and each refuses with exit 2. The target's sibling
//! is the other of `.unblock` and `_unblock` in the same parent. The binds-first check refuses the
//! target with `CliError::SiblingBindsFirst` when the pair's `.unblock` is a directory holding no
//! scaffold and its `_unblock` is the target or holds a scaffold, because discovery at their root
//! binds that `.unblock/` first. The clobber guard then picks the mode. A target holding neither
//! `config.toml` nor `unblock.db`, or any target under `--force`, is scaffolded. A target holding
//! `unblock.db` without `config.toml`, or `config.toml` under `--prefix`, is refused through the
//! CLI-local `CliError::AlreadyInitialized`. Any other target is a database-only candidate. The
//! sibling guard last refuses a target whose sibling holds a scaffold, naming the sibling. The
//! binds-first check and the sibling guard refuse with or without `--force`, and neither fires when
//! the sibling resolves to the target itself.
//!
//! The DATABASE-ONLY mode (PRD §4 D58 clause (7)) recreates the database under an existing
//! `config.toml`. It resolves that config without opening the database, so a malformed one exits 7
//! with nothing written, and reads the configured database path, which honours `db_filename`. A
//! non-empty database there is refused through `AlreadyInitialized`. A `-wal`, `-shm` or `-journal`
//! beside the absent or 0-byte database is refused through `CliError::OrphanSidecars` (exit 2),
//! because opening a database beside it lets `SQLite` delete or replay it. Otherwise the open creates
//! and migrates only the database and writes `.write.lock`; `config.toml` stays byte-identical.
//! Every `init` open sets `CliOverrides::create_db`, so a lost database is created instead of
//! refused.
//!
//! The open forwards the global flags with the target as its `--dir`, so `-o` and `--actor` reach
//! config (the spine §5b forwarder rule). An actor that fails validation fails that open (exit 7)
//! after `config.toml` is written — and, because the open is two-phase (PRD §4 D57 clause (6): the
//! runtime keys resolve only after the database is opened and migrated), after `unblock.db` is
//! created — and `init --force` recovers.
//!
//! Since D57 (clause (6)) `init` loads the user file through `unblock_config::load_user_config`
//! after the three checks and before it writes anything, so a user file the open would refuse fails
//! `init` with exit 7 and nothing written. After its open, a blocking fault in the database `config`
//! table fails `init` with `ConfigError::FaultyDbConfig` (exit 7) before any `--agents` write.
//!
//! `init --agents` (v1.1) then writes the managed `AGENTS.md` block through the write `agents`
//! runs, at the root of the workspace its own open bound, before the report renders. A bare `init`
//! prints a one-line stderr hint after the report instead, unless `-q` is set. The hint, a failed
//! `AGENTS.md` write and an `AlreadyInitialized` refusal under `--agents` all end with the same
//! retry text. It is the command `unblock agents --dir` followed by the canonical workspace dir,
//! single-quoted for the host's target shells.

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use snafu::ResultExt;
use unblock_config::{
    CliOverrides, ProcessEnv, UNBLOCK_DIR_NAMES, has_unblock_dir_name, load_user_config,
    open_with_storage_with_cli, open_workspace_with_cli, probe_workspace_root,
};
use unblock_model::normalize_prefix;

use crate::cli::{GlobalArgs, InitArgs};
use crate::commands::agents;
use crate::exit::{
    AlreadyInitializedSnafu, CliError, InitAgentsWriteSnafu, IoSnafu, OrphanSidecarsSnafu,
    SiblingBindsFirstSnafu,
};
use crate::output::{self, InitMode, InitReport, ToDiagnosticReport};

/// The scaffolded config filename.
const CONFIG_FILENAME: &str = "config.toml";
/// The scaffolded database filename (default; matches `ResolvedConfig::db_filename`).
const DB_FILENAME: &str = "unblock.db";
/// The default issue-id prefix when `--prefix` is absent (D21).
const DEFAULT_PREFIX: &str = "ub";
/// `init` names a new workspace dir with the name discovery probes first.
const NEW_UNBLOCK_DIR: &str = UNBLOCK_DIR_NAMES[0];
/// The `SQLite` sidecar suffixes the database-only mode refuses to open beside, in report order.
const SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// Run `unblock init`.
///
/// # Errors
/// - [`CliError::SiblingBindsFirst`] if discovery at the target's root binds a `.unblock` directory
///   holding no scaffold before an `_unblock` that is the target or holds a scaffold, with or
///   without `--force`;
/// - [`CliError::AlreadyInitialized`] (no `--force`) if the target holds `unblock.db` without
///   `config.toml`, holds `config.toml` under `--prefix`, or holds a non-empty configured database;
///   with or without `--force`, if a distinct sibling of the target holds a scaffold;
/// - [`CliError::OrphanSidecars`] if the database-only mode finds a `-wal`, `-shm` or `-journal`
///   beside the absent or 0-byte database;
/// - [`CliError::Io`] if reading the cwd, creating the directory, writing `config.toml` or reading
///   the database's file state fails;
/// - [`CliError::Config`] if the user file is refused or the existing `config.toml` is malformed
///   (both before anything is written), if opening/migrating the database fails, or if its
///   `config` table holds a blocking fault (`FaultyDbConfig`, before any `--agents` write);
/// - [`CliError::InitAgentsWrite`] if `--agents` cannot write `AGENTS.md` after the open;
/// - [`CliError::Render`]/[`CliError::Io`] if rendering / writing the report fails.
pub async fn run(args: &InitArgs, global: &GlobalArgs) -> Result<Option<u8>, CliError> {
    // (1) Resolve the target, probing a project root the way discovery does. The binds-first
    //     check refuses it when discovery at its root binds a `.unblock` holding no scaffold
    //     before an `_unblock` that is the target or holds one.
    let unblock_dir = target_unblock_dir(global)?;
    check_binds_first(&unblock_dir)?;

    // (2) The clobber guard (AF-3, D58) refuses or picks the mode.
    let mode = clobber_guard(&unblock_dir, args)?;

    // (3) The sibling guard refuses a target beside a distinct sibling that holds a scaffold.
    check_sibling(&unblock_dir, args.agents)?;

    // (3b) Load the user file BEFORE any write (PRD §4 D57 clause (6)): `load_user_config` is the
    //      single home of every user-file refusal, the same function the open below runs, so a
    //      user file the open would refuse fails `init` with exit 7 and NOTHING written.
    load_user_config(&ProcessEnv)?;

    // The open forwards the global flags, and the target replaces any raw `--dir`.
    let overrides = global.to_overrides().with_dir(&unblock_dir);
    let config_path = unblock_dir.join(CONFIG_FILENAME);
    let seeded_prefix = match mode {
        InitMode::Scaffold => Some(write_scaffold(&unblock_dir, &config_path, args)?),
        InitMode::DatabaseOnly => {
            // (4)-(6) Resolve the existing config and check the configured database.
            check_database_only(&unblock_dir, &overrides, args.agents).await?;
            None
        }
    };

    // (7) Open+migrate via the facade to create the migrated empty database (FR-9 no-drift).
    //     `create_db` lets the open create a database the `.write.lock` marker calls lost.
    let ctx = open_with_storage_with_cli(&overrides.with_create_db(true)).await?;

    // (8) A blocking database config fault fails `init` with the `issue config unset` hint BEFORE
    //     any `--agents` write (PRD §4 D57 clause (6)). A fresh database has no rows and passes; an
    //     `init --force` over a faulty table has already rewritten `config.toml`.
    if let Some(err) = ctx.config.runtime.blocking_fault_error() {
        return Err(err.into());
    }

    // (9) Under `--agents`, write the managed block at the root this open bound, before the
    //     report renders, so stdout never carries a second document.
    let agents_path = if args.agents {
        let path = agents::agents_path(&ctx.workspace_dir);
        agents::write_managed_block(&path)
            .await
            .context(InitAgentsWriteSnafu {
                unblock_dir: ctx.paths.unblock_dir.clone(),
                path: path.clone(),
            })?;
        Some(path)
    } else {
        None
    };

    // (10) Report what this run created. The database-only mode names the resolved prefix.
    let fmt = ctx.config.runtime.output_format;
    let report = InitReport {
        workspace_dir: ctx.workspace_dir,
        unblock_dir: ctx.paths.unblock_dir,
        db_path: ctx.paths.db_path,
        id_prefix: seeded_prefix.unwrap_or(ctx.config.startup.id_prefix),
        config_path,
        mode,
        agents_path,
    };
    output::emit_report(&report.to_report(), fmt)?;

    // A bare `init` names the next step on stderr, only after the report is out.
    if !args.agents && !global.quiet {
        output::diag(&agents_hint(&report.unblock_dir));
    }
    Ok(None)
}

/// The clobber guard (AF-3 as amended by PRD §4 D58 clause (7)) picks the mode for `target`.
///
/// `--force` scaffolds. Without it, a target holding neither `config.toml` nor `unblock.db` is
/// scaffolded, `unblock.db` without `config.toml` and `config.toml` under `--prefix` are refused,
/// and any other target is a database-only candidate.
fn clobber_guard(target: &Path, args: &InitArgs) -> Result<InitMode, CliError> {
    if args.force {
        return Ok(InitMode::Scaffold);
    }
    let has_config = target.join(CONFIG_FILENAME).exists();
    if !has_config && !target.join(DB_FILENAME).exists() {
        return Ok(InitMode::Scaffold);
    }
    if !has_config || args.prefix.is_some() {
        return Err(already_initialized(target.to_path_buf(), args.agents));
    }
    Ok(InitMode::DatabaseOnly)
}

/// Creates `unblock_dir` and hand-writes `config_path` with the normalized prefix, which it returns.
fn write_scaffold(
    unblock_dir: &Path,
    config_path: &Path,
    args: &InitArgs,
) -> Result<String, CliError> {
    std::fs::create_dir_all(unblock_dir).context(IoSnafu)?;
    // `ProjectConfig` is Deserialize-only (DR-8), so the TOML text is written here.
    let prefix = args
        .prefix
        .as_deref()
        .map_or_else(|| DEFAULT_PREFIX.to_string(), normalize_prefix);
    std::fs::write(config_path, render_config_toml(&prefix)).context(IoSnafu)?;
    Ok(prefix)
}

/// Steps (4)-(6) of the database-only mode, which write nothing.
///
/// (4) resolves the existing config without opening the database, which validates it and yields
/// the configured database path; (5) refuses a non-empty database there; (6) refuses an orphan
/// sidecar beside the absent or 0-byte database.
async fn check_database_only(
    target: &Path,
    overrides: &CliOverrides,
    agents: bool,
) -> Result<(), CliError> {
    let resolved = open_workspace_with_cli(overrides).await?;
    let db_path = resolved.paths.db_path;
    if !database_is_missing(&db_path)? {
        return Err(already_initialized(target.to_path_buf(), agents));
    }
    let files = orphan_sidecars(&db_path)?;
    if !files.is_empty() {
        return OrphanSidecarsSnafu { files }.fail();
    }
    Ok(())
}

/// Reports whether the database at `db_path` is absent or a 0-byte regular file.
fn database_is_missing(db_path: &Path) -> Result<bool, CliError> {
    match std::fs::metadata(db_path) {
        Ok(meta) => Ok(meta.is_file() && meta.len() == 0),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(true),
        Err(source) => Err(CliError::Io { source }),
    }
}

/// Lists the `-wal`, `-shm` and `-journal` entries present beside `db_path`, in that order. Any
/// directory entry counts, a dangling symlink included.
fn orphan_sidecars(db_path: &Path) -> Result<Vec<PathBuf>, CliError> {
    let mut files = Vec::new();
    for suffix in SIDECAR_SUFFIXES {
        let mut name = db_path.as_os_str().to_owned();
        name.push(suffix);
        let path = PathBuf::from(name);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => files.push(path),
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(source) => return Err(CliError::Io { source }),
        }
    }
    Ok(files)
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
fn check_sibling(target: &Path, agents: bool) -> Result<(), CliError> {
    let Some(sibling) = sibling_unblock_dir(target) else {
        return Ok(());
    };
    if !is_scaffolded(&sibling) || is_same_dir(&sibling, target) {
        return Ok(());
    }
    Err(already_initialized(sibling, agents))
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

/// Builds the `AlreadyInitialized` refusal naming `dir`. Under `--agents` its message ends with the
/// retry text for `dir`.
fn already_initialized(dir: PathBuf, agents: bool) -> CliError {
    let retry = agents.then(|| agents_retry(&canonical_existing(&dir)));
    AlreadyInitializedSnafu { path: dir, retry }.build()
}

/// Canonicalizes an existing directory exactly as discovery canonicalizes an explicit `--dir`.
fn canonical_existing(dir: &Path) -> PathBuf {
    unblock_config::discover_unblock_dir(None, &CliOverrides::new().with_dir(dir), &NoEnv)
        .unwrap_or_else(|_| dir.to_path_buf())
}

/// An `EnvSource` that holds no variables. The explicit-dir tier never reads the environment, so
/// this source is exact there.
struct NoEnv;

impl unblock_config::EnvSource for NoEnv {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
}

/// Names the shells a printed retry command must stay inert in.
#[derive(Debug, Clone, Copy)]
enum TargetShells {
    /// Covers the sh-family shells (sh, bash, zsh, dash and ksh), interactive or not.
    Posix,
    /// Covers PowerShell and Git Bash.
    Windows,
}

/// A Windows build prints for the Windows family, and every other build prints for the Posix one.
const HOST_SHELLS: TargetShells = if cfg!(windows) {
    TargetShells::Windows
} else {
    TargetShells::Posix
};

/// PowerShell reads each of these characters as a single quote.
const WINDOWS_SINGLE_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201a}', '\u{201b}'];

/// Builds the retry text that ends the hint, the `InitAgentsWrite` message and the refusal suffix.
pub(crate) fn agents_retry(unblock_dir: &Path) -> String {
    retry_for(unblock_dir, HOST_SHELLS)
}

/// Builds the retry text for `unblock_dir` as `shells` must read it.
///
/// `sanitize_inline` escapes control bytes but keeps U+2028 and U+2029, which some readers treat as
/// line breaks. Both become the text `\u{2028}` and `\u{2029}` here.
fn retry_for(unblock_dir: &Path, shells: TargetShells) -> String {
    let path = unblock_render::sanitize_inline(&unblock_dir.display().to_string())
        .replace('\u{2028}', r"\u{2028}")
        .replace('\u{2029}', r"\u{2029}");
    format!("unblock agents --dir {}", single_quote(&path, shells))
}

/// Single-quotes `text` for `shells`, escaping every character that ends a single-quoted span in
/// those shells.
fn single_quote(text: &str, shells: TargetShells) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for ch in text.chars() {
        match shells {
            TargetShells::Posix if ch == '\'' => quoted.push_str(r"'\''"),
            TargetShells::Windows if WINDOWS_SINGLE_QUOTES.contains(&ch) => {
                quoted.push(ch);
                quoted.push(ch);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('\'');
    quoted
}

/// Builds the one-line hint a bare `init` prints, which ends with the retry text.
fn agents_hint(unblock_dir: &Path) -> String {
    format!(
        "hint: to write the AGENTS.md block for this workspace, run {}",
        agents_retry(unblock_dir)
    )
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

    use super::{
        DEFAULT_PREFIX, TargetShells, agents_hint, clobber_guard, database_is_missing,
        is_scaffolded, orphan_sidecars, render_config_toml, retry_for, sibling_unblock_dir,
    };
    use crate::cli::InitArgs;
    use crate::exit::CliError;
    use crate::output::InitMode;
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

    /// The `InitArgs` for `--prefix` and `--force` as given, without `--agents`.
    fn init_args(prefix: Option<&str>, force: bool) -> InitArgs {
        InitArgs {
            prefix: prefix.map(str::to_string),
            force,
            agents: false,
        }
    }

    /// The clobber guard picks the mode or refuses, per PRD §4 D58 clause (7) step (2).
    #[test]
    fn clobber_guard_picks_the_mode_or_refuses() {
        let empty = tempfile::tempdir().expect("tempdir");
        let config_only = tempfile::tempdir().expect("tempdir");
        std::fs::write(config_only.path().join("config.toml"), "").expect("write config.toml");
        let db_only = tempfile::tempdir().expect("tempdir");
        std::fs::write(db_only.path().join("unblock.db"), b"").expect("write unblock.db");

        let mode = |dir: &Path, args: &InitArgs| clobber_guard(dir, args).ok();
        let bare = init_args(None, false);
        let prefixed = init_args(Some("x"), false);
        let forced = init_args(None, true);
        assert_eq!(mode(empty.path(), &bare), Some(InitMode::Scaffold));
        assert_eq!(mode(empty.path(), &prefixed), Some(InitMode::Scaffold));
        assert_eq!(
            mode(config_only.path(), &bare),
            Some(InitMode::DatabaseOnly)
        );
        for dir in [config_only.path(), db_only.path()] {
            assert_eq!(mode(dir, &forced), Some(InitMode::Scaffold));
        }
        for (dir, args) in [(config_only.path(), &prefixed), (db_only.path(), &bare)] {
            assert!(
                matches!(
                    clobber_guard(dir, args),
                    Err(CliError::AlreadyInitialized { .. })
                ),
                "{} under {args:?} is refused",
                dir.display()
            );
        }
    }

    #[test]
    fn database_is_missing_for_an_absent_or_zero_byte_file_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("unblock.db");
        assert!(database_is_missing(&db).expect("stat"), "absent");
        std::fs::write(&db, b"").expect("write unblock.db");
        assert!(database_is_missing(&db).expect("stat"), "0 bytes");
        std::fs::write(&db, b"x").expect("write unblock.db");
        assert!(!database_is_missing(&db).expect("stat"), "non-empty");
        let dir = tmp.path().join("dir.db");
        std::fs::create_dir(&dir).expect("mkdir dir.db");
        assert!(!database_is_missing(&dir).expect("stat"), "a directory");
    }

    #[test]
    fn orphan_sidecars_lists_present_sidecars_in_wal_shm_journal_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("tracker.db");
        assert!(orphan_sidecars(&db).expect("stat").is_empty());
        for suffix in ["-journal", "-wal", "-shm"] {
            std::fs::write(tmp.path().join(format!("tracker.db{suffix}")), b"")
                .expect("write a sidecar");
        }
        std::fs::write(tmp.path().join("unblock.db-wal"), b"").expect("another db's sidecar");
        let names: Vec<String> = orphan_sidecars(&db)
            .expect("stat")
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["tracker.db-wal", "tracker.db-shm", "tracker.db-journal"]
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

    // -- The retry text (ub-lp9.14) -----------------------------------------------------------
    //
    // These cells call `retry_for` with each shell family, so both quoting rules run on every host.
    // The four typographic single quotes are named by code point, because they look alike.

    const LEFT: char = '\u{2018}';
    const RIGHT: char = '\u{2019}';
    const LOW: char = '\u{201a}';
    const REVERSED: char = '\u{201b}';

    /// Asserts the retry text `shells` get for each `(path, expected)` pair, byte for byte.
    fn assert_retries(shells: TargetShells, cases: &[(&str, &str)]) {
        for (path, expected) in cases {
            assert_eq!(
                retry_for(Path::new(path), shells),
                *expected,
                "the {shells:?} retry text for {path:?}"
            );
        }
    }

    #[test]
    fn retry_quotes_a_plain_path_in_both_families() {
        assert_retries(
            TargetShells::Posix,
            &[(
                "/ws/proj/.unblock",
                "unblock agents --dir '/ws/proj/.unblock'",
            )],
        );
        assert_retries(
            TargetShells::Windows,
            &[(
                r"C:\ws\proj\.unblock",
                r"unblock agents --dir 'C:\ws\proj\.unblock'",
            )],
        );
    }

    #[test]
    fn posix_retry_keeps_bang_dollar_backtick_and_space_literal() {
        assert_retries(
            TargetShells::Posix,
            &[
                ("/t/a!b/.unblock", "unblock agents --dir '/t/a!b/.unblock'"),
                (
                    "/t/!#:3;id;#/.unblock",
                    "unblock agents --dir '/t/!#:3;id;#/.unblock'",
                ),
                (
                    "/t/$HOME/.unblock",
                    "unblock agents --dir '/t/$HOME/.unblock'",
                ),
                (
                    "/t/`id`/.unblock",
                    "unblock agents --dir '/t/`id`/.unblock'",
                ),
                ("/t/a b/.unblock", "unblock agents --dir '/t/a b/.unblock'"),
            ],
        );
    }

    #[test]
    fn posix_retry_writes_a_single_quote_as_close_escape_reopen() {
        assert_retries(
            TargetShells::Posix,
            &[
                (
                    "/t/it's/.unblock",
                    r"unblock agents --dir '/t/it'\''s/.unblock'",
                ),
                (
                    "/t/'q'/.unblock",
                    r"unblock agents --dir '/t/'\''q'\''/.unblock'",
                ),
            ],
        );
    }

    #[test]
    fn posix_retry_keeps_typographic_quotes_literal() {
        let path = format!("/t/{LEFT}a{RIGHT}{LOW}b{REVERSED} “c”/.unblock");
        let expected =
            format!("unblock agents --dir '/t/{LEFT}a{RIGHT}{LOW}b{REVERSED} “c”/.unblock'");
        assert_retries(TargetShells::Posix, &[(path.as_str(), expected.as_str())]);
    }

    #[test]
    fn windows_retry_single_quotes_a_path_without_single_quotes() {
        assert_retries(
            TargetShells::Windows,
            &[
                (
                    r"C:\ws\proj\.unblock",
                    r"unblock agents --dir 'C:\ws\proj\.unblock'",
                ),
                (
                    r"C:\t\a b\$HOME\`id`\!#\“c”\.unblock",
                    r"unblock agents --dir 'C:\t\a b\$HOME\`id`\!#\“c”\.unblock'",
                ),
            ],
        );
    }

    #[test]
    fn windows_retry_doubles_every_single_quote_form() {
        for quote in ['\'', LEFT, RIGHT, LOW, REVERSED] {
            let path = format!(r"C:\t\it{quote}s\.unblock");
            let expected = format!(r"unblock agents --dir 'C:\t\it{quote}{quote}s\.unblock'");
            assert_retries(TargetShells::Windows, &[(path.as_str(), expected.as_str())]);
        }

        let path = format!(r"C:\t\{LEFT}a{RIGHT}{LOW}b{REVERSED}\.unblock");
        let doubled = format!("{LEFT}{LEFT}a{RIGHT}{RIGHT}{LOW}{LOW}b{REVERSED}{REVERSED}");
        let expected = format!(r"unblock agents --dir 'C:\t\{doubled}\.unblock'");
        assert_retries(TargetShells::Windows, &[(path.as_str(), expected.as_str())]);
    }

    #[test]
    fn retry_sanitizes_control_bytes_in_both_families() {
        let cases = [
            (
                TargetShells::Posix,
                "/t/a\u{1b}b/.unblock",
                r"unblock agents --dir '/t/a\u{1b}b/.unblock'",
            ),
            (
                TargetShells::Posix,
                "/t/a\nb/.unblock",
                r"unblock agents --dir '/t/a\nb/.unblock'",
            ),
            (
                TargetShells::Windows,
                "C:\\t\\a\u{1b}b\\.unblock",
                r"unblock agents --dir 'C:\t\a\u{1b}b\.unblock'",
            ),
            (
                TargetShells::Windows,
                "C:\\t\\a\nb\\.unblock",
                r"unblock agents --dir 'C:\t\a\nb\.unblock'",
            ),
        ];
        for (shells, path, expected) in cases {
            let retry = retry_for(Path::new(path), shells);
            assert_eq!(retry, expected, "the {shells:?} retry text for {path:?}");
            assert!(
                !retry.chars().any(char::is_control),
                "no control character survives: {retry:?}"
            );
        }
    }

    #[test]
    fn retry_escapes_line_and_paragraph_separators_in_both_families() {
        let cases = [
            (
                TargetShells::Posix,
                "/t/a\u{2028}b\u{2029}c/.unblock",
                r"unblock agents --dir '/t/a\u{2028}b\u{2029}c/.unblock'",
            ),
            (
                TargetShells::Windows,
                "C:\\t\\a\u{2028}b\u{2029}c\\.unblock",
                r"unblock agents --dir 'C:\t\a\u{2028}b\u{2029}c\.unblock'",
            ),
        ];
        for (shells, path, expected) in cases {
            let retry = retry_for(Path::new(path), shells);
            assert_eq!(retry, expected, "the {shells:?} retry text for {path:?}");
            assert!(
                !retry.contains(['\u{2028}', '\u{2029}']),
                "no separator survives: {retry:?}"
            );
        }
    }

    #[test]
    fn agents_hint_is_one_line_ending_in_the_retry_text() {
        let hint = agents_hint(Path::new("/ws/proj/.unblock"));
        assert_eq!(
            hint,
            "hint: to write the AGENTS.md block for this workspace, run \
             unblock agents --dir '/ws/proj/.unblock'"
        );
        assert!(!hint.contains('\n'), "one line: {hint:?}");
        let (before, _) = hint
            .split_once("unblock agents --dir ")
            .expect("the hint ends with the retry text");
        assert!(
            !before.contains('\''),
            "no quote precedes the retry text: {before:?}"
        );
    }
}
