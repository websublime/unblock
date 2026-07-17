//! Comments — add / list / update (edit) / delete (soft-redact) (FR-6, D37; spine §3.2.1).
//!
//! Every mutation runs inside ONE [`with_immediate_tx`] so the comment row and its audit `Event`
//! commit together (FR-9), and every mutation bumps the owning `issues.updated_at` (FORK-S1 — it
//! feeds the `stale` read path). None of them touches `issues.content_hash`: comments are excluded
//! from the frozen hash field set (spine §1.8), so an add/edit/redact NEVER moves it and FR-26
//! import idempotency stays intact.
//!
//! ## The two INSERT paths are distinct (MUST-1 SCOPE — spine §3.2.1)
//!
//! [`add_comment`] MINTS a new comment: `created_at = now`, `updated_at` left NULL. **Only**
//! [`update_comment`] ever sets `updated_at` (MUST-1). That rule is scoped to this file's `add`;
//! the create/bulk/import seed INSERT (`crud::insert_issue_in_tx`) REPLAYS an existing comment and
//! binds both `updated_at` and `redacted_at` verbatim from the `Comment` it is given.

use chrono::Utc;
use libsql::Connection;

use unblock_model::{Comment, EventType};

use crate::error::{StorageError, map_libsql_err};

use super::events::append_event_in_tx;
use super::mappers::comment_from_row;
use super::{WriteHook, with_immediate_tx};

/// The `comments` projection — the POSITIONAL contract `comment_from_row` reads (ordinals 0..=6).
const COMMENT_COLUMNS: &str = "id, issue_id, author, text, created_at, updated_at, redacted_at";

/// Add a comment + `Event(Commented)` (spine §3.2.1).
///
/// **Existence guard first (FORK-3):** the target issue must exist and must not be tombstoned →
/// else [`StorageError::IssueNotFound`]. A CLOSED issue is allowed (post-mortem commentary).
///
/// `updated_at` is left NULL — this path is create-time only (MUST-1).
pub(super) async fn add_comment(
    conn: &Connection,
    hook: WriteHook<'_>,
    issue_id: &str,
    author: &str,
    body: &str,
    actor: &str,
) -> Result<Comment, StorageError> {
    let issue_id = issue_id.to_string();
    let author = author.to_string();
    let body = body.to_string();
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        // FORK-3 existence guard: absent OR tombstoned → IssueNotFound (a CLOSED issue passes).
        let mut rows = tx
            .query(
                "SELECT status FROM issues WHERE id = ?1",
                libsql::params![issue_id.as_str()],
            )
            .await
            .map_err(map_libsql_err)?;
        let found = match rows.next().await.map_err(map_libsql_err)? {
            Some(row) => {
                let status = match row.get_value(0).map_err(map_libsql_err)? {
                    libsql::Value::Text(status) => status,
                    _ => String::new(),
                };
                status != "tombstone"
            }
            None => false,
        };
        drop(rows);
        if !found {
            return Err(StorageError::IssueNotFound { id: issue_id });
        }

        let now = Utc::now();
        tx.execute(
            "INSERT INTO comments (issue_id, author, text, created_at) VALUES (?1, ?2, ?3, ?4)",
            libsql::params![
                issue_id.as_str(),
                author.as_str(),
                body.as_str(),
                now.to_rfc3339(),
            ],
        )
        .await
        .map_err(map_libsql_err)?;
        let comment_id = tx.last_insert_rowid();

        append_event_in_tx(
            &tx,
            &issue_id,
            &EventType::Commented,
            &actor,
            None,
            None,
            Some(&body),
        )
        .await?;

        bump_issue_updated_at(&tx, &issue_id, now).await?;

        let comment = select_comment_in_tx(&tx, comment_id).await?;
        Ok((comment, tx))
    })
    .await
}

/// List the comments on `issue_id` in canonical order (`created_at ASC, id ASC`).
///
/// Reads on the read connection (no write permit).
pub(super) async fn list_comments(
    conn: &Connection,
    issue_id: &str,
) -> Result<Vec<Comment>, StorageError> {
    let sql = format!(
        "SELECT {COMMENT_COLUMNS} FROM comments WHERE issue_id = ?1 ORDER BY created_at ASC, id ASC"
    );
    let mut rows = conn
        .query(&sql, libsql::params![issue_id])
        .await
        .map_err(map_libsql_err)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        out.push(comment_from_row(&row)?);
    }
    Ok(out)
}

/// Update a comment's body, **preserving provenance** (D-D; spine §3.2.1).
///
/// The `updated_at` bump and the `Event(CommentEdited)` (carrying old + new bodies) ARE the
/// provenance — an in-place replace without them is forbidden.
pub(super) async fn update_comment(
    conn: &Connection,
    hook: WriteHook<'_>,
    comment_id: i64,
    body: &str,
    actor: &str,
) -> Result<Comment, StorageError> {
    let body = body.to_string();
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        let existing = select_comment_in_tx_opt(&tx, comment_id)
            .await?
            .ok_or(StorageError::CommentNotFound { id: comment_id })?;

        let now = Utc::now();
        tx.execute(
            "UPDATE comments SET text = ?1, updated_at = ?2 WHERE id = ?3",
            libsql::params![body.as_str(), now.to_rfc3339(), comment_id],
        )
        .await
        .map_err(map_libsql_err)?;

        append_event_in_tx(
            &tx,
            &existing.issue_id,
            &EventType::CommentEdited,
            &actor,
            Some(&existing.body),
            Some(&body),
            None,
        )
        .await?;

        bump_issue_updated_at(&tx, &existing.issue_id, now).await?;

        let comment = select_comment_in_tx(&tx, comment_id).await?;
        Ok((comment, tx))
    })
    .await
}

/// **Soft-redact** a comment (D-E; spine §3.2.1) — the single deletion op, never a hard delete.
///
/// KEEPS the row, masks `text` to `""`, sets `redacted_at = now`, and writes
/// `Event(CommentRedacted)` RETAINING the original body (provenance — FORK-redact-wire).
/// Idempotent: an already-redacted comment is returned unchanged, with no new event and no
/// `updated_at` bump (mirroring `restore_issue`'s already-active no-op).
pub(super) async fn delete_comment(
    conn: &Connection,
    hook: WriteHook<'_>,
    comment_id: i64,
    actor: &str,
) -> Result<Comment, StorageError> {
    let actor = actor.to_string();

    with_immediate_tx(conn, hook, |tx| async move {
        let existing = select_comment_in_tx_opt(&tx, comment_id)
            .await?
            .ok_or(StorageError::CommentNotFound { id: comment_id })?;

        // Already redacted → idempotent no-op.
        if existing.redacted_at.is_some() {
            return Ok((existing, tx));
        }

        let now = Utc::now();
        tx.execute(
            "UPDATE comments SET redacted_at = ?1, text = '' WHERE id = ?2",
            libsql::params![now.to_rfc3339(), comment_id],
        )
        .await
        .map_err(map_libsql_err)?;

        append_event_in_tx(
            &tx,
            &existing.issue_id,
            &EventType::CommentRedacted,
            &actor,
            Some(&existing.body),
            None,
            None,
        )
        .await?;

        bump_issue_updated_at(&tx, &existing.issue_id, now).await?;

        let comment = select_comment_in_tx(&tx, comment_id).await?;
        Ok((comment, tx))
    })
    .await
}

/// Bump the owning issue's `updated_at` (FORK-S1 — feeds `stale`; NOT part of `content_hash`, so
/// the hash column is deliberately left untouched, spine §1.8 / FR-26).
async fn bump_issue_updated_at(
    tx: &libsql::Transaction,
    issue_id: &str,
    now: chrono::DateTime<Utc>,
) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE issues SET updated_at = ?1 WHERE id = ?2",
        libsql::params![now.to_rfc3339(), issue_id],
    )
    .await
    .map_err(map_libsql_err)?;
    Ok(())
}

/// Re-`SELECT` a comment by id inside the tx, or `None` if the row is absent.
async fn select_comment_in_tx_opt(
    tx: &libsql::Transaction,
    comment_id: i64,
) -> Result<Option<Comment>, StorageError> {
    let sql = format!("SELECT {COMMENT_COLUMNS} FROM comments WHERE id = ?1");
    let mut rows = tx
        .query(&sql, libsql::params![comment_id])
        .await
        .map_err(map_libsql_err)?;
    match rows.next().await.map_err(map_libsql_err)? {
        Some(row) => Ok(Some(comment_from_row(&row)?)),
        None => Ok(None),
    }
}

/// Re-`SELECT` a comment that MUST exist (the row was just written in this tx).
async fn select_comment_in_tx(
    tx: &libsql::Transaction,
    comment_id: i64,
) -> Result<Comment, StorageError> {
    select_comment_in_tx_opt(tx, comment_id)
        .await?
        .ok_or(StorageError::CommentNotFound { id: comment_id })
}
