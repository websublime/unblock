//! [`CloseOnceConnection`]: a libsql connection whose `sqlite3` handle is closed exactly **once**
//! (ub-q1u; works around upstream tursodatabase/libsql#2251, open against the pinned 0.9.30).
//!
//! # The defect
//!
//! When the last `libsql::Connection` to a local database drops, libsql 0.9.30 calls
//! `sqlite3_close_v2` on the handle **twice**. `LibsqlConnection`'s `Drop` calls
//! `local::Connection::disconnect`, and then the inner `local::Connection`'s own `Drop` calls
//! `disconnect` again. `disconnect` only checks `Arc::get_mut(drop_ref)`. That check passes both
//! times, so the second call closes a handle that the first call already freed. `SQLite` requires
//! the handle passed to `sqlite3_close_v2` to be "not previously closed".
//!
//! On its own the second call reads freed memory, finds a closed handle, returns `SQLITE_MISUSE`
//! and nobody notices. When other connections are opening at the same time, it becomes a
//! use-after-free across connections. Between the two calls the allocator can hand the freed block
//! to another thread's `sqlite3_open_v2`, and the stale second close then closes **that**
//! connection. Its owner's next call fails with "bad parameter or other API misuse". This was the
//! ub-q1u / ub-lp9.30 flake. It clusters at store open because a fresh open is what reuses the
//! block. A later reuse can also leave one store's handle pointing at another store's database, so
//! a committed write is missing from the store that made it. File-backed and in-memory stores are
//! equally affected. The shared cache plays no part.
//!
//! # The guard
//!
//! A prepared statement holds its own clone of the inner `local::Connection`. While the statement
//! is alive, `disconnect` sees a second owner, so neither `Drop` of the last `libsql::Connection`
//! closes anything. When the statement drops afterwards, the inner connection's `Drop` runs once.
//! That is a single `sqlite3_close_v2`, which leaves a zombie because the statement is not yet
//! finalized. Finalizing the statement then frees the zombie. The statement is never stepped, so it
//! takes no lock and opens no read transaction.
//!
//! Drop order is the whole mechanism. Struct fields drop in declaration order, so `conn` MUST stay
//! declared before `_close_guard`. Every libsql connection the backend opens goes through
//! [`CloseOnceConnection::connect`].

use std::ops::Deref;

use libsql::{Connection, Database, Statement};

use crate::error::{StorageError, map_libsql_err};

/// A libsql connection paired with the statement that makes its teardown close the handle exactly
/// once (see the module docs). Derefs to the [`Connection`].
pub(crate) struct CloseOnceConnection {
    /// The connection. Declared FIRST so it drops before `_close_guard`.
    conn: Connection,
    /// Prepared, never stepped. It keeps the handle open past `conn`'s double-closing `Drop`, then
    /// closes it once when it drops. Declared LAST.
    _close_guard: Statement,
}

impl CloseOnceConnection {
    /// Open a new connection to `db` and pair it with its close guard.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Backend`] if the connection cannot be opened or the guard statement
    /// cannot be prepared. In the second case the unguarded `conn` drops on the error path. That is
    /// a `SELECT 1` prepare on a handle opened a moment earlier, so it fails only on allocation
    /// failure.
    pub(super) async fn connect(db: &Database) -> Result<Self, StorageError> {
        let conn = db.connect().map_err(map_libsql_err)?;
        let close_guard = conn.prepare("SELECT 1").await.map_err(map_libsql_err)?;
        Ok(Self {
            conn,
            _close_guard: close_guard,
        })
    }
}

impl Deref for CloseOnceConnection {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use libsql::Builder;

    use super::CloseOnceConnection;

    /// A process-unique shared-cache in-memory URI. `SQLite` destroys such a database when the last
    /// handle to it closes, so whether a table survives tells whether a handle is still open.
    fn unique_memory_uri() -> String {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        format!("file:close_once_{seq}?mode=memory&cache=shared")
    }

    /// Whether table `t` exists in the shared-cache database at `uri`, judged through a fresh,
    /// guarded connection. That connection keeps the database alive only while it is open.
    async fn table_exists(uri: &str) -> bool {
        let db = Builder::new_local(uri).build().await.expect("build");
        let probe = CloseOnceConnection::connect(&db).await.expect("probe");
        let mut rows = probe
            .query(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 't'",
                (),
            )
            .await
            .expect("query sqlite_master");
        let row = rows.next().await.expect("step").expect("one row");
        row.get::<i64>(0).expect("count") == 1
    }

    /// The handle stays open after the connection drops, for as long as the guard lives. That is
    /// what defuses libsql's double close, because neither of the connection's two `Drop` calls
    /// closes anything. Dropping the guard then really closes the handle, so the guard does not leak
    /// it. If a libsql upgrade stops a prepared statement from holding its connection open, the first
    /// assertion fails, and that means the guard no longer guards.
    #[tokio::test]
    async fn guard_defers_the_close_past_the_connection_then_closes_it() {
        let uri = unique_memory_uri();
        let db = Builder::new_local(&uri).build().await.expect("build");
        let guarded = CloseOnceConnection::connect(&db).await.expect("connect");
        guarded
            .execute("CREATE TABLE t (x INTEGER)", ())
            .await
            .expect("create table");

        let CloseOnceConnection {
            conn,
            _close_guard: close_guard,
        } = guarded;
        drop(conn);
        assert!(
            table_exists(&uri).await,
            "with the guard alive, dropping the connection must not close the handle",
        );

        drop(close_guard);
        assert!(
            !table_exists(&uri).await,
            "dropping the guard must close the handle (the in-memory database is gone)",
        );
    }
}
