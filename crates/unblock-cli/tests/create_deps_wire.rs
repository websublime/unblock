//! D44 - `issue create {deps:[...]}` over the LIVE JSON-RPC stdio wire, against a real `unblock mcp`
//! CHILD PROCESS (PRD §4 D44; tracker `ub-lp9.20`).
//!
//! # Why this file exists at the wire level and not in-process
//!
//! The tracked defect was REPRODUCED this way - a real server child, a real workspace, real frames -
//! and its acceptance criteria demand the regression test be at the same level. An in-process test
//! shares the harness assumptions of the code under test; it cannot prove what a client actually
//! receives, and the three GA outcomes were all about exactly that: what came back on the wire while
//! the store said something else.
//!
//! It lives in `unblock-cli` because only this crate can spawn `unblock mcp` (`CARGO_BIN_EXE_unblock`);
//! an `unblock-engine` test doing so would be an L5-to-L7 back-edge the layering check rejects. It
//! reuses the `common::McpClient` spawn harness, whose `initialize` handshake is the readiness
//! barrier.
//!
//! Unix-only: `unblock mcp` is a no-op EOF path on Windows (NFR-11), the `mcp_lifecycle.rs`
//! precedent.
//!
//! Every cell names the MUTANT it kills.
#![cfg(unix)]

mod common;

use common::{McpClient, Workspace, id_set, issue_id};
use serde_json::{Value, json};

/// Create a titled issue through the wire and return its minted id.
fn create(client: &mut McpClient, title: &str) -> String {
    let (is_error, created) =
        client.call_tool("issue", &json!({"action": "create", "title": title}));
    assert!(!is_error, "setup create must succeed: {created}");
    issue_id(&created)
}

/// `issue show` for `id`, as the client sees it.
fn show(client: &mut McpClient, id: &str) -> Value {
    let (is_error, shown) = client.call_tool("issue", &json!({"action": "show", "id": id}));
    assert!(!is_error, "show must succeed: {shown}");
    shown
}

/// How many issues exist right now.
fn count(client: &mut McpClient) -> usize {
    let (_, listed) = client.call_tool("query", &json!({"kind": "list"}));
    id_set(&listed).len()
}

/// THE HEADLINE REGRESSION, at the wire. A create whose `deps[0].issue_id` names an EXISTING,
/// UNRELATED issue is refused with `VALIDATION_FAILED`; the victim gains no edge, its `updated_at`
/// does not move, it stays in the ready set, and nothing at all was minted.
///
/// This is outcome (2) of the live GA reproduction, and it was the dangerous one precisely because it
/// returned `isError:false`. The edge landed on the third party, which silently dropped out of the
/// ready set while its own change-detection fields stayed still - so no staleness query, no
/// `content_hash` comparison and no cross-view disagreement could ever surface it. A second server
/// process was then handed the wrongly-ready issue and claimed it cleanly, with no race involved.
///
/// MUTANT KILLED (the GA behaviour): an adapter or engine that uses the client-supplied source for
/// the edge write. The victim assertions go red - it gains an edge and leaves the ready set.
///
/// MUTANT KILLED (the dropper): an adapter that silently ignores the field and creates the issue
/// anyway. `isError` and the mint count both go red.
///
/// Why `updated_at` is asserted explicitly rather than left to the edge check: the absence of a
/// timestamp bump is what made the GA corruption UNDETECTABLE, so it is pinned as its own fact.
#[test]
fn a_create_naming_a_third_party_as_the_edge_source_is_refused_and_the_victim_is_untouched() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let victim = create(&mut client, "an unrelated third party");
    let blocker = create(&mut client, "the blocker");
    let victim_before = show(&mut client, &victim);
    let count_before = count(&mut client);

    let (is_error, payload) = client.call_tool(
        "issue",
        &json!({
            "action": "create",
            "title": "names the third party as its edge source",
            "deps": [{
                "issue_id": victim,
                "depends_on_id": blocker,
                "dep_type": "blocks"
            }]
        }),
    );

    assert!(
        is_error,
        "the GA payload shape must now be REFUSED: {payload}"
    );
    assert_eq!(payload["code"], "VALIDATION_FAILED", "{payload}");
    assert_eq!(payload["context"]["field"], "deps[0].issue_id", "{payload}");

    // The victim graph is untouched...
    let (_, victim_deps) = client.call_tool("dep", &json!({"action": "list", "id": victim}));
    assert_eq!(
        victim_deps["deps"].as_array().map(Vec::len),
        Some(0),
        "the third party must gain NO edge: {victim_deps}"
    );
    // ...its row did not move at all...
    assert_eq!(
        show(&mut client, &victim),
        victim_before,
        "the third party row must be byte-identical, `updated_at` and `content_hash` included"
    );
    // ...it is still offered as ready...
    let (_, ready) = client.call_tool("query", &json!({"kind": "ready"}));
    assert!(
        id_set(&ready).contains(&victim),
        "the third party must not silently drop out of the ready set: {ready}"
    );
    // ...and nothing was minted.
    assert_eq!(
        count(&mut client),
        count_before,
        "a refused create must mint NOTHING"
    );

    client.close_stdin();
    let status = common::wait_for(&mut client.child, std::time::Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "clean EOF exit 0");
}

/// THE READY-SET CONSEQUENCE, at the wire. An issue created with a declared `blocks` edge is NOT
/// returned by `query {kind: ready}` while its blocker is open, and IS returned once it closes.
///
/// This is the acceptance criterion that matters operationally: `ready` is the tool an agent calls to
/// pick up work, so an edge dropped during create does not merely lose data - it hands out an issue
/// whose stated blocker is still open. The GA build did exactly that, and nothing detected it:
/// `doctor` reported integrity ok, the foreign-key and integrity pragmas passed, the cycle report was
/// clean, and the ready and blocked views agreed with each other.
///
/// MUTANT KILLED: any build that drops a declared edge - the GA engine (an empty seeded list plus a
/// follow-up pass whose foreign key failed), an adapter mapping `deps` to nothing, or a storage body
/// skipping the seeded edges. All of them put the new id in the FIRST ready set.
///
/// The second half (ready after the close) is what makes the first non-vacuous: an issue missing from
/// the ready set for an unrelated reason would otherwise pass.
#[test]
fn an_issue_created_with_a_declared_blocker_is_not_offered_as_ready() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let blocker = create(&mut client, "the blocker");
    let (is_error, created) = client.call_tool(
        "issue",
        &json!({
            "action": "create",
            "title": "blocked from the moment it exists",
            "deps": [{"depends_on_id": blocker, "dep_type": "blocks"}]
        }),
    );
    assert!(!is_error, "the canonical create must succeed: {created}");
    let id = issue_id(&created);

    let (_, ready) = client.call_tool("query", &json!({"kind": "ready"}));
    let ready_ids = id_set(&ready);
    assert!(
        !ready_ids.contains(&id),
        "an issue whose declared blocker is OPEN must not be offered as ready: {ready}"
    );
    assert!(
        ready_ids.contains(&blocker),
        "the blocker itself is ready: {ready}"
    );

    let (is_error, closed) = client.call_tool("issue", &json!({"action": "close", "id": blocker}));
    assert!(!is_error, "close must succeed: {closed}");

    let (_, ready_after) = client.call_tool("query", &json!({"kind": "ready"}));
    assert!(
        id_set(&ready_after).contains(&id),
        "closing the blocker unblocks it, proving the declared edge was REAL: {ready_after}"
    );

    client.close_stdin();
    let status = common::wait_for(&mut client.child, std::time::Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "clean EOF exit 0");
}

/// A create whose declared edge cannot be satisfied persists NOTHING - asserted at the wire, where
/// the GA build returned an error while the issue row was already committed and already ready.
///
/// The unsatisfiable edge here is a declared GATING CYCLE, which is the shape a client can actually
/// construct now that the edge source is implicit. Setup: `far` blocks-depends on `parent`; the new
/// issue declares a `parent-child` edge to `parent` (an IN-edge under the D4 reversal) and a `blocks`
/// edge to `far` (an OUT-edge), closing the ring. The call must come back with `CYCLE_DETECTED` and
/// the store must be exactly as it was.
///
/// MUTANT KILLED: the GA shape - commit the row, then write each declared edge in its own follow-up
/// transaction. Under it the row and the first edge are already durable when the second is refused,
/// so `count` grows and `show` finds the orphan. The GA reproduction showed the practical cost: the
/// error is marked non-retryable but says nothing about the committed row, so a client that retries
/// anyway mints a fresh ready orphan per attempt.
#[test]
fn a_create_whose_declared_edges_are_refused_leaves_the_store_untouched() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let parent = create(&mut client, "parent");
    let far = create(&mut client, "far");
    let (is_error, added) = client.call_tool(
        "dep",
        &json!({"action": "add", "issue_id": far, "depends_on_id": parent, "dep_type": "blocks"}),
    );
    assert!(!is_error, "setup edge: {added}");

    let count_before = count(&mut client);
    let (_, ready_before) = client.call_tool("query", &json!({"kind": "ready"}));
    let ready_before = id_set(&ready_before);

    let (is_error, payload) = client.call_tool(
        "issue",
        &json!({
            "action": "create",
            "title": "closes a gating cycle",
            "deps": [
                {"depends_on_id": parent, "dep_type": "parent-child"},
                {"depends_on_id": far, "dep_type": "blocks"}
            ]
        }),
    );

    assert!(
        is_error,
        "the declared edges close a gating cycle: {payload}"
    );
    assert_eq!(payload["code"], "CYCLE_DETECTED", "{payload}");

    assert_eq!(
        count(&mut client),
        count_before,
        "ZERO rows persist - no orphan issue behind the error"
    );
    let (_, ready_after) = client.call_tool("query", &json!({"kind": "ready"}));
    assert_eq!(
        id_set(&ready_after),
        ready_before,
        "and the ready set is unchanged, so no orphan was offered to the next agent"
    );

    client.close_stdin();
    let status = common::wait_for(&mut client.child, std::time::Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "clean EOF exit 0");
}

/// THE ACCEPTANCE-CRITERION CELL: a create whose `deps` reference a NON-EXISTENT blocker, driven at
/// the wire, pinning exactly what D44 guarantees for that input and nothing more.
///
/// What D44 guarantees, and what is asserted: the create is ONE act - either the issue and its edge
/// are both there or neither is - and the edge is anchored on the id the server minted, never on
/// anything the caller chose. At GA this same payload was the loud outcome (1): a foreign-key failure
/// on the SOURCE column, raised after the issue row had already committed, edgeless and already in
/// the ready set. That is what these assertions refuse.
///
/// What is deliberately NOT asserted, so this cell cannot be misread as a claim: whether a
/// `depends_on_id` naming an issue that does not exist should be REFUSED. It is not refused today,
/// and D44 explicitly scopes that class out - it is a different defect with a different shape (the
/// blocker column carries no foreign key BY DESIGN, because `external:` targets are legitimate) and
/// it is tracked as `ub-lp9.25`, ruled to ship in the same 1.0.1 cut. When `ub-lp9.25` lands, the
/// call below starts failing and this cell is REWRITTEN to assert the refusal plus zero writes - its
/// failure at that point is the signal, not a regression.
///
/// MUTANT KILLED: the GA shape on this exact payload - a follow-up edge pass anchored on a
/// client-supplied source. It produced an error frame plus a committed, edgeless, ready orphan; here
/// the call succeeds, the edge is present, and it is anchored on the minted id.
#[test]
fn a_create_whose_blocker_does_not_exist_is_still_one_act_anchored_on_the_minted_id() {
    let ws = Workspace::init();
    let mut client = McpClient::spawn(ws.root());
    client.initialize();

    let count_before = count(&mut client);

    let (is_error, payload) = client.call_tool(
        "issue",
        &json!({
            "action": "create",
            "title": "declares a blocker that does not exist",
            "deps": [{"depends_on_id": "ub-no-such-issue", "dep_type": "blocks"}]
        }),
    );

    // ONE act: the outcome is all-or-nothing, never a row without its edge.
    if is_error {
        assert_eq!(
            count(&mut client),
            count_before,
            "if the create fails it must persist NOTHING: {payload}"
        );
    } else {
        let id = issue_id(&payload);
        assert_eq!(
            count(&mut client),
            count_before + 1,
            "exactly one issue was created: {payload}"
        );
        let (_, edges) = client.call_tool("dep", &json!({"action": "list", "id": id}));
        let listed = edges["deps"].as_array().expect("deps");
        assert_eq!(
            listed.len(),
            1,
            "the issue and its declared edge landed together - never the row alone: {edges}"
        );
        assert_eq!(
            listed[0]["issue_id"], id,
            "and the edge is anchored on the MINTED id, not on any caller-chosen source: {edges}"
        );
        assert_eq!(listed[0]["depends_on_id"], "ub-no-such-issue", "{edges}");
    }

    client.close_stdin();
    let status = common::wait_for(&mut client.child, std::time::Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "clean EOF exit 0");
}
