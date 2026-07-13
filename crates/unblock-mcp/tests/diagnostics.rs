//! End-to-end `diagnostics` tool coverage (FR-15, T2.7/D26) over the REAL server + in-memory duplex
//! transport (`mcp_server_duplex_for_test` runs the shipped `UnblockServer` path).
//!
//! The load-bearing case proves the adapter now THREADS the changelog `since` window into the engine
//! (D26/OQ-1): it FAILS under the old adapter, which accepted `changelog{since}` on the wire then
//! DROPPED it pre-call (returning the full window regardless).

mod common;

use std::sync::Arc;

use chrono::Utc;
use common::{call_tool, connect};
use serde_json::json;
use unblock_engine::Session;
use unblock_model::{Issue, Priority};

/// Extract the `findings[*].label` list from a `diagnostics` tool structured payload
/// (`DiagnosticReport { kind, findings: [{label, detail}] }`).
fn finding_labels(structured: &serde_json::Value) -> Vec<String> {
    structured
        .get("findings")
        .and_then(|f| f.as_array())
        .expect("findings array")
        .iter()
        .map(|f| {
            f.get("label")
                .and_then(|l| l.as_str())
                .expect("label")
                .to_string()
        })
        .collect()
}

/// Build a minimal valid open [`Issue`].
fn issue(id: &str, secs: i64) -> Issue {
    Issue {
        id: id.to_string(),
        title: format!("issue {id}"),
        priority: Priority::MEDIUM,
        created_at: chrono::TimeZone::timestamp_opt(&Utc, secs, 0)
            .single()
            .expect("valid ts"),
        updated_at: chrono::TimeZone::timestamp_opt(&Utc, secs, 0)
            .single()
            .expect("valid ts"),
        ..Issue::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_changelog_threads_the_since_window_into_the_engine() {
    let session: Arc<Session> = common::session().await;

    // Seed two issues closed at DIFFERENT times: ub-early first, then a marker, then ub-late.
    session
        .create(&issue("ub-early", 1000))
        .await
        .expect("create early");
    session
        .close_with_suggestions("ub-early", None)
        .await
        .expect("close early");
    let since = Utc::now();
    session
        .create(&issue("ub-late", 1001))
        .await
        .expect("create late");
    session
        .close_with_suggestions("ub-late", None)
        .await
        .expect("close late");

    let (client, server, _cancel) = connect(session).await;

    // changelog{since: <marker>}: only ub-late (closed at/after the marker). Under the OLD
    // drop-since adapter this would return BOTH — so this assertion is the non-vacuity proof.
    let (is_error, windowed) = call_tool(
        &client,
        "diagnostics",
        json!({ "kind": "changelog", "since": since.to_rfc3339() }),
    )
    .await;
    assert!(!is_error, "changelog is a read, never an error here");
    assert_eq!(
        finding_labels(&windowed),
        vec!["ub-late".to_string()],
        "the `since` window is threaded: only the later-closed issue surfaces"
    );

    // changelog{} (no since): both closed issues (the wire default is the full window).
    let (_, full) = call_tool(&client, "diagnostics", json!({ "kind": "changelog" })).await;
    assert_eq!(
        finding_labels(&full),
        vec!["ub-early".to_string(), "ub-late".to_string()],
        "the omitted `since` (wire default) returns every closed issue"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
