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
use unblock_error::StructuredError;

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
}
