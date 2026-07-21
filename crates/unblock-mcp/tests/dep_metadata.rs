//! D42 — `dep.metadata` on the WIRE, plus the pinned known-failure for the `create.deps` path.
//!
//! `DepInput.metadata` was accepted, typed and schema-published, then discarded by a 5-column
//! `INSERT` at L2. Live-reproduced before the fix: `dep add {metadata:"…"}` returned
//! `{"added":true}`, `select quote(metadata)` showed `'{}'`, and `dep list` returned a 5-key edge.

mod common;

use common::{call_tool, connect, session};
use serde_json::json;

/// The wire round-trip: `dep add {metadata}` then `dep list` returns it. Fails against either
/// 5-column INSERT.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dep_add_metadata_is_returned_by_dep_list() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    for title in ["A", "B"] {
        let (is_error, _) = call_tool(
            &client,
            "issue",
            json!({ "action": "create", "title": title, "quick": true }),
        )
        .await;
        assert!(!is_error, "setup create");
    }
    let (_, listed) = call_tool(&client, "query", json!({ "kind": "list" })).await;
    let ids: Vec<String> = listed["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .map(|i| i["id"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(ids.len(), 2);

    let (is_error, payload) = call_tool(
        &client,
        "dep",
        json!({
            "action": "add",
            "issue_id": ids[0],
            "depends_on_id": ids[1],
            "dep_type": "blocks",
            "metadata": "{\"why\":\"PROBE-KEEPME\"}"
        }),
    )
    .await;
    assert!(!is_error, "dep add: {payload}");

    let (is_error, payload) =
        call_tool(&client, "dep", json!({ "action": "list", "id": ids[0] })).await;
    assert!(!is_error, "dep list: {payload}");
    let edge = payload["deps"][0].as_object().expect("a dep object");
    assert_eq!(
        edge.get("metadata").and_then(serde_json::Value::as_str),
        Some("{\"why\":\"PROBE-KEEPME\"}"),
        "`dep.metadata` must survive the write — before D42 the INSERT bound 5 of 7 columns and the \
         `DEFAULT '{{}}'` + read-filter pair made the loss invisible: {edge:?}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// A dep added WITHOUT metadata still comes back without the key — the additive
/// `skip_serializing_if` shape is unchanged for existing records.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dep_without_metadata_keeps_its_shape() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    for title in ["A", "B"] {
        let _ = call_tool(
            &client,
            "issue",
            json!({ "action": "create", "title": title, "quick": true }),
        )
        .await;
    }
    let (_, listed) = call_tool(&client, "query", json!({ "kind": "list" })).await;
    let ids: Vec<String> = listed["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .map(|i| i["id"].as_str().expect("id").to_string())
        .collect();

    let _ = call_tool(
        &client,
        "dep",
        json!({ "action": "add", "issue_id": ids[0], "depends_on_id": ids[1], "dep_type": "blocks" }),
    )
    .await;
    let (_, payload) = call_tool(&client, "dep", json!({ "action": "list", "id": ids[0] })).await;
    let edge = payload["deps"][0].as_object().expect("a dep object");
    assert!(
        !edge.contains_key("metadata"),
        "absent metadata must stay ABSENT (binding `'{{}}'` rather than SQL NULL would surface it): \
         {edge:?}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// **PINNED KNOWN FAILURE — this asserts the CURRENT, BROKEN behaviour on purpose.**
///
/// `issue create {deps:[…]}` is a PRE-EXISTING GA defect that D42 deliberately does NOT fix:
/// `Session::create_issue` inserts the issue and THEN loops `storage.add_dependency` in a separate
/// call, so the operation is NON-ATOMIC; and `dep.issue_id` comes verbatim from the client, which
/// cannot know the server-minted id, so the FK fails AFTER the issue row has been committed.
///
/// It is written as a test rather than left invisible so the defect is visible in the suite. **It
/// must NOT be read as asserting that the behaviour is correct.** When the tracked issue is fixed,
/// this test is REWRITTEN to assert the repaired behaviour — its failure at that point is the
/// signal, not a regression.
///
/// Consequently: **no `dep.metadata` round-trip is claimed for `create.deps`.** The round-trip
/// D42 does deliver is on the `dep` tool path and on any pre-built `Issue.dependencies` (the JSONL
/// and bd-import legs, and `Session::create(&Issue)`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_create_with_deps_is_still_non_atomic_and_fk_fails() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let (_, blocker) = call_tool(
        &client,
        "issue",
        json!({ "action": "create", "title": "blocker", "quick": true }),
    )
    .await;
    let blocker_id = blocker["id"].as_str().expect("id").to_string();

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({
            "action": "create",
            "title": "with deps",
            "deps": [{
                "issue_id": "ub-client-cannot-know-the-minted-id",
                "depends_on_id": blocker_id,
                "dep_type": "blocks",
                "metadata": "{\"why\":\"unreachable today\"}"
            }]
        }),
    )
    .await;

    assert!(
        is_error,
        "CURRENT behaviour, tracked separately and NOT fixed by D42: the dep loop runs after the \
         issue row is already committed, and the client cannot supply the minted id, so the FK \
         fails. If this ever goes GREEN the tracked defect was fixed — REWRITE this test rather \
         than deleting it: {payload}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
