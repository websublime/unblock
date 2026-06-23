//! Race-sensitive single-statement mutations: atomic claim + defer/undefer (crate plan §3.3,
//! spine §3.2.1). Each runs inside one `BEGIN IMMEDIATE` transaction with its audit event.

use chrono::{DateTime, Utc};
use libsql::{Connection, Value};

use unblock_model::{EventType, Issue};

use crate::error::{StorageError, map_libsql_err};

use super::crud::get_issue;
use super::events::append_event_in_tx;
use super::mappers::ISSUE_COLUMNS;
use super::{WriteHook, with_immediate_tx};

/// Atomically claim `id` for `assignee` (FR-2). The guard is **assignee-only** — there is no status
/// predicate (spine §3.2.1, sqlite.rs:2888-2935):
///
/// `UPDATE … WHERE id = ? AND (assignee IS NULL OR TRIM(assignee) = '' OR assignee = ?<actor>)`.
///
/// Three outcomes: unassigned → claim succeeds (sets assignee + `in_progress`, writes
/// `AssigneeChanged` + `StatusChanged`); a **same-actor re-claim** short-circuits **before** the
/// `UPDATE` (idempotent `Ok`, no event, no `updated_at` change); held by a different actor → 0 rows →
/// re-`SELECT` the holder in-tx → `AlreadyClaimed{by}`.
pub(super) async fn claim_issue(
    conn: &Connection,
    hook: WriteHook<'_>,
    id: &str,
    assignee: &str,
    actor: &str,
) -> Result<Issue, StorageError> {
    let id_owned = id.to_string();
    let assignee = assignee.to_string();
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        // Load the current row inside the tx.
        let sql = format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE id = ?1");
        let mut rows = tx
            .query(&sql, libsql::params![id_owned.as_str()])
            .await
            .map_err(map_libsql_err)?;
        let Some(row) = rows.next().await.map_err(map_libsql_err)? else {
            return Err(StorageError::IssueNotFound { id: id_owned });
        };
        let issue = super::mappers::issue_from_row(&row)?;
        drop(rows);

        // Same-actor re-claim: idempotent short-circuit BEFORE the UPDATE (no event, no updated_at).
        if issue.assignee.as_deref() == Some(assignee.as_str()) {
            return Ok((issue, tx));
        }

        let now = Utc::now();
        // The hash after the claim mutation (assignee + status both affect content_hash).
        let mut after = issue.clone();
        after.assignee = Some(assignee.clone());
        after.status = unblock_model::Status::InProgress;
        after.updated_at = now;
        let new_hash = after.compute_content_hash();

        // The atomic, assignee-ONLY compare-and-set.
        let changed = tx
            .execute(
                "UPDATE issues SET assignee = ?1, status = 'in_progress', updated_at = ?2, \
                 content_hash = ?3 WHERE id = ?4 \
                 AND (assignee IS NULL OR TRIM(assignee) = '' OR assignee = ?5)",
                libsql::params![
                    assignee.as_str(),
                    now.to_rfc3339(),
                    new_hash,
                    id_owned.as_str(),
                    assignee.as_str(),
                ],
            )
            .await
            .map_err(map_libsql_err)?;

        if changed == 0 {
            // Lost the race: re-read the current holder in the same tx.
            let holder = current_holder(&tx, &id_owned).await?;
            return Err(StorageError::AlreadyClaimed {
                id: id_owned,
                by: holder,
            });
        }

        // Won: AssigneeChanged + StatusChanged (in that order — the §3.2.1 oracle).
        append_event_in_tx(
            &tx,
            &id_owned,
            &EventType::AssigneeChanged,
            &actor,
            issue.assignee.as_deref(),
            Some(&assignee),
            None,
        )
        .await?;
        append_event_in_tx(
            &tx,
            &id_owned,
            &EventType::StatusChanged,
            &actor,
            Some(issue.status.as_str()),
            Some(unblock_model::Status::InProgress.as_str()),
            None,
        )
        .await?;

        Ok((after, tx))
    })
    .await?;

    get_issue(conn, id)
        .await?
        .ok_or_else(|| StorageError::IssueNotFound { id: id.to_string() })
}

/// Re-read the current holder of `id` within the tx, normalising NULL/blank to `<unknown>`.
async fn current_holder(tx: &libsql::Transaction, id: &str) -> Result<String, StorageError> {
    let mut rows = tx
        .query(
            "SELECT assignee FROM issues WHERE id = ?1",
            libsql::params![id],
        )
        .await
        .map_err(map_libsql_err)?;
    let Some(row) = rows.next().await.map_err(map_libsql_err)? else {
        return Ok("<unknown>".to_string());
    };
    let holder = match row.get_value(0).map_err(map_libsql_err)? {
        Value::Text(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => "<unknown>".to_string(),
    };
    Ok(holder)
}

/// Defer `id` until `until` (sets `defer_until`), writing `Event(Updated)` (spine §3.2.1).
pub(super) async fn defer_issue(
    conn: &Connection,
    hook: WriteHook<'_>,
    id: &str,
    until: DateTime<Utc>,
    actor: &str,
) -> Result<Issue, StorageError> {
    set_defer(conn, hook, id, Some(until), actor).await
}

/// Undefer `id` (clears `defer_until`), writing `Event(Updated)` (spine §3.2.1).
pub(super) async fn undefer_issue(
    conn: &Connection,
    hook: WriteHook<'_>,
    id: &str,
    actor: &str,
) -> Result<Issue, StorageError> {
    set_defer(conn, hook, id, None, actor).await
}

/// Shared set/clear of `defer_until` + `updated_at` + `content_hash` + `Event(Updated)`.
async fn set_defer(
    conn: &Connection,
    hook: WriteHook<'_>,
    id: &str,
    until: Option<DateTime<Utc>>,
    actor: &str,
) -> Result<Issue, StorageError> {
    let id_owned = id.to_string();
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        let sql = format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE id = ?1");
        let mut rows = tx
            .query(&sql, libsql::params![id_owned.as_str()])
            .await
            .map_err(map_libsql_err)?;
        let Some(row) = rows.next().await.map_err(map_libsql_err)? else {
            return Err(StorageError::IssueNotFound { id: id_owned });
        };
        let mut issue = super::mappers::issue_from_row(&row)?;
        drop(rows);

        let now = Utc::now();
        issue.defer_until = until;
        issue.updated_at = now;
        // `defer_until` is not part of content_hash, so the hash is unchanged — but `updated_at`
        // advances regardless (the defer/undefer mutation always touches the row).
        let new_hash = issue.compute_content_hash();

        tx.execute(
            "UPDATE issues SET defer_until = ?1, updated_at = ?2, content_hash = ?3 WHERE id = ?4",
            libsql::params![
                until.map_or(Value::Null, |d| Value::Text(d.to_rfc3339())),
                now.to_rfc3339(),
                new_hash,
                id_owned.as_str(),
            ],
        )
        .await
        .map_err(map_libsql_err)?;

        append_event_in_tx(
            &tx,
            &id_owned,
            &EventType::Updated,
            &actor,
            None,
            until.map(|d| d.to_rfc3339()).as_deref(),
            Some(if until.is_some() {
                "deferred"
            } else {
                "undeferred"
            }),
        )
        .await?;

        Ok((issue, tx))
    })
    .await?;

    get_issue(conn, id)
        .await?
        .ok_or_else(|| StorageError::IssueNotFound { id: id.to_string() })
}
