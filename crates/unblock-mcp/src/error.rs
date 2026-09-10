//! The crate's error surface: [`McpServerError`] (server lifecycle only) + the boundary mappers.
//!
//! **Domain errors are NOT here** — they flow *in-band* as the shared structured error
//! (`is_error=true`, FR-11). This module owns only:
//!
//! - [`McpServerError`] — the snafu enum for server lifecycle/transport faults (the public type the
//!   CLI handles from [`crate::run_mcp_server`]).
//! - [`engine_error_to_structured`] — the spine §2.4/§5.6 boundary mapper: exactly `(&err).into()`
//!   (the blanket `From<&EngineError> for StructuredError`, F-6). Every `EngineError` variant is
//!   covered (it is `CodedError`), yielding an already-sanitized [`StructuredError`]; the JSON
//!   boundary makes `context` terminal-safe (serde escapes it) so no extra sanitize is needed here.
//! - [`to_rmcp_error_data`] — the RESOURCE-boundary mapper (T2.6/D25/F-2). Resources have no in-band
//!   channel like tools do, so a `read_resource` failure surfaces as an `ErrorData`: a not-found
//!   ([`unblock_error::ErrorCode::IssueNotFound`] — a missing `{id}` or an unknown URI) maps to
//!   `ErrorData::resource_not_found` (-32002); every other code maps to -32603 (a true internal fault,
//!   OR — since D34/MF-5 — the retryable `RateLimited` capacity cap, which shares that transport code
//!   but is distinguished by its structured `data.code`). The full structured payload rides `data` on
//!   both arms.

use rmcp::model::{ErrorCode as RmcpErrorCode, ErrorData};
use snafu::Snafu;
use unblock_engine::EngineError;
use unblock_error::{StructuredError, clip};

/// Server lifecycle/transport errors surfaced by [`crate::run_mcp_server`] (NOT domain errors).
///
/// Per-tool domain failures are returned *in-band* as the shared structured error (`is_error=true`,
/// always-valid JSON, FR-11); this enum is reserved for the server's own lifecycle — binding the stdio
/// transport or the run loop failing.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum McpServerError {
    /// The stdio transport could not be bound / the rmcp service failed to initialize.
    ///
    /// The source renders through [`describe_initialize_error`] (D49), so a refused first frame is
    /// DESCRIBED — its kind, its method, its envelope id — and never echoed.
    #[snafu(display(
        "failed to start the MCP server: {}",
        describe_initialize_error(source)
    ))]
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

/// Render an rmcp `ServerInitializeError` as a BOUNDED description (D49).
///
/// ONE function covers EVERY variant, so the arm that echoes client bytes today and any variant a
/// future rmcp adds are both bounded here. A refused first frame contributes at most three things —
/// the frame KIND, the METHOD name and the envelope ID — and every client-supplied string member
/// goes through [`unblock_error::clip`] before Rust's string `Debug` quotes it. `params`, `result`,
/// `error.message` and `error.data` are never rendered, in any form.
///
/// `TransportError` gets its own arm because a clip over rmcp's own text cuts inside a type name.
/// rmcp renders that variant as `Send message error {error}, when {context}`
/// (`rmcp-1.7.0/src/service/server.rs:75`), and its `{error}` is a nested `DynamicTransportError`
/// whose own `Display` is `Transport [{transport_name}] error: {error}`
/// (`rmcp-1.7.0/src/transport.rs:238`). `transport_name` is the TYPE NAME `std::any::type_name`
/// fills, and it runs past the 128-byte bound alone, so clipping rmcp's whole string hides the I/O
/// reason the operator needs.
///
/// The wildcard clips rmcp's own `Display` WITHOUT the `Debug` quoting, because rmcp's text is a
/// sentence rather than a client member. That omission is what keeps `Cancelled` byte-identical to
/// the line D38's diagnostic routing measured.
fn describe_initialize_error(err: &rmcp::service::ServerInitializeError) -> String {
    use rmcp::service::ServerInitializeError;

    match err {
        ServerInitializeError::ExpectedInitializeRequest(Some(frame)) => describe_frame(frame),
        ServerInitializeError::ExpectedInitializeRequest(None) => {
            "expected the initialize request, but no frame arrived".to_string()
        }
        ServerInitializeError::TransportError { error, context } => {
            let inner = error.error.to_string();
            let clipped_context = clip(context);
            let clipped_inner = clip(&inner);
            format!("a transport error while {clipped_context}: {clipped_inner}")
        }

        // Every other variant — including the deprecated one, which is never named here, and any
        // variant a future rmcp adds — arrives with rmcp's own sentence and leaves clipped.
        _ => {
            let rendered = err.to_string();
            clip(&rendered).into_owned()
        }
    }
}

/// Describe the refused first frame — its kind, its method and its envelope id, nothing else.
///
/// Which shape carries which member is read off rmcp's own structs. A request and a response both
/// require an id, a notification declares no id field at all, and only an error frame's id is
/// optional, so only that arm needs the "with no id" spelling.
fn describe_frame(frame: &rmcp::model::ClientJsonRpcMessage) -> String {
    use rmcp::model::JsonRpcMessage;

    match frame {
        JsonRpcMessage::Request(request) => {
            let method = quote_clipped(request.request.method());
            let id = render_id(&request.id);
            format!(
                "expected the initialize request, but the first frame was a request {method} with id {id}"
            )
        }
        JsonRpcMessage::Notification(notification) => {
            let method = quote_clipped(notification_method(&notification.notification));
            format!(
                "expected the initialize request, but the first frame was a notification {method}"
            )
        }
        JsonRpcMessage::Response(response) => {
            let id = render_id(&response.id);
            format!("expected the initialize request, but the first frame was a response to id {id}")
        }
        JsonRpcMessage::Error(error) => match &error.id {
            Some(id) => {
                let id = render_id(id);
                format!(
                    "expected the initialize request, but the first frame was an error response to id {id}"
                )
            }
            None => "expected the initialize request, but the first frame was an error response with no id"
                .to_string(),
        },
    }
}

/// The method name of a client notification.
///
/// rmcp exposes a `method()` accessor for requests only, so this match is hand-written — and it is
/// exhaustive, so an rmcp release that adds a sixth variant is a compile error rather than a silent
/// hole. The four typed arms return their own variant's const method through `ConstString::as_str`,
/// so the literal is rmcp's rather than ours.
fn notification_method(notification: &rmcp::model::ClientNotification) -> &str {
    use rmcp::model::{ClientNotification, ConstString as _};

    match notification {
        ClientNotification::CancelledNotification(n) => n.method.as_str(),
        ClientNotification::ProgressNotification(n) => n.method.as_str(),
        ClientNotification::InitializedNotification(n) => n.method.as_str(),
        ClientNotification::RootsListChangedNotification(n) => n.method.as_str(),
        ClientNotification::CustomNotification(n) => n.method.as_str(),
    }
}

/// Render an envelope id.
///
/// A NUMBER prints through `NumberOrString`'s own `Display`, unquoted and unclipped — an `i64`
/// renders at most 20 bytes of its own digits and carries no attacker text. A STRING is a client
/// member like any other, so it is clipped and then quoted.
fn render_id(id: &rmcp::model::NumberOrString) -> String {
    use rmcp::model::NumberOrString;

    match id {
        NumberOrString::Number(_) => id.to_string(),
        NumberOrString::String(text) => quote_clipped(text),
    }
}

/// Clip a client-supplied string, then quote it with Rust's string `Debug`.
///
/// The cut precedes the escape, so it can never land inside an escape sequence and yield a
/// misleading fragment. `Debug` then supplies both the surrounding quotes and the
/// control-character escaping, which leaves the downstream `sanitize_message` chokepoint nothing to
/// expand on these arms.
fn quote_clipped(value: &str) -> String {
    let clipped = clip(value);
    format!("{clipped:?}")
}

impl McpServerError {
    /// Is this the rmcp **cancellation outcome** — i.e. a shutdown that was ASKED FOR, not a fault?
    ///
    /// `true` only for [`McpServerError::Transport`] wrapping `ServerInitializeError::Cancelled`,
    /// which rmcp 1.7 returns from the OUTER `select!` arm of `serve_server_with_ct` when the
    /// caller's `CancellationToken` cancels an INCOMPLETE `initialize` handshake (spine §0.1). It is
    /// one of the two normal cooperative-shutdown outcomes (`Ok(())` is the other), so a caller that
    /// already knows a signal was recorded can report it as routine rather than blaming the process
    /// for obeying (D38 — the CLI's `commands/mcp.rs` routes it to `tracing::debug!` instead of an
    /// `error[CODE]` stderr line, while a GENUINE post-signal error keeps that line).
    ///
    /// **Deliberately NARROW — this matches what was MEASURED, not a plausible story.** The observed
    /// pre-handshake-signal child stderr is verbatim `failed to start the MCP server: Cancelled`.
    /// `ConnectionClosed(_)` is NOT part of THIS predicate — it is the peer hanging up (a client that
    /// disconnects before `initialize`, an EOF, not a cancellation), so folding it in here would make
    /// this predicate's NAME lie. It has its OWN peer predicate [`is_pre_handshake_disconnect`]
    /// ([`McpServerError::is_pre_handshake_disconnect`], D40); this one stays `Cancelled`-only.
    ///
    /// **Reconciled at D40 (T3.2.1 follow-up (b)):** an earlier draft said folding `ConnectionClosed`
    /// in "would demote a real hangup". D40 does now demote the pre-`initialize` disconnect — but via
    /// the SEPARATE predicate, and additionally flips its unsignalled exit code from 1 to 0 (a routine
    /// peer disconnect is not an internal fault, spine §5b). `is_cancellation()` is unchanged; adding a
    /// peer predicate rather than widening this one keeps the D38 "narrow and measured" discipline.
    ///
    /// Additive on a `#[non_exhaustive]` enum: no variant is added or changed, so this is not a
    /// contract event (no `CONTRACT_HASH`/`CONTRACT_VERSION` bump — D38).
    #[must_use]
    pub fn is_cancellation(&self) -> bool {
        match self {
            Self::Transport { source } => {
                matches!(**source, rmcp::service::ServerInitializeError::Cancelled)
            }
            Self::RunLoop { .. } => false,
        }
    }

    /// Is this the rmcp **pre-`initialize` peer disconnect** — i.e. the client closed the connection
    /// before completing the MCP `initialize` handshake, with no internal fault?
    ///
    /// `true` only for [`McpServerError::Transport`] wrapping
    /// `ServerInitializeError::ConnectionClosed(_)`. rmcp raises it from the handshake path's
    /// `expect_next_message`, which maps the transport's `receive() == None` to `ConnectionClosed`.
    /// `AsyncRwTransport::receive` returns `None` on a clean EOF, on a read IO error (which it logs via
    /// `tracing::error!` FIRST — nothing swallowed), and on garbage-then-peer-close; ALL collapse here.
    ///
    /// This is the D40 (T3.2.1 follow-up (b)) seam: on the UNSIGNALLED path the CLI's `resolve_mcp_exit`
    /// uses it to intercept the disconnect and delegate the exit code to `session.shutdown()` — a clean
    /// teardown → exit 0 (a routine peer disconnect is not an internal fault, unifying with the
    /// post-handshake EOF; spine §5b), a failing teardown still decides via its own 0–8 code. It is a
    /// SEPARATE predicate from [`is_cancellation`](McpServerError::is_cancellation) (which stays
    /// `Cancelled`-only): the two disjoint outcomes travel independently.
    ///
    /// Additive on a `#[non_exhaustive]` enum: no variant is added or changed (no
    /// `CONTRACT_HASH`/`CONTRACT_VERSION` bump — D40).
    #[must_use]
    pub fn is_pre_handshake_disconnect(&self) -> bool {
        match self {
            Self::Transport { source } => matches!(
                **source,
                rmcp::service::ServerInitializeError::ConnectionClosed(_)
            ),
            Self::RunLoop { .. } => false,
        }
    }

    /// Build a genuine [`McpServerError::Transport`] for tests (TEST-ONLY, `test-util` feature).
    ///
    /// Produces a REAL `ServerInitializeError::TransportError` (via the public
    /// [`rmcp::service::ServerInitializeError::transport`] constructor over the concrete
    /// `AsyncRwTransport` whose `Error` is [`std::io::Error`]) — NOT a fabricated placeholder. This
    /// exists solely so a downstream crate (`unblock-cli`'s exit-code boundary test) can construct
    /// the `Transport` arm to prove its D27/AF-4 mapping (`→ InternalError`, exit 1). The enum stays
    /// `#[non_exhaustive]`; this seam is feature-gated + `#[doc(hidden)]`, so the shipped public API
    /// is unchanged.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub fn __transport_error(message: &str) -> Self {
        use rmcp::RoleServer;
        use rmcp::service::ServerInitializeError;
        use snafu::IntoError as _;

        // A real transport error whose `T::Error` is `std::io::Error` (AsyncRwTransport's Error type),
        // wrapped through the public `transport` constructor into a genuine `ServerInitializeError`.
        type IoTransport = rmcp::transport::async_rw::AsyncRwTransport<
            RoleServer,
            tokio::io::DuplexStream,
            tokio::io::DuplexStream,
        >;
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, message.to_string());
        let init_err =
            ServerInitializeError::transport::<IoTransport>(io_err, "test-util transport");
        TransportSnafu.into_error(init_err)
    }

    /// Build a genuine CANCELLATION [`McpServerError::Transport`] for tests (TEST-ONLY,
    /// `test-util` feature) — the REAL `ServerInitializeError::Cancelled` rmcp returns when a
    /// cancel lands during the `initialize` handshake (spine §0.1), NOT a look-alike.
    ///
    /// Exists so `unblock-cli` can prove BOTH branches of the D38 post-signal diagnostic routing
    /// (cancellation → `tracing::debug!`; genuine → the `error[CODE]` stderr line) against the same
    /// error the live path produces. Same gating/rationale as
    /// [`McpServerError::__transport_error`].
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub fn __cancelled_error() -> Self {
        use rmcp::service::ServerInitializeError;
        use snafu::IntoError as _;

        TransportSnafu.into_error(ServerInitializeError::Cancelled)
    }

    /// Build a genuine pre-`initialize` DISCONNECT [`McpServerError::Transport`] for tests (TEST-ONLY,
    /// `test-util` feature) — the REAL `ServerInitializeError::ConnectionClosed(_)` rmcp returns when
    /// the peer closes the connection before completing the `initialize` handshake (`receive() == None`
    /// → `ConnectionClosed`), NOT a look-alike.
    ///
    /// Exists so `unblock-cli` can prove the D40 (T3.2.1 follow-up (b)) exit-0 delegation against the
    /// same error the live path produces. Same gating/rationale as
    /// [`McpServerError::__transport_error`].
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub fn __connection_closed_error() -> Self {
        use rmcp::service::ServerInitializeError;
        use snafu::IntoError as _;

        TransportSnafu.into_error(ServerInitializeError::ConnectionClosed(
            "initialize request".to_string(),
        ))
    }

    /// Build the REFUSED-FIRST-FRAME [`McpServerError::Transport`] for tests (TEST-ONLY,
    /// `test-util` feature) — the REAL `ServerInitializeError::ExpectedInitializeRequest(Some(_))`
    /// rmcp returns when a first frame is not the `initialize` request.
    ///
    /// `raw_frame_json` is deserialized into rmcp's own `ClientJsonRpcMessage`, so the cell drives
    /// the same four shapes the wire produces rather than a hand-built look-alike. Malformed JSON
    /// PANICS with the serde error, which is the tests-excepted arm of this repo's
    /// no-`unwrap`/`expect` rule — this constructor is never in a shipped build. Same
    /// gating/rationale as [`McpServerError::__transport_error`].
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub fn __expected_initialize_request(raw_frame_json: &str) -> Self {
        use rmcp::model::ClientJsonRpcMessage;
        use rmcp::service::ServerInitializeError;
        use snafu::IntoError as _;

        let frame: ClientJsonRpcMessage = serde_json::from_str(raw_frame_json)
            .expect("the raw frame must deserialize as an rmcp ClientJsonRpcMessage");
        TransportSnafu.into_error(ServerInitializeError::ExpectedInitializeRequest(Some(
            frame,
        )))
    }

    /// Build a genuine [`McpServerError::RunLoop`] for tests (TEST-ONLY, `test-util` feature).
    ///
    /// Produces a REAL [`tokio::task::JoinError`] by aborting a spawned task and awaiting its handle
    /// (the exact join-error the server's `running.waiting().await` surfaces on an aborted run loop),
    /// then wraps it through the `RunLoop` context selector. Same rationale/gating as
    /// [`McpServerError::__transport_error`].
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub async fn __run_loop_error() -> Self {
        use snafu::IntoError as _;

        // Abort a spawned task to obtain a genuine `JoinError` (a cancelled join), identical in shape
        // to the one the aborted rmcp run-loop task yields.
        let handle = tokio::spawn(async {
            // Park until aborted — never completes on its own.
            std::future::pending::<()>().await;
        });
        handle.abort();
        let join_err = handle
            .await
            .expect_err("an aborted task must yield a JoinError");
        RunLoopSnafu.into_error(join_err)
    }
}

/// Map an [`EngineError`] to a sanitized [`StructuredError`] at the MCP boundary (spine §2.4/§5.6).
///
/// Exactly `(&err).into()` — the blanket `From<&EngineError> for StructuredError` (F-6). The engine
/// error is `CodedError`, so this composes the union error → one `ErrorCode` →
/// `code`/`message`/`hint`/`retryable`/`context`, with a terminal-sanitized message/hint.
pub(crate) fn engine_error_to_structured(err: &EngineError) -> StructuredError {
    err.into()
}

/// Map a [`StructuredError`] to an rmcp [`ErrorData`] at the **`read_resource` boundary** (T2.6/D25/F-2).
///
/// Resources have no in-band channel like tools do (which return `CallToolResult::structured_error`,
/// FR-11), so a `read_resource` failure surfaces as an `ErrorData`. A not-found —
/// [`unblock_error::ErrorCode::IssueNotFound`], built for a missing `{id}` (`resources/issues.rs`) or
/// an unknown URI (`server::unknown_resource`) — maps to `ErrorData::resource_not_found` (-32002, the
/// pinned rmcp contract, `rmcp-1.7.0` `model.rs:544`). Every other code reaching this boundary maps to
/// `INTERNAL_ERROR` (-32603) — a true internal fault, OR (since D34/MF-5) the retryable `RateLimited`
/// capacity cap, which shares the -32603 transport code but is distinguished by its structured
/// `data.code`/`data.retryable`. The full structured payload is attached as `data` on BOTH arms, so a
/// client still sees `code`/`message`/`hint`/`retryable`/`context`.
pub(crate) fn to_rmcp_error_data(structured: &StructuredError) -> ErrorData {
    let data = serde_json::to_value(structured).ok();
    match structured.code {
        // Not-found at the read_resource boundary → -32002 (the pinned rmcp contract).
        unblock_error::ErrorCode::IssueNotFound => {
            ErrorData::resource_not_found(structured.message.clone(), data)
        }
        // Everything else → -32603: a true internal fault, or the retryable `RateLimited` capacity cap
        // (D34/MF-5) — the structured `code`/`retryable` ride `data`, so the client can distinguish.
        _ => ErrorData::new(
            RmcpErrorCode::INTERNAL_ERROR,
            structured.message.clone(),
            data,
        ),
    }
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

    #[test]
    fn issue_not_found_maps_to_resource_not_found() {
        use unblock_error::StructuredError;
        let structured = StructuredError::from_code(ErrorCode::IssueNotFound, "nope");
        let data = to_rmcp_error_data(&structured);
        assert_eq!(
            data.code.0, -32002,
            "IssueNotFound → -32002 resource_not_found"
        );
        let payload = data.data.expect("structured payload attached");
        assert_eq!(payload["code"], "ISSUE_NOT_FOUND");
    }

    #[test]
    fn non_not_found_codes_map_to_internal_error() {
        use unblock_error::StructuredError;
        for code in [ErrorCode::NotInitialized, ErrorCode::InternalError] {
            let structured = StructuredError::from_code(code, "boom");
            let data = to_rmcp_error_data(&structured);
            assert_eq!(data.code.0, -32603, "{code:?} → -32603 internal error");
            assert!(
                data.data.is_some(),
                "payload still attached on the -32603 arm"
            );
        }
    }

    /// The `test-util` constructors build the REAL lifecycle variants (not fakes) — a smoke test that
    /// they yield the expected `McpServerError` shape (the seam `unblock-cli`'s exit test depends on).
    #[cfg(feature = "test-util")]
    #[test]
    fn transport_test_util_builds_transport_variant() {
        use super::McpServerError;
        let err = McpServerError::__transport_error("boom");
        assert!(
            matches!(err, McpServerError::Transport { .. }),
            "the seam yields a genuine Transport variant"
        );
    }

    /// The async run-loop seam yields a genuine `RunLoop` variant (an aborted-task join error).
    #[cfg(feature = "test-util")]
    #[tokio::test]
    async fn run_loop_test_util_builds_run_loop_variant() {
        use super::McpServerError;
        let err = McpServerError::__run_loop_error().await;
        assert!(
            matches!(err, McpServerError::RunLoop { .. }),
            "the seam yields a genuine RunLoop variant"
        );
    }

    // -- D38 labelling clause: `is_cancellation()` — the cancellation class, NARROWLY. ------------

    /// The rmcp CANCELLATION outcome IS the cancellation class: a cancel landing during the
    /// `initialize` handshake is a shutdown that was asked for, not a fault. Inverting the predicate
    /// turns this RED.
    #[cfg(feature = "test-util")]
    #[test]
    fn cancelled_transport_is_the_cancellation_class() {
        use super::McpServerError;
        assert!(
            McpServerError::__cancelled_error().is_cancellation(),
            "Transport{{Cancelled}} is the cooperative-shutdown outcome (spine §0.1), not a fault"
        );
    }

    /// A GENUINE transport failure (a real bind/IO fault) is NOT the cancellation class — it must
    /// keep its `error[CODE]` stderr line even after a signal. Widening `is_cancellation()` to all
    /// `Transport` errors (the tempting over-generalization) turns this RED.
    #[cfg(feature = "test-util")]
    #[test]
    fn a_genuine_transport_failure_is_not_the_cancellation_class() {
        use super::McpServerError;
        assert!(
            !McpServerError::__transport_error("connection reset").is_cancellation(),
            "a REAL transport fault must never be demoted to routine-cancellation noise"
        );
    }

    /// A run-loop join error (an aborted/panicked task) is never a cancellation — it is exactly the
    /// genuine class that must stay loud.
    #[cfg(feature = "test-util")]
    #[tokio::test]
    async fn a_run_loop_failure_is_not_the_cancellation_class() {
        use super::McpServerError;
        assert!(!McpServerError::__run_loop_error().await.is_cancellation());
    }

    // -- D40 (T3.2.1 follow-up (b)): `is_pre_handshake_disconnect()` — the disconnect class, NARROWLY. --

    /// The pre-`initialize` peer DISCONNECT (`Transport{ConnectionClosed(_)}`) IS the disconnect class,
    /// and the class is NARROW: a genuine transport fault, the handshake's `ExpectedInitializeRequest`
    /// (the variant the `a_no_signal_run_loop_error_exits_1` e2e produces), and a run-loop join error
    /// are all NOT the disconnect class. **Inverting `is_pre_handshake_disconnect` turns this RED**, and
    /// so does widening it to any `Transport` (which would wrongly flip the genuine cases to exit 0).
    /// The two predicates are also proven DISJOINT so `Cancelled` and `ConnectionClosed` route apart.
    #[cfg(feature = "test-util")]
    #[tokio::test]
    async fn connection_closed_is_the_pre_handshake_disconnect_class_narrowly() {
        use super::{McpServerError, TransportSnafu};
        use rmcp::service::ServerInitializeError;
        use snafu::IntoError as _;

        // The disconnect IS the class.
        assert!(
            McpServerError::__connection_closed_error().is_pre_handshake_disconnect(),
            "Transport{{ConnectionClosed}} is the pre-`initialize` peer disconnect (D40)"
        );
        // A REAL transport fault is NOT — it must keep its InternalError/exit-1 mapping, never flip to 0.
        assert!(
            !McpServerError::__transport_error("connection reset").is_pre_handshake_disconnect(),
            "a REAL transport fault is not a routine disconnect"
        );
        // `ExpectedInitializeRequest` — a notification where the `initialize` request was expected — is
        // a DISTINCT variant. It is what `a_no_signal_run_loop_error_exits_1` produces, so it must NOT
        // be matched here (else that unsignalled Err would wrongly become exit 0).
        let expected_init: McpServerError =
            TransportSnafu.into_error(ServerInitializeError::ExpectedInitializeRequest(None));
        assert!(
            !expected_init.is_pre_handshake_disconnect(),
            "ExpectedInitializeRequest is a distinct variant — it must NOT flip to exit 0"
        );
        // A run-loop join error is never a disconnect.
        assert!(
            !McpServerError::__run_loop_error()
                .await
                .is_pre_handshake_disconnect()
        );

        // The two predicates are DISJOINT: neither outcome is misclassified as the other.
        assert!(
            !McpServerError::__connection_closed_error().is_cancellation(),
            "a disconnect is not a cancellation"
        );
        assert!(
            !McpServerError::__cancelled_error().is_pre_handshake_disconnect(),
            "a cancellation is not a disconnect"
        );
    }

    // -- D49 — the startup-failure message DESCRIBES the rejected first frame, never echoes it. ---

    /// `McpServerError::Transport` puts this prefix ahead of the rendered source. It measures 32
    /// bytes, D49 leaves it unchanged, and every expected message below opens with it.
    #[cfg(feature = "test-util")]
    const SNAFU_PREFIX: &str = "failed to start the MCP server: ";

    /// Build an oversized client member from its own sentinel tag plus filler well past the clip
    /// bound.
    ///
    /// The tag is what makes a forbidden member decidably ABSENT, because a clipped member still
    /// carries its own tag. Filling every member with the same byte would make that assertion
    /// unwritable.
    #[cfg(feature = "test-util")]
    fn oversized(sentinel: &str) -> String {
        format!(
            "{sentinel}{}",
            "z".repeat(2 * unblock_error::MAX_ECHOED_BYTES)
        )
    }

    /// Build the expected render of an [`oversized`] member — the kept bytes, the marker, and the
    /// quotes `Debug` adds.
    ///
    /// The expectation comes from the two constants rather than from `clip`, so removing the clip
    /// in production cannot move the expectation with it.
    #[cfg(feature = "test-util")]
    fn quoted_clip(sentinel: &str) -> String {
        use unblock_error::{MAX_ECHOED_BYTES, TRUNCATION_MARKER};

        let raw = oversized(sentinel);
        format!("\"{}{TRUNCATION_MARKER}\"", &raw[..MAX_ECHOED_BYTES])
    }

    /// This is THE regression pin. Each of the four `ClientJsonRpcMessage` shapes renders its own
    /// grammar, each client member arrives clipped and `Debug`-quoted, and `params` / `result` /
    /// `error.message` / `error.data` reach the message in no form at all.
    ///
    /// The oversized members are PER SHAPE because rmcp forbids most members on most shapes — a
    /// notification declares no id, and a response and an error frame carry neither a method nor
    /// `params`. Each shape asserts the FULLY CONSTRUCTED message, since a grammar match would let
    /// an appended rendered `params` through.
    #[cfg(feature = "test-util")]
    #[test]
    fn the_four_frame_shapes_render_bounded_summaries() {
        use super::McpServerError;

        // A REQUEST carries an oversized method, an oversized string id and oversized `params`.
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": oversized("RQID"),
            "method": oversized("RQMETHOD"),
            "params": {"tag": oversized("RQPARAMS")},
        })
        .to_string();
        let message = McpServerError::__expected_initialize_request(&request).to_string();
        let method = quoted_clip("RQMETHOD");
        let id = quoted_clip("RQID");
        assert_eq!(
            message,
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was a request {method} with id {id}"
            )
        );
        assert!(
            !message.contains("RQPARAMS"),
            "`params` must not reach the request summary, got `{message}`"
        );

        // A NOTIFICATION carries an oversized method and oversized `params`, and declares no id.
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": oversized("NTMETHOD"),
            "params": {"tag": oversized("NTPARAMS")},
        })
        .to_string();
        let message = McpServerError::__expected_initialize_request(&notification).to_string();
        let method = quoted_clip("NTMETHOD");
        assert_eq!(
            message,
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was a notification {method}"
            )
        );
        assert!(
            !message.contains("NTPARAMS"),
            "`params` must not reach the notification summary, got `{message}`"
        );

        // A RESPONSE carries an oversized string id and an oversized `result`.
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": oversized("RSID"),
            "result": {"tag": oversized("RSRESULT")},
        })
        .to_string();
        let message = McpServerError::__expected_initialize_request(&response).to_string();
        let id = quoted_clip("RSID");
        assert_eq!(
            message,
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was a response to id {id}"
            )
        );
        assert!(
            !message.contains("RSRESULT"),
            "`result` must not reach the response summary, got `{message}`"
        );

        // An ERROR frame carries an oversized string id, an oversized `error.message` and an
        // oversized `error.data`, plus rmcp's REQUIRED `error.code` — without which it does not
        // deserialize at all.
        let error = serde_json::json!({
            "jsonrpc": "2.0",
            "id": oversized("ERID"),
            "error": {
                "code": -32600,
                "message": oversized("ERMESSAGE"),
                "data": oversized("ERDATA"),
            },
        })
        .to_string();
        let message = McpServerError::__expected_initialize_request(&error).to_string();
        let id = quoted_clip("ERID");
        assert_eq!(
            message,
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was an error response to id {id}"
            )
        );
        assert!(
            !message.contains("ERMESSAGE"),
            "`error.message` must not reach the error summary, got `{message}`"
        );
        assert!(
            !message.contains("ERDATA"),
            "`error.data` must not reach the error summary, got `{message}`"
        );
    }

    /// This cell pins BOUND TWO to the byte. A notification method of 129 ESC bytes leaves `clip`
    /// exactly 128 of them, `Debug` renders each as `\u{1b}`, and the rendered member is
    /// 128 × 6 + 14 + 2 = 784 bytes inside an 888-byte message.
    ///
    /// It is a cell of its own because the four-shape cell's uppercase sentinels and filler escape
    /// to nothing, so nothing there can measure the escape's expansion.
    ///
    /// The `784` and the `888` are HARD totals rather than arithmetic over `MAX_ECHOED_BYTES`, so
    /// widening that bound reddens this cell. Deriving them from the constant would delete that
    /// detector.
    #[cfg(feature = "test-util")]
    #[test]
    fn an_escape_dense_method_renders_the_bound_two_ceiling() {
        use super::McpServerError;
        use unblock_error::{MAX_ECHOED_BYTES, TRUNCATION_MARKER};

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "\u{1b}".repeat(MAX_ECHOED_BYTES + 1),
        })
        .to_string();
        let message = McpServerError::__expected_initialize_request(&frame).to_string();

        let escaped = "\\u{1b}".repeat(MAX_ECHOED_BYTES);
        let member = format!("\"{escaped}{TRUNCATION_MARKER}\"");
        let grammar = format!(
            "{SNAFU_PREFIX}expected the initialize request, but the first frame was a notification "
        );
        let rendered_member = message
            .strip_prefix(&grammar)
            .expect("the message must open with the notification grammar");

        assert_eq!(
            rendered_member.len(),
            784,
            "one rendered member measures 6 × 128 + 14 + 2 bytes at BOUND TWO"
        );
        assert_eq!(message, format!("{grammar}{member}"));
        assert_eq!(
            message.len(),
            888,
            "the whole notification message measures 32 + 72 + 784 bytes at BOUND TWO"
        );
    }

    /// A TYPED notification renders rmcp's own const method. An oversized method deserializes only
    /// as `CustomNotification`, so without this cell four of the five arms go undriven — and
    /// `notifications/initialized` is the one the shipped lifecycle traffic sends.
    #[cfg(feature = "test-util")]
    #[test]
    fn a_typed_notification_renders_its_const_method() {
        use super::McpServerError;

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        })
        .to_string();
        assert_eq!(
            McpServerError::__expected_initialize_request(&frame).to_string(),
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was a notification \"notifications/initialized\""
            )
        );
    }

    /// The three const-keyed arms the cell above does not drive, each rendering its OWN method const.
    ///
    /// That cell drives `notifications/initialized` alone, so these three arms are otherwise
    /// undriven and a const swapped on any of them reaches no other cell. Each case asserts the
    /// full message, which is what names the swapped arm on a failure.
    #[cfg(feature = "test-util")]
    #[test]
    fn the_cancelled_progress_and_roots_arms_render_their_own_consts() {
        use super::McpServerError;

        let cases = [
            (
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/cancelled",
                    "params": {"requestId": 7, "reason": "user cancelled"},
                }),
                "notifications/cancelled",
            ),
            (
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/progress",
                    "params": {"progressToken": 1, "progress": 0.5},
                }),
                "notifications/progress",
            ),
            (
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/roots/list_changed",
                }),
                "notifications/roots/list_changed",
            ),
        ];

        for (frame, method) in cases {
            assert_eq!(
                McpServerError::__expected_initialize_request(&frame.to_string()).to_string(),
                format!(
                    "{SNAFU_PREFIX}expected the initialize request, but the first frame was a notification \"{method}\""
                )
            );
        }
    }

    /// No frame at all gets the fixed sentence — 32 + 53 = 85 bytes, exact rather than bounded,
    /// because it renders no client byte.
    #[cfg(feature = "test-util")]
    #[test]
    fn no_frame_renders_the_fixed_sentence() {
        use super::{McpServerError, TransportSnafu};
        use rmcp::service::ServerInitializeError;
        use snafu::IntoError as _;

        let err: McpServerError =
            TransportSnafu.into_error(ServerInitializeError::ExpectedInitializeRequest(None));
        assert_eq!(
            err.to_string(),
            format!("{SNAFU_PREFIX}expected the initialize request, but no frame arrived")
        );
        assert_eq!(err.to_string().len(), 85);
    }

    /// An error frame is the ONLY shape whose id may be absent, so it is the only arm that needs
    /// the "with no id" spelling. It still carries an oversized `error.message` and `error.data`,
    /// so a render leaking the error body on this arm alone turns the cell red.
    #[cfg(feature = "test-util")]
    #[test]
    fn an_error_frame_without_an_id_renders_the_no_id_sentence() {
        use super::McpServerError;

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32600,
                "message": oversized("NOIDMESSAGE"),
                "data": oversized("NOIDDATA"),
            },
        })
        .to_string();
        let message = McpServerError::__expected_initialize_request(&frame).to_string();

        assert_eq!(
            message,
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was an error response with no id"
            )
        );
        assert_eq!(
            message.len(),
            117,
            "the no-id sentence is exact at 32 + 85 bytes because it renders no client byte"
        );
        assert!(
            !message.contains("NOIDMESSAGE"),
            "`error.message` must not reach the no-id summary, got `{message}`"
        );
        assert!(
            !message.contains("NOIDDATA"),
            "`error.data` must not reach the no-id summary, got `{message}`"
        );
    }

    /// A NUMBER id prints through `NumberOrString`'s `Display` — unquoted and unclipped. Handing
    /// the id to `Debug` instead would render `Number(7)`, which a string-id-only suite never sees.
    /// The typed method beside it also drives `ClientRequest::method()` on a non-custom variant.
    #[cfg(feature = "test-util")]
    #[test]
    fn a_numeric_id_renders_unquoted_through_display() {
        use super::McpServerError;

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/list",
        })
        .to_string();
        assert_eq!(
            McpServerError::__expected_initialize_request(&frame).to_string(),
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was a request \"tools/list\" with id 7"
            )
        );
    }

    /// The RESPONSE and ERROR arms carry a NUMBER id too, and both reach it through the shared
    /// `render_id`. The four-shape cell drives a STRING id on those two arms and the cell above
    /// drives a number on the REQUEST arm, so an arm-local render of the NUMBER case on either of
    /// these two reaches no cell without this one.
    #[cfg(feature = "test-util")]
    #[test]
    fn a_numeric_id_renders_unquoted_on_the_response_and_error_arms() {
        use super::McpServerError;

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": {"tag": "RSRESULT"},
        })
        .to_string();
        assert_eq!(
            McpServerError::__expected_initialize_request(&response).to_string(),
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was a response to id 7"
            )
        );

        let error = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 9,
            "error": {"code": -32600, "message": "ERMESSAGE"},
        })
        .to_string();
        assert_eq!(
            McpServerError::__expected_initialize_request(&error).to_string(),
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was an error response to id 9"
            )
        );
    }

    /// The clip keeps the WHOLE `\u{1b}` escape and never a fragment of one, because the cut runs
    /// BEFORE the escape.
    ///
    /// The ESC sits at byte offset 127, the last byte `clip` keeps. That is the tightest
    /// demonstration rather than the only one — escaping before clipping spends a byte of the budget
    /// on `Debug`'s opening quote and loses the closing quote with six trailing bytes, so the
    /// full-message equality below catches it at every offset that clips. Offset 127 is the only one
    /// at which the marker lands immediately after the kept escape, which is what the second
    /// assertion reads.
    #[cfg(feature = "test-util")]
    #[test]
    fn the_clip_precedes_the_escape_at_the_boundary_byte() {
        use super::McpServerError;
        use unblock_error::{MAX_ECHOED_BYTES, TRUNCATION_MARKER};

        let filler = "a".repeat(MAX_ECHOED_BYTES - 1);
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": format!("{filler}\u{1b}{filler}"),
        })
        .to_string();
        let message = McpServerError::__expected_initialize_request(&frame).to_string();

        assert_eq!(
            message,
            format!(
                "{SNAFU_PREFIX}expected the initialize request, but the first frame was a notification \"{filler}\\u{{1b}}{TRUNCATION_MARKER}\""
            )
        );
        assert!(
            message.contains(&format!("\\u{{1b}}{TRUNCATION_MARKER}")),
            "the kept escape must be whole with the marker right after it, got `{message}`"
        );
    }

    /// `Cancelled` renders BYTE-IDENTICAL to the line the D38 doc comment on
    /// [`McpServerError::is_cancellation`] quotes as MEASURED child stderr. rmcp's own nine-byte
    /// text reaches no clip and the wildcard adds no `Debug` quoting, so the string D38's
    /// diagnostic routing measured survives the D49 render unchanged.
    #[cfg(feature = "test-util")]
    #[test]
    fn cancelled_renders_byte_identical_to_the_measured_line() {
        use super::McpServerError;

        assert_eq!(
            McpServerError::__cancelled_error().to_string(),
            "failed to start the MCP server: Cancelled"
        );
    }

    /// The DEDICATED transport arm keeps the inner I/O reason and clips it. Falling back to rmcp's
    /// whole display would cut inside the transport type name, which `std::any::type_name` fills
    /// past the bound on its own, and the operator would never reach the reason.
    #[cfg(feature = "test-util")]
    #[test]
    fn the_transport_arm_clips_the_inner_error() {
        use super::McpServerError;
        use unblock_error::{MAX_ECHOED_BYTES, TRUNCATION_MARKER};

        let reason = oversized("IOREASON");
        let message = McpServerError::__transport_error(&reason).to_string();

        assert_eq!(
            message,
            format!(
                "{SNAFU_PREFIX}a transport error while test-util transport: {}{TRUNCATION_MARKER}",
                &reason[..MAX_ECHOED_BYTES]
            )
        );
        assert!(
            message.ends_with(TRUNCATION_MARKER),
            "the inner error must be clipped, got `{message}`"
        );
        assert!(
            !message.contains("AsyncRwTransport"),
            "the transport TYPE NAME must never be rendered, got `{message}`"
        );
    }

    /// The transport arm clips its CONTEXT too. `__transport_error` cannot reach that clip — its
    /// context is a hard-coded literal — so this cell builds the variant through rmcp's own public
    /// `ServerInitializeError::transport` constructor with a context past the bound.
    #[cfg(feature = "test-util")]
    #[test]
    fn the_transport_arm_clips_a_long_context() {
        use super::{McpServerError, TransportSnafu};
        use rmcp::RoleServer;
        use rmcp::service::ServerInitializeError;
        use snafu::IntoError as _;
        use unblock_error::{MAX_ECHOED_BYTES, TRUNCATION_MARKER};

        type IoTransport = rmcp::transport::async_rw::AsyncRwTransport<
            RoleServer,
            tokio::io::DuplexStream,
            tokio::io::DuplexStream,
        >;

        let context = oversized("CONTEXT");
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe gone");
        let init_err = ServerInitializeError::transport::<IoTransport>(io_err, context.clone());
        let err: McpServerError = TransportSnafu.into_error(init_err);

        assert_eq!(
            err.to_string(),
            format!(
                "{SNAFU_PREFIX}a transport error while {}{TRUNCATION_MARKER}: pipe gone",
                &context[..MAX_ECHOED_BYTES]
            )
        );
    }

    /// The `#[non_exhaustive]` WILDCARD clips rmcp's own `Display`, so a future variant that echoes
    /// client bytes is bounded on the day it arrives. `ConnectionClosed` drives it because
    /// `__transport_error` reaches the dedicated arm instead, and every variant that lands here
    /// today is far shorter than the bound.
    #[cfg(feature = "test-util")]
    #[test]
    fn the_wildcard_clips_rmcps_own_display() {
        use super::{McpServerError, TransportSnafu};
        use rmcp::service::ServerInitializeError;
        use snafu::IntoError as _;
        use unblock_error::{MAX_ECHOED_BYTES, TRUNCATION_MARKER};

        let detail = oversized("CLOSEDBY");
        let err: McpServerError =
            TransportSnafu.into_error(ServerInitializeError::ConnectionClosed(detail.clone()));

        let rmcp_display = format!("connection closed: {detail}");
        assert_eq!(
            err.to_string(),
            format!(
                "{SNAFU_PREFIX}{}{TRUNCATION_MARKER}",
                &rmcp_display[..MAX_ECHOED_BYTES]
            )
        );
        assert!(
            !err.to_string().contains('"'),
            "rmcp's sentence is clipped WITHOUT the `Debug` quoting, got `{err}`"
        );
    }

    /// The rmcp error stays reachable as `std::error::Error::source()`. The bounded display sits on
    /// the snafu `display` attribute and the `source` attribute is untouched, so a future
    /// chain-walking renderer still finds the rmcp error behind the description.
    ///
    /// The concrete type behind the `dyn Error` is `Box<ServerInitializeError>`, because snafu boxes
    /// the source. A bare `downcast_ref::<ServerInitializeError>()` returns `None` here.
    #[cfg(feature = "test-util")]
    #[test]
    fn the_transport_variant_keeps_its_source() {
        use super::McpServerError;
        use rmcp::service::ServerInitializeError;
        use std::error::Error as _;

        let err = McpServerError::__cancelled_error();
        let source = err
            .source()
            .expect("the Transport variant carries a source");
        let boxed = source
            .downcast_ref::<Box<ServerInitializeError>>()
            .expect("the snafu source is the BOXED rmcp error");
        assert!(
            matches!(**boxed, ServerInitializeError::Cancelled),
            "the source is the SAME rmcp error the constructor wrapped"
        );
    }
}
