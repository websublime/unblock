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

/// THE ACCEPTANCE-CRITERION CELL, REWRITTEN AT D45: a create whose `deps` reference a NON-EXISTENT
/// blocker is REFUSED at the wire, with `ISSUE_NOT_FOUND` and zero writes.
///
/// # Why this cell was rewritten rather than re-balanced (D45, tracker `ub-lp9.25`)
///
/// It shipped at D44 written to ANTICIPATE this change, branching on `is_error`: one branch for the
/// then-current acceptance, one for the refusal to come. That shape is exactly the defect this
/// repository has shipped before - a cell that passes in BOTH worlds. Once the refusal landed, the
/// error branch asserted only that the issue count had not moved, which D45's whole-transaction
/// rollback satisfies trivially, the D44 anchoring assertions in the other branch became dead code,
/// and the docstring ("it is not refused today ... when `ub-lp9.25` lands, this cell is REWRITTEN")
/// became false prose in a green suite. So the branch is DELETED, not re-balanced: the refusal is
/// asserted UNCONDITIONALLY.
///
/// What D45 guarantees here, and what is asserted: a `depends_on_id` that names no issue row and is
/// not an `external:` target is rejected INSIDE the create transaction (`insert_issue_in_tx`, the
/// shared per-record insert body), so nothing at all is persisted - no issue, no edge - and the
/// error names BOTH ids, the dependent and the missing target, because on a batch path the target
/// alone does not say which record declared it. D44's own guarantee is preserved by construction and
/// is still pinned by the three sibling cells above: the create is ONE act, and a declared edge is
/// anchored on the minted id.
///
/// MUTANT KILLED: deleting the target-existence guard from the shared per-record insert body
/// (`crates/unblock-storage/src/libsql/crud.rs`). The call then succeeds, `is_error` goes red, and
/// the mint count goes red with it.
///
/// MUTANT KILLED (the weaker variant this rewrite exists to forbid): re-introducing the `is_error`
/// branch. Any build that ACCEPTS this payload now fails on the first assertion instead of silently
/// taking the other arm.
#[test]
fn a_create_whose_blocker_does_not_exist_is_refused_and_persists_nothing() {
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

    assert!(
        is_error,
        "a declared blocker that names no row and is not an `external:` target must be REFUSED \
         (D45): {payload}"
    );
    assert_eq!(payload["code"], "ISSUE_NOT_FOUND", "{payload}");
    assert_eq!(
        payload["context"]["blocker_id"], "ub-no-such-issue",
        "the refusal names the MISSING TARGET so the caller can fix the input from the message \
         alone: {payload}"
    );
    assert!(
        payload["context"]["issue_id"].is_string(),
        "and it names the DEPENDENT that declared the edge - on a batch path the target alone does \
         not say which record carried it: {payload}"
    );

    // ZERO writes: no issue...
    assert_eq!(
        count(&mut client),
        count_before,
        "a refused create persists NOTHING - the guard runs INSIDE the create transaction: {payload}"
    );
    // ...and no edge anywhere, which `dep {action:"cycles"}` would surface as a graph the store
    // still carries. The whole-graph read is the only client-visible way to ask "is there an edge I
    // cannot see from any issue", since the refused issue has no id to list edges for.
    let (_, graph) = client.call_tool("dep", &json!({"action": "graph"}));
    assert_eq!(
        graph["edges"].as_array().map(Vec::len),
        Some(0),
        "and no edge survived the rollback: {graph}"
    );

    client.close_stdin();
    let status = common::wait_for(&mut client.child, std::time::Duration::from_secs(20));
    assert_eq!(status.code(), Some(0), "clean EOF exit 0");
}
