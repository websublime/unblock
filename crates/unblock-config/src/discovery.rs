//! `.unblock/` workspace discovery — walk up the ancestors of `start` looking for an existing
//! `.unblock/` directory (single-workspace only, D11 — no town/mayor routing).
//!
//! On success, returns the **`workspace_dir`** (the directory that CONTAINS `.unblock/`), distinct
//! from `paths.unblock_dir` (= `workspace_dir/.unblock`). Both are intentional (spine §4). Workspace
//! **creation** belongs to `init` (T3.1); the T1.3a facades require an existing `.unblock/`.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;
use crate::paths::UNBLOCK_DIR_NAME;

/// Walk up from `start` looking for the nearest ancestor that contains a `.unblock/` directory.
///
/// `start` may be a directory or a file path: a file's parent is the first ancestor inspected. The
/// path is normalized to an absolute form (joined onto the current directory when relative) so the
/// walk terminates at the filesystem root. The returned path is the **project root**
/// (`workspace_dir`) — the directory that contains `.unblock/`.
///
/// # Errors
///
/// Returns [`ConfigError::WorkspaceNotFound`] when no ancestor (up to the filesystem root) contains
/// a `.unblock/` directory.
pub(crate) fn discover_workspace_dir(start: &Path) -> Result<PathBuf, ConfigError> {
    let absolute = absolutize(start);

    // A file path has its containing directory inspected first; a directory inspects itself first.
    // `is_dir()` follows symlinks and is false for a non-existent path, so a non-existent `start`
    // (e.g. a path that has not been created yet) is treated as a file-like leaf and its parent
    // chain is walked — which is the desired behaviour for "find the workspace above this path".
    let mut current: Option<&Path> = if absolute.is_dir() {
        Some(absolute.as_path())
    } else {
        absolute.parent()
    };

    while let Some(dir) = current {
        if is_workspace_dir(dir) {
            return Ok(dir.to_path_buf());
        }
        current = dir.parent();
    }

    Err(ConfigError::WorkspaceNotFound {
        start: start.to_path_buf(),
    })
}

/// Whether `dir` is a workspace root (contains a `.unblock/` **directory**).
fn is_workspace_dir(dir: &Path) -> bool {
    dir.join(UNBLOCK_DIR_NAME).is_dir()
}

/// Turn `path` into an absolute path without resolving symlinks or requiring it to exist.
///
/// Relative paths are joined onto the current working directory; if the current directory cannot be
/// read, the relative path is returned unchanged (the walk then terminates at its own root, which
/// still yields a faithful `WorkspaceNotFound`). This avoids `canonicalize`, which would fail on a
/// non-existent `start`.
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
    use super::discover_workspace_dir;
    use crate::error::ConfigError;
    use std::fs;

    #[test]
    fn finds_unblock_dir_from_nested_start() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("project");
        fs::create_dir_all(workspace.join(".unblock")).expect("mkdir .unblock");
        let nested = workspace.join("a").join("b").join("c");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let found = discover_workspace_dir(&nested).expect("discovery");
        // Canonicalize both sides so a symlinked tempdir (e.g. macOS `/var` -> `/private/var`)
        // does not spuriously fail the equality.
        assert_eq!(
            found.canonicalize().expect("canon found"),
            workspace.canonicalize().expect("canon workspace")
        );
    }

    #[test]
    fn finds_workspace_dir_itself_when_start_is_the_root() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(workspace.path().join(".unblock")).expect("mkdir .unblock");

        let found = discover_workspace_dir(workspace.path()).expect("discovery");
        assert_eq!(
            found.canonicalize().expect("canon found"),
            workspace.path().canonicalize().expect("canon workspace")
        );
    }

    #[test]
    fn not_found_when_no_unblock_dir_exists() {
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("x").join("y");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let err = discover_workspace_dir(&nested).expect_err("must not find a workspace");
        match err {
            ConfigError::WorkspaceNotFound { start } => assert_eq!(start, nested),
            other => panic!("expected WorkspaceNotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_plain_file_is_not_mistaken_for_a_workspace_dir() {
        // A regular file named `.unblock` must NOT count as a workspace (only a directory does).
        let root = tempfile::tempdir().expect("tempdir");
        fs::write(root.path().join(".unblock"), b"not a dir").expect("write file");

        let err = discover_workspace_dir(root.path()).expect_err("a file .unblock is not a ws");
        assert!(matches!(err, ConfigError::WorkspaceNotFound { .. }));
    }
}
