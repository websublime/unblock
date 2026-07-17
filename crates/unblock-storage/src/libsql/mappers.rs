//! Row ↔ domain mapping for the libsql backend (crate plan §3.3).
//!
//! - `content_hash` is **recomputed on load** (spine §1.8): the stored column is only a dedup cache,
//!   so [`issue_from_row`] never trusts it — it sets `content_hash = Some(issue.compute_content_hash())`.
//! - Empty-string text columns coalesce to `None` (matching the original `DEFAULT ''` columns);
//!   `None` binds to SQL `NULL` on the way back for nullable columns.
//! - Open-enum `Custom` variants round-trip through their `FromStr` (which never fails).
//! - Timestamps are RFC3339 on write; on read both RFC3339 and the `SQLite` default
//!   `YYYY-MM-DD HH:MM:SS` shapes are accepted.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use libsql::{Row, Value};

use unblock_model::{
    Comment, Dependency, DependencyType, Event, EventType, Issue, IssueType, Priority, Status,
};

use crate::error::StorageError;

/// The canonical `issues` column list, in schema order, for every full `SELECT`.
///
/// Kept as one constant so the read mapper's positional indices stay in lock-step with the projected
/// columns (the 38-column ordinal sequence is golden-pinned by the schema snapshot test).
pub(super) const ISSUE_COLUMNS: &str = "id, content_hash, title, description, design, \
    acceptance_criteria, notes, status, priority, issue_type, assignee, owner, estimated_minutes, \
    created_at, created_by, updated_at, closed_at, close_reason, closed_by_session, due_at, \
    defer_until, external_ref, source_system, source_repo, deleted_at, deleted_by, delete_reason, \
    original_type, compaction_level, compacted_at, compacted_at_commit, original_size, sender, \
    ephemeral, pinned, is_template, source_repo_path, agent_context";

/// Read the text at `idx` as `Option<String>`, treating SQL `NULL` as `None`.
fn opt_text(row: &Row, idx: i32) -> Result<Option<String>, StorageError> {
    let value = row.get_value(idx).map_err(crate::error::map_libsql_err)?;
    Ok(match value {
        Value::Text(s) => Some(s),
        _ => None,
    })
}

/// Read the text at `idx`, coalescing both `NULL` and the empty string to `None` (the `DEFAULT ''`
/// columns in the schema store `''` for "unset").
fn non_empty_text(row: &Row, idx: i32) -> Result<Option<String>, StorageError> {
    Ok(opt_text(row, idx)?.filter(|s| !s.is_empty()))
}

/// Read the required text at `idx`, defaulting `NULL` to the empty string.
fn req_text(row: &Row, idx: i32) -> Result<String, StorageError> {
    Ok(opt_text(row, idx)?.unwrap_or_default())
}

/// Read the integer at `idx` as `Option<i32>` (`NULL` → `None`).
fn opt_i32(row: &Row, idx: i32) -> Result<Option<i32>, StorageError> {
    let value = row.get_value(idx).map_err(crate::error::map_libsql_err)?;
    Ok(match value {
        Value::Integer(i) => i32::try_from(i).ok(),
        _ => None,
    })
}

/// Read the boolean flag at `idx` (`0`/`NULL` → `false`, non-zero → `true`).
fn flag(row: &Row, idx: i32) -> Result<bool, StorageError> {
    let value = row.get_value(idx).map_err(crate::error::map_libsql_err)?;
    Ok(matches!(value, Value::Integer(i) if i != 0))
}

/// Parse an optional timestamp column (`NULL`/empty → `None`).
fn opt_datetime(row: &Row, idx: i32) -> Result<Option<DateTime<Utc>>, StorageError> {
    match non_empty_text(row, idx)? {
        Some(text) => Ok(Some(parse_datetime(&text)?)),
        None => Ok(None),
    }
}

/// Parse a required timestamp column.
fn req_datetime(row: &Row, idx: i32) -> Result<DateTime<Utc>, StorageError> {
    let text = req_text(row, idx)?;
    parse_datetime(&text)
}

/// Parse a timestamp string (RFC3339 or the `SQLite` default `YYYY-MM-DD HH:MM:SS`).
///
/// A value that matches neither shape is a backend/corruption signal, surfaced as
/// [`StorageError::Backend`] (never a panic).
fn parse_datetime(text: &str) -> Result<DateTime<Utc>, StorageError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S") {
        return Ok(Utc.from_utc_datetime(&naive));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(Utc.from_utc_datetime(&naive));
    }
    Err(StorageError::Backend {
        source: crate::error::BackendOpaque::from_message(format!(
            "unparseable timestamp from storage: {text}"
        )),
    })
}

/// Map a full `issues` row (projected via [`ISSUE_COLUMNS`]) into an [`Issue`].
///
/// `content_hash` is recomputed from the loaded fields (spine §1.8); `labels`/`dependencies`/
/// `comments` are left empty for the caller to hydrate.
pub(super) fn issue_from_row(row: &Row) -> Result<Issue, StorageError> {
    // Open-enum parses are infallible (`FromStr` never errors for Status/IssueType), so a missing
    // string defaults to the model default.
    let status = non_empty_text(row, 7)?
        .map(|s| s.parse::<Status>().unwrap_or_default())
        .unwrap_or_default();
    let issue_type = non_empty_text(row, 9)?
        .map(|s| s.parse::<IssueType>().unwrap_or_default())
        .unwrap_or_default();
    let priority = Priority(opt_i32(row, 8)?.unwrap_or_else(|| Priority::default().0));

    let mut issue = Issue {
        id: req_text(row, 0)?,
        content_hash: None,
        title: req_text(row, 2)?,
        description: non_empty_text(row, 3)?,
        design: non_empty_text(row, 4)?,
        acceptance_criteria: non_empty_text(row, 5)?,
        notes: non_empty_text(row, 6)?,
        status,
        priority,
        issue_type,
        assignee: non_empty_text(row, 10)?,
        owner: non_empty_text(row, 11)?,
        estimated_minutes: opt_i32(row, 12)?,
        created_at: req_datetime(row, 13)?,
        created_by: non_empty_text(row, 14)?,
        updated_at: req_datetime(row, 15)?,
        closed_at: opt_datetime(row, 16)?,
        close_reason: non_empty_text(row, 17)?,
        closed_by_session: non_empty_text(row, 18)?,
        due_at: opt_datetime(row, 19)?,
        defer_until: opt_datetime(row, 20)?,
        external_ref: non_empty_text(row, 21)?,
        source_system: non_empty_text(row, 22)?,
        source_repo: non_empty_text(row, 23)?,
        deleted_at: opt_datetime(row, 24)?,
        deleted_by: non_empty_text(row, 25)?,
        delete_reason: non_empty_text(row, 26)?,
        original_type: non_empty_text(row, 27)?,
        compaction_level: opt_i32(row, 28)?,
        compacted_at: opt_datetime(row, 29)?,
        compacted_at_commit: non_empty_text(row, 30)?,
        original_size: opt_i32(row, 31)?,
        sender: non_empty_text(row, 32)?,
        ephemeral: flag(row, 33)?,
        pinned: flag(row, 34)?,
        is_template: flag(row, 35)?,
        source_repo_path: non_empty_text(row, 36)?,
        agent_context: non_empty_text(row, 37)?,
        labels: Vec::new(),
        dependencies: Vec::new(),
        comments: Vec::new(),
    };

    // Recompute the content hash from the loaded fields — never trust the stored column (spine §1.8).
    issue.content_hash = Some(issue.compute_content_hash());
    Ok(issue)
}

/// Bind an [`Issue`]'s columns for an `INSERT` matching the [`ISSUE_COLUMNS`] order, with
/// `content_hash` placed at position 1 (the caller computes and passes it so create/claim/delete can
/// re-derive it consistently).
///
/// Nullable columns bind `None` to SQL `NULL`; the `DEFAULT ''` text columns bind `''` for `None`
/// (matching the original's "empty string, not NULL, for bd compatibility" convention).
pub(super) fn bind_issue(issue: &Issue, content_hash: &str) -> Vec<Value> {
    vec![
        Value::Text(issue.id.clone()),
        Value::Text(content_hash.to_string()),
        Value::Text(issue.title.clone()),
        text_or_empty(issue.description.as_deref()),
        text_or_empty(issue.design.as_deref()),
        text_or_empty(issue.acceptance_criteria.as_deref()),
        text_or_empty(issue.notes.as_deref()),
        Value::Text(issue.status.as_str().to_string()),
        Value::Integer(i64::from(issue.priority.0)),
        Value::Text(issue.issue_type.as_str().to_string()),
        opt_text_value(issue.assignee.as_deref()),
        text_or_empty(issue.owner.as_deref()),
        opt_int_value(issue.estimated_minutes),
        Value::Text(issue.created_at.to_rfc3339()),
        text_or_empty(issue.created_by.as_deref()),
        Value::Text(issue.updated_at.to_rfc3339()),
        opt_datetime_value(issue.closed_at),
        text_or_empty(issue.close_reason.as_deref()),
        text_or_empty(issue.closed_by_session.as_deref()),
        opt_datetime_value(issue.due_at),
        opt_datetime_value(issue.defer_until),
        opt_text_value(issue.external_ref.as_deref()),
        text_or_empty(issue.source_system.as_deref()),
        text_or_empty(issue.source_repo.as_deref()),
        opt_datetime_value(issue.deleted_at),
        text_or_empty(issue.deleted_by.as_deref()),
        text_or_empty(issue.delete_reason.as_deref()),
        text_or_empty(issue.original_type.as_deref()),
        Value::Integer(i64::from(issue.compaction_level.unwrap_or(0))),
        opt_datetime_value(issue.compacted_at),
        opt_text_value(issue.compacted_at_commit.as_deref()),
        opt_int_value(issue.original_size),
        text_or_empty(issue.sender.as_deref()),
        Value::Integer(i64::from(issue.ephemeral)),
        Value::Integer(i64::from(issue.pinned)),
        Value::Integer(i64::from(issue.is_template)),
        opt_text_value(issue.source_repo_path.as_deref()),
        opt_text_value(issue.agent_context.as_deref()),
    ]
}

/// `Some(_)` → text; `None` → the empty string (`DEFAULT ''` columns).
fn text_or_empty(value: Option<&str>) -> Value {
    Value::Text(value.unwrap_or("").to_string())
}

/// `Some(_)` → text; `None` → SQL `NULL` (nullable columns).
fn opt_text_value(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |s| Value::Text(s.to_string()))
}

/// `Some(_)` → integer; `None` → SQL `NULL`.
fn opt_int_value(value: Option<i32>) -> Value {
    value.map_or(Value::Null, |i| Value::Integer(i64::from(i)))
}

/// `Some(_)` → RFC3339 text; `None` → SQL `NULL`.
fn opt_datetime_value(value: Option<DateTime<Utc>>) -> Value {
    value.map_or(Value::Null, |dt| Value::Text(dt.to_rfc3339()))
}

/// Map a `dependencies` row (`issue_id, depends_on_id, type, created_at, created_by, metadata,
/// thread_id`) into a [`Dependency`].
pub(super) fn dependency_from_row(row: &Row) -> Result<Dependency, StorageError> {
    let dep_type = req_text(row, 2)?
        .parse::<DependencyType>()
        .unwrap_or(DependencyType::Blocks);
    Ok(Dependency {
        issue_id: req_text(row, 0)?,
        depends_on_id: req_text(row, 1)?,
        dep_type,
        created_at: req_datetime(row, 3)?,
        created_by: non_empty_text(row, 4)?,
        metadata: non_empty_text(row, 5)?.filter(|m| m != "{}"),
        thread_id: non_empty_text(row, 6)?,
    })
}

/// Parse an event-type wire string into [`EventType`], mapping any unrecognised value to
/// [`EventType::Custom`] (the model open-enum tail). `EventType` has no `FromStr`, so the known wire
/// strings are matched directly (they are the model's `as_str()` outputs).
fn parse_event_type(value: &str) -> EventType {
    match value {
        "created" => EventType::Created,
        "updated" => EventType::Updated,
        "status_changed" => EventType::StatusChanged,
        "priority_changed" => EventType::PriorityChanged,
        "assignee_changed" => EventType::AssigneeChanged,
        "commented" => EventType::Commented,
        "closed" => EventType::Closed,
        "reopened" => EventType::Reopened,
        "dependency_added" => EventType::DependencyAdded,
        "dependency_removed" => EventType::DependencyRemoved,
        "label_added" => EventType::LabelAdded,
        "label_removed" => EventType::LabelRemoved,
        "compacted" => EventType::Compacted,
        "deleted" => EventType::Deleted,
        "restored" => EventType::Restored,
        // D37 — WITHOUT these two arms the catch-all below silently degrades a comment_edited /
        // comment_redacted row to Custom("comment_edited"), breaking the 17-named EventType oracle
        // with no compile error. Guarded by `parse_event_type_covers_all_seventeen_named` below.
        "comment_edited" => EventType::CommentEdited,
        "comment_redacted" => EventType::CommentRedacted,
        other => EventType::Custom(other.to_string()),
    }
}

/// Map an `events` row (`id, issue_id, event_type, actor, old_value, new_value, comment, created_at,
/// agent_name, harness, model`) into an [`Event`].
pub(super) fn event_from_row(row: &Row) -> Result<Event, StorageError> {
    let event_type_str = req_text(row, 2)?;
    let event_type = parse_event_type(&event_type_str);
    let id = match row.get_value(0).map_err(crate::error::map_libsql_err)? {
        Value::Integer(i) => i,
        _ => 0,
    };
    Ok(Event {
        id,
        issue_id: req_text(row, 1)?,
        event_type,
        actor: req_text(row, 3)?,
        old_value: opt_text(row, 4)?,
        new_value: opt_text(row, 5)?,
        comment: opt_text(row, 6)?,
        created_at: req_datetime(row, 7)?,
        agent_name: non_empty_text(row, 8)?,
        harness: non_empty_text(row, 9)?,
        model: non_empty_text(row, 10)?,
    })
}

/// Map a `comments` row (`id, issue_id, author, text, created_at, updated_at, redacted_at`) into a
/// [`Comment`].
///
/// The two D37 columns are read by POSITIONAL ordinal (5/6) — the `PRAGMA table_info(comments)`
/// column-order golden (`libsql/mod.rs`) is what guards that order.
pub(super) fn comment_from_row(row: &Row) -> Result<Comment, StorageError> {
    let id = match row.get_value(0).map_err(crate::error::map_libsql_err)? {
        Value::Integer(i) => i,
        _ => 0,
    };
    Ok(Comment {
        id,
        issue_id: req_text(row, 1)?,
        author: req_text(row, 2)?,
        body: req_text(row, 3)?,
        created_at: req_datetime(row, 4)?,
        updated_at: opt_datetime(row, 5)?,
        redacted_at: opt_datetime(row, 6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{ISSUE_COLUMNS, parse_event_type};
    use unblock_model::EventType;

    /// `parse_event_type` is storage's OWN wire→`EventType` map, SEPARATE from the model's
    /// hand-rolled `Deserialize`. Its `other => Custom(..)` catch-all makes a missing arm
    /// clippy-clean and test-silent: the row would read back as `Custom("comment_edited")`,
    /// breaking the 17-named oracle. This test is the guard — it FAILS if either D37 arm is
    /// dropped, and it fails again the next time the model gains a named variant without one.
    #[test]
    fn parse_event_type_covers_every_named_variant_and_never_falls_through_to_custom() {
        const ALL_NAMED: [EventType; 17] = [
            EventType::Created,
            EventType::Updated,
            EventType::StatusChanged,
            EventType::PriorityChanged,
            EventType::AssigneeChanged,
            EventType::Commented,
            EventType::Closed,
            EventType::Reopened,
            EventType::DependencyAdded,
            EventType::DependencyRemoved,
            EventType::LabelAdded,
            EventType::LabelRemoved,
            EventType::Compacted,
            EventType::Deleted,
            EventType::Restored,
            EventType::CommentEdited,
            EventType::CommentRedacted,
        ];
        for expected in ALL_NAMED {
            let parsed = parse_event_type(expected.as_str());
            assert!(
                !matches!(parsed, EventType::Custom(_)),
                "{} fell through to Custom — parse_event_type is missing its arm",
                expected.as_str()
            );
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn parse_event_type_maps_the_two_d37_comment_events() {
        assert_eq!(parse_event_type("comment_edited"), EventType::CommentEdited);
        assert_eq!(
            parse_event_type("comment_redacted"),
            EventType::CommentRedacted
        );
    }

    #[test]
    fn parse_event_type_unknown_still_becomes_custom() {
        assert_eq!(
            parse_event_type("frobnicated"),
            EventType::Custom("frobnicated".to_string())
        );
    }

    #[test]
    fn issue_columns_has_38_entries() {
        let count = ISSUE_COLUMNS.split(',').count();
        assert_eq!(count, 38, "issues projection must list all 38 columns");
    }

    #[test]
    fn issue_columns_starts_id_content_hash_title() {
        let first: Vec<&str> = ISSUE_COLUMNS.split(',').take(3).map(str::trim).collect();
        assert_eq!(first, ["id", "content_hash", "title"]);
    }

    #[test]
    fn issue_columns_ends_source_repo_path_agent_context() {
        let cols: Vec<&str> = ISSUE_COLUMNS.split(',').map(str::trim).collect();
        assert_eq!(cols[36], "source_repo_path");
        assert_eq!(cols[37], "agent_context");
    }
}
