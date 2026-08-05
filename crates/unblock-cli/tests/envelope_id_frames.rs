//! **[v1.0.1/D47] The UN-DECODABLE-ENVELOPE-`id` class over a REAL `unblock mcp` child.**
//!
//! Homed in `unblock-cli` because only this crate may exec `CARGO_BIN_EXE_unblock`: this is the one
//! suite that drives a real process, real pipes and real stdio framing, which is where the defect
//! was actually reproduced.
//!
//! # A NEW file rather than more cells in `duplicate_key_frames.rs`
//!
//! That file's module doc is D43-scoped ("DUPLICATE JSON KEYS"), and duplication is a MINORITY
//! route into the D47 class — a `null` id, a wrongly-typed id, an out-of-range number and the
//! `\u`-escaped key all reach the same silence without duplicating anything. Overloading that file
//! would make its own doc false. The ONE cell that must stay there is NS4, because it is the cell
//! that pinned the residual D47 closes.
//!
//! # Two observation channels, and why both are needed
//!
//! * `request_raw` CORRELATES BY ID and RETURNS. That is the strongest possible proof for the
//!   recovered arm: before D47 that call could not return at all, because nothing was ever written
//!   for that id. This is the cell that proves the hang is over.
//! * A reply whose id is OMITTED is INVISIBLE to any id-correlating reader, so the ambiguous arm is
//!   observed by SENTINEL FOLLOW plus a scan of `seen_lines`. No sleeps, no timeouts, no threads.
//!
//! NFR-14 rides along for free everywhere here: `read_response` panics on any stdout line that is
//! not valid JSON.

mod common;

use common::{McpClient, Workspace};
use unblock_mcp::envelope_id_corpus::divergence_corpus;

/// Fetch one divergence-corpus frame as text, so the CLI suite and the in-lib cells drive the
/// SAME bytes rather than two hand-copied spellings that can drift.
fn frame_text(entry: &str) -> String {
    let frame = divergence_corpus()
        .into_iter()
        .find(|f| f.id == entry)
        .unwrap_or_else(|| panic!("corpus entry {entry} is missing"))
        .frame;
    String::from_utf8(frame).expect("corpus frames are UTF-8")
}

/// Spawn a child and complete the handshake.
fn connected(ws: &Workspace) -> McpClient {
    let mut client = McpClient::spawn(ws.root());
    client.initialize();
    client
}

/// Send a known-good request and read its answer, draining everything written before it into
/// `seen_lines`. The shipped SENTINEL FOLLOW, which is what makes every negative here timeout-free.
fn sentinel(client: &mut McpClient) {
    let id = client.next_request_id();
    let frame = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping","params":{{}}}}"#);
    let answer = client.request_raw(id, &frame);
    assert!(
        answer.get("result").is_some(),
        "the sentinel must be answered — the connection must survive: {answer}"
    );
}

/// **D-P1** — the RECOVERED id is answered over real stdio, and the hang is provably over.
///
/// `request_raw` correlates by id, so it can only return if a reply carrying id 90001 actually
/// arrived. On `main` this call cannot return at all: nothing is ever written for that frame.
///
/// Mutant: passing `None` instead of `Some(id)` on the recovered arm.
#[test]
fn the_recovered_id_is_answered_over_real_stdio() {
    let ws = Workspace::init();
    let mut client = connected(&ws);

    let response = client.request_raw(90001, &frame_text("D01"));
    assert_eq!(
        response["error"]["code"], -32600,
        "the recovered-id frame must be answered -32600 ON that id: {response}"
    );
    assert!(
        response.get("result").is_none(),
        "an out-of-band error carries no result: {response}"
    );
}

/// **D-P2** — the connection RECOVERS: a normal tool call after an answer still works.
///
/// Mutant: `continue` replaced by `return None`, which would close the connection after answering.
#[test]
fn the_connection_recovers_after_an_answer() {
    let ws = Workspace::init();
    let mut client = connected(&ws);

    let created = client.call_tool_envelope(
        "issue",
        &serde_json::json!({"action":"create","title":"D47 recovery target"}),
    );
    let id = created["result"]["structuredContent"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("minted id in {created}"))
        .to_string();

    let answered = client.request_raw(90001, &frame_text("D01"));
    assert_eq!(answered["error"]["code"], -32600);

    let (is_error, shown) =
        client.call_tool("issue", &serde_json::json!({"action":"show","id":id}));
    assert!(!is_error, "the following tool call must succeed: {shown}");
    assert_eq!(
        shown["id"].as_str(),
        Some(id.as_str()),
        "and must return the real target: {shown}"
    );
}

/// **D-P3** — the AMBIGUOUS frame is answered with the id OMITTED, and neither candidate is used.
///
/// The `"id":null` half additionally pins the RATIFIED fallback spelling end to end: the missing id
/// is spelled by OMISSION, never as a literal null. `rmcp::model::JsonRpcError.id` is
/// `Option<RequestId>` under `skip_serializing_if`, so a null is not even reachable through the
/// codec — if that decision is ever revised, this is one of the assertions that must change.
///
/// Mutant: emitting any id at all on the fallback arm.
#[test]
fn the_ambiguous_answer_omits_the_id() {
    let ws = Workspace::init();
    let mut client = connected(&ws);

    client.write_raw_line(&frame_text("D04"));
    sentinel(&mut client);

    assert!(
        client.seen_lines.iter().any(|l| l.contains("-32600")),
        "the ambiguous frame must still be ANSWERED: {:?}",
        client.seen_lines
    );
    assert!(
        !client.saw_response_for(90_004) && !client.saw_response_for(90_005),
        "neither DIFFERING candidate id may be answered on"
    );
    assert!(
        !client.seen_lines.iter().any(|l| l.contains(r#""id":null"#)),
        "the fallback spells the missing id by OMISSION, not as a literal null: {:?}",
        client.seen_lines
    );
}

/// **D-P4** — a STRING id is recovered.
///
/// Observed through `seen_lines` rather than `request_raw`, because the harness correlates on
/// `as_i64` and structurally cannot match a string id. Discriminating jointly with the in-lib
/// byte cells: this one shows the string survives the real stdio round trip.
#[test]
fn a_string_id_is_recovered() {
    let ws = Workspace::init();
    let mut client = connected(&ws);

    client.write_raw_line(&frame_text("D02"));
    sentinel(&mut client);

    let hit = client
        .seen_lines
        .iter()
        .find(|l| l.contains(r#""id":"e2e-abc""#))
        .unwrap_or_else(|| panic!("no reply on the string id: {:?}", client.seen_lines));
    let parsed: serde_json::Value = serde_json::from_str(hit).expect("the reply is JSON");
    assert_eq!(parsed["error"]["code"], -32600);
}

/// **D-P5** — the `\u`-escaped `id` KEY is recovered, over a real process.
///
/// Both occurrences are escaped on purpose: a MIXED escaped/plain pair would still leave one plain
/// occurrence for a raw-span key comparator to find, and it would recover the same id and emit
/// byte-identical output. With both escaped, such a comparator sees ZERO occurrences and the frame
/// goes silent, which is what turns this cell red.
///
/// Mutant: comparing keys as raw spans instead of decoding them.
#[test]
fn an_escaped_key_duplicate_is_recovered() {
    let ws = Workspace::init();
    let mut client = connected(&ws);

    let raw = frame_text("D15");
    assert!(
        !raw.contains(r#""id":"#),
        "non-vacuity: BOTH occurrences must be escaped, or a raw-span comparator still finds one \
         plain key and this cell grades nothing: {raw}"
    );

    let response = client.request_raw(90015, &raw);
    assert_eq!(
        response["error"]["code"], -32600,
        "the escaped key IS a genuine envelope id and must be recovered: {response}"
    );
}

/// **D-N1** — a genuine id-LESS notification is still ignored, over real stdio.
///
/// The false-positive guard for D47's explicit carve-out, and a JSON-RPC requirement.
///
/// Mutant: deleting the `Absent` arm.
#[test]
fn a_genuine_notification_is_still_ignored() {
    let ws = Workspace::init();
    let mut client = connected(&ws);

    client.write_raw_line(r#"{"jsonrpc":"2.0","method":"notifications/foo","params":{}}"#);
    sentinel(&mut client);

    assert!(
        !client.seen_lines.iter().any(|l| l.contains("-32600")),
        "a notification with NO id must NEVER be answered: {:?}",
        client.seen_lines
    );
}

/// **D-N2** — the Decision-4 parity drop is still silent over real stdio.
///
/// F17 carries an `id`, so it looks like a class frame, but rmcp itself drops it — and our fork
/// exists to match rmcp byte for byte. The exclusion is structural: the frame returns `Ok(None)`
/// from the compatibility filter and never becomes a delivered message at all.
///
/// Mutant: neutering the compat filter's second arm.
#[test]
fn the_parity_drop_is_still_silent_over_real_stdio() {
    let ws = Workspace::init();
    let mut client = connected(&ws);

    client.write_raw_line(r#"{"jsonrpc":"2.0","id":17,"method":"notifications/foo","params":5}"#);
    sentinel(&mut client);

    assert!(
        !client.seen_lines.iter().any(|l| l.contains("-32600")),
        "the deliberate parity drop must stay SILENT: {:?}",
        client.seen_lines
    );
    assert!(
        !client.saw_response_for(17),
        "and its id must never be answered on"
    );
}
