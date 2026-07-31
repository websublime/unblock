//! D42 - `dep.metadata` on the WIRE - plus the D44 create-with-deps contract at the MCP boundary.
//!
//! `DepInput.metadata` was accepted, typed and schema-published, then discarded by a 5-column
//! `INSERT` at L2. Live-reproduced before the fix: `dep add {metadata:"..."}` returned
//! `{"added":true}`, `select quote(metadata)` showed the empty object, and `dep list` returned a
//! 5-key edge.
//!
//! Since D44 this file also carries the `issue create {deps:[...]}` boundary contract: `deps[i]` is
//! sourced IMPLICITLY on the issue being created, a client-supplied source is REFUSED, and the
//! declared edge round-trips anchored on the minted id with its `metadata` intact. The last test in
//! this file used to assert the OPPOSITE, under an instruction to rewrite it when the tracked defect
//! was fixed; that instruction has been carried out.

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

// ------------------------------------------------------------------------------------------------
// D44 - `issue create {deps:[...]}` at the MCP boundary
//
// This section REPLACES the test that used to live here. That test asserted the GA behaviour on
// purpose and carried an instruction: when the tracked defect is fixed, REWRITE it to assert the
// repaired behaviour rather than delete it. D44 is that fix, and this is that rewrite. Each cell
// names the MUTANT it kills.
// ------------------------------------------------------------------------------------------------

/// Create a titled issue and return its id.
async fn quick_create(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
    title: &str,
) -> String {
    let (is_error, payload) = call_tool(
        client,
        "issue",
        json!({ "action": "create", "title": title, "quick": true }),
    )
    .await;
    assert!(!is_error, "setup create: {payload}");
    payload["id"].as_str().expect("id").to_string()
}

/// How many issues exist right now.
async fn issue_count(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
) -> usize {
    let (_, listed) = call_tool(client, "query", json!({ "kind": "list" })).await;
    listed["issues"].as_array().expect("issues").len()
}

/// A create carrying a PRESENT `deps[i].issue_id` is REFUSED with `VALIDATION_FAILED`, the hint
/// names the offending field and the two-step workaround, and the ENGINE IS NEVER REACHED: nothing
/// is minted and the issue the field named is untouched.
///
/// The last clause is the load-bearing one. Every other cell in this cascade is satisfied by an
/// adapter that simply DROPS the field, so without an assertion that the call is REFUSED and that
/// nothing was written, the headline behaviour could ship unimplemented and green.
///
/// How "the engine is never reached" is established, since a test cannot see a call it did not make:
/// the error code carries `context.kind = dep_source_not_allowed`, a marker only the L7 adapter
/// emits, AND the store gained zero issues. Under D44 no engine-side failure mode exists for this
/// payload (the engine carrier `NewDep` has no source field at all), so a create that produced no
/// issue and an adapter-only marker did not run the mint.
///
/// MUTANT KILLED (the dropper): an adapter that ignores a present `issue_id` and proceeds. It
/// returns `isError:false` and mints an issue - both assertions go red.
///
/// MUTANT KILLED (the forwarder): an adapter that passes the client source through to the edge
/// write. That is the GA defect: the third party silently gains the edge, drops out of the ready
/// set, and its `updated_at` does not move - so the two victim assertions are the regression pin for
/// the exact live-reproduced corruption.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_carrying_a_dep_source_is_refused_and_writes_nothing() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let victim = quick_create(&client, "an unrelated third party").await;
    let blocker = quick_create(&client, "the blocker").await;
    let before = issue_count(&client).await;

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({
            "action": "create",
            "title": "names a third party as the edge source",
            "deps": [{
                "issue_id": victim,
                "depends_on_id": blocker,
                "dep_type": "blocks"
            }]
        }),
    )
    .await;

    assert!(
        is_error,
        "a present `deps[i].issue_id` must be REFUSED: {payload}"
    );
    assert_eq!(payload["code"], "VALIDATION_FAILED", "{payload}");
    assert_eq!(
        payload["context"]["kind"], "dep_source_not_allowed",
        "{payload}"
    );
    assert_eq!(payload["context"]["field"], "deps[0].issue_id", "{payload}");
    let hint = payload["hint"].as_str().expect("a hint");
    assert!(
        hint.contains("deps[0].issue_id"),
        "the hint must name the offending field: {hint}"
    );
    assert!(
        hint.contains("dep {action:") && hint.contains("add"),
        "the hint must name the `dep` add action as the documented workaround: {hint}"
    );

    // The engine was never reached: nothing minted...
    assert_eq!(
        issue_count(&client).await,
        before,
        "a refused create must mint NOTHING: {payload}"
    );
    // ...and the issue the field named is untouched.
    let (_, victim_deps) =
        call_tool(&client, "dep", json!({ "action": "list", "id": victim })).await;
    assert_eq!(
        victim_deps["deps"].as_array().map(Vec::len),
        Some(0),
        "the named third party must gain NO edge: {victim_deps}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// An EXPLICIT JSON `null` for `deps[i].issue_id` is an ABSENCE and is ACCEPTED.
///
/// D44 rules the field OPTIONAL with absence as the canonical form, and states that a literal
/// `null` is the JSON spelling of omitted - demanding a doubly-optional type to tell the two apart
/// would widen the published schema for no integrity gain.
///
/// MUTANT KILLED: a rejection implemented over the RAW JSON arguments - scanning `deps[i]` for the
/// KEY before `parse_args` runs - rather than over the deserialized `Option`. A key scan reads an
/// explicit `null` as present and refuses this create, breaking a payload the decision explicitly
/// admits. Verified: built, this cell went red while every other cell stayed green.
///
/// NOT a mutant, and worth recording so nobody reaches for it: retyping the field to
/// `Option<serde_json::Value>` does NOT distinguish the two either. Serde maps a JSON `null` onto
/// `None` for `Option<T>` whatever `T` is, so that change is behaviour-neutral here - it was tried,
/// and this cell stayed green. Only a doubly-optional type or a pre-parse key scan can tell them
/// apart, and D44 declines both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_null_dep_source_is_an_absence_and_is_accepted() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let blocker = quick_create(&client, "the blocker").await;

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({
            "action": "create",
            "title": "sends an explicit null source",
            "deps": [{
                "issue_id": serde_json::Value::Null,
                "depends_on_id": blocker,
                "dep_type": "blocks"
            }]
        }),
    )
    .await;

    assert!(
        !is_error,
        "an explicit JSON null is the spelling of `omitted` and must be ACCEPTED: {payload}"
    );
    let created = payload["id"].as_str().expect("the created id");
    let edges = payload["dependencies"].as_array().expect("dependencies");
    assert_eq!(edges.len(), 1, "the declared edge landed: {payload}");
    assert_eq!(
        edges[0]["issue_id"], created,
        "and it is anchored on the MINTED id: {payload}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// THE REPAIRED HAPPY PATH. With `deps[i].issue_id` OMITTED, the declared edge round-trips over the
/// wire: the created issue comes back carrying it, anchored on the id the server minted, with its
/// `metadata` intact - and a `dep list` on the new id agrees.
///
/// The retired test in this position ended by stating that no `dep.metadata` round-trip was claimed
/// for `create.deps`. It is claimed now, and this asserts it.
///
/// MUTANT KILLED (edge dropped): the GA engine, which built the issue with an empty dependency list
/// and wrote the edges afterwards. The returned `["dependencies"]` array is empty under it.
///
/// MUTANT KILLED (metadata dropped): a create-arm mapping that discards `DepInput.metadata` on the
/// way to the engine carrier. At L2 that loss is masked in both directions, so only a wire-level
/// round-trip assertion can see it.
///
/// MUTANT KILLED (wrong anchor): an engine stamping anything other than the minted id. The
/// `issue_id` on the returned edge is compared against the returned `id`, which is the entire
/// implicit-ownership contract expressed at the boundary the client actually sees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_with_an_omitted_dep_source_round_trips_the_edge_and_its_metadata() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let blocker = quick_create(&client, "the blocker").await;

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({
            "action": "create",
            "title": "omits the source, as D44 requires",
            "deps": [{
                "depends_on_id": blocker,
                "dep_type": "blocks",
                "metadata": "{\"why\":\"KEEP-ME\"}"
            }]
        }),
    )
    .await;
    assert!(!is_error, "the canonical form must be ACCEPTED: {payload}");

    let created = payload["id"].as_str().expect("the created id").to_string();
    let edges = payload["dependencies"].as_array().expect("dependencies");
    assert_eq!(edges.len(), 1, "the declared edge landed: {payload}");
    assert_eq!(
        edges[0]["issue_id"], created,
        "anchored on the MINTED id: {payload}"
    );
    assert_eq!(edges[0]["depends_on_id"], blocker.as_str(), "{payload}");
    assert_eq!(
        edges[0]["metadata"], "{\"why\":\"KEEP-ME\"}",
        "`metadata` must survive the create path, not only the `dep` add path: {payload}"
    );

    // The independent read agrees with the create response.
    let (_, listed) = call_tool(&client, "dep", json!({ "action": "list", "id": created })).await;
    assert_eq!(
        listed["deps"][0]["metadata"], "{\"why\":\"KEEP-ME\"}",
        "and it is DURABLE, not merely echoed by the create response: {listed}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
