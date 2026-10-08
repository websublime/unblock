//! **[ub-f1k] HARNESS SELF-TESTS for `RawDuplexClient` — a guard nobody drives can be deleted.**
//!
//! `read_response`'s overall deadline is HARNESS, not SUT, so nothing in the product turns red when
//! it is reverted. This file drives it directly, the twin of the CLI harness's H10
//! (`crates/unblock-cli/tests/mcp_stdout_channel.rs`). Its own file because every other duplex
//! suite here is scoped to a server decision, and this cell has no server at all: a FABRICATED peer
//! sits on the far end of a `tokio::io::duplex`.
//!
//! # Virtual time
//!
//! `start_paused = true` runs the cell on tokio's paused clock, so the 30-second deadline costs no
//! wall-clock time and host load cannot move it. The pause is per-test: no other binary sharing
//! `tests/common` runs on it.

mod common;

use std::time::Duration;

use common::{QUOTED_LINE_LIMIT, READ_RESPONSE_DEADLINE};
use tokio::io::AsyncWriteExt as _;

/// The reply the PRIOR `read_response` call correlates, so the deadline panic can be checked to
/// quote only the lines its own call read.
const PRIOR_REPLY: &str = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;

/// A valid JSON-RPC error with NO id, the reply shape a server gives a request it cannot correlate.
const ID_LESS_FRAME: &str =
    r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"premature request"}}"#;

/// A reply to id 2 whose id is the STRING `"2"`: the wrong-typed echo an id-correlating reader must
/// never accept as id 2.
const STRINGIFIED_ID_FRAME: &str = r#"{"jsonrpc":"2.0","id":"2","result":{}}"#;

/// How often the peer writes. Shorter than [`READ_RESPONSE_DEADLINE`], so EVERY single read
/// completes in time and only a deadline spanning ALL reads can end the call.
const TICK: Duration = Duration::from_secs(7);

/// **H10 parity** — `read_response` fails at its OVERALL deadline while uncorrelatable replies
/// keep arriving, and its panic quotes only this call's lines, each clipped.
///
/// The peer answers id 1, writes one over-long id-less frame, then writes an id-less frame and a
/// stringified-id frame every [`TICK`] with the pipe left open. No read ever waits a full
/// [`READ_RESPONSE_DEADLINE`], so a per-read timeout would never fire, and with no timeout the call
/// never ends: the outer `timeout` turns both regressions into a red instead of a hang.
///
/// Mutants: the deadline per read (`timeout` around each `read_line`) or removed — the outer
/// `timeout` fires; the panic quoting all of `seen_lines` — the prior reply appears; the clip
/// dropped — the full long frame appears; the clip slicing at a raw byte index — a char-boundary
/// panic replaces the deadline message.
#[tokio::test(start_paused = true)]
async fn the_read_deadline_spans_every_read_and_quotes_only_this_call() {
    let long_frame = format!(
        r#"{{"jsonrpc":"2.0","error":{{"code":-32600,"message":"{}"}}}}"#,
        "é".repeat(QUOTED_LINE_LIMIT)
    );
    assert!(
        long_frame.len() > QUOTED_LINE_LIMIT && !long_frame.is_char_boundary(QUOTED_LINE_LIMIT),
        "fixture: the long frame must be clipped, mid-character"
    );

    let (client_io, mut peer_io) = tokio::io::duplex(64 * 1024);
    let opening = format!("{PRIOR_REPLY}\n{long_frame}\n");
    tokio::spawn(async move {
        if peer_io.write_all(opening.as_bytes()).await.is_err() {
            return;
        }
        let tick = format!("{ID_LESS_FRAME}\n{STRINGIFIED_ID_FRAME}\n");
        // A write error means the client is gone; there is nobody left to feed.
        while peer_io.write_all(tick.as_bytes()).await.is_ok() {
            tokio::time::sleep(TICK).await;
        }
    });

    let mut client = common::raw_client(client_io);
    let prior = client.read_response(1).await;
    assert_eq!(prior["id"], 1, "the prior call must correlate id 1");

    let start = tokio::time::Instant::now();
    let reader = tokio::spawn(async move { client.read_response(2).await });
    let joined = tokio::time::timeout(READ_RESPONSE_DEADLINE * 4, reader)
        .await
        .expect(
            "read_response must end on its OVERALL deadline; still reading at four deadlines \
             means the deadline is per read or gone",
        );
    let waited = start.elapsed();
    let error = joined.expect_err("a peer that never answers id=2 must make read_response PANIC");
    assert!(
        error.is_panic(),
        "the read task must panic, not be cancelled"
    );
    let message = panic_message(&error.into_panic());

    assert!(
        message.contains("no response correlated to id=2"),
        "the deadline must be the thing that fired, not EOF or the framing guard: {message}"
    );
    assert!(
        waited >= READ_RESPONSE_DEADLINE,
        "the deadline must not fire early: it fired after {waited:?}"
    );
    assert!(
        message.contains(ID_LESS_FRAME) && message.contains(STRINGIFIED_ID_FRAME),
        "the call must read past both uncorrelatable shapes before the deadline: {message}"
    );
    assert!(
        !message.contains(PRIOR_REPLY),
        "the panic must quote only the lines THIS call read, not the prior call's: {message}"
    );
    // The long frame is followed by further quoted lines, so the clipped text is pinned at BOTH
    // ends: a clip past the limit (e.g. `ceil_char_boundary`) breaks the trailing `\n`.
    let clipped = &long_frame[..long_frame.floor_char_boundary(QUOTED_LINE_LIMIT)];
    assert!(
        !message.contains(&long_frame) && message.contains(&format!("\n  {clipped}\n")),
        "a quoted line must be clipped to {QUOTED_LINE_LIMIT} bytes on a char boundary: {message}"
    );
}

/// Extract a panic payload's message (`&str` or `String`), so the cell can assert WHICH panic fired
/// rather than merely that something did.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload.downcast_ref::<&str>().map_or_else(
        || {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "<non-string panic payload>".to_string())
        },
        |s| (*s).to_string(),
    )
}
