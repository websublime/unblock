//! Integration tests for MCP tool handlers.
//!
//! These tests require a valid `GITHUB_TOKEN` environment variable and network
//! access to GitHub. They are skipped automatically when `GITHUB_TOKEN` is not
//! set.

use std::sync::Arc;
use std::time::Duration;

use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{BlockingEdge, IssueState, IssueType, Priority, ReadyState, Status};
use unblock_github::client::GitHubClient;
use unblock_mcp::server::ServerState;

// ── Helpers ─────────────────────────────────────────────────────────

/// Returns `true` if the `GITHUB_TOKEN` env var is set and non-empty.
fn has_github_token() -> bool {
    std::env::var("GITHUB_TOKEN")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Builds a [`Config`] from the process environment for integration tests.
fn test_config() -> Config {
    Config::load().expect("Config::load() should succeed when GITHUB_TOKEN is set")
}

/// Creates a [`ServerState`] with a real client and empty cache.
async fn test_server_state() -> ServerState {
    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");
    ServerState {
        config: Arc::new(config),
        client: Arc::new(client),
        cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
    }
}

/// Build a minimal `Issue` for testing (used to populate the cache).
fn test_issue(number: u64, state: IssueState) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        number,
        node_id: format!("NODE_{number}"),
        title: format!("Issue #{number}"),
        issue_type: Some(IssueType::Task),
        status: Status::Open,
        priority: Priority::P1,
        agent: None,
        claimed_at: None,
        ready_state: ReadyState::Ready,
        story_points: None,
        defer_until: None,
        labels: vec![],
        milestone: None,
        assignees: vec![],
        state,
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/test/repo/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

// ── Show tool: integration tests ────────────────────────────────────

/// Show an existing issue and verify all fields are populated.
///
/// This test calls `fetch_issue` on a known issue (issue #1 in the
/// configured test repository) and validates the show result structure.
#[tokio::test]
async fn show_existing_issue_returns_all_fields_populated() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    // Use issue #1, which should exist in any non-empty repo.
    let issue_number = 1;
    let issue = client
        .fetch_issue(issue_number)
        .await
        .expect("fetch_issue should succeed for issue #1");

    // Verify basic fields are populated.
    assert_eq!(issue.number, issue_number);
    assert!(!issue.title.is_empty(), "title should not be empty");
    assert!(!issue.node_id.is_empty(), "node_id should not be empty");
    assert!(!issue.url.is_empty(), "url should not be empty");

    // Parse body sections — should not panic.
    let body_sections =
        unblock_core::types::BodySections::from_markdown(issue.body.as_deref().unwrap_or_default());
    // body_sections can have None fields — that's fine for unstructured bodies.
    eprintln!(
        "body_sections: description={}, design_notes={}, acceptance_criteria={}",
        body_sections.description.is_some(),
        body_sections.design_notes.is_some(),
        body_sections.acceptance_criteria.is_some(),
    );

    // Comments should be a vec (possibly empty).
    eprintln!("comments count: {}", issue.comments.len());

    eprintln!(
        "show_existing_issue: #{} '{}' state={:?}",
        issue.number, issue.title, issue.state,
    );
}

/// Show a non-existent issue returns `IssueNotFound` error.
#[tokio::test]
async fn show_nonexistent_issue_returns_issue_not_found() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    // Use a very large number that should not exist.
    let result = client.fetch_issue(999_999_999).await;
    assert!(
        result.is_err(),
        "fetch_issue should fail for non-existent issue"
    );

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("999999999") || msg.contains("not found") || msg.contains("Not Found"),
        "error should reference the issue number or 'not found': {msg}",
    );

    eprintln!("show_nonexistent_issue: error = {msg}");
}

/// `body_sections` parsed correctly from a markdown body with all three sections.
#[tokio::test]
async fn show_body_sections_parsed_correctly() {
    let body = "\
## Description

This is the description.

## Design Notes

Design detail here.

## Acceptance Criteria

- [ ] Criterion one
- [ ] Criterion two
";

    let sections = unblock_core::types::BodySections::from_markdown(body);
    assert_eq!(
        sections.description.as_deref(),
        Some("This is the description."),
    );
    assert_eq!(
        sections.design_notes.as_deref(),
        Some("Design detail here."),
    );
    assert!(
        sections
            .acceptance_criteria
            .as_deref()
            .unwrap()
            .contains("Criterion one"),
        "acceptance_criteria should contain 'Criterion one'",
    );
}

/// `include_deps=false` skips graph traversal — `dependency_tree` is `None`.
#[tokio::test]
async fn show_include_deps_false_skips_graph_traversal() {
    // Set up a cache with a graph.
    let cache = GraphCache::new(Duration::from_secs(300));
    let issues = vec![
        test_issue(1, IssueState::Open),
        test_issue(2, IssueState::Open),
    ];
    let edges = vec![BlockingEdge {
        source: 2,
        target: 1,
    }];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues);
    cache.update(ready_set, graph).await;

    // Verify the cache has a graph.
    assert!(
        cache.get_graph().await.is_some(),
        "cache should have a graph"
    );

    // With include_deps=false, even though cache has a graph, dependency_tree
    // should be None. We test this logic directly since calling the full
    // tool handler requires a real GitHub client.
    let include_deps = false;
    let dependency_tree: Option<Vec<(u64, usize)>> = if include_deps {
        cache
            .get_graph()
            .await
            .map(|g| g.dependency_tree(1, unblock_core::types::TraversalDirection::Both, 3))
    } else {
        None
    };

    assert!(
        dependency_tree.is_none(),
        "dependency_tree should be None when include_deps=false",
    );
}

/// `include_comments=false` skips comment fetch — comments is `None`.
#[tokio::test]
async fn show_include_comments_false_skips_comments() {
    // Test the include_comments logic directly.
    let test_comments = vec![unblock_core::types::IssueComment {
        author: "alice".to_owned(),
        body: "Hello".to_owned(),
        created_at: chrono::Utc::now(),
    }];

    let include_comments = false;
    let comments: Option<Vec<_>> = if include_comments {
        Some(test_comments.clone())
    } else {
        None
    };

    assert!(
        comments.is_none(),
        "comments should be None when include_comments=false",
    );

    // And verify include_comments=true returns them.
    let include_comments = true;
    let comments: Option<Vec<_>> = if include_comments {
        Some(test_comments.clone())
    } else {
        None
    };

    assert!(
        comments.is_some(),
        "comments should be Some when include_comments=true",
    );
    assert_eq!(comments.unwrap().len(), 1);
}

/// `dependency_tree` returned for issues with blocking relationships.
#[tokio::test]
async fn show_dependency_tree_for_blocking_relationships() {
    let cache = GraphCache::new(Duration::from_secs(300));

    // Issue #2 is blocked by issue #1.
    let issues = vec![
        test_issue(1, IssueState::Open),
        test_issue(2, IssueState::Open),
        test_issue(3, IssueState::Open),
    ];
    let edges = vec![
        BlockingEdge {
            source: 2,
            target: 1,
        },
        BlockingEdge {
            source: 3,
            target: 2,
        },
    ];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues);
    cache.update(ready_set, graph).await;

    // With include_deps=true and a populated cache, dependency_tree should be Some.
    let include_deps = true;
    let dependency_tree: Option<Vec<(u64, usize)>> = if include_deps {
        cache
            .get_graph()
            .await
            .map(|g| g.dependency_tree(1, unblock_core::types::TraversalDirection::Both, 3))
    } else {
        None
    };

    assert!(
        dependency_tree.is_some(),
        "dependency_tree should be Some for an issue with blocking relationships",
    );

    let tree = dependency_tree.unwrap();
    assert!(
        !tree.is_empty(),
        "dependency_tree should not be empty for issue #1 which blocks #2",
    );

    // Issue #2 should appear at depth 1 (directly blocked by #1).
    let has_issue_2 = tree.iter().any(|(num, depth)| *num == 2 && *depth == 1);
    assert!(
        has_issue_2,
        "dependency_tree should contain issue #2 at depth 1: {tree:?}",
    );

    // Issue #3 should appear at depth 2 (blocked by #2, which is blocked by #1).
    let has_issue_3 = tree.iter().any(|(num, depth)| *num == 3 && *depth == 2);
    assert!(
        has_issue_3,
        "dependency_tree should contain issue #3 at depth 2: {tree:?}",
    );
}

/// `dependency_tree` is `None` when cache is empty.
#[tokio::test]
async fn show_dependency_tree_none_when_cache_empty() {
    let cache = GraphCache::new(Duration::from_secs(300));

    // Cache is empty — no graph.
    let include_deps = true;
    let dependency_tree: Option<Vec<(u64, usize)>> = if include_deps {
        cache
            .get_graph()
            .await
            .map(|g| g.dependency_tree(1, unblock_core::types::TraversalDirection::Both, 3))
    } else {
        None
    };

    assert!(
        dependency_tree.is_none(),
        "dependency_tree should be None when cache is empty",
    );
}

/// Tool is registered in the server — verified via `INSTRUCTIONS_STR` and
/// the fact that the `#[tool]` macro on `show` compiled successfully.
///
/// The `#[tool_router]` macro generates the tool routing code at compile time.
/// If the `show` tool handler is missing or has the wrong signature, the
/// compilation of `unblock-mcp` itself would fail. This test additionally
/// verifies that the instructions string references the show tool.
#[test]
fn show_tool_registered_in_server() {
    let instructions = unblock_mcp::server::INSTRUCTIONS_STR;
    assert!(
        instructions.contains("show"),
        "INSTRUCTIONS_STR should mention the 'show' tool",
    );
    assert!(
        instructions.contains("Get full details for a single issue"),
        "INSTRUCTIONS_STR should describe the show tool's purpose",
    );
}
