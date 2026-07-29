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
//! # THE TRANSPORT NEVER REPLIES AND NEVER SHORT-CIRCUITS (normative)
//!
//! On a duplicate or an indeterminate scan it **still parses and still delivers** the message,
//! carrying the verdict. It emits exactly the responses `AsyncRwTransport` emits today (the
//! `-32700` parse-error reply, id omitted) and nothing else. The transport has no request id and no
//! in-band channel; inventing a reply here would force the out-of-band `-32602`/`-32700` arm back
//! open for a class the binding decision says must be answered IN-BAND. Rejection happens at
//! exactly one site: `call_tool` (`crate::server`).
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

use rmcp::model::ErrorData;
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use rmcp::transport::async_rw::JsonRpcMessageCodec;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::Encoder;
use unblock_error::dup_key::{DupScan, scan};

/// The scan root: the WHOLE `params` value of every decoded request — the reserved `_meta` member
/// included, NOT `params.arguments` alone.
///
/// `_meta` is attacker-controlled, is measured by the request quota, and reaches `call_tool` as
/// `context.meta`, so it has exactly the same in-band channel `arguments` does; excluding it would
/// leave one nested-duplicate class executing.
const SCAN_ROOT: &[&str] = &["params"];

/// UTF-8 byte order mark — RFC 8259 §8.1. Stripped exactly once, prefix only, mirroring rmcp.
const UTF8_BOM: &[u8; 3] = b"\xEF\xBB\xBF";

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
                    message.insert_extension(verdict);
                    return Some(message);
                }
                // Compat-ignored (an unknown client notification): emit nothing, answer nothing,
                // read the next line. Spelled as a fall-through rather than `continue` only
                // because it is the last arm; the semantics mirror rmcp's `continue` exactly.
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!("Parse error on incoming message: {e}");
                    let mut guard = self.write.lock().await;
                    let writer = guard.as_mut()?;
                    let response = TxJsonRpcMessage::<RoleServer>::error(
                        ErrorData::parse_error("Parse error", None),
                        None,
                    );
                    if write_frame(writer, response).await.is_err() {
                        return None;
                    }
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
    use rmcp::model::{GetExtensions, JsonRpcMessage};
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
    fn framing_corpus() -> Vec<Vec<u8>> {
        let mut corpus: Vec<Vec<u8>> = Vec::new();
        // F1 — BOM-prefixed CLEAN frame.
        let mut bom_clean = b"\xEF\xBB\xBF".to_vec();
        bom_clean.extend_from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#);
        corpus.push(bom_clean);
        // F2 — CRLF-terminated clean frame (the terminator is added by the writer below).
        corpus.push(br#"{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}"#.to_vec());
        // F3 — BOM-prefixed DUPLICATE frame.
        let mut bom_dup = b"\xEF\xBB\xBF".to_vec();
        bom_dup.extend_from_slice(
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"issue","arguments":{"action":"create","action":"delete"}}}"#,
        );
        corpus.push(bom_dup);
        // F4 — a padded duplicate whose second occurrence sits past the pad.
        let pad = "x".repeat(100 * 1024);
        corpus.push(
            format!(
                r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"issue","arguments":{{"pad":"{pad}","action":"create","action":"delete"}}}}}}"#
            )
            .into_bytes(),
        );
        // F5 — a blank line (skipped, no message, no reply).
        corpus.push(Vec::new());
        // F6 — a whitespace-only line: NOT empty, so it parses and fails => -32700 + recovery.
        corpus.push(b"   ".to_vec());
        // F8 — an unknown notification with a WELL-FORMED `params` object. It does NOT reach the
        // compatibility filter: rmcp's catch-all `CustomNotification` types it, so the frame is
        // DELIVERED (and the filter only runs on a typed-parse failure).
        corpus.push(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#.to_vec());
        // F12 — the same frame BOM-prefixed: delivered identically to F8, which is what pins the
        // BOM strip as happening before the typed parse.
        let mut bom_note = b"\xEF\xBB\xBF".to_vec();
        bom_note
            .extend_from_slice(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#);
        corpus.push(bom_note);
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
        corpus.push(br#"{"jsonrpc":"2.0","method":"notifications/foo","params":5}"#.to_vec());
        // F15 — LSP-style traffic (`$/cancelRequest`), same scalar `params`. Ignored ONLY by the
        // filter's first arm (no `id` + a non-standard method); its method does not start with
        // `notifications/`, so the second arm would let it through as a -32700.
        corpus.push(br#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":5}"#.to_vec());
        // F16 — the OVER-ignoring direction: a STANDARD notification with unusable `params` must
        // still be a -32700, not a silent drop. Both arms must decline it.
        corpus.push(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":5}"#.to_vec());
        // F17 — ignored ONLY by the filter's SECOND arm: it carries an `id`, so the first arm
        // declines it (it is not a notification), and only the `notifications/*`-prefix arm can
        // ignore it. rmcp ignores it, so we must too. WITHOUT this entry arm 2 is dead code under
        // test — replacing its whole `matches!` with `false` leaves the suite green while the fork
        // silently answers -32700 to a frame rmcp drops.
        corpus
            .push(br#"{"jsonrpc":"2.0","id":17,"method":"notifications/foo","params":5}"#.to_vec());
        // F9 — an unknown method WITH an id: delivered (the handler answers -32601).
        corpus.push(br#"{"jsonrpc":"2.0","id":9,"method":"nope/nope","params":{}}"#.to_vec());
        // F10 — non-UTF-8 bytes: -32700 + recovery.
        corpus.push(vec![b'{', 0xff, 0xfe, b'}']);
        // F11 — depth-130 nesting: past serde_json's 128-level limit for BOTH parsers => -32700.
        let mut deep = String::from(r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":"#);
        deep.push_str(&"[".repeat(130));
        deep.push_str(&"]".repeat(130));
        deep.push('}');
        corpus.push(deep.into_bytes());
        // NS2 — a duplicated envelope `params` KEY: a hard -32700 for both parsers.
        corpus.push(
            br#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"issue"},"params":{"name":"claim"}}"#
                .to_vec(),
        );
        // A trailing UNTERMINATED line at EOF (F7) — the writer omits the final newline for the
        // LAST entry, so this one exercises it.
        corpus.push(br#"{"jsonrpc":"2.0","id":7,"method":"ping","params":{}}"#.to_vec());
        corpus
    }

    /// Serialize the corpus into one byte stream: `\n` after every line except the last (F7), and
    /// CRLF after the second entry (F2).
    fn corpus_bytes(corpus: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        let last = corpus.len() - 1;
        for (index, line) in corpus.iter().enumerate() {
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
        // -32700 rather than leaving the client waiting on a response that never comes. (F17 above
        // is the deliberate exception rmcp itself defines, and only inside that prefix.)
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
