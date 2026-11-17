//! **The envelope-`id` corpus over a LIVE duplex** (first minted for D47; re-classified by PRD §4
//! D59 clauses (2) and (3)).
//!
//! The in-module transport cells (`src/wire.rs`) already pin the exact reply BYTES. What they
//! cannot show is that the parse-error arm's three rules are actually INSTALLED on the path
//! `run_mcp_server` uses, and — the part no channel assertion reaches — that a frame failing the
//! typed parse while naming a destructive action EXECUTES NOTHING. Since D59 a duplicated EQUAL id
//! draws rule (β), `-32700` on that id (D47 answered it `-32600`), and an unreadable id draws rule
//! (γ), rmcp's id-less `-32600`.
//!
//! # Why the effect oracle is the point of this file
//!
//! The most plausible wrong implementation of this decision is not "forget to reply". It is
//! "recover the id, REBUILD the frame as a `JsonRpcRequest`, and deliver it" — the fix-it-up
//! temptation. That implementation answers on the right id, writes plausible bytes, recovers the
//! connection, and passes every single channel-only assertion in this suite and in `wire.rs`. The
//! only thing that distinguishes it is that the tool RAN. Hence `store_fingerprint` before and
//! after, shared with `duplicate_key_duplex.rs` through `tests/common` so the two cannot drift.
//!
//! # Why every frame is hand-written
//!
//! An rmcp client serialises through `serde_json`, which builds a `Map` first and therefore cannot
//! emit a duplicated key, a `null` id or an out-of-`i64` number. `raw_tools_call` is no help either:
//! its signature takes one well-formed `i64` id. Every frame here is a whole envelope, written
//! verbatim, taken from the shared corpus.
//!
//! # Observation, without a single sleep
//!
//! A reply whose id is OMITTED is invisible to any id-correlating reader, so the ambiguous arm is
//! observed with the shipped SENTINEL FOLLOW: send the frame, send a known-good request, read the
//! sentinel, then inspect `seen_lines`. No sleeps, no threads. The only timer is
//! `read_response`'s overall deadline (ub-f1k), which the passing path never waits on.

mod common;

use unblock_mcp::Quotas;
use unblock_mcp::envelope_id_corpus::{ISSUE_ID_PLACEHOLDER, id_corpus};

/// Fetch one envelope-`id` corpus frame as text.
fn frame_text(entry: &str) -> String {
    let frame = id_corpus()
        .into_iter()
        .find(|f| f.id == entry)
        .unwrap_or_else(|| panic!("corpus entry {entry} is missing"))
        .frame;
    String::from_utf8(frame).expect("corpus frames are UTF-8")
}

/// **C-P1** — a `tools/call` with a duplicated EQUAL id naming a DESTRUCTIVE action is answered
/// `-32700` on that id (rule (β)), and executes NOTHING.
///
/// This is the ONLY cell that kills the "rebuild it as a Request on the recovered id and deliver
/// it" implementation. That implementation is answered-correctly and byte-plausible at every other
/// assertion in the suite; what gives it away is the issue no longer being there.
///
/// D16's frame names `issue delete` against a live, freshly minted id.
#[tokio::test]
async fn an_undecodable_id_tools_call_answers_and_executes_nothing() {
    let session = common::session().await;
    let (mut client, _server, _cancel) = common::connect_raw(session, Quotas::default()).await;

    let target = common::create_issue(&mut client, "D59 store-effect target").await;
    let before = common::store_fingerprint(&mut client, std::slice::from_ref(&target)).await;

    // Point the corpus frame at the live id. The `arguments` are otherwise verbatim.
    let raw = frame_text("D16").replace(ISSUE_ID_PLACEHOLDER, &target);
    assert!(
        raw.contains(&target) && raw.contains("delete"),
        "non-vacuity: the frame must really name a DESTRUCTIVE action against the live id: {raw}"
    );

    // The id is recoverable (both occurrences are 90016), so the answer rides it and
    // `read_response` can correlate on it. A regression that breaks correlation (e.g. a
    // stringified id) trips `read_response`'s bounded deadline (ub-f1k) instead of hanging.
    let response = client.request_raw(90016, &raw).await;
    assert_eq!(
        response["error"]["code"], -32700,
        "a duplicated equal id is answered -32700 on that id (rule (β), D59 clause (2)): {response}"
    );
    assert!(
        response.get("result").is_none(),
        "an out-of-band error carries no result: {response}"
    );

    let after = common::store_fingerprint(&mut client, std::slice::from_ref(&target)).await;
    assert_eq!(
        before, after,
        "THE EFFECT ORACLE FAILED — the frame was answered but the store MOVED, which is what a \
         `rebuild it as a Request and deliver it` implementation does. The frame must be answered \
         AND DROPPED, never executed."
    );
}

/// **C-P2** — rule (γ) is INSTALLED on the real serve path: the ambiguous frame is answered with
/// rmcp's id-less `-32600` while neither of its two candidate ids is ever answered.
///
/// Observed by SENTINEL FOLLOW over `seen_lines`, because a reply with no id is invisible to an
/// id-correlating reader.
#[tokio::test]
async fn the_arm_is_installed_on_the_real_serve_path() {
    let session = common::session().await;
    let (mut client, _server, _cancel) = common::connect_raw(session, Quotas::default()).await;

    // D04 carries two DIFFERING ids (90004 / 90005), so the reply is rule (γ)'s id-less -32600.
    client.write_raw_line(&frame_text("D04")).await;

    // The sentinel is a known-good request; reading its response drains everything the server wrote
    // before it, into `seen_lines`.
    let sentinel = client.next_request_id();
    let frame = format!(r#"{{"jsonrpc":"2.0","id":{sentinel},"method":"ping","params":{{}}}}"#);
    let answer = client.request_raw(sentinel, &frame).await;
    assert!(
        answer.get("result").is_some(),
        "the sentinel must be answered normally — the connection survives an out-of-band reply"
    );

    assert!(
        client.saw_line_containing(
            r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid request"}}"#
        ),
        "the ambiguous frame must still be ANSWERED with rmcp's id-less -32600: {:?}",
        client.seen_lines
    );
    assert!(
        !client.saw_response_for(90_004) && !client.saw_response_for(90_005),
        "neither candidate id may be answered on — the bytes are ambiguous, so the id is OMITTED"
    );
}

/// **D59 criterion (e)** — a `tools/call` `issue create` with `"id":1.0`, and again with `"id":1e2`,
/// each creates exactly one issue and is answered on `1` and `100`; with `"id":1.5` or
/// `"id":9007199254740992.0` it creates nothing and gets rule (γ)'s id-less `-32600`.
/// Mutant: a fork that refuses float-form ids (nothing created) or that answers them id-less.
#[tokio::test]
async fn float_ids_run_and_unreadable_floats_get_rule_gamma() {
    let session = common::session().await;
    let (mut client, _server, _cancel) = common::connect_raw(session, Quotas::default()).await;
    let create = |id: &str, title: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"issue","arguments":{{"action":"create","title":"{title}"}}}}}}"#
        )
    };
    let one = client.request_raw(1, &create("1.0", "float-one")).await;
    assert_ne!(one["result"]["isError"], true, "{one}");
    let hundred = client
        .request_raw(100, &create("1e2", "float-hundred"))
        .await;
    assert_ne!(hundred["result"]["isError"], true, "{hundred}");

    for id in ["1.5", "9007199254740992.0"] {
        client.write_raw_line(&create(id, "never")).await;
    }
    let sentinel = client.next_request_id();
    let pong = client
        .request_raw(
            sentinel,
            &format!(r#"{{"jsonrpc":"2.0","id":{sentinel},"method":"ping"}}"#),
        )
        .await;
    assert!(pong.get("result").is_some());
    let gamma = client
        .seen_lines
        .iter()
        .filter(|l| {
            l.as_str() == r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid request"}}"#
        })
        .count();
    assert_eq!(
        gamma, 2,
        "both unreadable floats get rule (γ): {:?}",
        client.seen_lines
    );
    let titles = common::store_titles(&mut client).await;
    for title in ["float-one", "float-hundred"] {
        assert_eq!(
            titles.iter().filter(|t| *t == title).count(),
            1,
            "EXACTLY one issue per float id ({title}): {titles:?}"
        );
    }
    assert!(!titles.contains(&"never".to_string()), "{titles:?}");
}

/// **D59 criterion (e)** — a `notifications/cancelled` carrying `"id":7` is a REQUEST on rmcp 3.10: it
/// gets `-32601` on id 7 and cancels nothing — an in-flight request it names, held inside its storage
/// write and released after that `-32601` arrives, still gets its response.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_notification_with_an_id_is_a_request_and_cancels_nothing() {
    let (session, gate) = common::session_create_gated().await;
    let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_io);
    tokio::spawn(async move {
        // A started lifecycle returns the RunningService; keep it alive (its drop cancels the server).
        if let Ok(running) = unblock_mcp::mcp_server_duplex_for_test(
            session,
            Quotas::default(),
            None,
            server_read,
            server_write,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        {
            let _ = running.waiting().await;
        }
    });
    let mut client = common::raw_client(client_io);
    client.initialize().await;
    client
        .write_raw_line(r#"{"jsonrpc":"2.0","id":55,"method":"tools/call","params":{"name":"issue","arguments":{"action":"create","title":"held"}}}"#)
        .await;
    gate.await_all_entered(1).await;
    let refused = client
        .request_raw(7, r#"{"jsonrpc":"2.0","id":7,"method":"notifications/cancelled","params":{"requestId":55}}"#)
        .await;
    assert_eq!(refused["error"]["code"], -32601, "{refused}");
    gate.release().await;
    let held = client.read_response(55).await;
    assert_ne!(
        held["result"]["isError"], true,
        "the named request still gets its response: {held}"
    );
}
