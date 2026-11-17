//! The PRE-LIFECYCLE FRAME GATE (D50, redesigned by D59) — a `Transport<RoleServer>` **decorator**
//! that intercepts every inbound frame arriving before EITHER MCP lifecycle has started, so rmcp
//! never reaches `ExpectedInitializeRequest` from the wire.
//!
//! # THE CLASS AND THE TWO DISPOSITIONS (normative, PRD §4 D50 as amended by D59 clause (11))
//!
//! A frame is in class when it arrives BEFORE either lifecycle has started: before the server has
//! answered `initialize`, and before rmcp has started a stateless 2026-07-28 session. rmcp 3.10's
//! pre-lifecycle loop still ends the process with `ExpectedInitializeRequest` on a Notification,
//! Response or Error frame (`rmcp-3.10.0/src/service/server.rs:673-676`), and the id-less
//! notification is still the cheapest kill an unauthenticated peer has under NFR-18. It answers every
//! pre-lifecycle REQUEST itself and continues (`:589-616`): `ping` with `{}`, `initialize` with the
//! handshake, `server/discover` with its `DiscoverResult`, a request lacking 2026-07-28 `_meta` with
//! `-32602` on its own id, and an unsupported inline version with `-32022` on its own id; or it
//! starts the stateless session.
//!
//! So, while the latch is shut, the gate DROPS every Notification, Response and Error frame with no
//! reply (JSON-RPC 2.0 §4.1 forbids replying to a Notification and a reply to a reply is
//! meaningless), and PASSES every Request upward. **The gate writes nothing on any path**: D50's
//! `-32600` and its reply constant are retired, and with them D50 clause (5), whose only subject was
//! that reply. A line rmcp cannot TYPE never reaches this gate: the transport answers it below the
//! gate by D59 clause (2)'s three rules, or the compatibility filter drops it silently.
//!
//! BUFFER-AND-REPLAY IS REJECTED on stated grounds. It retains an accumulator on the least-identified
//! path NFR-18 already carries an unbounded-parse residual for; replaying a Notification is provable
//! dead work because `UnblockServer` overrides no notification handler and rmcp's defaults are
//! `ready(())`; and replaying a client Response or Error can never correlate, because the server's
//! `Peer` has sent nothing yet.
//!
//! # THE LATCH OPENS ON EITHER LIFECYCLE'S START
//!
//! **The `initialize` edge** is the server's OWN outbound `InitializeResult`, observed as it passes
//! through [`PreHandshakeGateTransport::send`]; rmcp writes that frame at
//! `rmcp-3.10.0/src/service/server.rs:710-720`. The key is the `ServerResult::InitializeResult`
//! VARIANT, never the protocol version string. The pre-lifecycle `ping` reply is a Response carrying
//! `ServerResult::EmptyResult` and the `server/discover` reply one carrying
//! `ServerResult::DiscoverResult`, so both leave the latch shut and a `ping`-first or
//! `discover`-first client still reaches either lifecycle. The gate never waits for
//! `notifications/initialized`, because rmcp serves a request arriving before that notification
//! normally and this transport must not be stricter.
//!
//! **The stateless edge** is the INBOUND Request rmcp will serve as a session start, decided by the
//! SAME public rmcp predicate its pre-lifecycle loop runs (`:589-671`): the Request is neither
//! `initialize`, `ping` nor `server/discover`, its `_meta` misses none of the keys
//! `RequestMetaObject::missing_required_keys(&ProtocolVersion::V_2026_07_28)` names, and its
//! `protocol_version()` lies inside `UnblockServer::supported_protocol_versions()`. The gate passes
//! that frame and opens, so rmcp's serve loop receives every later notification, a
//! `notifications/cancelled` included. A shut latch that outlived a stateless start would drop
//! cancellations for the session's whole life; one that opened early would let a notification kill
//! the process. An outbound-only latch cannot serve this edge: the stateless session's first reply
//! is indistinguishable on the wire from a pre-lifecycle `-32602`, and flipping a flag after
//! `serve_with_ct` returns races the serve loop it spawns.
//!
//! THE MATCHES KEY ON THE VARIANT, never on `ClientRequest::method()`. `ClientRequest` is untagged
//! and ends in `CustomRequest`, so a frame spelled `"method":"initialize"` whose `params` do not type
//! decodes as `CustomRequest`; rmcp keys on the variant, answers that frame `-32602` (its `_meta`
//! names no 2026-07-28 context) and continues, and the gate passes it like any other Request.
//!
//! # ORDER: SCAN, THEN GATE
//!
//! The composition is `PreHandshakeGateTransport<DupScanningTransport<R, W>>` ([`crate::server`]), so
//! the receive order is scan, then gate. A frame that fails the typed parse is answered (or dropped)
//! by the scanner before this decorator ever sees it, which is what keeps D54's recovered-id reply
//! reachable before either lifecycle starts.
//!
//! # THE `ub-o8s` OVERSIZED-FRAME RESIDUAL, disclosed here
//!
//! The stdio read still accepts a line of any length (D59 clause (12)(iv)). A dropped premature frame
//! no longer kills the process, so the read buffer's peak is retained for the process lifetime and an
//! oversized premature frame is repeatable. `ub-o8s` stays OPEN.

use std::borrow::Cow;

use rmcp::model::{ClientRequest, GetMeta as _, JsonRpcMessage, ProtocolVersion, ServerResult};
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;

/// What the gate does with one inbound frame while the latch is shut.
#[derive(Debug, PartialEq, Eq)]
enum Disposition {
    /// Hand the Request upward; rmcp answers it and stays before either lifecycle.
    Pass,
    /// Hand the Request upward. It carries complete 2026-07-28 `_meta` naming this version and is
    /// none of `initialize`, `ping` and `server/discover`, so rmcp starts the stateless session on it
    /// exactly when the version is supported, and the gate opens on the same condition.
    PassStatelessCandidate(ProtocolVersion),
    /// Drop the frame with no reply — a Notification, a Response or an Error frame.
    Drop,
}

/// Decide one inbound frame's disposition while neither lifecycle has started.
///
/// The match over `JsonRpcMessage` carries NO wildcard arm, so an rmcp bump that adds a fifth frame
/// variant is a COMPILE ERROR here rather than a silent hole — the same rule [`crate::wire`] states
/// for its own exhaustive match.
fn classify_pre_handshake_frame(message: &RxJsonRpcMessage<RoleServer>) -> Disposition {
    match message {
        JsonRpcMessage::Request(request) => match stateless_start_version(&request.request) {
            Some(version) => Disposition::PassStatelessCandidate(version),
            None => Disposition::Pass,
        },
        JsonRpcMessage::Notification(_)
        | JsonRpcMessage::Response(_)
        | JsonRpcMessage::Error(_) => Disposition::Drop,
    }
}

/// The inline version a Request would start the stateless 2026-07-28 session on, if rmcp's
/// pre-lifecycle loop reaches its session-start branch for it.
///
/// The SAME public predicate `rmcp-3.10.0/src/service/server.rs:589-671` runs, in its order: not
/// `initialize`, not `ping`, complete 2026-07-28 `_meta` (`missing_required_keys` empty and a
/// readable `protocol_version()`), and not `server/discover`. The last condition, a version inside
/// `UnblockServer::supported_protocol_versions()`, is checked by the gate against the set it was
/// built with. `the_gate_predicate_agrees_with_rmcps_pre_lifecycle_loop` holds the whole edge to
/// rmcp's loop.
fn stateless_start_version(request: &ClientRequest) -> Option<ProtocolVersion> {
    if matches!(
        request,
        ClientRequest::InitializeRequest(_) | ClientRequest::PingRequest(_)
    ) {
        return None;
    }
    let meta = request.get_meta();
    if !meta
        .missing_required_keys(&ProtocolVersion::V_2026_07_28)
        .is_empty()
    {
        return None;
    }
    let version = meta.protocol_version()?;
    if matches!(request, ClientRequest::DiscoverRequest(_)) {
        return None;
    }
    Some(version)
}

/// Does this inbound frame START the stateless 2026-07-28 lifecycle — rmcp's pre-lifecycle loop
/// serving it as a session start, given the server's supported versions? The gate opens on exactly
/// this edge; `the_gate_predicate_agrees_with_rmcps_pre_lifecycle_loop` holds it to rmcp's loop.
pub(crate) fn starts_the_stateless_lifecycle(
    message: &RxJsonRpcMessage<RoleServer>,
    supported: &[ProtocolVersion],
) -> bool {
    matches!(
        classify_pre_handshake_frame(message),
        Disposition::PassStatelessCandidate(version) if supported.contains(&version)
    )
}

/// Does this outbound frame complete the `initialize` handshake?
///
/// The key is the `ServerResult::InitializeResult` VARIANT. `EmptyResult` (the pre-lifecycle `ping`
/// reply) and `DiscoverResult` leave the latch shut, and the match names every `JsonRpcMessage`
/// variant so an rmcp bump is a compile error here too.
fn completes_the_handshake(item: &TxJsonRpcMessage<RoleServer>) -> bool {
    match item {
        JsonRpcMessage::Response(response) => {
            matches!(response.result, ServerResult::InitializeResult(_))
        }
        JsonRpcMessage::Request(_) | JsonRpcMessage::Notification(_) | JsonRpcMessage::Error(_) => {
            false
        }
    }
}

/// A `Transport<RoleServer>` decorator that drops every non-Request frame until either MCP
/// lifecycle has started, and passes every Request upward.
///
/// It wraps [`crate::wire::DupScanningTransport`] ([`crate::server`]), so the receive order is scan,
/// then gate. The module doc carries the rules; this type carries the latch.
pub(crate) struct PreHandshakeGateTransport<T> {
    /// The wrapped transport. Every call delegates to it, `receive` after the classification and
    /// `send` after the latch check.
    inner: T,
    /// The versions the stateless edge accepts: `UnblockServer::supported_protocol_versions()`, the
    /// same set rmcp's loop checks an inline version against.
    supported: Cow<'static, [ProtocolVersion]>,
    /// Shut until either lifecycle starts. A plain `bool` suffices because `send` and `receive` both
    /// take `&mut self`, so the borrow checker gives each call exclusive access to this field. rmcp
    /// drives `send` and `receive` from one task and spawns only the write FUTURE that `send`
    /// returns, and that future never touches the latch.
    lifecycle_started: bool,
}

impl<T> PreHandshakeGateTransport<T> {
    /// Wrap `inner` with the latch shut, accepting `supported` on the stateless edge.
    pub(crate) fn new(inner: T, supported: Cow<'static, [ProtocolVersion]>) -> Self {
        Self {
            inner,
            supported,
            lifecycle_started: false,
        }
    }
}

impl<T> Transport<RoleServer> for PreHandshakeGateTransport<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    /// Open the latch on the server's own `InitializeResult`, then delegate.
    ///
    /// The latch is set in the SYNCHRONOUS head, before the delegated future is returned, because
    /// `Transport::send` requires that future to be `'static` and it therefore cannot touch `self`.
    /// rmcp calls `receive()` nowhere between reading `initialize` and writing its result
    /// (`rmcp-3.10.0/src/service/server.rs:680-720`), so nothing races that write.
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        if completes_the_handshake(&item) {
            self.lifecycle_started = true;
        }
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            let message = self.inner.receive().await?;
            if self.lifecycle_started {
                return Some(message);
            }
            match classify_pre_handshake_frame(&message) {
                Disposition::Pass => return Some(message),
                Disposition::PassStatelessCandidate(_) => {
                    // rmcp starts the stateless session exactly when the version is supported; an
                    // unsupported one is answered `-32022` and the loop stays before either lifecycle.
                    if starts_the_stateless_lifecycle(&message, &self.supported) {
                        self.lifecycle_started = true;
                    }
                    return Some(message);
                }
                Disposition::Drop => {
                    // The frame itself is deliberately NOT logged at any level: it is untrusted input.
                    tracing::debug!(
                        "a notification, response or error frame arrived before either lifecycle started; dropping it with no reply"
                    );
                }
            }
        }
    }

    fn close(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests {
    use super::PreHandshakeGateTransport;
    use rmcp::model::{
        ClientJsonRpcMessage, ClientRequest, InitializeResult, JsonRpcMessage, ProtocolVersion,
        RequestId, ServerCapabilities, ServerJsonRpcMessage, ServerResult,
    };
    use rmcp::service::RoleServer;
    use rmcp::transport::Transport;
    use std::borrow::Cow;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A well-formed `initialize` request, id 2 — the frame every cell ends on, so each one lands a
    /// POSITIVE outcome rather than only the absence of a death.
    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gate-cell","version":"0"}}}"#;

    /// A well-formed `ping` request, id 1.
    const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

    /// An id-less notification — the cheapest pre-lifecycle kill, and the probe every latch cell
    /// uses: dropped while the latch is shut, delivered once it is open.
    const CANCELLED: &str =
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":99}}"#;

    /// The inner transport the gate's own cells run over, with no rmcp serve loop anywhere.
    ///
    /// `receive` replays a scripted sequence and `send` appends to a shared log, so "dropped with no
    /// byte written" is read off an EMPTY log. The log holds decoded messages rather than bytes,
    /// which is the stronger claim — a reply the gate never hands down can write no byte at all.
    struct MockInner {
        /// The frames `receive` hands out, in order. An exhausted queue is EOF.
        inbound: VecDeque<ClientJsonRpcMessage>,
        /// Every frame handed to `send`, in order.
        written: Arc<Mutex<Vec<ServerJsonRpcMessage>>>,
    }

    impl Transport<RoleServer> for MockInner {
        type Error = std::io::Error;

        fn send(
            &mut self,
            item: ServerJsonRpcMessage,
        ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
            let log = Arc::clone(&self.written);
            async move {
                log.lock()
                    .expect("the write log is not poisoned")
                    .push(item);
                Ok(())
            }
        }

        async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
            self.inbound.pop_front()
        }

        async fn close(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Decode one scripted frame, naming the frame in the failure.
    fn frame(raw: &str) -> ClientJsonRpcMessage {
        serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("the scripted frame must decode: {e} — {raw}"))
    }

    /// Build a gate over a scripted inbound sequence, returning the gate and the write log. The gate
    /// accepts rmcp's five known versions on the stateless edge, the set `UnblockServer` keeps.
    fn gate(
        script: &[&str],
    ) -> (
        PreHandshakeGateTransport<MockInner>,
        Arc<Mutex<Vec<ServerJsonRpcMessage>>>,
    ) {
        let written = Arc::new(Mutex::new(Vec::new()));
        let inner = MockInner {
            inbound: script.iter().map(|raw| frame(raw)).collect(),
            written: Arc::clone(&written),
        };
        (
            PreHandshakeGateTransport::new(inner, Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)),
            written,
        )
    }

    /// The server's own `InitializeResult` response on id 2, at the given protocol version.
    fn initialize_result(version: ProtocolVersion) -> ServerJsonRpcMessage {
        ServerJsonRpcMessage::response(
            ServerResult::InitializeResult(
                InitializeResult::new(ServerCapabilities::default()).with_protocol_version(version),
            ),
            RequestId::Number(2),
        )
    }

    /// A snapshot of the write log.
    fn log(written: &Arc<Mutex<Vec<ServerJsonRpcMessage>>>) -> Vec<ServerJsonRpcMessage> {
        written
            .lock()
            .expect("the write log is not poisoned")
            .clone()
    }

    /// Assert one received frame is the `initialize` request — the positive landing every drop cell
    /// ends on.
    fn assert_is_initialize(message: &ClientJsonRpcMessage) {
        match message {
            JsonRpcMessage::Request(request) => assert!(
                matches!(request.request, ClientRequest::InitializeRequest(_)),
                "the handshake must still reach rmcp, got {:?}",
                request.request
            ),
            other => panic!("the handshake must still reach rmcp, got {other:?}"),
        }
    }

    /// Assert one received frame is the `tools/list` request.
    fn assert_is_list_tools(message: &ClientJsonRpcMessage) {
        match message {
            JsonRpcMessage::Request(request) => assert!(
                matches!(request.request, ClientRequest::ListToolsRequest(_)),
                "the request must reach rmcp, got {:?}",
                request.request
            ),
            other => panic!("the request must reach rmcp, got {other:?}"),
        }
    }

    /// **A premature REQUEST is passed upward and the gate writes nothing** (D59 clause (11)).
    ///
    /// rmcp 3.10 answers a pre-lifecycle Request itself (`-32602` here, since it carries no
    /// 2026-07-28 `_meta`) and continues, so the gate hands it upward. Answering it, or dropping it,
    /// turns this cell red.
    #[tokio::test]
    async fn a_premature_request_is_passed_through_and_nothing_is_written() {
        let script = [
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}"#,
            INITIALIZE,
        ];
        let (mut gate, written) = gate(&script);

        assert_is_list_tools(&gate.receive().await.expect("the request reaches rmcp"));
        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        assert!(
            log(&written).is_empty(),
            "the gate writes no frame of its own: {:?}",
            log(&written)
        );
    }

    /// **A premature NOTIFICATION is dropped with no reply, and it does not open the latch.**
    ///
    /// The script puts `notifications/initialized` first, a `tools/list` behind it and a second
    /// notification behind that. Delivering either notification, answering one, or letting the
    /// passed Request open the latch turns this cell red: the second notification must still be
    /// dropped, so `initialize` is the next frame rmcp sees.
    #[tokio::test]
    async fn a_premature_notification_is_dropped_and_leaves_the_latch_shut() {
        let script = [
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/list","params":{}}"#,
            CANCELLED,
            INITIALIZE,
        ];
        let (mut gate, written) = gate(&script);

        assert_is_list_tools(&gate.receive().await.expect("the request reaches rmcp"));
        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        assert!(
            log(&written).is_empty(),
            "a notification writes nothing: {:?}",
            log(&written)
        );
    }

    /// **A premature RESPONSE frame is dropped with no reply.**
    ///
    /// A reply to a reply is meaningless, so the write log stays empty and the `initialize` behind it
    /// still reaches rmcp.
    #[tokio::test]
    async fn a_premature_response_frame_is_dropped_with_no_reply() {
        let script = [r#"{"jsonrpc":"2.0","id":3,"result":{}}"#, INITIALIZE];
        let (mut gate, written) = gate(&script);

        let delivered = gate.receive().await.expect("the handshake frame survives");
        assert_is_initialize(&delivered);
        assert!(
            log(&written).is_empty(),
            "a response frame earns no reply: {:?}",
            log(&written)
        );
    }

    /// **A premature ERROR frame is dropped with no reply.**
    ///
    /// This is the fourth shape rmcp collapses into its one killing arm, and it earns no reply for
    /// the same reason the response frame does.
    #[tokio::test]
    async fn a_premature_error_frame_is_dropped_with_no_reply() {
        let script = [
            r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32601,"message":"no such method"}}"#,
            INITIALIZE,
        ];
        let (mut gate, written) = gate(&script);

        let delivered = gate.receive().await.expect("the handshake frame survives");
        assert_is_initialize(&delivered);
        assert!(
            log(&written).is_empty(),
            "an error frame earns no reply: {:?}",
            log(&written)
        );
    }

    /// **`ping` then `initialize` both pass through untouched.**
    ///
    /// rmcp answers the pre-lifecycle `ping` itself, so the gate must hand it upward and write
    /// nothing.
    #[tokio::test]
    async fn a_ping_then_an_initialize_both_pass_through() {
        let (mut gate, written) = gate(&[PING, INITIALIZE]);

        let first = gate.receive().await.expect("the ping reaches rmcp");
        match &first {
            JsonRpcMessage::Request(request) => assert!(
                matches!(request.request, ClientRequest::PingRequest(_)),
                "the ping must reach rmcp as a ping, got {:?}",
                request.request
            ),
            other => panic!("the ping must reach rmcp as a request, got {other:?}"),
        }

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        assert!(
            log(&written).is_empty(),
            "the gate answers neither frame: {:?}",
            log(&written)
        );
    }

    /// **The latch opens on the server's own `InitializeResult`.**
    ///
    /// The notification behind the handshake is DELIVERED rather than dropped, and the write log
    /// holds only the handshake response the cell sent. Never setting the latch turns this red.
    #[tokio::test]
    async fn the_latch_opens_on_the_initialize_result() {
        let (mut gate, written) = gate(&[INITIALIZE, CANCELLED]);

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        gate.send(initialize_result(ProtocolVersion::LATEST_WITH_INITIALIZE))
            .await
            .expect("the handshake response goes out");

        assert!(
            matches!(
                gate.receive().await.expect("the notification is delivered"),
                JsonRpcMessage::Notification(_)
            ),
            "a post-handshake notification passes through"
        );

        let entries = log(&written);
        assert_eq!(
            entries.len(),
            1,
            "the handshake response is the only frame written: {entries:?}"
        );
        assert!(
            matches!(&entries[0], JsonRpcMessage::Response(_)),
            "and it is the response the cell sent: {:?}",
            entries[0]
        );
    }

    /// **A SUPPORTED but older `InitializeResult` opens the latch too.**
    ///
    /// `2025-03-26` is a member of `ProtocolVersion::KNOWN_VERSIONS` and differs from the
    /// `LATEST_WITH_INITIALIZE` the other cells send, so a latch keyed on the version STRING leaves
    /// it shut here and drops the notification. The variant key opens it.
    #[tokio::test]
    async fn a_supported_non_latest_initialize_result_still_opens_the_latch() {
        let (mut gate, written) = gate(&[INITIALIZE, CANCELLED]);

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        assert_ne!(
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::LATEST_WITH_INITIALIZE,
            "non-vacuity: the cell's version must really differ from the newest handshake version"
        );
        gate.send(initialize_result(ProtocolVersion::V_2025_03_26))
            .await
            .expect("the handshake response goes out");

        assert!(
            matches!(
                gate.receive().await.expect("the notification is delivered"),
                JsonRpcMessage::Notification(_)
            ),
            "a version-keyed latch drops this notification"
        );
        assert_eq!(log(&written).len(), 1, "nothing else is written");
    }

    /// **The pre-lifecycle `ping` reply leaves the latch shut.**
    ///
    /// rmcp answers a pre-lifecycle `ping` with `ServerResult::EmptyResult`, which travels the same
    /// `send` path as the handshake response. A latch opened by `EmptyResult` would deliver the
    /// notification behind it instead of dropping it.
    #[tokio::test]
    async fn the_ping_reply_leaves_the_latch_shut() {
        let (mut gate, written) = gate(&[PING, CANCELLED, INITIALIZE]);

        gate.receive().await.expect("the ping reaches rmcp");
        gate.send(ServerJsonRpcMessage::response(
            ServerResult::empty(()),
            RequestId::Number(1),
        ))
        .await
        .expect("the ping reply goes out");

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        assert_eq!(
            log(&written).len(),
            1,
            "only the ping reply is written: {:?}",
            log(&written)
        );
    }

    /// **A request arriving before `notifications/initialized` is served.**
    ///
    /// rmcp enters its main loop straight after writing the handshake response and never waits for
    /// that notification, so a gate that waited would be stricter than the server it decorates.
    #[tokio::test]
    async fn a_request_arriving_before_the_initialized_notification_is_served() {
        let script = [
            INITIALIZE,
            r#"{"jsonrpc":"2.0","id":12,"method":"tools/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ];
        let (mut gate, written) = gate(&script);

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        gate.send(initialize_result(ProtocolVersion::LATEST_WITH_INITIALIZE))
            .await
            .expect("the handshake response goes out");

        assert_is_list_tools(
            &gate
                .receive()
                .await
                .expect("the request ahead of the notification is delivered"),
        );
        assert!(
            matches!(
                gate.receive().await.expect("the notification follows"),
                JsonRpcMessage::Notification(_)
            ),
            "the notification itself is delivered once the latch is open"
        );
        assert_eq!(
            log(&written).len(),
            1,
            "nothing but the handshake is written"
        );
    }

    /// **A `"method":"initialize"` frame whose `params` do not type is passed upward.**
    ///
    /// The `params` object here carries `protocolVersion` and omits the required `capabilities` and
    /// `clientInfo`, so rmcp's untagged `ClientRequest` falls through to `CustomRequest`. rmcp 3.10's
    /// pre-lifecycle loop keys on the `InitializeRequest` VARIANT and answers this frame `-32602`
    /// (its `_meta` names no 2026-07-28 context) and continues, so the gate passes it like any other
    /// Request, writes nothing, and the real `initialize` behind it still reaches rmcp.
    #[tokio::test]
    async fn a_method_initialize_frame_with_untypeable_params_is_passed_through() {
        let untypeable = r#"{"jsonrpc":"2.0","id":5,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        match frame(untypeable) {
            JsonRpcMessage::Request(request) => match request.request {
                ClientRequest::CustomRequest(custom) => assert_eq!(
                    custom.method, "initialize",
                    "non-vacuity: the frame really spells the initialize method"
                ),
                other => panic!("the frame must decode as a CustomRequest, got {other:?}"),
            },
            other => panic!("the frame must decode as a request, got {other:?}"),
        }

        let (mut gate, written) = gate(&[untypeable, CANCELLED, INITIALIZE]);
        match gate
            .receive()
            .await
            .expect("the custom request reaches rmcp")
        {
            JsonRpcMessage::Request(request) => assert!(
                matches!(request.request, ClientRequest::CustomRequest(_)),
                "the untypeable frame is passed upward, got {:?}",
                request.request
            ),
            other => panic!("the untypeable frame is passed upward, got {other:?}"),
        }
        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        assert!(
            log(&written).is_empty(),
            "the gate writes nothing: {:?}",
            log(&written)
        );
    }

    /// The complete 2026-07-28 `_meta` on a `tools/list`, at `version`.
    fn stateless(id: i64, version: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"{version}","io.modelcontextprotocol/clientCapabilities":{{}}}}}}}}"#
        )
    }

    /// **(g) — the latch opens on a STATELESS start**, so a `notifications/cancelled` after it reaches
    /// rmcp. A latch that opened only on `InitializeResult` drops that notification and turns this red.
    #[tokio::test]
    async fn a_stateless_session_start_opens_the_latch() {
        let start = stateless(1, "2026-07-28");
        let (mut gate, written) = gate(&[&start, CANCELLED]);
        assert!(matches!(
            gate.receive()
                .await
                .expect("the session start reaches rmcp"),
            JsonRpcMessage::Request(_)
        ));
        assert!(
            matches!(
                gate.receive().await.expect("the cancellation is delivered"),
                JsonRpcMessage::Notification(_)
            ),
            "after a stateless start every notification reaches rmcp's serve loop"
        );
        assert!(log(&written).is_empty());
    }

    /// **(g) — an UNSUPPORTED inline version leaves the latch shut**: rmcp answers it `-32022` and
    /// stays before either lifecycle, so the notification behind it is still dropped.
    #[tokio::test]
    async fn an_unsupported_inline_version_leaves_the_latch_shut() {
        let refused = stateless(1, "2030-01-01");
        let (mut gate, _written) = gate(&[&refused, CANCELLED, INITIALIZE]);
        gate.receive().await.expect("the request reaches rmcp");
        assert_is_initialize(&gate.receive().await.expect("the notification is dropped"));
    }

    /// **(g) — `server/discover` with complete `_meta` leaves the latch shut**: rmcp answers it and
    /// stays open to either lifecycle.
    #[tokio::test]
    async fn a_server_discover_leaves_the_latch_shut() {
        let discover = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#;
        let (mut gate, _written) = gate(&[discover, CANCELLED, INITIALIZE]);
        gate.receive().await.expect("discover reaches rmcp");
        assert_is_initialize(&gate.receive().await.expect("the notification is dropped"));
    }
}
