//! The server: [`UnblockServer`] (the `ServerHandler`), [`serve`], and the hand-written resource +
//! `get_info` methods (spine §5).
//!
//! The single `impl ServerHandler for UnblockServer` STACKS `#[tool_handler]` + `#[prompt_handler]`
//! (each detects the other as a sibling and the hand-written `get_info`, so neither emits its own)
//! plus the HAND-WRITTEN `get_info` / `list_resources` / `list_resource_templates` / `read_resource`
//! (rmcp has no resource macro). The tool routers from the 7 tool files compose via `+`; the prompt
//! router comes from `prompts::mod`. Holding `Arc<Session>`, the server is a thin adapter — the engine
//! owns the write Semaphore (D14), so there is no write orchestration here.

use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    AnnotateAble, ClientJsonRpcMessage, ClientRequest, ErrorData, GetPromptRequestParams,
    GetPromptResult, Implementation, JsonRpcMessage, JsonRpcRequest, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams, ProtocolVersion,
    RawResource, RawResourceTemplate, ReadResourceRequestParams, ReadResourceResult,
    ResourceContents, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::{Transport, stdio};
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

    /// Advertise the four CONCRETE (non-parameterized) resources via `resources/list` (CD-3).
    ///
    /// Per the MCP spec, only RFC-6570 URI *templates* (a `{param}` placeholder) belong in
    /// `resources/templates/list`; every fully-resolved URI belongs in `resources/list`. Four of our
    /// five resources are concrete — `unblock://issues/ready`, `unblock://issues/blocked`,
    /// `unblock://capabilities`, `unblock://schema` — so they are advertised HERE (the fifth,
    /// `unblock://issues/{id}`, is the only genuine template; see [`Self::list_resource_templates`]).
    /// Each carries the `application/json` mime type its [`Self::read_resource`] body serializes to.
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let resources = [
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
            RawResource::new(uri, name)
                .with_description(description)
                .with_mime_type("application/json")
                .no_annotation()
        })
        .collect();
        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    /// Advertise the ONE genuine RFC-6570 resource template — `unblock://issues/{id}` (CD-3, spine
    /// §5.4).
    ///
    /// The other four `unblock://...` URIs are concrete (no `{param}`) and are advertised via
    /// [`Self::list_resources`] instead — a strict MCP client treats `resources/templates/list`
    /// entries as parameterized, so a concrete URI listed here would be mis-advertised as a template.
    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let templates = vec![
            RawResourceTemplate::new("unblock://issues/{id}", "issue-by-id")
                .with_description("A single issue by id.")
                .with_mime_type("application/json")
                .no_annotation(),
        ];
        Ok(ListResourceTemplatesResult {
            resource_templates: templates,
            next_cursor: None,
            meta: None,
        })
    }

    /// Read a resource by URI (read-only, FR-10). A miss → `resource_not_found` (-32002); a domain
    /// error is surfaced as `ErrorData` (resources have no in-band channel like tools do).
    ///
    /// The body is stamped `mimeType: application/json` (CD-5) — the same type `resources/list` and
    /// `resources/templates/list` advertise. rmcp's [`ResourceContents::text`] hardcodes the
    /// non-IANA `"text"`, so we override it via [`ResourceContents::with_mime_type`]; the body bytes
    /// are unchanged.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        match self.read_resource_body(&request.uri).await {
            Ok(body) => Ok(ReadResourceResult::new(vec![
                ResourceContents::text(body.to_string(), request.uri)
                    .with_mime_type("application/json"),
            ])),
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
///
/// The transport is wrapped in a [`VersionClampingTransport`] (CD-4) so the SAME protocol-version
/// clamp guards both the shipped stdio [`serve`] and the test [`serve_duplex_for_test`] path.
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
    let transport = VersionClampingTransport::new(transport.into_transport());
    server
        .serve_with_ct(transport, cancel)
        .await
        .context(TransportSnafu)
}

/// A [`Transport`] decorator that clamps an UNSUPPORTED inbound `initialize` `protocolVersion` to the
/// server's latest supported [`ProtocolVersion`] BEFORE rmcp's serve-loop negotiation sees it (CD-4,
/// MCP lifecycle spec: an unsupported requested version MUST be answered with a version the server
/// supports — SHOULD be its latest — never echoed back).
///
/// ## Why a transport wrapper and NOT a `ServerHandler::initialize` override
///
/// rmcp 1.7's server serve-loop (`serve_server_with_ct_inner`) RE-DERIVES the wire `protocolVersion`
/// AFTER the handler returns: it takes `min(client_requested, handler_response)` by a purely LEXICAL
/// string compare and echoes the client's value whenever it sorts below the handler's. So a handler
/// that returns `LATEST` is overridden right back to a bogus client-sent `"1999-01-01"` (it sorts
/// below `LATEST`) — and there is NO handler return value `R` that can force `LATEST` for a
/// below-latest unsupported version (the loop yields the client whenever `client < R`, and `R` cannot
/// be simultaneously `LATEST` and `<= "1999-01-01"`; verified empirically). The only place we can
/// enforce conformance is BEFORE that negotiation, by clamping the inbound request. SUPPORTED (older)
/// versions are left untouched so the loop still echoes them, exactly as the spec requires.
struct VersionClampingTransport<T> {
    /// The wrapped transport; every call delegates to it, `receive` after the clamp.
    inner: T,
}

impl<T> VersionClampingTransport<T> {
    /// Wrap `inner`.
    fn new(inner: T) -> Self {
        Self { inner }
    }
}

impl<T> Transport<RoleServer> for VersionClampingTransport<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let mut message = self.inner.receive().await?;
        clamp_unsupported_initialize_version(&mut message);
        Some(message)
    }

    fn close(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

/// Rewrite an UNSUPPORTED `initialize` `protocolVersion` to [`ProtocolVersion::LATEST`] in place;
/// leave every other message — and every SUPPORTED requested version — untouched (CD-4).
///
/// "Supported" is rmcp's [`ProtocolVersion::KNOWN_VERSIONS`] set (the versions this SDK, and thus this
/// server, actually speaks). A version outside that set is clamped to `LATEST`; a version inside it is
/// preserved so rmcp's serve-loop still echoes it (spec: a supported requested version MUST be echoed).
fn clamp_unsupported_initialize_version(message: &mut ClientJsonRpcMessage) {
    if let JsonRpcMessage::Request(JsonRpcRequest {
        request: ClientRequest::InitializeRequest(request),
        ..
    }) = message
        && !ProtocolVersion::KNOWN_VERSIONS.contains(&request.params.protocol_version)
    {
        request.params.protocol_version = ProtocolVersion::LATEST;
    }
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
