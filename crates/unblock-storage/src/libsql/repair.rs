//! The online SQL repairs behind [`Storage::repair`](crate::Storage::repair) (D58 clause (5), spine
//! §3.2).
//!
//! In order, on the write connection the caller already holds: the `VACUUM INTO` evidence snapshot
//! outside any transaction, the optional D46 migration ladder, then every action in ONE `BEGIN
//! IMMEDIATE`. The snapshot is the only rewrite of the database file (D58 clause (3)).
//!
//! The snapshot must be a new path under `<db dir>/.recovery/`, confined by `lstat` and identity,
//! and it is left mode 0600 on unix.

use std::path::{Path, PathBuf};

use libsql::{Connection, Transaction, params};

use crate::error::{BackendOpaque, StorageError, map_libsql_err};
use crate::health_types::{RepairAction, RepairOutcome, RepairRequest};

use super::migrations::{current_user_version, run_migrations};
use super::recovery_fs::{confine, restrict_file};
use super::{WriteHook, with_immediate_tx};

/// Run `request` on the held write connection `conn`.
///
/// `db_path` is `None` for an in-memory store, which has no `.recovery/` and refuses every target.
pub(super) async fn repair(
    conn: &Connection,
    hook: WriteHook<'_>,
    db_path: Option<&Path>,
    request: &RepairRequest,
) -> Result<RepairOutcome, StorageError> {
    let snapshot = confine_snapshot(db_path, &request.snapshot)?;
    let Some(target) = snapshot.to_str() else {
        return Err(StorageError::RecoveryPathOutsideWorkspace {
            path: request.snapshot.clone(),
        });
    };

    // (1) The evidence snapshot. SQLite refuses `VACUUM INTO` inside a transaction, and the held
    // connection has none open.
    conn.execute("VACUUM INTO ?1", params![target])
        .await
        .map_err(map_libsql_err)?;
    let recovery_io = |source| StorageError::RecoveryIo {
        path: snapshot.clone(),
        source,
    };
    restrict_file(&snapshot).map_err(recovery_io)?;
    let snapshot_bytes = std::fs::metadata(&snapshot).map_err(recovery_io)?.len();

    // (2) The D46 ladder on THIS connection. `Storage::migrate` would wait on the write-connection
    // Mutex the caller already holds.
    let migrated = if request.migrate {
        let from = current_user_version(conn).await?;
        run_migrations(conn).await?;
        let to = current_user_version(conn).await?;
        Some((i64::from(from), i64::from(to)))
    } else {
        None
    };

    // (3) Every action in one transaction: a failure rolls them all back.
    if !request.actions.is_empty() {
        let actions = &request.actions;
        with_immediate_tx(conn, hook, |tx| async move {
            for action in actions {
                apply(&tx, action).await?;
            }
            Ok(((), tx))
        })
        .await?;
    }

    Ok(RepairOutcome {
        snapshot,
        snapshot_bytes,
        migrated,
        applied: request.actions.clone(),
    })
}

/// Apply one action inside the repair transaction.
async fn apply(tx: &Transaction, action: &RepairAction) -> Result<(), StorageError> {
    match action {
        RepairAction::Reindex { index } => {
            let mut rows = tx
                .query(
                    "SELECT 1 FROM sqlite_schema WHERE type = 'index' AND name = ?1",
                    params![index.as_str()],
                )
                .await
                .map_err(map_libsql_err)?;
            let known = rows.next().await.map_err(map_libsql_err)?.is_some();
            drop(rows);
            if !known {
                return Err(StorageError::Backend {
                    source: BackendOpaque::from_message(format!(
                        "REINDEX target is not an index in sqlite_schema: {index}"
                    )),
                });
            }
            let quoted = index.replace('"', "\"\"");
            tx.execute(&format!("REINDEX \"{quoted}\""), ())
                .await
                .map_err(map_libsql_err)?;
        }
        RepairAction::RaiseChildCounter { parent_id, to } => {
            tx.execute(
                "UPDATE child_counters SET last_child = MAX(last_child, ?2) WHERE parent_id = ?1",
                params![parent_id.as_str(), i64::from(*to)],
            )
            .await
            .map_err(map_libsql_err)?;
        }
        RepairAction::DedupeMetadataKey { key } => {
            tx.execute(
                "DELETE FROM metadata WHERE key = ?1 \
                 AND rowid < (SELECT MAX(rowid) FROM metadata WHERE key = ?1)",
                params![key.as_str()],
            )
            .await
            .map_err(map_libsql_err)?;
        }
    }
    Ok(())
}

/// Admit `snapshot` only when it is a NEW path under `<db dir>/.recovery/`, confined by `lstat` and
/// identity ([`confine`]). Returns it made absolute against the working directory.
fn confine_snapshot(db_path: Option<&Path>, snapshot: &Path) -> Result<PathBuf, StorageError> {
    let Some(db_path) = db_path else {
        return Err(StorageError::RecoveryPathOutsideWorkspace {
            path: snapshot.to_path_buf(),
        });
    };
    confine(db_path, snapshot)?;
    let target = std::path::absolute(snapshot).map_err(|source| StorageError::RecoveryIo {
        path: snapshot.to_path_buf(),
        source,
    })?;
    match std::fs::symlink_metadata(&target) {
        Ok(_) => Err(StorageError::RecoveryPathOutsideWorkspace {
            path: snapshot.to_path_buf(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(target),
        Err(source) => Err(StorageError::RecoveryIo {
            path: target,
            source,
        }),
    }
}
