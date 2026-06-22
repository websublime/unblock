//! Hierarchical-id child-counter maintenance (crate plan §3.3).
//!
//! The `Storage` trait receives an [`Issue`](unblock_model::Issue) whose `id` is already allocated
//! by the engine layer; storage's job for ids is to keep the `child_counters` table in step when a
//! **hierarchical** id (`ub-abc.1`, `ub-abc.1.2`) is inserted, so the next child number can be
//! handed out monotonically. libsql is real `SQLite`, so the counter is maintained with a genuine
//! **UPSERT** (`INSERT … ON CONFLICT … DO UPDATE`) — not the original's DELETE+INSERT *frankensqlite*
//! workaround.

use libsql::{Connection, Value};

use crate::error::{StorageError, map_libsql_err};

/// Escape `SQLite` `LIKE` metacharacters (`\`, `%`, `_`) so a literal value can be matched with
/// `LIKE ? ESCAPE '\'` without the value's own characters acting as wildcards.
pub(super) fn escape_like_pattern(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Record that `child_number` has been used under `parent_id`, advancing the stored counter when the
/// new child is greater (UPSERT; the counter is a high-water mark).
///
/// Run inside the caller's transaction.
pub(super) async fn update_child_counter_in_tx(
    tx: &libsql::Transaction,
    parent_id: &str,
    child_number: u32,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO child_counters (parent_id, last_child) VALUES (?1, ?2) \
         ON CONFLICT(parent_id) DO UPDATE SET last_child = MAX(last_child, excluded.last_child)",
        libsql::params![parent_id, i64::from(child_number)],
    )
    .await
    .map_err(map_libsql_err)?;
    Ok(())
}

/// Find the next available child number under `parent_id`.
///
/// Reads the `child_counters` high-water mark first (the source of truth); falls back to a
/// `LIKE`-escaped scan of existing `{parent_id}.N` ids for legacy data or a missing counter row.
/// Never panics: overflow saturates.
///
/// Reached by the gated `StorageTestkit::testkit_child_high_water` seam, which exercises the
/// counter's monotonic-advance contract from the NFR-16 suite; the engine's id allocator also
/// consumes it once that wiring lands. The `allow(dead_code)` only applies to the **plain** build
/// (no `testkit`, not under test), where the seam does not reach it yet.
#[cfg_attr(not(any(test, feature = "testkit")), allow(dead_code))]
pub(super) async fn next_child_number(
    conn: &Connection,
    parent_id: &str,
) -> Result<u32, StorageError> {
    // 1. Counter table (source of truth).
    let mut rows = conn
        .query(
            "SELECT last_child FROM child_counters WHERE parent_id = ?1",
            libsql::params![parent_id],
        )
        .await
        .map_err(map_libsql_err)?;
    if let Some(row) = rows.next().await.map_err(map_libsql_err)?
        && let Value::Integer(last) = row.get_value(0).map_err(map_libsql_err)?
    {
        let last = u32::try_from(last).unwrap_or(0);
        return Ok(last.saturating_add(1));
    }

    // 2. Legacy fallback: scan existing child ids.
    let pattern = format!("{}.%", escape_like_pattern(parent_id));
    let mut rows = conn
        .query(
            "SELECT id FROM issues WHERE id LIKE ?1 ESCAPE '\\'",
            libsql::params![pattern],
        )
        .await
        .map_err(map_libsql_err)?;

    let prefix_with_dot = format!("{parent_id}.");
    let mut max_child: u32 = 0;
    while let Some(row) = rows.next().await.map_err(map_libsql_err)? {
        if let Value::Text(id) = row.get_value(0).map_err(map_libsql_err)?
            && let Some(suffix) = id.strip_prefix(&prefix_with_dot)
        {
            // Only the direct child segment matters (`parent.1` and `parent.1.2` share `1`).
            if let Some(num) = suffix.split('.').next().and_then(|s| s.parse::<u32>().ok()) {
                max_child = max_child.max(num);
            }
        }
    }
    Ok(max_child.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::escape_like_pattern;

    #[test]
    fn escape_like_pattern_escapes_metachars() {
        assert_eq!(escape_like_pattern("ub-abc"), "ub-abc");
        assert_eq!(escape_like_pattern("a%b"), "a\\%b");
        assert_eq!(escape_like_pattern("a_b"), "a\\_b");
        assert_eq!(escape_like_pattern("a\\b"), "a\\\\b");
        // The backslash is escaped first so a later % escape is not double-counted.
        assert_eq!(escape_like_pattern("a\\%"), "a\\\\\\%");
    }
}
