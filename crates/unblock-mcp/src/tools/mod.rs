//! The 7 consolidated MCP tools (spine §5.1) + the shared quota preflight and JSON adapters.
//!
//! Each tool family is its own file (`issue`/`claim`/`defer`/`query`/`dep`/`sync`/`diagnostics`); the
//! tool functions live on [`crate::server::UnblockServer`] under one `#[tool_router]` (see
//! `server.rs`). This module owns the cross-tool helpers:
//!
//! - [`enforce_quota`] — the NFR-18 untrusted-input preflight, run **inside each tool body after
//!   `Parameters<T>` deserialization and BEFORE any `Session` call** (rmcp has no stdio quota hook).
//! - [`ok_json`] / [`err_json`] — map a domain result to an rmcp `CallToolResult` that is always
//!   valid JSON (success via `structured`, in-band domain error via `structured_error`, FR-11).

pub(crate) mod bulk_markdown;
pub(crate) mod claim;
pub(crate) mod defer;
pub(crate) mod dep;
pub(crate) mod diagnostics;
pub(crate) mod dto;
pub(crate) mod issue;
pub(crate) mod output;
pub(crate) mod query;
pub(crate) mod sync;

use rmcp::model::CallToolResult;
use serde::Serialize;
use unblock_engine::EngineError;
use unblock_error::{ErrorCode, StructuredError};

use crate::error::engine_error_to_structured;
use crate::options::Quotas;

/// The serialized output of a successful tool call (a spine §5.3 per-tool output type, D25).
///
/// Mapped to an rmcp `CallToolResult::structured` (content mirror + structured `data`,
/// `is_error=false`). Domain errors do NOT use this — they use [`err_json`]. `serde_json::to_value`
/// is the wire arbiter here, so the untagged per-tool output unions are byte-identical to the values
/// they wrap.
pub(crate) fn ok_json<T: Serialize>(output: &T) -> CallToolResult {
    match serde_json::to_value(output) {
        Ok(value) => CallToolResult::structured(value),
        // Serialization of a domain value should be infallible; if it ever fails, surface a
        // structured InternalError rather than panicking (no unwrap in library code).
        Err(err) => err_json(&StructuredError::from_code(
            ErrorCode::InternalError,
            format!("failed to serialize tool output: {err}"),
        )),
    }
}

/// Map a [`StructuredError`] to an **in-band** error `CallToolResult` (FR-11).
///
/// `CallToolResult::structured_error` carries the JSON content mirror + structured `data` +
/// `is_error=Some(true)` in one result — always valid JSON even on error (the shared in-band error
/// output, `SchemaBundle.error`; the rmcp `is_error` flag is the channel discriminator, §5.6).
/// `Err(ErrorData)` is reserved for true protocol faults.
pub(crate) fn err_json(structured: &StructuredError) -> CallToolResult {
    match serde_json::to_value(structured) {
        Ok(value) => CallToolResult::structured_error(value),
        Err(_) => CallToolResult::structured_error(serde_json::json!({
            "code": ErrorCode::InternalError.as_str(),
            "message": "failed to serialize structured error",
            "retryable": false,
        })),
    }
}

/// Map an [`EngineError`] to an in-band error `CallToolResult` (the boundary, spine §5.6).
pub(crate) fn engine_err_json(err: &EngineError) -> CallToolResult {
    err_json(&engine_error_to_structured(err))
}

/// The untrusted-input quota preflight (NFR-18), run inside each tool body **before** the engine.
///
/// rmcp provides NO built-in request-size / array-length / string-length / batch cap on the stdio
/// path, so this is the only enforcement point. It walks the already-deserialized JSON `args` and
/// rejects, in-band, any input that exceeds a [`Quotas`] limit — so an oversized payload never
/// reaches a `Session` call (the blast radius stays confined to the workspace).
///
/// Scope: this enforces `max_request_bytes`, `max_array_len`, and `max_string_len`. The
/// [`Quotas::max_batch`] cap (the bulk record-count limit, D22/T2.3) is enforced by
/// [`enforce_batch_quota`] at the `create_bulk` action AFTER the markdown parse (before any mint),
/// since it bounds the *parsed* record count, not a raw input array. `max_concurrent_requests` is an
/// rmcp-transport concern, not a per-call preflight one.
///
/// **Fail-closed:** an input that cannot even be re-serialized for size measurement is rejected as an
/// `InternalError` rather than waved through — the untrusted boundary never fails open.
///
/// Returns `Err(structured)` (a `ValidationFailed` over-quota error, or an `InternalError` on an
/// un-measurable input) on breach, `Ok(())` otherwise.
pub(crate) fn enforce_quota(
    args: &serde_json::Value,
    quotas: &Quotas,
) -> Result<(), StructuredError> {
    // Total serialized size. A `serde_json::Value` is plain data, so re-serializing it is effectively
    // infallible — but if it ever fails we fail closed (treat the input as un-measurable, reject it)
    // rather than measuring it as zero bytes and letting it through.
    let serialized = serde_json::to_string(args).map_err(|err| {
        StructuredError::from_code(
            ErrorCode::InternalError,
            format!("failed to serialize input for quota measurement: {err}"),
        )
    })?;
    if serialized.len() > quotas.max_request_bytes {
        return Err(over_quota(
            "request",
            serialized.len(),
            quotas.max_request_bytes,
        ));
    }
    check_value(args, quotas)
}

/// Recursively check arrays/strings against the per-element limits.
fn check_value(value: &serde_json::Value, quotas: &Quotas) -> Result<(), StructuredError> {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > quotas.max_string_len {
                return Err(over_quota("string", s.len(), quotas.max_string_len));
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            if items.len() > quotas.max_array_len {
                return Err(over_quota("array", items.len(), quotas.max_array_len));
            }
            for item in items {
                check_value(item, quotas)?;
            }
            Ok(())
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                check_value(v, quotas)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Enforce [`Quotas::max_batch`] on a PARSED bulk record count (D22/T2.3, F5).
///
/// Run by the `create_bulk` action AFTER the markdown parse and BEFORE any mint, so an over-cap
/// document is rejected in-band (a `ValidationFailed`) and never reaches `Session::create_bulk` (the
/// blast radius stays confined to the workspace; the spy `Session` records zero calls).
pub(crate) fn enforce_batch_quota(count: usize, quotas: &Quotas) -> Result<(), StructuredError> {
    if count > quotas.max_batch {
        return Err(over_quota("batch", count, quotas.max_batch));
    }
    Ok(())
}

/// Build the structured over-quota error (a `ValidationFailed` with the limit context).
fn over_quota(kind: &str, actual: usize, limit: usize) -> StructuredError {
    StructuredError::from_code(
        ErrorCode::ValidationFailed,
        format!("{kind} exceeds the configured quota ({actual} > {limit})"),
    )
    .with_hint("reduce the input size below the server quota and retry")
    .with_context("kind", serde_json::json!(kind))
    .with_context("actual", serde_json::json!(actual))
    .with_context("limit", serde_json::json!(limit))
}

#[cfg(test)]
mod tests {
    use super::{enforce_quota, err_json, ok_json};
    use crate::options::Quotas;
    use unblock_error::{ErrorCode, StructuredError};

    #[test]
    fn ok_json_is_not_an_error_result() {
        let result = ok_json(&serde_json::json!({"id": "ub-1"}));
        assert_eq!(result.is_error, Some(false));
        assert!(result.structured_content.is_some());
    }

    #[test]
    fn err_json_is_an_error_result_with_valid_json() {
        let structured = StructuredError::from_code(ErrorCode::IssueNotFound, "nope");
        let result = err_json(&structured);
        assert_eq!(result.is_error, Some(true));
        let payload = result.structured_content.expect("structured payload");
        assert_eq!(payload["code"], "ISSUE_NOT_FOUND");
    }

    #[test]
    fn enforce_quota_passes_small_input() {
        let args = serde_json::json!({"title": "small", "labels": ["a", "b"]});
        assert!(enforce_quota(&args, &Quotas::default()).is_ok());
    }

    #[test]
    fn enforce_quota_rejects_over_length_array() {
        let quotas = Quotas {
            max_array_len: 2,
            ..Quotas::default()
        };
        let args = serde_json::json!({"labels": ["a", "b", "c"]});
        let err = enforce_quota(&args, &quotas).expect_err("over array quota");
        assert_eq!(err.code, ErrorCode::ValidationFailed);
        assert_eq!(err.context["kind"], "array");
    }

    #[test]
    fn enforce_quota_rejects_over_length_string() {
        let quotas = Quotas {
            max_string_len: 4,
            ..Quotas::default()
        };
        let args = serde_json::json!({"title": "way too long"});
        let err = enforce_quota(&args, &quotas).expect_err("over string quota");
        assert_eq!(err.context["kind"], "string");
    }

    #[test]
    fn enforce_quota_rejects_over_request_bytes() {
        let quotas = Quotas {
            max_request_bytes: 8,
            ..Quotas::default()
        };
        let args = serde_json::json!({"title": "this whole request is too big"});
        let err = enforce_quota(&args, &quotas).expect_err("over request quota");
        assert_eq!(err.context["kind"], "request");
    }
}
