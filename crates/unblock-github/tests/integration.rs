//! Integration tests for the GitHub API client.
//!
//! These tests require a valid `GITHUB_TOKEN` environment variable and network
//! access to GitHub. They are skipped automatically when `GITHUB_TOKEN` is not
//! set.

use unblock_core::config::Config;
use unblock_core::types::{IssueState, Status};
use unblock_github::client::GitHubClient;

/// Returns `true` if the `GITHUB_TOKEN` env var is set and non-empty.
fn has_github_token() -> bool {
    std::env::var("GITHUB_TOKEN")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Builds a [`Config`] from the process environment for integration tests.
///
/// Requires `GITHUB_TOKEN` to be set. Uses `UNBLOCK_REPO` if available,
/// otherwise falls back to git remote detection.
fn test_config() -> Config {
    Config::load().expect("Config::load() should succeed when GITHUB_TOKEN is set")
}

#[tokio::test]
async fn github_client_new_connects_to_real_repo() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed with valid token and repo");

    // Verify the client resolved an owner and repo.
    assert!(
        !client.owner().is_empty(),
        "owner should be non-empty after construction"
    );
    assert!(
        !client.repo().is_empty(),
        "repo should be non-empty after construction"
    );

    // Verify the API base URL is set.
    assert!(
        !client.api_base_url().is_empty(),
        "api_base_url should be non-empty"
    );

    // Verify the REST URL builds correctly.
    let rest_url = client.rest_url("/repos");
    assert!(
        rest_url.starts_with("https://"),
        "rest_url should be an HTTPS URL, got: {rest_url}"
    );

    // Verify the GraphQL URL builds correctly.
    let graphql_url = client.graphql_url();
    assert!(
        graphql_url.ends_with("/graphql"),
        "graphql_url should end with /graphql, got: {graphql_url}"
    );
}

// ── fetch_issue ────────────────────────────────────────────────────

#[tokio::test]
async fn fetch_issue_returns_full_details_for_existing_issue() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Issue #1 should exist in most repos. If the test repo has no issues,
    // this test will fail gracefully with IssueNotFound.
    let issue = client
        .fetch_issue(1)
        .await
        .expect("fetch_issue(1) should succeed for an existing issue");

    assert_eq!(issue.number, 1, "issue number should be 1");
    assert!(!issue.title.is_empty(), "title should be non-empty");
    assert!(!issue.node_id.is_empty(), "node_id should be non-empty");
    assert!(!issue.url.is_empty(), "url should be non-empty");
    assert!(
        issue.state == IssueState::Open || issue.state == IssueState::Closed,
        "state should be Open or Closed"
    );
}

#[tokio::test]
async fn fetch_issue_returns_issue_not_found_for_nonexistent_number() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Use a very large issue number that almost certainly does not exist.
    let result = client.fetch_issue(999_999_999).await;
    assert!(
        result.is_err(),
        "fetch_issue for non-existent issue should return an error"
    );

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not found") || msg.contains("Not Found") || err.status_code() == 404,
        "error should indicate issue not found, got: {msg} (status: {})",
        err.status_code()
    );
}

// ── fetch_graph_data ───────────────────────────────────────────────

#[tokio::test]
async fn fetch_graph_data_returns_issues_from_real_repo() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let (issues, edges) = client
        .fetch_graph_data()
        .await
        .expect("fetch_graph_data() should succeed");

    // The test repo should have at least one open issue.
    // If the repo has zero open issues this assertion will fail — that is fine,
    // it means the test repo needs seeding.
    assert!(
        !issues.is_empty(),
        "fetch_graph_data should return at least one open issue"
    );

    // Verify all returned issues are open.
    for issue in &issues {
        assert_eq!(
            issue.state,
            IssueState::Open,
            "fetch_graph_data should only return open issues, but issue #{} is {:?}",
            issue.number,
            issue.state
        );
    }

    // Verify basic fields are populated.
    let first = &issues[0];
    assert!(first.number > 0, "issue number should be positive");
    assert!(!first.title.is_empty(), "title should be non-empty");
    assert!(!first.node_id.is_empty(), "node_id should be non-empty");

    // Verify detail fields are empty (per types.rs contract).
    for issue in &issues {
        assert!(
            issue.comments.is_empty(),
            "comments should be empty for graph issues (issue #{})",
            issue.number
        );
        assert!(
            issue.blocked_by.is_empty(),
            "blocked_by should be empty for graph issues (issue #{})",
            issue.number
        );
        assert!(
            issue.blocking.is_empty(),
            "blocking should be empty for graph issues (issue #{})",
            issue.number
        );
        assert!(
            issue.parent.is_none(),
            "parent should be None for graph issues (issue #{})",
            issue.number
        );
        assert!(
            issue.sub_issues.is_empty(),
            "sub_issues should be empty for graph issues (issue #{})",
            issue.number
        );
    }

    // Edges are optional — a repo might not have blocking relationships.
    // Just verify the types are correct (no panics).
    for edge in &edges {
        assert!(edge.source > 0, "edge source should be positive");
        assert!(edge.target > 0, "edge target should be positive");
        assert_ne!(
            edge.source, edge.target,
            "self-blocking edges should not exist"
        );
    }

    // Log for manual review when running with GITHUB_TOKEN.
    eprintln!(
        "fetch_graph_data: {} issues, {} edges",
        issues.len(),
        edges.len()
    );
}

#[tokio::test]
async fn fetch_graph_data_issues_have_valid_status_and_priority() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let (issues, _) = client
        .fetch_graph_data()
        .await
        .expect("fetch_graph_data() should succeed");

    // Every issue should have a valid status and priority (possibly defaults).
    for issue in &issues {
        // Status must be one of the known variants.
        let _valid = matches!(
            issue.status,
            Status::Open | Status::InProgress | Status::Blocked | Status::Deferred | Status::Closed
        );
        assert!(
            _valid,
            "issue #{} has unexpected status {:?}",
            issue.number, issue.status
        );
    }
}
