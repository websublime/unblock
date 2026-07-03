//! Issue resources: `unblock://issues/{id}`, `unblock://issues/ready`, `unblock://issues/blocked`
//! (spine §5.4). Read-only — never acquires the write permit (FR-10).
//!
//! These helpers run the `Session::{get, ready, blocked}` reads and return the JSON body the server's
//! `read_resource` wraps as `ReadResourceResult`. A missing `{id}` surfaces a structured
//! `IssueNotFound` (the server maps it to `ErrorData::resource_not_found`, -32002).

use unblock_engine::Session;
use unblock_error::{ErrorCode, StructuredError, find_similar_ids};
use unblock_model::ListFilters;

use crate::error::engine_error_to_structured;
use crate::resources::serialize_error;

/// Read a single issue by id (`unblock://issues/{id}`) → the issue JSON, or a structured not-found
/// carrying fuzzy `similar_ids` suggestions (T2.6/D25/FORK-3A).
pub(crate) async fn read_issue(
    session: &Session,
    id: &str,
) -> Result<serde_json::Value, StructuredError> {
    match session.get(id).await {
        Ok(Some(issue)) => serde_json::to_value(&issue).map_err(|e| serialize_error(&e)),
        Ok(None) => Err(issue_not_found_with_suggestions(session, id).await),
        Err(err) => Err(engine_error_to_structured(&err)),
    }
}

/// Build the not-found error for a missing `{id}` with fuzzy near-miss suggestions (T2.6/D25/FORK-3A).
///
/// Faithful adaptation of the original `issue_not_found_resource`
/// (`temp/beads_rust-main/src/mcp/resources.rs:60-87`) + `StructuredError::issue_not_found`
/// (`temp/beads_rust-main/src/error/structured.rs:336-359`), mapped onto `Session` reads (no new
/// storage surface): the candidate corpus = the FULL id set (the original `get_all_ids` — every row,
/// closed + tombstone included, `sqlite.rs:6962`) via [`Session::list`] with `include_deferred` /
/// `include_closed` / `include_tombstone` all true; cap = 3; a FAILED corpus scan SURFACES the scan
/// error (the original's pinned `..._surfaces_id_scan_failure` behaviour) instead of the not-found.
/// The hint follows the original's family ("Did you mean …?" / a list-discovery fallback); the beads
/// `br list` CLI pointer becomes the equivalent MCP `query{kind:list}` pointer; the context keys are
/// the original's `searched_id` + `similar_ids`.
async fn issue_not_found_with_suggestions(session: &Session, id: &str) -> StructuredError {
    let corpus = ListFilters {
        include_deferred: true,
        include_closed: true,
        include_tombstone: true,
        ..ListFilters::default()
    };
    let candidates: Vec<String> = match session.list(&corpus).await {
        Ok(issues) => issues.into_iter().map(|issue| issue.id).collect(),
        // Faithful: a failed corpus scan surfaces the scan error, NOT the not-found.
        Err(err) => return engine_error_to_structured(&err),
    };
    let similar = find_similar_ids(id, &candidates, 3);
    let hint = if similar.is_empty() {
        "Run `query` with {\"kind\":\"list\"} to see available issues.".to_string()
    } else if similar.len() == 1 {
        format!("Did you mean '{}'?", similar[0])
    } else {
        format!("Did you mean one of: {}?", similar.join(", "))
    };
    StructuredError::from_code(ErrorCode::IssueNotFound, format!("issue not found: {id}"))
        .with_hint(hint)
        .with_context("searched_id", serde_json::json!(id))
        .with_context("similar_ids", serde_json::json!(similar))
}

/// Read the default-complete ready set (`unblock://issues/ready`) → a JSON array of issues.
pub(crate) async fn read_ready(session: &Session) -> Result<serde_json::Value, StructuredError> {
    match session.ready(&ListFilters::default()).await {
        Ok(issues) => serde_json::to_value(&issues).map_err(|e| serialize_error(&e)),
        Err(err) => Err(engine_error_to_structured(&err)),
    }
}

/// Read the blocked set (`unblock://issues/blocked`) → a JSON array of issues.
pub(crate) async fn read_blocked(session: &Session) -> Result<serde_json::Value, StructuredError> {
    match session.blocked(&ListFilters::default()).await {
        Ok(issues) => serde_json::to_value(&issues).map_err(|e| serialize_error(&e)),
        Err(err) => Err(engine_error_to_structured(&err)),
    }
}
