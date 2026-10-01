//! The WIRE seam (D43) — an **owned** `Transport<RoleServer>` that scans the raw frame bytes for a
//! duplicate JSON key BEFORE `serde_json` collapses it, and stamps the verdict on the message.
//!
//! # Why an owned transport and not a decorator
//!
//! Two structural impossibilities force this shape:
//!
//! 1. A fix at the argument seam ([`crate::tools::args`]) is impossible: `Parameters` receives
//!    `context.arguments`, a `JsonObject` rmcp already built — the duplicate was collapsed before
//!    that type existed.
//! 2. A fix in a `Transport` **decorator** of the [`crate::server`] `VersionClampingTransport` kind
//!    is impossible for the same reason one level up: `receive()` hands a decorator an
//!    already-parsed `RxJsonRpcMessage`.
//!
//! So detection has to own the read framing. A byte-sniffing `AsyncRead` shim feeding a side queue
//! was rejected: `AsyncRwTransport::receive` consumes lines WITHOUT emitting a message on blank
//! lines, on compat-ignored notifications and on parse-error lines, so any arrival-ordered queue
//! desynchronises after the first such line. Worse, id-correlation is unsound in principle —
//! JSON-RPC request ids are client-chosen (they may repeat) and rmcp dispatches concurrently, so a
//! correlated queue can hand a `Clean` verdict computed for one frame to a DIFFERENT frame. That
//! fails **OPEN**.
//!
//! # THE TRANSPORT NEVER REPLIES AND NEVER SHORT-CIRCUITS *FOR THE D43 DUPLICATE-KEY CLASS* (normative)
//!
//! On a duplicate or an indeterminate scan it **still parses and still delivers** the message,
//! carrying the verdict, and adds NO reply of its own for that class, because that class HAS an
//! in-band channel: inventing a reply for it would force the out-of-band `-32602`/`-32700` arm back
//! open for a class the binding decision says must be answered IN-BAND. The only replies such a
//! frame can draw here are out-of-band ones its ENVELOPE draws, never one for the duplicate: the
//! `-32700` that ANY line failing the typed parse draws (carrying the D54 recovered id when there
//! is one), or D47's `-32600` when its envelope `id` is duplicated or does not decode. Rejection
//! for it happens at exactly one site: `call_tool` (`crate::server`).
//!
//! # THE ONE CLASS THE TRANSPORT ANSWERS WHERE RMCP STAYS SILENT — UN-DECODABLE ENVELOPE `id` (normative, PRD §4, D47)
//!
//! The two rules do not conflict, because this class never reaches `call_tool` at all. A frame whose
//! RAW BYTES carried a top-level `id` member and whose decode produced a `Notification` has no
//! response obligation in rmcp and no in-band channel here, so before D47 it evaporated: no reply,
//! no store effect, nothing on stdout, and a client that sent an id waiting forever. Duplication is
//! the MINORITY route — `null`, an object, an array, a boolean, a non-integer or out-of-i64-range
//! number, and the `id` key spelled "\u0069d" all land in the same place, because rmcp
//! tries the `Request` variant first and `JsonRpcRequest` requires an `id` that deserializes as a
//! number-or-string.
//!
//! The transport answers it **out-of-band `-32600 Invalid Request`**, on the id RECOVERED from the
//! raw line when the bytes yield one unambiguously (a single valid `RequestId`, or several all
//! EQUAL), with the id omitted when they do not (two different ids, or a value that is no
//! representable `RequestId`). Answering on the recovered id is the whole mechanism, not a nicety:
//! rmcp's client awaits untimed and DISCARDS an error that carries no id, so an id-less reply never
//! releases it.
//!
//! It then **DROPS** the frame. Dropping is required rather than tidy, and since **D50** it rests on
//! two reasons that outlive the fatality it once rested on. An answered frame must not ALSO take
//! effect, because rmcp's serve loop converts every delivered notification with
//! `TryInto<CancelledNotification>` before any handler runs (`rmcp-1.7.0/src/service.rs:981-996`).
//! And pre-handshake a delivered frame meets the D50 gate above this layer and is dropped there with
//! no reply and no recovered id, so answering-and-dropping here stays the only spelling that releases
//! a waiting peer.
//!
//! The predicate is [`crate::envelope_id::scan`], a `DeserializeSeed` over the ROOT object collecting
//! every top-level `id` member's value, guarded by an EXHAUSTIVE match on the `Notification` variant
//! so request traffic pays nothing. Keys are compared DECODED, never as raw spans.
//! [`unblock_error::dup_key::scan`] CANNOT serve as this predicate: it reports `Clean` for every
//! non-duplicated shape of the class, and its `Duplicate { key, path }` verdict retains no occurrence
//! VALUES, so equal and differing ids are indistinguishable to it.
//!
//! # THE `-32700` REPLY CARRIES A READABLE ID (normative, PRD §4, D54; closes `ub-788`)
//!
//! A line that FAILS the typed parse is answered `-32700 Parse error` exactly as rmcp answers it
//! (`rmcp-1.7.0/src/transport/async_rw.rs:145-153`), with ONE addition: the `id` recovered from the
//! raw bytes. All three legs are required: the typed parse failed; the line is STRICT JSON — after
//! the one BOM strip it is UTF-8 and `serde_json`'s `Value` parse accepts it; and
//! [`crate::envelope_id::scan`] returns `Recovered`. A line the compatibility filter drops (a
//! last-wins `method` that is a non-standard `notifications/*`) gets NO reply, as in rmcp (D54
//! clause (7)). Every other failed line — not strict JSON, no root `id`, differing ids, or an id
//! that is no `RequestId` — gets the id-less `-32700`,
//! byte-identical to rmcp's. The code stays `-32700`, so the `id` member is the ONLY byte the fork
//! adds. Answering on the id is the whole mechanism, as it is for D47: rmcp's client DROPS an id-less
//! error (`rmcp-1.7.0/src/service.rs:1030-1036`) while `Peer::send_request` awaits untimed
//! (`:442-447`).
//!
//! The strictness leg is NOT a second parse: it is the `Value` parse rmcp's compatibility filter
//! already runs on every failed UTF-8 line (`async_rw.rs:289`), surfaced as [`ParseFailure`]. It is
//! load-bearing: `scan` drains non-`id` members with `IgnoredAny`, which checks no nesting depth,
//! validates neither UTF-8 nor surrogate escapes inside the strings it skips, and range-checks no
//! number. Without the gate, trailing bytes after a complete object, a non-UTF-8 byte or a lone
//! surrogate inside a skipped string, and nesting past `serde_json`'s 128-level limit would each
//! recover an id from a line that is not JSON.
//!
//! # `receive()` IS CANCEL-SAFE (normative — D47 clause 8(v), closed by `ub-nbz`; D53)
//!
//! rmcp polls `receive()` as one arm of an UNBIASED `tokio::select!` (`src/service.rs:805`, the arm
//! at `:813`) and DROPS it whenever another arm wins, most often a handler response. So two kinds of
//! state must not live only inside the `receive()` future, and neither does.
//!
//! 1. **An out-of-band reply is never written inside `receive()`.** Both out-of-band arms, D47's
//!    `-32600` and the `-32700`, hand their reply to a `tokio::spawn`ed task. The task runs the
//!    SAME `'static` write future `send()` builds, and its `JoinHandle` is PARKED on the transport.
//!    The handle is awaited at the top of EVERY loop iteration, so the reply is on the wire before
//!    the next frame is read, which is the order an inline write gave. It is also awaited in
//!    `close()` before the write half is taken. `close()` waits for it exactly as it waits for any
//!    rmcp send holding the write lock, so a peer that has stopped reading holds a signalled
//!    teardown until it reads or a second signal escalates (D38 clause (2)). A write that fails, or
//!    a task that panics, ends that SAME `receive()` call with `None` (or, if that call was dropped,
//!    the NEXT one), which is the contract the inline write shipped and what D40's teardown reads. A
//!    dropped `receive()` drops only its borrow of the handle. The task keeps running, and the next
//!    call or `close()` settles it. The pre-handshake EOF path drops the whole transport without
//!    `close()`, but it reads that EOF only after the loop head has settled the reply, so the reply
//!    is still written. On the pre-handshake SIGNAL path rmcp drops the transport without
//!    `close()`; a parked reply's task is detached, keeps the write half, and finishes the reply
//!    unless the process exits first.
//! 2. **A dropped `receive()` never discards a partially read line** (D53). tokio's `read_until`
//!    leaves the bytes it consumed in the buffer when it is dropped, and documents calling it again
//!    as the recovery. So `line_buf` is cleared only after a COMPLETE line has been read and
//!    processed (or after a read error, below), never at the top of an iteration that a dropped
//!    call may have left half-filled. `Ok(0)` with a non-empty buffer is therefore the
//!    unterminated final line. Clearing first would destroy the head of a well-formed request
//!    split across reads, which happens to any frame larger than one 8 KiB `BufReader` fill, and
//!    to one a client writes in pieces. The tail would then be answered `-32700` on its own or,
//!    when the split falls on the terminator, skipped as an empty line with no reply at all;
//!    either way the request was never served. A read ERROR is different: it ends `receive()` with
//!    `None` and discards the half-read line, as rmcp does, so a call after it starts a fresh line.
//!
//! The two hazards are independent: the out-of-band frame was already read in full and is dropped
//! by design, so only the read side ever lost a frame.
//!
//! `tokio::spawn` makes a TOKIO RUNTIME a precondition of `receive()`, because it panics outside one.
//! Every driver already is one: rmcp spawns its own response writes (`src/service.rs:892`), and the
//! stdio pair is tokio's.
//!
//! The write mutex still buys only BYTE-ATOMICITY between concurrent writers. Cancellation-safety
//! comes from the task owning the write, which a dropped `receive()` cannot reach. A RUNTIME shutdown
//! can still abort that task mid-frame, exactly as it can any rmcp send task.
//!
//! ONE EFFECT IS REMOVED, deliberately: a `notifications/cancelled` frame carrying an un-decodable
//! `id` is DELIVERED today, and rmcp's serve loop cancels the matching in-flight request through it
//! before any handler runs (`src/service.rs:981-996`). Answered and dropped, that cancellation stops
//! happening. Preserving it would mean delivering the frame after answering it — the shape that kills
//! the server in the initialize slot. A conforming cancellation carries no `id` and is unaffected.
//!
//! # CD-7 — this module FORKS an undocumented rmcp internal
//!
//! `AsyncRwTransport`'s framing helpers (`try_parse_with_compatibility`, `should_ignore_notification`,
//! `is_standard_method`, `is_standard_notification`, `without_carriage_return`) are all PRIVATE to
//! rmcp's `transport::async_rw` module, so reproducing the read contract means re-implementing them.
//!
//! **The compatibility filter runs only AFTER the typed parse has already failed** (the mechanism
//! stated at [`should_ignore_notification`]). So it is *not* what makes an unknown notification
//! work: a `notifications/whatever` frame with a well-formed `params` object is accepted by rmcp's
//! own catch-all `CustomNotification` and DELIVERED, never reaching the filter. What the filter
//! governs is the narrower class rmcp cannot type at all — an LSP-style `$/cancelRequest`, or a
//! `notifications/*` frame whose `params` are not an object. Dropping it would answer `-32700` to
//! frames rmcp silently ignores: a JSON-RPC violation and an interop regression.
//!
//! That mechanism dictates how the fork must be pinned. The **differential harness** at the bottom
//! of this file (the CD-6 assumption-pin pattern) feeds ONE byte corpus to
//! `AsyncRwTransport::new_server` AND to [`DupScanningTransport`], asserting identical `receive()`
//! sequences and identical bytes written — but a corpus of frames that all parse cleanly executes
//! ZERO of the forked filter lines and stays green with the whole branch deleted. The corpus
//! therefore carries entries that FAIL the typed parse — F15 (the id-less non-standard-method arm),
//! F17 (the `notifications/*`-prefix arm, reachable ONLY with an `id` present) and F14/F16 (the
//! ignored and not-ignored sides) — and `the_compatibility_filter_is_entered_and_discriminates`
//! drives those same arms directly. Those two together are what stands between an rmcp bump and a
//! silent framing divergence; neither alone suffices, because each covers a mutation the other
//! survives.
//!
//! The WRITE half does not fork anything: it encodes through rmcp's own public
//! [`JsonRpcMessageCodec`], so the emitted bytes are identical by construction rather than by test.
//!
//! **THE FORK CARRIES THREE DELIBERATE DIVERGENCES FROM `AsyncRwTransport`, AND ONLY THREE.**
//!
//! 1. **D47, answer and drop.** A frame of the un-decodable-envelope-`id` class is answered `-32600`
//!    and never delivered, where rmcp delivers it as a Notification. The divergence corpus and its
//!    per-entry differential tier pin it (`the_diverging_entries_are_exactly_the_declared_ones`).
//! 2. **D53, a partial line survives a dropped `receive()`.** Under CANCELLATION the fork keeps a
//!    partially read line, where `AsyncRwTransport::receive` clears its buffer at the top of every
//!    iteration (`rmcp-1.7.0/src/transport/async_rw.rs:127-128`) and so loses a well-formed request
//!    split across reads. In a run that is never cancelled, every `read_until` completes and the two
//!    framings are byte-identical, and that is all the differential harness can observe. So the
//!    harness cannot see this divergence. It is pinned instead by
//!    `a_line_split_by_a_dropped_receive_is_delivered_whole` and
//!    `an_unterminated_line_split_by_a_dropped_receive_is_delivered_at_eof`.
//! 3. **D54, the `-32700` reply carries a readable id.** For a line in the D54 class (the section
//!    above), the fork writes rmcp's own id-less `-32700` plus EXACTLY the recovered `id` member. The
//!    per-entry differential's `IdInserted` tier pins it, asserting BOTH that rmcp's bytes are still
//!    the id-less `-32700` (so an rmcp that starts writing the id itself, or changes the message,
//!    code or framing, turns it red) AND that ours are rmcp's with the declared id spliced in;
//!    `the_diverging_entries_are_exactly_the_declared_ones` pins that no other entry diverges.
//!
//! The spawn-and-park reply path (`ub-nbz`) is not counted as a divergence of its own. In a run that is
//! never cancelled it changes no byte and no order; it changes only whether an out-of-band reply
//! survives a dropped `receive()`. rmcp awaits its own `-32700` inside `receive()` too
//! (`rmcp-1.7.0/src/transport/async_rw.rs:145-153`) and loses it the same way, so under
//! cancellation the fork writes a reply that rmcp would lose. That is recorded as the close of D47
//! clause 8(v), not as a framing divergence.

use std::sync::Arc;

use rmcp::model::{ErrorData, JsonRpcMessage, RequestId};
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use rmcp::transport::async_rw::JsonRpcMessageCodec;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::Encoder;
use unblock_error::dup_key::{DupScan, scan};

use crate::envelope_id::{self, EnvelopeId};

/// The scan root: the WHOLE `params` value of every decoded request — the reserved `_meta` member
/// included, NOT `params.arguments` alone.
///
/// `_meta` is attacker-controlled, is measured by the request quota, and reaches `call_tool` as
/// `context.meta`, so it has exactly the same in-band channel `arguments` does; excluding it would
/// leave one nested-duplicate class executing.
const SCAN_ROOT: &[&str] = &["params"];

/// The `-32600` message for the D47 un-decodable-envelope-id arm.
///
/// A COMPILE-TIME CONSTANT on purpose: zero attacker bytes are echoed into it, and `data` is `None`
/// for the same reason — so the reply's member set is exactly the shipped `-32700` reply's plus the
/// (protocol-mandated) `id`. A `data` carrying anything derived from the frame would open a NEW echo
/// channel for untrusted input with no protocol requirement behind it.
const INVALID_REQUEST_ID_MESSAGE: &str =
    "Invalid Request: the id member is duplicated or is not a valid JSON-RPC request id";

/// UTF-8 byte order mark — RFC 8259 §8.1. Stripped exactly once, prefix only, mirroring rmcp.
///
/// `pub(crate)` so [`crate::envelope_id`] strips exactly the SAME one: the scanner and the parser
/// must see the same document, and two copies of this constant is how they drift.
pub(crate) const UTF8_BOM: &[u8; 3] = b"\xEF\xBB\xBF";

/// The wire-scan verdict carried from the transport to `call_tool` (D43).
///
/// # Why this cannot be forged
///
/// It rides `rmcp::model::Extensions`, a `TypeId`-keyed typemap with **no `Serialize`/`Deserialize`
/// impl at all**, so no wire field can name it. The only type rmcp itself ever inserts on the
/// deserialize path is `rmcp::model::Meta`, sourced from the typed `params._meta` member.
///
/// **⚠️ THE DIRECTION IS THE WHOLE SECURITY PROPERTY.** An attacker cannot make the marker
/// *present*, but **absent is the default state of `Extensions::new()`** — so "present ⇒ duplicate,
/// absent ⇒ clean" would make any path reaching a handler without traversing this transport fail
/// **OPEN**. The gate therefore rejects the ABSENT verdict too (`crate::server::frame_scan_gate`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParamsScan {
    /// No duplicated key anywhere inside the request's `params` subtree.
    Clean,
    /// A duplicated key was found; `path` is an RFC 6901 pointer relative to `params`.
    Duplicate {
        /// The decoded duplicated key.
        key: String,
        /// The pointer to the object carrying the duplicate, relative to `params`.
        path: String,
    },
    /// The frame bytes could not be tokenized to a decision. **Never equivalent to `Clean`.**
    Indeterminate,
}

impl From<DupScan> for ParamsScan {
    fn from(value: DupScan) -> Self {
        match value {
            DupScan::Clean => Self::Clean,
            DupScan::Duplicate { key, path } => Self::Duplicate { key, path },
            DupScan::Indeterminate => Self::Indeterminate,
        }
    }
}

/// An owned `Transport<RoleServer>` over a byte stream pair. It reproduces `AsyncRwTransport`'s read
/// framing exactly in any run that is never cancelled, adds the D43 duplicate-key scan between the
/// read and the parse, and keeps a partially read line across a dropped `receive()` where rmcp loses
/// it (D53, the CD-7 section).
pub(crate) struct DupScanningTransport<R, W> {
    /// The buffered read half. `read_until(b'\n', ..)` — there is deliberately no line-length bound
    /// here, exactly as rmcp has none (`max_length: usize::MAX`).
    read: BufReader<R>,
    /// The reusable line buffer, mirroring rmcp's `line_buf`. It is cleared only once the line in it
    /// was COMPLETELY read (`line_complete`) or abandoned by a read error, never at the top of an
    /// iteration that a dropped `read_until` may have left half-filled (D53).
    line_buf: Vec<u8>,
    /// `true` when the next iteration must clear `line_buf`: the line in it was read to its end, or
    /// a read error ended the call and abandoned it, as rmcp does. `false` while the bytes of a
    /// `read_until` interrupted by a dropped `receive()` are still waiting to be resumed.
    line_complete: bool,
    /// The write half. The `Arc<Mutex<Option<W>>>` shape is what satisfies `Transport::send`'s
    /// `+ Send + 'static` return bound (the future must not borrow `self`), and `Option` is what
    /// makes a post-`close()` `send` fail with `NotConnected` instead of writing to a dead pipe.
    write: Arc<Mutex<Option<W>>>,
    /// The out-of-band reply in flight, if any. [`Self::park_reply`] spawns it so that a dropped
    /// `receive()` cannot take it along (`ub-nbz`). It is `None` whenever `read_until` is entered and
    /// whenever `receive()` returns: the loop head settles it before every read, and every arm that
    /// parks one loops back to that head.
    parked_reply: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl<R, W> DupScanningTransport<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    /// Wrap a read/write pair.
    pub(crate) fn new(read: R, write: W) -> Self {
        Self {
            read: BufReader::new(read),
            line_buf: Vec::new(),
            write: Arc::new(Mutex::new(Some(write))),
            parked_reply: None,
            line_complete: false,
        }
    }

    /// Hand ONE out-of-band error reply to its own task and park the handle; the next loop head
    /// ([`Self::settle_parked_reply`]) awaits it (`ub-nbz`).
    ///
    /// Both out-of-band arms, `-32700` and D47's `-32600`, go through here, so they are identical
    /// **by construction** rather than by review.
    ///
    /// **Takes two FIELDS, not `&mut self`.** `receive()` still holds `line`, an immutable borrow of
    /// `self.line_buf`, at every call site. Borrowing only `parked_reply` (mutably) and `write`
    /// (shared) keeps the borrows disjoint.
    ///
    /// Since **D50** the scanner is not the only emitter. The gate in [`crate::pre_handshake`] writes
    /// its own `-32600` through `self.inner.send(..)`, which is this transport's `send`, so that reply
    /// is byte-atomic under the same write mutex. It cannot route through here, because a decorator
    /// generic over `T: Transport` cannot reach this private function.
    fn park_reply(
        slot: &mut Option<tokio::task::JoinHandle<std::io::Result<()>>>,
        write: &Arc<Mutex<Option<W>>>,
        error: ErrorData,
        id: Option<RequestId>,
    ) {
        debug_assert!(
            slot.is_none(),
            "a reply is parked only after the loop head settled the previous one"
        );
        let item = TxJsonRpcMessage::<RoleServer>::error(error, id);
        *slot = Some(tokio::spawn(write_owned(Arc::clone(write), item)));
    }

    /// Await the parked reply, if any, and clear the slot.
    ///
    /// `None` ⇒ the caller must end the read with `None`. That happens when the write failed, when
    /// the write half was taken by `close()` (`NotConnected`), or when the task panicked (a
    /// `JoinError`). These are the conditions the inline write returned `None` on, plus the panic.
    ///
    /// The handle is awaited through `&mut`, so a `receive()` dropped here leaves it parked. The task
    /// runs on regardless, and the next call settles it.
    async fn settle_parked_reply(&mut self) -> Option<()> {
        let Some(handle) = self.parked_reply.as_mut() else {
            return Some(());
        };
        let joined = handle.await;
        self.parked_reply = None;
        match joined {
            Ok(Ok(())) => Some(()),
            Ok(Err(_)) | Err(_) => None,
        }
    }
}

impl<R, W> Transport<RoleServer> for DupScanningTransport<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        write_owned(self.write.clone(), item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            // ub-nbz: the previous frame's reply is on the wire, or has ended the read, before the next
            // frame is read. That is the order an inline write gave.
            self.settle_parked_reply().await?;
            // D53: clear only a line that was read to its END, or one a read error abandoned
            // (below). A `read_until` interrupted by a dropped `receive()` left the frame's head in
            // `line_buf`, and this call resumes it.
            if self.line_complete {
                self.line_buf.clear();
                self.line_complete = false;
            }
            match self.read.read_until(b'\n', &mut self.line_buf).await {
                // EOF with nothing pending. EOF with a NON-EMPTY buffer is the unterminated final line,
                // possibly begun by a dropped call, and it is processed like any other line.
                Ok(0) if self.line_buf.is_empty() => return None,
                Ok(_) => self.line_complete = true,
                Err(e) => {
                    // Nothing is swallowed here: the EOF/error shape is what the D40 pre-handshake
                    // teardown depends on.
                    tracing::error!("Error reading from stream: {}", e);
                    // rmcp clears at the loop head, so a re-call after an error starts a fresh line.
                    self.line_complete = true;
                    return None;
                }
            }
            // Strip ONE trailing `\n` (an unterminated final line at EOF is still processed), then
            // ONE trailing `\r`. A whitespace-only line is NOT empty and proceeds to the parse.
            let line = without_carriage_return(
                self.line_buf
                    .strip_suffix(b"\n")
                    .unwrap_or(self.line_buf.as_slice()),
            );
            if line.is_empty() {
                continue;
            }

            // D43 — the scan runs on the RAW bytes, before any parse, on EVERY decoded request.
            // Its verdict is CONSULTED at exactly one site (`call_tool`), because only `tools/call`
            // has an in-band channel; a stamped-but-unenforced verdict on other methods is the
            // documented residual.
            let verdict = ParamsScan::from(scan(line, SCAN_ROOT));

            match try_parse_with_compatibility::<RxJsonRpcMessage<RoleServer>>(line) {
                Ok(Some(mut message)) => {
                    // D47 / ub-cnv — the UN-DECODABLE-ENVELOPE-ID class.
                    //
                    // rmcp decodes into an UNTAGGED union (rmcp src/model.rs:575-588) tried
                    // Request-first, and `JsonRpcRequest` requires `id: RequestId` (:431-436). So
                    // ANY frame whose `id` member fails to decode falls through to the Notification
                    // variant — where the server has no response obligation and we register no
                    // notification handler — and EVAPORATES: no reply, no store effect, nothing on
                    // stdout. A client that DID send an id waits forever (`Peer::send_request` uses
                    // `no_options`, src/service.rs:442-447; the non-timeout await is bare,
                    // :344-346; an id-less error is dropped, :1030-1036).
                    //
                    // A frame with NO `id` member is a genuine Notification and is EXPLICITLY out
                    // of scope: it takes the `Absent` arm and behaves exactly as it did before D47.
                    //
                    // The match is EXHAUSTIVE on purpose (no `_` arm): an rmcp bump adding a fifth
                    // `JsonRpcMessage` variant that could carry a stray id must be a COMPILE ERROR
                    // here, not a silent hole.
                    let is_notification = match &message {
                        JsonRpcMessage::Notification(_) => true,
                        JsonRpcMessage::Request(_)
                        | JsonRpcMessage::Response(_)
                        | JsonRpcMessage::Error(_) => false,
                    };
                    if is_notification {
                        // The raw line is deliberately NOT logged at any level:
                        // `try_parse_with_compatibility` already logs it on its own failure path,
                        // and these frames never reach that path.
                        match envelope_id::scan(line) {
                            EnvelopeId::Absent => {}
                            EnvelopeId::Recovered(id) => {
                                tracing::debug!(
                                    "un-decodable envelope id; answering -32600 on the recovered id"
                                );
                                Self::park_reply(
                                    &mut self.parked_reply,
                                    &self.write,
                                    ErrorData::invalid_request(INVALID_REQUEST_ID_MESSAGE, None),
                                    Some(id),
                                );
                                continue; // ANSWER AND DROP — never delivered.
                            }
                            EnvelopeId::Unusable => {
                                tracing::debug!(
                                    "un-decodable envelope id, unrecoverable; answering -32600 with the id omitted"
                                );
                                Self::park_reply(
                                    &mut self.parked_reply,
                                    &self.write,
                                    ErrorData::invalid_request(INVALID_REQUEST_ID_MESSAGE, None),
                                    None,
                                );
                                continue;
                            }
                        }
                    }
                    message.insert_extension(verdict);
                    return Some(message);
                }
                // Compat-ignored (an unknown client notification): emit nothing, answer nothing,
                // read the next line. Spelled as a fall-through rather than `continue` only
                // because it is the last arm; the semantics mirror rmcp's `continue` exactly.
                Ok(None) => {}
                Err(failure) => {
                    tracing::debug!("Parse error on incoming message: {}", failure.error());
                    // D54 (closes `ub-788`): rmcp's own reply, plus the id recovered from the raw
                    // line when the line is strict JSON and its id is readable.
                    let id = parse_error_reply_id(&failure, line);
                    Self::park_reply(
                        &mut self.parked_reply,
                        &self.write,
                        ErrorData::parse_error("Parse error", None),
                        id,
                    );
                    // Recover: loop to the next line. This deliberately does NOT return.
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        // A parked reply is written BEFORE the write half is taken. Its own failure is moot here,
        // because the connection is closing either way. It waits as a response send holding the
        // lock does: a stalled peer holds the teardown here until it reads or a second signal
        // escalates.
        let _ = self.settle_parked_reply().await;
        let mut guard = self.write.lock().await;
        drop(guard.take());
        Ok(())
    }
}

/// The ONE write path of [`DupScanningTransport`]: lock the write half, encode through rmcp's own
/// [`JsonRpcMessageCodec`], write and flush.
///
/// The future is `Send + 'static` because it owns what it touches, a clone of the `Arc` and the
/// message. That is what lets `send()` return it and lets `DupScanningTransport::park_reply` SPAWN
/// it. `send()` and both out-of-band arms therefore write through the same future, so their bytes are
/// identical **by construction** rather than by review. It is a FREE function on purpose: an
/// `async fn` inside the `impl<R, W>` block would capture `R` too, and its future would then not be
/// `'static` for a merely-`Send` `R`.
///
/// The mutex buys BYTE-ATOMICITY: `write_frame` writes a whole frame under one guard, so no concurrent
/// writer can interleave bytes into it. The guard lives only inside this future, never across a
/// `read_until`. A write half already taken by `close()` fails with `NotConnected` instead of writing
/// to a dead pipe (the D40 teardown path).
async fn write_owned<W>(
    lock: Arc<Mutex<Option<W>>>,
    item: TxJsonRpcMessage<RoleServer>,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut guard = lock.lock().await;
    match guard.as_mut() {
        Some(writer) => write_frame(writer, item).await,
        None => Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "Transport is closed",
        )),
    }
}

/// Encode one message through rmcp's OWN codec (`serde_json::to_writer` + a single `b'\n'`, no BOM,
/// no CR) and flush it.
///
/// Going through [`JsonRpcMessageCodec`] rather than hand-rolling the two lines makes the emitted
/// bytes identical to `AsyncRwTransport`'s by construction.
async fn write_frame<W>(writer: &mut W, item: TxJsonRpcMessage<RoleServer>) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut buf = BytesMut::new();
    JsonRpcMessageCodec::<TxJsonRpcMessage<RoleServer>>::default().encode(item, &mut buf)?;
    writer.write_all(&buf).await?;
    writer.flush().await
}

/// Strip ONE trailing `\r`, only at the end (rmcp `without_carriage_return`).
fn without_carriage_return(s: &[u8]) -> &[u8] {
    s.strip_suffix(b"\r").unwrap_or(s)
}

/// Is `method` a standard MCP request or notification? (rmcp `is_standard_method`, MCP 2025-06-18.)
fn is_standard_method(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "ping"
            | "prompts/get"
            | "prompts/list"
            | "resources/list"
            | "resources/read"
            | "resources/subscribe"
            | "resources/unsubscribe"
            | "resources/templates/list"
            | "tools/call"
            | "tools/list"
            | "completion/complete"
            | "logging/setLevel"
            | "roots/list"
            | "sampling/createMessage"
    ) || is_standard_notification(method)
}

/// Is `method` a standard MCP notification? (rmcp `is_standard_notification`.)
fn is_standard_notification(method: &str) -> bool {
    matches!(
        method,
        "notifications/cancelled"
            | "notifications/initialized"
            | "notifications/message"
            | "notifications/progress"
            | "notifications/prompts/list_changed"
            | "notifications/resources/list_changed"
            | "notifications/resources/updated"
            | "notifications/roots/list_changed"
            | "notifications/tools/list_changed"
    )
}

/// Should this frame be silently ignored for client compatibility? (rmcp `should_ignore_notification`.)
///
/// No `id` member + a non-standard method ⇒ ignore (LSP-style traffic). Any `notifications/*` method
/// outside the standard set ⇒ ignore. **This runs ONLY on a serde failure** — a cleanly-parsing
/// frame is never filtered.
fn should_ignore_notification(json_value: &serde_json::Value, method: &str) -> bool {
    let is_notification = json_value.get("id").is_none();
    if is_notification && !is_standard_method(method) {
        return true;
    }
    matches!(
        (
            method.starts_with("notifications/"),
            is_standard_notification(method)
        ),
        (true, false)
    )
}

/// Parse one line with rmcp's compatibility handling (rmcp `try_parse_with_compatibility`).
///
/// `Ok(Some(_))` = deliver, `Ok(None)` = silently ignore, `Err(ParseFailure)` = answer `-32700` and
/// recover; the variant says whether the line was strict JSON, which gates D54's recovered id.
///
/// The BOM is stripped ONCE, prefix only, and `line` is REBOUND to the stripped slice **before**
/// both the primary parse and the compat re-parse — so a BOM-prefixed unknown notification is
/// ignored exactly like an un-prefixed one. Non-UTF-8 input skips the compat branch entirely.
///
/// The error type is [`ParseFailure`], not rmcp's codec error. On the read path rmcp's codec error
/// can only be its `Serde` variant (the length-bounded and I/O variants belong to the `Decoder`
/// path, which `receive()` does not use). The one bit the fork adds is whether the compatibility
/// filter's own `Value` parse ACCEPTED the line — that parse already runs here on every failed UTF-8
/// line, so D54's strict-JSON gate costs no extra parse. A non-UTF-8 line skips the compat branch
/// and is never strict JSON.
fn try_parse_with_compatibility<T: serde::de::DeserializeOwned>(
    line: &[u8],
) -> Result<Option<T>, ParseFailure> {
    let line = line.strip_prefix(UTF8_BOM.as_slice()).unwrap_or(line);
    if let Ok(line_str) = std::str::from_utf8(line) {
        match serde_json::from_slice(line) {
            Ok(item) => Ok(Some(item)),
            Err(e) => {
                let json_value = serde_json::from_str::<serde_json::Value>(line_str);
                if let Ok(json_value) = &json_value
                    && let Some(method) =
                        json_value.get("method").and_then(serde_json::Value::as_str)
                    && should_ignore_notification(json_value, method)
                {
                    return Ok(None);
                }
                tracing::debug!("Failed to parse message receive: {line_str} | Error: {e}");
                Err(match json_value {
                    Ok(_) => ParseFailure::NotAMessage(e),
                    Err(_) => ParseFailure::NotJson(e),
                })
            }
        }
    } else {
        serde_json::from_slice(line)
            .map(Some)
            .map_err(ParseFailure::NotJson)
    }
}

/// Why one line failed the typed parse — and whether it was STRICT JSON at all (D54).
#[derive(Debug)]
enum ParseFailure {
    /// The line is not strict JSON: not UTF-8 after the one BOM strip, or rejected by
    /// `serde_json`'s `Value` parse. No id is ever recovered from it.
    NotJson(serde_json::Error),
    /// The line IS strict JSON, the typed parse failed, and the compatibility filter declined to
    /// ignore it. The carried error is the TYPED parse's, as before D54.
    NotAMessage(serde_json::Error),
}

impl ParseFailure {
    /// The typed parse's error, for the existing debug line.
    fn error(&self) -> &serde_json::Error {
        match self {
            Self::NotJson(e) | Self::NotAMessage(e) => e,
        }
    }
}

/// D54: the id the `-32700` reply carries — the one [`crate::envelope_id::scan`] recovers from a
/// STRICT-JSON line, and none from any other.
///
/// Both matches are exhaustive with no `_` arm: a new verdict or a new failure kind must be routed
/// here deliberately. The recovered arm is spelled on ONE line beginning `EnvelopeId::Recovered`,
/// which keeps `scripts/checks/d47-envelope-id-claims.sh`'s `Q12` anchor (a bare `Some(id),` line)
/// single-sited on D47's own argument.
fn parse_error_reply_id(failure: &ParseFailure, line: &[u8]) -> Option<RequestId> {
    match failure {
        ParseFailure::NotJson(_) => None,
        ParseFailure::NotAMessage(_) => match envelope_id::scan(line) {
            EnvelopeId::Recovered(id) => Some(id),
            EnvelopeId::Absent | EnvelopeId::Unusable => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{DupScanningTransport, ParamsScan};
    use crate::envelope_id_corpus::{
        Expect, ParseExpect, divergence_corpus, expected_bytes, parse_error_bytes,
        parse_error_corpus,
    };
    use rmcp::model::{GetExtensions, JsonRpcMessage, RequestId};
    use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
    use rmcp::transport::Transport;
    use rmcp::transport::async_rw::AsyncRwTransport;
    use tokio::io::AsyncWriteExt as _;

    /// Read the stamped verdict off a received message (`JsonRpcMessage` itself exposes no
    /// `extensions()`; the typemap lives on the inner request, which is what rmcp's serve loop
    /// swaps into `RequestContext`).
    fn verdict_of(message: &RxJsonRpcMessage<RoleServer>) -> Option<ParamsScan> {
        match message {
            JsonRpcMessage::Request(request) => {
                request.request.extensions().get::<ParamsScan>().cloned()
            }
            JsonRpcMessage::Notification(notification) => notification
                .notification
                .extensions()
                .get::<ParamsScan>()
                .cloned(),
            _ => None,
        }
    }

    /// The §4.6 framing corpus — ONE byte corpus, consumed by both transports.
    ///
    /// Each entry is a raw line written to the transport's read half. It deliberately includes the
    /// shapes whose handling is invisible to any test that goes through an rmcp CLIENT: a client
    /// serializes an already-deduplicated object and structurally cannot emit a duplicate key.
    fn framing_corpus() -> Vec<(&'static str, Vec<u8>)> {
        let mut corpus: Vec<(&'static str, Vec<u8>)> = Vec::new();
        // F1 — BOM-prefixed CLEAN frame.
        let mut bom_clean = b"\xEF\xBB\xBF".to_vec();
        bom_clean.extend_from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#);
        corpus.push(("F1", bom_clean));
        // F2 — CRLF-terminated clean frame (the terminator is added by the writer below).
        corpus.push((
            "F2",
            br#"{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}"#.to_vec(),
        ));
        // F3 — BOM-prefixed DUPLICATE frame.
        let mut bom_dup = b"\xEF\xBB\xBF".to_vec();
        bom_dup.extend_from_slice(
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"issue","arguments":{"action":"create","action":"delete"}}}"#,
        );
        corpus.push(("F3", bom_dup));
        // F4 — a padded duplicate whose second occurrence sits past the pad.
        let pad = "x".repeat(100 * 1024);
        corpus.push((
            "F4",
            format!(
                r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"issue","arguments":{{"pad":"{pad}","action":"create","action":"delete"}}}}}}"#
            )
            .into_bytes(),
        ));
        // F5 — a blank line (skipped, no message, no reply).
        corpus.push(("F5", Vec::new()));
        // F6 — a whitespace-only line: NOT empty, so it parses and fails => -32700 + recovery.
        corpus.push(("F6", b"   ".to_vec()));
        // F8 — an unknown notification with a WELL-FORMED `params` object. It does NOT reach the
        // compatibility filter: rmcp's catch-all `CustomNotification` types it, so the frame is
        // DELIVERED (and the filter only runs on a typed-parse failure).
        corpus.push((
            "F8",
            br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#.to_vec(),
        ));
        // F12 — the same frame BOM-prefixed: delivered identically to F8, which is what pins the
        // BOM strip as happening before the typed parse.
        let mut bom_note = b"\xEF\xBB\xBF".to_vec();
        bom_note
            .extend_from_slice(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#);
        corpus.push(("F12", bom_note));
        // -- the four entries that actually EXERCISE the forked compatibility filter --------------
        //
        // Each one FAILS the typed parse, which is the only way into the filter. Without them the
        // ~56 forked lines below `should_ignore_notification` run zero times in this suite.
        //
        // What the CORPUS pins, exactly: F17 dies if arm 2 is neutered, and F16 dies if either arm
        // over-ignores. It does NOT see an arm-1 INVERSION: that makes F15 gain a -32700 and F16
        // lose one, and the two replies are byte-identical at the same stream position, so the
        // written bytes still match rmcp's and the differential stays green. Arm 1 is pinned
        // instead by the in-module cell `the_compatibility_filter_is_entered_and_discriminates`,
        // which asserts each frame's verdict directly rather than through the reply stream.
        //
        // F14 — an unknown notification whose `params` are a SCALAR: `CustomNotification` flattens
        // `_meta` out of `params` and so requires a map. Ignored, NOT -32700.
        corpus.push((
            "F14",
            br#"{"jsonrpc":"2.0","method":"notifications/foo","params":5}"#.to_vec(),
        ));
        // F15 — LSP-style traffic (`$/cancelRequest`), same scalar `params`. Ignored ONLY by the
        // filter's first arm (no `id` + a non-standard method); its method does not start with
        // `notifications/`, so the second arm would let it through as a -32700.
        corpus.push((
            "F15",
            br#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":5}"#.to_vec(),
        ));
        // F16 — the OVER-ignoring direction: a STANDARD notification with unusable `params` must
        // still be a -32700, not a silent drop. Both arms must decline it.
        corpus.push((
            "F16",
            br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":5}"#.to_vec(),
        ));
        // F17 — ignored ONLY by the filter's SECOND arm: it carries an `id`, so the first arm
        // declines it (it is not a notification), and only the `notifications/*`-prefix arm can
        // ignore it. rmcp ignores it, so we must too. WITHOUT this entry arm 2 is dead code under
        // test — replacing its whole `matches!` with `false` leaves the suite green while the fork
        // silently answers -32700 to a frame rmcp drops.
        //
        // [v1.0.1/D47] This is ALSO the DELIBERATE PARITY DROP (D47's Decision 4): it carries an
        // `id`, so a reader reasonably expects the D47 arm to answer it. It must NOT, and no
        // carve-out exists for it — the exclusion is STRUCTURAL. `try_parse_with_compatibility`
        // returns `Ok(None)` for this frame, so it never becomes a `message` at all, and a
        // predicate keyed on a DELIVERED Notification cannot see it. It stays byte-silent because
        // rmcp is byte-silent, which is the whole point of the fork.
        corpus.push((
            "F17",
            br#"{"jsonrpc":"2.0","id":17,"method":"notifications/foo","params":5}"#.to_vec(),
        ));
        // F9 — an unknown method WITH an id: delivered (the handler answers -32601).
        corpus.push((
            "F9",
            br#"{"jsonrpc":"2.0","id":9,"method":"nope/nope","params":{}}"#.to_vec(),
        ));
        // F10 — non-UTF-8 bytes: -32700 + recovery. F10 does NOT grade the D54 gate — `scan` finds
        // no `id` in it either way; the UTF-8 witness is parse-error corpus entry X03.
        corpus.push(("F10", vec![b'{', 0xff, 0xfe, b'}']));
        // F11 — depth-130 nesting: past serde_json's 128-level limit for BOTH parsers => -32700.
        // Since D54 this is ALSO the strict-JSON gate's depth witness: `envelope_id::scan` alone
        // recovers id 11 from it (`IgnoredAny` checks no depth), so it stays id-less on our side
        // only because the gate rejects it.
        let mut deep = String::from(r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":"#);
        deep.push_str(&"[".repeat(130));
        deep.push_str(&"]".repeat(130));
        deep.push('}');
        corpus.push(("F11", deep.into_bytes()));
        // -- [v1.0.1/D47] four NEVER-ANSWERED negatives ------------------------------------------
        //
        // These are PARITY entries: each must stay byte-identical to rmcp. They exist because the
        // D47 arm's failure mode is OVER-firing, and the shipped corpus contains no frame that
        // distinguishes "the predicate is correct" from "the predicate answers anything with an
        // `id`-shaped thing near it".
        //
        // F18 — a NESTED `id`. The envelope id is a ROOT member by definition, so `params.id` is
        // invisible: delivered, nothing written. Dies if the scan recurses instead of `IgnoredAny`.
        corpus.push((
            "F18",
            br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{"id":1}}"#.to_vec(),
        ));
        // F19 — a WELL-FORMED id on a `notifications/*` method. This is already a `CustomRequest`
        // today (the untagged union tries Request first), so it is delivered as a REQUEST and the
        // D47 arm — keyed on the Notification variant — structurally cannot see it. Dies if the
        // predicate is re-keyed onto `Request`.
        corpus.push((
            "F19",
            br#"{"jsonrpc":"2.0","id":19,"method":"notifications/cancelled","params":{"requestId":1,"reason":"x"}}"#
                .to_vec(),
        ));
        // F21 — a string VALUE whose bytes SPELL an id member's key. The four-byte window `"id"`
        // genuinely occurs on the wire here (it is the value `"id"`: `"`,`i`,`d`,`"`), while no `id`
        // MEMBER exists at any depth. That is the only way a JSON document can contain those four
        // bytes without containing an `id` member, and it is what kills a prefilter that trusts its
        // POSITIVE. A value written `"\"id\":1"` would NOT work: JSON forbids a raw quote inside a
        // string, so on the wire its bytes are escaped and the window never occurs.
        corpus.push((
            "F21",
            br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{"s":"id"}}"#.to_vec(),
        ));
        // A trailing UNTERMINATED line at EOF (F7) — the writer omits the final newline for the
        // LAST entry, so this one exercises it.
        corpus.push((
            "F7",
            br#"{"jsonrpc":"2.0","id":7,"method":"ping","params":{}}"#.to_vec(),
        ));
        corpus
    }

    /// Serialize the corpus into one byte stream: `\n` after every line except the last (F7), and
    /// CRLF after the second entry (F2).
    fn corpus_bytes(corpus: &[(&'static str, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        let last = corpus.len() - 1;
        for (index, (_label, line)) in corpus.iter().enumerate() {
            out.extend_from_slice(line);
            if index == last {
                continue; // F7: unterminated final line at EOF.
            }
            if index == 1 {
                out.push(b'\r'); // F2: CRLF terminator.
            }
            out.push(b'\n');
        }
        out
    }

    /// A compact, comparable rendering of one received message.
    fn render(message: &RxJsonRpcMessage<RoleServer>) -> String {
        serde_json::to_string(message).unwrap_or_else(|e| format!("<unserializable: {e}>"))
    }

    /// Drain a transport over the corpus, then `close()` it and pin the post-close `send`.
    ///
    /// Closing here is also what gives the caller's `read_to_end` an EOF: `close()` drops the write
    /// half, which is the peer of the duplex the caller reads.
    async fn drain<T>(mut transport: T) -> Vec<String>
    where
        T: Transport<RoleServer>,
    {
        let mut received = Vec::new();
        while let Some(message) = transport.receive().await {
            received.push(render(&message));
        }
        let _ = transport.close().await;
        let post_close = transport
            .send(TxJsonRpcMessage::<RoleServer>::error(
                rmcp::model::ErrorData::parse_error("Parse error", None),
                None,
            ))
            .await;
        assert!(
            post_close.is_err(),
            "a send after close() must fail with NotConnected, never write"
        );
        received
    }

    /// **CD-7 — the differential framing pin.**
    ///
    /// Feed ONE byte corpus to rmcp's `AsyncRwTransport` and to our `DupScanningTransport` and
    /// assert identical `receive()` sequences AND identical bytes written (the `-32700` replies,
    /// with the id OMITTED, and recovery on the next line). Every `-32700` in THIS corpus is outside
    /// the D54 class; the in-class lines live in `parse_error_corpus` and are graded by the
    /// per-entry `IdInserted` tier. This differential pin, TOGETHER WITH
    /// the arm-by-arm cell below (`the_compatibility_filter_is_entered_and_discriminates`), is
    /// what stands between an rmcp bump and a silent framing divergence — neither alone suffices,
    /// which is the module doc's standing note at the top of this file.
    #[tokio::test]
    async fn cd7_framing_is_identical_to_rmcp_async_rw_transport() {
        let corpus = framing_corpus();
        let bytes = corpus_bytes(&corpus);

        // rmcp's own transport.
        let (mut rmcp_in_w, rmcp_in_r) = tokio::io::duplex(1024 * 1024);
        let (rmcp_out_w, mut rmcp_out_r) = tokio::io::duplex(1024 * 1024);
        rmcp_in_w.write_all(&bytes).await.expect("write corpus");
        rmcp_in_w.shutdown().await.expect("close corpus writer");
        let rmcp_received = drain(AsyncRwTransport::new_server(rmcp_in_r, rmcp_out_w)).await;
        let rmcp_written = read_to_end(&mut rmcp_out_r).await;

        // Ours.
        let (mut our_in_w, our_in_r) = tokio::io::duplex(1024 * 1024);
        let (our_out_w, mut our_out_r) = tokio::io::duplex(1024 * 1024);
        our_in_w.write_all(&bytes).await.expect("write corpus");
        our_in_w.shutdown().await.expect("close corpus writer");
        let our_received = drain(DupScanningTransport::new(our_in_r, our_out_w)).await;
        let our_written = read_to_end(&mut our_out_r).await;

        assert_eq!(
            our_received, rmcp_received,
            "the receive() SEQUENCE diverged from rmcp's — the framing fork is broken"
        );
        assert_eq!(
            String::from_utf8_lossy(&our_written),
            String::from_utf8_lossy(&rmcp_written),
            "the bytes WRITTEN diverged from rmcp's"
        );

        // Non-vacuity: the corpus must actually produce both deliveries and parse-error replies.
        assert!(
            rmcp_received.len() >= 4,
            "the corpus must deliver several messages, got {}",
            rmcp_received.len()
        );
        let parse_errors = String::from_utf8_lossy(&rmcp_written)
            .matches("-32700")
            .count();
        assert!(
            parse_errors >= 3,
            "the corpus must provoke several -32700 replies, got {parse_errors}"
        );
        assert!(
            !String::from_utf8_lossy(&our_written).contains("\"id\":null"),
            "the -32700 reply must OMIT the id, not send a null one"
        );
    }

    /// **The forked compatibility filter, pinned arm by arm.**
    ///
    /// The differential harness above proves our framing MATCHES rmcp's; this proves the forked
    /// filter is REACHED and DISCRIMINATES. It is a separate cell because the filter runs only
    /// after the typed parse fails, and ordinary traffic — an unknown notification with a
    /// well-formed `params` object included — never fails it. A suite without frames of this shape
    /// leaves `should_ignore_notification`, `is_standard_method` and `is_standard_notification`
    /// executing zero times, and stays green with `Ok(None)` replaced by `unreachable!()`.
    ///
    /// "Arm by arm" is meant literally, and each arm has its own killer frame: F15 is ignored ONLY
    /// by the first arm (no `id` + a non-standard method) and F17 ONLY by the second (the
    /// `notifications/*` prefix, reachable only once an `id` has taken the first arm out of play),
    /// so neutering either arm alone turns this cell RED.
    #[test]
    fn the_compatibility_filter_is_entered_and_discriminates() {
        fn parse(line: &[u8]) -> Result<Option<RxJsonRpcMessage<RoleServer>>, super::ParseFailure> {
            super::try_parse_with_compatibility::<RxJsonRpcMessage<RoleServer>>(line)
        }

        // NOT filtered: the typed parse SUCCEEDS (rmcp's catch-all `CustomNotification`), so the
        // filter is never consulted and the frame is delivered. This is the F8/F12 corpus path,
        // and the reason those two entries alone cannot cover the fork.
        assert!(
            matches!(
                parse(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#),
                Ok(Some(_))
            ),
            "an unknown notification with an object `params` is DELIVERED, not filtered"
        );

        // F14 — filtered: scalar `params` fails the typed parse; no `id` + a non-standard method.
        assert!(
            matches!(
                parse(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":5}"#),
                Ok(None)
            ),
            "an unknown notification rmcp cannot type must be IGNORED, never answered -32700"
        );

        // F15 — filtered by the FIRST arm alone: `$/cancelRequest` does not start with
        // `notifications/`, so the second arm declines it. This is the frame that dies if that arm
        // is inverted.
        assert!(
            matches!(
                parse(br#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":5}"#),
                Ok(None)
            ),
            "LSP-style client traffic must be IGNORED (rmcp does), not answered -32700"
        );

        // F16 — NOT filtered, the over-ignoring direction: a STANDARD notification with unusable
        // `params` is a real client defect and must surface as -32700, not vanish.
        assert!(
            parse(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":5}"#).is_err(),
            "a malformed STANDARD notification must not be silently swallowed"
        );

        // F17 — filtered by the SECOND arm alone, and the ONLY frame here that is. It carries an
        // `id`, so `is_notification` is false and the first arm declines it; only the
        // `notifications/*`-prefix arm can ignore it. That rmcp swallows an ID-CARRYING frame at
        // all is rmcp's behaviour, not ours — the fork must reproduce it, so this is the cell that
        // dies when arm 2 is neutered.
        assert!(
            matches!(
                parse(br#"{"jsonrpc":"2.0","id":17,"method":"notifications/foo","params":5}"#),
                Ok(None)
            ),
            "an id-carrying `notifications/*` frame rmcp cannot type must be IGNORED exactly as \
             rmcp ignores it — answering -32700 here is a framing divergence"
        );

        // A request OUTSIDE the `notifications/` prefix is never filtered: the `id` makes the first
        // arm decline it and the prefix test makes the second decline it too, so it surfaces as
        // -32700 instead of vanishing. (F17 above is the deliberate exception rmcp itself defines,
        // and only inside that prefix.)
        //
        // Since D54 this frame IS answered on its id: it is strict JSON with a readable root `id`
        // that fails the typed parse, so the `-32700` carries id 1 and a waiting rmcp client is
        // released (parse-error corpus entry E22). This assertion pins only that the frame is not
        // IGNORED.
        //
        // ONE THING THIS DOES NOT SAY, because an earlier wording claimed it and it is not true
        // (PRD section 4, D47): it is not a transport-wide invariant. A frame whose raw bytes
        // carry a top-level `id` that FAILS to decode never reaches this arm at all: rmcp's
        // untagged union falls through to the Notification variant. That class is answered
        // -32600 on the recovered id and dropped, by the arm D47 adds — not by anything here.
        assert!(
            parse(br#"{"jsonrpc":"2.0","id":1,"method":"nope/nope","params":5}"#).is_err(),
            "a request outside `notifications/*` must never be ignored — the client is waiting on \
             its id"
        );
    }

    /// Drain everything currently readable from a duplex half (the peer write end is dropped by
    /// then, so this terminates at EOF).
    async fn read_to_end<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Vec<u8> {
        use tokio::io::AsyncReadExt as _;
        let mut out = Vec::new();
        let _ = reader.read_to_end(&mut out).await;
        out
    }

    // =============================================================================================
    // [v1.0.1/D47] THE UN-DECODABLE-ENVELOPE-`id` CELLS
    //
    // Homed here and not in an integration suite for one reason: only the in-module harness owns
    // the RAW WRITTEN STREAM and both transports. Neither integration harness can assert exact
    // bytes — each parses every line to a `Value` before the caller sees it, which loses member
    // ORDER and the presence-versus-null distinction, and those are exactly what these cells pin.
    // =============================================================================================

    /// Which tier one entry of the FULL corpus belongs to.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Tier {
        /// Must stay byte-identical to `AsyncRwTransport`.
        Parity,
        /// Must DIVERGE from rmcp in exactly the declared way (D47).
        Divergence(Expect),
        /// rmcp's id-less `-32700` plus EXACTLY the declared id (D54).
        IdInserted(ParseExpect),
    }

    /// Every tier as ONE labelled list: the framing corpus (parity), D47's divergence corpus, and
    /// D54's parse-error corpus (id-inserted for an in-class entry, parity for every other).
    fn full_corpus() -> Vec<(String, Vec<u8>, Tier)> {
        let mut all: Vec<(String, Vec<u8>, Tier)> = framing_corpus()
            .into_iter()
            .map(|(label, bytes)| (label.to_string(), bytes, Tier::Parity))
            .collect();
        all.extend(divergence_corpus().into_iter().map(|entry| {
            (
                entry.id.to_string(),
                entry.frame,
                Tier::Divergence(entry.expect),
            )
        }));
        all.extend(parse_error_corpus().into_iter().map(|entry| {
            let tier = match entry.expect {
                ParseExpect::RecoveredNum(_) | ParseExpect::RecoveredStr(_) => {
                    Tier::IdInserted(entry.expect)
                }
                ParseExpect::IdLess | ParseExpect::Silent => Tier::Parity,
            };
            (entry.id.to_string(), entry.frame, tier)
        }));
        all
    }

    /// Splice `"id":<declared>,` into rmcp's reply right after its `{"jsonrpc":"2.0",` head — the
    /// slot rmcp's own field order gives `id` (`JsonRpcError`: `jsonrpc`, `id`, `error`; rmcp
    /// `src/model.rs:462-470`). The id comes from the corpus row, NEVER from `envelope_id::scan`.
    fn insert_id(rmcp_reply: &[u8], expect: &ParseExpect) -> Vec<u8> {
        const HEAD: &[u8] = br#"{"jsonrpc":"2.0","#;
        let tail = rmcp_reply.strip_prefix(HEAD).expect(
            "rmcp's reply no longer opens with its jsonrpc member — re-derive the D54 id slot",
        );
        let id = match expect {
            ParseExpect::RecoveredNum(n) => n.to_string(),
            ParseExpect::RecoveredStr(s) => format!("\"{s}\""),
            ParseExpect::IdLess | ParseExpect::Silent => {
                panic!("only an in-class entry carries an id")
            }
        };
        [HEAD, b"\"id\":", id.as_bytes(), b",", tail].concat()
    }

    /// Run ONE frame through our transport; return `(received, written)`.
    async fn run_ours(frame: &[u8]) -> (Vec<String>, Vec<u8>) {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        let mut bytes = frame.to_vec();
        bytes.push(b'\n');
        in_w.write_all(&bytes).await.expect("write frame");
        in_w.shutdown().await.expect("close writer");
        let received = drain(DupScanningTransport::new(in_r, out_w)).await;
        let written = read_to_end(&mut out_r).await;
        (received, written)
    }

    /// Run ONE frame through rmcp's own transport; return `(received, written)`.
    async fn run_rmcp(frame: &[u8]) -> (Vec<String>, Vec<u8>) {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        let mut bytes = frame.to_vec();
        bytes.push(b'\n');
        in_w.write_all(&bytes).await.expect("write frame");
        in_w.shutdown().await.expect("close writer");
        let received = drain(AsyncRwTransport::new_server(in_r, out_w)).await;
        let written = read_to_end(&mut out_r).await;
        (received, written)
    }

    /// **W-D01..W-D23** — every divergence entry is answered BYTE FOR BYTE, and never delivered.
    ///
    /// The assertion is EXACT bytes, not "differs from rmcp": a mutant writing garbage satisfies
    /// "differs" and cannot satisfy this. The per-entry id choice is the only thing that actually
    /// pins the unambiguity rule.
    ///
    /// Mutants: the whole catalogue's byte-level half — `invalid_request` swapped for
    /// `parse_error`; `Some(id)` replaced by `None` on the recovered arm; `None` replaced by any
    /// id on the fallback arm; any edit to the message literal. **W-D23** additionally carries the
    /// raw-span VALUE comparator, and **W-D22** the D43-suppresses-D47 precedence mutant.
    #[tokio::test]
    async fn the_divergence_corpus_is_answered_byte_for_byte() {
        for entry in divergence_corpus() {
            let (received, written) = run_ours(&entry.frame).await;
            assert_eq!(
                String::from_utf8_lossy(&written),
                String::from_utf8_lossy(&expected_bytes(&entry.expect)),
                "{} wrote the wrong bytes — {}",
                entry.id,
                entry.why
            );
            assert!(
                received.is_empty(),
                "{} must be ANSWERED AND DROPPED, never delivered — got {received:?}",
                entry.id
            );
        }
    }

    /// **W-DROP** — an answered frame is never delivered, and exactly one reply is written per frame.
    ///
    /// Mutant: `continue` replaced by `return Some(message)` (answer AND deliver). Since D50 a frame
    /// delivered BEFORE the handshake meets the gate above this transport and is dropped there a
    /// second time. What this cell catches is the delivery ITSELF taking effect once the latch is
    /// open. rmcp runs every delivered notification through `TryInto<CancelledNotification>`, and a
    /// frame that converts cancels the in-flight request it names
    /// (`rmcp-1.7.0/src/service.rs:981-995`). The count below catches the other half, since exactly
    /// one reply per frame is written.
    #[tokio::test]
    async fn an_answered_frame_is_never_delivered() {
        let corpus = divergence_corpus();
        let pick = |id: &str| {
            corpus
                .iter()
                .find(|f| f.id == id)
                .unwrap_or_else(|| panic!("{id} missing"))
        };

        let mut bytes = Vec::new();
        for id in ["D01", "D04", "D06"] {
            bytes.extend_from_slice(&pick(id).frame);
            bytes.push(b'\n');
        }
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        in_w.write_all(&bytes).await.expect("write frames");
        in_w.shutdown().await.expect("close writer");
        let received = drain(DupScanningTransport::new(in_r, out_w)).await;
        let written = read_to_end(&mut out_r).await;

        assert!(received.is_empty(), "no class frame may be delivered");
        let lines = written
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .count();
        assert_eq!(
            lines, 3,
            "exactly ONE reply per frame, no more and no fewer"
        );
    }

    /// **W-RECOVER** — the connection SURVIVES an answer and the next frame is delivered normally.
    ///
    /// Mutant: `continue` replaced by `return None` (close the connection after answering).
    #[tokio::test]
    async fn the_connection_survives_and_the_next_frame_is_delivered() {
        let corpus = divergence_corpus();
        let ambiguous = corpus.iter().find(|f| f.id == "D04").expect("D04 missing");

        let mut bytes = ambiguous.frame.clone();
        bytes.push(b'\n');
        bytes.extend_from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#);
        bytes.push(b'\n');

        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        in_w.write_all(&bytes).await.expect("write frames");
        in_w.shutdown().await.expect("close writer");
        let received = drain(DupScanningTransport::new(in_r, out_w)).await;
        let written = read_to_end(&mut out_r).await;

        assert_eq!(received.len(), 1, "the FOLLOWING frame must still arrive");
        assert!(
            received[0].contains(r#""method":"ping""#),
            "the delivered frame must be the ping: {}",
            received[0]
        );
        assert_eq!(
            String::from_utf8_lossy(&written),
            String::from_utf8_lossy(&expected_bytes(&Expect::Omitted)),
            "exactly ONE reply — the fallback — and nothing for the clean ping"
        );
    }

    /// **W-N1** — a notification with NO `id` is DELIVERED and nothing is written.
    ///
    /// This is D47's explicit carve-out and a JSON-RPC requirement ("The Server MUST NOT reply to a
    /// Notification"), so it is a false-POSITIVE guard rather than a coverage cell.
    ///
    /// Mutant: deleting the `Absent => {}` arm, so a genuine notification falls into a reply arm.
    #[tokio::test]
    async fn a_notification_with_no_id_is_delivered_and_nothing_is_written() {
        let (received, written) =
            run_ours(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#).await;
        assert_eq!(
            received.len(),
            1,
            "a genuine notification must be delivered"
        );
        assert!(written.is_empty(), "and must NOT be answered");
    }

    /// **W-N2** — a NESTED `id` does not make a notification answerable.
    ///
    /// Mutant: recursing into nested objects instead of consuming them with `IgnoredAny`.
    #[tokio::test]
    async fn a_nested_id_does_not_make_a_notification_answerable() {
        let (received, written) =
            run_ours(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{"id":1}}"#).await;
        assert_eq!(received.len(), 1);
        assert!(written.is_empty(), "`params.id` is not an ENVELOPE id");
    }

    /// **W-N2b** — a string VALUE that spells `"id"` is not an `id` MEMBER.
    ///
    /// The four-byte window `"id"` genuinely occurs in this frame's wire bytes while no `id` member
    /// exists at any depth.
    ///
    /// Mutant: a window prefilter that trusts its POSITIVE (answer whenever the window occurs).
    /// **It does NOT kill the prefilter that trusts its NEGATIVE**, and the difference matters: that
    /// mutant is unsound only in the direction of MISSING the escaped key, which W-D14/W-D15 own.
    #[tokio::test]
    async fn an_id_shaped_string_value_is_not_an_id_member() {
        let frame = br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{"s":"id"}}"#;
        assert!(
            frame.windows(4).any(|w| w == br#""id""#),
            "non-vacuity: the four-byte window must really occur on the wire, or this cell grades \
             nothing at all"
        );
        let (received, written) = run_ours(frame).await;
        assert_eq!(received.len(), 1);
        assert!(written.is_empty());
    }

    /// **W-N3** — the Decision-4 parity drop stays silent.
    ///
    /// F17 carries an `id`, so a reader expects the D47 arm to answer it. The exclusion is
    /// STRUCTURAL rather than a carve-out: `try_parse_with_compatibility` returns `Ok(None)` for
    /// this frame, so it never becomes a `message` and the predicate — keyed on a DELIVERED
    /// Notification — cannot see it. rmcp drops it too, so the re-scoped acceptance criterion
    /// ("no frame that rmcp itself would answer may go unanswered") is satisfied.
    ///
    /// Mutants: neutering the compat filter's second arm; moving the D47 branch ABOVE the parse so
    /// it keys on raw bytes alone.
    #[tokio::test]
    async fn the_decision_4_parity_drop_stays_silent() {
        let (received, written) =
            run_ours(br#"{"jsonrpc":"2.0","id":17,"method":"notifications/foo","params":5}"#).await;
        assert!(received.is_empty(), "F17 is never delivered");
        assert!(
            written.is_empty(),
            "and never answered — rmcp answers nothing either"
        );
    }

    /// **W-N4** — a clean request is untouched: delivered, `Clean`, nothing written.
    ///
    /// Mutants: re-keying the predicate onto the `Request` variant; moving the branch above the
    /// parse.
    #[tokio::test]
    async fn a_clean_request_is_untouched() {
        let (received, written) =
            run_ours(br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#).await;
        assert_eq!(received.len(), 1);
        assert!(written.is_empty(), "request traffic must pay nothing");
    }

    /// **W-N5** — a D43 frame is not STOLEN by D47.
    ///
    /// The duplicate is inside `params.arguments` and the ROOT `id` is perfectly fine, so this
    /// stays a D43 frame end to end: delivered, carrying its `Duplicate` verdict, unanswered.
    ///
    /// Mutant: re-keying the predicate onto `Request`, which would answer it and never deliver it —
    /// silently disabling the whole D43 gate.
    #[tokio::test]
    async fn d43_frames_are_not_stolen_by_d47() {
        let (received, written) = run_ours(
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"issue","arguments":{"id":"a","id":"b"}}}"#,
        )
        .await;
        assert_eq!(received.len(), 1, "a D43 frame must still be DELIVERED");
        assert!(
            written.is_empty(),
            "and must NOT be answered by the D47 arm"
        );

        // And it must still carry its D43 verdict, which is what `call_tool` gates on.
        let (mut in_w, in_r) = tokio::io::duplex(64 * 1024);
        let (out_w, _out_r) = tokio::io::duplex(64 * 1024);
        in_w.write_all(
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"issue","arguments":{"id":"a","id":"b"}}}
"#,
        )
        .await
        .expect("write frame");
        in_w.shutdown().await.expect("close writer");
        let mut transport = DupScanningTransport::new(in_r, out_w);
        let message = transport.receive().await.expect("delivered");
        assert_eq!(
            verdict_of(&message),
            Some(ParamsScan::Duplicate {
                key: "id".to_string(),
                path: "/arguments".to_string()
            }),
            "the D43 verdict must survive untouched"
        );
    }

    /// **W-G2** — the per-entry differential over the FULL corpus, every tier (parity, D47
    /// divergence, D54 id-inserted).
    ///
    /// The whole-stream cell above keeps its own job (F2's CRLF, F5's blank line and F7's
    /// unterminated final line are STREAM properties a per-entry harness destroys, which is why
    /// that cell must not be deleted as redundant). This one grades each entry independently.
    #[tokio::test]
    async fn the_per_entry_differential_holds_for_every_tier() {
        for (label, frame, tier) in full_corpus() {
            let (our_received, our_written) = run_ours(&frame).await;
            let (rmcp_received, rmcp_written) = run_rmcp(&frame).await;
            match tier {
                Tier::Parity => {
                    assert_eq!(
                        our_received, rmcp_received,
                        "{label}: the receive() sequence diverged from rmcp's"
                    );
                    assert_eq!(
                        String::from_utf8_lossy(&our_written),
                        String::from_utf8_lossy(&rmcp_written),
                        "{label}: the bytes written diverged from rmcp's"
                    );
                }
                Tier::Divergence(expect) => {
                    assert!(
                        rmcp_written.is_empty(),
                        "{label}: rmcp must answer NOTHING — that IS the defect"
                    );
                    assert_eq!(
                        rmcp_received.len(),
                        1,
                        "{label}: rmcp must DELIVER it as a notification — that IS the defect"
                    );
                    assert_eq!(
                        String::from_utf8_lossy(&our_written),
                        String::from_utf8_lossy(&expected_bytes(&expect)),
                        "{label}: our reply bytes are wrong"
                    );
                    assert!(our_received.is_empty(), "{label}: we must answer AND DROP");
                }
                Tier::IdInserted(expect) => {
                    // (a) rmcp's OWN reply is still the id-less -32700 — a hand-written literal.
                    assert_eq!(
                        String::from_utf8_lossy(&rmcp_written),
                        String::from_utf8_lossy(&parse_error_bytes(&ParseExpect::IdLess)),
                        "{label}: rmcp's own reply must still be the id-less -32700 — if rmcp now \
                         writes the id itself, or changed the code, message or framing, re-derive D54"
                    );
                    // (b) ours is rmcp's ACTUAL bytes with EXACTLY the declared id spliced in.
                    assert_eq!(
                        String::from_utf8_lossy(&our_written),
                        String::from_utf8_lossy(&insert_id(&rmcp_written, &expect)),
                        "{label}: ours must be rmcp's bytes with EXACTLY the declared id inserted"
                    );
                    // (c) neither transport delivers it.
                    assert!(
                        our_received.is_empty() && rmcp_received.is_empty(),
                        "{label}: a line that fails the typed parse is never delivered"
                    );
                }
            }
        }
    }

    /// **W-G3** — the SET-EQUALITY / anti-drift guard. This is what replaces what a single
    /// "identical bytes" assertion used to buy.
    ///
    /// Without it, an OVER-firing predicate that also answered the id-less F8/F12 would leave both
    /// other tiers green: the parity tier would still match rmcp on the entries it did not touch,
    /// and the divergence tier would still match its expected bytes.
    ///
    /// Mutant: re-keying the predicate onto `Request` (F1–F4/F9/F19 gain replies), or any rmcp bump
    /// that migrates an entry between tiers. The declared set is the D47 divergence tier UNION the
    /// D54 id-inserted tier.
    #[tokio::test]
    async fn the_diverging_entries_are_exactly_the_declared_ones() {
        use std::collections::BTreeSet;

        let mut observed: BTreeSet<String> = BTreeSet::new();
        let mut declared: BTreeSet<String> = BTreeSet::new();
        let mut all_our_written = String::new();

        for (label, frame, tier) in full_corpus() {
            if matches!(tier, Tier::Divergence(_) | Tier::IdInserted(_)) {
                declared.insert(label.clone());
            }
            let (our_received, our_written) = run_ours(&frame).await;
            let (rmcp_received, rmcp_written) = run_rmcp(&frame).await;
            all_our_written.push_str(&String::from_utf8_lossy(&our_written));
            if our_written != rmcp_written || our_received != rmcp_received {
                observed.insert(label);
            }
        }

        assert_eq!(
            observed, declared,
            "the stream positions where we diverge from rmcp must be EXACTLY the declared \
             divergence tier — an entry in `observed` only is an over-firing predicate, an entry \
             in `declared` only is a fix that stopped working"
        );
        assert!(
            !declared.is_empty(),
            "a corpus with no declared divergence would make this guard vacuous"
        );

        // The RATIFIED fallback spelling, pinned over the whole DIVERGENCE stream. This is a
        // SECOND, WIDER copy of the guard the shipped CD-7 cell carries — not a migration of it.
        // That one pins the parity tier's omission and stays where it is; this one covers the
        // arms D47 and D54 add, which the shipped guard never sees.
        assert!(
            !all_our_written.contains("\"id\":null"),
            "D47's fallback spells the missing id by OMISSION: rmcp's JsonRpcError.id is \
             Option<RequestId> with skip_serializing_if = \"Option::is_none\" (model.rs:462-470), \
             so a literal null is not reachable through the codec. If the decision is ever revised \
             to a literal null (a deliberate codec bypass), THIS is the assertion to change."
        );
    }

    /// **W-G4** — divergence-kind coverage as a SET, never a count.
    ///
    /// A count would rot; a set cannot be off-by-one against itself. Its "site" is the CORPUS, not
    /// a production site: the mutation it grades is a corpus EDIT that silently drops one half of
    /// the recovery rule (e.g. deleting every string-id entry).
    #[test]
    fn every_divergence_kind_is_represented() {
        use crate::envelope_id_corpus::ExpectKind;
        use std::collections::BTreeSet;

        let present: BTreeSet<ExpectKind> = divergence_corpus()
            .iter()
            .map(|f| f.expect.kind())
            .collect();
        let required: BTreeSet<ExpectKind> = [
            ExpectKind::RecoveredNum,
            ExpectKind::RecoveredStr,
            ExpectKind::Omitted,
        ]
        .into_iter()
        .collect();
        assert_eq!(
            present, required,
            "the corpus must exercise all three reply shapes"
        );
    }

    /// **W-G5** — corpus non-vacuity: every entry really carries what its `why` claims.
    ///
    /// The failure this guards is the one this repo has paid for: someone "tidies" a frame into a
    /// well-formed one and the cell keeps passing while grading nothing.
    #[test]
    fn the_divergence_corpus_is_not_vacuous() {
        for entry in divergence_corpus() {
            let text = String::from_utf8_lossy(&entry.frame);

            // Every entry must be a frame our predicate can even see: the raw bytes must carry a
            // root `id` member in SOME spelling.
            assert!(
                text.contains(r#""id":"#) || text.contains(r#"d":"#),
                "{}: no `id` member in the frame text at all",
                entry.id
            );

            // The wrong-TYPE entries must really parse to a value rmcp's RequestId rejects.
            if entry.id.starts_with('D') && matches!(entry.expect, Expect::Omitted) {
                let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&entry.frame);
                if let Ok(serde_json::Value::Object(map)) = parsed
                    && let Some(id) = map.get("id")
                {
                    use serde::Deserialize as _;
                    assert!(
                        RequestId::deserialize(id.clone()).is_err()
                            || entry.id == "D04"
                            || entry.id == "D05",
                        "{}: claims to be unusable, but its (last-wins) id decodes fine",
                        entry.id
                    );
                }
            }

            // D23 SPECIFICALLY: the two occurrences must NOT be byte-identical while their decoded
            // values ARE equal. Without this guard, "tidying" the escape away degenerates the one
            // cell that kills a raw-span VALUE comparator into a duplicate of D02.
            if entry.id == "D23" {
                assert!(
                    !text.contains(r#""id":"a","id":"a""#),
                    "D23's occurrences must differ BYTEWISE, or it stops grading anything"
                );
                assert_eq!(
                    text.matches(r#""id":"#).count(),
                    2,
                    "D23 must carry exactly two plain `id` keys"
                );
            }
        }
    }

    /// The verdict is stamped PER FRAME, in order, on one connection — a transport that reused or
    /// shared a verdict across frames would pass every single-frame cell and fail here.
    #[tokio::test]
    async fn the_verdict_is_stamped_per_frame() {
        let duplicate = br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"issue","arguments":{"action":"create","action":"delete"}}}"#;
        let clean = br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"issue","arguments":{"action":"list"}}}"#;
        let nested = br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"issue","arguments":{"action":"show"},"_meta":{"trace":{"span":"x","span":"y"}}}}"#;

        let mut bytes = Vec::new();
        for line in [&duplicate[..], &clean[..], &nested[..]] {
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
        }

        let (mut in_w, in_r) = tokio::io::duplex(64 * 1024);
        let (out_w, _out_r) = tokio::io::duplex(64 * 1024);
        in_w.write_all(&bytes).await.expect("write frames");
        in_w.shutdown().await.expect("close writer");
        let mut transport = DupScanningTransport::new(in_r, out_w);

        let mut verdicts = Vec::new();
        while let Some(message) = transport.receive().await {
            verdicts.push(verdict_of(&message));
        }

        assert_eq!(
            verdicts,
            vec![
                Some(ParamsScan::Duplicate {
                    key: "action".to_string(),
                    path: "/arguments".to_string()
                }),
                Some(ParamsScan::Clean),
                Some(ParamsScan::Duplicate {
                    key: "span".to_string(),
                    path: "/_meta/trace".to_string()
                }),
            ],
            "the verdict must be recomputed per frame, never reused across frames"
        );
    }

    /// The scan runs on EVERY decoded request, not only `tools/call` — the enforcement site is
    /// singular, the scan is not.
    #[tokio::test]
    async fn every_decoded_request_carries_a_verdict() {
        let bytes = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
"#;
        let (mut in_w, in_r) = tokio::io::duplex(64 * 1024);
        let (out_w, _out_r) = tokio::io::duplex(64 * 1024);
        in_w.write_all(bytes).await.expect("write frames");
        in_w.shutdown().await.expect("close writer");
        let mut transport = DupScanningTransport::new(in_r, out_w);

        let mut count = 0usize;
        while let Some(message) = transport.receive().await {
            assert_eq!(
                verdict_of(&message),
                Some(ParamsScan::Clean),
                "every decoded request must carry a verdict"
            );
            count += 1;
        }
        assert_eq!(count, 2, "both frames must be delivered");
    }
    use std::sync::Arc;

    // =============================================================================================
    // [ub-nbz / ub-zja] CANCELLATION CELLS — `receive()` dropped mid-poll, exactly as rmcp's unbiased
    // serve-loop `select!` drops a losing arm (`rmcp-1.7.0/src/service.rs:805`, the arm at `:813`).
    //
    // Every cell here runs on the CURRENT-THREAD runtime `#[tokio::test]` builds by default, and that
    // is load-bearing: a task spawned inside `receive()` cannot be polled until the test itself
    // yields, so "polled once, then dropped" is ONE deterministic interleaving and never a race.
    // =============================================================================================

    /// Poll `receive()` EXACTLY ONCE, assert it is still pending, and drop it — the losing arm of
    /// rmcp's serve-loop `select!`. `biased` makes the poll order fixed: `receive()` first, then the
    /// always-ready arm that wins.
    async fn poll_once_and_drop<R, W>(transport: &mut DupScanningTransport<R, W>)
    where
        R: tokio::io::AsyncRead + Send + Unpin,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        tokio::select! {
            biased;
            delivered = transport.receive() => panic!(
                "the first poll must be PENDING for this cell to model a cancellation; got {delivered:?}"
            ),
            () = std::future::ready(()) => {}
        }
    }

    const PING_7: &[u8] = br#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#;

    /// **ub-nbz cell (i)** — an out-of-band reply SURVIVES the drop of the `receive()` that produced
    /// it, on ALL FOUR arms: `-32600` on a recovered id, `-32600` with the id omitted, `-32700`, and
    /// (D54) `-32700` on a recovered id.
    ///
    /// The write lock is held for the first poll, standing in for a handler response mid-write — the
    /// production precondition. The frame after the bad one is delivered, proving the connection
    /// survives, and the reply bytes are EXACTLY one reply.
    ///
    /// Mutants: the reply awaited inline again (the pre-fix shape) · the reply future built but never
    /// spawned or parked · the parked handle discarded by a `receive()` drop.
    #[tokio::test]
    async fn an_out_of_band_reply_survives_a_dropped_receive() {
        let corpus = divergence_corpus();
        let recovered = corpus.iter().find(|f| f.id == "D01").expect("D01 missing");
        let omitted = corpus.iter().find(|f| f.id == "D04").expect("D04 missing");
        assert!(
            matches!(recovered.expect, Expect::RecoveredNum(_)),
            "D01 must recover an id"
        );
        assert!(
            matches!(omitted.expect, Expect::Omitted),
            "D04 must omit the id"
        );

        let parse_corpus = parse_error_corpus();
        let in_class = parse_corpus
            .iter()
            .find(|f| f.id == "E01")
            .expect("E01 missing");
        assert!(
            matches!(in_class.expect, ParseExpect::RecoveredNum(_)),
            "E01 must recover an id"
        );

        let arms: [(&str, Vec<u8>, Vec<u8>); 4] = [
            (
                "-32600 recovered id",
                recovered.frame.clone(),
                expected_bytes(&recovered.expect),
            ),
            (
                "-32600 id omitted",
                omitted.frame.clone(),
                expected_bytes(&omitted.expect),
            ),
            (
                "-32700",
                b"this is not json".to_vec(),
                parse_error_bytes(&ParseExpect::IdLess),
            ),
            (
                "-32700 recovered id",
                in_class.frame.clone(),
                parse_error_bytes(&in_class.expect),
            ),
        ];
        let mut lost = Vec::new();
        for (arm, frame, expected) in arms {
            let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
            let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
            let mut bytes = frame;
            bytes.push(b'\n');
            bytes.extend_from_slice(PING_7);
            bytes.push(b'\n');
            in_w.write_all(&bytes).await.expect("write frames");
            in_w.shutdown().await.expect("close writer");

            let mut transport = DupScanningTransport::new(in_r, out_w);
            let held = Arc::clone(&transport.write).lock_owned().await;
            poll_once_and_drop(&mut transport).await;
            drop(held);

            let next = transport.receive().await.expect("the connection survives");
            assert!(
                render(&next).contains(r#""id":7"#),
                "{arm}: the following frame is delivered"
            );
            assert!(transport.receive().await.is_none(), "{arm}: then EOF");
            let _ = transport.close().await;
            drop(transport);
            let written = read_to_end(&mut out_r).await;
            if written != expected {
                lost.push(format!(
                    "{arm}: wrote {:?}",
                    String::from_utf8_lossy(&written)
                ));
            }
        }
        assert!(
            lost.is_empty(),
            "every arm's reply must be written exactly once although its receive() was dropped: {lost:#?}"
        );
    }

    /// A reply that CANNOT be written still ends the SAME `receive()` call with `None` — the
    /// contract the inline write shipped and D40's teardown relies on. The following ping is
    /// buffered and readable, so a `receive()` that ignored the failure would deliver it.
    ///
    /// Mutants: the parked write's `Err` mapped to success · the loop-head settle removed.
    #[tokio::test]
    async fn a_reply_that_cannot_be_written_ends_the_same_receive() {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, out_r) = tokio::io::duplex(1024 * 1024);
        drop(out_r); // every write to `out_w` now fails with BrokenPipe
        let mut bytes = b"this is not json\n".to_vec();
        bytes.extend_from_slice(PING_7);
        bytes.push(b'\n');
        in_w.write_all(&bytes).await.expect("write frames");
        let mut transport = DupScanningTransport::new(in_r, out_w);
        assert!(
            transport.receive().await.is_none(),
            "a failed reply write must end the read, never deliver the next frame"
        );
    }

    /// A reply that fails AFTER the `receive()` that parked it was dropped still ends the NEXT
    /// `receive()` with `None`, before it reads the frame behind it. This is what makes the settle
    /// await the handle IN PLACE: a settle that took the handle out of the slot before awaiting it
    /// loses the handle, and with it the failure, when that await is dropped.
    ///
    /// Mutants: the handle taken out of the slot before it is awaited · the failure ignored.
    #[tokio::test]
    async fn a_reply_failing_after_a_dropped_receive_ends_the_next_receive() {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, out_r) = tokio::io::duplex(1024 * 1024);
        drop(out_r);
        let mut bytes = b"this is not json\n".to_vec();
        bytes.extend_from_slice(PING_7);
        bytes.push(b'\n');
        in_w.write_all(&bytes).await.expect("write frames");
        let mut transport = DupScanningTransport::new(in_r, out_w);
        let held = Arc::clone(&transport.write).lock_owned().await;
        poll_once_and_drop(&mut transport).await;
        drop(held);
        assert!(
            transport.receive().await.is_none(),
            "the parked reply's failure must end the read, never deliver the next frame"
        );
    }

    /// A writer that PANICS inside the write — so the spawned reply task ends in a `JoinError`.
    struct PanickingWriter;
    impl tokio::io::AsyncWrite for PanickingWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            panic!("injected writer panic (expected by this cell)")
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// A reply task that PANICS ends the read with `None`, like any other failed reply.
    ///
    /// Mutant: a `JoinError` mapped to success.
    #[tokio::test]
    async fn a_panicked_reply_task_ends_the_receive() {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let mut bytes = b"this is not json\n".to_vec();
        bytes.extend_from_slice(PING_7);
        bytes.push(b'\n');
        in_w.write_all(&bytes).await.expect("write frames");
        let mut transport = DupScanningTransport::new(in_r, PanickingWriter);
        let outcome = tokio::spawn(async move { transport.receive().await.is_none() }).await;
        assert!(
            matches!(outcome, Ok(true)),
            "a panicked reply write must end the read with None, not deliver the next frame and not \
             unwind through receive(): {outcome:?}"
        );
    }

    /// Two consecutive bad frames are answered in ARRIVAL order, each exactly once, and the reply to
    /// the first is on the wire before the second is read — with the write lock held for the first
    /// poll, so both replies go through the parked path.
    ///
    /// Mutants: the parked slot overwritten without settling (the loop-head settle removed) · a
    /// second slot / queue that lets the replies race.
    #[tokio::test]
    async fn consecutive_bad_frames_are_answered_in_arrival_order() {
        let corpus = divergence_corpus();
        let recovered = corpus.iter().find(|f| f.id == "D01").expect("D01 missing");
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        let mut bytes = recovered.frame.clone();
        bytes.push(b'\n');
        bytes.extend_from_slice(b"this is not json\n");
        in_w.write_all(&bytes).await.expect("write frames");
        in_w.shutdown().await.expect("close writer");

        let mut transport = DupScanningTransport::new(in_r, out_w);
        let held = Arc::clone(&transport.write).lock_owned().await;
        poll_once_and_drop(&mut transport).await;
        drop(held);
        assert!(
            transport.receive().await.is_none(),
            "nothing is delivered, then EOF"
        );
        let _ = transport.close().await;
        drop(transport);

        let mut expected = expected_bytes(&recovered.expect);
        expected.extend_from_slice(&parse_error_bytes(&ParseExpect::IdLess));
        assert_eq!(
            String::from_utf8_lossy(&read_to_end(&mut out_r).await),
            String::from_utf8_lossy(&expected),
            "the -32600 then the -32700, each once, in arrival order"
        );
    }

    /// `close()` takes the write half EVEN WHEN the parked reply it settles has failed: a `send`
    /// after `close()` is `NotConnected`, never a write attempt on the dead pipe.
    ///
    /// Mutant: `close()` returns early when the settled reply failed, leaving the write half in place.
    #[tokio::test]
    async fn a_send_after_close_is_not_connected_even_when_the_parked_reply_failed() {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, out_r) = tokio::io::duplex(1024 * 1024);
        drop(out_r); // the reply's write fails with BrokenPipe
        in_w.write_all(b"this is not json\n")
            .await
            .expect("write frame");

        let mut transport = DupScanningTransport::new(in_r, out_w);
        // Hold the write lock so the parked reply cannot run before `close()` settles it.
        let held = transport.write.clone().lock_owned().await;
        poll_once_and_drop(&mut transport).await;
        drop(held);
        let _ = transport.close().await;
        let post_close = transport
            .send(TxJsonRpcMessage::<RoleServer>::error(
                rmcp::model::ErrorData::parse_error("Parse error", None),
                None,
            ))
            .await;
        assert_eq!(
            post_close
                .expect_err("a send after close() must fail")
                .kind(),
            std::io::ErrorKind::NotConnected,
            "close() must take the write half even when the parked reply failed"
        );
        drop(in_w);
    }

    /// `close()` WAITS for a parked reply before it takes the write half. The reply task is spawned
    /// but — on this current-thread runtime — not yet polled when `close()` runs, so a `close()` that
    /// took the writer first would turn the reply into a `NotConnected` and write nothing.
    ///
    /// Mutant: `close()` does not settle the parked handle.
    #[tokio::test]
    async fn close_waits_for_a_parked_reply() {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        // The writer stays OPEN: after the reply the read pends, so the poll is pending either way.
        in_w.write_all(b"this is not json\n")
            .await
            .expect("write frame");

        let mut transport = DupScanningTransport::new(in_r, out_w);
        poll_once_and_drop(&mut transport).await;
        let _ = transport.close().await;
        drop(transport);
        assert_eq!(
            String::from_utf8_lossy(&read_to_end(&mut out_r).await),
            String::from_utf8_lossy(&parse_error_bytes(&ParseExpect::IdLess)),
            "the reply parked before close() must still be written, exactly once"
        );
        drop(in_w);
    }

    /// **ub-zja** — a WELL-FORMED frame whose bytes are split across two reads, with the `receive()`
    /// that consumed the first half DROPPED, is still delivered WHOLE, and nothing is written.
    ///
    /// Mutants: `line_buf` cleared at the loop head unconditionally again (the pre-fix shape) · the
    /// completion flag never reset after a clear.
    #[tokio::test]
    async fn a_line_split_by_a_dropped_receive_is_delivered_whole() {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        // A COMPLETE line first, so the split line is not the transport's first: a clear keyed on
        // "some line was ever completed" rather than on THIS line must fail here too.
        in_w.write_all(br#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#)
            .await
            .expect("write");
        in_w.write_all(b"\n").await.expect("terminate");
        let (head, tail) = PING_7.split_at(20);
        in_w.write_all(head).await.expect("write the head");

        let mut transport = DupScanningTransport::new(in_r, out_w);
        let first = transport.receive().await;
        assert!(
            first
                .as_ref()
                .is_some_and(|m| render(m).contains(r#""id":6"#)),
            "the complete line is delivered first: {first:?}"
        );
        poll_once_and_drop(&mut transport).await; // `read_until` consumed the head, then pended

        in_w.write_all(tail).await.expect("write the tail");
        in_w.write_all(b"\n").await.expect("terminate");
        in_w.shutdown().await.expect("close writer");
        let delivered = transport.receive().await;
        assert!(
            delivered
                .as_ref()
                .is_some_and(|m| render(m).contains(r#""id":7"#)),
            "the split frame must be delivered whole: {delivered:?}"
        );
        assert!(transport.receive().await.is_none(), "then EOF");
        let _ = transport.close().await;
        drop(transport);
        assert!(
            read_to_end(&mut out_r).await.is_empty(),
            "nothing is written — in particular no -32700 for a tail parsed alone"
        );
    }

    /// **ub-zja** — an UNTERMINATED final line whose bytes were consumed by a DROPPED `receive()`
    /// is still processed at EOF: `read_until` then returns `Ok(0)` with a NON-EMPTY buffer.
    ///
    /// Mutant: `Ok(0)` returns `None` regardless of the buffer.
    #[tokio::test]
    async fn an_unterminated_line_split_by_a_dropped_receive_is_delivered_at_eof() {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, _out_r) = tokio::io::duplex(1024 * 1024);
        in_w.write_all(PING_7)
            .await
            .expect("write the unterminated frame");

        let mut transport = DupScanningTransport::new(in_r, out_w);
        poll_once_and_drop(&mut transport).await; // consumed the whole frame, no `\n` yet

        in_w.shutdown().await.expect("EOF with no newline");
        let delivered = transport.receive().await;
        assert!(
            delivered
                .as_ref()
                .is_some_and(|m| render(m).contains(r#""id":7"#)),
            "an unterminated final line is still a frame: {delivered:?}"
        );
        assert!(
            transport.receive().await.is_none(),
            "then EOF, exactly once"
        );
    }

    /// A reader that serves its chunks in order, one per `poll_read`: `Ok` bytes or an `Err`, then EOF.
    struct Scripted(std::collections::VecDeque<std::io::Result<Vec<u8>>>);

    impl tokio::io::AsyncRead for Scripted {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(match self.0.pop_front() {
                Some(Ok(bytes)) => {
                    buf.put_slice(&bytes);
                    Ok(())
                }
                Some(Err(e)) => Err(e),
                None => Ok(()),
            })
        }
    }

    /// Run `head`, a read error, then `tail` through one transport: the first `receive()` ends
    /// with the error, and what matters is what the RE-CALL yields and what gets written.
    async fn recall_after_read_error<T>(
        make: impl FnOnce(Scripted, tokio::io::DuplexStream) -> T,
    ) -> (Option<String>, Vec<u8>)
    where
        T: Transport<RoleServer>,
    {
        let (head, tail) = PING_1.split_at(10);
        let mut tail = tail.to_vec();
        tail.push(b'\n');
        let reader = Scripted(
            [
                Ok(head.to_vec()),
                Err(std::io::Error::other("injected read error")),
                Ok(tail),
            ]
            .into(),
        );
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        let mut transport = make(reader, out_w);
        assert!(
            transport.receive().await.is_none(),
            "the read error ends the first call"
        );
        let recalled = transport.receive().await.as_ref().map(render);
        let _ = transport.close().await;
        drop(transport);
        (recalled, read_to_end(&mut out_r).await)
    }

    const PING_1: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

    /// A `receive()` re-called after a READ ERROR discards the half-read line, exactly as rmcp's
    /// `AsyncRwTransport` does (it clears at every loop head): same result, same bytes written.
    ///
    /// Mutant: the `Err` arm leaves `line_complete` false, so the re-call resumes the partial line.
    #[tokio::test]
    async fn a_read_error_discards_the_partial_line_like_rmcp() {
        let rmcp = recall_after_read_error(AsyncRwTransport::new_server).await;
        let ours = recall_after_read_error(DupScanningTransport::new).await;
        assert_eq!(
            (ours.0, String::from_utf8_lossy(&ours.1)),
            (rmcp.0, String::from_utf8_lossy(&rmcp.1)),
            "after a read error the re-call must match rmcp (result, bytes written)"
        );
    }
}
