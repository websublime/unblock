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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use libsql::{Builder, Connection, Database, TransactionBehavior};
use tokio::sync::Mutex;

use unblock_model::{
    CountBucket, CountGroupBy, DepTree, Dependency, DependencyType, Event, Issue, ListFilters,
};

use crate::error::{StorageError, map_libsql_err};
use crate::filters::{DeletePlan, IssuePatch};
use crate::trait_def::Storage;

/// Native `busy_timeout`, in milliseconds (spine §3.3, OQ-2 RESOLVED).
///
/// Sleep-based and non-spinning — the sanctioned inverse of beads's `busy_timeout = 0` + backoff.
pub(crate) const BUSY_TIMEOUT_MS: u64 = 5000;

/// Monotonic counter giving each `open_in_memory()` a unique shared-cache name, so two in-memory
/// stores never collide on the process-global `SQLite` shared cache.
static MEMORY_DB_SEQ: AtomicU64 = AtomicU64::new(0);

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
        Self::from_database(db, true).await
    }

    /// Open a fresh, process-unique in-memory libsql database shared by the write and read
    /// connections (named shared-cache URI; see the module docs).
    ///
    /// The in-memory store uses **shared-cache, NOT WAL**: a `SQLite` in-memory database cannot use
    /// WAL (it always reports `journal_mode = memory`), so the WAL/`wal_autocheckpoint` pragmas are
    /// skipped on this path. (Asserting WAL there is both a no-op and an intermittent
    /// "API misuse"/`DatabaseLocked` flake source.) Real WAL + `busy_timeout` concurrency is validated
    /// by the **T0.8 contention lab on a file DB**, not here.
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
        let db = Builder::new_local(&uri)
            .build()
            .await
            .map_err(map_libsql_err)?;
        // In-memory: shared-cache, NOT WAL (see the method docs + `apply_pragmas`).
        Self::from_database(db, false).await
    }

    /// Build the two connections from an opened `Database` and apply the runtime pragmas to each.
    ///
    /// `file_backed` selects whether the WAL-only pragmas are applied (only file databases can use
    /// WAL; a shared-cache `:memory:` DB reports `journal_mode = memory` regardless).
    async fn from_database(db: Database, file_backed: bool) -> Result<Self, StorageError> {
        let write_conn = db.connect().map_err(map_libsql_err)?;
        let read_conn = db.connect().map_err(map_libsql_err)?;
        apply_pragmas(&write_conn, file_backed).await?;
        apply_pragmas(&read_conn, file_backed).await?;
        Ok(Self {
            _db: db,
            write_conn: Mutex::new(write_conn),
            read_conn,
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
/// `journal_mode = memory` — so asserting WAL there is a no-op AND an intermittent flake source
/// (under parallel shared-cache opens libsql can return "bad parameter or other API misuse" /
/// `DatabaseLocked` from the `PRAGMA journal_mode = WAL` on the in-memory path). Skipping it removes
/// the flake; the in-memory store relies on shared-cache + the native `busy_timeout`, and real WAL
/// concurrency is validated by the T0.8 contention lab on a file DB.
async fn apply_pragmas(conn: &Connection, file_backed: bool) -> Result<(), StorageError> {
    // Native, sleep-based busy handler (NFR-3). Set first so any subsequent switch can wait rather
    // than fail under a concurrent open.
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
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
/// `Err` (mutating-transaction helper, spine §3.3).
///
/// The closure returns its own `Result`; a transaction-open or commit failure is mapped through
/// [`map_libsql_err`]. On `Err` the transaction is rolled back (a rollback failure is swallowed —
/// the original error is the one worth surfacing; an uncommitted libsql `Transaction` also rolls
/// back on drop).
pub(super) async fn with_immediate_tx<F, Fut, T>(
    conn: &Connection,
    op: F,
) -> Result<T, StorageError>
where
    F: FnOnce(libsql::Transaction) -> Fut,
    Fut: std::future::Future<Output = Result<(T, libsql::Transaction), StorageError>>,
{
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await
        .map_err(map_libsql_err)?;
    match op(tx).await {
        Ok((value, tx)) => {
            tx.commit().await.map_err(map_libsql_err)?;
            Ok(value)
        }
        Err(err) => Err(err),
    }
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
        crud::create_issue(&conn, issue, actor).await
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
        crud::update_issue(&conn, id, patch, actor).await
    }

    async fn delete_issue(
        &self,
        plan: &DeletePlan,
        actor: &str,
    ) -> Result<DeletePlan, StorageError> {
        let conn = self.write().await;
        crud::delete_issue(&conn, plan, actor).await
    }

    async fn claim_issue(
        &self,
        id: &str,
        assignee: &str,
        actor: &str,
    ) -> Result<Issue, StorageError> {
        let conn = self.write().await;
        mutate::claim_issue(&conn, id, assignee, actor).await
    }

    async fn defer_issue(
        &self,
        id: &str,
        until: DateTime<Utc>,
        actor: &str,
    ) -> Result<Issue, StorageError> {
        let conn = self.write().await;
        mutate::defer_issue(&conn, id, until, actor).await
    }

    async fn undefer_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError> {
        let conn = self.write().await;
        mutate::undefer_issue(&conn, id, actor).await
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
        deps::add_dependency(&conn, dep, actor).await
    }

    async fn remove_dependency(
        &self,
        issue_id: &str,
        depends_on_id: &str,
        dep_type: &DependencyType,
        actor: &str,
    ) -> Result<(), StorageError> {
        let conn = self.write().await;
        deps::remove_dependency(&conn, issue_id, depends_on_id, dep_type, actor).await
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

    async fn detect_cycles(&self) -> Result<Vec<Vec<String>>, StorageError> {
        deps::detect_cycles(self.read()).await
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
    use super::{BUSY_TIMEOUT_MS, LibsqlStorage};
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
    /// each open an independent shared-cache in-memory store, migrate it, and write an issue. This
    /// pins the WAL-flake fix — before it, `PRAGMA journal_mode = WAL` on the `:memory:` path could
    /// intermittently return "API misuse" / `DatabaseLocked` under parallel opens. Every task must
    /// succeed.
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
