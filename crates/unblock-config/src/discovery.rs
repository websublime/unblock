//! `.unblock/` workspace discovery (single-workspace only, D11 — no town/mayor routing).
//!
//! **Precedence (MF-2):** `cli.dir` (the `--dir`/`UNBLOCK_DIR` override), when set, is the EXPLICIT
//! dir — used directly, **NO walk-up**. Only when `cli.dir` is unset does discovery **walk up**
//! ancestors from `start` (or CWD) looking for the nearest dir named **`.unblock` OR `_unblock`**
//! (the monorepo alias, FORK-2/D8). An explicit `--db` under a workspace dir derives the dir.
//!
//! **Seam C (FORK-3/SF-3):** the **discovered** dir is **canonicalized** (a symlinked workspace dir
//! is allowed but resolved to its canonical path) so [`crate::ConfigPaths::resolve`] confines
//! artifacts to the canonicalized subtree (NFR-18). The discovered dir always exists, so it is safe
//! to canonicalize; `start` (which may not exist on `init`) is **never** canonicalized.
//!
//! [`discover_unblock_dir`] returns the **`unblock_dir`** (the actual `.unblock`/`_unblock`
//! directory). The `workspace_dir` (project root) is its parent.

use std::path::{Path, PathBuf};

use crate::cli::CliOverrides;
use crate::env::EnvSource;
use crate::error::ConfigError;

/// Which precedence tier of workspace discovery bound the `unblock_dir` (D39).
///
/// Carried on both contexts (spine §4) so the CLI can report the binding and the winning tier at
/// startup. ADDITIVE — not part of the MCP wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSource {
    /// An explicit `--dir` / `UNBLOCK_DIR` (`cli.dir`).
    ExplicitDir,
    /// An explicit `--db` whose path derived the workspace dir.
    ExplicitDb,
    /// The host-injected `CLAUDE_PROJECT_DIR` project root (a ROOT-probe, D39).
    ProjectDir,
    /// The guarded cwd walk-up (the fallback).
    WalkUp,
}

impl WorkspaceSource {
    /// The human tier label used in the D39 startup-visibility line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ExplicitDir => "--dir/UNBLOCK_DIR",
            Self::ExplicitDb => "--db",
            Self::ProjectDir => "CLAUDE_PROJECT_DIR",
            Self::WalkUp => "walk-up from cwd",
        }
    }
}

/// The result of workspace discovery: the resolved `unblock_dir` + which tier bound it (D39).
pub(crate) struct DiscoveredWorkspace {
    pub(crate) unblock_dir: PathBuf,
    pub(crate) source: WorkspaceSource,
}

/// Whether `name` is a workspace-dir name: `.unblock` OR `_unblock` (FORK-2/D8 monorepo alias).
fn is_unblock_dir_name(name: &str) -> bool {
    name == ".unblock" || name == "_unblock"
}

/// Discover the active `unblock_dir` (the `.unblock`/`_unblock` directory itself).
///
/// Total precedence (MF-2 + D39): **`--db` (derive) > `--dir`/`UNBLOCK_DIR` (`cli.dir`, explicit, NO
/// walk-up) > `CLAUDE_PROJECT_DIR` (a project ROOT probe via `env`, NO walk-up; miss → fall through) >
/// guarded cwd walk-up.** `CLAUDE_PROJECT_DIR` and `$HOME`/`%USERPROFILE%` are read via the injected
/// [`EnvSource`] (production [`crate::env::ProcessEnv`]; tests [`crate::env::EnvSource`]-backed
/// `MapEnv`, never the process-global env — NFR-16). The discovered dir is **canonicalized** (Seam C).
///
/// The public return is the `unblock_dir` alone; [`discover_workspace`] carries which tier won so
/// [`crate::context`] can populate the context `source` field (D39). The two `&Path` facades and the
/// two `_with_cli` overloads stay signature-stable — the `env` is threaded below them.
///
/// # Errors
///
/// Returns [`ConfigError::WorkspaceNotFound`] when no `.unblock`/`_unblock` is found, or
/// [`ConfigError::InvalidValue`] when an explicit `--dir`/`--db` does not point at a workspace.
pub fn discover_unblock_dir(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<PathBuf, ConfigError> {
    discover_workspace(start, cli, env).map(|discovered| discovered.unblock_dir)
}

/// Discover the active workspace, returning the `unblock_dir` **and** the tier that bound it (D39).
///
/// The single home of the discovery precedence chain (see [`discover_unblock_dir`]).
///
/// # Errors
///
/// As [`discover_unblock_dir`].
pub(crate) fn discover_workspace(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<DiscoveredWorkspace, ConfigError> {
    // 1. An explicit --db under a workspace dir derives the dir (highest priority — original parity).
    if let Some(db) = cli.db.as_deref()
        && let Some(dir) = derive_dir_from_db(db)
    {
        return Ok(DiscoveredWorkspace {
            unblock_dir: canonicalize_existing(&dir),
            source: WorkspaceSource::ExplicitDb,
        });
    }

    // 2. An explicit --dir/UNBLOCK_DIR is used directly, NO walk-up (MF-2).
    if let Some(dir) = cli.dir.as_deref() {
        return Ok(DiscoveredWorkspace {
            unblock_dir: resolve_explicit_dir(dir)?,
            source: WorkspaceSource::ExplicitDir,
        });
    }

    // 3. CLAUDE_PROJECT_DIR (ambient project ROOT, D39): resolve it exactly like an explicit `--dir` —
    //    it is the `unblock_dir` itself when it points straight AT a `.unblock`/`_unblock` dir, else
    //    its `.unblock`/`_unblock` child (a workspace root), self-recognizing SYMMETRICALLY with
    //    `resolve_explicit_dir`. On a miss (neither shape) fall THROUGH to the guarded walk-up — it is
    //    a host hint, not a per-invocation user choice, so a miss must not hard-error.
    if let Some(root) = env_path(env, "CLAUDE_PROJECT_DIR")
        && let Some(dir) = resolve_root_or_self(&root)
    {
        return Ok(DiscoveredWorkspace {
            unblock_dir: canonicalize_existing(&dir),
            source: WorkspaceSource::ProjectDir,
        });
    }

    // 4. Otherwise walk up from `start` (or CWD) for the nearest `.unblock`/`_unblock`, BOUNDED by the
    //    D39 guard (`$HOME` / a `.git` repo root).
    let dir = walk_up_for_unblock_dir(start, home_dir(env).as_deref())?;
    Ok(DiscoveredWorkspace {
        unblock_dir: canonicalize_existing(&dir),
        source: WorkspaceSource::WalkUp,
    })
}

/// Discover the `unblock_dir` but allow "no workspace" when no explicit `--db` was provided.
///
/// For `init`/no-workspace commands. Suppresses [`ConfigError::WorkspaceNotFound`] (returns
/// `Ok(None)`) only when `cli.db` is unset; an explicit `--db`/`--dir` error still propagates.
///
/// # Errors
///
/// Propagates any error other than a suppressed [`ConfigError::WorkspaceNotFound`].
pub fn discover_optional_unblock_dir(
    start: Option<&Path>,
    cli: &CliOverrides,
    env: &dyn EnvSource,
) -> Result<Option<PathBuf>, ConfigError> {
    match discover_unblock_dir(start, cli, env) {
        Ok(path) => Ok(Some(path)),
        Err(ConfigError::WorkspaceNotFound { .. }) if cli.db.is_none() => Ok(None),
        Err(err) => Err(err),
    }
}

/// Extract the `unblock_dir` from a db path: `…/.unblock/unblock.db` → `…/.unblock` (FORK-2 aware).
///
/// Walks up the db path looking for an ancestor named `.unblock`/`_unblock`; returns `None` if the
/// path has no such component (an external db override — handled by normal discovery).
pub(crate) fn derive_dir_from_db(db: &Path) -> Option<PathBuf> {
    unblock_dir_from_db(db)
}

fn unblock_dir_from_db(db: &Path) -> Option<PathBuf> {
    let mut current = db.to_path_buf();
    loop {
        if current
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_unblock_dir_name)
        {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Resolve an explicit `--dir`/`UNBLOCK_DIR`: it must be (or contain) a `.unblock`/`_unblock` dir.
///
/// If the path itself is named `.unblock`/`_unblock`, it is the workspace dir. Otherwise the path is
/// treated as a workspace **root** and its `.unblock`/`_unblock` child is used. No walk-up (MF-2).
fn resolve_explicit_dir(dir: &Path) -> Result<PathBuf, ConfigError> {
    if let Some(candidate) = resolve_root_or_self(dir) {
        return Ok(canonicalize_existing(&candidate));
    }
    Err(ConfigError::InvalidValue {
        key: "--dir".to_string(),
        value: dir.display().to_string(),
        reason: "not a workspace (no .unblock/ or _unblock/ directory)".to_string(),
    })
}

/// Resolve a path aimed at a workspace to its `unblock_dir`, self-recognizing BOTH shapes (no
/// walk-up): the path is the `unblock_dir` ITSELF when it is named `.unblock`/`_unblock` and is a
/// directory, else it is a workspace **root** and its `.unblock`/`_unblock` child is used. Returns
/// the (non-canonical — the caller canonicalizes) `unblock_dir` on a hit, else `None`; never errors.
///
/// Shared by explicit-`--dir` resolution ([`resolve_explicit_dir`]) and the D39 `CLAUDE_PROJECT_DIR`
/// ROOT-probe so the two tiers self-recognize SYMMETRICALLY — a value pointing straight AT the
/// `.unblock`/`_unblock` dir is honored by either, and the two cannot drift apart on that self-check.
fn resolve_root_or_self(dir: &Path) -> Option<PathBuf> {
    // The path is ITSELF the unblock dir.
    if dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_unblock_dir_name)
        && dir.is_dir()
    {
        return Some(dir.to_path_buf());
    }
    // Or the path is a workspace ROOT holding `.unblock`/`_unblock`.
    probe_workspace_root(dir)
}

/// Probe `root` as a workspace ROOT: return its `.unblock`/`_unblock` child dir on a hit, else `None`.
///
/// The workspace-ROOT half of [`resolve_root_or_self`]. Returns the child path (non-canonical — the
/// caller canonicalizes); never errors (a miss is a `None`, not a failure).
fn probe_workspace_root(root: &Path) -> Option<PathBuf> {
    for name in [".unblock", "_unblock"] {
        let candidate = root.join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Walk up the ancestors of `start` (or CWD) for the nearest `.unblock`/`_unblock` directory,
/// **bounded** by the D39 guard.
///
/// At each ancestor the workspace dir is probed **first**; the walk then **stops** (INCLUSIVE — the
/// boundary dir itself is probed before the stop) at the first of: (i) a **repository root** — a dir
/// holding a `.git` entry (`dir.join(".git").exists()`, a plain `std::fs` stat — NOT a git operation
/// and NO git library linked, so D13/NFR-6 hold), or (ii) the user's **home** directory (`home`, read
/// from the injected [`EnvSource`]). This turns a repo-blind `cwd = $HOME` spawn from a silent bind of
/// an unrelated far-up `.unblock` (a wrong-DB integrity hazard) into an explicit
/// [`ConfigError::WorkspaceNotFound`].
fn walk_up_for_unblock_dir(
    start: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, ConfigError> {
    let begin = match start {
        Some(path) => absolutize(path),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    // A directory inspects itself first; a file path inspects its parent first. `is_dir` follows
    // symlinks and is false for a non-existent path, so a non-existent `start` walks its parents.
    let mut current: Option<&Path> = if begin.is_dir() {
        Some(begin.as_path())
    } else {
        begin.parent()
    };

    while let Some(dir) = current {
        // Probe FIRST — the boundary dir itself is inspected before the stop (INCLUSIVE).
        for name in [".unblock", "_unblock"] {
            let candidate = dir.join(name);
            if candidate.is_dir() {
                return Ok(candidate);
            }
        }
        // D39 guard: stop AFTER probing the boundary dir; never ascend above it.
        let at_repo_root = dir.join(".git").exists(); // `std::fs` stat — NOT a git op (D13/NFR-6).
        let at_home = home.is_some_and(|h| same_dir(dir, h));
        if at_repo_root || at_home {
            break;
        }
        current = dir.parent();
    }

    Err(ConfigError::WorkspaceNotFound {
        start: start.map_or_else(|| begin.clone(), Path::to_path_buf),
    })
}

/// Read a filesystem path from the injected env (`key`), trimming and treating empty as unset.
fn env_path(env: &dyn EnvSource, key: &str) -> Option<PathBuf> {
    env.get(key).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    })
}

/// The user's home dir for the D39 walk-up guard: `$HOME` (Unix) or `%USERPROFILE%` (Windows), read
/// via the injected [`EnvSource`] (never the process-global env — NFR-16). Best-effort: `None` when
/// the spawned env omits both (a sparse GUI env), which disables only the `$HOME` guard arm.
fn home_dir(env: &dyn EnvSource) -> Option<PathBuf> {
    env_path(env, "HOME").or_else(|| env_path(env, "USERPROFILE"))
}

/// Whether two existing dirs are the same, tolerating symlinked/relative forms via canonicalization
/// (falling back to a lexical compare when either side cannot be canonicalized).
fn same_dir(a: &Path, b: &Path) -> bool {
    match (dunce::canonicalize(a), dunce::canonicalize(b)) {
        (Ok(canon_a), Ok(canon_b)) => canon_a == canon_b,
        _ => a == b,
    }
}

/// Canonicalize an existing discovered dir (Seam C). Falls back to the path on canonicalize failure
/// (e.g. a platform that cannot canonicalize) so discovery never hard-fails on a present dir.
fn canonicalize_existing(dir: &Path) -> PathBuf {
    // `dunce::canonicalize` avoids Windows `\\?\` verbatim prefixes; it resolves symlinks like
    // `std::fs::canonicalize`. The discovered dir always exists, so this is safe (never canonicalize
    // `start`, which may not exist).
    match dunce::canonicalize(dir) {
        Ok(canonical) => canonical,
        Err(error) => {
            // The degrade is observable: confinement (Seam B) will use the non-canonical base, so a
            // symlinked workspace dir is no longer resolved to its real path. Behaviour is unchanged
            // (we still fall back so discovery never hard-fails) — only the warning is new.
            tracing::warn!(
                target: "unblock.config",
                dir = %dir.display(),
                %error,
                "failed to canonicalize the discovered workspace dir; path confinement will use \
                 the non-canonical base"
            );
            dir.to_path_buf()
        }
    }
}

/// Turn `path` into an absolute path without resolving symlinks or requiring it to exist.
fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WorkspaceSource, derive_dir_from_db, discover_optional_unblock_dir, discover_unblock_dir,
        discover_workspace,
    };
    use crate::cli::CliOverrides;
    use crate::env::EnvSource;
    use crate::error::ConfigError;
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    /// An injected [`EnvSource`] for the discovery tests — NEVER the process-global env (NFR-16: a
    /// host `$HOME` could otherwise bound a tempdir walk — the macOS-masks-Linux landmine).
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

    /// The no-boundary env for the migrated (pre-D39) cases: no `CLAUDE_PROJECT_DIR`, no `$HOME` — so
    /// the walk is bounded only by a real `.git`/filesystem root (a clean tempdir has neither), and the
    /// prior unbounded-walk assertions keep holding.
    fn empty_env() -> MapEnv {
        MapEnv::new(&[])
    }

    // -- migrated pre-D39 cases (MapEnv with no boundary set; assertions unchanged) ----------------

    #[test]
    fn walks_up_to_nearest_dot_unblock() {
        let root = tempfile::tempdir().expect("tempdir");
        let ws = root.path().join("project");
        fs::create_dir_all(ws.join(".unblock")).expect("mkdir .unblock");
        let nested = ws.join("a").join("b");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let found = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &empty_env())
            .expect("discover");
        assert_eq!(found, ws.join(".unblock").canonicalize().expect("canon"));
    }

    #[test]
    fn walks_up_to_underscore_alias() {
        let root = tempfile::tempdir().expect("tempdir");
        let ws = root.path().join("project");
        fs::create_dir_all(ws.join("_unblock")).expect("mkdir _unblock");
        let nested = ws.join("x");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let found = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &empty_env())
            .expect("discover");
        assert_eq!(found, ws.join("_unblock").canonicalize().expect("canon"));
    }

    #[test]
    fn explicit_dir_used_without_walk_up() {
        let root = tempfile::tempdir().expect("tempdir");
        // A workspace exists at root, but the explicit dir points at a DIFFERENT dir that DOES have
        // .unblock; walk-up must NOT kick in (the explicit dir is used directly).
        fs::create_dir_all(root.path().join(".unblock")).expect("mkdir root .unblock");
        let other = root.path().join("other");
        fs::create_dir_all(other.join(".unblock")).expect("mkdir other .unblock");

        let cli = CliOverrides::new().with_dir(&other);
        let discovered = discover_workspace(None, &cli, &empty_env()).expect("discover explicit");
        assert_eq!(
            discovered.unblock_dir,
            other.join(".unblock").canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ExplicitDir);
    }

    #[test]
    fn explicit_dir_pointing_at_unblock_dir_itself() {
        let root = tempfile::tempdir().expect("tempdir");
        let unblock = root.path().join(".unblock");
        fs::create_dir_all(&unblock).expect("mkdir .unblock");
        let cli = CliOverrides::new().with_dir(&unblock);
        let found = discover_unblock_dir(None, &cli, &empty_env()).expect("discover explicit dir");
        assert_eq!(found, unblock.canonicalize().expect("canon"));
    }

    #[test]
    fn explicit_dir_not_a_workspace_errors() {
        let root = tempfile::tempdir().expect("tempdir");
        let cli = CliOverrides::new().with_dir(root.path());
        let err = discover_unblock_dir(None, &cli, &empty_env()).expect_err("not a workspace");
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
    }

    #[test]
    fn db_under_unblock_derives_the_dir() {
        let root = tempfile::tempdir().expect("tempdir");
        let unblock = root.path().join(".unblock");
        fs::create_dir_all(&unblock).expect("mkdir .unblock");
        let db = unblock.join("unblock.db");
        let cli = CliOverrides::new().with_db(&db);
        let discovered = discover_workspace(None, &cli, &empty_env()).expect("derive from db");
        assert_eq!(
            discovered.unblock_dir,
            unblock.canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ExplicitDb);
    }

    #[test]
    fn derive_dir_from_db_finds_unblock_component() {
        let derived = derive_dir_from_db(Path::new("/ws/.unblock/unblock.db"));
        assert_eq!(derived.as_deref(), Some(Path::new("/ws/.unblock")));
        let none = derive_dir_from_db(Path::new("/ws/elsewhere.db"));
        assert!(none.is_none());
    }

    #[test]
    fn not_found_errors() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("x").join("y");
        fs::create_dir_all(&nested).expect("mkdir nested");
        let err = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &empty_env())
            .expect_err("must not find");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }

    #[test]
    fn optional_suppresses_not_found_without_db() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("x");
        fs::create_dir_all(&nested).expect("mkdir nested");
        let result =
            discover_optional_unblock_dir(Some(&nested), &CliOverrides::default(), &empty_env())
                .expect("optional ok");
        assert!(result.is_none());
    }

    #[test]
    fn symlinked_workspace_dir_is_canonicalized() {
        // A symlink TO the real `.unblock` dir is resolved to the real (canonical) path (Seam C).
        let root = tempfile::tempdir().expect("tempdir");
        let real_ws = root.path().join("real");
        fs::create_dir_all(real_ws.join(".unblock")).expect("mkdir real .unblock");
        let link = root.path().join("link");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_ws, &link).expect("symlink");
            let cli = CliOverrides::new().with_dir(&link);
            let found =
                discover_unblock_dir(None, &cli, &empty_env()).expect("discover via symlink");
            // The canonical path goes through `real`, not `link`.
            assert_eq!(
                found,
                real_ws.join(".unblock").canonicalize().expect("canon")
            );
        }
        #[cfg(not(unix))]
        let _ = link;
    }

    #[test]
    fn a_file_named_unblock_is_not_a_workspace() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::write(root.path().join(".unblock"), b"not a dir").expect("write file");
        let err = discover_unblock_dir(Some(root.path()), &CliOverrides::default(), &empty_env())
            .expect_err("a file .unblock is not a ws");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }

    // -- D39: CLAUDE_PROJECT_DIR (ROOT-probe) tier ------------------------------------------------

    #[test]
    fn claude_project_dir_hit_binds_the_root_child() {
        // CLAUDE_PROJECT_DIR is a project ROOT whose `.unblock` child exists; the cwd `start` is
        // ELSEWHERE (no workspace) — the root probe binds, no walk-up. Source is `ProjectDir`.
        let root = tempfile::tempdir().expect("tempdir");
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".unblock")).expect("mkdir project .unblock");
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("mkdir elsewhere");

        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", project.to_str().expect("utf8"))]);
        let discovered = discover_workspace(Some(&elsewhere), &CliOverrides::default(), &env)
            .expect("project-dir hit");
        assert_eq!(
            discovered.unblock_dir,
            project.join(".unblock").canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ProjectDir);
    }

    #[test]
    fn claude_project_dir_underscore_alias_hit() {
        // The `_unblock` monorepo alias is honored by the ROOT probe, exactly like the walk-up.
        let root = tempfile::tempdir().expect("tempdir");
        let project = root.path().join("mono");
        fs::create_dir_all(project.join("_unblock")).expect("mkdir _unblock");

        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", project.to_str().expect("utf8"))]);
        let discovered =
            discover_workspace(None, &CliOverrides::default(), &env).expect("alias hit");
        assert_eq!(
            discovered.unblock_dir,
            project.join("_unblock").canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ProjectDir);
    }

    #[test]
    fn claude_project_dir_pointing_at_unblock_dir_itself() {
        // Symmetry with `explicit_dir_pointing_at_unblock_dir_itself`: `CLAUDE_PROJECT_DIR` aimed
        // STRAIGHT AT a `.unblock`/`_unblock` dir (not its parent root) self-recognizes and binds it
        // directly, still as `ProjectDir`, no walk-up — the ROOT-probe tier resolves BOTH shapes just
        // as `--dir` does. Before the FIX-3 self-check this MISSED (probe-child-only) and fell
        // through to the walk-up.
        let root = tempfile::tempdir().expect("tempdir");
        let unblock = root.path().join(".unblock");
        fs::create_dir_all(&unblock).expect("mkdir .unblock");

        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", unblock.to_str().expect("utf8"))]);
        let discovered =
            discover_workspace(None, &CliOverrides::default(), &env).expect("self-recognized");
        assert_eq!(
            discovered.unblock_dir,
            unblock.canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ProjectDir);
    }

    #[test]
    fn claude_project_dir_miss_falls_through_to_walk_up() {
        // CLAUDE_PROJECT_DIR points at a root with NO `.unblock` child; the `start` cwd DOES have one
        // → discovery FALLS THROUGH to the walk-up (source `WalkUp`), not a hard error.
        let root = tempfile::tempdir().expect("tempdir");
        let no_ws = root.path().join("no_ws");
        fs::create_dir_all(&no_ws).expect("mkdir no_ws");
        let cwd_ws = root.path().join("cwd_ws");
        fs::create_dir_all(cwd_ws.join(".unblock")).expect("mkdir cwd .unblock");
        let nested = cwd_ws.join("a");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", no_ws.to_str().expect("utf8"))]);
        let discovered = discover_workspace(Some(&nested), &CliOverrides::default(), &env)
            .expect("fall through to walk-up");
        assert_eq!(
            discovered.unblock_dir,
            cwd_ws.join(".unblock").canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::WalkUp);
    }

    #[test]
    fn claude_project_dir_miss_and_no_cwd_ws_is_not_found() {
        // A miss on the root AND no cwd workspace → the guarded walk-up finds nothing.
        let root = tempfile::tempdir().expect("tempdir");
        let no_ws = root.path().join("no_ws");
        fs::create_dir_all(&no_ws).expect("mkdir no_ws");
        let nested = root.path().join("elsewhere").join("deep");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", no_ws.to_str().expect("utf8"))]);
        let err = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &env)
            .expect_err("miss + no cwd ws");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn claude_project_dir_symlink_root_is_canonicalized() {
        // CLAUDE_PROJECT_DIR is a symlink to the real project → the discovered child is canonicalized
        // (Seam C): the bound path goes through `real`, never `link`.
        let root = tempfile::tempdir().expect("tempdir");
        let real = root.path().join("real");
        fs::create_dir_all(real.join(".unblock")).expect("mkdir real .unblock");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", link.to_str().expect("utf8"))]);
        let found = discover_unblock_dir(None, &CliOverrides::default(), &env)
            .expect("symlinked project dir");
        assert_eq!(found, real.join(".unblock").canonicalize().expect("canon"));
    }

    // -- D39: total precedence ---------------------------------------------------------------------

    #[test]
    fn db_override_wins_over_project_dir() {
        let root = tempfile::tempdir().expect("tempdir");
        let db_ws = root.path().join("db_ws");
        fs::create_dir_all(db_ws.join(".unblock")).expect("mkdir db .unblock");
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".unblock")).expect("mkdir project .unblock");

        let cli = CliOverrides::new().with_db(db_ws.join(".unblock").join("unblock.db"));
        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", project.to_str().expect("utf8"))]);
        let discovered = discover_workspace(None, &cli, &env).expect("db wins");
        assert_eq!(
            discovered.unblock_dir,
            db_ws.join(".unblock").canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ExplicitDb);
    }

    #[test]
    fn db_override_wins_over_explicit_dir() {
        // The top of the precedence chain, pinned directly: BOTH `--db` (under one workspace) and
        // `--dir` (a DIFFERENT workspace) are set. `--db` derivation wins, so the bound dir is the
        // db-derived one and the source is `ExplicitDb` — previously only covered transitively (via
        // `db_override_wins_over_project_dir` + `explicit_dir_wins_over_project_dir`), never on the
        // db>dir adjacency itself.
        let root = tempfile::tempdir().expect("tempdir");
        let db_ws = root.path().join("db_ws");
        fs::create_dir_all(db_ws.join(".unblock")).expect("mkdir db .unblock");
        let dir_ws = root.path().join("dir_ws");
        fs::create_dir_all(dir_ws.join(".unblock")).expect("mkdir dir .unblock");

        let cli = CliOverrides::new()
            .with_db(db_ws.join(".unblock").join("unblock.db"))
            .with_dir(&dir_ws);
        let discovered = discover_workspace(None, &cli, &empty_env()).expect("db wins over dir");
        assert_eq!(
            discovered.unblock_dir,
            db_ws.join(".unblock").canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ExplicitDb);
    }

    #[test]
    fn explicit_dir_wins_over_project_dir() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir_ws = root.path().join("dir_ws");
        fs::create_dir_all(dir_ws.join(".unblock")).expect("mkdir dir .unblock");
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".unblock")).expect("mkdir project .unblock");

        let cli = CliOverrides::new().with_dir(&dir_ws);
        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", project.to_str().expect("utf8"))]);
        let discovered = discover_workspace(None, &cli, &env).expect("dir wins");
        assert_eq!(
            discovered.unblock_dir,
            dir_ws.join(".unblock").canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ExplicitDir);
    }

    #[test]
    fn project_dir_wins_over_cwd_walk_up() {
        let root = tempfile::tempdir().expect("tempdir");
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".unblock")).expect("mkdir project .unblock");
        let cwd_ws = root.path().join("cwd_ws");
        fs::create_dir_all(cwd_ws.join(".unblock")).expect("mkdir cwd .unblock");
        let nested = cwd_ws.join("a");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let env = MapEnv::new(&[("CLAUDE_PROJECT_DIR", project.to_str().expect("utf8"))]);
        let discovered = discover_workspace(Some(&nested), &CliOverrides::default(), &env)
            .expect("project dir wins over walk-up");
        assert_eq!(
            discovered.unblock_dir,
            project.join(".unblock").canonicalize().expect("canon")
        );
        assert_eq!(discovered.source, WorkspaceSource::ProjectDir);
    }

    // -- D39: walk-up guard (bounded, INCLUSIVE) ---------------------------------------------------

    #[test]
    fn guard_stops_at_git_repo_root_inclusive() {
        // Repo root `/ws` has `.git`; a stray `/outer/.unblock` sits ABOVE it. Start under `/ws` with
        // no `.unblock` in `/ws` → the walk stops at the `.git` root and never binds `/outer/.unblock`.
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join(".unblock")).expect("mkdir outer .unblock (stray)");
        let ws = root.path().join("ws");
        fs::create_dir_all(ws.join(".git")).expect("mkdir .git dir");
        let nested = ws.join("a").join("b");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let err = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &empty_env())
            .expect_err("must not cross the .git boundary");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }

    #[test]
    fn guard_probes_the_git_repo_root_before_stopping() {
        // The boundary dir is INCLUSIVE: a `.unblock` AT the `.git` repo root is still found.
        let root = tempfile::tempdir().expect("tempdir");
        let ws = root.path().join("ws");
        fs::create_dir_all(ws.join(".git")).expect("mkdir .git dir");
        fs::create_dir_all(ws.join(".unblock")).expect("mkdir ws .unblock");
        let nested = ws.join("a").join("b");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let found = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &empty_env())
            .expect("repo-root .unblock found");
        assert_eq!(found, ws.join(".unblock").canonicalize().expect("canon"));
    }

    #[test]
    fn guard_detects_a_git_file_worktree_pointer() {
        // A `.git` FILE (worktree/submodule gitdir pointer), not a dir, is still a boundary.
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join(".unblock")).expect("mkdir outer .unblock (stray)");
        let ws = root.path().join("ws");
        fs::create_dir_all(&ws).expect("mkdir ws");
        fs::write(ws.join(".git"), b"gitdir: /elsewhere").expect("write .git file");
        let nested = ws.join("a");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let err = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &empty_env())
            .expect_err("a .git FILE is a boundary too");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }

    #[test]
    fn guard_stops_at_home_inclusive() {
        // `$HOME` bounds the walk: a stray `.unblock` ABOVE `$HOME` is not bound.
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join(".unblock"))
            .expect("mkdir above-home .unblock (stray)");
        let home = root.path().join("home");
        fs::create_dir_all(&home).expect("mkdir home");
        let nested = home.join("proj").join("a");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let env = MapEnv::new(&[("HOME", home.to_str().expect("utf8"))]);
        let err = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &env)
            .expect_err("must not cross the $HOME boundary");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }

    #[test]
    fn guard_probes_home_before_stopping() {
        // INCLUSIVE: a deliberate `$HOME/.unblock` is still found (the boundary dir is probed first).
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        fs::create_dir_all(home.join(".unblock")).expect("mkdir $HOME/.unblock");
        let nested = home.join("proj").join("a");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let env = MapEnv::new(&[("HOME", home.to_str().expect("utf8"))]);
        let found = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &env)
            .expect("$HOME/.unblock found");
        assert_eq!(found, home.join(".unblock").canonicalize().expect("canon"));
    }

    #[cfg(windows)]
    #[test]
    fn guard_stops_at_userprofile_inclusive() {
        // On Windows the home dir is `%USERPROFILE%`; a stray `.unblock` above it is not bound.
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join(".unblock")).expect("mkdir above-home .unblock");
        let home = root.path().join("home");
        fs::create_dir_all(&home).expect("mkdir home");
        let nested = home.join("proj");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let env = MapEnv::new(&[("USERPROFILE", home.to_str().expect("utf8"))]);
        let err = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &env)
            .expect_err("must not cross the %USERPROFILE% boundary");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }

    #[test]
    fn guard_with_home_unset_still_stops_at_git() {
        // `$HOME` unset (a sparse GUI env) disables only the home arm — the `.git` root still bounds.
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join(".unblock")).expect("mkdir outer .unblock (stray)");
        let ws = root.path().join("ws");
        fs::create_dir_all(ws.join(".git")).expect("mkdir .git dir");
        let nested = ws.join("a");
        fs::create_dir_all(&nested).expect("mkdir nested");

        // No HOME / USERPROFILE in the env.
        let err = discover_unblock_dir(Some(&nested), &CliOverrides::default(), &empty_env())
            .expect_err("`.git` bounds even with no $HOME");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }
}
