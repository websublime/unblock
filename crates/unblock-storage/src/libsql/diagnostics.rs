//! Pure-DB diagnostics support (FR-15) — **no git, no network** (NFR-6).
//!
//! `orphan_candidates` matches the commit-hash pattern in Rust/SQL; it never shells to git. There is
//! no `Command::new("git")` and no git crate anywhere in this crate.

use chrono::{DateTime, Utc};
use libsql::{Connection, Value};

use unblock_model::Issue;

use crate::error::{StorageError, map_libsql_err};

use super::crud::get_issue;
use super::mappers::{ISSUE_COLUMNS, issue_from_row};

/// Run `PRAGMA integrity_check`, returning the raw problem rows.
///
/// A healthy database returns an empty `Vec` (the `"ok"` sentinel is normalized away).
pub(super) async fn integrity_check(conn: &Connection) -> Result<Vec<String>, StorageError> {
    let mut rows = conn
        .query("PRAGMA integrity_check", ())
        .await
        .map_err(map_libsql_err)?;
    let mut problems = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        if let Value::Text(message) = row.get_value(0).map_err(map_libsql_err)?
            && message != "ok"
        {
            problems.push(message);
        }
    }
    Ok(problems)
}

/// Return issues closed since `since` (or all closed issues when `since` is `None`), by `closed_at`.
///
/// Pure-DB changelog source; never shells to git (NFR-6).
pub(super) async fn closed_since(
    conn: &Connection,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<Issue>, StorageError> {
    let sql = format!(
        "SELECT {ISSUE_COLUMNS} FROM issues WHERE status = 'closed' AND closed_at IS NOT NULL{} \
         ORDER BY closed_at ASC, id ASC",
        if since.is_some() {
            " AND closed_at >= ?1"
        } else {
            ""
        }
    );

    let mut rows = if let Some(since) = since {
        conn.query(&sql, libsql::params![since.to_rfc3339()])
            .await
            .map_err(map_libsql_err)?
    } else {
        conn.query(&sql, ()).await.map_err(map_libsql_err)?
    };

    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        ids.push(issue_from_row(&row)?.id);
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Some(issue) = get_issue(conn, id).await? {
            out.push(issue);
        }
    }
    Ok(out)
}

/// Return orphan candidates: issues whose `external_ref` looks like a git commit hash (7–40 lowercase
/// hex chars). The hex shape is checked in Rust — **never** by invoking git or the network (NFR-6).
pub(super) async fn orphan_candidates(conn: &Connection) -> Result<Vec<Issue>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT id, external_ref FROM issues WHERE external_ref IS NOT NULL AND external_ref != '' \
             ORDER BY id ASC",
            (),
        )
        .await
        .map_err(map_libsql_err)?;

    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        let Value::Text(id) = row.get_value(0).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(external_ref) = row.get_value(1).map_err(map_libsql_err)? else {
            continue;
        };
        if looks_like_commit_hash(&external_ref) {
            ids.push(id);
        }
    }

    let mut out = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Some(issue) = get_issue(conn, id).await? {
            out.push(issue);
        }
    }
    Ok(out)
}

/// Whether `value` is a plausible git commit hash: 7–40 lowercase hex characters.
fn looks_like_commit_hash(value: &str) -> bool {
    let len = value.len();
    (7..=40).contains(&len)
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::looks_like_commit_hash;

    #[test]
    fn commit_hash_pattern() {
        assert!(looks_like_commit_hash("a1b2c3d"));
        assert!(looks_like_commit_hash(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(!looks_like_commit_hash("short")); // 5 chars
        assert!(!looks_like_commit_hash("A1B2C3D")); // uppercase
        assert!(!looks_like_commit_hash("g1b2c3d")); // 'g' not hex
        assert!(!looks_like_commit_hash("jira-123")); // non-hex
        assert!(!looks_like_commit_hash(
            "0123456789abcdef0123456789abcdef012345678" // 41 chars
        ));
    }
}
