//! The server: [`UnblockServer`] (the `ServerHandler`), [`serve`], and the hand-written resource +
//! `get_info` methods (spine §5).
//!
//! The single `impl ServerHandler for UnblockServer` STACKS `#[tool_handler]` + `#[prompt_handler]`
//! (each detects the other as a sibling and the hand-written `get_info`, so neither emits its own)
//! plus the HAND-WRITTEN `get_info` / `list_resource_templates` / `read_resource` (rmcp has no
//! resource macro). The tool routers from the 7 tool files compose via `+`; the prompt router comes
//! from `prompts::mod`. Holding `Arc<Session>`, the server is a thin adapter — the engine owns the
//! write Semaphore (D14), so there is no write orchestration here.

use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    AnnotateAble, ErrorData, GetPromptRequestParams, GetPromptResult, Implementation,
    ListPromptsResult, ListResourceTemplatesResult, PaginatedRequestParams, RawResourceTemplate,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::stdio;
use rmcp::{Service, ServiceExt};
use serde::Serialize;
use snafu::ResultExt;
use unblock_engine::Session;
use unblock_error::StructuredError;

use crate::error::{McpServerError, RunLoopSnafu, TransportSnafu};
use crate::options::{CONTRACT_VERSION, Quotas, ServeOptions};
use crate::resources::{self, ResourceUri, capabilities, schema_bundle};
use crate::tools::enforce_quota;

/// The MCP server handler — a thin adapter over [`Session`] (spine §5).
///
/// Holds `Arc<Session>` (so it is `Send + Sync`, as `ServerHandler` requires) plus the request
/// [`Quotas`]. It carries NO write lock — the engine owns the write Semaphore (D14).
///
/// `#[doc(hidden)] pub` (not part of the documented contract) only so the feature-gated
/// [`serve_duplex_for_test`] can name it in its return type; normal consumers use [`serve`].
#[doc(hidden)]
#[derive(Clone)]
pub struct UnblockServer {
    /// The single mutation home (FR-9). Every tool/resource call delegates to it.
    pub(crate) session: Arc<Session>,
    /// The untrusted-input limits (NFR-18), enforced in [`UnblockServer::preflight`].
    pub(crate) quotas: Quotas,
    /// Optional human-readable instructions advertised to clients in [`UnblockServer::get_info`]
    /// (from `ServeOptions::instructions`). `None` falls back to a generated default summary.
    pub(crate) instructions: Option<String>,
}

impl UnblockServer {
    /// Build the server handler.
    pub(crate) fn new(session: Arc<Session>, quotas: Quotas, instructions: Option<String>) -> Self {
        Self {
            session,
            quotas,
            instructions,
        }
    }

    /// Aggregate the 7 tool routers into one (composed via `+`, spine §5.1).
    fn aggregate_tool_router() -> ToolRouter<Self> {
        Self::issue_router()
            + Self::claim_router()
            + Self::defer_router()
            + Self::query_router()
            + Self::dep_router()
            + Self::sync_router()
            + Self::diagnostics_router()
    }

    /// The NFR-18 quota preflight, run inside each tool body BEFORE any `Session` call.
    ///
    /// Re-serializes the already-deserialized typed input and runs [`enforce_quota`]; an oversized
    /// input is rejected in-band (it never reaches the engine). Returns `Err(structured)` on breach.
    ///
    /// **Fail-closed:** if the typed input cannot be re-serialized for measurement, this returns an
    /// in-band `InternalError` rather than letting an un-measurable input slip past the quota — the
    /// untrusted-input boundary must never fail open (NFR-18).
    pub(crate) fn preflight<T: Serialize>(&self, input: &T) -> Result<(), StructuredError> {
        let value = serde_json::to_value(input).map_err(|err| {
            StructuredError::from_code(
                unblock_error::ErrorCode::InternalError,
                format!("failed to serialize input for quota measurement: {err}"),
            )
        })?;
        enforce_quota(&value, &self.quotas)
    }

    /// Build the `ReadResourceResult` text body for a parsed resource URI (read-only, FR-10).
    async fn read_resource_body(&self, uri: &str) -> Result<serde_json::Value, StructuredError> {
        match resources::parse_uri(uri) {
            ResourceUri::IssueById(id) => resources::issues::read_issue(&self.session, &id).await,
            ResourceUri::Ready => resources::issues::read_ready(&self.session).await,
            ResourceUri::Blocked => resources::issues::read_blocked(&self.session).await,
            ResourceUri::Capabilities => {
                serde_json::to_value(capabilities()).map_err(|e| resources::serialize_error(&e))
            }
            ResourceUri::Schema => {
                serde_json::to_value(schema_bundle()).map_err(|e| resources::serialize_error(&e))
            }
            ResourceUri::Unknown => Err(unknown_resource(uri)),
        }
    }
}

#[rmcp::tool_handler(router = Self::aggregate_tool_router())]
#[rmcp::prompt_handler(router = Self::prompt_router())]
impl ServerHandler for UnblockServer {
    /// Advertise tools + prompts + resources, the server identity, and the instructions.
    ///
    /// HAND-WRITTEN (not macro-generated): the two stacked handler macros both detect this method and
    /// skip emitting their own. It enables all three capabilities so the resource methods below are
    /// discoverable.
    ///
    /// The `server_info` is pinned to our real identity — name `"unblock"`, version this crate's
    /// `CARGO_PKG_VERSION` — instead of rmcp's `from_build_env()` default (which would advertise the
    /// `rmcp` crate). The instructions honor `ServeOptions::instructions` when the caller set one,
    /// else fall back to a generated capability summary.
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .enable_resources()
            .build();
        let instructions = self.instructions.clone().unwrap_or_else(|| {
            format!(
                "unblock MCP server (contract {CONTRACT_VERSION}). 7 tools, 5 resources, 3 prompts."
            )
        });
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new("unblock", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }

    /// Advertise the resource templates (the 5 `unblock://...` URIs, spine §5.4).
    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let templates = [
            (
                "unblock://issues/{id}",
                "issue-by-id",
                "A single issue by id.",
            ),
            (
                "unblock://issues/ready",
                "ready-issues",
                "The default-complete ready set.",
            ),
            (
                "unblock://issues/blocked",
                "blocked-issues",
                "The blocked set.",
            ),
            (
                "unblock://capabilities",
                "capabilities",
                "The discovery document (FR-12).",
            ),
            (
                "unblock://schema",
                "schema",
                "The JsonSchema bundle for every tool I/O (FR-12).",
            ),
        ]
        .into_iter()
        .map(|(uri, name, description)| {
            RawResourceTemplate::new(uri, name)
                .with_description(description)
                .with_mime_type("application/json")
                .no_annotation()
        })
        .collect();
        Ok(ListResourceTemplatesResult {
            resource_templates: templates,
            next_cursor: None,
            meta: None,
        })
    }

    /// Read a resource by URI (read-only, FR-10). A miss → `resource_not_found` (-32002); a domain
    /// error is surfaced as `ErrorData` (resources have no in-band channel like tools do).
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        match self.read_resource_body(&request.uri).await {
            Ok(body) => Ok(ReadResourceResult::new(vec![ResourceContents::text(
                body.to_string(),
                request.uri,
            )])),
            Err(structured) => Err(crate::error::to_rmcp_error_data(&structured)),
        }
    }
}

/// Build, bind, and run the MCP stdio server until cancellation (FR-17).
///
/// Binds the `transport-io` stdio transport and runs `serve_with_ct` with the caller's
/// [`ServeOptions::cancel`] token; a `cancel()` drains in-flight work and returns cleanly. The
/// `session` is shared as `Arc<Session>` (the engine owns the write Semaphore, D14).
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize/bind.
/// - [`McpServerError::RunLoop`] if the run loop ends abnormally (the background task is aborted).
pub async fn serve(session: Arc<Session>, opts: ServeOptions) -> Result<(), McpServerError> {
    let server = UnblockServer::new(session, opts.quotas, opts.instructions);
    let running = serve_handler(server, stdio(), opts.cancel).await?;
    running.waiting().await.context(RunLoopSnafu)?;
    Ok(())
}

/// Generic over the transport so the lifecycle test can drive an in-memory duplex transport.
async fn serve_handler<T, E, A>(
    server: UnblockServer,
    transport: T,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<rmcp::service::RunningService<RoleServer, UnblockServer>, McpServerError>
where
    UnblockServer: Service<RoleServer>,
    T: rmcp::transport::IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    server
        .serve_with_ct(transport, cancel)
        .await
        .context(TransportSnafu)
}

/// Build and run the server over an arbitrary in-memory transport (TEST-ONLY, `test-util` feature).
///
/// Drives the **same** `UnblockServer` + `serve_with_ct` path as [`serve`], but over a caller-
/// supplied duplex transport instead of stdio — so the M2 lifecycle exit-gate (`tests/lifecycle.rs`)
/// can run a full in-process MCP client/server flow without touching real stdio. Feature-gated and
/// `#[doc(hidden)]` so it never widens the shipped public surface.
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize over the transport.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub async fn serve_duplex_for_test<T, E, A>(
    session: Arc<Session>,
    quotas: Quotas,
    instructions: Option<String>,
    transport: T,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<rmcp::service::RunningService<RoleServer, UnblockServer>, McpServerError>
where
    T: rmcp::transport::IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let server = UnblockServer::new(session, quotas, instructions);
    serve_handler(server, transport, cancel).await
}

/// Build the structured not-found for an unknown resource URI (→ -32002 at the boundary).
fn unknown_resource(uri: &str) -> StructuredError {
    StructuredError::from_code(
        unblock_error::ErrorCode::IssueNotFound,
        format!("unknown resource: {uri}"),
    )
    .with_context("uri", serde_json::json!(uri))
}

#[cfg(test)]
mod tests {
    use super::UnblockServer;
    use crate::options::Quotas;

    const fn assert_send_sync<T: Send + Sync + 'static>() {}

    #[test]
    fn unblock_server_is_send_sync() {
        assert_send_sync::<UnblockServer>();
    }

    #[test]
    fn quotas_default_round_trips() {
        // A compile-witness that the server's quota type is constructible here.
        let _q = Quotas::default();
    }
}
