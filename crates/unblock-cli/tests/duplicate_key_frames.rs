//! **D43 — DUPLICATE JSON KEYS, over a real `unblock mcp` child on real JSON-RPC stdio.**
//!
//! This is the suite that reproduces the live exploit. Before D43,
//!
//! ```text
//! {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"issue",
//!  "arguments":{"action":"create","ids":["ub-vog"],"action":"delete"}}}
//! ```
//!
//! returned `isError:false` and TOMBSTONED the issue: `serde_json` collapses the duplicated
//! `action` last-wins while rmcp decodes the frame, so a frame whose text reads as a create
//! executes a delete.
//!
//! **No other suite in this repo can express that input.** `serde_json::to_string`, `json!` and any
//! rmcp client all build a `Map` before writing bytes, and the `Map` is where the collapse happens —
//! so the ONLY way to test this is to write the bytes by hand, which is what
//! `common::McpClient::request_raw` exists for. A green test that cannot express the input proves
//! nothing, so every cell here also asserts its own non-vacuity.
//!
//! It lives in `unblock-cli` because it spawns `CARGO_BIN_EXE_unblock`: an `unblock-mcp` test
//! reaching for the binary would invert the crate dependency. `error_channel.rs` is the precedent.
//!
//! The case corpus is declared ONCE in `unblock_mcp::duplicate_key_corpus` and shared with
//! `unblock-mcp`'s duplex suite, which drives the SAME cells over an in-process transport. Running
//! one corpus over both entry points is what proves the scan lives in the shared transport rather
//! than being bolted to one of them.

#![cfg(unix)]

mod common;

use common::{McpClient, Workspace};
use serde_json::{Value, json};
use unblock_mcp::duplicate_key_corpus::{CELLS, instantiate, raw_tools_call};

/// Spawn an initialized MCP child over a fresh workspace.
fn client(ws: &Workspace) -> McpClient {
    let mut c = McpClient::spawn(ws.root());
    c.initialize();
    c
}

/// Create an issue and return its minted id.
fn create_issue(c: &mut McpClient, title: &str) -> String {
    let (is_error, structured) = c.call_tool("issue", &json!({"action":"create","title":title}));
    assert!(!is_error, "fixture create must succeed: {structured}");
    structured["id"]
        .as_str()
        .unwrap_or_else(|| panic!("minted id in {structured}"))
        .to_string()
}

/// The D43 in-band contract on a rejected frame.
fn assert_in_band_duplicate_key(resp: &Value, cell: &str, key: &str, pointer: &str) {
    assert!(
        resp.get("error").is_none(),
        "{cell}: THE CHANNEL INVARIANT FAILED — the duplicate came back OUT-OF-BAND. Keeping this \
         in-band is the whole point of rejecting at `call_tool` rather than at the transport: {resp}"
    );
    let result = &resp["result"];
    assert_eq!(
        result["isError"], true,
        "{cell}: must be an in-band ERROR result: {resp}"
    );
    let payload = &result["structuredContent"];
    assert_eq!(
        payload["code"], "VALIDATION_FAILED",
        "{cell}: minting an ErrorCode would move CONTRACT_HASH — the kind rides `context`: {payload}"
    );
    assert_eq!(payload["retryable"], true, "{cell}: {payload}");
    assert!(
        !payload["hint"].as_str().unwrap_or_default().is_empty(),
        "{cell}: a non-empty hint is mandatory — on a flattening client it is one of the only two \
         signals that survive: {payload}"
    );
    assert_eq!(
        payload["context"]["kind"], "duplicate_key",
        "{cell}: {payload}"
    );
    assert_eq!(payload["context"]["field"], key, "{cell}: {payload}");
    assert_eq!(
        payload["context"]["path"], pointer,
        "{cell}: a NESTED duplicate is unlocatable without the pointer: {payload}"
    );
}

/// A comparable fingerprint of everything a corpus cell could disturb.
fn store_fingerprint(c: &mut McpClient, ids: &[String]) -> Value {
    let mut out = serde_json::Map::new();
    for id in ids {
        out.insert(
            format!("issue:{id}"),
            c.call_tool_envelope("issue", &json!({"action":"show","id":id}))["result"].clone(),
        );
        out.insert(
            format!("comments:{id}"),
            c.call_tool_envelope("comment", &json!({"action":"list","issue_id":id}))["result"]
                .clone(),
        );
        out.insert(
            format!("deps:{id}"),
            c.call_tool_envelope("dep", &json!({"action":"list","id":id}))["result"].clone(),
        );
    }
    out.insert(
        "count".to_string(),
        c.call_tool_envelope("query", &json!({"kind":"count"}))["result"].clone(),
    );
    Value::Object(out)
}

/// **THE LIVE EXPLOIT, verbatim.**
///
/// This exact frame tombstoned a real issue on GA. It is spelled out here rather than generated so
/// that the regression is unmistakable, and the effect oracle proves the target survives — an
/// `isError:true` on a frame that already deleted the row would be worthless.
#[test]
fn the_live_exploit_frame_is_rejected_and_the_target_survives() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let target = create_issue(&mut c, "the exploit target");

    let id = c.next_request_id();
    let frame = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"issue","arguments":{{"action":"create","ids":["{target}"],"action":"delete"}}}}}}"#
    );
    assert_eq!(
        frame.matches("\"action\"").count(),
        2,
        "non-vacuity: the exploit frame must really carry `action` twice"
    );
    let resp = c.request_raw(id, &frame);
    assert_in_band_duplicate_key(&resp, "live exploit", "action", "/arguments");

    // THE EFFECT ORACLE: the issue must still exist, un-tombstoned, with its original fields.
    let (is_error, shown) = c.call_tool("issue", &json!({"action":"show","id":target}));
    assert!(!is_error, "the target must still be readable: {shown}");
    assert_eq!(shown["id"], target);
    assert_eq!(
        shown["status"], "open",
        "the target must be untouched: {shown}"
    );
    assert_eq!(shown["title"], "the exploit target");
}

/// Every corpus cell, over the real child: rejected IN-BAND, with the store UNCHANGED.
///
/// The effect oracle is not optional. A fix that rejects AFTER mutating passes every channel
/// assertion in this file and still does the exact harm the defect did.
#[test]
fn every_corpus_cell_is_rejected_in_band_with_zero_effect() {
    let ws = Workspace::init();
    let mut c = client(&ws);

    for cell in CELLS {
        let first = create_issue(&mut c, &format!("{} target A", cell.id));
        let second = create_issue(&mut c, &format!("{} target B", cell.id));
        let ids = [first.clone(), second.clone()];
        let before = store_fingerprint(&mut c, &ids);

        let id = c.next_request_id();
        let arguments = instantiate(cell.arguments_text, &first, &second);
        // Non-vacuity: the ARGUMENTS text really carries the key twice (the second term catches the
        // escape-equivalent cell, whose second occurrence is byte-DIFFERENT from the first).
        assert_eq!(
            arguments
                .matches(&format!("\"{}\"", cell.duplicated_key))
                .count()
                + arguments
                    .matches(&format!("\"\\u00{:02x}", cell.duplicated_key.as_bytes()[0]))
                    .count(),
            2,
            "{}: the cell is vacuous — {arguments}",
            cell.id
        );
        let resp = c.request_raw(id, &raw_tools_call(id, cell.tool, &arguments));
        assert_in_band_duplicate_key(&resp, cell.id, cell.duplicated_key, cell.pointer);

        let after = store_fingerprint(&mut c, &ids);
        assert_eq!(
            before, after,
            "{}: THE STORE CHANGED — a fix that rejects after mutating is not a fix",
            cell.id
        );
    }
}

/// **The ACCEPT half.** Every cell's SHOWN arm, with the duplicate deleted, must EXECUTE.
///
/// Without it a rejection cell can pass for the wrong reason — a renamed field, a changed tag, or a
/// scanner that simply refuses everything — and the reject half proves nothing. This is the same
/// discipline the D42 deny-guard states for its own cases.
#[test]
fn the_shown_arm_of_every_cell_still_executes_when_the_duplicate_is_removed() {
    for cell in CELLS {
        // A FRESH workspace per cell: several shown arms are destructive (delete, claim, defer), so
        // sharing one workspace would let cell N's effect decide cell N+1's outcome.
        let ws = Workspace::init();
        let mut c = client(&ws);
        let first = create_issue(&mut c, &format!("{} accept A", cell.id));
        let second = create_issue(&mut c, &format!("{} accept B", cell.id));

        let arguments = instantiate(cell.shown, &first, &second);
        assert_eq!(
            arguments
                .matches(&format!("\"{}\"", cell.duplicated_key))
                .count(),
            1,
            "{}: the ACCEPT half must carry the key exactly ONCE: {arguments}",
            cell.id
        );
        let id = c.next_request_id();
        let resp = c.request_raw(id, &raw_tools_call(id, cell.tool, &arguments));
        assert!(
            resp.get("error").is_none(),
            "{}: the accept half must not fault: {resp}",
            cell.id
        );
        assert_ne!(
            resp["result"]["isError"], true,
            "{}: the SHOWN arm must EXECUTE once the duplicate is removed — otherwise the reject \
             half above proves nothing about duplicates: {resp}",
            cell.id
        );
    }
}

// -------------------------------------------------------------------------------------------
// Framing / normalization, end to end through the real child.
// -------------------------------------------------------------------------------------------

/// F3 — a BOM-prefixed duplicate frame is still rejected (the BOM is stripped exactly once, prefix
/// only, by BOTH the scanner and the parser, so they see the same document).
#[test]
fn f3_a_bom_prefixed_duplicate_frame_is_rejected() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let target = create_issue(&mut c, "bom target");

    let id = c.next_request_id();
    let frame = format!(
        "\u{feff}{}",
        raw_tools_call(
            id,
            "issue",
            &format!(r#"{{"action":"show","id":"{target}","action":"close"}}"#)
        )
    );
    let resp = c.request_raw(id, &frame);
    assert_in_band_duplicate_key(&resp, "F3 BOM + duplicate", "action", "/arguments");
}

/// F1 — a BOM-prefixed CLEAN frame still executes (the BOM strip does not itself break framing).
#[test]
fn f1_a_bom_prefixed_clean_frame_executes() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let target = create_issue(&mut c, "bom clean target");

    let id = c.next_request_id();
    let frame = format!(
        "\u{feff}{}",
        raw_tools_call(
            id,
            "issue",
            &format!(r#"{{"action":"show","id":"{target}"}}"#)
        )
    );
    let resp = c.request_raw(id, &frame);
    assert_ne!(resp["result"]["isError"], true, "{resp}");
    assert_eq!(resp["result"]["structuredContent"]["id"], target);
}

/// F4 — a 100 KiB pad AHEAD of the second occurrence. The duplicate is found regardless of where in
/// the frame it sits; a scanner that only looked at a prefix would miss it.
#[test]
fn f4_a_padded_duplicate_past_100kib_is_rejected() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let target = create_issue(&mut c, "padded target");

    let pad = "x".repeat(100 * 1024);
    let id = c.next_request_id();
    let arguments =
        format!(r#"{{"action":"show","id":"{target}","description":"{pad}","action":"close"}}"#);
    let resp = c.request_raw(id, &raw_tools_call(id, "issue", &arguments));
    assert_in_band_duplicate_key(&resp, "F4 padded duplicate", "action", "/arguments");
}

/// F13 — `resources/read` with a duplicated `uri`.
///
/// Resources are SCANNED (the scan runs on the raw bytes before rmcp classifies the method) but
/// deliberately NOT gated: they have no in-band channel, so gating them would have to answer
/// out-of-band and reopen the arm this design keeps shut. That is safe because the frame does not
/// execute anyway — it fails rmcp's own typed parse and falls to the non-executing `-32601` class.
/// This cell pins that, so the residual is a measured fact rather than an assumption.
#[test]
fn f13_a_duplicated_resource_uri_does_not_execute_last_wins() {
    let ws = Workspace::init();
    let mut c = client(&ws);

    let id = c.next_request_id();
    let frame = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"resources/read","params":{{"uri":"unblock://issues/ready","uri":"unblock://issues/blocked"}}}}"#
    );
    let resp = c.request_raw(id, &frame);
    assert_eq!(
        resp["error"]["code"], -32601,
        "the frame must fall to the already non-executing class, NOT read the last-wins URI: {resp}"
    );
    assert!(
        resp.get("result").is_none(),
        "nothing may be returned for it: {resp}"
    );
}

// -------------------------------------------------------------------------------------------
// NEGATIVE SCOPE — pins the ACTUAL boundary of the scan.
//
// Without these, the fix can silently re-open the out-of-band arm, or silently narrow the scan root
// back to `arguments` alone, with every positive cell above still green.
// -------------------------------------------------------------------------------------------

/// NS1 — a duplicated `params.name` is out-of-band `-32601`.
///
/// NOT because our scanner ignores it: `name` is reached through rmcp's own flatten, so the typed
/// variant merely fails and the untagged fallback lands on the custom-request handler. Unrelated
/// mechanism, already non-executing.
#[test]
fn ns1_a_duplicated_params_name_is_out_of_band_and_non_executing() {
    let ws = Workspace::init();
    let mut c = client(&ws);

    let id = c.next_request_id();
    let frame = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"issue","arguments":{{"kind":"stats"}},"name":"diagnostics"}}}}"#
    );
    let resp = c.request_raw(id, &frame);
    assert_eq!(resp["error"]["code"], -32601, "{resp}");
}

/// NS2 — a duplicated `params` KEY (not a duplicate INSIDE it) is a hard `-32700`, and the
/// connection RECOVERS on the next line.
#[test]
fn ns2_a_duplicated_params_key_is_a_parse_error_and_recovers() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let target = create_issue(&mut c, "ns2 target");

    let bad = r#"{"jsonrpc":"2.0","id":9001,"method":"tools/call","params":{"name":"issue"},"params":{"name":"claim"}}"#;
    c.write_raw_line(bad);

    // The sentinel proves BOTH that the parse error did not kill the connection and that the next
    // frame is served normally.
    let id = c.next_request_id();
    let resp = c.request_raw(
        id,
        &raw_tools_call(
            id,
            "issue",
            &format!(r#"{{"action":"show","id":"{target}"}}"#),
        ),
    );
    assert_ne!(
        resp["result"]["isError"], true,
        "the connection must recover after a -32700: {resp}"
    );
    let saw_parse_error = c
        .seen_lines
        .iter()
        .any(|line| line.contains("-32700") && !line.contains("\"id\":9001"));
    assert!(
        saw_parse_error,
        "a -32700 with the id OMITTED must have been emitted; lines: {:?}",
        c.seen_lines
    );
}

/// NS4 — a duplicated ENVELOPE `id` produces NO RESPONSE AT ALL.
///
/// It parses cleanly (so the compatibility filter never runs) and decodes as a NOTIFICATION, which
/// the notification path ignores — a client waiting on that id hangs forever. This is a KNOWN,
/// SCOPED-OUT residual: there is no request id to answer on, so an in-band reply is impossible and
/// an out-of-band one would reopen the arm this design closes. The cell pins the observed behaviour
/// so the residual stays a measured fact.
///
/// Proved by a SENTINEL FOLLOW — no timeouts, no sleeps, no threads.
#[test]
fn ns4_a_duplicated_envelope_id_gets_no_response_at_all() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let target = create_issue(&mut c, "ns4 target");

    let ghost_id = 424_242_i64;
    let bad = format!(
        r#"{{"jsonrpc":"2.0","id":{ghost_id},"id":999999,"method":"tools/call","params":{{"name":"issue","arguments":{{"action":"show","id":"{target}"}}}}}}"#
    );
    c.write_raw_line(&bad);

    // SENTINEL: a known-good request with a fresh id. Reading its response proves the server has
    // moved past the ghost frame.
    let sentinel = c.next_request_id();
    let resp = c.request_raw(
        sentinel,
        &raw_tools_call(
            sentinel,
            "issue",
            &format!(r#"{{"action":"show","id":"{target}"}}"#),
        ),
    );
    assert_ne!(
        resp["result"]["isError"], true,
        "sentinel must answer: {resp}"
    );

    assert!(
        !c.saw_response_for(ghost_id),
        "the duplicated-envelope-id frame is expected to get NO response (a scoped-out residual); \
         if this now answers, the residual closed and the note must be updated. Lines: {:?}",
        c.seen_lines
    );
    assert!(
        !c.saw_response_for(999_999),
        "nor under the last-wins id. Lines: {:?}",
        c.seen_lines
    );
}

/// NS5 — a duplicate OUTSIDE `params` is not flagged: the scan root is `params`, and a duplicate in
/// a sibling envelope member is invisible to the scanner BY CONSTRUCTION.
#[test]
fn ns5_a_duplicate_outside_params_is_not_flagged() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let target = create_issue(&mut c, "ns5 target");

    let id = c.next_request_id();
    let frame = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","extra":{{"a":1,"a":2}},"params":{{"name":"issue","arguments":{{"action":"show","id":"{target}"}}}}}}"#
    );
    let resp = c.request_raw(id, &frame);
    assert!(resp.get("error").is_none(), "{resp}");
    assert_ne!(
        resp["result"]["isError"], true,
        "a duplicate outside the scan root must NOT be flagged: {resp}"
    );
}

/// NS3 / N4 — a duplicate INSIDE the reserved `_meta` value IS rejected.
///
/// This is the tripwire against silently narrowing the scan root back to `params.arguments`: under
/// an `arguments`-only scan this frame reports CLEAN and executes.
#[test]
fn ns3_a_duplicate_inside_meta_is_rejected_in_band() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let target = create_issue(&mut c, "meta target");

    let id = c.next_request_id();
    let frame = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"issue","arguments":{{"action":"show","id":"{target}"}},"_meta":{{"trace":{{"span":"x","span":"y"}}}}}}}}"#
    );
    let resp = c.request_raw(id, &frame);
    assert_in_band_duplicate_key(&resp, "NS3 _meta nested", "span", "/_meta/trace");
}
