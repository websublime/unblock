//! `PRAGMA user_version`-based forward migrations (crate plan §3.3).
//!
//! v1 has a single schema version: `MIGRATIONS` is empty, so `run_migrations` only ever
//! **bootstraps** a fresh database (apply `SCHEMA_SQL`, stamp `user_version`, truncate-checkpoint the
//! WAL) or **no-ops** an already-current one. A database stamped at a version **greater** than this
//! build's [`CURRENT_SCHEMA_VERSION`] is rejected with [`StorageError::SchemaMismatch`]. Forward
//! steps (each a `{ version, up_sql }` applied in its own tx) are added additively from v1.1 on; the
//! invariant is never to edit an applied step in place (forward migrations only).

use libsql::Connection;

use crate::error::{StorageError, map_libsql_err};

use super::schema::{CURRENT_SCHEMA_VERSION, SCHEMA_SQL};

/// One forward migration step: bring a database from `version - 1` to `version` by running `up_sql`.
///
/// Empty in v1 (the baseline schema is the only version). Kept as a typed seam so v1.1+ steps are
/// purely additive.
#[allow(dead_code)]
pub(crate) struct Migration {
    /// The `user_version` this step stamps on success.
    pub(crate) version: i32,
    /// The forward DDL applied by this step.
    pub(crate) up_sql: &'static str,
}

/// The ordered forward steps. **Empty in v1** — a fresh database is bootstrapped directly from
/// `SCHEMA_SQL`; v1.0.0 is the first shipped schema, so there is no prior on-disk `user_version` to
/// migrate from.
pub(crate) const MIGRATIONS: &[Migration] = &[];

/// Read the database's `PRAGMA user_version`.
///
/// A fresh, unstamped database reports `0`.
pub(crate) async fn current_user_version(conn: &Connection) -> Result<i32, StorageError> {
    let mut rows = conn
        .query("PRAGMA user_version", ())
        .await
        .map_err(map_libsql_err)?;
    let row = rows
        .next()
        .await
        .map_err(map_libsql_err)?
        .ok_or(StorageError::NotInitialized)?;
    let value = row.get_value(0).map_err(map_libsql_err)?;
    Ok(value
        .as_integer()
        .copied()
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0))
}

/// Bring the database at `conn` to [`CURRENT_SCHEMA_VERSION`].
///
/// - A **fresh** database (`user_version == 0`) is bootstrapped: `SCHEMA_SQL` is applied, the version
///   is stamped, and the WAL is truncate-checkpointed once.
/// - An **already-current** database is a no-op (idempotent re-migrate).
/// - A database at a version **between** `0` and current runs the ordered [`MIGRATIONS`] steps
///   (none in v1).
/// - A database at a version **greater** than current is rejected with
///   [`StorageError::SchemaMismatch`].
pub(crate) async fn run_migrations(conn: &Connection) -> Result<(), StorageError> {
    let found = current_user_version(conn).await?;

    if found > CURRENT_SCHEMA_VERSION {
        return Err(StorageError::SchemaMismatch {
            found,
            expected: CURRENT_SCHEMA_VERSION,
        });
    }

    if found == CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    if found == 0 {
        bootstrap_fresh(conn).await?;
        return Ok(());
    }

    // Run any forward steps strictly greater than the found version (none in v1; additive seam).
    for step in MIGRATIONS.iter().filter(|m| m.version > found) {
        apply_step(conn, step).await?;
    }
    Ok(())
}

/// Bootstrap a fresh (`user_version == 0`) database: apply the canonical schema, stamp the version,
/// and truncate-checkpoint the WAL so the freshly written pages do not linger in the log.
async fn bootstrap_fresh(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(SCHEMA_SQL)
        .await
        .map_err(|e| StorageError::Migration {
            from: 0,
            to: CURRENT_SCHEMA_VERSION,
            reason: e.to_string(),
        })?;
    stamp_version(conn, CURRENT_SCHEMA_VERSION).await?;
    // Manual truncate checkpoint (wal_autocheckpoint is disabled — see `apply_pragmas`). A query is
    // used because `wal_checkpoint` returns a result row; a non-WAL DB simply yields no rows.
    let _ = conn.query("PRAGMA wal_checkpoint(TRUNCATE)", ()).await;
    Ok(())
}

/// Apply one forward [`Migration`] step in its own transaction and stamp the new version on success.
#[allow(dead_code)]
async fn apply_step(conn: &Connection, step: &Migration) -> Result<(), StorageError> {
    conn.execute_batch(step.up_sql)
        .await
        .map_err(|e| StorageError::Migration {
            from: step.version - 1,
            to: step.version,
            reason: e.to_string(),
        })?;
    stamp_version(conn, step.version).await
}

/// Stamp `PRAGMA user_version`. `PRAGMA` does not accept bound parameters, so the version (a
/// validated `i32` from this crate's own constants, never user input) is formatted inline.
async fn stamp_version(conn: &Connection, version: i32) -> Result<(), StorageError> {
    // `query` (not `execute`): the PRAGMA setter form may surface a row in libsql, which `execute`
    // rejects with `ExecuteReturnedRows`.
    let _ = conn
        .query(&format!("PRAGMA user_version = {version}"), ())
        .await
        .map_err(map_libsql_err)?;
    Ok(())
}
