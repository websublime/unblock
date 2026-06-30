//! Error-mapping conformance (spine §5.6, FR-11): every engine error surfaces as an **in-band**
//! `ToolOutput::Error` that is valid JSON carrying `code`/`message`/`retryable` (and `context` where
//! the engine provides it). The tool call still SUCCEEDS at the protocol level (`Err(ErrorData)` is
//! reserved for protocol faults) — the domain failure rides `is_error=true` + structured content.

mod common;

use common::{call_tool, connect};
use serde_json::json;
use unblock_engine::NewIssue;

/// Assert the structured payload is a valid error envelope (code + message + retryable present).
fn assert_error_envelope(structured: &serde_json::Value, expected_code: &str) {
    assert_eq!(
        structured["code"], expected_code,
        "unexpected error code: {structured}"
    );
    assert!(
        structured["message"].is_string(),
        "message must be a string: {structured}"
    );
    assert!(
        structured["retryable"].is_boolean(),
        "retryable must be present: {structured}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_missing_issue_is_in_band_issue_not_found() {
    let session = common::session().await;
    let (client, server, _cancel) = connect(session).await;

    let (is_error, structured) = call_tool(
        &client,
        "issue",
        json!({ "action": "show", "id": "ub-nope" }),
    )
    .await;
    assert!(is_error, "a missing show target must be an in-band error");
    assert_error_envelope(&structured, "ISSUE_NOT_FOUND");
    assert_eq!(structured["context"]["id"], "ub-nope");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_contention_is_in_band_already_claimed() {
    let session = common::session().await;
    // Seed an issue and claim it once directly so the MCP claim loses the race.
    let issue = session
        .create_issue(NewIssue {
            title: "contended".to_string(),
            ..NewIssue::default()
        })
        .await
        .expect("create");
    session
        .claim(&issue.id, "first-winner")
        .await
        .expect("first claim");

    let (client, server, _cancel) = connect(session).await;
    let (is_error, structured) = call_tool(
        &client,
        "claim",
        json!({ "id": issue.id, "assignee": "second" }),
    )
    .await;
    assert!(is_error, "a lost claim must be an in-band error");
    assert_error_envelope(&structured, "ALREADY_CLAIMED");
    assert_eq!(
        structured["retryable"], true,
        "ALREADY_CLAIMED is retryable"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_seam_surfaces_feature_not_wired_in_band() {
    let session = common::session().await;
    let (client, server, _cancel) = connect(session).await;

    // The sync seam returns EngineError::FeatureNotWired{"sync"} (-> INTERNAL_ERROR) at T2.2; it must
    // surface as a CLEAN in-band structured error (valid JSON), never a protocol fault.
    let (is_error, structured) = call_tool(&client, "sync", json!({ "action": "export" })).await;
    assert!(is_error, "the sync seam must be an in-band error");
    assert_error_envelope(&structured, "INTERNAL_ERROR");
    assert!(
        structured["message"]
            .as_str()
            .unwrap_or_default()
            .contains("sync"),
        "the message must name the not-wired feature: {structured}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_validation_failure_is_in_band() {
    let session = common::session().await;
    let (client, server, _cancel) = connect(session).await;

    // A blank title fails IssueValidator::validate in the engine -> VALIDATION_FAILED.
    let (is_error, structured) =
        call_tool(&client, "issue", json!({ "action": "create", "title": "" })).await;
    assert!(is_error, "an invalid create must be an in-band error");
    assert_error_envelope(&structured, "VALIDATION_FAILED");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
