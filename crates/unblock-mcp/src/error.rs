//! The crate's error surface: [`McpServerError`] (server lifecycle only) + the boundary mappers.
//!
//! **Domain errors are NOT here** — they flow *in-band* as `ToolOutput::Error` (FR-11). This module
//! owns only:
//!
//! - [`McpServerError`] — the snafu enum for server lifecycle/transport faults (the public type the
//!   CLI handles from [`crate::serve`]).
//! - [`engine_error_to_structured`] — the spine §2.4/§5.6 boundary mapper: exactly `(&err).into()`
//!   (the blanket `From<&EngineError> for StructuredError`, F-6). Every `EngineError` variant is
//!   covered (it is `CodedError`), yielding an already-sanitized [`StructuredError`]; the JSON
//!   boundary makes `context` terminal-safe (serde escapes it) so no extra sanitize is needed here.
//! - [`to_rmcp_error_data`] — wrap a [`StructuredError`] as an rmcp protocol-fault `ErrorData`
//!   (reserved for true protocol faults; domain errors use the in-band channel).

use rmcp::model::{ErrorCode as RmcpErrorCode, ErrorData};
use snafu::Snafu;
use unblock_engine::EngineError;
use unblock_error::StructuredError;

/// Server lifecycle/transport errors surfaced by [`crate::serve`] (NOT domain errors).
///
/// Per-tool domain failures are returned *in-band* as `ToolOutput::Error` (always-valid JSON,
/// FR-11); this enum is reserved for the server's own lifecycle — binding the stdio transport or the
/// run loop failing.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum McpServerError {
    /// The stdio transport could not be bound / the rmcp service failed to initialize.
    #[snafu(display("failed to start the MCP server: {source}"))]
    Transport {
        /// The underlying rmcp service initialization error (boxed — it is a large enum).
        #[snafu(source(from(rmcp::service::ServerInitializeError, Box::new)))]
        source: Box<rmcp::service::ServerInitializeError>,
    },

    /// The server run loop ended abnormally (the background task panicked or was aborted).
    #[snafu(display("the MCP server run loop ended abnormally: {source}"))]
    RunLoop {
        /// The join error from the rmcp run loop task.
        source: tokio::task::JoinError,
    },
}

/// Map an [`EngineError`] to a sanitized [`StructuredError`] at the MCP boundary (spine §2.4/§5.6).
///
/// Exactly `(&err).into()` — the blanket `From<&EngineError> for StructuredError` (F-6). The engine
/// error is `CodedError`, so this composes the union error → one `ErrorCode` →
/// `code`/`message`/`hint`/`retryable`/`context`, with a terminal-sanitized message/hint.
pub(crate) fn engine_error_to_structured(err: &EngineError) -> StructuredError {
    err.into()
}

/// Wrap a [`StructuredError`] as an rmcp **protocol-fault** [`ErrorData`].
///
/// Reserved for true protocol faults (transport/serialization/auth-level). Domain errors do NOT use
/// this — they flow in-band via `CallToolResult::structured_error` (FR-11). The full structured
/// payload is attached as `data` so a client still sees `code`/`message`/`hint`/`retryable`/`context`.
pub(crate) fn to_rmcp_error_data(structured: &StructuredError) -> ErrorData {
    let data = serde_json::to_value(structured).ok();
    ErrorData::new(
        RmcpErrorCode::INTERNAL_ERROR,
        structured.message.clone(),
        data,
    )
}

#[cfg(test)]
mod tests {
    use super::{engine_error_to_structured, to_rmcp_error_data};
    use unblock_engine::EngineError;
    use unblock_error::ErrorCode;

    #[test]
    fn feature_not_wired_maps_to_internal_error_structured() {
        let err = EngineError::FeatureNotWired { feature: "sync" };
        let structured = engine_error_to_structured(&err);
        assert_eq!(structured.code, ErrorCode::InternalError);
        assert!(!structured.retryable);
    }

    #[test]
    fn workspace_not_open_maps_to_not_initialized() {
        let structured = engine_error_to_structured(&EngineError::WorkspaceNotOpen);
        assert_eq!(structured.code, ErrorCode::NotInitialized);
    }

    #[test]
    fn rmcp_error_data_carries_the_structured_payload() {
        let structured = engine_error_to_structured(&EngineError::ShutdownInProgress);
        let data = to_rmcp_error_data(&structured);
        let payload = data.data.expect("structured payload attached");
        assert_eq!(payload["code"], "INTERNAL_ERROR");
    }
}
