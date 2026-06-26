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
use crate::error::ConfigError;

/// Whether `name` is a workspace-dir name: `.unblock` OR `_unblock` (FORK-2/D8 monorepo alias).
fn is_unblock_dir_name(name: &str) -> bool {
    name == ".unblock" || name == "_unblock"
}

/// Discover the active `unblock_dir` (the `.unblock`/`_unblock` directory itself).
///
/// `cli.dir`/`UNBLOCK_DIR` (when set) is the EXPLICIT override — used directly with NO walk-up
/// (MF-2). Otherwise walks up ancestors from `start` (or the CWD). An explicit `cli.db` under a
/// workspace dir derives the dir. The discovered dir is **canonicalized** (Seam C).
///
/// # Errors
///
/// Returns [`ConfigError::WorkspaceNotFound`] when no `.unblock`/`_unblock` is found, or
/// [`ConfigError::InvalidValue`] when an explicit `--dir`/`--db` does not point at a workspace.
pub fn discover_unblock_dir(
    start: Option<&Path>,
    cli: &CliOverrides,
) -> Result<PathBuf, ConfigError> {
    // 1. An explicit --db under a workspace dir derives the dir (highest priority — original parity).
    if let Some(db) = cli.db.as_deref()
        && let Some(dir) = derive_dir_from_db(db)
    {
        return Ok(canonicalize_existing(&dir));
    }

    // 2. An explicit --dir/UNBLOCK_DIR is used directly, NO walk-up (MF-2).
    if let Some(dir) = cli.dir.as_deref() {
        return resolve_explicit_dir(dir);
    }

    // 3. Otherwise walk up from `start` (or CWD) for the nearest `.unblock`/`_unblock` dir.
    let dir = walk_up_for_unblock_dir(start)?;
    Ok(canonicalize_existing(&dir))
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
) -> Result<Option<PathBuf>, ConfigError> {
    match discover_unblock_dir(start, cli) {
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
    // The explicit path is itself the unblock dir.
    if dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_unblock_dir_name)
        && dir.is_dir()
    {
        return Ok(canonicalize_existing(dir));
    }
    // Or the explicit path is a workspace root holding `.unblock`/`_unblock`.
    for name in [".unblock", "_unblock"] {
        let candidate = dir.join(name);
        if candidate.is_dir() {
            return Ok(canonicalize_existing(&candidate));
        }
    }
    Err(ConfigError::InvalidValue {
        key: "--dir".to_string(),
        value: dir.display().to_string(),
        reason: "not a workspace (no .unblock/ or _unblock/ directory)".to_string(),
    })
}

/// Walk up the ancestors of `start` (or CWD) for the nearest `.unblock`/`_unblock` directory.
fn walk_up_for_unblock_dir(start: Option<&Path>) -> Result<PathBuf, ConfigError> {
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
        for name in [".unblock", "_unblock"] {
            let candidate = dir.join(name);
            if candidate.is_dir() {
                return Ok(candidate);
            }
        }
        current = dir.parent();
    }

    Err(ConfigError::WorkspaceNotFound {
        start: start.map_or_else(|| begin.clone(), Path::to_path_buf),
    })
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
    use super::{derive_dir_from_db, discover_optional_unblock_dir, discover_unblock_dir};
    use crate::cli::CliOverrides;
    use crate::error::ConfigError;
    use std::fs;
    use std::path::Path;

    #[test]
    fn walks_up_to_nearest_dot_unblock() {
        let root = tempfile::tempdir().expect("tempdir");
        let ws = root.path().join("project");
        fs::create_dir_all(ws.join(".unblock")).expect("mkdir .unblock");
        let nested = ws.join("a").join("b");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let found =
            discover_unblock_dir(Some(&nested), &CliOverrides::default()).expect("discover");
        assert_eq!(found, ws.join(".unblock").canonicalize().expect("canon"));
    }

    #[test]
    fn walks_up_to_underscore_alias() {
        let root = tempfile::tempdir().expect("tempdir");
        let ws = root.path().join("project");
        fs::create_dir_all(ws.join("_unblock")).expect("mkdir _unblock");
        let nested = ws.join("x");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let found =
            discover_unblock_dir(Some(&nested), &CliOverrides::default()).expect("discover");
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
        let found = discover_unblock_dir(None, &cli).expect("discover explicit");
        assert_eq!(found, other.join(".unblock").canonicalize().expect("canon"));
    }

    #[test]
    fn explicit_dir_pointing_at_unblock_dir_itself() {
        let root = tempfile::tempdir().expect("tempdir");
        let unblock = root.path().join(".unblock");
        fs::create_dir_all(&unblock).expect("mkdir .unblock");
        let cli = CliOverrides::new().with_dir(&unblock);
        let found = discover_unblock_dir(None, &cli).expect("discover explicit dir");
        assert_eq!(found, unblock.canonicalize().expect("canon"));
    }

    #[test]
    fn explicit_dir_not_a_workspace_errors() {
        let root = tempfile::tempdir().expect("tempdir");
        let cli = CliOverrides::new().with_dir(root.path());
        let err = discover_unblock_dir(None, &cli).expect_err("not a workspace");
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
    }

    #[test]
    fn db_under_unblock_derives_the_dir() {
        let root = tempfile::tempdir().expect("tempdir");
        let unblock = root.path().join(".unblock");
        fs::create_dir_all(&unblock).expect("mkdir .unblock");
        let db = unblock.join("unblock.db");
        let cli = CliOverrides::new().with_db(&db);
        let found = discover_unblock_dir(None, &cli).expect("derive from db");
        assert_eq!(found, unblock.canonicalize().expect("canon"));
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
        let err = discover_unblock_dir(Some(&nested), &CliOverrides::default())
            .expect_err("must not find");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }

    #[test]
    fn optional_suppresses_not_found_without_db() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("x");
        fs::create_dir_all(&nested).expect("mkdir nested");
        let result = discover_optional_unblock_dir(Some(&nested), &CliOverrides::default())
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
            let found = discover_unblock_dir(None, &cli).expect("discover via symlink");
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
        let err = discover_unblock_dir(Some(root.path()), &CliOverrides::default())
            .expect_err("a file .unblock is not a ws");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }
}
