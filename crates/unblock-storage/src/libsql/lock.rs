//! The cross-process advisory **write lock** (D31 — a D14 amendment) — the restored beads
//! `.write.lock` serializer, re-homed as an L2 [`crate::Storage`] primitive.
//!
//! Under the supported child-per-client stdio topology (PRD §8.2) multiple MCP servers (`unblock mcp`)
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
//! - **Re-entrancy (MF4 — faithful to beads `config/mod.rs::write_lock_already_held`):** an in-memory
//!   held-marker ([`Arc<AtomicBool>`]) on [`WriteLock`] records whether **this** process's store
//!   currently holds the lock. A **nested** [`acquire`](WriteLock::acquire) by the current holder
//!   returns a **no-op borrowed guard** — it opens NO fresh fd. OS advisory locks are per-open-file-
//!   description, so a second `open` + `try_lock` in the same process would spuriously block against
//!   the holder's OWN fd (self-contention → a bogus [`StorageError::DatabaseLocked`]); the marker skips
//!   that. The in-process [`tokio::sync::Semaphore`] (L5) already bounds the store to one in-process
//!   writer, so the marker's check-then-set is serialized; the marker is set only AFTER the flock is
//!   truly held and cleared only when the **real** guard drops (a re-entrant no-op guard clears
//!   nothing). Nesting is stack-disciplined — the inner guard drops before the outer.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// The in-memory held-marker (MF4). `true` while **this** store holds the flock. A nested acquire
    /// by the current holder short-circuits to a no-op guard instead of self-contending on a fresh fd
    /// (faithful to beads `write_lock_already_held`). `Arc` so the RAII guard can clear it on drop.
    held: Arc<AtomicBool>,
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
            held: Arc::new(AtomicBool::new(false)),
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
    /// A `Duration::ZERO` timeout is a single non-blocking try (fail-fast) — used by `migrate`, which
    /// bypasses the per-mutation chokepoint and must refuse rather than queue behind a concurrent
    /// writer.
    ///
    /// A **nested** acquire by the current holder (MF4) short-circuits to a no-op borrowed guard
    /// regardless of `timeout` (it never touches the flock), so a `Duration::ZERO` nested acquire
    /// still succeeds.
    ///
    /// # Errors
    ///
    /// As [`acquire`](Self::acquire).
    pub(crate) async fn acquire_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<WriteLockGuard, StorageError> {
        // Re-entrancy (MF4): if THIS store already holds the flock, a nested acquire is a no-op
        // borrowed guard — do NOT open a fresh fd (the OS advisory lock is per-open-fd, so a second
        // fd in the same process would spuriously block against our own hold → a bogus
        // `DatabaseLocked`). The L5 `Semaphore` bounds the store to one in-process writer, so this
        // load-then-set is serialized; the marker is set only AFTER the flock is truly held (below).
        if self.held.load(Ordering::Acquire) {
            return Ok(WriteLockGuard {
                inner: GuardInner::Reentrant,
            });
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)
            .map_err(|err| open_error(&self.lock_path, &err))?;

        // Fast path: one non-blocking try for the uncontended common case.
        match file.try_lock() {
            Ok(()) => return Ok(self.locked_guard(file)),
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
                Ok(()) => return Ok(self.locked_guard(file)),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(err)) => return Err(lock_error(&self.lock_path, &err)),
            }
        }
    }

    /// Mark the store as holding the flock and wrap the locked file in a **real** RAII guard.
    ///
    /// Called only after `try_lock` truly acquired the OS advisory lock, so the in-memory marker
    /// reflects a genuinely held flock; the returned guard clears the marker (and unlocks) on drop.
    fn locked_guard(&self, file: File) -> WriteLockGuard {
        self.held.store(true, Ordering::Release);
        WriteLockGuard {
            inner: GuardInner::Locked {
                file,
                held: Arc::clone(&self.held),
            },
        }
    }
}

/// An RAII guard for the exclusive `.write.lock`. Dropping the **real** guard releases the OS advisory
/// lock; a **re-entrant** guard (MF4) owns nothing and releases nothing.
///
/// A **public opaque** guard (returned across the `Storage` trait boundary as
/// `Option<WriteLockGuard>`): it carries no public fields (the [`GuardInner`] enum is private), so no
/// backend/libsql type leaks (spine §6 rule 2). `Send` + `'static` — the engine (L5) holds it across
/// the whole mutation. The real variant, on `Drop`, calls [`File::unlock`] explicitly (a legible
/// backstop; closing the fd would also release the lock, e.g. on a panic-driven drop) and clears the
/// store's in-memory held-marker.
#[derive(Debug)]
pub struct WriteLockGuard {
    /// Real (owns the locked file + held-marker) vs re-entrant no-op (owns nothing).
    inner: GuardInner,
}

/// The two guard shapes: a **real** hold of the flock, or a **re-entrant** no-op (MF4).
#[derive(Debug)]
enum GuardInner {
    /// The real guard: its fd holds the OS advisory lock, and `held` is the store's in-memory marker
    /// (set to `true` when this guard was created), cleared on drop.
    Locked {
        /// The locked lock file. Its fd holds the OS advisory lock until this guard drops.
        file: File,
        /// The store's held-marker; cleared to `false` when this real guard drops.
        held: Arc<AtomicBool>,
    },
    /// A nested acquire by the current holder: the flock was already held, so this guard owns nothing
    /// and releases nothing on drop (it must NOT clear the marker — the outer real guard owns that).
    Reentrant,
}

impl Drop for WriteLockGuard {
    fn drop(&mut self) {
        if let GuardInner::Locked { file, held } = &self.inner {
            // Best-effort explicit release. Closing the fd (on this drop) also releases the advisory
            // lock, so a failure here cannot leak the lock; the explicit call just makes intent
            // legible. Clear the in-memory marker AFTER releasing the flock so a subsequent acquire
            // (which loads the marker) then re-opens + `try_lock`s a fresh fd against a free lock.
            let _ = file.unlock();
            held.store(false, Ordering::Release);
        }
        // Reentrant: nothing owned, nothing to release.
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

    /// A held lock makes a **separate holder** (a distinct `WriteLock` instance = distinct in-memory
    /// marker + distinct fd, modelling a second process on the same `.write.lock`) **time out**
    /// (bounded — no infinite park; AC-5). The wait sleeps to the (short) timeout, then returns
    /// retryable `DatabaseLocked` rather than hanging. (A nested acquire by the SAME holder is instead
    /// a re-entrant no-op — see `nested_acquire_by_the_current_holder_is_a_noop`.)
    #[tokio::test]
    async fn held_lock_makes_a_separate_holder_time_out_bounded() {
        let dir = temp_dir("crossholder");
        // Two independent holders on the same lock file (two markers, two fds ≈ two processes).
        let holder_a = WriteLock::new(&dir, 150); // 150 ms budget.
        let holder_b = WriteLock::new(&dir, 150);
        let held = holder_a.acquire().await.expect("A acquires");

        let start = Instant::now();
        let err = holder_b
            .acquire()
            .await
            .expect_err("B must time out while A holds");
        let elapsed = start.elapsed();

        assert!(
            matches!(err, StorageError::DatabaseLocked),
            "a lock-acquire timeout must surface retryable DatabaseLocked, got {err:?}"
        );
        // It waited ~the timeout (bounded), never an unbounded hang.
        assert!(
            elapsed >= Duration::from_millis(100) && elapsed < Duration::from_secs(5),
            "wait must be bounded by the timeout, elapsed = {elapsed:?}"
        );

        // Releasing A lets B acquire (the lock is not stuck).
        drop(held);
        let regained = holder_b
            .acquire()
            .await
            .expect("B acquires after A releases");
        drop(regained);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A NESTED acquire by the **current holder** (the SAME `WriteLock` instance = the same in-memory
    /// marker) is a re-entrant no-op: it returns immediately WITHOUT opening a fresh fd, so it never
    /// blocks and never surfaces a spurious `DatabaseLocked` (the ported beads `write_lock_already_held`
    /// guard, MF4). Even a `timeout=0` nested acquire succeeds, since it never touches the flock.
    #[tokio::test]
    async fn nested_acquire_by_the_current_holder_is_a_noop() {
        let dir = temp_dir("reentrant");
        // A short timeout: were the nested acquire to actually contend on the flock it would block
        // ~150 ms then fail — so an immediate success proves the re-entrancy short-circuit fired.
        let lock = WriteLock::new(&dir, 150);
        let outer = lock.acquire().await.expect("outer acquires");

        let start = Instant::now();
        let nested = lock
            .acquire()
            .await
            .expect("nested acquire by the holder must NOT block or error");
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "nested acquire must be an immediate no-op (no poll/park)"
        );
        // A zero-timeout nested acquire also succeeds (it never touches the flock).
        let nested_zero = lock
            .acquire_with_timeout(Duration::ZERO)
            .await
            .expect("nested timeout=0 acquire is a no-op success");

        // Drop the re-entrant guards first (they own/clear nothing), then the real outer guard, which
        // releases the flock and clears the marker.
        drop(nested_zero);
        drop(nested);
        drop(outer);

        // After the real guard released, a separate holder can acquire the now-free lock.
        let other = WriteLock::new(&dir, 150);
        let g = other
            .acquire()
            .await
            .expect("separate holder acquires after the real guard released");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A zero timeout is fail-fast: a single try, no poll — a contended `timeout=0` from a SEPARATE
    /// holder (distinct instance = distinct marker + fd, modelling the cross-process `migrate` path)
    /// returns immediately with `DatabaseLocked`.
    #[tokio::test]
    async fn zero_timeout_fails_fast_on_cross_holder_contention() {
        let dir = temp_dir("failfast");
        let holder_a = WriteLock::new(&dir, DEFAULT_WRITE_LOCK_TIMEOUT_MS);
        let migrator = WriteLock::new(&dir, DEFAULT_WRITE_LOCK_TIMEOUT_MS);
        let held = holder_a.acquire().await.expect("A acquires");

        let start = Instant::now();
        let err = migrator
            .acquire_with_timeout(Duration::ZERO)
            .await
            .expect_err("timeout=0 must fail fast under cross-holder contention");
        assert!(matches!(err, StorageError::DatabaseLocked));
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "timeout=0 must not poll/park (single try)"
        );

        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
