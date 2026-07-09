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

/// Deterministic per-stage fault-injection seam for the EXPORT atomic write (T3.4/NFR-4, D30).
///
/// Behind the non-default `fault-injection` feature ONLY — every guard call-site in
/// [`write_atomic_blocking`] is itself `#[cfg(feature = "fault-injection")]`, so the shipped
/// byte-path is byte-unchanged under default features (this whole module VANISHES). The fault plan is
/// a **process-global `AtomicU8`** (`#![forbid(unsafe_code)]` forbids `static mut`), armed from the
/// external `tests/` crate via [`arm_fault`] and reset via [`clear_faults`]; the seam tests run
/// serially. Import has no atomic fs write path (it applies via one `Storage::create_issues` tx), so
/// import failure-injection rides the DB-transaction rollback instead — never this seam.
#[cfg(feature = "fault-injection")]
mod fault {
    use std::sync::atomic::{AtomicU8, Ordering};

    use crate::error::SyncError;

    /// A deterministic per-stage fault point in the atomic export write (T3.4/NFR-4).
    ///
    /// Write/Flush/SyncAll/Rename all fire BEFORE the atomic rename commits (the target never
    /// changes); [`ParentDirFsync`](Self::ParentDirFsync) fires AFTER the rename (the new content is
    /// already applied, only durability is unconfirmed).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FaultPoint {
        /// Fail the `write_all` of the line content.
        Write,
        /// Fail the `flush` of the temp file.
        Flush,
        /// Fail the `sync_all` (fsync) of the temp file.
        SyncAll,
        /// Fail the atomic `rename` of the temp over the target.
        Rename,
        /// Fail the POST-rename parent-dir fsync (durability-unconfirmed; the new content IS applied).
        ParentDirFsync,
    }

    /// The disarmed sentinel (no fault point armed).
    const DISARMED: u8 = 0;

    /// The process-global armed fault point (`0` = disarmed; `1..=5` = a [`FaultPoint`]). An
    /// `AtomicU8` (never `static mut` — `#![forbid(unsafe_code)]`), so it crosses into the
    /// `spawn_blocking` closure the write runs in.
    static ARMED: AtomicU8 = AtomicU8::new(DISARMED);

    impl FaultPoint {
        /// The `1..=5` `u8` encoding stored in [`ARMED`].
        const fn code(self) -> u8 {
            match self {
                FaultPoint::Write => 1,
                FaultPoint::Flush => 2,
                FaultPoint::SyncAll => 3,
                FaultPoint::Rename => 4,
                FaultPoint::ParentDirFsync => 5,
            }
        }
    }

    /// Arm the process-global fault plan to fail at `point` on the next atomic export write.
    ///
    /// Driven from the external `tests/atomic_failure.rs` seam tests (which run serially and
    /// [`clear_faults`] between cases).
    pub fn arm_fault(point: FaultPoint) {
        ARMED.store(point.code(), Ordering::SeqCst);
    }

    /// Disarm the process-global fault plan (reset between serial seam-test cases).
    pub fn clear_faults() {
        ARMED.store(DISARMED, Ordering::SeqCst);
    }

    /// If `point` is currently armed, build the [`SyncError::Io`] that stage would surface (the
    /// check is inlined at each stage behind `#[cfg(feature = "fault-injection")]`).
    pub(crate) fn injected(
        point: FaultPoint,
        path: &std::path::Path,
        action: &'static str,
    ) -> Option<SyncError> {
        (ARMED.load(Ordering::SeqCst) == point.code()).then(|| SyncError::Io {
            path: path.to_path_buf(),
            action,
            source: std::io::Error::other(format!("fault-injection: {point:?}")),
        })
    }
}

#[cfg(feature = "fault-injection")]
pub use fault::{FaultPoint, arm_fault, clear_faults};

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
        // NFR-13 (D30): the best-effort perms guard routes through the SSOT WARN arm WITHOUT
        // re-leveling — the four-field body carries the io error as the `reason`.
        crate::reliability::reliability_warn!(
            operation = "export",
            path = path.display(),
            result = "perms-not-set",
            reason = e,
        );
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
    // NFR-13 (D30): the non-unix parent-fsync-skip DEBUG routes through the SSOT `reliability_detail!`
    // (four-field body) — do NOT carve it out of the standardization.
    crate::reliability::reliability_detail!(
        operation = "export",
        path = dir.display(),
        result = "parent-fsync-skipped",
        reason = "no portable directory fsync on this target",
    );
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
/// (2)-(3) Allocate a fresh temp file next to `final_path` via a `create_new` collision-retry loop.
///
/// Each candidate is re-validated against the temp allowlist before opening; a taken name retries the
/// next suffix, any other open error surfaces, and exhausting [`MAX_TEMP_ATTEMPTS`] is a
/// [`SyncError::PathTraversal`] (`TempCollision`).
fn allocate_temp_file(
    final_path: &Path,
    confine_root: &Path,
) -> Result<(File, PathBuf), SyncError> {
    for attempt in 0..MAX_TEMP_ATTEMPTS {
        let candidate = temp_path_for(final_path, attempt);
        validate_temp_path(&candidate, final_path, confine_root)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((file, candidate)),
            // Name taken — try the next attempt suffix on the next loop iteration.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(SyncError::Io {
                    path: candidate,
                    action: "creating temp file",
                    source,
                });
            }
        }
    }
    Err(SyncError::PathTraversal {
        path: temp_path_for(final_path, 0),
        reason: crate::path::PathReject::TempCollision,
    })
}

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
    let (mut file, temp_path) = allocate_temp_file(final_path, confine_root)?;

    // (4) RAII: remove the temp on any early return below.
    let guard = TempFileGuard::new(temp_path.clone());

    // (5) restrictive perms BEFORE writing content.
    set_restrictive_perms(&temp_path);

    // (6) write all lines with a trailing newline each.
    #[cfg(feature = "fault-injection")]
    if let Some(err) = fault::injected(fault::FaultPoint::Write, &temp_path, "writing to temp file")
    {
        return Err(err);
    }
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
    #[cfg(feature = "fault-injection")]
    if let Some(err) = fault::injected(fault::FaultPoint::Flush, &temp_path, "flushing temp file") {
        return Err(err);
    }
    file.flush().map_err(|source| SyncError::Io {
        path: temp_path.clone(),
        action: "flushing temp file",
        source,
    })?;
    #[cfg(feature = "fault-injection")]
    if let Some(err) = fault::injected(fault::FaultPoint::SyncAll, &temp_path, "fsyncing temp file")
    {
        return Err(err);
    }
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
    #[cfg(feature = "fault-injection")]
    if let Some(err) = fault::injected(
        fault::FaultPoint::Rename,
        final_path,
        "renaming temp file over",
    ) {
        return Err(err);
    }
    std::fs::rename(&temp_path, final_path).map_err(|source| SyncError::Io {
        path: final_path.to_path_buf(),
        action: "renaming temp file over",
        source,
    })?;

    // (10) DRIFT-1: fsync the parent dir so the rename's dir-entry update is crash-durable.
    // The ParentDirFsync fault fires HERE — POST-rename: the new content is already applied to the
    // target (the temp became the target), only durability is unconfirmed.
    #[cfg(feature = "fault-injection")]
    if let Some(err) = fault::injected(
        fault::FaultPoint::ParentDirFsync,
        parent,
        "fsyncing parent dir of",
    ) {
        return Err(err);
    }
    sync_directory(parent)?;

    // (11) the temp is now the target — do NOT remove it.
    guard.persist();
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{temp_path_for, write_atomic};
    use crate::error::SyncError;

    /// Count the `*.tmp` files directly under `dir` (orphan-temp detector).
    fn tmp_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .expect("read_dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count()
    }

    #[tokio::test]
    async fn post_temp_failure_removes_orphan_temp_via_raii_guard() {
        // SF-3 (NON-VACUOUS): a failure that fires AFTER the temp file has been created must still
        // leave ZERO orphan `*.tmp` files — the `TempFileGuard::drop` RAII cleanup fires on the early
        // `Err` return. The three `atomic_failure.rs` integration tests all reject at PREFLIGHT (an
        // external target — before any temp exists), so they cannot exercise the guard. Here the
        // target NAME resolves to a valid confined `issues.jsonl`, so the temp IS created and written;
        // the failure is injected at the PRE-RENAME re-validation, which sees the target path is a
        // DIRECTORY (not a regular file) and returns `PathTraversal(NonRegularFile)` — strictly after
        // the temp exists and the guard is armed.
        //
        // Neutering `TempFileGuard::drop` (making it a no-op) leaves the orphan `*.tmp` behind, so the
        // `tmp_count == 0` assertion then FAILS (proven by mutation).
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("issues.jsonl");
        // Make the target a NON-EMPTY DIRECTORY: the temp is created first, then the pre-rename
        // `validate_sync_path(final_path)` rejects the directory AFTER the temp already exists.
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("occupant"), b"x").unwrap();

        let err = write_atomic(&target, dir.path(), std::iter::once("line".to_string()))
            .await
            .expect_err("a directory target must fail the write");
        assert!(
            matches!(err, SyncError::PathTraversal { .. }),
            "expected a post-temp PathTraversal(NonRegularFile), got {err:?}"
        );

        // RAII: despite the temp being created + written before the failure, no orphan `*.tmp` remains.
        assert_eq!(
            tmp_count(dir.path()),
            0,
            "the temp guard must remove the orphan temp on a post-temp failure"
        );
        // The pre-existing directory (the "original") is untouched.
        assert!(target.is_dir(), "the target directory is left intact");
        assert!(target.join("occupant").exists(), "its contents survive");
    }

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
