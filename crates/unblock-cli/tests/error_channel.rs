//! **D42 — the ERROR-CHANNEL matrix, over a real `unblock mcp` child on real JSON-RPC stdio.**
//!
//! A content hash over the discovery documents cannot express a CHANNEL. Whether a malformed
//! argument comes back as an out-of-band JSON-RPC `error` or as an in-band
//! `CallToolResult{isError:true}` carrying the FR-11 `StructuredError` is invisible to
//! `CONTRACT_HASH`, to the goldens, and to every schema assertion — it is only observable by
//! driving the wire. That is what this file does.
//!
//! **`resp["error"].is_none()` is THE assertion.** Every other assertion here is secondary. Before
//! D42, `comment{add, issue_id}` with no `body` returned an out-of-band `-32602 "failed to
//! deserialize parameters: missing field `body`"` with `data: null` — no code, no hint, no context,
//! nothing an agent can self-correct from.
//!
//! It lives in `unblock-cli` because it spawns `CARGO_BIN_EXE_unblock`: an `unblock-mcp` test
//! reaching for the binary would invert the crate dependency. `mcp_round_trip.rs` is the precedent.
//!
//! Cells are pinned **PER ACTION**, not per tool. "8 tools x 3 malformations" would let an
//! implementer satisfy the matrix with one action per tool, leaving most arms unexercised.

#![cfg(unix)]

mod common;

use common::{McpClient, Workspace};
use serde_json::{Value, json};

/// Spawn an initialized MCP child over a fresh workspace.
fn client(ws: &Workspace) -> McpClient {
    let mut c = McpClient::spawn(ws.root());
    c.initialize();
    c
}

/// Assert the D42 in-band contract on a `tools/call` response envelope.
fn assert_in_band_validation_failure(resp: &Value, cell: &str) {
    assert!(
        resp.get("error").is_none(),
        "{cell}: THE CHANNEL INVARIANT FAILED — this came back as an OUT-OF-BAND JSON-RPC error \
         instead of in-band. That is the D42 defect (or the `Parameters` seam was reverted to \
         rmcp's, which re-deserializes inside the extractor). Envelope: {resp}"
    );
    let result = &resp["result"];
    assert_eq!(
        result["isError"], true,
        "{cell}: must be an in-band ERROR result: {resp}"
    );
    let payload = &result["structuredContent"];
    assert_eq!(payload["code"], "VALIDATION_FAILED", "{cell}: {payload}");
    assert_eq!(
        payload["retryable"], true,
        "{cell}: VALIDATION_FAILED is in the retryable set — a client that stops retrying on it is \
         acting on a false signal: {payload}"
    );
    let hint = payload["hint"].as_str().unwrap_or_default();
    assert!(
        !hint.is_empty(),
        "{cell}: a non-empty hint is mandatory. On a flattening MCP client the hint and the field \
         descriptions are the ONLY two signals that survive, so an empty hint leaves the agent with \
         nothing: {payload}"
    );
}

/// Malformation 1 — a MISSING REQUIRED field. Pre-D42 this escaped as `-32602`.
#[test]
fn missing_required_field_is_in_band_per_action() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    for (cell, tool, args) in [
        (
            "comment{add} without body",
            "comment",
            json!({"action":"add","issue_id":"ub-1"}),
        ),
        (
            "issue{create} without title",
            "issue",
            json!({"action":"create"}),
        ),
        ("issue{show} without id", "issue", json!({"action":"show"})),
        ("claim without assignee", "claim", json!({"id":"ub-1"})),
        (
            "defer{defer} without until",
            "defer",
            json!({"action":"defer","id":"ub-1"}),
        ),
        (
            "dep{add} without dep_type",
            "dep",
            json!({"action":"add","issue_id":"a","depends_on_id":"b"}),
        ),
        (
            "query{search} without query",
            "query",
            json!({"kind":"search"}),
        ),
        (
            "sync{import} without path",
            "sync",
            json!({"action":"import"}),
        ),
    ] {
        let resp = c.call_tool_envelope(tool, &args);
        assert_in_band_validation_failure(&resp, cell);
        assert_eq!(
            resp["result"]["structuredContent"]["context"]["kind"], "missing_field",
            "{cell}"
        );
    }
}

/// Malformation 2 — an UNKNOWN / misspelled field. Pre-D42 this was SILENTLY DISCARDED with
/// `isError:false`: genuine data loss, the defect class's headline harm.
#[test]
fn unknown_field_is_rejected_in_band_per_action() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    for (cell, tool, args, field) in [
        (
            "issue{create} misspelled description",
            "issue",
            json!({"action":"create","title":"t","prioriti":0,"descriptionn":"LOST"}),
            // TWO unknown keys; serde reports the FIRST it encounters. `serde_json::Map` is a
            // BTreeMap by default, so iteration is sorted and `descriptionn` < `prioriti` — the
            // reported field is deterministic and therefore assertable. (Same `preserve_order`
            // caveat as the quota walk: enabling that feature would make this order input-dependent.)
            "descriptionn",
        ),
        (
            "issue{update} misspelled title",
            "issue",
            json!({"action":"update","ids":["ub-1"],"titlee":"x"}),
            "titlee",
        ),
        (
            "issue{close} misspelled reason",
            "issue",
            json!({"action":"close","id":"ub-1","resaon":"done"}),
            "resaon",
        ),
        (
            "comment{add} misspelled body",
            "comment",
            json!({"action":"add","issue_id":"ub-1","body":"b","bodyy":"LOST"}),
            "bodyy",
        ),
        (
            "claim misspelled assignee",
            "claim",
            json!({"id":"ub-1","assignee":"a","assignie":"LOST"}),
            "assignie",
        ),
        (
            "query{ready} misspelled filter",
            "query",
            json!({"kind":"ready","assignie":"a"}),
            "assignie",
        ),
        (
            "dep{add} misspelled metadata",
            "dep",
            json!({"action":"add","issue_id":"a","depends_on_id":"b","dep_type":"blocks","metadataa":"LOST"}),
            "metadataa",
        ),
        (
            "sync{import} misspelled dry_run",
            "sync",
            json!({"action":"import","path":"x.jsonl","dry_runn":true}),
            "dry_runn",
        ),
        (
            "defer{defer} misspelled until",
            "defer",
            json!({"action":"defer","id":"ub-1","until":"2030-01-01T00:00:00Z","untill":"x"}),
            "untill",
        ),
        (
            "diagnostics{changelog} misspelled since",
            "diagnostics",
            json!({"kind":"changelog","sinse":"2030-01-01T00:00:00Z"}),
            "sinse",
        ),
    ] {
        let resp = c.call_tool_envelope(tool, &args);
        assert_in_band_validation_failure(&resp, cell);
        let payload = &resp["result"]["structuredContent"];
        assert_eq!(payload["context"]["kind"], "unknown_field", "{cell}");
        assert_eq!(
            payload["context"]["field"], field,
            "{cell}: the offending field must be NAMED — an agent cannot self-correct otherwise"
        );
    }
}

/// The NESTED case. `deny_unknown_fields` is NOT recursive, so `CreateInput`'s attribute does
/// nothing for the elements of `deps` — `DepInput` needs its own. Without it this cell is the only
/// thing that goes red.
#[test]
fn an_unknown_field_nested_in_a_dep_element_is_rejected() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let resp = c.call_tool_envelope(
        "issue",
        &json!({
            "action":"create","title":"t",
            "deps":[{"issue_id":"a","depends_on_id":"b","dep_type":"blocks","metadataa":"LOST"}]
        }),
    );
    assert_in_band_validation_failure(&resp, "nested DepInput");
    assert_eq!(
        resp["result"]["structuredContent"]["context"]["field"],
        "metadataa"
    );
}

/// An `_`-prefixed unknown key INSIDE `arguments` is REJECTED, not stripped.
///
/// There is deliberately no `_`-prefix strip at the seam: a conformant MCP `_meta` is a SIBLING of
/// `arguments` on `CallToolRequestParams` and rmcp destructures it away before the extractor, so it
/// never reaches `context.arguments`. A blanket strip would therefore protect nothing while
/// permanently re-opening the silent drop for any key an agent happens to prefix with `_`.
#[test]
fn an_underscore_prefixed_unknown_key_is_rejected_not_stripped() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let resp = c.call_tool_envelope("issue", &json!({"action":"show","id":"ub-1","_junk":"X"}));
    assert_in_band_validation_failure(&resp, "_junk inside arguments");
    assert_eq!(
        resp["result"]["structuredContent"]["context"]["field"],
        "_junk"
    );
}

/// Malformation 3 — a TYPE MISMATCH.
#[test]
fn type_mismatch_is_in_band_per_action() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    for (cell, tool, args) in [
        (
            "comment{add} body as a number",
            "comment",
            json!({"action":"add","issue_id":"ub-1","body":42}),
        ),
        (
            "issue{update} ids as a string",
            "issue",
            json!({"action":"update","ids":"ub-1"}),
        ),
        (
            "issue{create} title as an object",
            "issue",
            json!({"action":"create","title":{}}),
        ),
        (
            "query{count} limit as a string",
            "query",
            json!({"kind":"count","limit":"ten"}),
        ),
    ] {
        let resp = c.call_tool_envelope(tool, &args);
        assert_in_band_validation_failure(&resp, cell);
    }
}

/// An UNKNOWN ACTION on a tagged-enum input.
#[test]
fn an_unknown_action_discriminator_is_in_band() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let resp = c.call_tool_envelope("issue", &json!({"action":"obliterate","id":"ub-1"}));
    assert_in_band_validation_failure(&resp, "issue{obliterate}");
}

/// Malformation 4 — `arguments` OMITTED ENTIRELY. A very plausible agent failure mode: pre-D42 this
/// was an out-of-band `-32602` complaining about a missing `action` field; the deferring seam's
/// `unwrap_or_default()` turns the absent member into an empty object, which reaches the in-band
/// channel.
#[test]
fn a_tools_call_with_no_arguments_member_is_in_band() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let resp = c.call_tool_raw_params(&json!({"name": "issue"}));
    assert_in_band_validation_failure(&resp, "issue with no `arguments` member");
}

// --- HAPPY-PATH NON-VACUITY ----------------------------------------------------------------------
//
// A rejection-only matrix stays fully green if the attribute placement broke an action arm outright.
// These cells prove the tools still WORK.

/// A create carrying the FLATTENED `agent_name` (`Attribution`) must stay `Ok`. This is the exact
/// case a misplaced `deny_unknown_fields` breaks — a denying container that carries
/// `#[serde(flatten)]` must still accept the flattened target's legitimate keys.
#[test]
fn a_create_with_flattened_attribution_still_succeeds() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let (is_error, payload) = c.call_tool(
        "issue",
        &json!({"action":"create","title":"t","agent_name":"claude","harness":"cc","model":"opus"}),
    );
    assert!(
        !is_error,
        "the flattened Attribution keys must be accepted: {payload}"
    );
}

/// The full ready -> claim -> close round trip still works per action (the FR-20 AC).
#[test]
fn the_happy_path_still_round_trips_per_action() {
    let ws = Workspace::init();
    let mut c = client(&ws);

    let (is_error, created) = c.call_tool("issue", &json!({"action":"create","title":"work"}));
    assert!(!is_error, "create: {created}");
    let id = created["id"].as_str().expect("minted id").to_string();

    for (cell, tool, args) in [
        ("query ready", "query", json!({"kind":"ready"})),
        ("issue show", "issue", json!({"action":"show","id":id})),
        ("claim", "claim", json!({"id":id,"assignee":"me"})),
        (
            "comment add",
            "comment",
            json!({"action":"add","issue_id":id,"body":"note"}),
        ),
        (
            "comment list",
            "comment",
            json!({"action":"list","issue_id":id}),
        ),
        (
            "diagnostics version",
            "diagnostics",
            json!({"kind":"version"}),
        ),
        (
            "issue close",
            "issue",
            json!({"action":"close","id":id,"suggest_next":true}),
        ),
    ] {
        let (is_error, payload) = c.call_tool(tool, &args);
        assert!(!is_error, "{cell} must still succeed: {payload}");
    }
}

// --- NEGATIVE SCOPE: what deliberately STAYS out-of-band ------------------------------------------
//
// Without these, nothing stops the deferring seam being over-applied to genuine protocol faults.

/// An UNKNOWN TOOL NAME is a protocol fault and stays out-of-band. No seam under our control
/// reaches it — the router rejects before any extractor runs.
#[test]
fn an_unknown_tool_name_stays_a_protocol_fault() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let resp = c.call_tool_envelope("no_such_tool", &json!({}));
    assert!(
        resp.get("error").is_some(),
        "an unknown tool name MUST stay an out-of-band protocol fault: {resp}"
    );
}

/// A NON-OBJECT `arguments` is a protocol fault: rmcp fails to deserialize
/// `CallToolRequestParams` itself, so `ServerHandler::call_tool` is never entered and the in-band
/// channel is structurally unreachable.
#[test]
fn a_non_object_arguments_stays_a_protocol_fault() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let resp = c.call_tool_raw_params(&json!({"name": "issue", "arguments": 42}));
    assert!(
        resp.get("error").is_some(),
        "a non-object `arguments` MUST stay out-of-band — it never reaches call_tool: {resp}"
    );
}

/// `params.task` is rejected by rmcp (`TaskSupport::Forbidden` is the default) BEFORE
/// `ServerHandler::call_tool`, so `request.task` is always `None` at our quota check. It is
/// therefore NOT an exploitable bypass channel. This documents that, and fails loudly if a future
/// rmcp changes the defaulting.
#[test]
fn task_invocation_stays_a_protocol_fault() {
    let ws = Workspace::init();
    let mut c = client(&ws);
    let resp = c.call_tool_raw_params(&json!({
        "name": "issue", "arguments": {"action":"show","id":"ub-1"}, "task": {}
    }));
    assert!(
        resp.get("error").is_some(),
        "`params.task` MUST stay a protocol fault. If this goes green, rmcp changed its TaskSupport \
         defaulting and `task` became a live channel that our quota now has to be re-checked \
         against: {resp}"
    );
}
