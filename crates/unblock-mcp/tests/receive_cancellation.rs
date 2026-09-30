//! **[ub-nbz / ub-zja] `receive()` CANCELLATION over the REAL serve path.**
//!
//! rmcp polls the transport's `receive()` as one arm of an UNBIASED `tokio::select!`
//! (`rmcp-1.7.0/src/service.rs:805`, the arm at `:813`) and DROPS it whenever another arm — a handler
//! response, most often — wins. The in-module cells in `src/wire.rs` pin each consequence at one
//! deterministic interleaving. These cells pin the same two properties through the INSTALLED stack
//! (`VersionClampingTransport(PreHandshakeGateTransport(DupScanningTransport))` via
//! `mcp_server_duplex_for_test`), in the regime that loses them in production: fast handlers in
//! flight and a promptly draining client.
//!
//! # Observation, without a single timeout
//!
//! The shipped SENTINEL FOLLOW: one burst ending in a known-good sentinel request, read up to the
//! sentinel's response, then inspect `seen_lines`. The PASS side is not a race. The transport writes
//! a parked reply to completion before it reads the next frame, so the reply is on the wire before
//! the sentinel is even read — ordering by construction, never by timing.

mod common;

use std::fmt::Write as _;

use unblock_mcp::Quotas;
use unblock_mcp::envelope_id_corpus::divergence_corpus;

/// Pings in flight ahead of each bad frame. Eight is the deepest in-flight depth the reply loss was
/// measured at; any depth that keeps a response `send()` holding the write lock would do.
const IN_FLIGHT: usize = 8;

/// A server→client pipe this small makes every response write span several polls.
const SMALL_OUTBOUND: usize = 32;

/// **ub-nbz cell (ii)** — an out-of-band reply survives a `receive()` dropped by the serve loop,
/// on every arm, with pings in flight over a small outbound pipe. Three rounds per arm on ONE
/// connection, so the cell also proves the connection keeps serving after each reply.
///
/// Mutants: the reply awaited inline inside `receive()` again (the pre-fix shape), which fails
/// first on a torn frame (the "every server line must be JSON-RPC framing" panic in
/// `tests/common/mod.rs`) before the lost-reply assertion.
#[tokio::test(flavor = "current_thread")]
async fn an_out_of_band_reply_survives_pings_in_flight() {
    let frame = |id: &str| {
        let entry = divergence_corpus()
            .into_iter()
            .find(|f| f.id == id)
            .unwrap_or_else(|| panic!("{id} missing"));
        String::from_utf8(entry.frame).expect("corpus frames are UTF-8")
    };
    let (recovered, omitted) = (frame("D01"), frame("D04"));
    let arms: [(&str, &str, &str); 3] = [
        (
            "-32600 recovered id",
            recovered.as_str(),
            r#""id":90001,"error":{"code":-32600"#,
        ),
        (
            "-32600 id omitted",
            omitted.as_str(),
            r#"{"jsonrpc":"2.0","error":{"code":-32600"#,
        ),
        ("-32700", "this is not json", r#""error":{"code":-32700"#),
    ];

    let (mut client, _server, _cancel) = common::connect_raw_with_outbound_capacity(
        common::session().await,
        Quotas::default(),
        SMALL_OUTBOUND,
    )
    .await;
    let mut lost = Vec::new();
    for (arm, frame, reply) in arms {
        for round in 0..3 {
            let mut burst = String::new();
            for _ in 0..IN_FLIGHT {
                let id = client.next_request_id();
                writeln!(burst, r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#)
                    .expect("write to a String");
            }
            burst.push_str(frame);
            burst.push('\n');
            let sentinel = client.next_request_id();
            write!(
                burst,
                r#"{{"jsonrpc":"2.0","id":{sentinel},"method":"ping"}}"#
            )
            .expect("write to a String");
            client.seen_lines.clear();
            client.write_raw_line(&burst).await;
            client.read_response(sentinel).await;
            if !client.saw_line_containing(reply) {
                lost.push(format!("{arm}, round {round}"));
            }
        }
    }
    assert!(
        lost.is_empty(),
        "every out-of-band reply must reach the client ahead of the sentinel's response; lost: {lost:?}"
    );
}

/// **ub-zja** — a WELL-FORMED request larger than one 8 KiB `BufReader` fill, arriving in two writes
/// while ping responses land, is served whole. Pre-fix, the `receive()` pending mid-line is dropped
/// by the serve loop, the loop head clears the consumed head, and the tail is answered `-32700` alone.
///
/// Mutant: `line_buf` cleared at the loop head unconditionally again.
#[tokio::test(flavor = "current_thread")]
async fn a_frame_larger_than_one_read_survives_pings_in_flight() {
    let (mut client, _server, _cancel) = common::connect_raw_with_outbound_capacity(
        common::session().await,
        Quotas::default(),
        1024 * 1024,
    )
    .await;

    let big_id = 9_000;
    let padding = "x".repeat(20 * 1024);
    let big = format!(
        r#"{{"jsonrpc":"2.0","id":{big_id},"method":"tools/list","params":{{"_meta":{{"pad":"{padding}"}}}}}}"#
    );
    let (head, tail) = big.split_at(12 * 1024);

    // Burst 1: pings, then the HEAD of the big frame. Reading every ping's response guarantees the
    // server read the head and pended mid-line while those responses were being written.
    let mut burst = String::new();
    let mut last_ping = 0;
    for _ in 0..IN_FLIGHT {
        last_ping = client.next_request_id();
        writeln!(
            burst,
            r#"{{"jsonrpc":"2.0","id":{last_ping},"method":"ping"}}"#
        )
        .expect("write to a String");
    }
    burst.push_str(head);
    client.write_raw_bytes(burst.as_bytes()).await;
    client.read_response(last_ping).await;

    // Burst 2: the TAIL, then a sentinel.
    let sentinel = client.next_request_id();
    client
        .write_raw_line(&format!(
            "{tail}\n{{\"jsonrpc\":\"2.0\",\"id\":{sentinel},\"method\":\"ping\"}}"
        ))
        .await;
    client.read_response(sentinel).await;

    assert!(
        client.saw_response_for(big_id),
        "the split request must be served whole; lines seen: {:?}",
        client
            .seen_lines
            .iter()
            .map(|l| l.chars().take(120).collect::<String>())
            .collect::<Vec<_>>()
    );
    assert!(
        !client.saw_line_containing(r#""code":-32700"#),
        "no fragment of the split request may be answered as a parse error"
    );
}
