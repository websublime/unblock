//! Issue resources: `unblock://issues/{id}`, `unblock://issues/ready`, `unblock://issues/blocked`
//! (spine §5.4). Read-only — never acquires the write permit (FR-10).
//!
//! These helpers run the `Session::{get, ready, blocked}` reads and return the JSON body the server's
//! `read_resource` wraps as `ReadResourceResult`. A missing `{id}` surfaces a structured
//! `IssueNotFound` (the server maps it to `ErrorData::resource_not_found`, -32002).

use unblock_engine::Session;
use unblock_error::{ErrorCode, StructuredError};
use unblock_model::ListFilters;

use crate::error::engine_error_to_structured;

/// Read a single issue by id (`unblock://issues/{id}`) → the issue JSON, or a structured not-found.
pub(crate) async fn read_issue(
    session: &Session,
    id: &str,
) -> Result<serde_json::Value, StructuredError> {
    match session.get(id).await {
        Ok(Some(issue)) => serde_json::to_value(&issue).map_err(|e| serialize_error(&e)),
        Ok(None) => Err(StructuredError::from_code(
            ErrorCode::IssueNotFound,
            format!("issue not found: {id}"),
        )
        .with_context("id", serde_json::json!(id))),
        Err(err) => Err(engine_error_to_structured(&err)),
    }
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

/// Map a JSON serialization failure to a structured `InternalError` (no panic in library code).
fn serialize_error(err: &serde_json::Error) -> StructuredError {
    StructuredError::from_code(
        ErrorCode::InternalError,
        format!("failed to serialize resource body: {err}"),
    )
}
