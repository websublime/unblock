//! **[v1.0.1/D47] The UN-DECODABLE-ENVELOPE-`id` class over a LIVE duplex.**
//!
//! The in-module transport cells (`src/wire.rs`) already pin the exact reply BYTES. What they
//! cannot show is that the arm is actually INSTALLED on the path `run_mcp_server` uses, and — the
//! part no channel assertion reaches — that a class frame naming a destructive action EXECUTES
//! NOTHING.
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
//! # Observation, without a single timeout
//!
//! A reply whose id is OMITTED is invisible to any id-correlating reader, so the ambiguous arm is
//! observed with the shipped SENTINEL FOLLOW: send the frame, send a known-good request, read the
//! sentinel, then inspect `seen_lines`. No sleeps, no threads, no timeouts.

mod common;

use unblock_mcp::Quotas;
use unblock_mcp::envelope_id_corpus::{ISSUE_ID_PLACEHOLDER, divergence_corpus};

/// Fetch one divergence-corpus frame as text.
fn frame_text(entry: &str) -> String {
    let frame = divergence_corpus()
        .into_iter()
        .find(|f| f.id == entry)
        .unwrap_or_else(|| panic!("corpus entry {entry} is missing"))
        .frame;
    String::from_utf8(frame).expect("corpus frames are UTF-8")
}

/// **C-P1** — a class `tools/call` naming a DESTRUCTIVE action is answered, and executes NOTHING.
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

    let target = common::create_issue(&mut client, "D47 store-effect target").await;
    let before = common::store_fingerprint(&mut client, std::slice::from_ref(&target)).await;

    // Point the corpus frame at the live id. The `arguments` are otherwise verbatim.
    let raw = frame_text("D16").replace(ISSUE_ID_PLACEHOLDER, &target);
    assert!(
        raw.contains(&target) && raw.contains("delete"),
        "non-vacuity: the frame must really name a DESTRUCTIVE action against the live id: {raw}"
    );

    // The id is recoverable (both occurrences are 90016), so the answer rides it and
    // `read_response` can correlate on it — which is also what proves the hang is over.
    let response = client.request_raw(90016, &raw).await;
    assert_eq!(
        response["error"]["code"], -32600,
        "the class frame must be answered -32600: {response}"
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

/// **C-P2** — the arm is INSTALLED on the real serve path, and the ambiguous frame is answered with
/// no id while neither of its two candidate ids is ever answered.
///
/// Observed by SENTINEL FOLLOW over `seen_lines`, because a reply with no id is invisible to an
/// id-correlating reader.
#[tokio::test]
async fn the_arm_is_installed_on_the_real_serve_path() {
    let session = common::session().await;
    let (mut client, _server, _cancel) = common::connect_raw(session, Quotas::default()).await;

    // D04 carries two DIFFERING ids (90004 / 90005), so the reply takes the OMITTED arm.
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
        client.saw_line_containing("-32600"),
        "the ambiguous class frame must still be ANSWERED, with the id omitted: {:?}",
        client.seen_lines
    );
    assert!(
        !client.saw_response_for(90_004) && !client.saw_response_for(90_005),
        "neither candidate id may be answered on — the bytes are ambiguous, so the id is OMITTED"
    );
}
