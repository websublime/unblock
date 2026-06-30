//! FR-9 parity at the L7 boundary (renamed from the planned `cli_parity.rs` — there is no CLI until
//! T3.1, so this proves **MCP-vs-`Session` parity**, NOT CLI parity).
//!
//! The same op via the MCP `issue`/`query` tool vs. via `Session` directly yields identical results:
//! the MCP adapter is a thin pass-through over the single mutation home (`Session`), so behaviour
//! cannot drift (the spine §4.2 property at the L7 boundary). CLI parity proper lands at T3.1.

mod common;

use common::{call_tool, connect, session};
use serde_json::json;
use unblock_engine::NewIssue;
use unblock_model::ListFilters;

/// A `create` then `show` via the MCP `issue` tool returns the SAME issue `Session::get` returns
/// directly — same id, title, and fields (the adapter does not transform the domain value).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_and_show_via_mcp_equals_session_get() {
    let s = session().await;
    let (client, server, _cancel) = connect(s.clone()).await;

    // Create via the MCP tool.
    let (is_error, created) = call_tool(
        &client,
        "issue",
        json!({ "action": "create", "title": "parity issue", "design": "the design" }),
    )
    .await;
    assert!(!is_error, "create succeeds");
    let id = created["id"].as_str().expect("minted id").to_string();

    // Show via the MCP tool.
    let (_e, shown) = call_tool(&client, "issue", json!({ "action": "show", "id": id })).await;

    // The SAME issue via Session::get directly.
    let direct = s.get(&id).await.expect("get").expect("present");
    let direct_json = serde_json::to_value(&direct).expect("serialize");

    assert_eq!(
        shown, direct_json,
        "MCP `show` == `Session::get` (no adapter drift)"
    );
    assert_eq!(direct.title, "parity issue");
    assert_eq!(direct.design.as_deref(), Some("the design"));

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// A `query{ready}` via the MCP tool returns the SAME ready set `Session::ready` returns directly over
/// the same store (the same op through two call sites yields identical state — FR-9).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_ready_via_mcp_equals_session_ready() {
    let s = session().await;

    // Seed three issues directly through the Session (the mutation home).
    for title in ["a", "b", "c"] {
        s.create_issue(NewIssue {
            title: title.to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("create");
    }

    let (client, server, _cancel) = connect(s.clone()).await;
    let (_e, ready_via_mcp) = call_tool(&client, "query", json!({ "kind": "ready" })).await;

    let ready_direct = s.ready(&ListFilters::default()).await.expect("ready");
    let direct_json = serde_json::to_value(&ready_direct).expect("serialize");

    assert_eq!(
        ready_via_mcp, direct_json,
        "MCP query{{ready}} == Session::ready (identical results through two call sites)",
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// A `create_bulk` via the MCP tool persists the SAME issues `Session::list` returns directly — the
/// adapter routes through the ATOMIC `Session::create_bulk` (no L7 loop).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_bulk_via_mcp_persists_through_session() {
    let s = session().await;
    let (client, server, _cancel) = connect(s.clone()).await;

    let markdown = "## Alpha\n### Type\ntask\n\n## Beta\n### Type\nfeature\n";
    let (is_error, created) = call_tool(
        &client,
        "issue",
        json!({ "action": "create_bulk", "markdown": markdown }),
    )
    .await;
    assert!(!is_error, "bulk create succeeds");
    let created_arr = created.as_array().expect("the Vec output");
    assert_eq!(created_arr.len(), 2, "two issues created");

    // The same issues are visible through Session::list directly.
    let listed = s.list(&ListFilters::default()).await.expect("list");
    assert_eq!(
        listed.len(),
        2,
        "both persisted through the one Session path"
    );
    let titles: std::collections::BTreeSet<&str> =
        listed.iter().map(|i| i.title.as_str()).collect();
    assert!(titles.contains("Alpha") && titles.contains("Beta"));

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
