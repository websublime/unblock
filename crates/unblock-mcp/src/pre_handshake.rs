//! The PRE-HANDSHAKE FRAME GATE (D50) — a `Transport<RoleServer>` **decorator** that intercepts
//! every inbound frame arriving before the server has answered `initialize`, so rmcp never reaches
//! `ExpectedInitializeRequest` from the wire.
//!
//! # THE CLASS AND THE FOUR SHAPES (normative, PRD §4 D50, spine §5.6)
//!
//! A frame is in class when it arrives BEFORE the server has responded to the `initialize` request
//! and is neither an `initialize` Request nor a `ping` Request. rmcp 1.7.0 reaches that class by two
//! spellings and both end the process. A non-Request frame returns `ExpectedInitializeRequest`
//! (`rmcp-1.7.0/src/service/server.rs:193`), and a Request that is not `initialize` breaks the loop
//! at `:191` and returns the same error at `:200-204`. The gate covers all FOUR JSON-RPC shapes,
//! because rmcp collapses Notification, Response and Error into ONE `other =>` arm at `:192-196`
//! and the id-less notification is the cheapest kill an unauthenticated peer has under NFR-18.
//! `ping` stays the single pre-handshake exception, answered by rmcp itself with
//! `ServerResult::EmptyResult` (`:175-189`) and followed by the loop continuing.
//!
//! # DISPOSITION, PER SHAPE — exactly one shape writes bytes
//!
//! A premature REQUEST is answered `-32600 Invalid Request` carrying THAT FRAME'S OWN id and is then
//! DROPPED, never handed to rmcp. A premature NOTIFICATION, RESPONSE or ERROR frame is DROPPED with
//! no reply, because JSON-RPC 2.0 §4.1 forbids replying to a Notification and a reply to a reply is
//! meaningless.
//!
//! Answering the Request shape is forced by a property of rmcp's client. Today the client's only
//! signal is the child's death, which a stdio peer reads as EOF. A surviving server that dropped
//! silently would give neither a reply nor an EOF, and an rmcp client awaits untimed
//! ([`crate::wire`] states that await and its citations), so silence would turn today's fast failure
//! into an unbounded hang. `-32600` adds NO wire vocabulary — D47 already emits that constant from
//! the transport below this one.
//!
//! BUFFER-AND-REPLAY IS REJECTED on stated grounds. It retains an accumulator on the least-identified
//! path NFR-18 already carries an unbounded-parse residual for; a byte bound needs an overflow policy
//! that re-asks this same question inside itself; replaying a Notification is provable dead work
//! because `UnblockServer` overrides no notification handler and rmcp's defaults are `ready(())`
//! (`rmcp-1.7.0/src/handler/server.rs:296-321`); and replaying a client Response or Error can never
//! correlate, since `Peer` is built at `rmcp-1.7.0/src/service/server.rs:205` and has sent nothing.
//!
//! # THE GATE MUST SEE A FRAME AFTER THE D47 ARM — an ordering that is a correctness requirement
//!
//! The receive order is scan, gate, clamp, so [`crate::wire::DupScanningTransport`] answers and drops
//! the D47 un-decodable-`id` class BEFORE this decorator ever sees it. That order is required rather
//! than preferred. A D47 frame's id lives ONLY in the raw bytes, so a gate running BEFORE that arm
//! would see the frame as a plain premature Notification, drop it with no reply, and leave the peer
//! D47's recovered-id answer exists to release waiting forever.
//!
//! Hosting the gate INSIDE the scanner is the expensive option. `run_ours` feeds each CD-7 corpus
//! entry as its own standalone stream, so a latch hosted there would be shut for every entry, and the
//! per-entry differential tier and the whole-stream tier would diverge from rmcp on nearly the whole
//! corpus. The decorator leaves that harness byte-unchanged. The clamp is disqualified by its stated
//! deletion endgame ([`crate::server`]), since a permanent gate must not ride a transport slated for
//! removal.
//!
//! The reply travels through `self.inner.send(..)`, which is the scanner's `send`, so it is written
//! under the SAME write mutex and is byte-atomic by inheritance. The cost is stated rather than
//! hidden — the single-emission-helper property becomes two sites, because a decorator generic over
//! `T: Transport` cannot reach the scanner's private `answer_error`.
//!
//! # THE LATCH opens on the server's OWN outbound `InitializeResult`
//!
//! The opening edge is a `ServerJsonRpcMessage::Response` carrying `ServerResult::InitializeResult`,
//! observed as it passes through [`PreHandshakeGateTransport::send`]; rmcp writes that frame at
//! `rmcp-1.7.0/src/service/server.rs:240-248`. The MCP lifecycle names that same edge, so the gate's
//! boundary and the decision's boundary are one boundary by construction. The pre-handshake `ping`
//! reply is a Response carrying `ServerResult::EmptyResult` (`:180`), so it leaves the latch shut and
//! a `ping`-first client still reaches a normal handshake. The gate never waits for
//! `notifications/initialized`, because rmcp serves a request arriving before that notification
//! normally and this transport must not be stricter.
//!
//! THE LATCH KEYS ON THE VARIANT AND NEVER ON ANYTHING THE CLAMP CAN REWRITE. `VersionClampingTransport`
//! wraps the gate, so on the receive side the gate sees an UNCLAMPED `initialize`. Keying on
//! `protocolVersion`, or on any value the clamp may rewrite, would desynchronise the two layers on a
//! future clamp change, and a latch keyed on the version STRING is already wrong today, because a
//! supported non-latest `InitializeResult` leaves it shut while the variant key opens it.
//!
//! THE RECEIVE-SIDE PASS ARM KEYS ON THE VARIANT TOO, matching `ClientRequest::InitializeRequest(_)`
//! and `ClientRequest::PingRequest(_)` and never `ClientRequest::method()`. `ClientRequest` is
//! `#[serde(untagged)]` (`rmcp-1.7.0/src/model.rs:3241`) and ends in `CustomRequest` (`:3288`), so a
//! frame spelled `"method":"initialize"` whose `params` do not type decodes as `CustomRequest`, and a
//! method-string gate would pass it upward into the same `ExpectedInitializeRequest` death. The
//! variant is what rmcp itself keys on at `rmcp-1.7.0/src/service/server.rs:176` and `:200-204`.
//!
//! The inbound `initialize` REQUEST is deliberately NOT the latch event. rmcp calls
//! `transport.receive()` nowhere between the break at `:191` and the write at `:240-248`, so the two
//! edges are separated by ZERO inbound frames and behave identically in 1.7.0. The outbound edge is
//! chosen because it stays correct if a future rmcp reads ahead, and because it is the completion
//! edge by construction.
//!
//! # THE `ub-nbz` RESIDUAL IS INHERITED AND NOT WIDENED IN THE REGIME THAT MATTERS
//!
//! The reply is written INSIDE `receive()`, the shape [`crate::wire`] already discloses, so rmcp may
//! drop that future mid-poll and take the unwritten reply with it. Pre-handshake the only dropper is
//! the cancellation select at `rmcp-1.7.0/src/service/server.rs:150-155`, so a lost reply coincides
//! with a signal, where it is irrelevant. The unbiased request-traffic select
//! (`rmcp-1.7.0/src/service.rs:805-813`) exists only after the handshake, which is after the latch has
//! opened and the gate no longer writes. Pre-handshake is the regime the `ub-nbz` harness measured at
//! 0 of 40 losses. `ub-nbz` stays OPEN.
//!
//! # THE `ub-o8s` OVERSIZED-FRAME RESIDUAL CHANGES SHAPE, and the new shape is disclosed here
//!
//! The read is unchanged, since the stdio transport still accepts a line of any length. The gate
//! changes what follows that read. The read buffer's peak is now retained for the process lifetime
//! instead of being reclaimed by the death, and an oversized premature frame is repeatable where it
//! used to be one-shot. Four consecutive 16 MB frames held resident memory flat, so this is
//! retention and not a leak. `ub-o8s` stays OPEN.

use rmcp::model::{ClientRequest, ErrorData, JsonRpcMessage, RequestId, ServerResult};
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;

/// The `-32600` message for the D50 pre-handshake arm.
///
/// A COMPILE-TIME CONSTANT on purpose, and `data` is absent for the same reason D47 gives. The reply
/// carries no client bytes beyond the JSON-RPC id it must echo to stay correlatable, so its member
/// set is exactly D47's `-32600` reply's, and a `data` derived from the frame would open a new echo
/// channel for untrusted input with no protocol requirement behind it.
const PRE_HANDSHAKE_REJECTION_MESSAGE: &str = "the server has not completed the initialize handshake and accepts only initialize and ping until it has";

/// What the gate does with one inbound frame while the latch is shut.
#[derive(Debug)]
enum Disposition {
    /// Hand the frame upward — an `initialize` or a `ping` Request.
    Pass,
    /// Answer `-32600` on the carried id, then drop the frame.
    Answer(RequestId),
    /// Drop the frame with no reply — a Notification, a Response or an Error frame.
    Drop,
}

/// Decide one inbound frame's disposition while the handshake is still open.
///
/// The match over `JsonRpcMessage` carries NO wildcard arm, so an rmcp bump that adds a fifth frame
/// variant is a COMPILE ERROR here rather than a silent hole — the same rule [`crate::wire`] states
/// for its own exhaustive match. The Request arm keys on the `ClientRequest` VARIANT; its trailing
/// arm is correct by default, because any request variant rmcp gains later is neither `initialize`
/// nor `ping` and is answered.
fn classify_pre_handshake_frame(message: &RxJsonRpcMessage<RoleServer>) -> Disposition {
    match message {
        JsonRpcMessage::Request(request) => match &request.request {
            ClientRequest::InitializeRequest(_) | ClientRequest::PingRequest(_) => {
                Disposition::Pass
            }
            _ => Disposition::Answer(request.id.clone()),
        },
        JsonRpcMessage::Notification(_)
        | JsonRpcMessage::Response(_)
        | JsonRpcMessage::Error(_) => Disposition::Drop,
    }
}

/// Does this outbound frame complete the handshake?
///
/// The key is the `ServerResult::InitializeResult` VARIANT. `EmptyResult` — the pre-handshake `ping`
/// reply — leaves the latch shut, and the match names every `JsonRpcMessage` variant so an rmcp bump
/// is a compile error here too.
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

/// A `Transport<RoleServer>` decorator that passes only `initialize` and `ping` upward until the
/// server's own `InitializeResult` has gone out.
///
/// It is slotted between `VersionClampingTransport` and [`crate::wire::DupScanningTransport`]
/// ([`crate::server`]), so the receive order is scan, gate, clamp and the send order is clamp, gate,
/// scan. The module doc carries the rules; this type carries the latch.
pub(crate) struct PreHandshakeGateTransport<T> {
    /// The wrapped transport. Every call delegates to it, `receive` after the classification and
    /// `send` after the latch check.
    inner: T,
    /// Shut until the server's own `InitializeResult` passes through [`Self::send`]. A plain `bool`
    /// suffices because `send` and `receive` both take `&mut self`, so the borrow checker gives each
    /// call exclusive access to this field. rmcp drives `send` and `receive` from one task and
    /// spawns only the write FUTURE that `send` returns, and that future never touches the latch.
    initialize_answered: bool,
}

impl<T> PreHandshakeGateTransport<T> {
    /// Wrap `inner` with the latch shut.
    pub(crate) fn new(inner: T) -> Self {
        Self {
            inner,
            initialize_answered: false,
        }
    }
}

impl<T> Transport<RoleServer> for PreHandshakeGateTransport<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    /// Open the latch on the way out, then delegate.
    ///
    /// The latch is set in the SYNCHRONOUS head, before the delegated future is returned, because
    /// `Transport::send` requires that future to be `'static` and it therefore cannot touch `self`.
    /// Setting it here also makes the edge the frame's ARRIVAL at the transport rather than the
    /// completion of its write. Nothing races that write in rmcp 1.7.0, which calls `receive()`
    /// nowhere between the break at `rmcp-1.7.0/src/service/server.rs:191` and the write at
    /// `:240-248`. The ordering is forward-looking, so a future rmcp that overlapped the two could
    /// not read a stale latch.
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        if completes_the_handshake(&item) {
            self.initialize_answered = true;
        }
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            let message = self.inner.receive().await?;
            if self.initialize_answered {
                return Some(message);
            }
            match classify_pre_handshake_frame(&message) {
                Disposition::Pass => return Some(message),
                Disposition::Answer(id) => {
                    // The frame itself is deliberately NOT logged at any level, because it is
                    // untrusted input and the reply echoes nothing from it but the id.
                    tracing::debug!(
                        "a request arrived before the initialize handshake; answering -32600 on its own id"
                    );
                    let reply = TxJsonRpcMessage::<RoleServer>::error(
                        ErrorData::invalid_request(PRE_HANDSHAKE_REJECTION_MESSAGE, None),
                        Some(id),
                    );
                    // A failed write ends the read with `None` from `receive()`, which is D47's
                    // `answer_error` contract and reaches D40's teardown delegation.
                    self.inner.send(reply).await.ok()?;
                }
                Disposition::Drop => {
                    tracing::debug!(
                        "a notification, response or error frame arrived before the initialize handshake; dropping it with no reply"
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
    use super::{PRE_HANDSHAKE_REJECTION_MESSAGE, PreHandshakeGateTransport};
    use rmcp::model::{
        ClientJsonRpcMessage, ClientRequest, ErrorCode, InitializeResult, JsonRpcError,
        JsonRpcMessage, ProtocolVersion, RequestId, ServerCapabilities, ServerJsonRpcMessage,
        ServerResult,
    };
    use rmcp::service::RoleServer;
    use rmcp::transport::Transport;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A well-formed `initialize` request, id 2 — the frame every cell ends on, so each one lands a
    /// POSITIVE outcome rather than only the absence of a death.
    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gate-cell","version":"0"}}}"#;

    /// A well-formed `ping` request, id 1 — the one non-`initialize` request that passes.
    const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

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
        /// When set, every `send` fails, which drives the failed-reply-write arm.
        write_fails: bool,
    }

    impl Transport<RoleServer> for MockInner {
        type Error = std::io::Error;

        fn send(
            &mut self,
            item: ServerJsonRpcMessage,
        ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
            let log = Arc::clone(&self.written);
            let fails = self.write_fails;
            async move {
                if fails {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "the scripted write half is broken",
                    ));
                }
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

    /// Build a gate over a scripted inbound sequence, returning the gate and the write log.
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
            write_fails: false,
        };
        (PreHandshakeGateTransport::new(inner), written)
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

    /// Read one logged frame as a JSON-RPC error, naming what was logged instead.
    fn as_error(entry: &ServerJsonRpcMessage) -> JsonRpcError {
        match entry {
            JsonRpcMessage::Error(error) => error.clone(),
            other => panic!("the gate must write an error frame, got {other:?}"),
        }
    }

    /// Assert one logged frame is the gate's rejection, carried on `id`.
    fn assert_rejection(entry: &ServerJsonRpcMessage, id: i64) {
        let error = as_error(entry);
        assert_eq!(
            error.id,
            Some(RequestId::Number(id)),
            "the rejection must carry the refused frame's OWN id"
        );
        assert_eq!(
            error.error.code,
            ErrorCode::INVALID_REQUEST,
            "the rejection is -32600 Invalid Request"
        );
        assert_eq!(
            error.error.message, PRE_HANDSHAKE_REJECTION_MESSAGE,
            "the message is the compile-time constant, verbatim"
        );
        assert!(
            error.error.data.is_none(),
            "the reply echoes no client bytes beyond the id, so `data` is absent: {:?}",
            error.error.data
        );
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

    /// **A premature REQUEST is answered on its OWN id and never delivered.**
    ///
    /// The `tools/list` frame vanishes from the receive sequence and the `initialize` behind it is
    /// what rmcp sees, so the cell lands the positive outcome rather than the absence of a death.
    /// Passing `None` as the reply id turns the id assertion red.
    #[tokio::test]
    async fn a_premature_request_is_answered_on_its_own_id_and_dropped() {
        let script = [
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}"#,
            INITIALIZE,
        ];
        let (mut gate, written) = gate(&script);

        let delivered = gate.receive().await.expect("the handshake frame survives");
        assert_is_initialize(&delivered);

        let entries = log(&written);
        assert_eq!(entries.len(), 1, "exactly one reply per refused request");
        assert_rejection(&entries[0], 7);

        assert!(
            gate.receive().await.is_none(),
            "the refused frame is dropped, so the script is exhausted"
        );
    }

    /// **A premature NOTIFICATION is dropped with no reply, and it does not open the latch.**
    ///
    /// The script puts `notifications/initialized` first and a `tools/list` behind it. A gate that
    /// answered the notification would log a second frame, and a latch opened by that notification
    /// would deliver the `tools/list` instead of refusing it, so both mutations turn this cell red.
    #[tokio::test]
    async fn a_premature_notification_is_dropped_and_leaves_the_latch_shut() {
        let script = [
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/list","params":{}}"#,
            INITIALIZE,
        ];
        let (mut gate, written) = gate(&script);

        let delivered = gate.receive().await.expect("the handshake frame survives");
        assert_is_initialize(&delivered);

        let entries = log(&written);
        assert_eq!(
            entries.len(),
            1,
            "the notification writes nothing, so the only reply is the refused request's: {entries:?}"
        );
        assert_rejection(&entries[0], 8);
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
    /// rmcp answers the pre-handshake `ping` itself, so the gate must hand it upward and write
    /// nothing. Dropping `PingRequest` from the pass arm turns this cell red at the first assertion.
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
    /// The `tools/list` behind the handshake is delivered rather than refused, and the write log
    /// holds only the handshake response the cell sent. Never setting the latch turns this red.
    #[tokio::test]
    async fn the_latch_opens_on_the_initialize_result() {
        let script = [
            INITIALIZE,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/list","params":{}}"#,
        ];
        let (mut gate, written) = gate(&script);

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        gate.send(initialize_result(ProtocolVersion::LATEST))
            .await
            .expect("the handshake response goes out");

        let delivered = gate.receive().await.expect("the next request is delivered");
        match &delivered {
            JsonRpcMessage::Request(request) => assert!(
                matches!(request.request, ClientRequest::ListToolsRequest(_)),
                "a post-handshake request passes through, got {:?}",
                request.request
            ),
            other => panic!("a post-handshake request passes through, got {other:?}"),
        }

        let entries = log(&written);
        assert_eq!(
            entries.len(),
            1,
            "the handshake response is the only frame written: {entries:?}"
        );
        assert!(
            matches!(&entries[0], JsonRpcMessage::Response(_)),
            "and it is the response the cell sent, never a rejection: {:?}",
            entries[0]
        );
    }

    /// **A SUPPORTED but non-latest `InitializeResult` opens the latch too.**
    ///
    /// `2025-03-26` is a member of `ProtocolVersion::KNOWN_VERSIONS` while `LATEST` is `2025-11-25`,
    /// so a latch keyed on the version STRING leaves it shut here and refuses the `tools/list`. The
    /// variant key opens it, which is what this cell pins and no other one does.
    #[tokio::test]
    async fn a_supported_non_latest_initialize_result_still_opens_the_latch() {
        let script = [
            INITIALIZE,
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/list","params":{}}"#,
        ];
        let (mut gate, written) = gate(&script);

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        assert_ne!(
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::LATEST,
            "non-vacuity: the cell's version must really differ from LATEST"
        );
        gate.send(initialize_result(ProtocolVersion::V_2025_03_26))
            .await
            .expect("the handshake response goes out");

        let delivered = gate.receive().await.expect("the next request is delivered");
        match &delivered {
            JsonRpcMessage::Request(request) => assert!(
                matches!(request.request, ClientRequest::ListToolsRequest(_)),
                "a version-keyed latch refuses this frame, got {:?}",
                request.request
            ),
            other => panic!("a post-handshake request passes through, got {other:?}"),
        }
        assert_eq!(
            log(&written).len(),
            1,
            "no rejection is written after the latch opens"
        );
    }

    /// **The pre-handshake `ping` reply leaves the latch shut.**
    ///
    /// rmcp answers a pre-handshake `ping` with `ServerResult::EmptyResult`, which travels the same
    /// `send` path as the handshake response. A latch opened by `EmptyResult` would deliver the
    /// `tools/list` behind it instead of refusing it.
    #[tokio::test]
    async fn the_ping_reply_leaves_the_latch_shut() {
        let script = [
            PING,
            r#"{"jsonrpc":"2.0","id":11,"method":"tools/list","params":{}}"#,
            INITIALIZE,
        ];
        let (mut gate, written) = gate(&script);

        gate.receive().await.expect("the ping reaches rmcp");
        gate.send(ServerJsonRpcMessage::response(
            ServerResult::empty(()),
            RequestId::Number(1),
        ))
        .await
        .expect("the ping reply goes out");

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));

        let entries = log(&written);
        assert_eq!(
            entries.len(),
            2,
            "the ping reply and the rejection behind it: {entries:?}"
        );
        assert_rejection(&entries[1], 11);
    }

    /// **A request arriving before `notifications/initialized` is served.**
    ///
    /// rmcp enters its main loop straight after writing the handshake response and never waits for
    /// that notification, so a gate that waited would be stricter than the server it decorates. The
    /// script delivers the `tools/list` BEFORE the notification and the notification after it, and
    /// both pass.
    #[tokio::test]
    async fn a_request_arriving_before_the_initialized_notification_is_served() {
        let script = [
            INITIALIZE,
            r#"{"jsonrpc":"2.0","id":12,"method":"tools/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ];
        let (mut gate, written) = gate(&script);

        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));
        gate.send(initialize_result(ProtocolVersion::LATEST))
            .await
            .expect("the handshake response goes out");

        let early = gate
            .receive()
            .await
            .expect("the request ahead of the notification is delivered");
        match &early {
            JsonRpcMessage::Request(request) => assert!(
                matches!(request.request, ClientRequest::ListToolsRequest(_)),
                "the early request is served, got {:?}",
                request.request
            ),
            other => panic!("the early request is served, got {other:?}"),
        }
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
            "nothing is refused after the latch opens"
        );
    }

    /// **A `"method":"initialize"` frame whose `params` do not type is answered and dropped.**
    ///
    /// The `params` object here carries `protocolVersion` and omits the required `capabilities` and
    /// `clientInfo`, so rmcp's untagged `ClientRequest` falls through to `CustomRequest`. rmcp's own
    /// handshake match keys on the `InitializeRequest` VARIANT, so a `method`-string gate would pass
    /// this frame upward into the same `ExpectedInitializeRequest` death. The cell asserts the
    /// decoded variant first, which is what tells a variant-matched classifier from a `method`-string
    /// one, and under such a gate the `assert_is_initialize` call below goes red.
    ///
    /// The `params` must be an OBJECT to reach `CustomRequest` at all. A scalar `params` fails every
    /// variant, so [`crate::wire`] answers it `-32700` and the frame never reaches this gate.
    #[tokio::test]
    async fn a_method_initialize_frame_with_untypeable_params_is_answered_and_dropped() {
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

        let (mut gate, written) = gate(&[untypeable, INITIALIZE]);
        assert_is_initialize(&gate.receive().await.expect("the handshake frame survives"));

        let entries = log(&written);
        assert_eq!(entries.len(), 1, "exactly one reply: {entries:?}");
        assert_rejection(&entries[0], 5);
    }

    /// **A failed reply write ends the read.**
    ///
    /// `receive()` returns `None` on a write failure, which is D47's `answer_error` contract and what
    /// routes the process into D40's teardown delegation. The `initialize` behind the refused frame is
    /// what gives the cell teeth — swallowing the failure hands that request upward on a dead pipe,
    /// and the `is_none()` assertion turns red on it.
    #[tokio::test]
    async fn a_failed_reply_write_ends_the_receive() {
        let inner = MockInner {
            inbound: [
                frame(r#"{"jsonrpc":"2.0","id":13,"method":"tools/list","params":{}}"#),
                frame(INITIALIZE),
            ]
            .into_iter()
            .collect(),
            written: Arc::new(Mutex::new(Vec::new())),
            write_fails: true,
        };
        let mut gate = PreHandshakeGateTransport::new(inner);
        assert!(
            gate.receive().await.is_none(),
            "a failed rejection write ends the read"
        );
    }
}
