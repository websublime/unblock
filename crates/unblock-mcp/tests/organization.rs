//! The FR-6 remainder over the live MCP surface (PRD §4 D56 clause (7), spine §5.1–§5.3): the four
//! discriminator arms `query kind: labels` / `epics` and `issue action: relabel` / `close_eligible`,
//! driven end to end through the real server over a duplex transport, plus the D56 (n) contract
//! checks — the arms sit LAST in their published `oneOf`, the tool count stays 8, an unknown
//! argument on any new arm is rejected in-band with ZERO writes as `RecordingStorage` observes
//! them, and `agent_name`/`harness`/`model` are accepted and discarded on both mutating arms.

mod common;

use common::{call_tool, connect, session, session_recording};
use serde_json::{Value, json};

/// `issue create` through the live client, returning the minted id.
async fn create(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
    args: Value,
) -> String {
    let (is_error, payload) = call_tool(client, "issue", args).await;
    assert!(!is_error, "fixture create must succeed: {payload}");
    payload["id"].as_str().expect("minted id").to_string()
}

/// The create-time `deps` entry declaring the `parent-child` edge to `parent` (a `parent` field only
/// mints the hierarchical `parent.N` id; the rollup counts EDGES, D26).
fn child_of(parent: &str) -> Value {
    json!([{"depends_on_id": parent, "dep_type": "parent-child"}])
}

/// The discriminant VALUE an input `oneOf` arm publishes for `key` (`const` or a one-member `enum`).
fn arm_discriminant(arm: &Value, key: &str) -> String {
    let prop = &arm["properties"][key];
    prop.get("const")
        .or_else(|| prop["enum"].get(0))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("arm without a `{key}` discriminant: {arm}"))
        .to_string()
}

/// D56 (g)/(h)/(i) over the wire: `query labels` returns `{"labels":[{label,count}…]}` and `query
/// epics` returns `{"epics":[EpicStatus…]}` with the FULL issue embedded; by-id errors are in-band
/// `ISSUE_NOT_FOUND` / `VALIDATION_FAILED`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d56_query_labels_and_epics_round_trip() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let epic = create(
        &client,
        json!({"action":"create","title":"epic","issue_type":"epic","labels":["b-label"]}),
    )
    .await;
    let child = create(
        &client,
        json!({"action":"create","title":"child","parent":epic,"deps":child_of(&epic),"labels":["A-label","b-label"]}),
    )
    .await;
    let task = create(&client, json!({"action":"create","title":"task"})).await;

    let (is_error, payload) = call_tool(&client, "query", json!({"kind":"labels"})).await;
    assert!(!is_error, "{payload}");
    assert_eq!(
        payload,
        json!({"labels":[{"label":"A-label","count":1},{"label":"b-label","count":2}]})
    );

    let (is_error, payload) = call_tool(&client, "query", json!({"kind":"epics"})).await;
    assert!(!is_error, "{payload}");
    let epics = payload["epics"].as_array().expect("object-wrapped list");
    assert_eq!(epics.len(), 1);
    assert_eq!(epics[0]["epic"]["id"], json!(epic));
    assert_eq!(epics[0]["epic"]["title"], json!("epic"), "the full issue");
    assert_eq!(epics[0]["total_children"], json!(1));
    assert_eq!(epics[0]["closed_children"], json!(0));
    assert_eq!(epics[0]["eligible_for_close"], json!(false));

    let (is_error, _) = call_tool(&client, "issue", json!({"action":"close","id":child})).await;
    assert!(!is_error);
    let (is_error, payload) = call_tool(
        &client,
        "query",
        json!({"kind":"epics","id":epic,"eligible_only":true}),
    )
    .await;
    assert!(!is_error, "{payload}");
    assert_eq!(payload["epics"][0]["eligible_for_close"], json!(true));

    let (is_error, payload) =
        call_tool(&client, "query", json!({"kind":"epics","id":"ub-nope"})).await;
    assert!(is_error);
    assert_eq!(payload["code"], json!("ISSUE_NOT_FOUND"), "{payload}");
    let (is_error, payload) = call_tool(&client, "query", json!({"kind":"epics","id":task})).await;
    assert!(is_error);
    assert_eq!(payload["code"], json!("VALIDATION_FAILED"), "{payload}");
    assert_eq!(payload["retryable"], json!(true));
    assert!(
        payload.to_string().contains("not an epic"),
        "the field reason names the type mismatch: {payload}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// D56 (a)/(b)/(k)/(l)/(o) over the wire: `relabel` returns `{"old_name","new_name","affected_issues"}`;
/// `close_eligible` returns `{"epic_ids","count","dry_run"}` — the dry run lists the cascade and the
/// real run closes it, child epic before parent — and NEVER the `close` arm's `closed` key.
/// Attribution is accepted and discarded on both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d56_relabel_and_close_eligible_round_trip() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let parent = create(
        &client,
        json!({"action":"create","title":"A","issue_type":"epic"}),
    )
    .await;
    let nested = create(
        &client,
        json!({"action":"create","title":"B","issue_type":"epic","parent":parent,"deps":child_of(&parent)}),
    )
    .await;
    let leaf = create(
        &client,
        json!({"action":"create","title":"leaf","parent":nested,"deps":child_of(&nested),"labels":["old"]}),
    )
    .await;
    let (is_error, _) = call_tool(&client, "issue", json!({"action":"close","id":leaf})).await;
    assert!(!is_error);

    let attribution = json!({"agent_name":"bot","harness":"h","model":"m"});
    let mut relabel = json!({"action":"relabel","old_name":"old","new_name":"new"});
    relabel
        .as_object_mut()
        .unwrap()
        .extend(attribution.as_object().unwrap().clone());
    let (is_error, payload) = call_tool(&client, "issue", relabel).await;
    assert!(!is_error, "{payload}");
    assert_eq!(
        payload,
        json!({"old_name":"old","new_name":"new","affected_issues":1})
    );

    let mut dry = json!({"action":"close_eligible","dry_run":true});
    dry.as_object_mut()
        .unwrap()
        .extend(attribution.as_object().unwrap().clone());
    let (is_error, payload) = call_tool(&client, "issue", dry).await;
    assert!(!is_error, "{payload}");
    assert_eq!(
        payload,
        json!({"epic_ids":[nested, parent],"count":2,"dry_run":true})
    );
    assert!(payload.get("closed").is_none());

    let (is_error, payload) = call_tool(&client, "issue", json!({"action":"close_eligible"})).await;
    assert!(!is_error, "{payload}");
    assert_eq!(
        payload,
        json!({"epic_ids":[nested, parent],"count":2,"dry_run":false}),
        "dry_run defaults to false; no newly_unblocked"
    );
    let (_, shown) = call_tool(&client, "issue", json!({"action":"show","id":parent})).await;
    assert_eq!(shown["status"], json!("closed"));
    assert_eq!(shown["close_reason"], json!("All children completed"));

    let (is_error, payload) = call_tool(&client, "issue", json!({"action":"close_eligible"})).await;
    assert!(!is_error);
    assert_eq!(payload, json!({"epic_ids":[],"count":0,"dry_run":false}));

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// D56 (n) + (e): an unknown argument on ANY new arm, and an invalid label name on `relabel`, is
/// rejected in-band with ZERO writes as the `RecordingStorage` spy observes them — the spy records
/// the two new mutations, so these cells cannot pass vacuously (proven by the final positive call).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d56_new_arms_reject_unknown_arguments_with_zero_writes() {
    let (session, spy) = session_recording().await;
    let (client, server, _cancel) = connect(session).await;
    let id = create(
        &client,
        json!({"action":"create","title":"t","labels":["old"]}),
    )
    .await;
    let baseline = spy.mutation_count();

    for (tool, args) in [
        ("query", json!({"kind":"labels","bogus":1})),
        ("query", json!({"kind":"epics","eligible":true})),
        (
            "issue",
            json!({"action":"relabel","old_name":"old","new_name":"new","bogus":1}),
        ),
        ("issue", json!({"action":"close_eligible","dryrun":true})),
        (
            "issue",
            json!({"action":"relabel","old_name":"bad name","new_name":"new"}),
        ),
        (
            "issue",
            json!({"action":"relabel","old_name":"old","new_name":"no!"}),
        ),
    ] {
        let (is_error, payload) = call_tool(&client, tool, args.clone()).await;
        assert!(is_error, "{args} must be rejected in-band: {payload}");
        assert_eq!(
            payload["code"],
            json!("VALIDATION_FAILED"),
            "{args}: {payload}"
        );
        assert_eq!(spy.mutation_count(), baseline, "{args}: zero writes");
    }
    let (_, shown) = call_tool(&client, "issue", json!({"action":"show","id":id})).await;
    assert_eq!(shown["labels"], json!(["old"]));

    // Non-vacuity: the spy DOES observe both new mutations.
    let (is_error, _) = call_tool(
        &client,
        "issue",
        json!({"action":"relabel","old_name":"old","new_name":"new"}),
    )
    .await;
    assert!(!is_error);
    assert_eq!(spy.mutation_count(), baseline + 1, "relabel is recorded");
    let epic = create(
        &client,
        json!({"action":"create","title":"e","issue_type":"epic"}),
    )
    .await;
    let kid = create(
        &client,
        json!({"action":"create","title":"k","parent":epic,"deps":child_of(&epic)}),
    )
    .await;
    let (is_error, _) = call_tool(&client, "issue", json!({"action":"close","id":kid})).await;
    assert!(!is_error);
    let before_close = spy.mutation_count();
    let (is_error, _) = call_tool(&client, "issue", json!({"action":"close_eligible"})).await;
    assert!(!is_error);
    assert_eq!(
        spy.mutation_count(),
        before_close + 1,
        "close_eligible is recorded"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// D56 (n): the four input arms sit LAST in their tool's published `oneOf` (schemars emits
/// declaration order, which `CONTRACT_HASH` digests), and the tool count stays 8.
#[test]
fn d56_arms_are_appended_last_and_the_tool_count_stays_eight() {
    let bundle = unblock_mcp::schema_bundle();
    let issue_arms: Vec<String> = bundle.issue.input["oneOf"]
        .as_array()
        .expect("issue oneOf")
        .iter()
        .map(|arm| arm_discriminant(arm, "action"))
        .collect();
    assert_eq!(
        issue_arms[issue_arms.len() - 2..],
        ["relabel".to_string(), "close_eligible".to_string()]
    );
    let query_arms: Vec<String> = bundle.query.input["oneOf"]
        .as_array()
        .expect("query oneOf")
        .iter()
        .map(|arm| arm_discriminant(arm, "kind"))
        .collect();
    assert_eq!(
        query_arms,
        [
            "list", "ready", "blocked", "search", "count", "stale", "labels", "epics"
        ]
    );
    assert_eq!(unblock_mcp::capabilities().tools.len(), 8);
    assert_eq!(unblock_mcp::CONTRACT_VERSION, "unblock.mcp.v1.11");
}
