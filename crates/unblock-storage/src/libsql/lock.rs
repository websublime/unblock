//! The cross-process advisory **write lock** (D31 — a D14 amendment) — the restored beads
//! `.write.lock` serializer, re-homed as an L2 [`crate::Storage`] primitive.
//!
//! Under the supported child-per-client stdio topology (PRD §8.2) multiple `unblock serve`
//! processes share one `unblock.db`; the in-process [`tokio::sync::Semaphore`] (L5) and the
//! per-store write-connection [`tokio::sync::Mutex`] (L2) serialize writers only *within* one
//! process. This primitive restores **cross-process** write serialization: every mutation acquires
//! an OS advisory lock on `.unblock/.write.lock` (a NEW file, a sibling of `unblock.db`, **distinct**
//! from the vestigial `.unblock.lock` `OrphanedLockFile` detector target) for the WHOLE mutation —
//! the same span the engine holds the write permit.
//!
//! # Mechanism (faithful port of beads `sync/mod.rs::blocking_write_lock_with_timeout`)
//!
//! - **Primitive:** [`std::fs::File::try_lock`] (stable since 1.89; available on the pinned 1.96 with
//!   **no external crate**, so `#![forbid(unsafe_code)]` holds — std does the platform FFI). A single
//!   **EXCLUSIVE** lock; reads take no lock (WAL MVCC).
//! - **Lock file:** opened `read(true).write(true).create(true).truncate(false)` — **no content is
//!   ever written**; it is a pure flock target. A crashed holder's advisory lock is released by the
//!   kernel when its fd closes, so the file **never orphans** (which is exactly why the `OrphanedLock`
//!   detector watches a *different*, presence-based file).
//! - **Non-spinning (NFR-3):** a fast-path non-blocking `try_lock()`, then — on contention — an async
//!   [`tokio::time::sleep`] poll at [`POLL_INTERVAL`] (25 ms) to a bounded timeout. It **never** calls
//!   the blocking `File::lock()` on a worker thread and **never** busy-spins.
//! - **Timeout:** the configured `lock_timeout_ms` (default
//!   [`DEFAULT_WRITE_LOCK_TIMEOUT_MS`](crate::DEFAULT_WRITE_LOCK_TIMEOUT_MS), threaded down from
//!   `unblock-config`); a `Duration::ZERO` timeout degenerates to a single try (fail-fast, used
//!   by the schema-advancing `migrate`). A timeout maps to the retryable
//!   [`StorageError::DatabaseLocked`] (no new `ErrorCode`).
//! - **RAII:** [`WriteLockGuard`] owns the locked [`File`] and releases the lock on `Drop` (an
//!   explicit `unlock()` backstop; closing the fd also releases it).

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{BackendOpaque, StorageError};

/// The restored cross-process advisory write-lock file name (a sibling of `unblock.db`).
///
/// **Distinct** from the vestigial `.unblock.lock` `OrphanedLockFile` detector target: this file is
/// the live flock serializer, never written to, kernel-released on crash.
pub(crate) const WRITE_LOCK_FILE_NAME: &str = ".write.lock";

/// The poll interval between `try_lock` retries while contended (faithful to beads'
/// `WRITE_LOCK_POLL_INTERVAL`). Sleep-based (`tokio::time::sleep`), **never** a busy-spin (NFR-3).
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The cross-process advisory write lock for one file-backed store.
///
/// Holds the lock-file path and the configured per-mutation timeout. Each acquire opens a **fresh**
/// file descriptor and `try_lock`s it (matching beads' fresh-open-per-acquire discipline), so the
/// returned [`WriteLockGuard`] is owned/`'static` and can be held by the engine (L5) across the whole
/// mutation and across the `Arc<dyn Storage>` boundary.
#[derive(Debug, Clone)]
pub(crate) struct WriteLock {
    /// `<db.parent()>/.write.lock` — derived in L2 from the db-file parent (no `unblock-config` dep).
    lock_path: PathBuf,
    /// The per-mutation acquire timeout (from `write_lock_timeout_ms`).
    timeout: Duration,
}

impl WriteLock {
    /// Build the lock for a file-backed store: `<db_parent>/.write.lock` with the given timeout.
    ///
    /// The path is derived **inside L2** from the db-file parent directory storage already holds —
    /// no dependency on `unblock-config` (L4), so no back-edge (NFR-15). The in-memory store never
    /// constructs a `WriteLock` (there is no workspace dir and no cross-process sharing).
    pub(crate) fn new(db_parent: &Path, lock_timeout_ms: u64) -> Self {
        Self {
            lock_path: db_parent.join(WRITE_LOCK_FILE_NAME),
            timeout: Duration::from_millis(lock_timeout_ms),
        }
    }

    /// Acquire the exclusive lock for one mutation, using the store's configured timeout.
    ///
    /// # Errors
    ///
    /// - [`StorageError::DatabaseLocked`] (retryable) if the lock is not acquired within the timeout.
    /// - [`StorageError::Backend`] if the lock file cannot be opened or the OS lock call fails.
    pub(crate) async fn acquire(&self) -> Result<WriteLockGuard, StorageError> {
        self.acquire_with_timeout(self.timeout).await
    }

    /// Acquire the exclusive lock with an explicit timeout.
    ///
    /// A `Duration::ZERO` timeout is a single non-blocking try (fail-fast) — used by the
    /// schema-advancing `migrate`, which bypasses the per-mutation chokepoint and must refuse rather
    /// than queue behind a concurrent writer.
    ///
    /// # Errors
    ///
    /// As [`acquire`](Self::acquire).
    pub(crate) async fn acquire_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<WriteLockGuard, StorageError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)
            .map_err(|err| open_error(&self.lock_path, &err))?;

        // Fast path: one non-blocking try for the uncontended common case.
        match file.try_lock() {
            Ok(()) => return Ok(WriteLockGuard { file }),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(err)) => return Err(lock_error(&self.lock_path, &err)),
        }

        // Contended. A zero timeout is fail-fast (single try) — never park.
        if timeout.is_zero() {
            return Err(StorageError::DatabaseLocked);
        }

        // Bounded, sleeping poll — cooperative (yields the worker), never a busy-spin (NFR-3).
        let start = Instant::now();
        loop {
            if start.elapsed() >= timeout {
                return Err(StorageError::DatabaseLocked);
            }
            let remaining = timeout.saturating_sub(start.elapsed());
            tokio::time::sleep(remaining.min(POLL_INTERVAL)).await;
            match file.try_lock() {
                Ok(()) => return Ok(WriteLockGuard { file }),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(err)) => return Err(lock_error(&self.lock_path, &err)),
            }
        }
    }
}

/// An RAII guard that holds the exclusive `.write.lock`. Dropping it releases the OS advisory lock.
///
/// A **public opaque** guard (returned across the `Storage` trait boundary as
/// `Option<WriteLockGuard>`): it owns the locked [`File`] and carries no public fields, so no
/// backend/libsql type leaks (spine §6 rule 2). `Send` + `'static` — the engine (L5) holds it across
/// the whole mutation. On `Drop` it calls [`File::unlock`] explicitly (a legible backstop; closing
/// the fd would also release the lock, e.g. on a panic-driven drop).
#[derive(Debug)]
pub struct WriteLockGuard {
    /// The locked lock file. Its fd holds the OS advisory lock until this guard drops.
    file: File,
}

impl Drop for WriteLockGuard {
    fn drop(&mut self) {
        // Best-effort explicit release. Closing the fd (on this drop) also releases the advisory
        // lock, so a failure here cannot leak the lock; the explicit call just makes intent legible.
        let _ = self.file.unlock();
    }
}

/// Map a lock-file **open** failure to an opaque backend error (a broken lock path is not contention).
fn open_error(lock_path: &Path, err: &std::io::Error) -> StorageError {
    StorageError::Backend {
        source: BackendOpaque::from_message(format!(
            "failed to open write lock at {}: {err}",
            lock_path.display()
        )),
    }
}

/// Map an OS **lock** failure (not `WouldBlock`) to an opaque backend error.
fn lock_error(lock_path: &Path, err: &std::io::Error) -> StorageError {
    StorageError::Backend {
        source: BackendOpaque::from_message(format!(
            "failed to acquire write lock at {}: {err}",
            lock_path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{WRITE_LOCK_FILE_NAME, WriteLock};
    use crate::DEFAULT_WRITE_LOCK_TIMEOUT_MS;
    use crate::error::StorageError;
    use std::time::{Duration, Instant};

    /// A fresh temp dir for a lock file (unique per call).
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ub_write_lock_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// An uncontended acquire succeeds on the fast path and the lock file is created next to the db.
    #[tokio::test]
    async fn uncontended_acquire_succeeds_and_creates_the_file() {
        let dir = temp_dir("uncontended");
        let lock = WriteLock::new(&dir, DEFAULT_WRITE_LOCK_TIMEOUT_MS);
        let guard = lock.acquire().await.expect("acquire");
        assert!(dir.join(WRITE_LOCK_FILE_NAME).exists(), "lock file created");
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A held lock makes a second same-process acquire **time out** (bounded — no self-deadlock, no
    /// infinite park; AC-5). The wait sleeps to the (short) timeout, then returns retryable
    /// `DatabaseLocked` rather than hanging.
    #[tokio::test]
    async fn held_lock_makes_second_acquire_time_out_bounded() {
        let dir = temp_dir("selfblock");
        let lock = WriteLock::new(&dir, 150); // 150 ms budget.
        let held = lock.acquire().await.expect("first acquires");

        let start = Instant::now();
        let err = lock
            .acquire()
            .await
            .expect_err("second must time out while the first is held");
        let elapsed = start.elapsed();

        assert!(
            matches!(err, StorageError::DatabaseLocked),
            "a lock-acquire timeout must surface retryable DatabaseLocked, got {err:?}"
        );
        // It waited ~the timeout (bounded), never an unbounded hang.
        assert!(
            elapsed >= Duration::from_millis(100) && elapsed < Duration::from_secs(5),
            "wait must be bounded by the timeout (no self-deadlock), elapsed = {elapsed:?}"
        );

        // Releasing the first lets a fresh acquire succeed (the lock is not stuck).
        drop(held);
        let regained = lock.acquire().await.expect("reacquire after release");
        drop(regained);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A zero timeout is fail-fast: a single try, no poll — a contended `timeout=0` returns
    /// immediately (the `migrate` path).
    #[tokio::test]
    async fn zero_timeout_fails_fast_on_contention() {
        let dir = temp_dir("failfast");
        let lock = WriteLock::new(&dir, DEFAULT_WRITE_LOCK_TIMEOUT_MS);
        let held = lock.acquire().await.expect("first acquires");

        let start = Instant::now();
        let err = lock
            .acquire_with_timeout(Duration::ZERO)
            .await
            .expect_err("timeout=0 must fail fast under contention");
        assert!(matches!(err, StorageError::DatabaseLocked));
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "timeout=0 must not poll/park (single try)"
        );

        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
