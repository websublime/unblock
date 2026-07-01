//! Atomic, durable JSONL write (FR-7/NFR-4/5, DRIFT-1).
//!
//! Sequence: write a pid-scoped temp in the SAME dir → `flush` → `sync_all` (fsync the FILE) →
//! atomic `rename` over the target → **fsync the PARENT DIR** (DRIFT-1, faithful `util::
//! durable_rename` — without it the rename is not crash-durable). On any error the temp is removed
//! (RAII guard) and the original is left untouched. All blocking fs runs in ONE `spawn_blocking`;
//! the paths are cloned to owned `PathBuf` BEFORE the closure (MF-11(b) — `&Path` is not `'static`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::SyncError;
use crate::path::{validate_sync_path, validate_temp_path};

/// The max number of temp-name collision-retry attempts before giving up.
const MAX_TEMP_ATTEMPTS: u32 = 64;

/// A temp file that removes itself on drop unless [`TempFileGuard::persist`] is called.
struct TempFileGuard {
    path: PathBuf,
    persisted: bool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            persisted: false,
        }
    }
    fn persist(mut self) {
        self.persisted = true;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if !self.persisted {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Build the temp filename for `final_path` at `attempt`: `<final_name>.<pid_variant>.tmp`.
///
/// SH-4: built from `file_name()` + push (NOT `with_extension`, which would replace the extension).
/// Attempt 0 uses the raw pid; later attempts derive a digits-only variant so the name still matches
/// the `*.jsonl.<digits>.tmp` allowlist.
fn temp_path_for(final_path: &Path, attempt: u32) -> PathBuf {
    let pid = std::process::id();
    let variant = if attempt == 0 {
        pid
    } else {
        pid.saturating_mul(100).saturating_add(attempt)
    };
    let mut name = final_path
        .file_name()
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    name.push(format!(".{variant}.tmp"));
    match final_path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Set restrictive (0600) permissions on `path` (unix); a warn-not-fail no-op elsewhere.
#[cfg(unix)]
fn set_restrictive_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(target: "unblock.reliability", path = %path.display(), error = %e, "could not set 0600 perms on temp file");
    }
}

#[cfg(not(unix))]
fn set_restrictive_perms(_path: &Path) {}

/// Fsync a directory so a rename's directory-entry update is crash-durable (DRIFT-1).
///
/// Unix opens the dir and `sync_all`s it; other targets have no portable directory-fsync API, so
/// this traces + skips (preserving the atomic rename already performed).
#[cfg(unix)]
fn sync_directory(dir: &Path) -> Result<(), SyncError> {
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|source| SyncError::Io {
            path: dir.to_path_buf(),
            action: "fsyncing parent dir of",
            source,
        })
}

#[cfg(not(unix))]
fn sync_directory(dir: &Path) -> Result<(), SyncError> {
    tracing::debug!(target: "unblock.reliability", path = %dir.display(), "skipping parent-dir fsync: no portable directory fsync on this target");
    Ok(())
}

/// Atomically write `lines` to `final_path` (temp → fsync → rename → parent-dir fsync), returning the
/// number of lines written. Each line gets a trailing `\n`.
///
/// # Errors
///
/// [`SyncError::Io`] on any fs step; [`SyncError::PathTraversal`] if a temp/final path fails
/// validation; [`SyncError::PathTraversal`] (`TempCollision`) if temp allocation cannot find a free
/// name after [`MAX_TEMP_ATTEMPTS`] tries.
pub async fn write_atomic(
    final_path: &Path,
    confine_root: &Path,
    lines: impl Iterator<Item = String> + Send + 'static,
) -> Result<usize, SyncError> {
    // MF-11(b): own the paths BEFORE the 'static spawn_blocking closure (&Path is not 'static).
    let final_path = final_path.to_path_buf();
    let confine_root = confine_root.to_path_buf();

    let handle = tokio::task::spawn_blocking(move || {
        write_atomic_blocking(&final_path, &confine_root, lines)
    });
    handle.await.map_err(|join_err| SyncError::Io {
        path: PathBuf::new(),
        action: "joining the blocking write task for",
        source: std::io::Error::other(join_err),
    })?
}

/// The blocking body of [`write_atomic`] (runs inside `spawn_blocking`).
fn write_atomic_blocking(
    final_path: &Path,
    confine_root: &Path,
    lines: impl Iterator<Item = String>,
) -> Result<usize, SyncError> {
    let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| SyncError::Io {
        path: parent.to_path_buf(),
        action: "creating parent dir of",
        source,
    })?;

    // (2)-(3) allocate a temp file with `create_new` collision-retry.
    let mut temp_file: Option<(File, PathBuf)> = None;
    for attempt in 0..MAX_TEMP_ATTEMPTS {
        let candidate = temp_path_for(final_path, attempt);
        validate_temp_path(&candidate, final_path, confine_root)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temp_file = Some((file, candidate));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Name taken — try the next attempt suffix on the next loop iteration.
            }
            Err(source) => {
                return Err(SyncError::Io {
                    path: candidate,
                    action: "creating temp file",
                    source,
                });
            }
        }
    }
    let Some((mut file, temp_path)) = temp_file else {
        return Err(SyncError::PathTraversal {
            path: temp_path_for(final_path, 0),
            reason: crate::path::PathReject::TempCollision,
        });
    };

    // (4) RAII: remove the temp on any early return below.
    let guard = TempFileGuard::new(temp_path.clone());

    // (5) restrictive perms BEFORE writing content.
    set_restrictive_perms(&temp_path);

    // (6) write all lines with a trailing newline each.
    let mut count = 0usize;
    for line in lines {
        file.write_all(line.as_bytes())
            .map_err(|source| SyncError::Io {
                path: temp_path.clone(),
                action: "writing to temp file",
                source,
            })?;
        file.write_all(b"\n").map_err(|source| SyncError::Io {
            path: temp_path.clone(),
            action: "writing to temp file",
            source,
        })?;
        count += 1;
    }

    // (7) flush + fsync the FILE.
    file.flush().map_err(|source| SyncError::Io {
        path: temp_path.clone(),
        action: "flushing temp file",
        source,
    })?;
    file.sync_all().map_err(|source| SyncError::Io {
        path: temp_path.clone(),
        action: "fsyncing temp file",
        source,
    })?;
    drop(file);

    // (8) re-validate both paths just before the rename.
    validate_temp_path(&temp_path, final_path, confine_root)?;
    validate_sync_path(final_path, confine_root, false)?;

    // (9) atomic rename over the target.
    std::fs::rename(&temp_path, final_path).map_err(|source| SyncError::Io {
        path: final_path.to_path_buf(),
        action: "renaming temp file over",
        source,
    })?;

    // (10) DRIFT-1: fsync the parent dir so the rename's dir-entry update is crash-durable.
    sync_directory(parent)?;

    // (11) the temp is now the target — do NOT remove it.
    guard.persist();
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{temp_path_for, write_atomic};
    use crate::error::SyncError;

    #[tokio::test]
    async fn writes_target_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("issues.jsonl");
        let lines = vec!["a".to_string(), "b".to_string()];
        let n = write_atomic(&target, dir.path(), lines.into_iter())
            .await
            .expect("write");
        assert_eq!(n, 2);
        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "a\nb\n");
    }

    #[test]
    fn temp_name_byte_form_is_file_name_plus_pid_tmp() {
        // SH-4: `issues.jsonl.<pid>.tmp`, built via file_name()+push (NOT with_extension).
        let final_path = std::path::Path::new("/ws/.unblock/issues.jsonl");
        let temp = temp_path_for(final_path, 0);
        let name = temp.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("issues.jsonl."), "name: {name}");
        assert!(name.ends_with(".tmp"), "name: {name}");
        // The digits between are the pid.
        let mid = name
            .strip_prefix("issues.jsonl.")
            .and_then(|s| s.strip_suffix(".tmp"))
            .unwrap();
        assert!(
            mid.chars().all(|c| c.is_ascii_digit()),
            "pid segment: {mid}"
        );
        assert_eq!(temp.parent(), final_path.parent());
    }

    #[tokio::test]
    async fn rejects_disallowed_target_extension() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("issues.txt");
        let err = write_atomic(&bad, dir.path(), std::iter::empty())
            .await
            .expect_err("bad ext");
        assert!(matches!(err, SyncError::PathTraversal { .. }));
        assert!(!bad.exists(), "no target written for a rejected path");
    }

    #[tokio::test]
    async fn overwrites_existing_target_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("issues.jsonl");
        std::fs::write(&target, "old\n").unwrap();
        write_atomic(&target, dir.path(), std::iter::once("new".to_string()))
            .await
            .expect("overwrite");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new\n");
    }
}
