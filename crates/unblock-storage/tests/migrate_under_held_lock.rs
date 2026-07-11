//! AC-6 / M4 (D31/T3.4.1) — `migrate()` fails FAST under a held `.write.lock`.
//!
//! `migrate` takes the cross-process advisory `.unblock/.write.lock` EXCLUSIVE for the whole command
//! with a **zero** timeout (single-try fail-fast, MF2): a concurrent writer holding the lock must make
//! `migrate` refuse with a retryable [`StorageError::DatabaseLocked`] rather than block/queue behind
//! it (never interleaving a schema change with an in-flight mutation).
//!
//! This drives the REAL `Storage::migrate()` entry point (not the `WriteLock` primitive directly). A
//! SEPARATE store instance holds the lock (via the public `acquire_write_lock`) and releases it after a
//! short delay:
//!
//! - **`timeout=0` (correct):** `migrate` fails fast **immediately** — while the lock is still held —
//!   so it returns `DatabaseLocked` (asserted below).
//! - **Non-vacuity:** were `migrate`'s acquire timeout NON-zero, it would POLL, outlast the holder's
//!   delayed release, then ACQUIRE and SUCCEED — turning the `DatabaseLocked` assertion RED. So
//!   flipping migrate's `Duration::ZERO` to any non-zero timeout makes this test fail (the guard is
//!   load-bearing).

#![cfg(unix)]

use std::time::Duration;

use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage, StorageError};

/// How long the separate holder keeps the `.write.lock` before releasing it. Long enough that the
/// zero-timeout `migrate` fail-fast (sub-millisecond) happens well inside the window, short enough
/// that a (mutated) polling `migrate` acquires soon after the release.
const HOLD_MS: u64 = 500;

/// Driving the REAL `migrate()` under a `.write.lock` held by another instance must fail FAST with
/// `DatabaseLocked` (NOT via the `WriteLock` primitive directly — this is the `Storage::migrate`
/// entry, MF2). A non-zero migrate acquire timeout would instead wait out the release and succeed,
/// turning this RED (the AC-6 non-vacuity).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migrate_under_a_held_write_lock_fails_fast() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("unblock.db");

    // The migrator store: a fresh file DB left at the PRE-migration schema version (user_version = 0,
    // below CURRENT_SCHEMA_VERSION), so `migrate` genuinely has schema work to do once it acquires.
    // `open_local` does NOT run migrations, so the DB stays non-current until `migrate` is called.
    let migrator = LibsqlStorage::open_local(&db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open migrator");

    // A SEPARATE store instance (distinct fd + distinct in-memory marker ≈ a second process) acquires
    // and HOLDS the cross-process `.write.lock`, then releases it after HOLD_MS.
    let holder = LibsqlStorage::open_local(&db_path, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open holder");
    let guard = holder
        .acquire_write_lock()
        .await
        .expect("acquire_write_lock")
        .expect("a file-backed store yields a real guard");
    let releaser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(HOLD_MS)).await;
        drop(guard); // release the flock
        drop(holder);
    });

    // The lock is held right now. migrate() must fail FAST (timeout=0), NOT wait out the HOLD_MS
    // release. A polling (non-zero) migrate would outlast the release and SUCCEED — that is the RED
    // this test manufactures.
    let start = std::time::Instant::now();
    let result = migrator.migrate().await;
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(StorageError::DatabaseLocked)),
        "migrate under a held .write.lock must fail-fast with DatabaseLocked, got {result:?} \
         (a non-zero migrate acquire timeout would have waited out the release and succeeded)"
    );
    // Secondary signal: it returned WELL before the holder's release (single-try, not a poll).
    assert!(
        elapsed < Duration::from_millis(HOLD_MS / 2),
        "migrate must fail-fast (single try), not poll to a timeout (elapsed = {elapsed:?})"
    );

    // Let the holder release, then migrate must succeed once the lock is free and advance the schema
    // (proving the lock is not stuck and migrate actually does its work under the lock).
    releaser.await.expect("holder task");
    migrator
        .migrate()
        .await
        .expect("migrate succeeds once the .write.lock is released");
    assert_eq!(
        migrator.schema_version().await.expect("schema_version"),
        1,
        "migrate advanced the fresh DB to the current schema version"
    );
}
