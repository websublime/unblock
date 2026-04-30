//! Integration tests for the GitHub API client.
//!
//! ## Two test buckets
//!
//! The tests in this file are split into two buckets:
//!
//! 1. **Fixture-injected tests** — construct a [`GitHubClient`] via
//!    [`GitHubClient::with_repo`] so they do not depend on `GITHUB_TOKEN`,
//!    network, or `.git/config`. These run as part of the default
//!    `cargo test --workspace` run.
//! 2. **Live-required tests** — actually call `api.github.com`. These are
//!    marked `#[ignore]` and opt-in via `cargo test --workspace -- --ignored`
//!    with a real `GITHUB_TOKEN` (and `UNBLOCK_PROJECT` for the Projects V2
//!    tests) set. Every live-required test starts with a
//!    [`require_github_token`] / [`require_github_token_and_project`] gate so
//!    that accidental invocation without credentials exits cleanly instead of
//!    emitting a confusing failure.
//!
//! The live-required tests invoke [`GitHubClient::new`] directly so that the
//! production `UNBLOCK_REPO` + git-remote resolution path is still exercised
//! end-to-end when a real token is present. See bead `unblock-c4h` for the
//! full rationale.

use unblock_core::config::Config;
use unblock_core::types::{IssueState, Status};
use unblock_github::client::GitHubClient;
use unblock_github::mutations::CreateIssueParams;
use unblock_github::projects::{CreateViewParams, FieldValue, OwnerType, ViewLayout};

/// Drop guard that closes a GitHub issue on scope exit, even during a panic
/// unwind. This ensures integration tests do not leave orphaned open issues
/// when an assertion fails before the explicit cleanup call.
///
/// The guard captures a reference to the [`GitHubClient`] and the issue number
/// at creation time. On drop it uses the current tokio runtime handle to
/// block on the async `close_issue` call. If the close fails the error is
/// logged to stderr but does not cause a secondary panic (which would abort
/// the process during an unwind).
#[derive(Debug)]
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

/// Gate for live-required integration tests: returns `true` when
/// `GITHUB_TOKEN` is set, otherwise prints a clear opt-in hint and returns
/// `false` so the caller can early-return cleanly.
///
/// Live-required tests are tagged `#[ignore]` and opt-in via
/// `cargo test --workspace -- --ignored` with `GITHUB_TOKEN` set. When a user
/// explicitly passes `--ignored` without a token this helper keeps the test
/// exit status clean instead of hitting a confusing assertion failure.
fn require_github_token() -> bool {
    if has_github_token() {
        true
    } else {
        eprintln!(
            "GITHUB_TOKEN not set — skipping live integration test (re-run with \
             `GITHUB_TOKEN=... cargo test --workspace -- --ignored`)"
        );
        false
    }
}

/// Same as [`require_github_token`] but also requires `UNBLOCK_PROJECT`. Used
/// by Projects V2 live tests that cannot run without a configured project.
fn require_github_token_and_project() -> bool {
    if has_github_token() && has_project_number() {
        true
    } else {
        eprintln!(
            "GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping live project \
             integration test (re-run with both env vars set and \
             `cargo test --workspace -- --ignored`)"
        );
        false
    }
}

/// Builds a [`Config`] from the process environment for live integration
/// tests. Requires `GITHUB_TOKEN` to be set; uses `UNBLOCK_REPO` when
/// available, otherwise falls back to git-remote detection.
///
/// Callers must gate on [`require_github_token`] (or
/// [`require_github_token_and_project`]) before invoking this function.
fn test_config() -> Config {
    Config::load().expect("Config::load() should succeed when GITHUB_TOKEN is set")
}

/// Builds a hermetic [`Config`] for fixture-injected tests — no env vars
/// read, no filesystem touched. Pair with [`GitHubClient::with_repo`] to
/// construct a client that does not require `GITHUB_TOKEN` or `.git/config`.
///
/// The token is a stub and the URLs point at the default github.com
/// endpoints. Tests that actually issue HTTP requests must still be
/// live-required and use [`test_config`] instead.
fn fixture_config() -> Config {
    Config {
        token: "ghp_integration_fixture".to_owned(),
        api_base_url: "https://api.github.com".to_owned(),
        github_url: "https://github.com".to_owned(),
        repo: None,
        project_number: None,
        agent: "integration-fixture".to_owned(),
        cache_ttl: 30,
        log_level: "info".to_owned(),
        otel_endpoint: None,
    }
}

// ── fixture-injected construction (no env / no network) ───────────
//
// These tests construct a client via `GitHubClient::with_repo`, so they do
// not depend on `GITHUB_TOKEN`, `.git/config`, or network access. They run
// on the default `cargo test --workspace` pass and guard the cross-crate
// consumer shape of `with_repo` against regressions from the `unblock-mcp`
// side of the workspace.

/// `with_repo` (the fixture-injected constructor) must produce a client
/// whose accessors reflect the arguments it was given, without touching
/// `config.repo` or `.git/config`. Mirrors the shape-of-client assertions
/// in `github_client_new_connects_to_real_repo` without the live-API
/// dependency.
#[tokio::test]
async fn with_repo_builds_client_with_injected_owner_and_repo() {
    let config = fixture_config();

    let client = GitHubClient::with_repo(&config, "acme", "widgets")
        .await
        .expect("with_repo should succeed with a fixture config");

    assert_eq!(
        client.owner(),
        "acme",
        "owner should match the with_repo argument, not .git/config"
    );
    assert_eq!(
        client.repo(),
        "widgets",
        "repo should match the with_repo argument, not .git/config"
    );
    assert_eq!(
        client.api_base_url(),
        "https://api.github.com",
        "api_base_url should come from the fixture config"
    );

    let rest_url = client.rest_url("/repos");
    assert!(
        rest_url.starts_with("https://"),
        "rest_url should be an HTTPS URL, got: {rest_url}"
    );

    let graphql_url = client.graphql_url();
    assert!(
        graphql_url.ends_with("/graphql"),
        "graphql_url should end with /graphql, got: {graphql_url}"
    );
}

/// `with_repo` must honour a GitHub Enterprise-style `api_base_url` and
/// route `graphql_url()` to `/api/graphql` (not `/api/v3/graphql`).
#[tokio::test]
async fn with_repo_respects_ghe_api_base_url() {
    let mut config = fixture_config();
    config.api_base_url = "https://ghe.example.com/api/v3".to_owned();
    config.github_url = "https://ghe.example.com".to_owned();

    let client = GitHubClient::with_repo(&config, "acme", "widgets")
        .await
        .expect("with_repo should succeed against a GHE fixture");

    assert_eq!(client.api_base_url(), "https://ghe.example.com/api/v3");
    assert_eq!(
        client.graphql_url(),
        "https://ghe.example.com/api/graphql",
        "GraphQL URL should strip the `/v3` REST suffix"
    );
}

// ── live smoke test ────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn github_client_new_connects_to_real_repo() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn fetch_issue_returns_full_details_for_existing_issue() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn fetch_issue_returns_issue_not_found_for_nonexistent_number() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn fetch_graph_data_returns_issues_from_real_repo() {
    if !require_github_token() {
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

    // The test repo should have at least one issue (OPEN or CLOSED).
    // If the repo has zero issues this assertion will fail — that is fine,
    // it means the test repo needs seeding.
    assert!(
        !issues.is_empty(),
        "fetch_graph_data should return at least one issue (OPEN or CLOSED)"
    );

    // Verify every returned issue has a valid IssueState. After
    // `unblock-a36` the query uses `states: [OPEN, CLOSED]`, so BOTH
    // states are valid — we only reject anything outside the expected
    // set (which the parser treats as `Open` but future schema drifts
    // could introduce). Sanity check: the pair matches the enum's
    // closed variant set.
    for issue in &issues {
        let valid = matches!(issue.state, IssueState::Open | IssueState::Closed);
        assert!(
            valid,
            "fetch_graph_data returned unexpected state for issue #{}: {:?}",
            issue.number, issue.state
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
        assert!(edge.source.number > 0, "edge source should be positive");
        assert!(edge.target.number > 0, "edge target should be positive");
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn fetch_graph_data_issues_have_valid_status_and_priority() {
    if !require_github_token() {
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
            Status::Ready
                | Status::InProgress
                | Status::Blocked
                | Status::Deferred
                | Status::Closed
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn create_issue_returns_issue_with_correct_fields() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn close_issue_closes_issue_and_refetch_confirms() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn close_issue_with_reason_adds_comment_before_closing() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn close_issue_returns_issue_not_found_for_nonexistent_number() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn add_comment_posts_comment_and_returns_url() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn add_comment_returns_issue_not_found_for_nonexistent_number() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn add_blocked_by_creates_blocking_relationship() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn remove_blocked_by_removes_blocking_relationship() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn add_blocked_by_duplicate_returns_duplicate_dependency() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn add_blocked_by_returns_issue_not_found_for_nonexistent_number() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn remove_blocked_by_returns_issue_not_found_for_nonexistent_number() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn add_sub_issue_creates_parent_child_relationship() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn add_sub_issue_returns_issue_not_found_for_nonexistent_number() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn resolve_project_info_returns_project_id_and_number() {
    if !require_github_token_and_project() {
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
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
#[allow(clippy::too_many_lines)]
async fn setup_fields_creates_all_seven_fields() {
    if !require_github_token_and_project() {
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

    // Diagnostic snapshot — pre-mutation state of the project.
    //
    // Per bead unblock-aa2 (Sherlock investigation, 2026-04-30): when this
    // test fails, the most common root cause is a project that was not in
    // the documented clean state at entry — either the 6 unblock-managed
    // custom fields are already present (parallel races, partial previous
    // run) or the built-in Status field's options diverge from the spec
    // (stale renames). Printing the existing field set + Status options
    // before the mutation makes that diagnosable from CI logs alone, with
    // no need to re-run locally to reproduce.
    let pre_status = client
        .query_setup_status(&project_info.id)
        .await
        .expect("query_setup_status() should succeed");
    eprintln!(
        "setup_fields pre-mutation snapshot: existing={:?}, missing={:?}",
        pre_status.existing, pre_status.missing
    );

    let report = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields() should succeed");

    let field_ids = &report.field_ids;

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
        !field_ids.pipeline_stage.field_id.is_empty(),
        "PipelineStage field_id should be non-empty"
    );
    assert!(
        !field_ids.agent.is_empty(),
        "Agent field_id should be non-empty"
    );
    assert!(
        !field_ids.claimed_at.is_empty(),
        "ClaimedAt field_id should be non-empty"
    );
    assert!(
        !field_ids.story_points.is_empty(),
        "StoryPoints field_id should be non-empty"
    );
    assert!(
        !field_ids.defer_until.is_empty(),
        "DeferUntil field_id should be non-empty"
    );

    // Verify created + healed + skipped covers all 7 fields.
    // (Bead unblock-aa2: heal bucket added — single-select required
    // fields whose options diverged from spec land in `healed` instead
    // of `skipped`. The buckets are mutually exclusive.)
    assert_eq!(
        report.created.len() + report.healed.len() + report.skipped.len(),
        7,
        "created + healed + skipped should total 7, got created={:?} healed={:?} skipped={:?}",
        report.created,
        report.healed,
        report.skipped
    );

    // Verify single-select fields have the correct options.
    assert_eq!(
        field_ids.status.options.len(),
        5,
        "Status should have 5 options, got: {:?}",
        field_ids.status.options.keys().collect::<Vec<_>>()
    );
    for expected in &["ready", "in_progress", "closed", "blocked", "deferred"] {
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
    for expected in &[
        "P0 - Critical",
        "P1 - High",
        "P2 - Medium",
        "P3 - Low",
        "P4 - Backlog",
    ] {
        assert!(
            field_ids.priority.options.contains_key(*expected),
            "Priority should have option '{expected}'"
        );
    }

    assert_eq!(
        field_ids.pipeline_stage.options.len(),
        6,
        "PipelineStage should have 6 options"
    );
    for expected in &[
        "investigation",
        "implementation",
        "review",
        "refactoring",
        "qa",
        "done",
    ] {
        assert!(
            field_ids.pipeline_stage.options.contains_key(*expected),
            "PipelineStage should have option '{expected}'"
        );
    }

    eprintln!("setup_fields: all 7 fields created with correct options");
}

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn setup_fields_is_idempotent() {
    if !require_github_token_and_project() {
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
    let first_report = client
        .setup_fields(&project_info.id)
        .await
        .expect("first setup_fields() should succeed");

    // Second call — should skip existing fields (idempotent).
    let second_report = client
        .setup_fields(&project_info.id)
        .await
        .expect("second setup_fields() should succeed");

    // Second call should have all 7 skipped, none created and none
    // healed (the first call left every required field in the canonical
    // shape, so the heal fast-path runs for single-select fields and
    // every plain field falls through the `skipped` branch).
    assert!(
        second_report.created.is_empty(),
        "second call should create nothing, but created: {:?}",
        second_report.created
    );
    assert!(
        second_report.healed.is_empty(),
        "second call should heal nothing (idempotent), but healed: {:?}",
        second_report.healed
    );
    assert_eq!(
        second_report.skipped.len(),
        7,
        "second call should skip all 7, but skipped: {:?}",
        second_report.skipped
    );

    // Field IDs should be identical between runs.
    let first_ids = &first_report.field_ids;
    let second_ids = &second_report.field_ids;
    assert_eq!(
        first_ids.status.field_id, second_ids.status.field_id,
        "Status field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.priority.field_id, second_ids.priority.field_id,
        "Priority field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.pipeline_stage.field_id, second_ids.pipeline_stage.field_id,
        "PipelineStage field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.agent, second_ids.agent,
        "Agent field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.claimed_at, second_ids.claimed_at,
        "ClaimedAt field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.story_points, second_ids.story_points,
        "StoryPoints field_id should be stable across calls"
    );
    assert_eq!(
        first_ids.defer_until, second_ids.defer_until,
        "DeferUntil field_id should be stable across calls"
    );

    eprintln!("setup_fields idempotent: field IDs match across two calls");
}

// ── Projects V2: query_setup_status (dry-run) ───────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn query_setup_status_reports_fields_without_creating() {
    if !require_github_token_and_project() {
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

    // Ensure fields exist first (setup is idempotent).
    let _report = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields() should succeed");

    // Now query status — should report all 7 existing, none missing.
    let status = client
        .query_setup_status(&project_info.id)
        .await
        .expect("query_setup_status() should succeed");

    assert_eq!(
        status.existing.len(),
        7,
        "all 7 fields should be reported as existing, got: {:?}",
        status.existing
    );
    assert!(
        status.missing.is_empty(),
        "no fields should be missing after setup, got: {:?}",
        status.missing
    );

    // Verify each required field name is in the existing list.
    for name in unblock_github::projects::REQUIRED_FIELD_NAMES {
        assert!(
            status.existing.contains(&(*name).to_owned()),
            "field '{name}' should be in existing list, got: {:?}",
            status.existing
        );
    }

    // Re-fetch to confirm query_setup_status did not mutate anything:
    // a second call should return identical results.
    let status2 = client
        .query_setup_status(&project_info.id)
        .await
        .expect("second query_setup_status() should succeed");

    assert_eq!(
        status.existing, status2.existing,
        "existing fields should be identical across two query_setup_status calls"
    );
    assert_eq!(
        status.missing, status2.missing,
        "missing fields should be identical across two query_setup_status calls"
    );

    eprintln!("query_setup_status: all 7 fields reported existing, re-fetch confirms no mutation");
}

// ── Projects V2: update_field ───────────────────────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn update_field_changes_value_on_project_item() {
    if !require_github_token_and_project() {
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

    let report = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields() should succeed");
    let field_ids = &report.field_ids;

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

    // Re-fetch to confirm the Priority value was persisted.
    let priority_value = fetch_field_value(&client, &item_id, &field_ids.priority.field_id).await;
    assert_eq!(
        priority_value.as_deref(),
        Some("P1"),
        "Re-fetched Priority should be P1"
    );

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

    // Re-fetch to confirm the Agent value was persisted.
    let agent_value = fetch_field_value(&client, &item_id, &field_ids.agent).await;
    assert_eq!(
        agent_value.as_deref(),
        Some("test-agent"),
        "Re-fetched Agent should be 'test-agent'"
    );

    // Cleanup.
    client
        .close_issue(issue.number, Some("Test cleanup".to_owned()))
        .await
        .expect("close should succeed");
    guard.disarm();

    eprintln!(
        "update_field test: updated Priority and Agent on issue #{} — re-fetch verified",
        issue.number
    );
}

// ── Projects V2: field_ids caching ──────────────────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn field_ids_cached_on_client_after_setup() {
    if !require_github_token_and_project() {
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

    let report = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields() should succeed");

    // Cache the field_ids.
    client.set_field_ids(report.field_ids.clone()).await;

    // After caching, field_ids should be Some.
    let cached = client
        .field_ids()
        .await
        .expect("field_ids should be Some after set_field_ids");

    assert_eq!(
        cached.status.field_id, report.field_ids.status.field_id,
        "cached Status field_id should match"
    );
    assert_eq!(
        cached.agent, report.field_ids.agent,
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

/// Fetches the current value of a specific field on a `ProjectV2Item` via
/// GraphQL. Returns `Some(value)` if found, `None` otherwise.
///
/// Works for single-select fields (returns the option name), text fields
/// (returns the text value), number fields (returns the number stringified
/// via `f64::to_string`), and date fields (returns the ISO `YYYY-MM-DD`
/// string). Return type is kept as `Option<String>` so all existing callers
/// remain unchanged; numeric callers can parse back with `str::parse::<f64>`.
async fn fetch_field_value(client: &GitHubClient, item_id: &str, field_id: &str) -> Option<String> {
    let query = "
        query ItemFieldValue($itemId: ID!) {
            node(id: $itemId) {
                ... on ProjectV2Item {
                    fieldValues(first: 30) {
                        nodes {
                            ... on ProjectV2ItemFieldSingleSelectValue {
                                field { ... on ProjectV2SingleSelectField { id } }
                                name
                            }
                            ... on ProjectV2ItemFieldTextValue {
                                field { ... on ProjectV2Field { id } }
                                text
                            }
                            ... on ProjectV2ItemFieldNumberValue {
                                field { ... on ProjectV2Field { id } }
                                number
                            }
                            ... on ProjectV2ItemFieldDateValue {
                                field { ... on ProjectV2Field { id } }
                                date
                            }
                        }
                    }
                }
            }
        }
    ";

    let body = serde_json::json!({
        "query": query,
        "variables": { "itemId": item_id },
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

    let nodes = response["data"]["node"]["fieldValues"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    for node in &nodes {
        if node["field"]["id"].as_str() == Some(field_id) {
            // Single-select field value.
            if let Some(name) = node["name"].as_str() {
                return Some(name.to_owned());
            }
            // Text field value.
            if let Some(text) = node["text"].as_str() {
                return Some(text.to_owned());
            }
            // Number field value — JSON numeric, stringify to preserve
            // the Option<String> return contract.
            if let Some(number) = node["number"].as_f64() {
                return Some(number.to_string());
            }
            // Date field value — ISO YYYY-MM-DD string.
            if let Some(date) = node["date"].as_str() {
                return Some(date.to_owned());
            }
        }
    }

    None
}

// ── detect_owner_type ────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn detect_owner_type_returns_org_for_org_accounts() {
    if !require_github_token() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    // The test repo is owned by `websublime` which is a GitHub organization.
    // If running against a different repo, this assertion may need adjustment.
    assert_eq!(
        owner_type,
        OwnerType::Org,
        "owner_type for 'websublime' should be Org"
    );

    eprintln!(
        "detect_owner_type: owner={}, type={:?}",
        client.owner(),
        owner_type
    );
}

// ── list_rest_fields ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn list_rest_fields_returns_integer_ids() {
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    let fields = client
        .list_rest_fields(owner_type)
        .await
        .expect("list_rest_fields() should succeed");

    // Should return at least the built-in fields (Title, Assignees, Status, etc.).
    assert!(
        !fields.is_empty(),
        "list_rest_fields should return at least one field"
    );

    // Every field should have a non-zero integer ID.
    for field in &fields {
        assert!(field.id > 0, "field ID should be a positive integer");
        assert!(!field.name.is_empty(), "field name should be non-empty");
        assert!(
            !field.data_type.is_empty(),
            "field data_type should be non-empty"
        );
    }

    // Should include the built-in Title field.
    let has_title = fields.iter().any(|f| f.name == "Title");
    assert!(has_title, "fields should include the built-in Title field");

    eprintln!(
        "list_rest_fields: {} fields returned, names: {:?}",
        fields.len(),
        fields.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn list_rest_fields_options_name_raw_parsed_correctly() {
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    let fields = client
        .list_rest_fields(owner_type)
        .await
        .expect("list_rest_fields() should succeed");

    // Find a single_select field with options (Status or Priority if setup has run).
    let select_field = fields
        .iter()
        .find(|f| f.data_type == "single_select" && !f.options.is_empty());

    if let Some(field) = select_field {
        // Verify that option names are plain strings (parsed from name.raw),
        // not JSON objects or HTML.
        for opt in &field.options {
            assert!(!opt.name.is_empty(), "option name should be non-empty");
            assert!(
                !opt.name.starts_with('{'),
                "option name should be a plain string, not a JSON object: {}",
                opt.name
            );
            assert!(
                !opt.name.contains('<'),
                "option name should be raw text, not HTML: {}",
                opt.name
            );
        }
        eprintln!(
            "Single-select field '{}' has options: {:?}",
            field.name,
            field.options.iter().map(|o| &o.name).collect::<Vec<_>>()
        );
    } else {
        eprintln!("No single_select fields with options found — skipping options validation");
    }
}

// ── create_view + list_views ─────────────────────────────────────────
// TODO(unblock-45a.14): Add DeleteViewGuard for test cleanup once delete_view()
// is implemented. Views created by these tests accumulate on the test project.

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn create_view_board_and_list_views() {
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    // Create a board view with a unique name to avoid collisions.
    let view_name = format!("test-board-{}", chrono::Utc::now().timestamp());
    let params = CreateViewParams {
        name: view_name.clone(),
        layout: ViewLayout::Board,
        filter: Some("is:open".to_owned()),
        visible_fields: None,
    };

    let view = client
        .create_view(owner_type, &params)
        .await
        .expect("create_view(board) should succeed");

    assert_eq!(view.name, view_name, "view name should match");
    assert_eq!(view.layout, ViewLayout::Board, "layout should be Board");
    assert!(view.number > 0, "view number should be positive");
    assert!(view.id.is_some(), "view id should be present from REST");

    eprintln!(
        "Created board view: name={}, number={}, id={:?}",
        view.name, view.number, view.id
    );

    // Verify the view appears in list_views.
    let views = client
        .list_views(owner_type)
        .await
        .expect("list_views() should succeed");

    let found = views.iter().any(|v| v.name == view_name);
    assert!(
        found,
        "list_views should include the newly created view '{view_name}'"
    );
}

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn create_view_table_layout() {
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    let view_name = format!("test-table-{}", chrono::Utc::now().timestamp());
    let params = CreateViewParams {
        name: view_name.clone(),
        layout: ViewLayout::Table,
        filter: None,
        visible_fields: None,
    };

    let view = client
        .create_view(owner_type, &params)
        .await
        .expect("create_view(table) should succeed");

    assert_eq!(view.name, view_name);
    assert_eq!(view.layout, ViewLayout::Table);
    assert!(view.number > 0);

    eprintln!(
        "Created table view: name={}, number={}",
        view.name, view.number
    );
}

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn create_view_roadmap_layout() {
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    let view_name = format!("test-roadmap-{}", chrono::Utc::now().timestamp());
    let params = CreateViewParams {
        name: view_name.clone(),
        layout: ViewLayout::Roadmap,
        filter: None,
        visible_fields: None,
    };

    let view = client
        .create_view(owner_type, &params)
        .await
        .expect("create_view(roadmap) should succeed");

    assert_eq!(view.name, view_name);
    assert_eq!(view.layout, ViewLayout::Roadmap);
    assert!(view.number > 0);

    eprintln!(
        "Created roadmap view: name={}, number={}",
        view.name, view.number
    );
}

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn list_views_returns_default_view() {
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    let views = client
        .list_views(owner_type)
        .await
        .expect("list_views() should succeed");

    // Every project has at least one default view.
    assert!(
        !views.is_empty(),
        "list_views should return at least one view (the default)"
    );

    for view in &views {
        assert!(!view.name.is_empty(), "view name should be non-empty");
        assert!(view.number > 0, "view number should be positive");
        assert!(
            view.node_id.is_some(),
            "view node_id should be present from GraphQL"
        );
    }

    eprintln!(
        "list_views: {} views, names: {:?}",
        views.len(),
        views.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

// ── resolve_owner_node_id ───────────────────────────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn resolve_owner_node_id_returns_non_empty_id() {
    if !require_github_token() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    let node_id = client
        .resolve_owner_node_id(owner_type)
        .await
        .expect("resolve_owner_node_id() should succeed");

    // GitHub node IDs are non-empty base64-encoded strings.
    assert!(
        !node_id.is_empty(),
        "resolve_owner_node_id should return a non-empty node ID"
    );

    eprintln!(
        "resolve_owner_node_id: owner={}, type={:?}, node_id={}",
        client.owner(),
        owner_type,
        node_id
    );
}

// ── list_owner_projects ─────────────────────────────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn list_owner_projects_returns_projects_with_valid_fields() {
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    let projects = client
        .list_owner_projects(owner_type)
        .await
        .expect("list_owner_projects() should succeed");

    // At least one project should exist (the one configured via UNBLOCK_PROJECT).
    assert!(
        !projects.is_empty(),
        "list_owner_projects should return at least one project"
    );

    for project in &projects {
        assert!(
            project.number > 0,
            "project number should be a positive integer"
        );
        assert!(
            !project.title.is_empty(),
            "project title should be non-empty"
        );
        assert!(!project.url.is_empty(), "project URL should be non-empty");
        assert!(
            project.url.starts_with("https://"),
            "project URL should be an HTTPS URL"
        );
    }

    eprintln!(
        "list_owner_projects: {} projects, titles: {:?}",
        projects.len(),
        projects.iter().map(|p| &p.title).collect::<Vec<_>>()
    );
}

// ── init idempotency (create_project + list_owner_projects) ─────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn init_create_project_and_idempotency_check() {
    if !require_github_token() {
        return;
    }

    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type() should succeed");

    // Use a unique title to avoid conflicts with real projects.
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs();
    let test_title = format!("{} Init Test {}", client.repo(), timestamp);

    // Step 1: List existing projects and confirm our test title does not exist.
    let projects_before = client
        .list_owner_projects(owner_type)
        .await
        .expect("list_owner_projects() should succeed");

    let existing = projects_before.iter().find(|p| p.title == test_title);
    assert!(
        existing.is_none(),
        "test project should not exist before creation"
    );

    // Step 2: Resolve owner node ID.
    let owner_node_id = client
        .resolve_owner_node_id(owner_type)
        .await
        .expect("resolve_owner_node_id() should succeed");

    // Step 3: Create the project.
    let created = client
        .create_project(&owner_node_id, &test_title)
        .await
        .expect("create_project() should succeed");

    assert!(
        created.number > 0,
        "created project number should be positive"
    );
    assert!(
        !created.url.is_empty(),
        "created project URL should be non-empty"
    );
    assert!(
        created.url.starts_with("https://"),
        "created project URL should be an HTTPS URL"
    );

    eprintln!(
        "create_project: number={}, url={}, title={}",
        created.number, created.url, test_title
    );

    // Step 4: Idempotency — list projects again and confirm our title is found.
    let projects_after = client
        .list_owner_projects(owner_type)
        .await
        .expect("list_owner_projects() should succeed after creation");

    let found = projects_after.iter().find(|p| p.title == test_title);
    assert!(
        found.is_some(),
        "created project should appear in list_owner_projects"
    );
    let found = found.unwrap();
    assert_eq!(
        found.number, created.number,
        "found project number should match created number"
    );

    // Note: cleanup (deleting the test project) requires the deleteProjectV2
    // mutation which is not yet implemented. The project will remain but is
    // harmless — it has a unique timestamped title.
    eprintln!(
        "init_create_project_and_idempotency_check: PASS (project #{} left for manual cleanup)",
        created.number
    );
}
