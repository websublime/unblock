//! The **M2 exit-gate** end-to-end (spine §6 conformance, FR-20/FR-11/FR-17):
//!
//! An in-process MCP client drives `query{ready}` → `claim` → `issue{close, suggest_next}` over an
//! in-memory `tokio::io::duplex` transport against the REAL server (`serve_duplex_for_test` runs the
//! same `UnblockServer` + `serve_with_ct` path as the shipped `serve`), asserting the close surfaces
//! the newly-unblocked issue. A second case proves cooperative shutdown: a `cancel()` mid-session
//! returns cleanly (FR-17).

mod common;

use common::{call_tool, connect, connect_with_instructions};
use serde_json::json;
use unblock_engine::NewIssue;
use unblock_model::{Dependency, DependencyType};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_advertises_unblock_identity_and_default_instructions() {
    let session = common::session().await;
    let (client, server, _cancel) = connect(session).await;

    // After initialize, the client peer holds the server's `InitializeResult` (its `ServerInfo`).
    let info = client.peer_info().expect("server info after handshake");
    assert_eq!(
        info.server_info.name, "unblock",
        "server identity name must be `unblock`, not rmcp's build-env default"
    );
    assert_eq!(
        info.server_info.version,
        env!("CARGO_PKG_VERSION"),
        "server version must be this crate's package version"
    );
    // No `ServeOptions::instructions` → the generated capability-summary default is advertised.
    let instructions = info
        .instructions
        .as_deref()
        .expect("default instructions present");
    assert!(
        instructions.contains("unblock MCP server"),
        "default instructions summarize the surface, got: {instructions}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_honors_caller_supplied_instructions() {
    let session = common::session().await;
    let custom = "use the query tool first".to_string();
    let (client, server, _cancel) = connect_with_instructions(session, Some(custom.clone())).await;

    let info = client.peer_info().expect("server info after handshake");
    assert_eq!(
        info.instructions.as_deref(),
        Some(custom.as_str()),
        "a non-None ServeOptions::instructions must be advertised verbatim"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m2_exit_gate_ready_claim_close_surfaces_newly_unblocked() {
    let session = common::session().await;

    // Seed a blocker and a dependent (both via the MINTING create path so the ids are real), then add
    // a `Blocks` edge dependent -> blocker. The dependent is NOT ready until the blocker closes.
    let blocker = session
        .create_issue(NewIssue {
            title: "blocker".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("create blocker");
    let dependent = session
        .create_issue(NewIssue {
            title: "dependent".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("create dependent");
    session
        .add_dep(&Dependency {
            issue_id: dependent.id.clone(),
            depends_on_id: blocker.id.clone(),
            dep_type: DependencyType::Blocks,
            created_at: chrono::Utc::now(),
            created_by: Some("tester".to_string()),
            metadata: None,
            thread_id: None,
        })
        .await
        .expect("add blocking edge");

    let (client, server, _cancel) = connect(session).await;

    // 1) query{ready}: the blocker is ready, the dependent is NOT (it is blocked).
    let (is_error, ready) = call_tool(&client, "query", json!({ "kind": "ready" })).await;
    assert!(!is_error, "ready query must succeed");
    // CD-2 (spine §5.3): the `query` list arm is object-wrapped as `{"issues":[…]}`, never a bare array.
    let ready_ids: Vec<String> = ready["issues"]
        .as_array()
        .expect("ready is CD-2 object-wrapped as {\"issues\":[…]}")
        .iter()
        .map(|issue| issue["id"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(ready_ids.contains(&blocker.id), "blocker must be ready");
    assert!(
        !ready_ids.contains(&dependent.id),
        "dependent must be blocked, not ready"
    );

    // 2) claim the blocker.
    let (is_error, claimed) = call_tool(
        &client,
        "claim",
        json!({ "id": blocker.id, "assignee": "agent-a" }),
    )
    .await;
    assert!(!is_error, "claim must succeed");
    assert_eq!(claimed["assignee"], "agent-a");

    // 3) close the blocker with suggest_next: the close must surface the now-unblocked dependent.
    let (is_error, close) = call_tool(
        &client,
        "issue",
        json!({ "action": "close", "id": blocker.id, "suggest_next": true }),
    )
    .await;
    assert!(!is_error, "close must succeed");
    let unblocked_ids: Vec<String> = close["newly_unblocked"]
        .as_array()
        .expect("newly_unblocked is an array")
        .iter()
        .map(|issue| issue["id"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        unblocked_ids.contains(&dependent.id),
        "closing the blocker must surface the dependent as newly unblocked (FR-11/FR-20)"
    );

    // Clean shutdown.
    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cooperative_cancel_mid_session_returns_cleanly() {
    let session = common::session().await;
    let (client, server, cancel) = connect(session).await;

    // A normal call works first.
    let (is_error, _ready) = call_tool(&client, "query", json!({ "kind": "ready" })).await;
    assert!(!is_error);

    // Cancel the server mid-session (FR-17): waiting on it returns cleanly (no panic, no hang).
    cancel.cancel();
    let quit = server.waiting().await.expect("server loop joins cleanly");
    // The reason is a cancellation/closure — either is a clean exit.
    let _ = quit;

    // The client side then closes cleanly too.
    let _ = client.cancel().await;
}
