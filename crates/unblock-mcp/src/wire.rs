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
//! 2. A fix in a `Transport` **decorator** (the [`crate::pre_handshake`] kind) is impossible for the
//!    same reason one level up: `receive()` hands a decorator an already-parsed
//!    `RxJsonRpcMessage`.
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
//! frame can draw here are out-of-band ones its ENVELOPE draws when the line fails the typed parse
//! (the three rules below), never one for the duplicate. Rejection for it happens at exactly one
//! site: `call_tool` (`crate::server`).
//!
//! # A LINE THAT FAILS THE TYPED PARSE — THREE RULES (normative, PRD §4 D54, which closed `ub-788`, as kept by D59 clause (2))
//!
//! rmcp 3.10's own reader writes NOTHING for a line that is not valid JSON
//! (`rmcp-3.10.0/src/transport/async_rw.rs:317-323`) and answers a well-formed line of the wrong
//! shape with an id-less `-32600 Invalid request` (`:325-330`, flushed at `:253-268`). An rmcp client
//! drops an id-less error (`rmcp-3.10.0/src/service.rs:1766-1771`) and awaits its request with no
//! timeout by default (`:790-803`), so following rmcp would turn every readable-id parse failure into
//! a caller that waits forever — the hang D54 exists to end. This transport therefore answers the
//! typed-parse `Err` arm by three rules, keyed on D54's strictness leg and never on serde's error
//! category (RULED by Miguel, tracker comment 406):
//!
//! - **(α)** a line that is NOT strict JSON (not UTF-8 after the one BOM strip, truncated, trailing
//!   bytes, a lone surrogate, nesting at or past 128) gets the id-less `-32700 Parse error`, where
//!   rmcp writes nothing;
//! - **(β)** a strict-JSON line whose id [`crate::envelope_id::scan`] recovers gets `-32700 Parse
//!   error` ON that id, where rmcp writes the id-less `-32600`;
//! - **(γ)** a strict-JSON line whose id cannot be read (no `id`, an id that is no `RequestId`, two
//!   differing ids, a non-object root) gets rmcp's own reply, the id-less
//!   `ErrorData::invalid_request("Invalid request", None)`, byte for byte.
//!
//! A line the compatibility filter drops still gets no reply, at parity with rmcp: a strict-JSON line
//! whose last-wins `method` is a non-standard `notifications/*`, with or without an `id`, and an
//! id-LESS line whose `method` is any non-standard method (`async_rw.rs:437-455`).
//!
//! The strictness leg is NOT a second parse: it is the `Value` parse rmcp's compatibility filter
//! already runs on every failed UTF-8 line, surfaced as [`ParseFailure`]. It is load-bearing: `scan`
//! drains non-`id` members with `IgnoredAny`, which checks no nesting depth, validates neither UTF-8
//! nor surrogate escapes inside the strings it skips, and range-checks no number. Without the gate,
//! trailing bytes after a complete object, a non-UTF-8 byte or a lone surrogate inside a skipped
//! string, and nesting past `serde_json`'s 128-level limit would each recover an id from a line that
//! is not JSON.
//!
//! rmcp 3.10 decodes a frame by key PRESENCE first (`rmcp-3.10.0/src/model.rs:907`), so a frame
//! carrying both `method` and `id` is a Request or nothing: it runs when it types and otherwise
//! meets the rules above. D47's Notification-keyed arm and its `-32600` are RETIRED (D59 clause (3)),
//! and a float id that is an exact integer below 2^53 (`1.0`, `1e2`) types as that integer and runs.
//!
//! # `receive()` IS CANCEL-SAFE (normative — D47 clause 8(v), closed by `ub-nbz`; D53)
//!
//! rmcp polls `receive()` as one arm of a `tokio::select!` in its serve loop and DROPS it whenever
//! another arm wins, most often a handler response. So two kinds of state must not live only inside
//! the `receive()` future, and neither does.
//!
//! 1. **An out-of-band reply is never written inside `receive()`.** Every reply of the three rules
//!    is handed to a `tokio::spawn`ed task. The task runs the SAME `'static` write future `send()`
//!    builds, and its `JoinHandle` is PARKED on the transport. The handle is awaited at the top of
//!    EVERY loop iteration, so the reply is on the wire before the next frame is read, which is the
//!    order an inline write gave. It is also awaited in `close()` before the write half is taken.
//!    `close()` waits for it exactly as it waits for any rmcp send holding the write lock, so a peer
//!    that has stopped reading holds a signalled teardown until it reads or a second signal
//!    escalates (D38 clause (2)). A write that fails, or a task that panics, ends that SAME
//!    `receive()` call with `None` (or, if that call was dropped, the NEXT one), which is the
//!    contract the inline write shipped and what D40's teardown reads. A dropped `receive()` drops
//!    only its borrow of the handle. The task keeps running, and the next call or `close()` settles
//!    it. On the pre-lifecycle SIGNAL path rmcp drops the transport without `close()`; a parked
//!    reply's task is detached, keeps the write half, and finishes the reply unless the process
//!    exits first.
//! 2. **A dropped `receive()` never discards a partially read line** (D53). tokio's `read_until`
//!    leaves the bytes it consumed in the buffer when it is dropped, and documents calling it again
//!    as the recovery. So `line_buf` is cleared only after a COMPLETE line has been read and
//!    processed, never at the top of an iteration that a dropped call may have left half-filled.
//!    rmcp 3.10 now does the same upstream (`rmcp-3.10.0/src/transport/async_rw.rs:271-280`), so
//!    this is parity. A read ERROR also keeps the half-read line, as rmcp 3.10 does (`LineRead::Io`
//!    at `:142` and `:293-296`; its buffer is cleared only at `:306` and `:311`), so a re-called
//!    `receive()` RESUMES it (D59 clause (7)). EOF ends the read with `None` and DROPS an
//!    unterminated final line, as rmcp 3.10 does (`:290-292`): the MCP stdio transport delimits
//!    every message by a newline, so a conforming client never sends one.
//!
//! `tokio::spawn` makes a TOKIO RUNTIME a precondition of `receive()`, because it panics outside one.
//! Every driver already is one: rmcp spawns its own response writes, and the stdio pair is tokio's.
//!
//! The write mutex still buys only BYTE-ATOMICITY between concurrent writers. Cancellation-safety
//! comes from the task owning the write, which a dropped `receive()` cannot reach. A RUNTIME shutdown
//! can still abort that task mid-frame, exactly as it can any rmcp send task.
//!
//! # CD-7 — this module FORKS an undocumented rmcp internal
//!
//! `AsyncRwTransport`'s framing helpers (`try_parse_with_compatibility`, `should_ignore_notification`,
//! `is_standard_method`, `is_standard_notification`, `without_carriage_return`) are all PRIVATE to
//! rmcp's `transport::async_rw` module, so reproducing the read contract means re-implementing them.
//! The copies here are rmcp 3.10's (`rmcp-3.10.0/src/transport/async_rw.rs:396-470`).
//!
//! **The compatibility filter runs only AFTER the typed parse has already failed** (the mechanism
//! stated at [`should_ignore_notification`]). So it is *not* what makes an unknown notification
//! work: a `notifications/whatever` frame with a well-formed `params` object is accepted by rmcp's
//! own catch-all `CustomNotification` and DELIVERED, never reaching the filter. What the filter
//! governs is the narrower class rmcp cannot type at all — an LSP-style `$/cancelRequest`, or a
//! `notifications/*` frame whose `params` are not an object.
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
//! [`JsonRpcMessageCodec`], whose encoder is byte-identical (`async_rw.rs:610-617`), so the emitted
//! bytes are identical by construction rather than by test.
//!
//! **THE FORK CARRIES FOUR DELIBERATE DIVERGENCES FROM rmcp 3.10's `AsyncRwTransport`, AND ONLY
//! FOUR** (D59 clause (12)).
//!
//! 1. **Rule (α).** Our id-less `-32700` where rmcp writes nothing. The per-entry differential's
//!    `Silenced` tier pins it, asserting BOTH that rmcp writes zero bytes and that ours are the
//!    id-less `-32700`. One shape of it is `Recoded` instead (D59 clause (12), amended 2026-11-17):
//!    a line with trailing bytes whose typed decode fails on SHAPE before `end()` reaches them, for
//!    which rmcp writes its id-less `-32600` while ours keeps the id-less `-32700`.
//! 2. **Rule (β).** Our `-32700` on the recovered id where rmcp writes the id-less `-32600`. The
//!    `IdInserted` tier pins it, asserting BOTH that rmcp's bytes are still the id-less `-32600` (so
//!    an rmcp that starts answering on the id, or changes the message, code or framing, turns it
//!    red) AND that ours are the declared `-32700` on the declared id;
//!    `the_diverging_entries_are_exactly_the_declared_ones` pins that no other entry diverges.
//! 3. **The D43 duplicate-key scan** and its stamped verdict, which change no byte and no delivery.
//! 4. **The line length.** Unbounded here (`read_until`, no cap), where rmcp drops a line over
//!    16 MiB (`async_rw.rs:58`). DISCLOSED, not closed: `ub-o8s` stays OPEN and OUT of scope.
//!
//! Everything else — rule (γ), the EOF drop, the read-error resume, the partial line kept across a
//! dropped `receive()`, the BOM and CR strips and the blank-line skip — is PARITY.
//!
//! The spawn-and-park reply path (`ub-nbz`) is not counted as a divergence of its own. In a run that is
//! never cancelled it changes no byte and no order; it changes only whether an out-of-band reply
//! survives a dropped `receive()`. That is recorded as the close of D47 clause 8(v), not as a framing
//! divergence.

use std::sync::Arc;

use rmcp::model::{ErrorData, RequestId};
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
/// deserialize path is `rmcp::model::RequestMetaObject`, sourced from the typed `params._meta` member.
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

/// An owned `Transport<RoleServer>` over a byte stream pair. It reproduces rmcp 3.10's
/// `AsyncRwTransport` read framing except for the four divergences the CD-7 section lists: rules (α)
/// and (β), the D43 duplicate-key scan between the read and the parse, and the unbounded line.
pub(crate) struct DupScanningTransport<R, W> {
    /// The buffered read half. `read_until(b'\n', ..)` — there is deliberately no line-length bound
    /// here, exactly as rmcp has none (`max_length: usize::MAX`).
    read: BufReader<R>,
    /// The reusable line buffer, mirroring rmcp's `line_buf`. It is cleared only once the line in it
    /// was COMPLETELY read (`line_complete`), never at the top of an iteration that a dropped
    /// `read_until` or a read error may have left half-filled (D53, D59 clause (7)).
    line_buf: Vec<u8>,
    /// `true` when the next iteration must clear `line_buf`: the line in it was read to its end.
    /// `false` while the bytes of a `read_until` interrupted by a dropped `receive()` or by a read
    /// error are still waiting to be resumed.
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
    /// All three rules' replies go through here, so they are written identically **by
    /// construction** rather than by review.
    ///
    /// **Takes two FIELDS, not `&mut self`.** `receive()` still holds `line`, an immutable borrow of
    /// `self.line_buf`, at every call site. Borrowing only `parked_reply` (mutably) and `write`
    /// (shared) keeps the borrows disjoint.
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
            // D53: clear only a line that was read to its END. A `read_until` interrupted by a dropped
            // `receive()` or by a read error left the frame's head in `line_buf`, and this call
            // resumes it.
            if self.line_complete {
                self.line_buf.clear();
                self.line_complete = false;
            }
            match self.read.read_until(b'\n', &mut self.line_buf).await {
                // EOF. Any bytes still in `line_buf` are an unterminated final line with no
                // delimiter, so there is nothing to deliver: it is DROPPED, as rmcp 3.10 drops it
                // (D59 clause (7)).
                Ok(_) if !self.line_buf.ends_with(b"\n") => return None,
                Ok(_) => self.line_complete = true,
                Err(e) => {
                    // Nothing is swallowed here: the EOF/error shape is what the D40 pre-lifecycle
                    // teardown depends on. The half-read line is KEPT, as rmcp 3.10 keeps it, so a
                    // re-called `receive()` resumes it (D59 clause (7)).
                    tracing::error!("Error reading from stream: {}", e);
                    return None;
                }
            }
            // Strip the trailing `\n`, then ONE trailing `\r`. A whitespace-only line is NOT empty and
            // proceeds to the parse.
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
                    message.insert_extension(verdict);
                    return Some(message);
                }
                // Compat-ignored (an unknown client notification): emit nothing, answer nothing,
                // read the next line. Spelled as a fall-through rather than `continue` only
                // because it is the last arm; the semantics mirror rmcp's `continue` exactly.
                Ok(None) => {}
                Err(failure) => {
                    tracing::debug!("Parse error on incoming message: {}", failure.error());
                    // D54 KEPT by D59 clause (2), three rules. (β) a strict-JSON line whose id is
                    // readable gets `-32700` ON that id; (α) a line that is not strict JSON gets the
                    // id-less `-32700`; (γ) a strict-JSON line whose id cannot be read gets rmcp
                    // 3.10's own id-less `-32600`, byte for byte.
                    let id = parse_error_reply_id(&failure, line);
                    if answers_invalid_request(&failure, id.as_ref()) {
                        Self::park_reply(
                            &mut self.parked_reply,
                            &self.write,
                            ErrorData::invalid_request("Invalid request", None),
                            None,
                        );
                    } else {
                        Self::park_reply(
                            &mut self.parked_reply,
                            &self.write,
                            ErrorData::parse_error("Parse error", None),
                            id,
                        );
                    }
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
/// it. `send()` and the replies of all three rules therefore write through the same future, so their bytes are
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

/// Is `method` a standard MCP request or notification? (rmcp 3.10 `is_standard_method`, MCP
/// 2026-07-28, `rmcp-3.10.0/src/transport/async_rw.rs:396-418`.)
fn is_standard_method(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "ping"
            | "server/discover"
            | "prompts/get"
            | "prompts/list"
            | "resources/list"
            | "resources/read"
            | "resources/subscribe"
            | "resources/unsubscribe"
            | "resources/templates/list"
            | "subscriptions/listen"
            | "tools/call"
            | "tools/list"
            | "completion/complete"
            | "elicitation/create"
            | "logging/setLevel"
            | "roots/list"
            | "sampling/createMessage"
    ) || is_standard_notification(method)
}

/// Is `method` a standard MCP notification? (rmcp 3.10 `is_standard_notification`,
/// `rmcp-3.10.0/src/transport/async_rw.rs:420-434`.)
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
            | "notifications/subscriptions/acknowledged"
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
/// `Ok(Some(_))` = deliver, `Ok(None)` = silently ignore, `Err(ParseFailure)` = answer by the three
/// rules and recover; the variant says whether the line was strict JSON, which selects rule (α).
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
/// and is never strict JSON. rmcp 3.10 keys its own reply on serde's error CATEGORY instead
/// (`async_rw.rs:316-331`); this fork keys on strictness, as D54 measured it.
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

/// D54: the id the reply to a failed line carries — the one [`crate::envelope_id::scan`] recovers
/// from a STRICT-JSON line (rule (β)), and none from any other.
///
/// Both matches are exhaustive with no `_` arm: a new verdict or a new failure kind must be routed
/// here deliberately.
fn parse_error_reply_id(failure: &ParseFailure, line: &[u8]) -> Option<RequestId> {
    match failure {
        ParseFailure::NotJson(_) => None,
        ParseFailure::NotAMessage(_) => match envelope_id::scan(line) {
            EnvelopeId::Recovered(id) => Some(id),
            EnvelopeId::Absent | EnvelopeId::Unusable => None,
        },
    }
}

/// Is this failed line answered by rule (γ) — rmcp 3.10's own id-less `invalid_request("Invalid
/// request", None)` — rather than by a `-32700`? (D59 clause (2), RULED by Miguel in tracker
/// comment 406.)
///
/// Exactly the strict-JSON lines whose id cannot be read: no `id`, an id that is no `RequestId`, two
/// differing ids, a non-object root. A line that is not strict JSON keeps D54's id-less `-32700`
/// (rule (α)), and a readable id keeps its `-32700` on that id (rule (β)).
fn answers_invalid_request(failure: &ParseFailure, id: Option<&RequestId>) -> bool {
    matches!(failure, ParseFailure::NotAMessage(_)) && id.is_none()
}

#[cfg(test)]
mod tests {
    use super::{DupScanningTransport, ParamsScan};
    use crate::envelope_id_corpus::{
        Expect, ParseExpect, ParseRole, expected_bytes, id_corpus, parse_error_bytes,
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

    /// The §4.6 framing corpus — ONE byte corpus, consumed by both transports, each entry with its
    /// tier. The whole-stream cell feeds the PARITY entries; the per-entry differential grades all.
    ///
    /// Each entry is a raw line written to the transport's read half. It deliberately includes the
    /// shapes whose handling is invisible to any test that goes through an rmcp CLIENT: a client
    /// serializes an already-deduplicated object and structurally cannot emit a duplicate key.
    fn framing_corpus() -> Vec<(&'static str, Vec<u8>, Tier)> {
        let mut tiered: Vec<(&'static str, Vec<u8>, Tier)> = Vec::new();
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
        // F6 — a whitespace-only line: NOT empty, so it parses and fails. It is not strict JSON, so
        // rule (α) answers the id-less -32700 where rmcp 3.10 writes nothing: the SILENCED tier.
        tiered.push(("F6", b"   ".to_vec(), Tier::Silenced));
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
        // still be answered, not silently dropped. Both arms must decline it. It is strict JSON
        // with no id, so rule (γ) answers rmcp's own id-less -32600: parity.
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
        // This is ALSO the DELIBERATE PARITY DROP: it carries a readable `id`, so a reader
        // reasonably expects rule (β) to answer it. It must NOT, and no carve-out exists for it —
        // the exclusion is STRUCTURAL. `try_parse_with_compatibility` returns `Ok(None)` for this
        // frame, so it never reaches the parse-error arm. It stays byte-silent because rmcp is
        // byte-silent, which is the whole point of the fork.
        corpus.push((
            "F17",
            br#"{"jsonrpc":"2.0","id":17,"method":"notifications/foo","params":5}"#.to_vec(),
        ));
        // F9 — an unknown method WITH an id: delivered (the handler answers -32601).
        corpus.push((
            "F9",
            br#"{"jsonrpc":"2.0","id":9,"method":"nope/nope","params":{}}"#.to_vec(),
        ));
        // F10 — non-UTF-8 bytes: rule (α), the id-less -32700, where rmcp writes nothing (SILENCED).
        // F10 does NOT grade the D54 gate — `scan` finds no `id` in it either way; the UTF-8
        // witness is parse-error corpus entry X03.
        tiered.push(("F10", vec![b'{', 0xff, 0xfe, b'}'], Tier::Silenced));
        // F11 — depth-130 nesting: past serde_json's 128-level limit for BOTH parsers, so rule (α)
        // (SILENCED). It is ALSO the strict-JSON gate's depth witness: `envelope_id::scan` alone
        // recovers id 11 from it (`IgnoredAny` checks no depth), so it stays id-less on our side
        // only because the gate rejects it.
        let mut deep = String::from(r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":"#);
        deep.push_str(&"[".repeat(130));
        deep.push_str(&"]".repeat(130));
        deep.push('}');
        tiered.push(("F11", deep.into_bytes(), Tier::Silenced));
        // -- NEVER-ANSWERED negatives ---------------------------------------------------------------
        //
        // These are PARITY entries: each must stay byte-identical to rmcp. They exist because an
        // id-recovering arm's failure mode is OVER-firing, and they distinguish "the predicate is
        // correct" from "the predicate answers anything with an `id`-shaped thing near it".
        //
        // F18 — a NESTED `id`. The envelope id is a ROOT member by definition, so `params.id` is
        // invisible: delivered, nothing written. Dies if the scan recurses instead of `IgnoredAny`.
        corpus.push((
            "F18",
            br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{"id":1}}"#.to_vec(),
        ));
        // F19 — a WELL-FORMED id on a `notifications/*` method. rmcp 3.10 decodes a `method`+`id`
        // frame as a Request (`CustomRequest` here), so it is DELIVERED and nothing is written.
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
        // LAST entry, so this one exercises it. rmcp 3.10 DROPS it at EOF, and so do we (D59
        // clause (7)): parity, nothing delivered and nothing written.
        corpus.push((
            "F7",
            br#"{"jsonrpc":"2.0","id":7,"method":"ping","params":{}}"#.to_vec(),
        ));
        let last = corpus.pop().expect("F7 closes the corpus");
        let mut all: Vec<(&'static str, Vec<u8>, Tier)> = corpus
            .into_iter()
            .map(|(label, bytes)| (label, bytes, Tier::Parity))
            .collect();
        all.extend(tiered);
        all.push((last.0, last.1, Tier::Parity));
        all
    }

    /// The framing corpus's PARITY entries, in order — the whole-stream cell's input.
    fn parity_framing_corpus() -> Vec<(&'static str, Vec<u8>)> {
        framing_corpus()
            .into_iter()
            .filter(|(_, _, tier)| *tier == Tier::Parity)
            .map(|(label, bytes, _)| (label, bytes))
            .collect()
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
    /// Feed ONE byte corpus to rmcp 3.10's `AsyncRwTransport` and to our `DupScanningTransport` and
    /// assert identical `receive()` sequences AND identical bytes written (rule (γ)'s id-less
    /// `-32600`, recovery on the next line, the EOF drop). Only the framing corpus's PARITY entries
    /// ride this stream; the rule-(α) entries (F6, F10, F11) are graded per entry by the `Silenced`
    /// tier, and the rule-(β) lines by the `IdInserted` tier. This differential pin, TOGETHER WITH
    /// the arm-by-arm cell below (`the_compatibility_filter_is_entered_and_discriminates`), is
    /// what stands between an rmcp bump and a silent framing divergence — neither alone suffices,
    /// which is the module doc's standing note at the top of this file.
    #[tokio::test]
    async fn cd7_framing_is_identical_to_rmcp_async_rw_transport() {
        let corpus = parity_framing_corpus();
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

        // Non-vacuity: the corpus must actually produce deliveries and a rule-(γ) reply, and the
        // unterminated F7 must be DROPPED by both (its id 7 is delivered by neither).
        assert!(
            rmcp_received.len() >= 4,
            "the corpus must deliver several messages, got {}",
            rmcp_received.len()
        );
        let invalid_requests = String::from_utf8_lossy(&rmcp_written)
            .matches("-32600")
            .count();
        assert!(
            invalid_requests >= 1,
            "the corpus must provoke rmcp's id-less -32600, got {invalid_requests}"
        );
        assert!(
            !rmcp_received.iter().any(|m| m.contains(r#""id":7"#)),
            "the unterminated final line is dropped at EOF (D59 clause (7))"
        );
        assert!(
            !String::from_utf8_lossy(&our_written).contains("\"id\":null"),
            "the id-less reply must OMIT the id, not send a null one"
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
        // `params` is a real client defect and must be answered (rule (γ)), not vanish.
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
        // arm decline it and the prefix test makes the second decline it too, so it is answered
        // instead of vanishing. (F17 above is the deliberate exception rmcp itself defines, and
        // only inside that prefix.) It is strict JSON with a readable root `id` that fails the
        // typed parse, so rule (β) answers -32700 on id 1 and a waiting rmcp client is released
        // (parse-error corpus entry E22). This assertion pins only that the frame is not IGNORED.
        assert!(
            parse(br#"{"jsonrpc":"2.0","id":1,"method":"nope/nope","params":5}"#).is_err(),
            "a request outside `notifications/*` must never be ignored — the client is waiting on \
             its id"
        );

        // The rmcp 3.10 lists (D59 clause (12), criterion (o)): an id-LESS `server/discover` with
        // untypeable `params` is NOT filtered, because 3.10's `is_standard_method` names
        // `server/discover`. rmcp 1.7's 2025-06-18 list did not, so its first arm (no `id` + a
        // non-standard method) dropped this frame. With an id the two lists give the same verdict
        // (`rmcp-3.10.0/src/transport/async_rw.rs:437-455`), so only the id-less frame bites.
        // `subscriptions/listen` and `elicitation/create` are the other two methods 3.10 added.
        for method in [
            "server/discover",
            "subscriptions/listen",
            "elicitation/create",
        ] {
            let line = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":5}}"#);
            assert!(
                parse(line.as_bytes()).is_err(),
                "an id-less `{method}` with untypeable params is a STANDARD method on rmcp 3.10 and \
                 must be answered, never filtered"
            );
        }
        // `notifications/subscriptions/acknowledged` is the one standard NOTIFICATION 3.10 added:
        // with an id and untypeable params, the prefix arm would drop it under the 1.7 list.
        assert!(
            parse(
                br#"{"jsonrpc":"2.0","id":3,"method":"notifications/subscriptions/acknowledged","params":5}"#
            )
            .is_err(),
            "a STANDARD 3.10 notification with untypeable params is answered, never filtered"
        );
    }

    /// **Criterion (o)** — the fork's standard-method verdict equals rmcp 3.10's LIVE reader on the
    /// id-less `server/discover` frame: rmcp answers it (its id-less `-32600`), so the fork must not
    /// filter it either. The differential is live, so an rmcp bump that changes its list turns this
    /// red even if the copy above is never edited.
    #[tokio::test]
    async fn the_id_less_server_discover_frame_gets_rmcps_verdict() {
        let frame = br#"{"jsonrpc":"2.0","method":"server/discover","params":5}"#;
        let (our_received, our_written) = run_ours(frame).await;
        let (rmcp_received, rmcp_written) = run_rmcp(frame).await;
        assert_eq!(
            String::from_utf8_lossy(&rmcp_written),
            String::from_utf8_lossy(&parse_error_bytes(&ParseExpect::InvalidRequest)),
            "rmcp 3.10 must answer the id-less server/discover frame, not filter it"
        );
        assert_eq!(
            (our_received, String::from_utf8_lossy(&our_written)),
            (rmcp_received, String::from_utf8_lossy(&rmcp_written)),
            "the fork must give rmcp's verdict, byte for byte"
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
    // THE PARSE-ERROR AND ENVELOPE-`id` CELLS (D54, re-tiered by D59)
    //
    // Homed here and not in an integration suite for one reason: only the in-module harness owns
    // the RAW WRITTEN STREAM and both transports. Neither integration harness can assert exact
    // bytes — each parses every line to a `Value` before the caller sees it, which loses member
    // ORDER and the presence-versus-null distinction, and those are exactly what these cells pin.
    // =============================================================================================

    /// Which tier one entry of the FULL corpus belongs to (D59 clause (12)).
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Tier {
        /// Must stay byte-identical to rmcp 3.10's `AsyncRwTransport`, rule (γ) and the EOF drop
        /// included.
        Parity,
        /// Rule (α): rmcp writes NOTHING, ours writes the id-less `-32700`.
        Silenced,
        /// Rule (α) on a line whose typed decode fails on SHAPE before `end()` reaches its trailing
        /// bytes: rmcp writes its id-less `-32600`, ours the id-less `-32700` (D59 clause (12),
        /// amended 2026-11-17).
        Recoded,
        /// Rule (β): rmcp writes its id-less `-32600`, ours writes `-32700` ON the declared id.
        IdInserted(ParseExpect),
    }

    /// Every tier as ONE labelled list: the framing corpus, the envelope-`id` corpus (rule (β) for a
    /// recovered id, parity for rule (γ) and for a float id that runs) and D54's parse-error corpus
    /// (rule (β) for an in-class entry, `Silenced` or `Recoded` for rule (α), parity for every
    /// other).
    fn full_corpus() -> Vec<(String, Vec<u8>, Tier)> {
        let mut all: Vec<(String, Vec<u8>, Tier)> = framing_corpus()
            .into_iter()
            .map(|(label, bytes, tier)| (label.to_string(), bytes, tier))
            .collect();
        all.extend(id_corpus().into_iter().map(|entry| {
            let tier = match entry.expect {
                Expect::RecoveredNum(n) => Tier::IdInserted(ParseExpect::RecoveredNum(n)),
                Expect::RecoveredStr(s) => Tier::IdInserted(ParseExpect::RecoveredStr(s)),
                Expect::Omitted | Expect::Runs(_) => Tier::Parity,
            };
            (entry.id.to_string(), entry.frame, tier)
        }));
        all.extend(parse_error_corpus().into_iter().map(|entry| {
            let tier = match entry.expect {
                ParseExpect::RecoveredNum(_) | ParseExpect::RecoveredStr(_) => {
                    Tier::IdInserted(entry.expect)
                }
                ParseExpect::IdLess if entry.role == ParseRole::Recoded => Tier::Recoded,
                ParseExpect::IdLess => Tier::Silenced,
                ParseExpect::InvalidRequest | ParseExpect::Silent => Tier::Parity,
            };
            (entry.id.to_string(), entry.frame, tier)
        }));
        all
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

    /// **W-D01..W-D26** — every envelope-`id` entry is answered BYTE FOR BYTE and never delivered,
    /// or, for a float-form integer id, delivered on that integer with nothing written.
    ///
    /// The assertion is EXACT bytes, not "differs from rmcp": a mutant writing garbage satisfies
    /// "differs" and cannot satisfy this. The per-entry id choice is the only thing that actually
    /// pins the unambiguity rule.
    ///
    /// Mutants: `invalid_request` swapped for `parse_error` on either arm; the recovered id replaced
    /// by `None`; `None` replaced by any id on rule (γ); answer AND deliver. **W-D23** additionally
    /// carries the raw-span VALUE comparator, **W-D22** the D43-suppresses-D54 precedence mutant,
    /// and **W-D10/W-D11/W-D26** a fork that refuses float-form integer ids rmcp 3.10 accepts.
    #[tokio::test]
    async fn the_id_corpus_is_answered_byte_for_byte() {
        for entry in id_corpus() {
            let (received, written) = run_ours(&entry.frame).await;
            assert_eq!(
                String::from_utf8_lossy(&written),
                String::from_utf8_lossy(&expected_bytes(&entry.expect)),
                "{} wrote the wrong bytes — {}",
                entry.id,
                entry.why
            );
            if let Expect::Runs(id) = entry.expect {
                assert_eq!(received.len(), 1, "{} must RUN as a request", entry.id);
                assert!(
                    received[0].contains(&format!(r#""id":{id},"#)),
                    "{} must be delivered on the INTEGER id {id}: {}",
                    entry.id,
                    received[0]
                );
            } else {
                assert!(
                    received.is_empty(),
                    "{} must be ANSWERED AND DROPPED, never delivered — got {received:?}",
                    entry.id
                );
            }
        }
    }

    /// **W-DROP** — an answered frame is never delivered, and exactly one reply is written per frame.
    ///
    /// Mutant: the parse-error arm answering AND delivering a frame. A frame that failed the typed
    /// parse has no message to deliver, so this pins that no arm fabricates one. The count below
    /// catches the other half, since exactly one reply per frame is written.
    #[tokio::test]
    async fn an_answered_frame_is_never_delivered() {
        let corpus = id_corpus();
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

        assert!(received.is_empty(), "no answered frame may be delivered");
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
        let corpus = id_corpus();
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
            "exactly ONE reply — rule (γ)'s — and nothing for the clean ping"
        );
    }

    /// **W-N1** — a notification with NO `id` is DELIVERED and nothing is written.
    ///
    /// This is a JSON-RPC requirement ("The Server MUST NOT reply to a Notification"), so it is a
    /// false-POSITIVE guard rather than a coverage cell.
    ///
    /// Mutant: a reply arm that fires on a frame that typed.
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
    /// F17 carries a readable `id`, so a reader expects rule (β) to answer it. The exclusion is
    /// STRUCTURAL rather than a carve-out: `try_parse_with_compatibility` returns `Ok(None)` for
    /// this frame, so it never reaches the parse-error arm. rmcp drops it too.
    ///
    /// Mutants: neutering the compat filter's second arm; running the id recovery ABOVE the parse
    /// so it keys on raw bytes alone.
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
    /// Mutant: an id-recovering branch moved above the parse.
    #[tokio::test]
    async fn a_clean_request_is_untouched() {
        let (received, written) =
            run_ours(br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#).await;
        assert_eq!(received.len(), 1);
        assert!(written.is_empty(), "request traffic must pay nothing");
    }

    /// **W-N5** — a D43 frame is not STOLEN by the parse-error arm.
    ///
    /// The duplicate is inside `params.arguments` and the ROOT `id` is perfectly fine, so this
    /// stays a D43 frame end to end: delivered, carrying its `Duplicate` verdict, unanswered.
    ///
    /// Mutant: answering any frame whose raw bytes hold a duplicate, which would never deliver it —
    /// silently disabling the whole D43 gate.
    #[tokio::test]
    async fn d43_frames_are_not_stolen_by_the_parse_error_arm() {
        let (received, written) = run_ours(
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"issue","arguments":{"id":"a","id":"b"}}}"#,
        )
        .await;
        assert_eq!(received.len(), 1, "a D43 frame must still be DELIVERED");
        assert!(
            written.is_empty(),
            "and must NOT be answered by the parse-error arm"
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

    /// **W-G2** — the per-entry differential over the FULL corpus against rmcp 3.10's LIVE reader,
    /// every tier: Parity (rule (γ) and the EOF drop included), `Silenced` (rule (α)) and
    /// `IdInserted` (rule (β)) — D59 clause (12), criteria (c) and (d).
    ///
    /// The whole-stream cell above keeps its own job (F2's CRLF, F5's blank line and F7's
    /// unterminated final line are STREAM properties a per-entry harness destroys, which is why
    /// that cell must not be deleted as redundant). This one grades each entry independently.
    ///
    /// Each diverging tier asserts BOTH halves: that rmcp's own bytes are still what D59 recorded
    /// (nothing for rule (α), the id-less `-32600` for rule (β)), so an rmcp that starts answering
    /// those lines itself turns it red, and that ours are the declared reply, so a fork that drifts
    /// to rmcp's bytes turns it red.
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
                Tier::Silenced => {
                    // (a) rmcp 3.10 writes NOTHING for a line that is not strict JSON.
                    assert!(
                        rmcp_written.is_empty(),
                        "{label}: rmcp must write nothing for this line — if it now answers it, \
                         re-derive rule (α): {}",
                        String::from_utf8_lossy(&rmcp_written)
                    );
                    // (b) ours is D54's id-less -32700 — a hand-written literal.
                    assert_eq!(
                        String::from_utf8_lossy(&our_written),
                        String::from_utf8_lossy(&parse_error_bytes(&ParseExpect::IdLess)),
                        "{label}: ours must be the id-less -32700 (rule (α))"
                    );
                    assert!(
                        our_received.is_empty() && rmcp_received.is_empty(),
                        "{label}: a line that fails the typed parse is never delivered"
                    );
                }
                Tier::Recoded => {
                    // (a) rmcp 3.10's decode stops at the shape error first: its id-less -32600.
                    assert_eq!(
                        String::from_utf8_lossy(&rmcp_written),
                        String::from_utf8_lossy(&parse_error_bytes(&ParseExpect::InvalidRequest)),
                        "{label}: rmcp must still write its id-less -32600 for this line — if it \
                         now writes nothing, move the entry to the Silenced tier"
                    );
                    // (b) ours keeps rule (α)'s id-less -32700 (ruling 406).
                    assert_eq!(
                        String::from_utf8_lossy(&our_written),
                        String::from_utf8_lossy(&parse_error_bytes(&ParseExpect::IdLess)),
                        "{label}: ours must be the id-less -32700 (rule (α))"
                    );
                    assert!(
                        our_received.is_empty() && rmcp_received.is_empty(),
                        "{label}: a line that fails the typed parse is never delivered"
                    );
                }
                Tier::IdInserted(expect) => {
                    // (a) rmcp's OWN reply is still the id-less -32600 — a hand-written literal.
                    assert_eq!(
                        String::from_utf8_lossy(&rmcp_written),
                        String::from_utf8_lossy(&parse_error_bytes(&ParseExpect::InvalidRequest)),
                        "{label}: rmcp's own reply must still be the id-less -32600 — if rmcp now \
                         answers on the id itself, or changed the code, message or framing, \
                         re-derive rule (β)"
                    );
                    // (b) ours is the -32700 ON EXACTLY the declared id — a hand-written literal.
                    assert_eq!(
                        String::from_utf8_lossy(&our_written),
                        String::from_utf8_lossy(&parse_error_bytes(&expect)),
                        "{label}: ours must be -32700 on EXACTLY the declared id (rule (β))"
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
    /// Without it, an OVER-firing arm that also answered the id-less F8/F12 would leave both other
    /// tiers green: the parity tier would still match rmcp on the entries it did not touch, and the
    /// diverging tiers would still match their expected bytes.
    ///
    /// Mutant: a reply arm that fires on frames that type (F1–F4/F9/F19 gain replies), or any rmcp
    /// bump that migrates an entry between tiers. The declared set is EXACTLY `Silenced` ∪ `Recoded`
    /// ∪ `IdInserted`; D47's tier is deleted (D59 clause (12), amended 2026-11-17).
    #[tokio::test]
    async fn the_diverging_entries_are_exactly_the_declared_ones() {
        use std::collections::BTreeSet;

        let mut observed: BTreeSet<String> = BTreeSet::new();
        let mut declared: BTreeSet<String> = BTreeSet::new();
        let mut all_our_written = String::new();

        for (label, frame, tier) in full_corpus() {
            if matches!(tier, Tier::Silenced | Tier::Recoded | Tier::IdInserted(_)) {
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
             Silenced, Recoded and IdInserted entries — an entry in `observed` only is an \
             over-firing arm, an entry in `declared` only is a divergence that stopped working"
        );
        assert!(
            !declared.is_empty(),
            "a corpus with no declared divergence would make this guard vacuous"
        );

        // The RATIFIED id-less spelling, pinned over the whole stream. This is a SECOND, WIDER copy
        // of the guard the CD-7 whole-stream cell carries — not a migration of it.
        assert!(
            !all_our_written.contains("\"id\":null"),
            "an id-less reply spells the missing id by OMISSION: rmcp's JsonRpcError.id is \
             Option<RequestId> with skip_serializing_if = \"Option::is_none\", so a literal null \
             is not reachable through the codec. If the decision is ever revised to a literal null \
             (a deliberate codec bypass), THIS is the assertion to change."
        );
    }

    /// **W-G4** — envelope-`id` outcome coverage as a SET, never a count.
    ///
    /// A count would rot; a set cannot be off-by-one against itself. Its "site" is the CORPUS, not
    /// a production site: the mutation it grades is a corpus EDIT that silently drops one half of
    /// the recovery rule (e.g. deleting every string-id entry) or every float-id entry.
    #[test]
    fn every_id_corpus_kind_is_represented() {
        use crate::envelope_id_corpus::ExpectKind;
        use std::collections::BTreeSet;

        let present: BTreeSet<ExpectKind> = id_corpus().iter().map(|f| f.expect.kind()).collect();
        let required: BTreeSet<ExpectKind> = [
            ExpectKind::RecoveredNum,
            ExpectKind::RecoveredStr,
            ExpectKind::Omitted,
            ExpectKind::Runs,
        ]
        .into_iter()
        .collect();
        assert_eq!(
            present, required,
            "the corpus must exercise all four outcomes"
        );
    }

    /// **W-G5** — corpus non-vacuity: every entry really carries what its `why` claims.
    ///
    /// The failure this guards is the one this repo has paid for: someone "tidies" a frame into a
    /// well-formed one and the cell keeps passing while grading nothing.
    #[test]
    fn the_id_corpus_is_not_vacuous() {
        for entry in id_corpus() {
            let text = String::from_utf8_lossy(&entry.frame);

            // Every entry must be a frame the id scan can even see: the raw bytes must carry a root
            // `id` member in SOME spelling.
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

            // A float-id entry must really be written in FLOAT form and decode to its declared
            // integer, or it stops grading rmcp 3.10's float-id acceptance.
            if let Expect::Runs(want) = entry.expect {
                use serde::Deserialize as _;
                let parsed: serde_json::Value =
                    serde_json::from_slice(&entry.frame).expect("a float-id entry is JSON");
                let id = &parsed["id"];
                assert!(
                    id.is_f64(),
                    "{}: a float-id entry must spell its id in float form: {id}",
                    entry.id
                );
                assert_eq!(
                    RequestId::deserialize(id.clone()).ok(),
                    Some(RequestId::Number(want)),
                    "{}: the float-form id must decode to the declared integer",
                    entry.id
                );
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

    /// **W-E1** — every D54 parse-error entry is answered BYTE FOR BYTE, and never delivered.
    ///
    /// Exact hand-written bytes (`parse_error_bytes`), never the production encoder: a mutant that
    /// answers on a wrong id, a stringified id, `-32600`, another message or `"id":null` satisfies
    /// "differs from rmcp" and cannot satisfy this. It needs no rmcp, and it is the only check of the
    /// out-of-class entries against a hand literal rather than against rmcp's live bytes.
    ///
    /// Mutants: the recovered arm answering `None` (every E entry); the strict-JSON gate removed or
    /// weakened (X01, X03, X04, X05, X12, F11 in CD-7); a non-UTF-8 line classed `NotAMessage` (X03
    /// alone); a byte prefilter on the literal `"id"` (E16 alone); numeric ids only (E04, E14, E15);
    /// a gate stricter than `Value` (E18, E24); answer AND deliver (every E entry); rule (γ)
    /// answered `-32700` like rule (α) (X06–X10, X14).
    #[tokio::test]
    async fn the_parse_error_corpus_is_answered_byte_for_byte() {
        for entry in parse_error_corpus() {
            let (received, written) = run_ours(&entry.frame).await;
            assert_eq!(
                String::from_utf8_lossy(&written),
                String::from_utf8_lossy(&parse_error_bytes(&entry.expect)),
                "{}: wrong reply bytes — {}",
                entry.id,
                entry.why
            );
            assert!(
                received.is_empty(),
                "{}: a line that fails the typed parse is never delivered",
                entry.id
            );
        }
    }

    /// **W-G5′** — parse-error corpus non-vacuity: every entry still REACHES what its role says.
    ///
    /// The byte cells stay green on a "tidied" entry that grades nothing — X03's bad byte moved into
    /// a key (where `scan` finds no id at all), or an E entry rewritten so the typed parse succeeds.
    /// This is the ub-cnv failure mode, and this cell is what catches it.
    #[test]
    fn the_parse_error_corpus_is_not_vacuous() {
        use crate::envelope_id::{EnvelopeId, scan};
        use crate::envelope_id_corpus::{GateHalf, ParseRole};
        use rmcp::model::NumberOrString;

        fn parse(line: &[u8]) -> Result<Option<RxJsonRpcMessage<RoleServer>>, super::ParseFailure> {
            super::try_parse_with_compatibility::<RxJsonRpcMessage<RoleServer>>(line)
        }

        for entry in parse_error_corpus() {
            let id = entry.id;
            let verdict = scan(&entry.frame);
            let scan_recovers = matches!(verdict, EnvelopeId::Recovered(_));
            let parsed = parse(&entry.frame);
            let body = entry
                .frame
                .strip_prefix(super::UTF8_BOM.as_slice())
                .unwrap_or(&entry.frame);
            match entry.role {
                ParseRole::InClass => {
                    assert!(
                        matches!(parsed, Err(super::ParseFailure::NotAMessage(_))),
                        "{id}: an in-class entry must fail the typed parse AND pass the gate"
                    );
                    let declared = match (&verdict, &entry.expect) {
                        (
                            EnvelopeId::Recovered(NumberOrString::Number(got)),
                            ParseExpect::RecoveredNum(want),
                        ) => got == want,
                        (
                            EnvelopeId::Recovered(NumberOrString::String(got)),
                            ParseExpect::RecoveredStr(want),
                        ) => &**got == *want,
                        _ => false,
                    };
                    assert!(declared, "{id}: scan must recover EXACTLY the declared id");
                }
                ParseRole::GateWitness(half) => {
                    assert!(
                        matches!(parsed, Err(super::ParseFailure::NotJson(_))),
                        "{id}: a gate witness must fail the gate"
                    );
                    assert!(
                        scan_recovers,
                        "{id}: a gate witness must be a line scan ALONE would answer on an id"
                    );
                    assert_eq!(
                        std::str::from_utf8(body).is_err(),
                        half == GateHalf::Utf8,
                        "{id}: the entry must witness the gate half it declares"
                    );
                }
                ParseRole::ScanWitness => {
                    assert!(
                        matches!(parsed, Err(super::ParseFailure::NotAMessage(_))),
                        "{id}: a scan witness must pass the gate and fail the typed parse"
                    );
                    assert!(!scan_recovers, "{id}: a scan witness must recover no id");
                }
                ParseRole::CompatDropped => {
                    assert!(
                        matches!(parsed, Ok(None)),
                        "{id}: the compatibility filter must drop it"
                    );
                }
                ParseRole::ParityOnly => {
                    assert!(
                        matches!(parsed, Err(super::ParseFailure::NotJson(_))) && !scan_recovers,
                        "{id}: a parity-only entry fails the gate and recovers nothing"
                    );
                }
                ParseRole::Recoded => {
                    assert!(
                        matches!(parsed, Err(super::ParseFailure::NotJson(_))) && scan_recovers,
                        "{id}: a recoded entry fails the gate while scan alone recovers an id"
                    );
                    let typed = serde_json::from_slice::<RxJsonRpcMessage<RoleServer>>(body)
                        .expect_err("a recoded entry fails the typed decode");
                    assert_eq!(
                        typed.classify(),
                        serde_json::error::Category::Data,
                        "{id}: the typed decode must fail on SHAPE before `end()`, which is what \
                         makes rmcp 3.10 answer -32600 here"
                    );
                }
            }
        }
    }

    /// **W-G4′** — parse-error reply kinds AND gate halves as SETS, never counts.
    ///
    /// Mutant (a corpus edit): deleting every string-id entry, or deleting X03 — after which no
    /// entry sees a non-UTF-8 line classed as JSON — or deleting (or reshaping) a witness required
    /// by id. X01, X03, X10, E16 and E24 are each the only entry that kills its mutant. E15 and E17
    /// are each the only PARSE-ERROR-corpus entry that kills its mutant, which `envelope_id`'s own
    /// cells and the envelope-`id` corpus (D23, D19) also kill. E18 is X01's in-class boundary
    /// partner, required by id although X06 and X07 also kill its mutant.
    #[test]
    fn every_parse_error_kind_is_represented() {
        use crate::envelope_id_corpus::{GateHalf, ParseExpect, ParseExpectKind, ParseRole};
        use std::collections::BTreeSet;

        let corpus = parse_error_corpus();
        let kinds: BTreeSet<ParseExpectKind> = corpus.iter().map(|f| f.expect.kind()).collect();
        let required: BTreeSet<ParseExpectKind> = [
            ParseExpectKind::RecoveredNum,
            ParseExpectKind::RecoveredStr,
            ParseExpectKind::IdLess,
            ParseExpectKind::InvalidRequest,
            ParseExpectKind::Silent,
        ]
        .into_iter()
        .collect();
        assert_eq!(
            kinds, required,
            "the corpus must exercise all five reply shapes"
        );

        let halves: BTreeSet<GateHalf> = corpus
            .iter()
            .filter_map(|f| match f.role {
                ParseRole::GateWitness(half) => Some(half),
                _ => None,
            })
            .collect();
        assert_eq!(
            halves,
            [GateHalf::Utf8, GateHalf::Json].into_iter().collect(),
            "the corpus must witness BOTH halves of the strict-JSON gate"
        );

        // The row-unique witnesses, required BY ID with their declared reply and role, so neither
        // deleting nor reshaping one passes while every other cell stays green.
        let witnesses: [(&str, ParseExpect, ParseRole); 9] = [
            (
                "X01",
                ParseExpect::IdLess,
                ParseRole::GateWitness(GateHalf::Json),
            ),
            ("X15", ParseExpect::IdLess, ParseRole::Recoded),
            (
                "X03",
                ParseExpect::IdLess,
                ParseRole::GateWitness(GateHalf::Utf8),
            ),
            ("X10", ParseExpect::InvalidRequest, ParseRole::ScanWitness),
            ("E15", ParseExpect::RecoveredStr("a"), ParseRole::InClass),
            ("E16", ParseExpect::RecoveredNum(54016), ParseRole::InClass),
            ("E17", ParseExpect::RecoveredNum(54017), ParseRole::InClass),
            ("E18", ParseExpect::RecoveredNum(54018), ParseRole::InClass),
            ("E24", ParseExpect::RecoveredNum(54124), ParseRole::InClass),
        ];
        for (id, expect, role) in witnesses {
            assert!(
                corpus
                    .iter()
                    .any(|f| f.id == id && f.expect == expect && f.role == role),
                "{id}: a row-unique witness must stay in the corpus with its declared reply and role"
            );
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
    // [ub-nbz / ub-zja] CANCELLATION CELLS — `receive()` dropped mid-poll, exactly as rmcp's
    // serve-loop `select!` drops a losing arm.
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
        let corpus = id_corpus();
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
                "-32700 on the id of a duplicated id",
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
        let corpus = id_corpus();
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
            "the -32700 on the id then the id-less -32700, each once, in arrival order"
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

    /// **ub-zja, inverted by D59 clause (7)** — an UNTERMINATED final line whose bytes were consumed
    /// by a DROPPED `receive()` is DROPPED at EOF, as rmcp 3.10 drops it
    /// (`rmcp-3.10.0/src/transport/async_rw.rs:290-292`): nothing is delivered and nothing written.
    ///
    /// Mutant: the EOF arm processing a non-empty buffer as a line (the pre-D59 shape).
    #[tokio::test]
    async fn an_unterminated_line_split_by_a_dropped_receive_is_dropped_at_eof() {
        let (mut in_w, in_r) = tokio::io::duplex(1024 * 1024);
        let (out_w, mut out_r) = tokio::io::duplex(1024 * 1024);
        in_w.write_all(PING_7)
            .await
            .expect("write the unterminated frame");

        let mut transport = DupScanningTransport::new(in_r, out_w);
        poll_once_and_drop(&mut transport).await; // consumed the whole frame, no `\n` yet

        in_w.shutdown().await.expect("EOF with no newline");
        let delivered = transport.receive().await;
        assert!(
            delivered.is_none(),
            "an unterminated final line is dropped at EOF: {delivered:?}"
        );
        assert!(transport.receive().await.is_none(), "and EOF stays EOF");
        let _ = transport.close().await;
        drop(transport);
        assert!(
            read_to_end(&mut out_r).await.is_empty(),
            "nothing is written for the dropped line"
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

    /// A `receive()` re-called after a READ ERROR RESUMES the half-read line, exactly as rmcp 3.10's
    /// `AsyncRwTransport` does (`LineRead::Io` keeps its buffer, `async_rw.rs:142` and `:293-296`):
    /// same result, same bytes written (D59 clause (7), inverted from the rmcp 1.7 discard).
    ///
    /// Mutant: the `Err` arm marks the line complete, so the re-call discards the head and answers
    /// the tail alone.
    #[tokio::test]
    async fn a_read_error_resumes_the_partial_line_like_rmcp() {
        let rmcp = recall_after_read_error(AsyncRwTransport::new_server).await;
        let ours = recall_after_read_error(DupScanningTransport::new).await;
        assert!(
            rmcp.0.as_deref().is_some_and(|m| m.contains(r#""id":1"#)),
            "non-vacuity: rmcp 3.10 resumes the half-read ping: {rmcp:?}"
        );
        assert_eq!(
            (ours.0, String::from_utf8_lossy(&ours.1)),
            (rmcp.0, String::from_utf8_lossy(&rmcp.1)),
            "after a read error the re-call must match rmcp (result, bytes written)"
        );
    }
}
