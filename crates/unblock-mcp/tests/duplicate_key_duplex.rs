//! **D43 — the DUPLICATE-KEY matrix over a live duplex server, driven at the RAW WIRE.**
//!
//! A duplicate JSON key is invisible to every existing suite in this repo, and not by accident: an
//! rmcp client, `serde_json::to_string` and `json!` all build a `Map` before writing bytes, and the
//! `Map` is exactly where the collapse happens. So a duplicate is only expressible by a harness that
//! owns the framing — `common::RawDuplexClient` — and that is what every cell here uses.
//!
//! The corpus itself is declared ONCE in `unblock_mcp::duplicate_key_corpus` and shared with
//! `unblock-cli`'s raw-stdio suite, which drives the same cells through a real spawned child.
//!
//! What this file pins that nothing else can:
//! - every corpus cell is rejected IN-BAND, with the duplicated key AND its RFC 6901 pointer;
//! - the store is UNCHANGED afterwards (a fix that rejects AFTER mutating would pass a
//!   channel-only assertion — and mutating first is precisely the live harm);
//! - the verdict is PER FRAME, not sticky across a connection;
//! - the fail-closed ABSENT-verdict arm actually fires on the one un-scanned in-tree path;
//! - the cells are real, schema-clean flips (non-vacuity), and the corpus covers every published
//!   tool.

mod common;

use serde_json::Value;
use unblock_mcp::duplicate_key_corpus::{
    CELLS, FlipCell, covered_tools, instantiate, parses_as_tool_input, raw_tools_call,
};

/// Create an issue and return its minted id.
async fn create_issue(client: &mut common::RawDuplexClient, title: &str) -> String {
    let response = client
        .call_tool(
            "issue",
            serde_json::json!({"action":"create","title":title}),
        )
        .await;
    let result = &response["result"];
    assert_ne!(
        result["isError"], true,
        "fixture create must succeed: {response}"
    );
    result["structuredContent"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("minted id in {response}"))
        .to_string()
}

/// The D43 in-band contract on a rejected frame.
fn assert_in_band_duplicate_key(response: &Value, cell: &str, key: &str, pointer: &str) {
    assert!(
        response.get("error").is_none(),
        "{cell}: THE CHANNEL INVARIANT FAILED — the duplicate came back OUT-OF-BAND. The whole \
         design turns on this staying in-band: {response}"
    );
    let result = &response["result"];
    assert_eq!(
        result["isError"], true,
        "{cell}: must be an in-band ERROR result: {response}"
    );
    let payload = &result["structuredContent"];
    assert_eq!(
        payload["code"], "VALIDATION_FAILED",
        "{cell}: a NEW ErrorCode would move CONTRACT_HASH — the kind must ride `context`: {payload}"
    );
    assert_eq!(payload["retryable"], true, "{cell}: {payload}");
    assert!(
        !payload["hint"].as_str().unwrap_or_default().is_empty(),
        "{cell}: a non-empty hint is mandatory: {payload}"
    );
    assert_eq!(
        payload["context"]["kind"], "duplicate_key",
        "{cell}: the only filterable discriminator is `context.kind`: {payload}"
    );
    assert_eq!(
        payload["context"]["field"], key,
        "{cell}: the duplicated key must be named: {payload}"
    );
    assert_eq!(
        payload["context"]["path"], pointer,
        "{cell}: a NESTED duplicate is unlocatable without the pointer: {payload}"
    );
}

/// A comparable fingerprint of everything the corpus could plausibly disturb.
///
/// THE EFFECT ORACLE (§4.7): without it, a fix that rejects *after* mutating passes the whole
/// matrix — and mutating first is exactly the live harm.
async fn store_fingerprint(client: &mut common::RawDuplexClient, ids: &[String]) -> Value {
    let mut out = serde_json::Map::new();
    for id in ids {
        let shown = client
            .call_tool("issue", serde_json::json!({"action":"show","id":id}))
            .await;
        out.insert(format!("issue:{id}"), shown["result"].clone());
        let comments = client
            .call_tool(
                "comment",
                serde_json::json!({"action":"list","issue_id":id}),
            )
            .await;
        out.insert(format!("comments:{id}"), comments["result"].clone());
        let deps = client
            .call_tool("dep", serde_json::json!({"action":"list","id":id}))
            .await;
        out.insert(format!("deps:{id}"), deps["result"].clone());
    }
    let count = client
        .call_tool("query", serde_json::json!({"kind":"count"}))
        .await;
    out.insert("count".to_string(), count["result"].clone());
    Value::Object(out)
}

/// **Row 6.3 + the effect oracle.** Every corpus cell, over the live scanning transport.
#[tokio::test]
async fn every_corpus_cell_is_rejected_in_band_with_zero_effect() {
    let session = common::session().await;
    let (mut client, server, cancel) =
        common::connect_raw(session, unblock_mcp::Quotas::default()).await;

    for cell in CELLS {
        let first = create_issue(&mut client, &format!("{} target A", cell.id)).await;
        let second = create_issue(&mut client, &format!("{} target B", cell.id)).await;
        let ids = [first.clone(), second.clone()];

        let before = store_fingerprint(&mut client, &ids).await;

        let id = client.next_request_id();
        let arguments = instantiate(cell.arguments_text, &first, &second);
        let frame = raw_tools_call(id, cell.tool, &arguments);
        // Non-vacuity: the ARGUMENTS text we are about to write must REALLY carry the key twice.
        // (Counted over `arguments`, not the whole frame — the JSON-RPC envelope has its own `id`.)
        // The second term catches the escape-equivalent cell, whose second occurrence is spelled
        // with a `\u00XX` escape and is therefore NOT byte-equal to the first.
        assert_eq!(
            arguments
                .matches(&format!("\"{}\"", cell.duplicated_key))
                .count()
                + arguments
                    .matches(&format!("\"\\u00{:02x}", cell.duplicated_key.as_bytes()[0]))
                    .count(),
            2,
            "{}: the arguments must carry the duplicated key exactly twice, else the cell is \
             vacuous: {arguments}",
            cell.id
        );
        let response = client.request_raw(id, &frame).await;

        assert_in_band_duplicate_key(&response, cell.id, cell.duplicated_key, cell.pointer);

        let after = store_fingerprint(&mut client, &ids).await;
        assert_eq!(
            before, after,
            "{}: THE STORE CHANGED. A fix that rejects AFTER mutating passes every channel \
             assertion and still does the harm.",
            cell.id
        );
    }

    let _ = server.cancel().await;
    cancel.cancel();
}

/// **Row 6.10 — the SEQUENCE cell.** Every other cell is a single frame, so a transport that stamped
/// a stale or shared verdict would pass the entire matrix. Three frames, one connection, in order.
#[tokio::test]
async fn the_verdict_is_per_frame_not_sticky() {
    let session = common::session().await;
    let (mut client, server, cancel) =
        common::connect_raw(session, unblock_mcp::Quotas::default()).await;
    let target = create_issue(&mut client, "sequence target").await;

    // (1) a duplicate-key call — rejected.
    let id1 = client.next_request_id();
    let dup = raw_tools_call(
        id1,
        "issue",
        &format!(r#"{{"action":"show","id":"{target}","action":"close"}}"#),
    );
    let first = client.request_raw(id1, &dup).await;
    assert_in_band_duplicate_key(&first, "sequence(1)", "action", "/arguments");

    // (2) a CLEAN call on the SAME connection, next request — must succeed AND execute.
    let clean = client
        .call_tool("issue", serde_json::json!({"action":"show","id":target}))
        .await;
    assert!(
        clean.get("error").is_none(),
        "sequence(2): a clean frame after a rejected one must still work: {clean}"
    );
    assert_ne!(
        clean["result"]["isError"], true,
        "sequence(2): a STICKY verdict would reject this too: {clean}"
    );
    assert_eq!(
        clean["result"]["structuredContent"]["id"], target,
        "sequence(2): the clean frame must actually execute: {clean}"
    );

    // (3) a second duplicate-key call — rejected AGAIN (the verdict is not consumed once).
    let id3 = client.next_request_id();
    let dup2 = raw_tools_call(
        id3,
        "issue",
        &format!(r#"{{"action":"show","id":"{target}","action":"delete"}}"#),
    );
    let third = client.request_raw(id3, &dup2).await;
    assert_in_band_duplicate_key(&third, "sequence(3)", "action", "/arguments");

    let _ = server.cancel().await;
    cancel.cancel();
}

/// **N4 — a duplicate nested inside the reserved `params._meta` value.**
///
/// This is the cell the whole-`params` scan root exists for: `_meta` is attacker-controlled, is
/// measured by the request quota, and reaches `call_tool` as `context.meta`, so it has exactly the
/// same in-band channel `arguments` does. An `arguments`-only scan reports this frame CLEAN — which
/// makes this cell the tripwire against silently narrowing the root back.
#[tokio::test]
async fn n4_a_duplicate_inside_meta_is_rejected_in_band() {
    let session = common::session().await;
    let (mut client, server, cancel) =
        common::connect_raw(session, unblock_mcp::Quotas::default()).await;
    let target = create_issue(&mut client, "meta target").await;

    let id = client.next_request_id();
    let frame = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"issue","arguments":{{"action":"show","id":"{target}"}},"_meta":{{"trace":{{"span":"x","span":"y"}}}}}}}}"#
    );
    let response = client.request_raw(id, &frame).await;
    assert_in_band_duplicate_key(&response, "N4 _meta nested", "span", "/_meta/trace");

    let _ = server.cancel().await;
    cancel.cancel();
}

/// **NS5 — a duplicate OUTSIDE `params` entirely is not flagged.**
///
/// The scan root is `params`; a duplicate in a sibling envelope member is invisible to the scanner
/// BY CONSTRUCTION. This discriminates a genuine outside-the-root miss from the envelope-field
/// duplicates (`jsonrpc`/`method`/`params`/`name`), which fail for an UNRELATED reason — rmcp's own
/// typed parse — and never reach the gate at all.
#[tokio::test]
async fn ns5_a_duplicate_outside_params_is_not_flagged() {
    let session = common::session().await;
    let (mut client, server, cancel) =
        common::connect_raw(session, unblock_mcp::Quotas::default()).await;
    let target = create_issue(&mut client, "outside target").await;

    let id = client.next_request_id();
    let frame = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","extra":{{"a":1,"a":2}},"params":{{"name":"issue","arguments":{{"action":"show","id":"{target}"}}}}}}"#
    );
    let response = client.request_raw(id, &frame).await;
    assert!(
        response.get("error").is_none(),
        "NS5: must not become an out-of-band fault: {response}"
    );
    assert_ne!(
        response["result"]["isError"], true,
        "NS5: a duplicate outside the scan root must NOT be flagged: {response}"
    );

    let _ = server.cancel().await;
    cancel.cancel();
}

/// **Row 6.8 — the fail-closed ABSENT-verdict arm.**
///
/// `mcp_server_duplex_unclamped_for_test` is the CD-6 RAW rmcp serve path and deliberately installs
/// NO scan, so a tool call through it reaches `call_tool` with no verdict at all. That arm — "absent
/// ⇒ reject" — is the entire security property (an empty `Extensions` is the DEFAULT state, so the
/// opposite encoding would fail OPEN), and before this cell it had zero coverage: the only in-tree
/// driver of that helper does an `initialize` handshake and never calls a tool.
#[tokio::test]
async fn an_unscanned_path_is_rejected_fail_closed() {
    let session = common::session().await;
    // TWO connections over the SAME session: the un-scanned one under test, and a normal scanned
    // one used ONLY to read the store back (every call on the un-scanned connection is refused, so
    // it cannot witness its own effect).
    let (mut unscanned, unscanned_server, unscanned_cancel) =
        common::connect_raw_unscanned(session.clone(), unblock_mcp::Quotas::default()).await;
    let (mut scanned, scanned_server, scanned_cancel) =
        common::connect_raw(session, unblock_mcp::Quotas::default()).await;

    let before = scanned
        .call_tool("query", serde_json::json!({"kind":"list"}))
        .await;

    // Even a perfectly ordinary, duplicate-free call is refused: the gate cannot tell an un-scanned
    // frame from a hostile one, and guessing is the fail-OPEN direction.
    let response = unscanned
        .call_tool(
            "issue",
            serde_json::json!({"action":"create","title":"must never exist"}),
        )
        .await;
    assert!(
        response.get("error").is_none(),
        "the unscanned reject must still be IN-BAND: {response}"
    );
    let payload = &response["result"]["structuredContent"];
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_eq!(payload["code"], "INTERNAL_ERROR", "{payload}");
    assert_eq!(payload["context"]["kind"], "unscanned_frame", "{payload}");
    assert_eq!(
        payload["retryable"], false,
        "an un-scanned frame is not a transient condition: {payload}"
    );
    // The HINT-SHAPE honesty rule, pinned on the producer (spine §2.4). `INTERNAL_ERROR` advertises
    // `hint_shape: "none"` in the capabilities error map, and that value is frozen in the contract
    // snapshot — so this site must attach NO `hint`. Nothing else observes it: the contract suite
    // compares the descriptor to the const fn and never sees a produced payload, so without these
    // two lines a hint could be re-added here and no test would notice.
    assert!(
        payload["hint"].is_null(),
        "INTERNAL_ERROR advertises hint_shape `none`; attaching a hint here breaks the taxonomy \
         it publishes on the wire: {payload}"
    );
    assert!(
        payload["context"]["diagnostic"].is_string(),
        "the wiring diagnostic must survive on the free-form `context` (which is not hashed), \
         not be dropped along with the hint: {payload}"
    );

    // The effect oracle, read over the SCANNED connection: the create must NOT have run.
    let after = scanned
        .call_tool("query", serde_json::json!({"kind":"list"}))
        .await;
    assert_eq!(
        before["result"]["structuredContent"], after["result"]["structuredContent"],
        "the refused create must have written nothing"
    );
    assert_eq!(
        after["result"]["structuredContent"]["issues"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "sanity: the fixture store must be empty, else the oracle above is vacuous: {after}"
    );

    let _ = unscanned_server.cancel().await;
    unscanned_cancel.cancel();
    let _ = scanned_server.cancel().await;
    scanned_cancel.cancel();
}

// -------------------------------------------------------------------------------------------
// Non-vacuity guards — a corpus that stopped being a corpus must go RED, not quietly pass.
// -------------------------------------------------------------------------------------------

/// **G1 — every cell really IS a flip, and its schema claim is checked, not asserted.**
///
/// If a schema change kills the hidden arm, the cell stops being a flip; this goes RED instead of
/// continuing to "pass" against an input that no longer demonstrates anything.
#[test]
fn g1_every_cell_is_a_real_schema_clean_flip() {
    for cell in CELLS {
        let raw = instantiate(cell.arguments_text, "ub-aaa", "ub-bbb");
        let shown = instantiate(cell.shown, "ub-aaa", "ub-bbb");
        let hidden = instantiate(cell.hidden, "ub-aaa", "ub-bbb");

        let collapsed: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: the cell text must be valid JSON: {e}", cell.id));
        let expected_hidden: Value = serde_json::from_str(&hidden)
            .unwrap_or_else(|e| panic!("{}: `hidden` must be valid JSON: {e}", cell.id));
        let expected_shown: Value = serde_json::from_str(&shown)
            .unwrap_or_else(|e| panic!("{}: `shown` must be valid JSON: {e}", cell.id));

        assert_eq!(
            collapsed, expected_hidden,
            "{}: serde_json must collapse the cell to the HIDDEN arm — if it does not, the cell is \
             not a last-wins flip at all",
            cell.id
        );
        assert_ne!(
            expected_shown, expected_hidden,
            "{}: a cell whose shown and hidden arms are equal proves nothing",
            cell.id
        );

        let shown_parses = parses_as_tool_input(cell.tool, &expected_shown);
        let hidden_parses = parses_as_tool_input(cell.tool, &expected_hidden);
        if cell.both_arms_schema_clean {
            assert!(
                shown_parses.is_ok(),
                "{}: the SHOWN arm must be schema-clean, got {shown_parses:?}",
                cell.id
            );
            assert!(
                hidden_parses.is_ok(),
                "{}: the HIDDEN arm must be schema-clean — otherwise the frame is rejected for an \
                 UNRELATED reason and the cell proves nothing about D43. Got {hidden_parses:?}",
                cell.id
            );
        } else {
            // THE `false` BRANCH IS LIVE TOO. A flag nothing checks is a flag that silently
            // switches the assertions above off, so a cell claiming "not both arms are clean" has
            // to really be that shape: the SHOWN arm rejected by the published schema, and the
            // HIDDEN one — the arm `serde_json` actually builds, and the only reason the cell is
            // dangerous — accepted by it.
            assert!(
                shown_parses.is_err(),
                "{}: the cell claims `both_arms_schema_clean: false`, but its SHOWN arm parses \
                 fine. If the HIDDEN arm parses too, flip the flag to `true` (and get the stronger \
                 guard); if the HIDDEN arm is the broken one, the cell's collapse cannot execute \
                 and it demonstrates nothing.",
                cell.id
            );
            assert!(
                hidden_parses.is_ok(),
                "{}: a ONE-SIDED flip is only harmful when what serde BUILDS runs — the HIDDEN arm \
                 must be schema-clean. Got {hidden_parses:?}",
                cell.id
            );
        }
    }
}

/// **G2 — cover-set EQUALITY against the live `tools/list`, never a literal count.**
///
/// A 9th tool lands RED automatically.
#[tokio::test]
async fn g2_the_corpus_covers_exactly_the_published_tool_set() {
    let session = common::session().await;
    let (mut client, server, cancel) =
        common::connect_raw(session, unblock_mcp::Quotas::default()).await;

    let listed = client.request("tools/list", serde_json::json!({})).await;
    let published: std::collections::BTreeSet<String> = listed["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(ToString::to_string))
        .collect();
    let covered: std::collections::BTreeSet<String> = covered_tools()
        .into_iter()
        .map(ToString::to_string)
        .collect();

    assert_eq!(
        covered, published,
        "the corpus must cover EXACTLY the published tool set — a new tool with no duplicate-key \
         cell is an untested attack surface"
    );

    let _ = server.cancel().await;
    cancel.cancel();
}

/// **G3 — the per-tool ARM floor, derived from the LIVE schema, not hand-listed.**
///
/// Every union arm each tagged tool publishes is either exercised by a corpus cell or named in the
/// exemption set below. The uncovered set is COMPUTED from `tools/list`, so a new arm shows up in it
/// automatically and turns this RED until it is either covered or consciously exempted.
#[tokio::test]
async fn g3_every_published_union_arm_is_covered_or_explicitly_exempt() {
    /// Arms with no corpus cell, each consciously exempted.
    ///
    /// Every entry is `tool:arm`. A duplicate-key flip is a property of the FRAME, not of the arm,
    /// so covering one arm per tool proves the mechanism; these are exempt because covering them
    /// adds fixtures without adding a distinct failure mode.
    const EXEMPT: &[&str] = &[
        // `comment:list`/`comment:delete` and `defer:undefer`/`defer:defer` are NOT here: the two
        // one-sided tag-flip cells cover them — T8 reads as `list` and collapses into `delete`,
        // T9 reads as `undefer` and collapses into `defer`.
        "comment:update",
        "dep:list",
        "dep:tree",
        "dep:cycles",
        "dep:graph",
        "diagnostics:info",
        "diagnostics:where",
        "diagnostics:version",
        "diagnostics:lint",
        "diagnostics:changelog",
        // D45 (v1.0.1) — the 8th `diagnostics` kind. Exempt on the SAME ground as its five
        // siblings above and no new one: a duplicate-key flip is a property of the FRAME, and this
        // arm is parameterless, so it carries strictly less argument surface than the two
        // `diagnostics` arms the corpus already exercises. This entry is a CONSCIOUS exemption, not
        // a silent one — G3 derives the uncovered set from the live schema, so the arm appeared
        // here by itself the moment it was published.
        "diagnostics:dangling",
        "issue:create_bulk",
        "issue:reopen",
        "issue:restore",
        "query:list",
        "query:search",
        "query:count",
        "query:stale",
        "sync:import_bd",
    ];

    let session = common::session().await;
    let (mut client, server, cancel) =
        common::connect_raw(session, unblock_mcp::Quotas::default()).await;

    let listed = client.request("tools/list", serde_json::json!({})).await;
    let mut published_arms: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for tool in listed["result"]["tools"].as_array().expect("tools array") {
        let name = tool["name"].as_str().unwrap_or_default();
        let Some(arms) = tool["inputSchema"]["oneOf"].as_array() else {
            continue; // `claim` is not a union.
        };
        for arm in arms {
            let Some(properties) = arm["properties"].as_object() else {
                continue;
            };
            for tag in ["action", "kind"] {
                if let Some(schema) = properties.get(tag) {
                    if let Some(value) = schema["const"].as_str() {
                        published_arms.insert(format!("{name}:{value}"));
                    } else if let Some(values) = schema["enum"].as_array() {
                        for value in values.iter().filter_map(Value::as_str) {
                            published_arms.insert(format!("{name}:{value}"));
                        }
                    }
                }
            }
        }
    }
    assert!(
        !published_arms.is_empty(),
        "the arm extraction found NOTHING — this guard would pass vacuously. The published schema \
         shape changed; fix the extraction, do not delete the guard. Schema: {}",
        listed["result"]["tools"]
    );

    let mut covered: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for cell in CELLS {
        for arm_json in [cell.shown, cell.hidden] {
            let value: Value = serde_json::from_str(&instantiate(arm_json, "ub-a", "ub-b"))
                .expect("arm json parses");
            for tag in ["action", "kind"] {
                if let Some(arm) = value[tag].as_str() {
                    covered.insert(format!("{}:{arm}", cell.tool));
                }
            }
        }
    }

    let exempt: std::collections::BTreeSet<String> =
        EXEMPT.iter().map(ToString::to_string).collect();
    let uncovered: std::collections::BTreeSet<String> =
        published_arms.difference(&covered).cloned().collect();
    assert_eq!(
        uncovered, exempt,
        "the set of published union arms with NO corpus cell must equal the declared exemption \
         set. A new arm appears here automatically; cover it or exempt it consciously."
    );

    let _ = server.cancel().await;
    cancel.cancel();
}

/// A compile-time witness that the corpus type is nameable from another crate (it is shared with
/// `unblock-cli`'s raw-stdio suite, which must see the SAME cells).
#[test]
fn the_corpus_is_shareable() {
    let cell: &FlipCell = &CELLS[0];
    assert!(!cell.arguments_text.is_empty());
}
