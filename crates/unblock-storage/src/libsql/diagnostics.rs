//! Pure-DB diagnostics support (FR-15) — **no git, no network** (NFR-6).
//!
//! `orphan_candidates` matches the commit-hash pattern in Rust/SQL; it never shells to git. There is
//! no `Command::new("git")` and no git crate anywhere in this crate.

use chrono::{DateTime, Utc};
use libsql::{Connection, Value};

use unblock_model::{GraphEdge, Issue};

use crate::error::{StorageError, map_libsql_err};

use super::crud::get_issue;
use super::deps::parse_dep_type;
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

/// Per-epic `parent-child` child rollup — bd's `get_epic_counts` ported 1:1 (`sqlite.rs:6978-7006`),
/// the ONE additive `stats` primitive (D26/T2.7).
///
/// A `parent-child` edge is stored `epic = depends_on_id`, `child = issue_id` (see `deps.rs` /
/// `query.rs` pass 3), so the JOIN `d.issue_id = i.id` hydrates the CHILD row. Over every
/// `parent-child` edge whose CHILD is non-template, this counts the child `total` and the children
/// whose `status IN ('closed','tombstone')`, grouped by the epic id (`d.depends_on_id`). The result
/// is **sorted by epic id in SQL** (`ORDER BY`) — deterministic (NFR-14), unlike bd's non-deterministic
/// `HashMap`. The epic-side active + non-template filter is applied IN-MEMORY by the engine (D26).
///
/// Pure-DB; never shells to git (NFR-6).
pub(super) async fn epic_child_rollup(
    conn: &Connection,
) -> Result<Vec<(String, (usize, usize))>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT d.depends_on_id AS epic, \
                    COUNT(*) AS total, \
                    SUM(CASE WHEN i.status IN ('closed', 'tombstone') THEN 1 ELSE 0 END) AS closed \
             FROM dependencies d \
             JOIN issues i ON d.issue_id = i.id \
             WHERE d.type = 'parent-child' \
               AND (i.is_template = 0 OR i.is_template IS NULL) \
             GROUP BY d.depends_on_id \
             ORDER BY d.depends_on_id ASC",
            (),
        )
        .await
        .map_err(map_libsql_err)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        let Value::Text(epic) = row.get_value(0).map_err(map_libsql_err)? else {
            continue;
        };
        let total = match row.get_value(1).map_err(map_libsql_err)? {
            Value::Integer(i) => usize::try_from(i).unwrap_or(0),
            _ => 0,
        };
        // `SUM(CASE …)` yields NULL for an all-zero group in some engines and an integer otherwise;
        // both map to a plain count (NULL → 0).
        let closed = match row.get_value(2).map_err(map_libsql_err)? {
            Value::Integer(i) => usize::try_from(i).unwrap_or(0),
            _ => 0,
        };
        out.push((epic, (total, closed)));
    }
    Ok(out)
}

/// Every stored dependency edge whose TARGET denotes **nothing** — the D45 `dangling` diagnostic's
/// ONE read (spine §3.2.1 `dangling`, as AMENDED 2026-08-02; trait contract in `trait_def.rs`).
///
/// ONE statement replaces the engine-side composition D45 first specified (whole-graph edge load
/// differenced against a fully-inclusive `list_issues` id set), which measured **10.72 s** at 250k
/// rows and took `Session::doctor()` to **16.31 s** against a 15 s boundedness guard.
///
/// # The join predicate is EXISTENCE ALONE — never status
///
/// `LEFT JOIN issues i ON i.id = d.depends_on_id` + `WHERE i.id IS NULL` selects exactly the edges
/// whose target has NO row. **Adding a status term to either clause** (`AND i.status NOT IN
/// ('closed','tombstone')` being the obvious one) **reintroduces the retired D45 defect through a new
/// door:** a closed / deferred / tombstoned blocker row EXISTS, so its edge is not dangling, and
/// reporting it makes the diagnostic fabricate its own findings. The corpus is therefore every row in
/// `issues`, deliberately WIDER than the export corpus — an edge into an ephemeral / `-wisp-` row is
/// NOT dangling, because the row exists.
///
/// # `NOT LIKE 'external:%'` is the SQL twin, not a second dialect
///
/// It is the same ASCII-case-insensitive predicate `unblock_model::is_external_target` implements
/// (spine §1.9 invariant 3): `SQLite`'s `LIKE` folds ASCII only and the prefix is pure ASCII, so the
/// two accept precisely the same strings — including on the non-ASCII near-misses both reject. The
/// NFR-16 contract suite's equivalence cell asks the DATABASE itself and keeps the halves honest.
///
/// # The ORDER is pinned HERE
///
/// `ORDER BY d.issue_id, d.type, d.depends_on_id` IS the finding order (NFR-14, snapshot-pinned).
/// The `type` column stores exactly `DependencyType::as_str()`, and `SQLite`'s default `BINARY`
/// collation compares those bytes exactly as Rust's `str` `Ord` does — so the caller forwards the
/// rows AS RETURNED and must NOT re-sort, since a redundant re-sort would mask a broken `ORDER BY`.
///
/// Pure-DB; never shells to git (NFR-6).
pub(super) async fn dangling_dependencies(
    conn: &Connection,
) -> Result<Vec<GraphEdge>, StorageError> {
    let mut rows = conn
        .query(
            "SELECT d.issue_id, d.depends_on_id, d.type \
             FROM dependencies d \
             LEFT JOIN issues i ON i.id = d.depends_on_id \
             WHERE i.id IS NULL \
               AND d.depends_on_id NOT LIKE 'external:%' \
             ORDER BY d.issue_id ASC, d.type ASC, d.depends_on_id ASC",
            (),
        )
        .await
        .map_err(map_libsql_err)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        let Value::Text(issue_id) = row.get_value(0).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(depends_on) = row.get_value(1).map_err(map_libsql_err)? else {
            continue;
        };
        let Value::Text(type_str) = row.get_value(2).map_err(map_libsql_err)? else {
            continue;
        };
        out.push(GraphEdge {
            from: issue_id,
            to: depends_on,
            // SHARED with the whole-graph loader: an unknown stored type is `Custom`, never a
            // fabricated gating `Blocks` (D5/GATE-NIT-4).
            dep_type: parse_dep_type(&type_str),
        });
    }
    Ok(out)
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
