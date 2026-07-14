//! The server: [`UnblockServer`] (the `ServerHandler`), [`run_mcp_server`], and the hand-written resource +
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
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    AnnotateAble, CallToolRequestParams, CallToolResult, ClientJsonRpcMessage, ClientRequest,
    ErrorData, GetPromptRequestParams, GetPromptResult, Implementation, JsonRpcMessage,
    JsonRpcRequest, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    PaginatedRequestParams, ProtocolVersion, RawResource, RawResourceTemplate,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::{Transport, stdio};
use rmcp::{Service, ServiceExt};
use serde::Serialize;
use snafu::ResultExt;
use tokio::sync::Semaphore;
use unblock_engine::Session;
use unblock_error::{ErrorCode, StructuredError};

use crate::error::{McpServerError, RunLoopSnafu, TransportSnafu};
use crate::options::{CONTRACT_VERSION, McpServerOptions, Quotas};
use crate::resources::{self, ResourceUri, capabilities, schema_bundle};
use crate::tools::{enforce_quota, err_json};

/// The MCP server handler — a thin adapter over [`Session`] (spine §5).
///
/// Holds `Arc<Session>` (so it is `Send + Sync`, as `ServerHandler` requires) plus the request
/// [`Quotas`]. It carries NO write lock — the engine owns the write Semaphore (D14) — but it DOES
/// carry the NFR-18 rate-limit [`Semaphore`] (D34-F5), which sits STRICTLY ABOVE that write permit.
///
/// `#[doc(hidden)] pub` (not part of the documented contract) only so the feature-gated
/// [`mcp_server_duplex_for_test`] can name it in its return type; normal consumers use [`run_mcp_server`].
#[doc(hidden)]
#[derive(Clone)]
pub struct UnblockServer {
    /// The single mutation home (FR-9). Every tool/resource call delegates to it.
    pub(crate) session: Arc<Session>,
    /// The untrusted-input limits (NFR-18), enforced in [`UnblockServer::preflight`].
    pub(crate) quotas: Quotas,
    /// Optional human-readable instructions advertised to clients in [`UnblockServer::get_info`]
    /// (from `McpServerOptions::instructions`). `None` falls back to a generated default summary.
    pub(crate) instructions: Option<String>,
    /// The NFR-18 request-rate chokepoint (D34-F5, spine §5.6): a `Semaphore` of
    /// `quotas.max_concurrent_requests` permits, built ONCE in [`UnblockServer::new`]. `try_acquire`d
    /// (non-blocking) around the tool dispatch AND `read_resource`; saturation fast-fails with
    /// [`ErrorCode::RateLimited`]. `Arc` because the handler derives `Clone` and every clone MUST share
    /// the SAME permit pool (a per-clone semaphore would not bound total concurrency).
    pub(crate) rate_limit: Arc<Semaphore>,
}

impl UnblockServer {
    /// Build the server handler.
    pub(crate) fn new(session: Arc<Session>, quotas: Quotas, instructions: Option<String>) -> Self {
        // Clamp the UPPER bound only: `tokio::sync::Semaphore::new` panics on `> MAX_PERMITS`, and this
        // is a lib path (`#![forbid(unsafe_code)]` / no-panic-in-lib) reachable only via an absurd
        // operator config. A `0` is INTENTIONALLY preserved (fully-closed is fail-safe, and the SF-6
        // pin test drives 0 permits to prove every request rejects) — do NOT floor it to 1.
        let permits = quotas.max_concurrent_requests.min(Semaphore::MAX_PERMITS);
        let rate_limit = Arc::new(Semaphore::new(permits));
        Self {
            session,
            quotas,
            instructions,
            rate_limit,
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
    /// Dispatch a tool call — HAND-WRITTEN to install the NFR-18 rate-limit chokepoint (D34-F5, spine
    /// §5.6). Because this method is present, `#[rmcp::tool_handler]` does NOT emit its own `call_tool`
    /// (it only generates one when absent — `rmcp-macros-1.7.0/src/tool_handler.rs:44`, verified). The
    /// `rate_limit_chokepoint_is_the_live_tool_dispatch_path` assumption pin (`tests/rate_limit.rs`)
    /// fails LOUDLY if a future rmcp stops honouring that suppression, pointing maintainers straight here.
    ///
    /// A non-blocking [`Semaphore::try_acquire`] gates the WHOLE dispatch: on a held permit it
    /// replicates the macro's generated body EXACTLY ([`ToolCallContext::new`] +
    /// `aggregate_tool_router().call`), holding the permit for the entire `.await` (dropped on return);
    /// on saturation it FAST-FAILS in-band (MF-5 — tools have an in-band channel, unlike resources)
    /// with a retryable [`ErrorCode::RateLimited`] rather than backpressuring. The rate-limit
    /// `Semaphore` sits STRICTLY ABOVE the engine write `Semaphore(1)` (§4.2), so this non-blocking
    /// acquire can never join a wait cycle (deadlock-free vs D14/D31).
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.rate_limit.try_acquire() {
            Ok(_permit) => {
                let tcc = ToolCallContext::new(self, request, context);
                Self::aggregate_tool_router().call(tcc).await
            }
            Err(_) => Ok(err_json(&rate_limited_error())),
        }
    }

    /// Advertise tools + prompts + resources, the server identity, and the instructions.
    ///
    /// HAND-WRITTEN (not macro-generated): the two stacked handler macros both detect this method and
    /// skip emitting their own. It enables all three capabilities so the resource methods below are
    /// discoverable.
    ///
    /// The `server_info` is pinned to our real identity — name `"unblock"`, version this crate's
    /// `CARGO_PKG_VERSION` — instead of rmcp's `from_build_env()` default (which would advertise the
    /// `rmcp` crate). The instructions honor `McpServerOptions::instructions` when the caller set one,
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
        // NFR-18 rate-limit chokepoint (D34-F5, MF-5): resources have NO in-band channel like tools
        // do, so a saturated cap surfaces OUT-OF-BAND as `ErrorData` (asymmetric to `call_tool`). The
        // permit is held for the whole read (dropped on return).
        let Ok(_permit) = self.rate_limit.try_acquire() else {
            return Err(crate::error::to_rmcp_error_data(&rate_limited_error()));
        };
        match self.read_resource_body(&request.uri).await {
            Ok(body) => Ok(ReadResourceResult::new(vec![
                ResourceContents::text(body.to_string(), request.uri)
                    .with_mime_type("application/json"),
            ])),
            Err(structured) => Err(crate::error::to_rmcp_error_data(&structured)),
        }
    }
}

/// Build the NFR-18 rate-limit reject (D34-F5): a retryable [`ErrorCode::RateLimited`] structured
/// error, surfaced when the `max_concurrent_requests` cap is saturated. Emitted in-band for tools
/// (`err_json`) and out-of-band as `ErrorData` for resources (MF-5) — both carry this SAME payload
/// (`code=RATE_LIMITED`, `retryable=true`). Attaches NO `hint` — the code's `hint_shape` is `None`
/// (OQ-2: the `retryable` flag carries the back-off signal), so a hint here would break that taxonomy.
fn rate_limited_error() -> StructuredError {
    StructuredError::from_code(
        ErrorCode::RateLimited,
        "server at capacity: too many concurrent requests — retry after a short backoff",
    )
}

/// Build, bind, and run the MCP stdio server until cancellation (FR-17).
///
/// Binds the `transport-io` stdio transport and runs `serve_with_ct` with the caller's
/// [`McpServerOptions::cancel`] token; a `cancel()` drains in-flight work and returns cleanly. The
/// `session` is shared as `Arc<Session>` (the engine owns the write Semaphore, D14).
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize/bind.
/// - [`McpServerError::RunLoop`] if the run loop ends abnormally (the background task is aborted).
pub async fn run_mcp_server(
    session: Arc<Session>,
    opts: McpServerOptions,
) -> Result<(), McpServerError> {
    let server = UnblockServer::new(session, opts.quotas, opts.instructions);
    let running = run_mcp_server_handler(server, stdio(), opts.cancel).await?;
    running.waiting().await.context(RunLoopSnafu)?;
    Ok(())
}

/// Generic over the transport so the lifecycle test can drive an in-memory duplex transport.
///
/// The transport is wrapped in a [`VersionClampingTransport`] (CD-4) so the SAME protocol-version
/// clamp guards both the shipped stdio [`run_mcp_server`] and the test [`mcp_server_duplex_for_test`] path.
async fn run_mcp_server_handler<T, E, A>(
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
///
/// ## Assumption pin (CD-6) — this clamp is COUPLED to an UNDOCUMENTED rmcp internal
///
/// The correctness of this wrapper depends entirely on rmcp 1.7's serve-loop deriving the wire version
/// as a lexical `min(client, handler)` (above). That is an rmcp implementation detail, not a documented
/// contract, so a future rmcp bump could change it and silently make this clamp wrong or redundant.
/// Two `tests/protocol_version.rs` pins fail LOUDLY if that happens, pointing maintainers straight here:
/// - `rmcp_serve_loop_echoes_unsupported_below_latest_version_verbatim` drives the RAW, UNCLAMPED serve
///   path ([`mcp_server_duplex_unclamped_for_test`]) and asserts rmcp still echoes an unsupported
///   below-latest version verbatim — the exact misbehaviour this wrapper compensates for;
/// - `known_versions_and_latest_match_the_clamp_key_set` pins rmcp's [`ProtocolVersion::KNOWN_VERSIONS`]
///   / [`ProtocolVersion::LATEST`] (the set this clamp keys on) to their expected values.
///
/// If either pin fails, an rmcp upgrade changed the version-negotiation assumption — re-evaluate (and
/// possibly REMOVE) this transport before touching the pin. The eventual CD-6 endgame is an UPSTREAM
/// rmcp fix making unsupported-version clamping a first-class `ServerHandler` contract, after which this
/// wrapper can be deleted; that upstream work is OUT OF SCOPE here (this crate only pins the assumption).
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
///
/// The `KNOWN_VERSIONS`/`LATEST` set this keys on is pinned by
/// `known_versions_and_latest_match_the_clamp_key_set` (`tests/protocol_version.rs`); an rmcp bump that
/// adds, removes, or re-orders supported versions fails that pin — see [`VersionClampingTransport`] for
/// the full CD-6 coupling.
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
/// Drives the **same** `UnblockServer` + `serve_with_ct` path as [`run_mcp_server`], but over a caller-
/// supplied duplex transport instead of stdio — so the M2 lifecycle exit-gate (`tests/lifecycle.rs`)
/// can run a full in-process MCP client/server flow without touching real stdio. Feature-gated and
/// `#[doc(hidden)]` so it never widens the shipped public surface.
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize over the transport.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub async fn mcp_server_duplex_for_test<T, E, A>(
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
    run_mcp_server_handler(server, transport, cancel).await
}

/// Build and run the server over an arbitrary in-memory transport WITHOUT the
/// [`VersionClampingTransport`] — the RAW rmcp serve path (TEST-ONLY, `test-util` feature).
///
/// This is the CD-6 assumption-pin seam. Unlike [`mcp_server_duplex_for_test`] (which routes through
/// [`run_mcp_server_handler`] and therefore wraps the transport in [`VersionClampingTransport`]), this helper
/// calls `serve_with_ct` on the caller's transport DIRECTLY, installing NO clamp. It exists ONLY so the
/// `protocol_version` pin (`tests/protocol_version.rs`) can observe — and fail loudly on a change to —
/// rmcp 1.7's UN-guarded serve-loop version negotiation that [`clamp_unsupported_initialize_version`]
/// compensates for. NEVER route a shipped path through this: it deliberately reproduces the
/// spec-non-conformant echo of an unsupported requested version.
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize over the transport.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub async fn mcp_server_duplex_unclamped_for_test<T, E, A>(
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
    server
        .serve_with_ct(transport, cancel)
        .await
        .context(TransportSnafu)
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
