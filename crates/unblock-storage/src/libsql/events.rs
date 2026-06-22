//! Audit-event read + the in-transaction event writer used by every mutation (crate plan §3.3).
//!
//! Events are **append-only**: there is no update/delete path. The Tier-1 attribution columns
//! (`agent_name`/`harness`/`model`) are capture-only and never enforced — every writer here passes
//! `NULL` for them in v1 (the attribution capture surface is wired at the L7 boundary later).

use chrono::Utc;
use libsql::{Connection, Value};

use unblock_model::{Event, EventType};

use crate::error::{StorageError, map_libsql_err};

use super::mappers::event_from_row;

/// Append one audit event inside the caller's transaction (rows + audit commit together, FR-9).
///
/// `old_value`/`new_value`/`comment` are optional change context. The `created_at` is set to the
/// current instant in RFC3339 (the same convention the issue rows use). Tier-1 attribution columns
/// are left `NULL` (capture-only; wired later).
pub(super) async fn append_event_in_tx(
    tx: &libsql::Transaction,
    issue_id: &str,
    event_type: &EventType,
    actor: &str,
    old_value: Option<&str>,
    new_value: Option<&str>,
    comment: Option<&str>,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO events (issue_id, event_type, actor, old_value, new_value, comment, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        libsql::params![
            issue_id,
            event_type.as_str(),
            actor,
            opt(old_value),
            opt(new_value),
            opt(comment),
            Utc::now().to_rfc3339(),
        ],
    )
    .await
    .map_err(map_libsql_err)?;
    Ok(())
}

/// `Some(_)` → text; `None` → SQL `NULL`.
fn opt(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |s| Value::Text(s.to_string()))
}

/// List the append-only audit events for `issue_id`, **oldest first** (the trait contract — a
/// deliberate divergence from the original's newest-first `get_events`).
pub(super) async fn list_events(
    conn: &Connection,
    issue_id: &str,
) -> Result<Vec<Event>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT id, issue_id, event_type, actor, old_value, new_value, comment, created_at, \
             agent_name, harness, model \
             FROM events WHERE issue_id = ?1 ORDER BY created_at ASC, id ASC",
            libsql::params![issue_id],
        )
        .await
        .map_err(map_libsql_err)?;

    let mut events = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        events.push(event_from_row(&row)?);
    }
    Ok(events)
}
