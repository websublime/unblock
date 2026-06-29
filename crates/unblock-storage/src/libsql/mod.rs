//! [`LibsqlStorage`] — the only backend-aware [`Storage`] implementation (libsql / bundled `SQLite`).
//!
//! # Connection model (OQ-5, spine §3.3 — RESOLVED)
//!
//! `LibsqlStorage` holds **two** connections opened from one `libsql::Database`:
//! - a serialized **write** connection — every mutation runs through a `BEGIN IMMEDIATE` transaction
//!   on it; the engine's D14 `Semaphore` serializes writers at L5, so the storage layer itself does
//!   not need its own write lock; and
//! - a separate **read** connection — WAL gives it concurrent MVCC reader snapshots against the
//!   single writer (FR-10), so reads never serialize behind writes.
//!
//! For [`open_in_memory`](LibsqlStorage::open_in_memory) a bare `:memory:` is connection-private, so
//! both connections would otherwise see different databases. The constructor therefore opens a
//! **named shared-cache in-memory URI** (`file:<unique>?mode=memory&cache=shared`) — valid because
//! libsql-ffi compiles `SQLite` with `SQLITE_USE_URI` — so the write and read connections share the
//! same in-memory database while remaining isolated from any other `open_in_memory()` instance.
//!
//! # Concurrency discipline (NFR-3)
//!
//! Both connections set a **native** `busy_timeout` ([`BUSY_TIMEOUT_MS`]) — sleep-based, never
//! spinning. This is the sanctioned **inverse** of the original `beads` storage, which set
//! `busy_timeout = 0` and hand-rolled a flock + sleep backoff to dodge *frankensqlite*'s hot-spin;
//! libsql ships real `SQLite`, whose native timeout resolves that defect by construction.

mod crud;
mod deps;
mod diagnostics;
mod events;
mod ids;
mod mappers;
mod migrations;
mod mutate;
mod query;
mod schema;

// The `StorageTestkit` impl for `LibsqlStorage` lives **in-module** (gated) so it can reach the
// `pub(super)` connection accessors (`read`/`write`) and `ids::next_child_number` without widening
// any visibility at the crate root (resolved-decision #1). It is compiled for the crate's own tests
// and when the `testkit` feature is on.
#[cfg(any(test, feature = "testkit"))]
mod testkit;

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use libsql::{Builder, Connection, Database, TransactionBehavior};
use tokio::sync::Mutex;

use unblock_model::{
    CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue, ListFilters,
};

use crate::error::{StorageError, is_busy_locked, map_libsql_err};
use crate::filters::{DeletePlan, IssuePatch};
use crate::trait_def::Storage;

/// Native `busy_timeout`, in milliseconds (spine §3.3, OQ-2 RESOLVED).
///
/// Sleep-based and non-spinning — the sanctioned inverse of beads's `busy_timeout = 0` + backoff.
pub(crate) const BUSY_TIMEOUT_MS: u64 = 5000;

/// Passive WAL-checkpoint cadence: fire `PRAGMA wal_checkpoint(PASSIVE)` on the held write connection
/// once every `CHECKPOINT_EVERY_N_MUTATIONS` committed mutations (spine §3.3 — resolved at T0.8).
///
/// PASSIVE never blocks readers or writers and never takes the exclusive lock that TRUNCATE would, so
/// it cannot manufacture contention in the write hot path; it only opportunistically folds committed
/// WAL frames back into the main database so a long-lived `serve` does not grow the WAL unboundedly
/// (`wal_autocheckpoint = 0` disables `SQLite`'s own automatic checkpointing — see [`apply_pragmas`]).
/// The T0.8 contention lab asserts the WAL sidecar stays bounded under sustained multi-instance
/// contention with this cadence on, and (a `#[ignore]`d negative control) that it breaches the
/// ceiling with it off.
pub(crate) const CHECKPOINT_EVERY_N_MUTATIONS: u64 = 50;

/// Monotonic counter giving each `open_in_memory()` a unique shared-cache name, so two in-memory
/// stores never collide on the process-global `SQLite` shared cache.
static MEMORY_DB_SEQ: AtomicU64 = AtomicU64::new(0);

/// Serializes the **shared-cache in-memory open** sequence across the whole process.
///
/// `open_in_memory()` opens a `cache=shared` URI. Opening a shared-cache database mutates `SQLite`'s
/// **process-global shared-cache registry** (the `sqlite3SharedCacheList` linked list), and libsql
/// opens connections with the default mutex flags — so two threads racing `sqlite3_open_v2` on
/// shared-cache URIs concurrently can corrupt that global step and surface `SQLITE_MISUSE`
/// ("bad parameter or other API misuse"). This is a genuine `SQLite` shared-cache concurrency
/// limitation, not cross-store contention: even *distinct* shared-cache names race in the global
/// list. Serializing the open (build + both `connect()`s + pragmas) removes the race at its source —
/// this is mutual exclusion around a non-reentrant global op, NOT a retry/sleep band-aid.
///
/// Scoped to the **in-memory** path only: `open_local()` opens a private file with no shared cache and
/// is unaffected (and production runs on file DBs — D14/D15 — so this never serializes a hot path; an
/// open happens once per workspace). The guard is a `tokio` async mutex because the guarded sequence
/// awaits (`build`/pragmas); the brief critical section is the open, not the lifetime of the store.
fn memory_open_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Process-internal instrumentation for a [`LibsqlStorage`] — three monotonic counters plus the
/// test-controllable checkpoint cadence and the (test-only) busy-witness/spin toggles.
///
/// In production only the mutation counter and the passive checkpoint cadence are live (the witness
/// and spin toggles default off and are flipped solely by the gated `StorageTestkit` seam). The
/// counters are plain [`AtomicU64`]/[`AtomicBool`] — unsafe-free — and exist so the T0.8 contention
/// lab can prove (a) contention materialized (busy-retry > 0 under contention, == 0 without) and
/// (b) the passive checkpoint keeps the WAL bounded.
#[derive(Debug)]
pub(super) struct StorageInstrument {
    /// Committed mutations (every successful `with_immediate_tx` commit bumps this).
    mutation_count: AtomicU64,
    /// Witnessed write-lock contention events (a `BEGIN IMMEDIATE` that observed the file write-lock
    /// held by another writer — see [`with_immediate_tx`]).
    busy_retry_count: AtomicU64,
    /// Passive WAL checkpoints fired by the periodic cadence.
    checkpoint_count: AtomicU64,
    /// Mutations between passive checkpoints (`0` disables the cadence). Defaults to
    /// [`CHECKPOINT_EVERY_N_MUTATIONS`]; the contention lab sets it to `0` inside the timed brackets.
    checkpoint_interval: AtomicU64,
    /// When `true`, each mutating `BEGIN IMMEDIATE` first runs a **zero-timeout probe** to witness
    /// write-lock contention deterministically without changing the real (blocking) transaction's
    /// semantics. Off in production; the lab enables it so the busy-retry witness is observable.
    busy_witness: AtomicBool,
}

impl Default for StorageInstrument {
    fn default() -> Self {
        Self {
            mutation_count: AtomicU64::new(0),
            busy_retry_count: AtomicU64::new(0),
            checkpoint_count: AtomicU64::new(0),
            checkpoint_interval: AtomicU64::new(CHECKPOINT_EVERY_N_MUTATIONS),
            busy_witness: AtomicBool::new(false),
        }
    }
}

impl StorageInstrument {
    /// Record one committed mutation; returns `true` when the passive-checkpoint cadence is due.
    ///
    /// The cadence is due when the interval is non-zero and the new count is an exact multiple of it.
    fn record_mutation(&self) -> bool {
        let count = self.mutation_count.fetch_add(1, Ordering::Relaxed) + 1;
        let interval = self.checkpoint_interval.load(Ordering::Relaxed);
        interval != 0 && count.is_multiple_of(interval)
    }

    /// Bump the witnessed-contention counter.
    fn record_busy(&self) {
        self.busy_retry_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Bump the passive-checkpoint counter.
    fn record_checkpoint(&self) {
        self.checkpoint_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether the zero-timeout busy-witness probe is enabled.
    fn witness_enabled(&self) -> bool {
        self.busy_witness.load(Ordering::Relaxed)
    }

    /// Read the committed-mutation count (testkit seam).
    #[cfg(any(test, feature = "testkit"))]
    pub(super) fn mutation_count(&self) -> u64 {
        self.mutation_count.load(Ordering::Relaxed)
    }

    /// Read the witnessed write-lock-contention count (testkit seam).
    #[cfg(any(test, feature = "testkit"))]
    pub(super) fn busy_retry_count(&self) -> u64 {
        self.busy_retry_count.load(Ordering::Relaxed)
    }

    /// Read the passive-checkpoint count (testkit seam).
    #[cfg(any(test, feature = "testkit"))]
    pub(super) fn checkpoint_count(&self) -> u64 {
        self.checkpoint_count.load(Ordering::Relaxed)
    }

    /// Set the passive-checkpoint cadence (`0` disables it) — testkit seam. The contention lab sets
    /// `0` inside its timed brackets so checkpoint CPU never enters the CPU-per-write ratio.
    #[cfg(any(test, feature = "testkit"))]
    pub(super) fn set_checkpoint_interval(&self, n: u64) {
        self.checkpoint_interval.store(n, Ordering::Relaxed);
    }

    /// Enable/disable the zero-timeout busy-witness probe — testkit seam. The contention lab enables
    /// it so the busy-retry witness is observable from safe Rust.
    #[cfg(any(test, feature = "testkit"))]
    pub(super) fn set_busy_witness(&self, on: bool) {
        self.busy_witness.store(on, Ordering::Relaxed);
    }
}

/// The instrumentation context threaded into the mutating sibling functions so their
/// [`with_immediate_tx`] calls witness contention (and, in the forced-spin control, spin-retry) and
/// drive the passive-checkpoint cadence. Borrowed from the [`LibsqlStorage`] for one mutation.
#[derive(Clone, Copy)]
pub(super) struct WriteHook<'a> {
    /// The store's instrumentation (counters + cadence + toggles).
    pub(super) instrument: &'a StorageInstrument,
    /// The native `busy_timeout` (ms) — `BUSY_TIMEOUT_MS` in production, `0` for the forced-spin control.
    pub(super) busy_timeout_ms: u64,
}

/// The libsql-backed [`Storage`] implementation (local file / bundled `SQLite`).
///
/// Holds a serialized write connection and a separate read connection (see the module docs). No
/// libsql type appears in any public signature (spine §6 rule 2): construction is via
/// [`open_local`](Self::open_local) / [`open_in_memory`](Self::open_in_memory), and failures surface
/// as [`StorageError`].
pub struct LibsqlStorage {
    /// Keeps the underlying `Database` alive for the lifetime of the connections. For a shared-cache
    /// in-memory DB the cache is reference-counted by `SQLite`; holding the handle documents the
    /// ownership and keeps a single source of truth for both connections.
    _db: Database,
    /// The serialized write connection. The async [`Mutex`] guarantees one in-flight `BEGIN
    /// IMMEDIATE` mutation at a time *within this process* even if the engine's D14 permit is ever
    /// bypassed; under normal operation it is uncontended.
    write_conn: Mutex<Connection>,
    /// The read connection (WAL MVCC reader snapshots; never serialized behind the writer).
    read_conn: Connection,
    /// The native `busy_timeout` (ms) applied to both connections. Always [`BUSY_TIMEOUT_MS`] in
    /// production; the gated forced-spin test constructor sets it to `0` so losers spin-retry
    /// (proving the contention-lab metric actually detects a hot-spin).
    busy_timeout_ms: u64,
    /// Process-internal instrumentation (counters + checkpoint cadence + the test-only
    /// busy-witness/spin toggles). See [`StorageInstrument`].
    instrument: StorageInstrument,
}

impl LibsqlStorage {
    /// Open (creating if absent) a local libsql database at `path`.
    ///
    /// Applies the runtime pragmas (WAL, native `busy_timeout`, foreign keys, …) to both the write
    /// and read connections. Does **not** run migrations — call [`Storage::migrate`] next.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the database cannot be opened or a pragma fails.
    pub async fn open_local(path: &Path) -> Result<Self, StorageError> {
        let db = Builder::new_local(path)
            .build()
            .await
            .map_err(map_libsql_err)?;
        // A real file: WAL applies (real WAL + native busy_timeout concurrency, validated by the
        // T0.8 contention lab on a file DB).
        Self::from_database(db, true, BUSY_TIMEOUT_MS).await
    }

    /// Open a local libsql database at `path` with a **non-default** native `busy_timeout`.
    ///
    /// Gated to tests / the `testkit` feature: the only sanctioned caller is the T0.8 contention
    /// lab's **forced-spin control**, which passes `busy_timeout_ms = 0` so write-lock losers
    /// surface `SQLITE_BUSY` immediately and the storage spin-retries them at the application level
    /// (the beads anti-pattern, deliberately reproduced). That proves the lab's CPU-per-write ratio
    /// metric actually *detects* a hot-spin (a non-vacuous gate). Production never uses this path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the database cannot be opened or a pragma fails.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn open_local_with_busy_timeout(
        path: &Path,
        busy_timeout_ms: u64,
    ) -> Result<Self, StorageError> {
        let db = Builder::new_local(path)
            .build()
            .await
            .map_err(map_libsql_err)?;
        Self::from_database(db, true, busy_timeout_ms).await
    }

    /// Open a fresh, process-unique in-memory libsql database shared by the write and read
    /// connections (named shared-cache URI; see the module docs).
    ///
    /// The in-memory store uses **shared-cache, NOT WAL**: a `SQLite` in-memory database cannot use
    /// WAL (it always reports `journal_mode = memory`), so the WAL/`wal_autocheckpoint` pragmas are
    /// skipped on this path (asserting WAL there is a no-op). Real WAL + `busy_timeout` concurrency is
    /// validated by the **T0.8 contention lab on a file DB**, not here.
    ///
    /// The whole open sequence (build + connect + pragmas) is serialized process-wide via
    /// [`memory_open_lock`]: opening a `cache=shared` URI mutates `SQLite`'s global shared-cache
    /// registry, which is not safe to run concurrently and otherwise intermittently surfaces
    /// `SQLITE_MISUSE` ("bad parameter or other API misuse") under parallel opens — a real `SQLite`
    /// limitation, fixed at source here rather than masked (T0.9; root-fix of the T0.6 flake).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the database cannot be opened or a pragma fails.
    pub async fn open_in_memory() -> Result<Self, StorageError> {
        let seq = MEMORY_DB_SEQ.fetch_add(1, Ordering::Relaxed);
        // A unique name per instance keeps two in-memory stores isolated while letting this store's
        // two connections share one cache. `mode=memory` + `cache=shared` is interpreted because
        // libsql-ffi compiles SQLite with SQLITE_USE_URI.
        let uri = format!("file:unblock_mem_{seq}?mode=memory&cache=shared");
        // Serialize the shared-cache open: `sqlite3_open_v2` on a `cache=shared` URI mutates SQLite's
        // process-global shared-cache registry, which is not safe to run concurrently from multiple
        // threads (it intermittently surfaces SQLITE_MISUSE). Hold the global open-lock for the whole
        // build + connect + pragma sequence (`from_database`), then drop it; the store then runs
        // fully concurrently. Scoped to in-memory only — `open_local` has no shared cache. See
        // [`memory_open_lock`].
        let _open_guard = memory_open_lock().lock().await;
        let db = Builder::new_local(&uri)
            .build()
            .await
            .map_err(map_libsql_err)?;
        // In-memory: shared-cache, NOT WAL (see the method docs + `apply_pragmas`).
        Self::from_database(db, false, BUSY_TIMEOUT_MS).await
    }

    /// Build the two connections from an opened `Database` and apply the runtime pragmas to each.
    ///
    /// `file_backed` selects whether the WAL-only pragmas are applied (only file databases can use
    /// WAL; a shared-cache `:memory:` DB reports `journal_mode = memory` regardless).
    /// `busy_timeout_ms` is the native sleep-based busy timeout applied to both connections (always
    /// [`BUSY_TIMEOUT_MS`] except on the gated forced-spin control path, which passes `0`).
    async fn from_database(
        db: Database,
        file_backed: bool,
        busy_timeout_ms: u64,
    ) -> Result<Self, StorageError> {
        let write_conn = db.connect().map_err(map_libsql_err)?;
        let read_conn = db.connect().map_err(map_libsql_err)?;
        apply_pragmas(&write_conn, file_backed, busy_timeout_ms).await?;
        apply_pragmas(&read_conn, file_backed, busy_timeout_ms).await?;
        Ok(Self {
            _db: db,
            write_conn: Mutex::new(write_conn),
            read_conn,
            busy_timeout_ms,
            instrument: StorageInstrument::default(),
        })
    }

    /// Borrow the read connection (WAL reader snapshots; never serialized behind the writer).
    pub(super) fn read(&self) -> &Connection {
        &self.read_conn
    }

    /// Lock and borrow the write connection. Mutations acquire this, run a `BEGIN IMMEDIATE`
    /// transaction, and release it on return.
    pub(super) async fn write(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        self.write_conn.lock().await
    }

    /// Borrow the process-internal instrumentation (counters + checkpoint cadence + test toggles).
    ///
    /// Gated: the only consumer is the in-module [`StorageTestkit`](crate::testkit::StorageTestkit)
    /// impl, which exposes the counters/toggles through the gated seam.
    #[cfg(any(test, feature = "testkit"))]
    pub(super) fn instrument(&self) -> &StorageInstrument {
        &self.instrument
    }

    /// The per-mutation instrumentation context for the mutating sibling functions.
    pub(super) fn hook(&self) -> WriteHook<'_> {
        WriteHook {
            instrument: &self.instrument,
            busy_timeout_ms: self.busy_timeout_ms,
        }
    }
}

/// Apply the runtime pragmas to a connection (spine §3.3, ported from the original schema.rs:606-643
/// — except `busy_timeout`, which is the native non-spinning inverse of beads).
///
/// Sets, in order: native `busy_timeout`; (file-backed only) WAL journal mode + `wal_autocheckpoint
/// = 0`; `foreign_keys = ON`; `synchronous = NORMAL`; `temp_store = MEMORY`; `cache_size = -8000`
/// (≈8 MiB); `journal_size_limit = 33554432` (bound WAL growth).
///
/// **The WAL-only pragmas (`journal_mode = WAL`, `wal_autocheckpoint = 0`) are applied only when
/// `file_backed`.** A shared-cache `:memory:` database cannot use WAL — it always reports
/// `journal_mode = memory` — so asserting WAL there is a no-op; the in-memory store relies on
/// shared-cache + the native `busy_timeout`, and real WAL concurrency is validated by the T0.8
/// contention lab on a file DB. (The intermittent "bad parameter or other API misuse" seen under
/// parallel in-memory opens is **not** caused by this pragma — it is the `SQLite` shared-cache
/// global-open race, serialized at source by [`memory_open_lock`] in `open_in_memory`.)
async fn apply_pragmas(
    conn: &Connection,
    file_backed: bool,
    busy_timeout_ms: u64,
) -> Result<(), StorageError> {
    // Native, sleep-based busy handler (NFR-3). Set first so any subsequent switch can wait rather
    // than fail under a concurrent open. `busy_timeout_ms` is always `BUSY_TIMEOUT_MS` (5000) in
    // production; the gated forced-spin control passes `0` so losers surface `SQLITE_BUSY`.
    conn.busy_timeout(Duration::from_millis(busy_timeout_ms))
        .map_err(map_libsql_err)?;

    // WAL-only pragmas: file-backed databases only (in-memory cannot use WAL).
    if file_backed {
        for pragma in ["PRAGMA journal_mode = WAL", "PRAGMA wal_autocheckpoint = 0"] {
            let _ = conn.query(pragma, ()).await.map_err(map_libsql_err)?;
        }
    }

    // Several of these PRAGMAs (journal_size_limit, …) return a result row, which `execute` rejects
    // with `ExecuteReturnedRows`. Run them via `query`, which consumes any returned rows. The `Rows`
    // is dropped immediately (we only set, never read here).
    for pragma in [
        "PRAGMA foreign_keys = ON",
        "PRAGMA synchronous = NORMAL",
        "PRAGMA temp_store = MEMORY",
        "PRAGMA cache_size = -8000",
        "PRAGMA journal_size_limit = 33554432",
    ] {
        let _ = conn.query(pragma, ()).await.map_err(map_libsql_err)?;
    }
    Ok(())
}

/// Run `op` inside a `BEGIN IMMEDIATE` transaction on `conn`, committing on `Ok` and rolling back on
/// `Err` (mutating-transaction helper, spine §3.3), instrumented for the contention lab.
///
/// The closure returns its own `Result`; a transaction-open or commit failure is mapped through
/// [`map_libsql_err`]. On `Err` the transaction is rolled back (a rollback failure is swallowed —
/// the original error is the one worth surfacing; an uncommitted libsql `Transaction` also rolls
/// back on drop).
///
/// # Acquiring the `BEGIN IMMEDIATE` write lock
///
/// `conn` is the **held** write connection (its [`Mutex`] guard is owned by the caller), so this
/// function has exclusive use of it. How the write lock is acquired depends on the configured
/// `busy_timeout_ms`:
///
/// - **Normal (production) — `busy_timeout_ms > 0`.** A single blocking `BEGIN IMMEDIATE`; the native
///   sleep-based busy handler resolves cross-instance write-lock contention by *blocking*, never
///   spinning (NFR-3). When the (test-only) busy-witness toggle is on, a **zero-timeout probe** runs
///   first: it flips the connection's `busy_timeout` to 0 and tries to begin. If the probe observes
///   the write lock held by another writer (`SQLITE_BUSY`/`SQLITE_LOCKED`), it records one witnessed
///   contention event and falls through to the real blocking begin; if the probe *acquires*, that
///   very transaction is used (no redundant begin). The probe changes nothing about the blocking
///   semantics the gate measures — it only makes contention observable from safe Rust (libsql exposes
///   no busy-handler callback). It is off in production.
/// - **Forced-spin control — `busy_timeout_ms == 0`.** The native handler is disabled, so a contended
///   `BEGIN IMMEDIATE` returns `SQLITE_BUSY` immediately. This deliberately reproduces the beads
///   anti-pattern: a tight application-level retry loop re-begins until it acquires, recording one
///   busy-retry per spin. That burns CPU and is the *only* path that does — it exists so the
///   contention lab can prove its CPU-per-write ratio metric actually detects a hot-spin.
/// # Post-commit instrumentation
///
/// On a successful commit the mutation counter is bumped and, once every
/// [`CHECKPOINT_EVERY_N_MUTATIONS`] committed mutations (the test-controllable cadence on
/// `instrument`), a **passive** WAL checkpoint fires on this same held connection. A failed/rolled-back
/// transaction touches no counter. This is the single commit chokepoint every `Storage` mutation funnels
/// through, so the cadence is exact and process-wide for the store.
pub(super) async fn with_immediate_tx<F, Fut, T>(
    conn: &Connection,
    hook: WriteHook<'_>,
    op: F,
) -> Result<T, StorageError>
where
    F: FnOnce(libsql::Transaction) -> Fut,
    Fut: std::future::Future<Output = Result<(T, libsql::Transaction), StorageError>>,
{
    let tx = begin_immediate(conn, hook).await?;
    let value = match op(tx).await {
        Ok((value, tx)) => {
            tx.commit().await.map_err(map_libsql_err)?;
            value
        }
        Err(err) => return Err(err),
    };
    // The mutation committed: record it and fire the passive checkpoint when the cadence is due.
    if hook.instrument.record_mutation() {
        passive_checkpoint(conn).await;
        hook.instrument.record_checkpoint();
    }
    Ok(value)
}

/// Acquire a `BEGIN IMMEDIATE` transaction on the held write connection, applying the busy policy
/// described on [`with_immediate_tx`] (witness probe in normal mode, spin-retry in forced-spin mode).
async fn begin_immediate(
    conn: &Connection,
    hook: WriteHook<'_>,
) -> Result<libsql::Transaction, StorageError> {
    let WriteHook {
        instrument,
        busy_timeout_ms,
    } = hook;
    if busy_timeout_ms == 0 {
        // Forced-spin control ONLY (never a production path — production always uses
        // `BUSY_TIMEOUT_MS`): the native handler is off, so begin returns SQLITE_BUSY immediately on
        // contention. This **tight, non-yielding** retry loop deliberately reproduces the *frankensqlite*
        // defect-243 hot-spin — it pins the worker thread, burning CPU while it waits, so the
        // contention lab's CPU-per-write metric can prove it actually detects a spin. (A cooperative
        // `yield_now()` here would defer to the runtime and *not* burn CPU, masking the very failure
        // the control exists to surface — so it is intentionally absent. The forced-spin control runs
        // on a runtime sized with more worker threads than writers so the lock holder never starves.)
        loop {
            match conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .await
            {
                Ok(tx) => return Ok(tx),
                Err(err) if is_busy_locked(&err) => instrument.record_busy(),
                Err(err) => return Err(map_libsql_err(err)),
            }
        }
    }

    if instrument.witness_enabled() {
        // Zero-timeout witness probe: flip busy_timeout to 0, try to begin. The held-connection
        // guard guarantees exclusive use, so toggling the timeout is race-free.
        conn.busy_timeout(Duration::ZERO).map_err(map_libsql_err)?;
        let probe = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await;
        // Restore the real (blocking) timeout before anything else can begin on this connection.
        conn.busy_timeout(Duration::from_millis(busy_timeout_ms))
            .map_err(map_libsql_err)?;
        match probe {
            // The probe acquired the write lock with no contention: use this transaction directly.
            Ok(tx) => return Ok(tx),
            // The probe observed another writer holding the lock: record it, then fall through to the
            // real blocking begin (which the native handler resolves by sleeping, not spinning).
            Err(err) if is_busy_locked(&err) => instrument.record_busy(),
            Err(err) => return Err(map_libsql_err(err)),
        }
    }

    // The real, blocking begin (native sleep-based busy handler; never spins).
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .await
        .map_err(map_libsql_err)
}

/// Fire a **passive** WAL checkpoint on the (held) write connection, swallowing any error.
///
/// PASSIVE checkpoints opportunistically copy committed WAL frames into the main database without
/// taking the exclusive lock that TRUNCATE would and without blocking concurrent readers/writers, so
/// it can never manufacture contention in the write hot path. A non-WAL connection (the in-memory
/// shared-cache path) simply yields no rows. The error is swallowed: a checkpoint is best-effort
/// housekeeping — a transient failure must never fail the mutation that already committed.
async fn passive_checkpoint(conn: &Connection) {
    let _ = conn.query("PRAGMA wal_checkpoint(PASSIVE)", ()).await;
}

#[async_trait]
impl Storage for LibsqlStorage {
    async fn migrate(&self) -> Result<(), StorageError> {
        // Migration is a write-path operation: serialize it through the write connection.
        let conn = self.write().await;
        migrations::run_migrations(&conn).await
    }

    async fn integrity_check(&self) -> Result<Vec<String>, StorageError> {
        diagnostics::integrity_check(self.read()).await
    }

    async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError> {
        let conn = self.write().await;
        crud::create_issue(&conn, self.hook(), issue, actor).await
    }

    async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError> {
        crud::get_issue(self.read(), id).await
    }

    async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError> {
        crud::get_issues(self.read(), ids).await
    }

    async fn update_issue(
        &self,
        id: &str,
        patch: &IssuePatch,
        actor: &str,
    ) -> Result<Issue, StorageError> {
        let conn = self.write().await;
        crud::update_issue(&conn, self.hook(), id, patch, actor).await
    }

    async fn delete_issue(
        &self,
        plan: &DeletePlan,
        actor: &str,
    ) -> Result<DeletePlan, StorageError> {
        let conn = self.write().await;
        crud::delete_issue(&conn, self.hook(), plan, actor).await
    }

    async fn restore_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
        let conn = self.write().await;
        crud::restore_issue(&conn, self.hook(), id, actor).await
    }

    async fn claim_issue(
        &self,
        id: &str,
        assignee: &str,
        actor: &str,
    ) -> Result<Issue, StorageError> {
        let conn = self.write().await;
        mutate::claim_issue(&conn, self.hook(), id, assignee, actor).await
    }

    async fn defer_issue(
        &self,
        id: &str,
        until: DateTime<Utc>,
        actor: &str,
    ) -> Result<Issue, StorageError> {
        let conn = self.write().await;
        mutate::defer_issue(&conn, self.hook(), id, until, actor).await
    }

    async fn undefer_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
        let conn = self.write().await;
        mutate::undefer_issue(&conn, self.hook(), id, actor).await
    }

    async fn list_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
        query::list_issues(self.read(), filters).await
    }

    async fn ready_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
        query::ready_issues(self.read(), filters).await
    }

    async fn blocked_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError> {
        query::blocked_issues(self.read(), filters).await
    }

    async fn search_issues(
        &self,
        query: &str,
        filters: &ListFilters,
    ) -> Result<Vec<Issue>, StorageError> {
        query::search_issues(self.read(), query, filters).await
    }

    async fn count_issues(
        &self,
        filters: &ListFilters,
        group_by: Option<CountGroupBy>,
    ) -> Result<Vec<CountBucket>, StorageError> {
        query::count_issues(self.read(), filters, group_by).await
    }

    async fn stale_issues(
        &self,
        older_than: DateTime<Utc>,
        filters: &ListFilters,
    ) -> Result<Vec<Issue>, StorageError> {
        query::stale_issues(self.read(), older_than, filters).await
    }

    async fn add_dependency(&self, dep: &Dependency, actor: &str) -> Result<(), StorageError> {
        let conn = self.write().await;
        deps::add_dependency(&conn, self.hook(), dep, actor).await
    }

    async fn remove_dependency(
        &self,
        issue_id: &str,
        depends_on_id: &str,
        dep_type: &DependencyType,
        actor: &str,
    ) -> Result<(), StorageError> {
        let conn = self.write().await;
        deps::remove_dependency(&conn, self.hook(), issue_id, depends_on_id, dep_type, actor).await
    }

    async fn list_dependencies(&self, id: &str) -> Result<Vec<Dependency>, StorageError> {
        deps::list_dependencies(self.read(), id).await
    }

    async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError> {
        deps::dependency_tree(self.read(), id).await
    }

    async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError> {
        deps::dependency_graph(self.read(), roots).await
    }

    async fn detect_cycles(&self, blocking_only: bool) -> Result<Vec<Vec<String>>, StorageError> {
        deps::detect_cycles(self.read(), blocking_only).await
    }

    async fn list_events(&self, issue_id: &str) -> Result<Vec<Event>, StorageError> {
        events::list_events(self.read(), issue_id).await
    }

    async fn closed_since(&self, since: Option<DateTime<Utc>>) -> Result<Vec<Issue>, StorageError> {
        diagnostics::closed_since(self.read(), since).await
    }

    async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError> {
        diagnostics::orphan_candidates(self.read()).await
    }
}

#[cfg(test)]
mod tests {
    use super::{BUSY_TIMEOUT_MS, CHECKPOINT_EVERY_N_MUTATIONS, LibsqlStorage};
    use crate::{Storage, StorageError};

    /// `apply_pragmas` readback for the in-memory store: the native `busy_timeout` and foreign-key
    /// enforcement are live on **both** connections. (`SQLite` in-memory databases cannot use WAL —
    /// they always report `journal_mode = memory`; WAL is verified on the file path below.)
    #[tokio::test]
    async fn pragmas_readback_in_memory() {
        let storage = LibsqlStorage::open_in_memory().await.expect("open");

        for label in ["read", "write"] {
            let conn = if label == "read" {
                storage.read().clone()
            } else {
                storage.write().await.clone()
            };

            let mut rows = conn
                .query("PRAGMA busy_timeout", ())
                .await
                .expect("busy_timeout");
            let row = rows.next().await.expect("row").expect("present");
            let timeout = row.get_value(0).expect("val");
            assert_eq!(
                timeout.as_integer().copied(),
                Some(BUSY_TIMEOUT_MS.try_into().unwrap()),
                "{label} busy_timeout"
            );

            let mut rows = conn.query("PRAGMA foreign_keys", ()).await.expect("fk");
            let row = rows.next().await.expect("row").expect("present");
            let fk = row.get_value(0).expect("val");
            assert_eq!(fk.as_integer().copied(), Some(1), "{label} foreign_keys");
        }
    }

    /// WAL journal mode is live on a file-backed store (in-memory cannot use WAL).
    #[tokio::test]
    async fn wal_journal_mode_on_file() {
        let dir = std::env::temp_dir().join(format!(
            "unblock_wal_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("unblock.db");

        let storage = LibsqlStorage::open_local(&path).await.expect("open");
        let conn = storage.read().clone();
        let mut rows = conn
            .query("PRAGMA journal_mode", ())
            .await
            .expect("journal");
        let row = rows.next().await.expect("row").expect("present");
        let mode = row.get_value(0).expect("val");
        assert_eq!(mode.as_text().map(String::as_str), Some("wal"));

        drop(storage);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The periodic passive WAL checkpoint **bounds** the `-wal` sidecar (the mechanism the T0.8
    /// integration gate relies on but cannot reach from outside the crate, since the periodic
    /// checkpoint fires on the held write connection).
    ///
    /// With `wal_autocheckpoint = 0` and no checkpoint, the `-wal` file grows monotonically with every
    /// committed frame (it is never folded back into the main DB, so its space is never reused). A
    /// periodic **passive** checkpoint folds committed frames back so the file's space is reused
    /// in place — the file stops growing, staying *bounded* (PASSIVE reuses the WAL rather than
    /// truncating it, so the proof is "bounded", not "shrinks to zero"). The test writes the **same**
    /// batch twice over two fresh stores — once with the cadence OFF, once ON — and asserts the
    /// checkpointed run's `-wal` is materially smaller (bounded) than the unbounded run's, and that the
    /// passive-checkpoint counter advanced. (A single-writer store never contends, so the busy-retry
    /// witness must stay 0.)
    #[tokio::test]
    async fn passive_checkpoint_bounds_wal_sidecar() {
        use chrono::{TimeZone, Utc};
        use std::sync::atomic::Ordering;
        use unblock_model::Issue;

        // Drive one fresh file-backed store through `writes` creates with the given checkpoint
        // cadence; return the final `-wal` size and the store (so its counters can be inspected).
        async fn run(cadence: u64, writes: u32) -> (u64, u64, u64) {
            let dir = std::env::temp_dir().join(format!(
                "unblock_wal_bound_{cadence}_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir_all(&dir).expect("mkdir");
            let path = dir.join("unblock.db");
            let wal_path = dir.join("unblock.db-wal");

            let storage = LibsqlStorage::open_local(&path).await.expect("open");
            storage.migrate().await.expect("migrate");
            storage
                .instrument()
                .checkpoint_interval
                .store(cadence, Ordering::Relaxed);

            for n in 0..writes {
                let issue = Issue {
                    id: format!("ub-wal-{n}"),
                    title: format!("grow the wal {n}"),
                    created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                    updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                    ..Issue::default()
                };
                storage
                    .create_issue(&issue, "writer")
                    .await
                    .expect("create");
            }

            let wal = std::fs::metadata(&wal_path).map_or(0, |m| m.len());
            let checkpoints = storage.instrument().checkpoint_count();
            let busy = storage.instrument().busy_retry_count();
            drop(storage);
            let _ = std::fs::remove_dir_all(&dir);
            (wal, checkpoints, busy)
        }

        // Enough writes for several checkpoint intervals so the bound is visible.
        let writes = 600u32;
        let (unbounded_wal, off_checkpoints, off_busy) = run(0, writes).await;
        let (bounded_wal, on_checkpoints, on_busy) =
            run(CHECKPOINT_EVERY_N_MUTATIONS, writes).await;
        let expected_checkpoints = u64::from(writes) / CHECKPOINT_EVERY_N_MUTATIONS;

        assert_eq!(off_checkpoints, 0, "cadence off fires no checkpoint");
        assert!(
            on_checkpoints >= expected_checkpoints - 1,
            "cadence on must fire ~{expected_checkpoints} checkpoints (got {on_checkpoints})"
        );
        assert!(
            bounded_wal < unbounded_wal,
            "the periodic passive checkpoint must bound the -wal sidecar \
             (unbounded {unbounded_wal} bytes vs bounded {bounded_wal} bytes)"
        );
        // A single-writer store never contends: the busy-retry witness stays 0 on both runs.
        assert_eq!(
            off_busy, 0,
            "uncontended writer records no busy-retry (off)"
        );
        assert_eq!(on_busy, 0, "uncontended writer records no busy-retry (on)");
    }

    /// Opening twice (migrate run twice) is idempotent — the second `migrate` is a no-op.
    #[tokio::test]
    async fn migrate_is_idempotent() {
        let storage = LibsqlStorage::open_in_memory().await.expect("open");
        storage.migrate().await.expect("first migrate");
        storage.migrate().await.expect("second migrate (no-op)");
    }

    /// Foreign keys are enforced: an event for a non-existent issue is rejected.
    #[tokio::test]
    async fn foreign_keys_enforced() {
        let storage = LibsqlStorage::open_in_memory().await.expect("open");
        storage.migrate().await.expect("migrate");
        let conn = storage.write().await;
        let result = conn
            .execute(
                "INSERT INTO events (issue_id, event_type, actor) VALUES ('ub-missing', 'created', 'x')",
                (),
            )
            .await;
        assert!(result.is_err(), "FK violation should be rejected");
    }

    /// A write on the write connection is visible on the separate read connection (the shared-cache
    /// in-memory DB is genuinely shared between the two connections — the OQ-5 property).
    #[tokio::test]
    async fn write_visible_on_read_connection() {
        use chrono::{TimeZone, Utc};
        use unblock_model::Issue;

        let storage = LibsqlStorage::open_in_memory().await.expect("open");
        storage.migrate().await.expect("migrate");

        let issue = Issue {
            id: "ub-share1".to_string(),
            title: "shared".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        };
        storage
            .create_issue(&issue, "tester")
            .await
            .expect("create");

        // The read connection (distinct handle) sees the committed write.
        let fetched = storage.get_issue("ub-share1").await.expect("get");
        assert!(fetched.is_some(), "read conn must see the committed write");
    }

    /// Stress the `open_in_memory` + migrate + first-write path under heavy parallelism: 32 tasks
    /// each open an independent shared-cache in-memory store, migrate it, and write an issue. This is
    /// the regression guard for the `SQLite` shared-cache global-open race (T0.6 flake): concurrent
    /// `sqlite3_open_v2` on `cache=shared` URIs intermittently returned "bad parameter or other API
    /// misuse" until `open_in_memory` began serializing the open via `memory_open_lock` (T0.9). Every
    /// task must succeed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn open_in_memory_parallel_first_write_stress() {
        use chrono::{TimeZone, Utc};
        use unblock_model::Issue;

        let mut handles = Vec::new();
        for n in 0..32 {
            handles.push(tokio::spawn(async move {
                let storage = LibsqlStorage::open_in_memory().await?;
                storage.migrate().await?;
                let issue = Issue {
                    id: format!("ub-stress-{n}"),
                    title: format!("stress {n}"),
                    created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                    updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                    ..Issue::default()
                };
                storage.create_issue(&issue, "stress").await?;
                // Confirm the first write is visible through the read connection.
                let fetched = storage.get_issue(&format!("ub-stress-{n}")).await?;
                assert!(fetched.is_some(), "task {n}: first write must be visible");
                Ok::<(), StorageError>(())
            }));
        }

        for (n, handle) in handles.into_iter().enumerate() {
            handle
                .await
                .expect("join")
                .unwrap_or_else(|e| panic!("task {n} failed: {e:?}"));
        }
    }

    /// `PRAGMA table_info(issues)` column order is golden-pinned (the 38-column ordinal sequence).
    #[tokio::test]
    async fn issues_column_order_golden() {
        let storage = LibsqlStorage::open_in_memory().await.expect("open");
        storage.migrate().await.expect("migrate");
        let conn = storage.read();

        let mut rows = conn
            .query("PRAGMA table_info(issues)", ())
            .await
            .expect("table_info");
        let mut columns = Vec::new();
        while let Some(row) = rows.next().await.expect("row") {
            // table_info columns: cid, name, type, notnull, dflt_value, pk.
            if let libsql::Value::Text(name) = row.get_value(1).expect("name") {
                columns.push(name);
            }
        }
        assert_eq!(columns.len(), 38, "issues must have 38 columns");
        insta::assert_debug_snapshot!("issues_column_order", columns);
    }

    /// The `idx_%` index list is golden-pinned.
    #[tokio::test]
    async fn issue_index_list_golden() {
        let storage = LibsqlStorage::open_in_memory().await.expect("open");
        storage.migrate().await.expect("migrate");
        let conn = storage.read();

        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%' \
                 ORDER BY name ASC",
                (),
            )
            .await
            .expect("index list");
        let mut indexes = Vec::new();
        while let Some(row) = rows.next().await.expect("row") {
            if let libsql::Value::Text(name) = row.get_value(0).expect("name") {
                indexes.push(name);
            }
        }
        insta::assert_debug_snapshot!("issue_indexes", indexes);
    }

    /// The `issues` CHECK constraints reject an out-of-range priority and the closed-at invariant.
    #[tokio::test]
    async fn check_constraints_reject_bad_rows() {
        let storage = LibsqlStorage::open_in_memory().await.expect("open");
        storage.migrate().await.expect("migrate");
        let conn = storage.write().await;

        let bad_priority = conn
            .execute(
                "INSERT INTO issues (id, title, priority, created_at, updated_at) \
                 VALUES ('ub-bad1', 't', 9, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                (),
            )
            .await;
        assert!(bad_priority.is_err(), "priority 9 must violate the CHECK");

        let bad_closed = conn
            .execute(
                "INSERT INTO issues (id, title, status, created_at, updated_at) \
                 VALUES ('ub-bad2', 't', 'closed', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                (),
            )
            .await;
        assert!(
            bad_closed.is_err(),
            "closed without closed_at must violate the CHECK"
        );
    }

    /// `migrate` stamps `user_version = 1` on a fresh DB.
    #[tokio::test]
    async fn migrate_stamps_user_version_one() {
        let storage = LibsqlStorage::open_in_memory().await.expect("open");
        storage.migrate().await.expect("migrate");
        let conn = storage.read();
        let mut rows = conn.query("PRAGMA user_version", ()).await.expect("uv");
        let row = rows.next().await.expect("row").expect("present");
        assert_eq!(
            row.get_value(0).expect("val").as_integer().copied(),
            Some(1)
        );
    }

    /// A DB stamped at a future `user_version` is rejected with `SchemaMismatch`.
    #[tokio::test]
    async fn migrate_rejects_future_version() {
        let storage = LibsqlStorage::open_in_memory().await.expect("open");
        {
            let conn = storage.write().await;
            let _ = conn
                .query("PRAGMA user_version = 99", ())
                .await
                .expect("stamp");
        }
        let result = storage.migrate().await;
        assert!(matches!(
            result,
            Err(StorageError::SchemaMismatch {
                found: 99,
                expected: 1
            })
        ));
    }
}
