//! Integration tests for the GitHub API client.
//!
//! These tests require a valid `GITHUB_TOKEN` environment variable and network
//! access to GitHub. They are skipped automatically when `GITHUB_TOKEN` is not
//! set.

use unblock_core::config::Config;
use unblock_core::types::{IssueState, Status};
use unblock_github::client::GitHubClient;
use unblock_github::mutations::CreateIssueParams;
use unblock_github::projects::FieldValue;

/// Drop guard that closes a GitHub issue on scope exit, even during a panic
/// unwind. This ensures integration tests do not leave orphaned open issues
/// when an assertion fails before the explicit cleanup call.
///
/// The guard captures a reference to the [`GitHubClient`] and the issue number
/// at creation time. On drop it uses the current tokio runtime handle to
/// block on the async `close_issue` call. If the close fails the error is
/// logged to stderr but does not cause a secondary panic (which would abort
/// the process during an unwind).
struct CloseIssueGuard<'a> {
    client: &'a GitHubClient,
    issue_number: u64,
    /// Set to `true` once the test completes successfully and the caller has
    /// already cleaned up (or does not need cleanup). When `true`, the guard
    /// skips the `close_issue` call in `Drop`.
    disarmed: bool,
}

impl<'a> CloseIssueGuard<'a> {
    /// Creates an armed guard that will close `issue_number` on drop.
    fn new(client: &'a GitHubClient, issue_number: u64) -> Self {
        Self {
            client,
            issue_number,
            disarmed: false,
        }
    }

    /// Disarms the guard so that `Drop` becomes a no-op. Call this after the
    /// test has successfully cleaned up on its own.
    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for CloseIssueGuard<'_> {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        // We are inside an async tokio test, so a runtime handle is available.
        // `block_in_place` + `block_on` lets us run the async close from a
        // synchronous `Drop` context without panicking about nested runtimes.
        let number = self.issue_number;
        let client = self.client;
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            if let Err(e) = handle.block_on(client.close_issue(
                number,
                Some("Automated test cleanup (drop guard)".to_owned()),
            )) {
                eprintln!("CloseIssueGuard: failed to close issue #{number}: {e}");
            } else {
                eprintln!("CloseIssueGuard: cleaned up issue #{number}");
            }
        });
    }
}

/// Returns `true` if the `GITHUB_TOKEN` env var is set and non-empty.
fn has_github_token() -> bool {
    std::env::var("GITHUB_TOKEN")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Returns `true` if `UNBLOCK_PROJECT` env var is set and non-empty.
fn has_project_number() -> bool {
    std::env::var("UNBLOCK_PROJECT")
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
        let valid = matches!(
            issue.status,
            Status::Open | Status::InProgress | Status::Blocked | Status::Deferred | Status::Closed
        );
        assert!(
            valid,
            "issue #{} has unexpected status {:?}",
            issue.number, issue.status
        );
    }
}

// ── mutations: create_issue ─────────────────────────────────────────

#[tokio::test]
async fn create_issue_returns_issue_with_correct_fields() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let params = CreateIssueParams {
        title: "[test] create_issue integration test".to_owned(),
        body: Some("Automated integration test — safe to close.".to_owned()),
        labels: vec!["test".to_owned()],
        milestone: None,
        assignees: vec![],
    };

    let issue = client
        .create_issue(params)
        .await
        .expect("create_issue() should succeed");

    // Arm the drop guard so the issue is closed even if an assertion panics.
    let mut guard = CloseIssueGuard::new(&client, issue.number);

    // Verify the returned issue has the correct fields.
    assert!(issue.number > 0, "issue number should be positive");
    assert_eq!(
        issue.title, "[test] create_issue integration test",
        "title should match what was provided"
    );
    assert!(!issue.node_id.is_empty(), "node_id should be non-empty");
    assert!(!issue.url.is_empty(), "url should be non-empty");
    assert_eq!(
        issue.state,
        IssueState::Open,
        "newly created issue should be Open"
    );

    // Explicit cleanup on the happy path; disarm the guard so it does not
    // double-close.
    client
        .close_issue(issue.number, Some("Automated test cleanup".to_owned()))
        .await
        .expect("close_issue() cleanup should succeed");
    guard.disarm();

    eprintln!(
        "create_issue test: created and closed issue #{}",
        issue.number
    );
}

// ── mutations: close_issue ──────────────────────────────────────────

#[tokio::test]
async fn close_issue_closes_issue_and_refetch_confirms() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Create an issue to close.
    let params = CreateIssueParams {
        title: "[test] close_issue integration test".to_owned(),
        body: Some("Automated integration test — will be closed.".to_owned()),
        labels: vec!["test".to_owned()],
        milestone: None,
        assignees: vec![],
    };

    let issue = client
        .create_issue(params)
        .await
        .expect("create_issue() should succeed");

    // Arm the drop guard so the issue is closed even if an assertion panics.
    let mut guard = CloseIssueGuard::new(&client, issue.number);

    // Close it without a reason.
    client
        .close_issue(issue.number, None)
        .await
        .expect("close_issue() should succeed");

    // The issue is now closed; disarm since cleanup is no longer needed.
    guard.disarm();

    // Re-fetch and verify it is closed.
    let refetched = client
        .fetch_issue(issue.number)
        .await
        .expect("fetch_issue() after close should succeed");

    assert_eq!(
        refetched.state,
        IssueState::Closed,
        "issue should be Closed after close_issue()"
    );

    eprintln!("close_issue test: closed issue #{}", issue.number);
}

#[tokio::test]
async fn close_issue_with_reason_adds_comment_before_closing() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Create an issue to close with a reason.
    let params = CreateIssueParams {
        title: "[test] close_issue_with_reason integration test".to_owned(),
        body: Some("Automated integration test — will be closed with reason.".to_owned()),
        labels: vec!["test".to_owned()],
        milestone: None,
        assignees: vec![],
    };

    let issue = client
        .create_issue(params)
        .await
        .expect("create_issue() should succeed");

    // Arm the drop guard so the issue is closed even if an assertion panics.
    let mut guard = CloseIssueGuard::new(&client, issue.number);

    let reason_text = "Closing because the test is complete.";

    // Close with a reason (adds comment first).
    client
        .close_issue(issue.number, Some(reason_text.to_owned()))
        .await
        .expect("close_issue() with reason should succeed");

    // The issue is now closed; disarm since cleanup is no longer needed.
    guard.disarm();

    // Re-fetch and verify the comment appears.
    let refetched = client
        .fetch_issue(issue.number)
        .await
        .expect("fetch_issue() after close should succeed");

    assert_eq!(
        refetched.state,
        IssueState::Closed,
        "issue should be Closed after close_issue()"
    );

    // The reason comment should appear in the comments list.
    let has_reason_comment = refetched
        .comments
        .iter()
        .any(|c| c.body.contains(reason_text));
    assert!(
        has_reason_comment,
        "reason comment should appear in the issue comments, got: {:?}",
        refetched
            .comments
            .iter()
            .map(|c| &c.body)
            .collect::<Vec<_>>()
    );

    eprintln!(
        "close_issue_with_reason test: closed issue #{} with reason",
        issue.number
    );
}

#[tokio::test]
async fn close_issue_returns_issue_not_found_for_nonexistent_number() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let result = client.close_issue(999_999_999, None).await;
    assert!(
        result.is_err(),
        "close_issue for non-existent issue should return an error"
    );

    let err = result.unwrap_err();
    assert_eq!(
        err.status_code(),
        404,
        "error should be 404 IssueNotFound, got: {} ({})",
        err.status_code(),
        err
    );
}

// ── mutations: add_comment ──────────────────────────────────────────

#[tokio::test]
async fn add_comment_posts_comment_and_returns_url() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Create an issue to comment on.
    let params = CreateIssueParams {
        title: "[test] add_comment integration test".to_owned(),
        body: Some("Automated integration test — will receive a comment.".to_owned()),
        labels: vec!["test".to_owned()],
        milestone: None,
        assignees: vec![],
    };

    let issue = client
        .create_issue(params)
        .await
        .expect("create_issue() should succeed");

    // Arm the drop guard so the issue is closed even if an assertion panics.
    let mut guard = CloseIssueGuard::new(&client, issue.number);

    let comment_body = "Integration test comment — hello from add_comment!";

    let comment_url = client
        .add_comment(issue.number, comment_body.to_owned())
        .await
        .expect("add_comment() should succeed");

    // Verify the URL is non-empty and looks like a GitHub URL.
    assert!(!comment_url.is_empty(), "comment URL should be non-empty");
    assert!(
        comment_url.contains("github"),
        "comment URL should contain 'github', got: {comment_url}"
    );

    // Re-fetch and verify the comment appears.
    let refetched = client
        .fetch_issue(issue.number)
        .await
        .expect("fetch_issue() after comment should succeed");

    let has_comment = refetched
        .comments
        .iter()
        .any(|c| c.body.contains(comment_body));
    assert!(
        has_comment,
        "comment should appear in issue comments after add_comment()"
    );

    // Explicit cleanup on the happy path; disarm the guard so it does not
    // double-close.
    client
        .close_issue(issue.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close_issue() cleanup should succeed");
    guard.disarm();

    eprintln!(
        "add_comment test: commented on issue #{}, URL: {comment_url}",
        issue.number
    );
}

#[tokio::test]
async fn add_comment_returns_issue_not_found_for_nonexistent_number() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let result = client
        .add_comment(999_999_999, "should fail".to_owned())
        .await;
    assert!(
        result.is_err(),
        "add_comment for non-existent issue should return an error"
    );

    let err = result.unwrap_err();
    assert_eq!(
        err.status_code(),
        404,
        "error should be 404 IssueNotFound, got: {} ({})",
        err.status_code(),
        err
    );
}

// ── mutations: add_blocked_by / remove_blocked_by ───────────────────

#[tokio::test]
async fn add_blocked_by_creates_blocking_relationship() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Create two issues: A will be blocked by B.
    let issue_a = client
        .create_issue(CreateIssueParams {
            title: "[test] add_blocked_by issue A (blocked)".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create_issue A should succeed");

    let mut guard_a = CloseIssueGuard::new(&client, issue_a.number);

    let issue_b = client
        .create_issue(CreateIssueParams {
            title: "[test] add_blocked_by issue B (blocker)".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create_issue B should succeed");

    let mut guard_b = CloseIssueGuard::new(&client, issue_b.number);

    // Add blocking relationship: A is blocked by B.
    client
        .add_blocked_by(issue_a.number, issue_b.number)
        .await
        .expect("add_blocked_by should succeed");

    // Re-fetch A and verify B is in blockedBy.
    let refetched_a = client
        .fetch_issue(issue_a.number)
        .await
        .expect("fetch_issue A should succeed after add_blocked_by");

    let has_blocker = refetched_a
        .blocked_by
        .iter()
        .any(|r| r.number == issue_b.number);
    assert!(
        has_blocker,
        "issue A should show B in blocked_by, got: {:?}",
        refetched_a
            .blocked_by
            .iter()
            .map(|r| r.number)
            .collect::<Vec<_>>()
    );

    // Cleanup: remove the relationship, then close both issues.
    let _ = client
        .remove_blocked_by(issue_a.number, issue_b.number)
        .await;
    client
        .close_issue(issue_a.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close A should succeed");
    guard_a.disarm();
    client
        .close_issue(issue_b.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close B should succeed");
    guard_b.disarm();

    eprintln!(
        "add_blocked_by test: #{} blocked by #{} — verified",
        issue_a.number, issue_b.number
    );
}

#[tokio::test]
async fn remove_blocked_by_removes_blocking_relationship() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Create two issues: A will be blocked by B, then unblocked.
    let issue_a = client
        .create_issue(CreateIssueParams {
            title: "[test] remove_blocked_by issue A".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create_issue A should succeed");

    let mut guard_a = CloseIssueGuard::new(&client, issue_a.number);

    let issue_b = client
        .create_issue(CreateIssueParams {
            title: "[test] remove_blocked_by issue B".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create_issue B should succeed");

    let mut guard_b = CloseIssueGuard::new(&client, issue_b.number);

    // Add, then remove the blocking relationship.
    client
        .add_blocked_by(issue_a.number, issue_b.number)
        .await
        .expect("add_blocked_by should succeed");

    client
        .remove_blocked_by(issue_a.number, issue_b.number)
        .await
        .expect("remove_blocked_by should succeed");

    // Re-fetch A and verify B is no longer in blockedBy.
    let refetched_a = client
        .fetch_issue(issue_a.number)
        .await
        .expect("fetch_issue A should succeed after remove_blocked_by");

    let has_blocker = refetched_a
        .blocked_by
        .iter()
        .any(|r| r.number == issue_b.number);
    assert!(
        !has_blocker,
        "issue A should NOT show B in blocked_by after removal, got: {:?}",
        refetched_a
            .blocked_by
            .iter()
            .map(|r| r.number)
            .collect::<Vec<_>>()
    );

    // Cleanup.
    client
        .close_issue(issue_a.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close A should succeed");
    guard_a.disarm();
    client
        .close_issue(issue_b.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close B should succeed");
    guard_b.disarm();

    eprintln!(
        "remove_blocked_by test: #{} no longer blocked by #{} — verified",
        issue_a.number, issue_b.number
    );
}

#[tokio::test]
async fn add_blocked_by_duplicate_returns_duplicate_dependency() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Create two issues.
    let issue_a = client
        .create_issue(CreateIssueParams {
            title: "[test] duplicate_dependency issue A".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create_issue A should succeed");

    let mut guard_a = CloseIssueGuard::new(&client, issue_a.number);

    let issue_b = client
        .create_issue(CreateIssueParams {
            title: "[test] duplicate_dependency issue B".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create_issue B should succeed");

    let mut guard_b = CloseIssueGuard::new(&client, issue_b.number);

    // First add should succeed.
    client
        .add_blocked_by(issue_a.number, issue_b.number)
        .await
        .expect("first add_blocked_by should succeed");

    // Second add should return DuplicateDependency (status 409).
    let result = client.add_blocked_by(issue_a.number, issue_b.number).await;
    assert!(
        result.is_err(),
        "second add_blocked_by should return an error"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.status_code(),
        409,
        "error should be 409 DuplicateDependency, got: {} ({})",
        err.status_code(),
        err
    );

    // Cleanup.
    let _ = client
        .remove_blocked_by(issue_a.number, issue_b.number)
        .await;
    client
        .close_issue(issue_a.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close A should succeed");
    guard_a.disarm();
    client
        .close_issue(issue_b.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close B should succeed");
    guard_b.disarm();

    eprintln!("duplicate_dependency test: second add correctly rejected — verified");
}

#[tokio::test]
async fn add_blocked_by_returns_issue_not_found_for_nonexistent_number() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Both issues non-existent.
    let result = client.add_blocked_by(999_999_999, 999_999_998).await;
    assert!(
        result.is_err(),
        "add_blocked_by with non-existent issues should fail"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.status_code(),
        404,
        "error should be 404 IssueNotFound, got: {} ({})",
        err.status_code(),
        err
    );
}

#[tokio::test]
async fn remove_blocked_by_returns_issue_not_found_for_nonexistent_number() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let result = client.remove_blocked_by(999_999_999, 999_999_998).await;
    assert!(
        result.is_err(),
        "remove_blocked_by with non-existent issues should fail"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.status_code(),
        404,
        "error should be 404 IssueNotFound, got: {} ({})",
        err.status_code(),
        err
    );
}

// ── mutations: add_sub_issue ────────────────────────────────────────

#[tokio::test]
async fn add_sub_issue_creates_parent_child_relationship() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Create parent and child issues.
    let parent = client
        .create_issue(CreateIssueParams {
            title: "[test] add_sub_issue parent".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create parent should succeed");

    let mut guard_parent = CloseIssueGuard::new(&client, parent.number);

    let child = client
        .create_issue(CreateIssueParams {
            title: "[test] add_sub_issue child".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create child should succeed");

    let mut guard_child = CloseIssueGuard::new(&client, child.number);

    // Add sub-issue relationship.
    client
        .add_sub_issue(parent.number, child.number)
        .await
        .expect("add_sub_issue should succeed");

    // Re-fetch parent and verify child appears in subIssues.
    let refetched_parent = client
        .fetch_issue(parent.number)
        .await
        .expect("fetch_issue parent should succeed after add_sub_issue");

    let has_child = refetched_parent
        .sub_issues
        .iter()
        .any(|r| r.number == child.number);
    assert!(
        has_child,
        "parent should show child in sub_issues, got: {:?}",
        refetched_parent
            .sub_issues
            .iter()
            .map(|r| r.number)
            .collect::<Vec<_>>()
    );

    // Cleanup.
    client
        .close_issue(child.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close child should succeed");
    guard_child.disarm();
    client
        .close_issue(parent.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close parent should succeed");
    guard_parent.disarm();

    eprintln!(
        "add_sub_issue test: #{} is sub-issue of #{} — verified",
        child.number, parent.number
    );
}

#[tokio::test]
async fn add_sub_issue_returns_issue_not_found_for_nonexistent_number() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let result = client.add_sub_issue(999_999_999, 999_999_998).await;
    assert!(
        result.is_err(),
        "add_sub_issue with non-existent issues should fail"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.status_code(),
        404,
        "error should be 404 IssueNotFound, got: {} ({})",
        err.status_code(),
        err
    );
}

// ── Projects V2: resolve_project_info ───────────────────────────────

#[tokio::test]
async fn resolve_project_info_returns_project_id_and_number() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping project integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info() should succeed");

    assert!(
        !project_info.id.is_empty(),
        "project ID should be non-empty"
    );
    assert!(project_info.number > 0, "project number should be positive");

    eprintln!(
        "resolve_project_info: id={}, number={}",
        project_info.id, project_info.number
    );
}

// ── Projects V2: setup_fields ───────────────────────────────────────

#[tokio::test]
async fn setup_fields_creates_all_seven_fields() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping project integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info() should succeed");

    let field_ids = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields() should succeed");

    // Verify all 7 fields have non-empty IDs.
    assert!(
        !field_ids.status.field_id.is_empty(),
        "Status field_id should be non-empty"
    );
    assert!(
        !field_ids.priority.field_id.is_empty(),
        "Priority field_id should be non-empty"
    );
    assert!(
        !field_ids.issue_type.field_id.is_empty(),
        "IssueType field_id should be non-empty"
    );
    assert!(
        !field_ids.agent.is_empty(),
        "Agent field_id should be non-empty"
    );
    assert!(
        !field_ids.story_points.is_empty(),
        "StoryPoints field_id should be non-empty"
    );
    assert!(
        !field_ids.defer_until.is_empty(),
        "DeferUntil field_id should be non-empty"
    );
    assert!(
        !field_ids.ready_state.field_id.is_empty(),
        "ReadyState field_id should be non-empty"
    );

    // Verify single-select fields have the correct options.
    assert_eq!(
        field_ids.status.options.len(),
        5,
        "Status should have 5 options, got: {:?}",
        field_ids.status.options.keys().collect::<Vec<_>>()
    );
    for expected in &["Backlog", "In Progress", "Done", "Blocked", "Deferred"] {
        assert!(
            field_ids.status.options.contains_key(*expected),
            "Status should have option '{expected}', got: {:?}",
            field_ids.status.options.keys().collect::<Vec<_>>()
        );
    }

    assert_eq!(
        field_ids.priority.options.len(),
        5,
        "Priority should have 5 options"
    );
    for expected in &["P0", "P1", "P2", "P3", "P4"] {
        assert!(
            field_ids.priority.options.contains_key(*expected),
            "Priority should have option '{expected}'"
        );
    }

    assert_eq!(
        field_ids.issue_type.options.len(),
        5,
        "IssueType should have 5 options"
    );
    for expected in &["Task", "Bug", "Feature", "Epic", "Chore"] {
        assert!(
            field_ids.issue_type.options.contains_key(*expected),
            "IssueType should have option '{expected}'"
        );
    }

    assert_eq!(
        field_ids.ready_state.options.len(),
        2,
        "ReadyState should have 2 options"
    );
    for expected in &["Ready", "Not Ready"] {
        assert!(
            field_ids.ready_state.options.contains_key(*expected),
            "ReadyState should have option '{expected}'"
        );
    }

    eprintln!("setup_fields: all 7 fields created with correct options");
}

#[tokio::test]
async fn setup_fields_is_idempotent() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping project integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info() should succeed");

    // First call — creates or finds fields.
    let first_ids = client
        .setup_fields(&project_info.id)
        .await
        .expect("first setup_fields() should succeed");

    // Second call — should skip existing fields (idempotent).
    let second_ids = client
        .setup_fields(&project_info.id)
        .await
        .expect("second setup_fields() should succeed");

    // Field IDs should be identical between runs.
    assert_eq!(
        first_ids.status.field_id, second_ids.status.field_id,
        "Status field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.priority.field_id, second_ids.priority.field_id,
        "Priority field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.issue_type.field_id, second_ids.issue_type.field_id,
        "IssueType field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.agent, second_ids.agent,
        "Agent field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.story_points, second_ids.story_points,
        "StoryPoints field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.defer_until, second_ids.defer_until,
        "DeferUntil field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.ready_state.field_id, second_ids.ready_state.field_id,
        "ReadyState field_id should be stable across calls"
    );

    eprintln!("setup_fields idempotent: field IDs match across two calls");
}

// ── Projects V2: update_field ───────────────────────────────────────

#[tokio::test]
async fn update_field_changes_value_on_project_item() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping project integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info() should succeed");

    let field_ids = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields() should succeed");

    // Create an issue to test field updates on.
    let issue = client
        .create_issue(CreateIssueParams {
            title: "[test] update_field integration test".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: vec![],
        })
        .await
        .expect("create_issue should succeed");

    let mut guard = CloseIssueGuard::new(&client, issue.number);

    // The issue must be added to the project to get a ProjectV2Item ID.
    // create_issue already adds it if UNBLOCK_PROJECT is set. We need the
    // item ID — fetch it via GraphQL.
    let item_id = fetch_project_item_id(&client, &project_info.id, &issue.node_id).await;

    if item_id.is_empty() {
        eprintln!(
            "Could not find ProjectV2Item for issue #{} — skipping update_field test",
            issue.number
        );
        client
            .close_issue(issue.number, Some("Test cleanup".to_owned()))
            .await
            .expect("close should succeed");
        guard.disarm();
        return;
    }

    // Update the Priority field to P1.
    let p1_option_id = field_ids
        .priority
        .options
        .get("P1")
        .expect("P1 option should exist");

    client
        .update_field(
            &project_info.id,
            &item_id,
            &field_ids.priority.field_id,
            &FieldValue::SingleSelectOption(p1_option_id.clone()),
        )
        .await
        .expect("update_field(Priority=P1) should succeed");

    // Update the Agent text field.
    client
        .update_field(
            &project_info.id,
            &item_id,
            &field_ids.agent,
            &FieldValue::Text("test-agent".to_owned()),
        )
        .await
        .expect("update_field(Agent=test-agent) should succeed");

    // Cleanup.
    client
        .close_issue(issue.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close should succeed");
    guard.disarm();

    eprintln!(
        "update_field test: updated Priority and Agent on issue #{} — verified",
        issue.number
    );
}

// ── Projects V2: field_ids caching ──────────────────────────────────

#[tokio::test]
async fn field_ids_cached_on_client_after_setup() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping project integration test");
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    // Before setup, field_ids should be None.
    assert!(
        client.field_ids().await.is_none(),
        "field_ids should be None before setup_fields"
    );

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info() should succeed");

    let field_ids = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields() should succeed");

    // Cache the field_ids.
    client.set_field_ids(field_ids.clone()).await;

    // After caching, field_ids should be Some.
    let cached = client
        .field_ids()
        .await
        .expect("field_ids should be Some after set_field_ids");

    assert_eq!(
        cached.status.field_id, field_ids.status.field_id,
        "cached Status field_id should match"
    );
    assert_eq!(
        cached.agent, field_ids.agent,
        "cached Agent field_id should match"
    );

    eprintln!("field_ids caching: verified cache populated after setup_fields");
}

/// Fetches the `ProjectV2Item` ID for a given issue node ID within a project.
///
/// Uses the public `http()` and `graphql_url()` accessors on [`GitHubClient`]
/// because the `graphql()` helper is `pub(crate)` and not accessible from
/// integration tests.
async fn fetch_project_item_id(
    client: &GitHubClient,
    project_id: &str,
    issue_node_id: &str,
) -> String {
    let query = "
        query ProjectItemId($nodeId: ID!) {
            node(id: $nodeId) {
                ... on Issue {
                    projectItems(first: 10) {
                        nodes {
                            id
                            project {
                                id
                            }
                        }
                    }
                }
            }
        }
    ";

    let body = serde_json::json!({
        "query": query,
        "variables": { "nodeId": issue_node_id },
    });

    let response: serde_json::Value = client
        .http()
        .post(client.graphql_url())
        .json(&body)
        .send()
        .await
        .expect("GraphQL request should succeed")
        .json()
        .await
        .expect("GraphQL response should be valid JSON");

    let nodes = response["data"]["node"]["projectItems"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    for node in &nodes {
        if node["project"]["id"].as_str() == Some(project_id) {
            return node["id"].as_str().unwrap_or_default().to_owned();
        }
    }

    String::new()
}
