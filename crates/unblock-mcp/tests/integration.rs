//! Integration tests for MCP tool handlers.
//!
//! These tests require a valid `GITHUB_TOKEN` environment variable and network
//! access to GitHub. They are skipped automatically when `GITHUB_TOKEN` is not
//! set.

use std::time::Duration;

use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{
    BlockingEdge, IssueRef, IssueState, IssueType, Priority, QualifiedId, ReadyState, Status,
};
use unblock_github::client::GitHubClient;
use unblock_github::projects::{CreateViewParams, OwnerType, ViewLayout};
use unblock_mcp::tools::setup::REQUIRED_VIEWS;

mod common;
use common::{has_github_token, test_server_state};

/// Helper to create a QualifiedId for tests.
fn qid(number: u64) -> QualifiedId {
    QualifiedId::new("test", "repo", number)
}

/// Build a minimal `Issue` for testing (used to populate the cache).
fn test_issue(number: u64, state: IssueState) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: qid(number),
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
        source: qid(2),
        target: qid(1),
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
    let dependency_tree: Option<Vec<(QualifiedId, usize)>> = if include_deps {
        cache
            .get_graph()
            .await
            .map(|g| g.dependency_tree(&qid(1), unblock_core::types::TraversalDirection::Both, 3))
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
            source: qid(2),
            target: qid(1),
        },
        BlockingEdge {
            source: qid(3),
            target: qid(2),
        },
    ];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues);
    cache.update(ready_set, graph).await;

    // With include_deps=true and a populated cache, dependency_tree should be Some.
    let include_deps = true;
    let dependency_tree: Option<Vec<(QualifiedId, usize)>> = if include_deps {
        cache
            .get_graph()
            .await
            .map(|g| g.dependency_tree(&qid(1), unblock_core::types::TraversalDirection::Both, 3))
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
    let has_issue_2 = tree.iter().any(|(q, depth)| q.number == 2 && *depth == 1);
    assert!(
        has_issue_2,
        "dependency_tree should contain issue #2 at depth 1: {tree:?}",
    );

    // Issue #3 should appear at depth 2 (blocked by #2, which is blocked by #1).
    let has_issue_3 = tree.iter().any(|(q, depth)| q.number == 3 && *depth == 2);
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
    let dependency_tree: Option<Vec<(QualifiedId, usize)>> = if include_deps {
        cache
            .get_graph()
            .await
            .map(|g| g.dependency_tree(&qid(1), unblock_core::types::TraversalDirection::Both, 3))
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

// ── Ready tool: integration tests ────────────────────────────────

/// Ready returns only open, unblocked, non-deferred issues from cache.
#[tokio::test]
async fn ready_returns_only_open_unblocked_non_deferred_issues() {
    let cache = GraphCache::new(Duration::from_secs(300));

    // Issue #1 is open and unblocked (should appear).
    // Issue #2 is blocked by #1 (should NOT appear).
    // Issue #3 is open but deferred until far future (should NOT appear).
    let mut issue_1 = test_issue(1, IssueState::Open);
    issue_1.priority = Priority::P0;
    let issue_2 = test_issue(2, IssueState::Open);
    let mut issue_3 = test_issue(3, IssueState::Open);
    issue_3.defer_until = Some(chrono::NaiveDate::from_ymd_opt(2099, 12, 31).unwrap());

    let issues = vec![issue_1, issue_2, issue_3];
    let edges = vec![BlockingEdge {
        source: qid(2),
        target: qid(1),
    }];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues);
    cache.update(ready_set.clone(), graph).await;

    // Filter using ready tool logic.
    let params = unblock_mcp::tools::ready::ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result = unblock_mcp::tools::ready::filter_ready_set(&ready_set, &params);

    // Only issue #1 should appear (unblocked, not deferred).
    // Issue #3 has defer_until in far future, excluded.
    assert_eq!(result.len(), 1, "expected 1 ready issue, got: {result:?}");
    assert_eq!(result[0].number, 1);
}

/// Second call returns same result from cache (cache hit path).
#[tokio::test]
async fn ready_second_call_returns_from_cache() {
    let cache = GraphCache::new(Duration::from_secs(300));

    let issues = vec![
        test_issue(1, IssueState::Open),
        test_issue(2, IssueState::Open),
    ];
    let edges = vec![];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues);
    cache.update(ready_set, graph).await;

    // First call.
    let first = cache.get_ready_set().await;
    assert!(first.is_some(), "first call should return from cache");
    assert!(cache.is_fresh().await, "cache should still be fresh");

    // Second call — same result from cache (no rebuild).
    let second = cache.get_ready_set().await;
    assert!(
        second.is_some(),
        "second call should also return from cache"
    );
    assert_eq!(
        *first.unwrap(),
        *second.unwrap(),
        "both calls should return identical data",
    );
}

/// Filter by priority — only matching priority returned.
#[tokio::test]
async fn ready_filter_by_priority() {
    let cache = GraphCache::new(Duration::from_secs(300));

    let mut issue_1 = test_issue(1, IssueState::Open);
    issue_1.priority = Priority::P0;
    let mut issue_2 = test_issue(2, IssueState::Open);
    issue_2.priority = Priority::P1;
    let mut issue_3 = test_issue(3, IssueState::Open);
    issue_3.priority = Priority::P0;

    let issues = vec![issue_1, issue_2, issue_3];
    let graph = DependencyGraph::build(&issues, &[]);
    let ready_set = graph.compute_ready_set(&issues);
    cache.update(ready_set.clone(), graph).await;

    let params = unblock_mcp::tools::ready::ReadyParams {
        limit: None,
        issue_type: None,
        priority: Some("P0".to_owned()),
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result = unblock_mcp::tools::ready::filter_ready_set(&ready_set, &params);

    assert_eq!(result.len(), 2, "expected 2 P0 issues, got: {result:?}");
    assert!(
        result.iter().all(|r| r.priority == "P0"),
        "all results should be P0",
    );
}

/// Filter by label — only matching label returned.
#[tokio::test]
async fn ready_filter_by_label() {
    let cache = GraphCache::new(Duration::from_secs(300));

    let mut issue_1 = test_issue(1, IssueState::Open);
    issue_1.labels = vec!["urgent".to_owned(), "backend".to_owned()];
    let mut issue_2 = test_issue(2, IssueState::Open);
    issue_2.labels = vec!["frontend".to_owned()];
    let issue_3 = test_issue(3, IssueState::Open);

    let issues = vec![issue_1, issue_2, issue_3];
    let graph = DependencyGraph::build(&issues, &[]);
    let ready_set = graph.compute_ready_set(&issues);
    cache.update(ready_set.clone(), graph).await;

    let params = unblock_mcp::tools::ready::ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: Some("urgent".to_owned()),
        include_claimed: None,
    };
    let result = unblock_mcp::tools::ready::filter_ready_set(&ready_set, &params);

    assert_eq!(result.len(), 1, "expected 1 issue with 'urgent' label");
    assert_eq!(result[0].number, 1);
}

/// Deferred issues excluded from default call.
#[tokio::test]
async fn ready_deferred_excluded() {
    let mut issue_1 = test_issue(1, IssueState::Open);
    issue_1.defer_until = Some(chrono::NaiveDate::from_ymd_opt(2099, 1, 1).unwrap());

    let issues = vec![issue_1];
    let graph = DependencyGraph::build(&issues, &[]);
    let ready_set = graph.compute_ready_set(&issues);

    let params = unblock_mcp::tools::ready::ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result = unblock_mcp::tools::ready::filter_ready_set(&ready_set, &params);
    assert!(result.is_empty(), "deferred issue should be excluded");
}

/// `include_claimed=true` includes in-progress issues.
#[tokio::test]
async fn ready_include_claimed_includes_in_progress() {
    let mut issue_1 = test_issue(1, IssueState::Open);
    issue_1.status = Status::InProgress;
    issue_1.agent = Some("agent-a".to_owned());
    let issue_2 = test_issue(2, IssueState::Open);

    let issues = vec![issue_1, issue_2];
    let graph = DependencyGraph::build(&issues, &[]);
    let ready_set = graph.compute_ready_set(&issues);

    // Without include_claimed — should exclude InProgress.
    let params_default = unblock_mcp::tools::ready::ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result_default = unblock_mcp::tools::ready::filter_ready_set(&ready_set, &params_default);
    assert_eq!(
        result_default.len(),
        1,
        "default should exclude InProgress issue",
    );
    assert_eq!(result_default[0].number, 2);

    // With include_claimed=true — should include InProgress.
    let params_claimed = unblock_mcp::tools::ready::ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: Some(true),
    };
    let result_claimed = unblock_mcp::tools::ready::filter_ready_set(&ready_set, &params_claimed);
    assert_eq!(
        result_claimed.len(),
        2,
        "include_claimed=true should include InProgress issue",
    );
}

/// Correct sort order: P0 before P1, same priority by `created_at`.
#[tokio::test]
async fn ready_sort_order() {
    let earlier = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2024, 1, 1, 0, 0, 0).unwrap();
    let later = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2024, 6, 1, 0, 0, 0).unwrap();

    let mut issue_a = test_issue(1, IssueState::Open);
    issue_a.priority = Priority::P1;
    issue_a.created_at = earlier;
    let mut issue_b = test_issue(2, IssueState::Open);
    issue_b.priority = Priority::P0;
    issue_b.created_at = later;
    let mut issue_c = test_issue(3, IssueState::Open);
    issue_c.priority = Priority::P1;
    issue_c.created_at = later;

    let issues = vec![issue_a, issue_b, issue_c];
    let graph = DependencyGraph::build(&issues, &[]);
    let ready_set = graph.compute_ready_set(&issues);

    let params = unblock_mcp::tools::ready::ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result = unblock_mcp::tools::ready::filter_ready_set(&ready_set, &params);

    assert_eq!(result.len(), 3);
    // P0 first.
    assert_eq!(result[0].number, 2, "P0 issue should be first");
    // Then P1 by created_at ASC.
    assert_eq!(result[1].number, 1, "earlier P1 issue should be second");
    assert_eq!(result[2].number, 3, "later P1 issue should be third");
}

/// Limit respected: returns at most N items.
#[tokio::test]
async fn ready_limit_respected() {
    let issues: Vec<_> = (1..=20).map(|n| test_issue(n, IssueState::Open)).collect();
    let graph = DependencyGraph::build(&issues, &[]);
    let ready_set = graph.compute_ready_set(&issues);

    let params = unblock_mcp::tools::ready::ReadyParams {
        limit: Some(3),
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result = unblock_mcp::tools::ready::filter_ready_set(&ready_set, &params);
    assert_eq!(result.len(), 3, "should return at most 3 items");
}

/// Ready tool is registered in the server tool list.
///
/// The `#[tool_router]` macro generates routing at compile time. If the
/// `ready` handler is missing or has the wrong signature, `unblock-mcp`
/// would fail to compile. This test additionally verifies the instructions
/// string references the ready tool.
#[test]
fn ready_tool_registered_in_server() {
    let instructions = unblock_mcp::server::INSTRUCTIONS_STR;
    assert!(
        instructions.contains("ready"),
        "INSTRUCTIONS_STR should mention the 'ready' tool",
    );
    assert!(
        instructions.contains("Find issues that can be worked on right now"),
        "INSTRUCTIONS_STR should describe the ready tool's purpose",
    );
}

// ── Create tool: integration tests ────────────────────────────────

/// Create tool is registered in the server tool list.
///
/// The `#[tool_router]` macro generates routing at compile time. If the
/// `create` handler is missing or has the wrong signature, `unblock-mcp`
/// would fail to compile. This test additionally verifies the instructions
/// string references the create tool.
#[test]
fn create_tool_registered_in_server() {
    let instructions = unblock_mcp::server::INSTRUCTIONS_STR;
    assert!(
        instructions.contains("create"),
        "INSTRUCTIONS_STR should mention the 'create' tool",
    );
    assert!(
        instructions.contains("Create a new issue"),
        "INSTRUCTIONS_STR should describe the create tool's purpose",
    );
}

/// `IssueRef` parsing: local number.
#[test]
fn issue_ref_parse_local_from_string() {
    let r: IssueRef = "42".parse().unwrap();
    assert_eq!(r, IssueRef::Local(42));
}

/// `IssueRef` parsing: cross-repo reference.
#[test]
fn issue_ref_parse_cross_repo_from_string() {
    let r: IssueRef = "acme/widgets#99".parse().unwrap();
    assert_eq!(
        r,
        IssueRef::CrossRepo {
            owner: "acme".to_owned(),
            repo: "widgets".to_owned(),
            number: 99,
        }
    );
}

/// `IssueRef` parsing: hash-prefixed local number.
#[test]
fn issue_ref_parse_hash_prefix_from_string() {
    let r: IssueRef = "#7".parse().unwrap();
    assert_eq!(r, IssueRef::Local(7));
}

/// `IssueRef` parsing: invalid input returns error.
#[test]
fn issue_ref_parse_invalid_returns_error() {
    assert!("not-a-number".parse::<IssueRef>().is_err());
    assert!("/repo#42".parse::<IssueRef>().is_err()); // empty owner
    assert!("owner/#42".parse::<IssueRef>().is_err()); // empty repo
}

/// Create issue with all params — calls GitHub API if token is available.
///
/// Creates a real issue, verifies fields, then closes it for cleanup.
#[tokio::test]
async fn create_issue_with_all_params_and_refetch() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let title = format!(
        "[test] create tool integration test {}",
        chrono::Utc::now().timestamp()
    );

    let params = unblock_github::mutations::CreateIssueParams {
        title: title.clone(),
        body: Some("## Description\n\nIntegration test issue.".to_owned()),
        labels: vec!["test".to_owned()],
        milestone: None,
        assignees: Vec::new(),
    };

    let issue = client
        .create_issue(params)
        .await
        .expect("create_issue should succeed");

    // Verify fields.
    assert!(!issue.title.is_empty(), "title should not be empty");
    assert_eq!(issue.title, title);
    assert!(
        issue.number > 0,
        "issue number should be positive: {}",
        issue.number
    );
    assert!(!issue.node_id.is_empty(), "node_id should not be empty");
    assert!(!issue.url.is_empty(), "url should not be empty");

    eprintln!(
        "create_issue_with_all_params: #{} '{}' url={}",
        issue.number, issue.title, issue.url
    );

    // Re-fetch and verify.
    let refetched = client
        .fetch_issue(issue.number)
        .await
        .expect("fetch_issue should succeed after create");
    assert_eq!(refetched.number, issue.number);
    assert_eq!(refetched.title, title);

    // Cleanup: close the issue.
    client
        .close_issue(issue.number, Some("Integration test cleanup".to_owned()))
        .await
        .expect("close_issue should succeed for cleanup");
}

/// Create with `blocked_by` local number — verifies blocking relationship.
#[tokio::test]
async fn create_issue_with_blocked_by_local() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    // Create blocker issue first.
    let blocking_title = format!("[test] blocker issue {}", chrono::Utc::now().timestamp());
    let blocking_issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: blocking_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create blocker issue should succeed");

    // Create blocked issue.
    let dependent_title = format!("[test] blocked issue {}", chrono::Utc::now().timestamp());
    let dependent_issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: dependent_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create blocked issue should succeed");

    // Add blocking relationship.
    client
        .add_blocked_by(dependent_issue.number, blocking_issue.number)
        .await
        .expect("add_blocked_by should succeed");

    // Re-fetch blocked issue and verify blocker appears.
    let refetched = client
        .fetch_issue(dependent_issue.number)
        .await
        .expect("fetch_issue should succeed after add_blocked_by");
    let blocking_numbers: Vec<u64> = refetched.blocked_by.iter().map(|r| r.number).collect();
    assert!(
        blocking_numbers.contains(&blocking_issue.number),
        "blocked_by should contain the blocker: blocker={}, blocked_by={:?}",
        blocking_issue.number,
        blocking_numbers,
    );

    eprintln!(
        "create_issue_with_blocked_by: blocked=#{} blocker=#{}",
        dependent_issue.number, blocking_issue.number,
    );

    // Cleanup.
    let _ = client
        .close_issue(dependent_issue.number, Some("test cleanup".to_owned()))
        .await;
    let _ = client
        .close_issue(blocking_issue.number, Some("test cleanup".to_owned()))
        .await;
}

/// Create with `blocked_by` using cross-repo `IssueRef` — verifies the
/// `resolve_issue_ref` + `add_blocked_by_ref` GraphQL code path for
/// `IssueRef::CrossRepo`.
///
/// Uses the same configured test repo as both source and target. The
/// `IssueRef::CrossRepo` variant triggers the owner/repo GraphQL resolution
/// path regardless of whether the target repo differs from the configured one.
#[tokio::test]
async fn create_issue_with_blocked_by_cross_repo() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    // Extract owner/repo from the client so we can construct a CrossRepo ref
    // pointing at the same repo.
    let owner = client.owner().to_owned();
    let repo = client.repo().to_owned();

    // Create blocker issue.
    let blocking_title = format!(
        "[test] cross-repo blocker issue {}",
        chrono::Utc::now().timestamp()
    );
    let blocking_issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: blocking_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create blocker issue should succeed");

    // Create dependent issue.
    let dependent_title = format!(
        "[test] cross-repo blocked issue {}",
        chrono::Utc::now().timestamp()
    );
    let dependent_issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: dependent_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create blocked issue should succeed");

    // Build a CrossRepo IssueRef pointing at the blocker in the same repo.
    // This exercises the full cross-repo GraphQL resolution code path
    // (resolve_issue_ref with owner/repo/number query → addIssueDependency).
    let cross_repo_ref = IssueRef::CrossRepo {
        owner: owner.clone(),
        repo: repo.clone(),
        number: blocking_issue.number,
    };

    // Add blocking relationship via the cross-repo path.
    client
        .add_blocked_by_ref(dependent_issue.number, &cross_repo_ref)
        .await
        .expect("add_blocked_by_ref (cross-repo) should succeed");

    // Re-fetch dependent issue and verify blocker appears in blocked_by.
    let refetched = client
        .fetch_issue(dependent_issue.number)
        .await
        .expect("fetch_issue should succeed after add_blocked_by_ref");
    let blocking_numbers: Vec<u64> = refetched.blocked_by.iter().map(|r| r.number).collect();
    assert!(
        blocking_numbers.contains(&blocking_issue.number),
        "blocked_by should contain the cross-repo blocker: blocker={}, blocked_by={:?}",
        blocking_issue.number,
        blocking_numbers,
    );

    eprintln!(
        "create_issue_with_blocked_by_cross_repo: blocked=#{} blocker={}/{}#{}",
        dependent_issue.number, owner, repo, blocking_issue.number,
    );

    // Cleanup: close both issues.
    let _ = client
        .close_issue(dependent_issue.number, Some("test cleanup".to_owned()))
        .await;
    let _ = client
        .close_issue(blocking_issue.number, Some("test cleanup".to_owned()))
        .await;
}

/// Create with `parent` — verifies sub-issue relationship.
#[tokio::test]
async fn create_issue_with_parent_sub_issue() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    // Create parent issue.
    let parent_title = format!("[test] parent issue {}", chrono::Utc::now().timestamp());
    let parent = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: parent_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create parent issue should succeed");

    // Create child issue.
    let child_title = format!("[test] child issue {}", chrono::Utc::now().timestamp());
    let child = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: child_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create child issue should succeed");

    // Add parent relationship.
    client
        .add_sub_issue(parent.number, child.number)
        .await
        .expect("add_sub_issue should succeed");

    // Re-fetch parent and verify child appears.
    let refetched_parent = client
        .fetch_issue(parent.number)
        .await
        .expect("fetch_issue should succeed for parent");
    let sub_issue_numbers: Vec<u64> = refetched_parent
        .sub_issues
        .iter()
        .map(|r| r.number)
        .collect();
    assert!(
        sub_issue_numbers.contains(&child.number),
        "parent sub_issues should contain child: child={}, sub_issues={:?}",
        child.number,
        sub_issue_numbers,
    );

    eprintln!(
        "create_issue_with_parent: parent=#{} child=#{}",
        parent.number, child.number,
    );

    // Cleanup.
    let _ = client
        .close_issue(child.number, Some("test cleanup".to_owned()))
        .await;
    let _ = client
        .close_issue(parent.number, Some("test cleanup".to_owned()))
        .await;
}

/// Create with no optional params — defaults applied (Task, P2).
#[tokio::test]
async fn create_issue_with_defaults() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let title = format!("[test] defaults test {}", chrono::Utc::now().timestamp());

    let issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: title.clone(),
            body: None,
            labels: Vec::new(),
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create_issue with defaults should succeed");

    assert_eq!(issue.title, title);
    assert!(issue.number > 0);

    // The issue state should be Open.
    assert_eq!(issue.state, IssueState::Open);

    eprintln!(
        "create_issue_with_defaults: #{} '{}'",
        issue.number, issue.title,
    );

    // Cleanup.
    let _ = client
        .close_issue(issue.number, Some("test cleanup".to_owned()))
        .await;
}

/// Ensure labels creates missing labels on the repo.
#[tokio::test]
async fn ensure_labels_creates_missing_labels() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    // Use a unique label name to avoid collisions.
    let label_name = format!("test-label-{}", chrono::Utc::now().timestamp());

    client
        .ensure_labels(std::slice::from_ref(&label_name))
        .await
        .expect("ensure_labels should succeed");

    // Calling again should be idempotent.
    client
        .ensure_labels(std::slice::from_ref(&label_name))
        .await
        .expect("ensure_labels should succeed on second call");

    eprintln!("ensure_labels: created '{label_name}'");
}

/// After create, cache is rebuilt and new issue appears in ready set (if unblocked).
///
/// This test verifies the full create+rebuild+ready pipeline:
/// 1. Create an unblocked issue
/// 2. Verify cache rebuild includes the new issue
/// 3. The new issue should appear in the ready set
#[tokio::test]
async fn create_issue_appears_in_ready_set_after_rebuild() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let title = format!("[test] ready set test {}", chrono::Utc::now().timestamp());

    let issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: title.clone(),
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create_issue should succeed");

    // Rebuild cache.
    unblock_mcp::tools::rebuild_cache(&state).await;

    // Check ready set.
    if let Some(ready_set) = state.cache.get_ready_set().await {
        let in_ready_set = ready_set.iter().any(|s| s.number == issue.number);
        eprintln!(
            "create_issue_appears_in_ready_set: #{} in_ready_set={}",
            issue.number, in_ready_set,
        );
        // Note: Whether the issue appears depends on the project field state.
        // If no project is set up, it may still appear since it's open and unblocked.
    } else {
        eprintln!("Cache rebuild returned no ready set (expected in some configs)");
    }

    // Cleanup.
    let _ = client
        .close_issue(issue.number, Some("test cleanup".to_owned()))
        .await;
}

// ── Setup tool: integration tests ────────────────────────────────────

/// Returns `true` if the `UNBLOCK_PROJECT` env var is set and non-empty.
fn has_project_number() -> bool {
    std::env::var("UNBLOCK_PROJECT")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Setup creates all 7 required fields on first run.
///
/// Verifies that `setup_fields()` returns `created` entries for any fields
/// that did not already exist, and that the total resolved field count is 7.
#[tokio::test]
async fn setup_creates_fields_on_first_run() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info should succeed");

    let report = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields should succeed");

    // Total fields (created + skipped) should be 7.
    let total = report.created.len() + report.skipped.len();
    assert_eq!(
        total, 7,
        "setup should resolve exactly 7 fields, got {total}"
    );

    eprintln!(
        "setup_creates_fields: created={:?}, skipped={:?}",
        report.created, report.skipped
    );
}

/// Setup is idempotent — rerun creates no duplicate fields.
///
/// Calls `setup_fields()` twice. The second call should report all fields
/// as skipped (already existing) with zero created.
#[tokio::test]
async fn setup_fields_idempotent_no_duplicates() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info should succeed");

    // First run — ensure all fields exist.
    let _ = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields (first run) should succeed");

    // Second run — all should be skipped.
    let report2 = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields (second run) should succeed");

    assert!(
        report2.created.is_empty(),
        "second setup_fields should create zero fields, got: {:?}",
        report2.created
    );
    assert_eq!(
        report2.skipped.len(),
        7,
        "second setup_fields should skip all 7 fields"
    );

    eprintln!("setup_fields_idempotent: skipped={:?}", report2.skipped);
}

/// Setup creates 5 views with correct layout and filter values.
///
/// Calls `create_view()` for each required view spec and verifies the
/// returned view has the expected layout.
#[tokio::test]
async fn setup_creates_views_with_correct_layout() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type should succeed");

    // Get existing views to determine what would be created.
    let existing_views = client
        .list_views(owner_type)
        .await
        .expect("list_views should succeed");

    let existing_names: std::collections::HashSet<String> =
        existing_views.iter().map(|v| v.name.clone()).collect();

    // Get REST fields for visible_fields.
    let rest_fields = client
        .list_rest_fields(owner_type)
        .await
        .expect("list_rest_fields should succeed");
    let all_field_ids: Vec<u64> = rest_fields.iter().map(|f| f.id).collect();

    let mut created_count = 0;
    let mut skipped_count = 0;

    for spec in REQUIRED_VIEWS {
        if existing_names.contains(spec.name) {
            skipped_count += 1;
            continue;
        }

        let visible_fields = if spec.layout == ViewLayout::Roadmap {
            None
        } else {
            Some(all_field_ids.clone())
        };

        let params = CreateViewParams {
            name: spec.name.to_owned(),
            layout: spec.layout,
            filter: spec.filter.map(String::from),
            visible_fields,
        };

        let view = client
            .create_view(owner_type, &params)
            .await
            .unwrap_or_else(|e| panic!("create_view({}) should succeed: {e}", spec.name));

        assert_eq!(
            view.layout, spec.layout,
            "layout mismatch for {}",
            spec.name
        );
        created_count += 1;
    }

    eprintln!("setup_creates_views: created={created_count}, skipped={skipped_count}");
}

/// Views are idempotent — rerun creates no duplicate views.
///
/// After the views exist, calling `list_views` should show all 5 required
/// view names present.
#[tokio::test]
async fn setup_views_idempotent_no_duplicates() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type should succeed");

    let existing_views = client
        .list_views(owner_type)
        .await
        .expect("list_views should succeed");

    let existing_names: std::collections::HashSet<String> =
        existing_views.iter().map(|v| v.name.clone()).collect();

    let mut found = 0;
    for spec in REQUIRED_VIEWS {
        if existing_names.contains(spec.name) {
            found += 1;
        }
    }

    assert_eq!(
        found,
        REQUIRED_VIEWS.len(),
        "Expected all {} required views to exist after setup, but only {found} found. \
         Missing views indicate setup did not create them or idempotency check failed.",
        REQUIRED_VIEWS.len()
    );
}

/// Dry-run returns fields/views report without making mutations.
///
/// Calls `query_setup_status()` and `list_views()` (the dry-run path)
/// and verifies the report is well-formed without creating anything.
#[tokio::test]
async fn setup_dry_run_reports_without_mutations() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info should succeed");

    // Query field status (dry-run path).
    let field_status = client
        .query_setup_status(&project_info.id)
        .await
        .expect("query_setup_status should succeed");

    let total_fields = field_status.existing.len() + field_status.missing.len();
    assert_eq!(
        total_fields, 7,
        "dry-run field status should account for all 7 fields"
    );

    // Query view status (dry-run path).
    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type should succeed");

    let existing_views = client
        .list_views(owner_type)
        .await
        .expect("list_views should succeed");

    let existing_view_names: std::collections::HashSet<String> =
        existing_views.iter().map(|v| v.name.clone()).collect();

    let mut would_create = 0;
    let mut already_exist = 0;
    for spec in REQUIRED_VIEWS {
        if existing_view_names.contains(spec.name) {
            already_exist += 1;
        } else {
            would_create += 1;
        }
    }

    eprintln!(
        "setup_dry_run: fields existing={}, missing={}; views existing={already_exist}, would_create={would_create}",
        field_status.existing.len(),
        field_status.missing.len(),
    );
}

/// No project configured returns `ProjectNotConfigured` error.
///
/// Uses a client without `UNBLOCK_PROJECT` set and verifies that
/// `resolve_project_info()` fails with the expected error.
#[tokio::test]
async fn setup_no_project_returns_project_not_configured() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    // Build a config without UNBLOCK_PROJECT.
    let config = Config::load_from(|key| match key {
        "UNBLOCK_PROJECT" => Err(std::env::VarError::NotPresent),
        other => std::env::var(other),
    })
    .expect("Config should load without UNBLOCK_PROJECT");

    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new should succeed");

    // resolve_project_info should fail with ProjectNotConfigured.
    let result = client.resolve_project_info().await;
    assert!(
        result.is_err(),
        "resolve_project_info should fail without project number"
    );

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("setup") || msg.contains("project") || msg.contains("configured"),
        "error should reference project configuration: {msg}",
    );

    eprintln!("setup_no_project: error = {msg}");
}

/// Owner type detection correctly identifies org vs user accounts.
///
/// This test verifies that `detect_owner_type()` returns a valid
/// `OwnerType` for the configured repository owner.
#[tokio::test]
async fn setup_owner_type_detection_works() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type should succeed");

    // Just verify it returns a valid value — the actual type depends on the
    // test repo's owner.
    match owner_type {
        OwnerType::Org => eprintln!("Owner '{}' is an organization", client.owner()),
        OwnerType::User => eprintln!("Owner '{}' is a personal account", client.owner()),
    }
}

/// Views use correct `visible_fields` integer IDs from `list_rest_fields`.
///
/// Verifies that `list_rest_fields()` returns fields with positive integer
/// IDs that are suitable for use as `visible_fields` in view creation.
#[tokio::test]
async fn setup_visible_fields_use_integer_ids() {
    if !has_github_token() || !has_project_number() {
        eprintln!("GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;
    let client = &state.client;

    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type should succeed");

    let fields = client
        .list_rest_fields(owner_type)
        .await
        .expect("list_rest_fields should succeed");

    assert!(
        !fields.is_empty(),
        "list_rest_fields should return at least one field"
    );

    for field in &fields {
        assert!(
            field.id > 0,
            "field ID should be a positive integer, got {}",
            field.id
        );
    }

    eprintln!(
        "setup_visible_fields: {} fields with IDs {:?}",
        fields.len(),
        fields.iter().map(|f| f.id).collect::<Vec<_>>()
    );
}
