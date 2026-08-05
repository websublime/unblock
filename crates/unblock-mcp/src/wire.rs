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
//! carrying the verdict. For that class it emits exactly the responses `AsyncRwTransport` emits
//! today (the `-32700` parse-error reply, id omitted) and nothing else, because that class HAS an
//! in-band channel: inventing a reply for it would force the out-of-band `-32602`/`-32700` arm back
//! open for a class the binding decision says must be answered IN-BAND. Rejection for it happens at
//! exactly one site: `call_tool` (`crate::server`).
//!
//! # THE ONE CLASS THE TRANSPORT DOES ANSWER — UN-DECODABLE ENVELOPE `id` (normative, PRD §4, D47)
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
//! It then **DROPS** the frame. Dropping is load-bearing: rmcp's `expect_next_message` returns
//! `ExpectedInitializeRequest` for ANY non-Request message in the initialize slot, so DELIVERING one
//! of these pre-handshake kills the server — precisely the failure this arm removes.
//!
//! The predicate is [`crate::envelope_id::scan`], a `DeserializeSeed` over the ROOT object collecting
//! every top-level `id` member's value, guarded by an EXHAUSTIVE match on the `Notification` variant
//! so request traffic pays nothing. Keys are compared DECODED, never as raw spans.
//! [`unblock_error::dup_key::scan`] CANNOT serve as this predicate: it reports `Clean` for every
//! non-duplicated shape of the class, and its `Duplicate { key, path }` verdict retains no occurrence
//! VALUES, so equal and differing ids are indistinguishable to it.
//!
//! DISCLOSED and deliberately left open (tracked as `ub-788`): the `-32700` arm omits a readable id
//! unconditionally, so a duplicated `method`/`jsonrpc` frame still leaves an rmcp client pending.
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

/// An owned `Transport<RoleServer>` over a byte stream pair, reproducing `AsyncRwTransport`'s read
/// framing exactly and adding the D43 duplicate-key scan between the read and the parse.
pub(crate) struct DupScanningTransport<R, W> {
    /// The buffered read half. `read_until(b'\n', ..)` — there is deliberately no line-length bound
    /// here, exactly as rmcp has none (`max_length: usize::MAX`).
    read: BufReader<R>,
    /// The reusable line buffer (cleared per frame), mirroring rmcp's `line_buf`.
    line_buf: Vec<u8>,
    /// The write half. The `Arc<Mutex<Option<W>>>` shape is what satisfies `Transport::send`'s
    /// `+ Send + 'static` return bound (the future must not borrow `self`), and `Option` is what
    /// makes a post-`close()` `send` fail with `NotConnected` instead of writing to a dead pipe.
    write: Arc<Mutex<Option<W>>>,
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
        }
    }

    /// Write ONE out-of-band error reply and report whether the connection is still usable.
    ///
    /// `None` ⇒ the caller must `return None` from `receive()` — the SAME two conditions the shipped
    /// `-32700` arm returns `None` on: the write half was taken by `close()` (⇒ `NotConnected`, the
    /// D40 teardown path), or the write itself failed.
    ///
    /// **Takes the WRITE HALF, not `&self` — and that is a `Send` requirement, not a preference.**
    /// `receive()` holds `line`, an immutable borrow of `self.line_buf`, so an `&mut self` helper
    /// would conflict with it; but `&self` does not work either, because `Transport::receive`
    /// requires its future to be `Send` and `&Self` is `Send` only if `Self: Sync` — which this
    /// transport is not (`BufReader<R>` is not `Sync` for a merely-`Send` `R`). Borrowing the ONE
    /// field that is touched satisfies both: `Arc<Mutex<Option<W>>>` is `Send + Sync` for `W: Send`,
    /// and a shared borrow of `self.write` is disjoint from the shared borrow of `self.line_buf`.
    ///
    /// The guard is scoped to this function, so it is released before `receive()` parks in the next
    /// `read_until`. Holding it across that read would block every `send()` for the whole idle
    /// period.
    ///
    /// Both out-of-band arms (`-32700` and D47's `-32600`) go through here so they are identical
    /// **by construction** rather than by review, and both encode through rmcp's own
    /// [`JsonRpcMessageCodec`] — there is no hand-rolled byte path.
    async fn answer_error(
        write: &Arc<Mutex<Option<W>>>,
        error: ErrorData,
        id: Option<RequestId>,
    ) -> Option<()> {
        let mut guard = write.lock().await;
        let writer = guard.as_mut()?;
        let response = TxJsonRpcMessage::<RoleServer>::error(error, id);
        write_frame(writer, response).await.ok()
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
        let lock = self.write.clone();
        async move {
            let mut guard = lock.lock().await;
            match guard.as_mut() {
                Some(writer) => write_frame(writer, item).await,
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "Transport is closed",
                )),
            }
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            self.line_buf.clear();
            match self.read.read_until(b'\n', &mut self.line_buf).await {
                Ok(0) => return None,
                Ok(_) => {}
                Err(e) => {
                    // Nothing is swallowed here: the EOF/error shape is what the D40 pre-handshake
                    // teardown depends on.
                    tracing::error!("Error reading from stream: {}", e);
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
                                Self::answer_error(
                                    &self.write,
                                    ErrorData::invalid_request(INVALID_REQUEST_ID_MESSAGE, None),
                                    Some(id),
                                )
                                .await?;
                                continue; // ANSWER AND DROP — never delivered.
                            }
                            EnvelopeId::Unusable => {
                                tracing::debug!(
                                    "un-decodable envelope id, unrecoverable; answering -32600 with the id omitted"
                                );
                                Self::answer_error(
                                    &self.write,
                                    ErrorData::invalid_request(INVALID_REQUEST_ID_MESSAGE, None),
                                    None,
                                )
                                .await?;
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
                Err(e) => {
                    tracing::debug!("Parse error on incoming message: {e}");
                    Self::answer_error(
                        &self.write,
                        ErrorData::parse_error("Parse error", None),
                        None,
                    )
                    .await?;
                    // Recover: loop to the next line. This deliberately does NOT return.
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        let mut guard = self.write.lock().await;
        drop(guard.take());
        Ok(())
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
/// `Ok(Some(_))` = deliver, `Ok(None)` = silently ignore, `Err(_)` = answer `-32700` and recover.
///
/// The BOM is stripped ONCE, prefix only, and `line` is REBOUND to the stripped slice **before**
/// both the primary parse and the compat re-parse — so a BOM-prefixed unknown notification is
/// ignored exactly like an un-prefixed one. Non-UTF-8 input skips the compat branch entirely.
///
/// The error type is narrowed to `serde_json::Error`: on the read path rmcp's codec error can only
/// ever be its `Serde` variant (the length-bounded and I/O variants belong to the `Decoder` path,
/// which `receive()` does not use).
fn try_parse_with_compatibility<T: serde::de::DeserializeOwned>(
    line: &[u8],
) -> Result<Option<T>, serde_json::Error> {
    let line = line.strip_prefix(UTF8_BOM.as_slice()).unwrap_or(line);
    if let Ok(line_str) = std::str::from_utf8(line) {
        match serde_json::from_slice(line) {
            Ok(item) => Ok(Some(item)),
            Err(e) => {
                if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(line_str)
                    && let Some(method) =
                        json_value.get("method").and_then(serde_json::Value::as_str)
                    && should_ignore_notification(&json_value, method)
                {
                    return Ok(None);
                }
                tracing::debug!("Failed to parse message receive: {line_str} | Error: {e}");
                Err(e)
            }
        }
    } else {
        serde_json::from_slice(line).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::{DupScanningTransport, ParamsScan};
    use crate::envelope_id_corpus::{Expect, divergence_corpus, expected_bytes};
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
        // F10 — non-UTF-8 bytes: -32700 + recovery.
        corpus.push(("F10", vec![b'{', 0xff, 0xfe, b'}']));
        // F11 — depth-130 nesting: past serde_json's 128-level limit for BOTH parsers => -32700.
        let mut deep = String::from(r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":"#);
        deep.push_str(&"[".repeat(130));
        deep.push_str(&"]".repeat(130));
        deep.push('}');
        corpus.push(("F11", deep.into_bytes()));
        // NS2 — a duplicated envelope `params` KEY on a `tools/call`: a hard -32700 for both
        // parsers. The outcome is METHOD-DEPENDENT and this entry pins only the `tools/call` half:
        // the same duplication on `ping` — a request with no `params` at all — is a plain SUCCESS
        // (PRD section 4 D47, spine section 5.6). This entry is graded ONLY by the whole-stream
        // equality below, never on its own -32700; do not read it as a per-frame assertion.
        corpus.push((
            "NS2",
            br#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"issue"},"params":{"name":"claim"}}"#
                .to_vec(),
        ));
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
        // F20 — a duplicated `method`. Still a `-32700` with the id OMITTED, on both transports:
        // this pins that D47 did NOT close the disclosed residual (`ub-788`), and dies if the
        // recovered-id logic is extended to the `Err` arm.
        corpus.push((
            "F20",
            br#"{"jsonrpc":"2.0","id":20,"method":"tools/call","method":"ping","params":{}}"#
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
    /// with the id OMITTED, and recovery on the next line). This differential pin, TOGETHER WITH
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
        fn parse(line: &[u8]) -> Result<Option<RxJsonRpcMessage<RoleServer>>, serde_json::Error> {
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
        // TWO THINGS THIS DOES NOT SAY, because an earlier wording claimed both and neither is true
        // (PRD section 4, D47):
        // 1. It does NOT say the client stops waiting. Our -32700 omits the id, exactly as rmcp
        //    does, and an rmcp client DROPS an id-less error while awaiting untimed — so the reply
        //    is a diagnostic on the connection, not a resolution of the pending request. That is a
        //    DISCLOSED residual (D47 clause 8), deliberately left open and tracked as `ub-788`.
        // 2. It is not a transport-wide invariant. A frame whose raw bytes carry a top-level `id`
        //    that FAILS to decode never reaches this arm at all: rmcp's untagged union falls
        //    through to the Notification variant. That class is answered -32600 on the recovered id
        //    and dropped, by the arm D47 adds — not by anything here.
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
        /// Must DIVERGE from rmcp in exactly the declared way.
        Divergence(Expect),
    }

    /// The parity tier and the divergence tier as ONE labelled list.
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
    /// Mutant: `continue` replaced by `return Some(message)` (answer AND deliver) — the shape that
    /// kills the server in rmcp's initialize slot.
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

    /// **W-R1** — the DISCLOSED residual: the `-32700` arm still omits a readable id.
    ///
    /// A duplicated `method` hard-fails the parse, so it never reaches the D47 arm at all. The
    /// reply carries a diagnostic but no id, and an rmcp client DROPS an id-less error while
    /// awaiting untimed — so that client stays pending.
    ///
    /// **This residual is OPEN by decision, not by oversight**, and is tracked as its own issue
    /// `ub-788`. Closing it is a DELIBERATE future change that will turn this cell RED; that is the
    /// cell working, not breaking. It exists so the residual stays a measured fact rather than
    /// prose.
    ///
    /// Mutant: extending the recovered-id logic to the `Err` arm.
    #[tokio::test]
    async fn the_duplicated_method_residual_is_still_id_less() {
        let (_received, written) =
            run_ours(br#"{"jsonrpc":"2.0","id":6,"method":"ping","method":"ping","params":{}}"#)
                .await;
        assert_eq!(
            String::from_utf8_lossy(&written),
            "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n",
            "the -32700 arm must be byte-unchanged by D47: code -32700, and NO id"
        );
    }

    /// **W-G2** — the per-entry differential over the FULL corpus, both tiers.
    ///
    /// The whole-stream cell above keeps its own job (F2's CRLF, F5's blank line and F7's
    /// unterminated final line are STREAM properties a per-entry harness destroys, which is why
    /// that cell must not be deleted as redundant). This one grades each entry independently.
    #[tokio::test]
    async fn the_per_entry_differential_holds_for_both_tiers() {
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
    /// that migrates an entry between tiers.
    #[tokio::test]
    async fn the_diverging_entries_are_exactly_the_declared_ones() {
        use std::collections::BTreeSet;

        let mut observed: BTreeSet<String> = BTreeSet::new();
        let mut declared: BTreeSet<String> = BTreeSet::new();
        let mut all_our_written = String::new();

        for (label, frame, tier) in full_corpus() {
            if matches!(tier, Tier::Divergence(_)) {
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
        // That one pins the -32700 arm's own omission and stays where it is; this one covers the
        // arm D47 adds, which the shipped guard never sees.
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
}
