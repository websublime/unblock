//! The server: [`UnblockServer`] (the `ServerHandler`), [`run_mcp_server`], and the hand-written resource +
//! `get_info` methods (spine §5).
//!
//! The single `impl ServerHandler for UnblockServer` STACKS `#[tool_handler]` + `#[prompt_handler]`
//! (each detects the other as a sibling and the hand-written `get_info`, so neither emits its own)
//! plus the HAND-WRITTEN `get_info` / `list_resources` / `list_resource_templates` / `read_resource`
//! (rmcp has no resource macro). The tool routers from the 8 tool modules compose via `+`; the prompt
//! router comes from `prompts::mod`. Holding `Arc<Session>`, the server is a thin adapter — the engine
//! owns the write Semaphore (D14), so there is no write orchestration here.

use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ErrorData, Implementation,
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, RequestMetaObject,
    Resource, ResourceContents, ResourceTemplate, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::stdio;
use rmcp::{Service, ServiceExt};
use snafu::ResultExt;
use tokio::sync::Semaphore;
use unblock_engine::Session;
use unblock_error::{ErrorCode, StructuredError};

use crate::error::{McpServerError, RunLoopSnafu, TransportSnafu};
use crate::options::{CONTRACT_VERSION, McpServerOptions, Quotas};
use crate::pre_handshake::PreHandshakeGateTransport;
use crate::resources::{self, ResourceUri, capabilities, schema_bundle};
use crate::tools::args::{duplicate_key_error, indeterminate_frame_error, unscanned_frame_error};
use crate::tools::{enforce_quota, err_json};
use crate::wire::{DupScanningTransport, ParamsScan};

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
    /// The untrusted-input limits (NFR-18), enforced in [`UnblockServer::enforce_request_quota`].
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
    /// TEST-ONLY (`test-util`): the HELD-HANDLER-RETURNED signal of PRD §4 D59 criterion (17)'s
    /// deterministic hold seams. One permit is added as `call_tool` returns, i.e. BEFORE rmcp's
    /// request task hands the response to the serve loop, whose send task then writes it (or drops it
    /// for a cancelled request, `rmcp-3.10.0/src/service.rs:1528-1531` and `:1640-1651`). A cell that
    /// needs the reply on the wire must still READ it; the signal proves only that the handler, and
    /// the write it ran, finished.
    #[cfg(feature = "test-util")]
    pub(crate) returned: Option<Arc<Semaphore>>,
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
            #[cfg(feature = "test-util")]
            returned: None,
        }
    }

    /// Aggregate the 8 tool routers into one (composed via `+`, spine §5.1).
    fn aggregate_tool_router() -> ToolRouter<Self> {
        Self::issue_router()
            + Self::claim_router()
            + Self::defer_router()
            + Self::query_router()
            + Self::dep_router()
            + Self::sync_router()
            + Self::diagnostics_router()
            + Self::comment_router()
    }

    /// The NFR-18 per-request quota check (D42), run **once in [`Self::call_tool`] before dispatch**.
    ///
    /// **"per-request bytes" is DEFINED as the re-serialized `CallToolRequestParams` plus
    /// `RequestContext.meta`** (D59 clause (4)) — on rmcp 3.10 that is `name` + `arguments` +
    /// `inputResponses` + `requestState` + `_meta` — excluding the JSON-RPC envelope
    /// (`jsonrpc`/`id`/`method`), which is not reachable from `ServerHandler::call_tool`.
    ///
    /// This replaces the pre-D42 `preflight`, which measured the **re-serialized typed DTO**. That
    /// was the defect (facet (c)): a payload parked under an unknown key never appeared in the typed
    /// value, so it was never measured at all, while `#[serde(default)]` padding *was* counted.
    ///
    /// **The re-serialization here is NOT that defect.** Every `CallToolRequestParams` field is raw
    /// JSON (`name: Cow<str>`, `arguments: Option<JsonObject>`, `input_responses:
    /// Option<InputResponses>` — a map of JSON values —, `request_state: Option<String>`, and
    /// `meta: Option<RequestMetaObject>`, transparent over a `JsonObject`), so `to_value` is lossless
    /// by construction — pinned by `request_measurement_is_lossless`. The struct is
    /// `#[non_exhaustive]`, so a future rmcp field is picked up automatically: the fail-safe
    /// direction.
    ///
    /// # `_meta` must be passed SEPARATELY — verified by execution, not assumed
    ///
    /// `request.meta` is **always `None` here.** rmcp's serve loop moves `_meta` out of the request
    /// and into `RequestContext.meta` (a [`RequestMetaObject`], `rmcp-3.10.0/src/service.rs:1237`)
    /// *before* `handle_request`. So serializing the request alone measures `name` + `arguments`
    /// and **silently misses the live `_meta` bypass** — a 300 KB `_meta` blob would still sail past
    /// the cap. This was caught by a test that failed for exactly that reason; it is not a
    /// hypothetical.
    ///
    /// The caller therefore hands us `context.meta`, which is where the bytes actually are.
    /// `params.task` is NOT measured: rmcp 3.10's `CallToolRequestParams` has no `task` field and
    /// denies no unknown key, so a present `task` is discarded during the typed decode, never reaches
    /// `call_tool` and drives no work (D59 clause (4)); it joins every other unknown `params` key,
    /// whose bytes cost only the parse (the `ub-o8s` residual).
    ///
    /// **Fail-closed:** an un-measurable request is rejected as an `InternalError` rather than waved
    /// through — the untrusted-input boundary must never fail open (NFR-18).
    pub(crate) fn enforce_request_quota(
        &self,
        request: &CallToolRequestParams,
        meta: &RequestMetaObject,
    ) -> Result<(), StructuredError> {
        let measurement_error = |err: &serde_json::Error| {
            StructuredError::from_code(
                ErrorCode::InternalError,
                format!("failed to serialize request for quota measurement: {err}"),
            )
        };
        let mut value = serde_json::to_value(request).map_err(|e| measurement_error(&e))?;
        if !meta.is_empty()
            && let Some(object) = value.as_object_mut()
        {
            // Re-attach under the wire name so an over-quota `_meta` is reported against the same
            // key the client sent.
            object.insert(
                "_meta".to_string(),
                serde_json::to_value(meta).map_err(|e| measurement_error(&e))?,
            );
        }
        enforce_quota(&value, &self.quotas)
    }

    /// The rate-limited, scanned, quota-checked dispatch [`ServerHandler::call_tool`] runs (D34-F5,
    /// D43, D42); split out only so `call_tool` can raise the `test-util` returned signal on EVERY
    /// return path.
    async fn call_tool_gated(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match self.rate_limit.try_acquire() {
            Ok(_permit) => {
                // D43: the duplicate-JSON-key gate, FIRST — before the quota. The scan itself
                // already ran at the transport (`crate::wire`), so this is O(1); it goes first
                // because a frame whose MEANING is ambiguous must be reported as such, not masked
                // as over-quota. Inside the permit for the same D34-F5 reason the quota walk is.
                if let Err(structured) = frame_scan_gate(&context.extensions) {
                    return Ok(err_json(&structured).into());
                }
                // D42: the NFR-18 quota chokepoint, over the WHOLE `params`, INSIDE the permit and
                // BEFORE dispatch. Inside the permit because the walk is O(request bytes) of CPU —
                // running it above `try_acquire` would let unbounded concurrent oversized requests
                // burn CPU outside the D34-F5 bound, inverting the invariant documented above.
                // `try_acquire` is non-blocking, so a rejected request holds a permit for
                // microseconds.
                //
                // `context.meta` is passed explicitly: rmcp SWAPS `_meta` out of the request into
                // the RequestContext before this method is called, so `request.meta` is always
                // `None` here and measuring the request alone would miss the `_meta` channel
                // entirely — which is the live bypass this piece exists to close.
                if let Err(structured) = self.enforce_request_quota(&request, &context.meta) {
                    return Ok(err_json(&structured).into());
                }
                let tcc = ToolCallContext::new(self, request, context);
                Self::aggregate_tool_router().call(tcc).await
            }
            Err(_) => Ok(err_json(&rate_limited_error()).into()),
        }
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
    /// (it only generates one when absent — `rmcp-macros-3.10.0/src/tool_handler.rs:44`, verified). The
    /// `rate_limit_chokepoint_is_the_live_tool_dispatch_path` assumption pin (`tests/rate_limit.rs`)
    /// fails LOUDLY if a future rmcp stops honouring that suppression, pointing maintainers straight here.
    ///
    /// A non-blocking [`Semaphore::try_acquire`] gates the WHOLE dispatch: on a held permit it runs
    /// the macro's generated body ([`ToolCallContext::new`] + `aggregate_tool_router().call`),
    /// holding the permit for the entire `.await` (dropped on return). **Since D42 the body is no
    /// longer byte-identical to the macro's**: [`Self::enforce_request_quota`] runs first, INSIDE the
    /// permit. It is inside deliberately — the quota walk is O(request bytes) of CPU, so running it
    /// above `try_acquire` would let unbounded concurrent oversized requests burn CPU outside the
    /// D34-F5 bound, inverting the "the permit gates the WHOLE dispatch" invariant. On saturation it
    /// FAST-FAILS in-band (MF-5 — tools have an in-band channel, unlike resources)
    /// with a retryable [`ErrorCode::RateLimited`] rather than backpressuring. The rate-limit
    /// `Semaphore` sits STRICTLY ABOVE the engine write `Semaphore(1)` (§4.2), so this non-blocking
    /// acquire can never join a wait cycle (deadlock-free vs D14/D31).
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let response = self.call_tool_gated(request, context).await;
        #[cfg(feature = "test-util")]
        if let Some(returned) = &self.returned {
            returned.add_permits(1);
        }
        response
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
    ///
    /// It returns [`ServerConfig`], the name rmcp 3.4 gave this type when it deprecated `ServerInfo`
    /// (D59 clause (14)). Its `protocol_version` stays rmcp's default; rmcp 3.10's
    /// `negotiate_protocol_version` answers an `initialize` with the client's version when it is a
    /// supported handshake version and with 2025-11-25 otherwise (D59 clause (9)).
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .enable_resources()
            .build();
        let instructions = self.instructions.clone().unwrap_or_else(|| {
            // The counts are DERIVED from `capabilities()` — never embedded literals. This string
            // is the LIVE, agent-visible handshake and is not golden-pinned, so a hand-maintained
            // literal here would silently ship a stale lie the moment the surface changes
            // (`instructions_counts_match_capabilities` below is the backstop).
            let caps = crate::resources::capabilities();
            format!(
                "unblock MCP server (contract {CONTRACT_VERSION}). {} tools, {} resources, {} prompts.",
                caps.tools.len(),
                caps.resources.len(),
                caps.prompts.len(),
            )
        });
        ServerConfig::new(capabilities)
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
            Resource::new(uri, name)
                .with_description(description)
                .with_mime_type("application/json")
        })
        .collect();
        Ok(ListResourcesResult::with_all_items(resources))
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
            ResourceTemplate::new("unblock://issues/{id}", "issue-by-id")
                .with_description("A single issue by id.")
                .with_mime_type("application/json"),
        ];
        Ok(ListResourceTemplatesResult::with_all_items(templates))
    }

    /// Read a resource by URI (read-only, FR-10). A miss → `resource_not_found` (-32002, which rmcp
    /// 3.10 rewrites to `-32602` with the same `data` for a request on the 2026-07-28 revision, D59
    /// clause (9)); a domain error is surfaced as `ErrorData` (resources have no in-band channel like
    /// tools do).
    ///
    /// The body is stamped `mimeType: application/json` (CD-5) — the same type `resources/list` and
    /// `resources/templates/list` advertise. rmcp's [`ResourceContents::text`] defaults to
    /// `"text/plain"` (`rmcp-3.10.0/src/model/resource.rs:193-199`), so we override it via
    /// [`ResourceContents::with_mime_type`]; the body bytes are unchanged.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
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
            ])
            .into()),
            Err(structured) => Err(crate::error::to_rmcp_error_data(&structured)),
        }
    }
}

/// **The D43 duplicate-key gate — the single ENFORCEMENT site.**
///
/// The scanning transport ([`crate::wire::DupScanningTransport`]) stamps a [`ParamsScan`] verdict on
/// EVERY decoded request, `resources/*` and `prompts/*` included, because the scan runs on the raw
/// bytes before rmcp even classifies the method. The verdict is **consulted here and nowhere else**:
/// only `tools/call` has an in-band channel (FR-11), so gating another method would have to answer
/// out-of-band, reopening the `-32602` arm that `crates/unblock-cli/tests/error_channel.rs` exists
/// to keep shut. Non-`tools/call` methods therefore carry a stamped-but-unenforced verdict — a
/// documented residual, not an oversight. (`read_resource` was probed: a duplicated `uri` does not
/// execute last-wins; it falls to the already non-executing `-32601` class.)
///
/// **All three non-`Clean` arms reject, INCLUDING the absent one.** `Extensions` starts empty, so
/// treating absent as clean would make any un-scanned path fail OPEN. The one in-tree path that
/// reaches a handler with no verdict is the test-only, `test-util`-gated
/// [`mcp_server_duplex_raw_for_test`] (deliberately raw — see its docs); that path is what
/// makes this arm executably testable instead of merely asserted.
///
/// Reads `context.extensions` DIRECTLY rather than through rmcp's `Extension<T>` extractor: that
/// extractor's absent arm is `ErrorData::invalid_params(...)` ⇒ **`-32602`**, i.e. exactly the
/// out-of-band arm this whole design keeps shut.
///
/// # Errors
/// The FR-11 [`StructuredError`] for the offending verdict (see [`crate::tools::args`]).
fn frame_scan_gate(extensions: &rmcp::model::Extensions) -> Result<(), StructuredError> {
    match extensions.get::<ParamsScan>() {
        Some(ParamsScan::Clean) => Ok(()),
        Some(ParamsScan::Duplicate { key, path }) => Err(duplicate_key_error(key, path)),
        Some(ParamsScan::Indeterminate) => Err(indeterminate_frame_error()),
        None => Err(unscanned_frame_error()),
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
/// [`McpServerOptions::cancel`] token. The `session` is shared as `Arc<Session>` (the engine owns the
/// write Semaphore, D14).
///
/// # Cancellation is TWO-outcome (NORMATIVE — spine §0.1, D38)
///
/// A `cancel()` drains in-flight work and returns `Ok(())` **only if the rmcp `initialize` handshake
/// had already completed**. rmcp's `serve_server_with_ct` wraps the WHOLE handshake in a `select!`
/// against the token, so a cancel landing **during** the handshake instead returns
/// [`McpServerError::Transport`] (wrapping `ServerInitializeError::Cancelled`).
///
/// **Both are normal cooperative-shutdown outcomes.** A caller MUST NOT treat that `Err` as an
/// independent fault: it must still run its clean teardown (`Session::shutdown()`), and if a signal
/// was recorded the exit is `128+signo` regardless of which outcome occurred (spine §5b — the CLI's
/// `commands/mcp.rs` is the reference consumer). Assuming the unconditional `Ok` this doc previously
/// claimed is what made `unblock mcp` hang forever on a pre-handshake SIGTERM (PRD §4/D38).
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize/bind — **including the
///   normal `Cancelled` outcome above**, which is a shutdown signal, not a failure.
/// - [`McpServerError::RunLoop`] if the run loop ends abnormally (the background task is aborted).
pub async fn run_mcp_server(
    session: Arc<Session>,
    opts: McpServerOptions,
) -> Result<(), McpServerError> {
    let server = UnblockServer::new(session, opts.quotas, opts.instructions);
    let (read, write) = stdio();
    let running = run_mcp_server_handler(server, read, write, opts.cancel).await?;
    running.waiting().await.context(RunLoopSnafu)?;
    Ok(())
}

/// Generic over the BYTE STREAMS so the lifecycle test can drive an in-memory duplex pair.
///
/// # Why the bound is byte-level since D43
///
/// It used to take an `IntoTransport`. It cannot any more: the duplicate-key scan has to see the
/// raw frame bytes BEFORE `serde_json` collapses a duplicated key, and no `Transport` decorator can
/// — `receive()` hands a decorator an already-parsed message. So this function now OWNS the read
/// framing, via [`DupScanningTransport`].
///
/// The composition is `PreHandshakeGateTransport<DupScanningTransport<R, W>>` (D59 clause (10)):
/// scan innermost, the D50 gate above it, so the receive order is scan, then gate. The gate sits
/// ABOVE the scan because a line that fails the typed parse is answered by the scanner's three
/// rules (D59 clause (2)), and a readable id lives only in the raw bytes. The CD-4 version clamp
/// that once wrapped both is RETIRED: rmcp 3.10 negotiates the `initialize` version per the
/// specification itself (`rmcp-3.10.0/src/service/server.rs:478-509`). The gate is built with
/// [`UnblockServer`]'s own `supported_protocol_versions()`, the set rmcp checks an inline version
/// against, so the gate's stateless edge and rmcp's agree on it.
///
/// [`run_mcp_server`]'s public 2-argument signature is UNCHANGED by this — the byte-level bound is
/// confined to this private function and the `test-util` duplex helper.
async fn run_mcp_server_handler<R, W>(
    server: UnblockServer,
    read: R,
    write: W,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<rmcp::service::RunningService<RoleServer, UnblockServer>, McpServerError>
where
    UnblockServer: Service<RoleServer>,
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let supported = ServerHandler::supported_protocol_versions(&server);
    let transport =
        PreHandshakeGateTransport::new(DupScanningTransport::new(read, write), supported);
    server
        .serve_with_ct(transport, cancel)
        .await
        .context(TransportSnafu)
}

/// Build and run the server over an arbitrary in-memory byte-stream pair (TEST-ONLY, `test-util`).
///
/// Drives the **same** `UnblockServer` + `serve_with_ct` path as [`run_mcp_server`], but over a caller-
/// supplied duplex pair instead of stdio — so the M2 lifecycle exit-gate (`tests/lifecycle.rs`)
/// can run a full in-process MCP client/server flow without touching real stdio. Feature-gated and
/// `#[doc(hidden)]` so it never widens the shipped public surface.
///
/// Since D43 it takes a `(read, write)` PAIR rather than an `IntoTransport`: the scanning transport
/// owns the read framing, so it must be handed the byte streams themselves. A caller holding a
/// single duplex stream splits it with `tokio::io::split`.
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize over the transport.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub async fn mcp_server_duplex_for_test<R, W>(
    session: Arc<Session>,
    quotas: Quotas,
    instructions: Option<String>,
    read: R,
    write: W,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<rmcp::service::RunningService<RoleServer, UnblockServer>, McpServerError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let server = UnblockServer::new(session, quotas, instructions);
    run_mcp_server_handler(server, read, write, cancel).await
}

/// Build and run the server over an arbitrary in-memory transport on the RAW rmcp serve path
/// (TEST-ONLY, `test-util` feature).
///
/// Unlike [`mcp_server_duplex_for_test`] (which routes through [`run_mcp_server_handler`]), this
/// helper calls `serve_with_ct` on the caller's transport DIRECTLY. It installs **NO
/// duplicate-key scan**, so every request reaching a handler through it carries NO [`ParamsScan`]
/// verdict at all, and **NO pre-lifecycle gate**, so a premature frame sent through it still reaches
/// rmcp's pre-lifecycle loop. NEVER route a shipped path through this.
///
/// Its one remaining use is D43's fail-closed absent-verdict cell: it is the only honest in-tree
/// witness that [`frame_scan_gate`]'s ABSENT arm actually fires (`tests/`). It took this name when
/// the CD-4 clamp it once bypassed was retired (D59 clause (10)).
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize over the transport.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub async fn mcp_server_duplex_raw_for_test<T, E, A>(
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

/// Like [`mcp_server_duplex_for_test`], but every returned `call_tool` adds one permit to `returned`
/// — the HELD-HANDLER-RETURNED signal of PRD §4 D59 criterion (17) (TEST-ONLY, `test-util`). The
/// permit is added before rmcp's send task writes the response; see [`UnblockServer`]'s `returned`.
///
/// # Errors
/// - [`McpServerError::Transport`] if the rmcp service fails to initialize over the transport.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub async fn mcp_server_duplex_with_returned_signal_for_test<R, W>(
    session: Arc<Session>,
    quotas: Quotas,
    read: R,
    write: W,
    cancel: tokio_util::sync::CancellationToken,
    returned: Arc<Semaphore>,
) -> Result<rmcp::service::RunningService<RoleServer, UnblockServer>, McpServerError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let mut server = UnblockServer::new(session, quotas, None);
    server.returned = Some(returned);
    run_mcp_server_handler(server, read, write, cancel).await
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
    use super::{ParamsScan, UnblockServer, frame_scan_gate, run_mcp_server_handler};
    use crate::envelope_id_corpus::{ParseExpect, parse_error_bytes, parse_error_corpus};
    use crate::options::Quotas;
    use rmcp::model::Extensions;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};
    use unblock_engine::{Session, SessionConfig};
    use unblock_error::ErrorCode;
    use unblock_storage::{LibsqlStorage, Storage};

    const fn assert_send_sync<T: Send + Sync + 'static>() {}

    /// A `Session` over a fresh in-memory libsql backend, mirroring the integration harness.
    async fn session() -> Arc<Session> {
        let storage = LibsqlStorage::open_in_memory()
            .await
            .expect("open in-memory storage");
        storage.migrate().await.expect("migrate");
        let storage: Arc<dyn Storage> = Arc::new(storage);

        let workspace_dir = std::path::PathBuf::from("/tmp/unblock-mcp-composition-cell");
        let unblock_dir = workspace_dir.join(".unblock");
        let mut config = ResolvedConfig::default();
        config.runtime.actor = "tester".to_string();
        config.runtime.sources.actor = unblock_model::ConfigSource::OsUser;
        let paths = ConfigPaths {
            db_path: unblock_dir.join(&config.startup.db_filename),
            jsonl_path: unblock_dir.join(&config.startup.jsonl_filename),
            unblock_dir,
        };
        let ctx = WorkspaceContext {
            storage,
            workspace_dir,
            config,
            reloader: unblock_config::ConfigReloader::new(
                unblock_config::CliOverrides::default(),
                unblock_config::EnvOverrides::default(),
                Some("tester".to_string()),
                None,
                None,
            ),
            paths,
            source: WorkspaceSource::WalkUp,
            schema_version_before_migrate: 0,
        };
        Arc::new(
            Session::open(ctx, SessionConfig::default())
                .await
                .expect("open session"),
        )
    }

    /// **[v1.0.1/D54]** An in-class `-32700` line arriving as the FIRST frame is answered ON its id by
    /// the scanner — below the D50 gate, which never sees it — and the handshake still completes.
    ///
    /// The frame is parse-error corpus entry E23, the shape `pre_handshake.rs` names: an `initialize`
    /// whose scalar `params` fails every variant. A cell asserting the GATE did something with it
    /// would be vacuous, so this one asserts only the written bytes.
    ///
    /// Mutants: the recovered arm answering `None`; D54 handling moved into the gate, where the frame
    /// never arrives; the `Err` arm returning `None` instead of looping to the next line.
    #[tokio::test]
    async fn a_parse_error_frame_before_the_handshake_is_answered_on_its_id_and_the_handshake_survives()
     {
        let corpus = parse_error_corpus();
        let entry = corpus.iter().find(|f| f.id == "E23").expect("E23 missing");
        assert_eq!(
            entry.expect,
            ParseExpect::RecoveredNum(5),
            "E23 must recover id 5"
        );

        let (mut client, server_io) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_io);
        client.write_all(&entry.frame).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client
            .write_all(br#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"d54","version":"0"}}}"#)
            .await
            .unwrap();
        client.write_all(b"\n").await.unwrap();
        client.shutdown().await.unwrap();

        let server = UnblockServer::new(session().await, Quotas::default(), None);
        run_mcp_server_handler(
            server,
            server_read,
            server_write,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("the server survives an in-class first frame");

        let mut written = Vec::new();
        client.read_to_end(&mut written).await.unwrap();
        let text = String::from_utf8_lossy(&written).to_string();
        let mut lines = text.split_inclusive('\n');
        assert_eq!(
            lines.next().map(str::as_bytes),
            Some(parse_error_bytes(&entry.expect).as_slice()),
            "the first reply must be the -32700 ON id 5: {text}"
        );
        let second = lines.next().expect("the handshake is answered");
        assert!(
            second.contains(r#""id":2"#) && second.contains(r#""result""#),
            "the initialize is answered after the parse error: {second}"
        );
    }

    /// **The `Indeterminate ⇒ reject` arm — its only executable coverage.**
    ///
    /// The wire path cannot reach this arm: the shapes that make the scanner indeterminate
    /// (malformed JSON, non-UTF-8, nesting past the 128-level recursion limit) also fail rmcp's own
    /// parse, so the frame is answered `-32700` and never reaches `call_tool`. The arm is
    /// defence-in-depth, which is exactly why it needs a direct pin — an uncovered fail-closed arm
    /// is one refactor away from silently becoming fail-open.
    ///
    /// This lives IN-CRATE because `ParamsScan` and `frame_scan_gate` are both crate-private: an
    /// external `tests/*.rs` binary is a separate compilation unit and could not construct the
    /// verdict at all.
    #[test]
    fn frame_scan_gate_rejects_an_indeterminate_verdict() {
        let mut extensions = Extensions::new();
        extensions.insert(ParamsScan::Indeterminate);
        let error = frame_scan_gate(&extensions).expect_err("indeterminate must REJECT");
        assert_eq!(error.code, ErrorCode::ValidationFailed);
        assert_eq!(
            error.context.get("kind").and_then(|v| v.as_str()),
            Some("indeterminate_frame")
        );
    }

    /// The ABSENT verdict rejects — an empty `Extensions` is the DEFAULT state, so the opposite
    /// encoding would make every un-scanned path fail OPEN.
    #[test]
    fn frame_scan_gate_rejects_an_absent_verdict() {
        let error = frame_scan_gate(&Extensions::new()).expect_err("absent must REJECT");
        assert_eq!(error.code, ErrorCode::InternalError);
        assert_eq!(
            error.context.get("kind").and_then(|v| v.as_str()),
            Some("unscanned_frame")
        );
        assert!(!error.retryable, "an un-scanned frame is not transient");
    }

    /// A duplicate verdict rejects and NAMES both the key and its RFC 6901 pointer.
    #[test]
    fn frame_scan_gate_rejects_a_duplicate_and_names_the_pointer() {
        let mut extensions = Extensions::new();
        extensions.insert(ParamsScan::Duplicate {
            key: "action".to_string(),
            path: "/arguments/deps/0".to_string(),
        });
        let error = frame_scan_gate(&extensions).expect_err("duplicate must REJECT");
        assert_eq!(error.code, ErrorCode::ValidationFailed);
        assert_eq!(
            error.context.get("kind").and_then(|v| v.as_str()),
            Some("duplicate_key")
        );
        assert_eq!(
            error.context.get("field").and_then(|v| v.as_str()),
            Some("action")
        );
        assert_eq!(
            error.context.get("path").and_then(|v| v.as_str()),
            Some("/arguments/deps/0")
        );
    }

    /// A clean verdict proceeds — without this the gate could "pass" by rejecting everything.
    #[test]
    fn frame_scan_gate_admits_a_clean_verdict() {
        let mut extensions = Extensions::new();
        extensions.insert(ParamsScan::Clean);
        assert!(frame_scan_gate(&extensions).is_ok());
    }

    /// An oversized attacker key/pointer is CLIPPED before it is echoed.
    ///
    /// `with_context` performs no sanitization at all, so the caller is the only bound — and the
    /// pointer is clipped ASSEMBLED, never per segment (many short segments could otherwise sum to
    /// an unbounded total).
    #[test]
    fn an_oversized_duplicate_payload_is_clipped() {
        let mut extensions = Extensions::new();
        extensions.insert(ParamsScan::Duplicate {
            key: "k".repeat(64 * 1024),
            path: format!("/{}", vec!["seg"; 4096].join("/")),
        });
        let error = frame_scan_gate(&extensions).expect_err("duplicate must REJECT");
        let bound = unblock_error::MAX_ECHOED_BYTES + unblock_error::TRUNCATION_MARKER.len();
        for field in ["field", "path"] {
            let echoed = error.context[field].as_str().unwrap_or_default();
            assert!(
                echoed.len() <= bound,
                "`context.{field}` echoed {} bytes, over the {bound}-byte bound",
                echoed.len()
            );
        }
        assert!(
            error.message.len() <= 6 * unblock_error::MAX_ECHOED_BYTES + 128,
            "the message must stay bounded too: {} bytes",
            error.message.len()
        );
    }

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
