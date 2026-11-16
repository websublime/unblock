//! The `.recovery/` evidence tree beside the database file (PRD §4 D58 clause (9), spine §3.3).
//!
//! [`confine`] admits a path only when it lies under `<db dir>/.recovery/`, judged by `lstat` and
//! identity rather than by resolving both sides. `.recovery` must be a real directory, and every
//! directory from the target's parent up to it must be one too, so a symlink anywhere on that
//! chain refuses and nothing is written through it. The exclusive window confines its run
//! directory with it, and `repair` its snapshot.
//!
//! [`create_private_dir`] and [`restrict_file`] give the evidence its unix modes: 0700 for the
//! directories, 0600 for the files.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};

use crate::error::StorageError;

/// The evidence directory beside the database file (D58 clause (9)).
pub(super) const RECOVERY_DIR: &str = ".recovery";

/// The identity [`confine`] compares: device and inode on unix. Windows uses the canonical path,
/// because Rust 1.96 still marks its file index unstable.
#[cfg(unix)]
type DirId = (u64, u64);
#[cfg(windows)]
type DirId = PathBuf;

/// The recovery root of the database at `db_path`: `<db dir>/.recovery`.
pub(super) fn recovery_root(db_path: &Path) -> PathBuf {
    db_dir(db_path).join(RECOVERY_DIR)
}

/// The directory holding the database file (`.` for a bare file name).
fn db_dir(db_path: &Path) -> &Path {
    db_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Require `target` to lie under `<db dir>/.recovery/`.
///
/// `.recovery` must be a real directory per `lstat`. The walk starts at `target`'s parent and
/// goes up one component at a time, each a real directory per `lstat`, until it reaches the entry
/// with `.recovery`'s identity. A `..` component, a symlink or other non-directory on the way, or
/// a walk that never reaches `.recovery` refuses. `target` itself is the caller's to check. On
/// Windows the identity is the canonical path and the symlink test is
/// `symlink_metadata().file_type().is_symlink()`, which stays unmeasured.
///
/// # Errors
///
/// - [`StorageError::RecoveryPathOutsideWorkspace`] for a refused `target`.
/// - [`StorageError::RecoveryIo`] when an `lstat` fails for a reason other than an absent path.
pub(super) fn confine(db_path: &Path, target: &Path) -> Result<(), StorageError> {
    let refuse = || StorageError::RecoveryPathOutsideWorkspace {
        path: target.to_path_buf(),
    };
    if target
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(refuse());
    }
    let Some(root) = real_dir(&recovery_root(db_path))? else {
        return Err(refuse());
    };
    let absolute = std::path::absolute(target).map_err(|source| recovery_io(target, source))?;
    let Some(parent) = absolute.parent() else {
        return Err(refuse());
    };
    for dir in parent.ancestors() {
        match real_dir(dir)? {
            Some(id) if id == root => return Ok(()),
            Some(_) => {}
            None => return Err(refuse()),
        }
    }
    Err(refuse())
}

/// Whether `path` is a real directory (not a symlink) per `lstat`, with its identity. An absent
/// path is not one.
///
/// # Errors
///
/// [`StorageError::RecoveryIo`] when the `lstat` fails for a reason other than an absent path.
pub(super) fn real_dir(path: &Path) -> Result<Option<DirId>, StorageError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(recovery_io(path, source)),
    };
    let kind = meta.file_type();
    if kind.is_symlink() || !kind.is_dir() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(Some((meta.dev(), meta.ino())))
    }
    #[cfg(windows)]
    {
        fs::canonicalize(path)
            .map(Some)
            .map_err(|source| recovery_io(path, source))
    }
}

/// Create the directory `dir`, mode 0700 on unix. Its parent must exist, and an existing `dir` is
/// an `AlreadyExists` error.
///
/// # Errors
///
/// The `mkdir` or `chmod` error.
pub(super) fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    set_mode(dir, 0o700)
}

/// Restrict the evidence file at `path` to mode 0600 on unix.
///
/// # Errors
///
/// The `chmod` error.
pub(super) fn restrict_file(path: &Path) -> io::Result<()> {
    set_mode(path, 0o600)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Windows has no unix modes; its ACLs stay as inherited.
#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// A [`StorageError::RecoveryIo`] for `path`.
fn recovery_io(path: &Path, source: io::Error) -> StorageError {
    StorageError::RecoveryIo {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use super::confine;
    use crate::StorageError;

    fn refused(db: &Path, target: &Path) -> bool {
        matches!(
            confine(db, target),
            Err(StorageError::RecoveryPathOutsideWorkspace { .. })
        )
    }

    /// Only a path under a real `<db dir>/.recovery/`, reached through real directories, passes.
    /// `..`, the root's own parent, a sibling sharing the prefix, a symlinked root and a symlinked
    /// run directory all refuse.
    #[test]
    fn confinement_is_by_lstat_and_identity() {
        let ws = tempfile::tempdir().expect("tempdir");
        let db = ws.path().join("unblock.db");
        let run = ws.path().join(".recovery").join("run");
        fs::create_dir_all(&run).expect("run dir");
        fs::create_dir(ws.path().join(".recovery-x")).expect("sibling");

        assert!(!refused(&db, &run));
        assert!(!refused(&db, &run.join("snapshot.db")));
        assert!(!refused(
            &db,
            &ws.path().join(".recovery/./run/snapshot.db")
        ));
        assert!(refused(&db, &ws.path().join(".recovery")));
        assert!(refused(
            &db,
            &ws.path().join(".recovery/run/../../unblock.db")
        ));
        assert!(refused(&db, &ws.path().join(".recovery-x/snapshot.db")));
        assert!(refused(&db, &ws.path().join("snapshot.db")));
        assert!(refused(
            &db,
            &ws.path().join(".recovery/absent/snapshot.db")
        ));

        let outside = tempfile::tempdir().expect("outside");
        fs::create_dir(outside.path().join("run")).expect("outside run");
        symlink(
            outside.path().join("run"),
            ws.path().join(".recovery/linked"),
        )
        .expect("link");
        assert!(refused(
            &db,
            &ws.path().join(".recovery/linked/snapshot.db")
        ));

        let linked_ws = tempfile::tempdir().expect("linked workspace");
        symlink(outside.path(), linked_ws.path().join(".recovery")).expect("link root");
        let linked_db = linked_ws.path().join("unblock.db");
        assert!(refused(&linked_db, &linked_ws.path().join(".recovery/run")));
        assert!(refused(
            &linked_db,
            &linked_ws.path().join(".recovery/run/snapshot.db")
        ));
    }
}
