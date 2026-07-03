//! The FR-12 AC-2 client-side drift e2e (F-1) + the -32002 resource boundary (F-2) + the
//! `similar_ids` not-found fold (FORK-3A), over a LIVE `serve_duplex_for_test` duplex.
//!
//! A real MCP client issues wire-level `read_resource` over ALL 5 URIs and the unknown-URI branch:
//! - the two discovery documents (`unblock://capabilities`/`unblock://schema`) are parsed CLIENT-SIDE
//!   into `Capabilities`/`SchemaBundle` and asserted to stamp `contract_version == CONTRACT_VERSION`
//!   AND to body-equal the pure builders — this IS the "a client can detect drift" FR-12 AC (§AC-2);
//! - `issues/{id}`/`ready`/`blocked` round-trip real data;
//! - an unknown URI and a missing `{id}` return rmcp `-32002 resource_not_found` (NOT `-32603`) with
//!   the full structured payload as `data`;
//! - a near-miss `{id}` yields non-empty `similar_ids` naming the real id + a "Did you mean" hint;
//!   an empty workspace falls back to the `query{kind:list}` hint.

mod common;

use common::{connect, session, session_failing_list};
use rmcp::model::ReadResourceRequestParams;
use rmcp::service::ServiceError;
use serde_json::Value;
use unblock_engine::{NewIssue, Session};
use unblock_mcp::{CONTRACT_VERSION, Capabilities, SchemaBundle, capabilities, schema_bundle};

/// `-32002` (`RESOURCE_NOT_FOUND`) as the raw rmcp code.
const RESOURCE_NOT_FOUND: i32 = -32002;

/// Read a resource URI over the live client and return the single text body parsed as JSON.
async fn read_resource_text(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
    uri: &str,
) -> Value {
    let result = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .expect("read_resource round-trips");
    let contents = result.contents.into_iter().next().expect("one content");
    let text = match contents {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text,
        rmcp::model::ResourceContents::BlobResourceContents { .. } => {
            panic!("expected text resource contents, got a blob")
        }
    };
    serde_json::from_str(&text).expect("resource body is valid JSON")
}

/// Read a resource URI expecting a not-found error; return its `(code, data)`.
async fn read_resource_err(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
    uri: &str,
) -> (i32, Value) {
    let err = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .expect_err("read_resource must fail");
    match err {
        ServiceError::McpError(data) => (
            data.code.0,
            data.data.expect("structured payload attached as data"),
        ),
        other => panic!("expected an rmcp McpError, got {other:?}"),
    }
}

/// Seed one issue via the MINTING create path (the engine mints the real id) and return it.
async fn seed_one(session: &Session, title: &str) -> unblock_model::Issue {
    session
        .create_issue(NewIssue {
            title: title.to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("create issue")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_capabilities_parses_client_side_and_stamps_contract_version() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let body = read_resource_text(&client, "unblock://capabilities").await;
    let caps: Capabilities =
        serde_json::from_value(body.clone()).expect("client parses Capabilities");
    assert_eq!(
        caps.contract_version, CONTRACT_VERSION,
        "a client can detect drift by comparing contract_version"
    );
    // Body parity with the pure builder (complements the T2.3 list parity).
    let built = serde_json::to_value(capabilities()).unwrap();
    assert_eq!(body, built, "served capabilities body == the pure builder");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_schema_parses_client_side_and_stamps_contract_version() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let body = read_resource_text(&client, "unblock://schema").await;
    let bundle: SchemaBundle =
        serde_json::from_value(body.clone()).expect("client parses SchemaBundle");
    assert_eq!(bundle.contract_version, CONTRACT_VERSION);
    let built = serde_json::to_value(schema_bundle()).unwrap();
    assert_eq!(body, built, "served schema bundle body == the pure builder");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_issue_by_id_round_trips() {
    let session = session().await;
    let issue = seed_one(&session, "seeded").await;
    let id = issue.id.clone();
    let (client, server, _cancel) = connect(session).await;

    let body = read_resource_text(&client, &format!("unblock://issues/{id}")).await;
    assert_eq!(body["id"].as_str(), Some(id.as_str()));

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_ready_and_blocked_partition_a_dep_pair() {
    let session = session().await;
    let blocker = seed_one(&session, "blocker").await;
    let dependent = seed_one(&session, "dependent").await;
    session
        .add_dep(&unblock_model::Dependency {
            issue_id: dependent.id.clone(),
            depends_on_id: blocker.id.clone(),
            dep_type: unblock_model::DependencyType::Blocks,
            created_at: chrono::Utc::now(),
            created_by: Some("tester".to_string()),
            metadata: None,
            thread_id: None,
        })
        .await
        .expect("add blocking edge");
    let (client, server, _cancel) = connect(session).await;

    let ready = read_resource_text(&client, "unblock://issues/ready").await;
    let ready_ids: Vec<&str> = ready
        .as_array()
        .expect("ready is an array")
        .iter()
        .filter_map(|i| i["id"].as_str())
        .collect();
    assert!(ready_ids.contains(&blocker.id.as_str()), "blocker is ready");
    assert!(
        !ready_ids.contains(&dependent.id.as_str()),
        "dependent is blocked, not ready"
    );

    let blocked_body = read_resource_text(&client, "unblock://issues/blocked").await;
    let blocked_ids: Vec<&str> = blocked_body
        .as_array()
        .expect("blocked is an array")
        .iter()
        .filter_map(|i| i["id"].as_str())
        .collect();
    assert!(
        blocked_ids.contains(&dependent.id.as_str()),
        "dependent is blocked"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_uri_is_resource_not_found() {
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let (code, data) = read_resource_err(&client, "unblock://nope").await;
    assert_eq!(code, RESOURCE_NOT_FOUND, "unknown URI must be -32002");
    assert_eq!(data["code"], "ISSUE_NOT_FOUND");
    assert!(data.get("context").is_some(), "structured context present");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_id_is_resource_not_found_with_similar_ids() {
    let session = session().await;
    let issue = seed_one(&session, "real").await;
    let real_id = issue.id.clone();
    // Drop the last char → Levenshtein distance 1 from the real id (a near-miss).
    let near_miss = &real_id[..real_id.len() - 1];
    let (client, server, _cancel) = connect(session).await;

    let (code, data) = read_resource_err(&client, &format!("unblock://issues/{near_miss}")).await;
    assert_eq!(code, RESOURCE_NOT_FOUND, "missing {{id}} must be -32002");
    let similar = data["context"]["similar_ids"]
        .as_array()
        .expect("similar_ids array present");
    // Non-vacuity: the assertion FAILS if the hint construction is removed.
    assert!(
        similar.iter().any(|v| v.as_str() == Some(real_id.as_str())),
        "similar_ids must name the real id {real_id}, got {similar:?}"
    );
    assert!(
        data["hint"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Did you mean"),
        "the did-you-mean hint is present, got {:?}",
        data["hint"]
    );
    assert_eq!(data["context"]["searched_id"].as_str(), Some(near_miss));

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_id_scan_failure_surfaces_the_scan_error_not_the_not_found() {
    // FORK-3A fidelity: a FAILED corpus scan surfaces the scan error (DatabaseError), NOT a fresh
    // IssueNotFound-with-suggestions (the original's pinned `..._surfaces_id_scan_failure` behaviour).
    let session = session_failing_list().await;
    let (client, server, _cancel) = connect(session).await;

    let (code, data) = read_resource_err(&client, "unblock://issues/ub-missing").await;
    // The scan error is a true internal fault at this boundary → -32603, and its code is NOT the
    // resource IssueNotFound (which would carry `similar_ids`).
    assert_eq!(code, -32603, "a scan failure is a true internal fault");
    assert_eq!(
        data["code"], "DATABASE_ERROR",
        "the SCAN error surfaces, not IssueNotFound"
    );
    assert!(
        data["context"].get("similar_ids").is_none(),
        "no suggestion context on a scan failure"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_id_with_no_neighbours_falls_back_to_query_hint() {
    // Empty workspace: no candidate is close, so the hint is the query{kind:list} fallback.
    let session = session().await;
    let (client, server, _cancel) = connect(session).await;

    let (code, data) = read_resource_err(&client, "unblock://issues/ub-zzzzz").await;
    assert_eq!(code, RESOURCE_NOT_FOUND);
    assert_eq!(
        data["context"]["similar_ids"].as_array().map(Vec::len),
        Some(0),
        "no neighbours → empty similar_ids"
    );
    assert!(
        data["hint"].as_str().unwrap_or_default().contains("query"),
        "empty workspace falls back to the query{{kind:list}} hint, got {:?}",
        data["hint"]
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
