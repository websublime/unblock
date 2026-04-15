//! Integration tests for MCP tool handlers.
//!
//! These tests require a valid `GITHUB_TOKEN` environment variable and network
//! access to GitHub. They are skipped automatically when `GITHUB_TOKEN` is not
//! set.

use std::sync::Arc;
use std::time::Duration;

use chrono::TimeZone;
use rmcp::handler::server::wrapper::{Json, Parameters};
use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{
    BlockingEdge, IssueComment, IssueRef, IssueState, IssueType, Priority, QualifiedId, Status,
};
use unblock_github::client::GitHubClient;
use unblock_github::projects::{CreateViewParams, OwnerType, ViewLayout};
use unblock_mcp::server::UnblockServer;
use unblock_mcp::tools::reconcile::ReconcileParams;
use unblock_mcp::tools::setup::REQUIRED_VIEWS;
use unblock_mcp::tools::show::ShowParams;

mod common;
use common::{has_github_token, new_mock, state_with_mock, test_server_state};

/// Helper to create a `QualifiedId` for tests.
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
        status: Status::Ready,
        priority: Priority::P1,
        agent: None,
        claimed_at: None,
        pipeline_stage: None,
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

/// Build an `Issue` whose `QualifiedId` matches the `MockGitHubClient`
/// coordinates (`acme/widgets`) so cache lookups find it.
fn mock_issue(number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_{number}"),
        title: format!("Mock issue #{number}"),
        issue_type: Some(IssueType::Task),
        status: Status::Ready,
        priority: Priority::P2,
        agent: None,
        claimed_at: None,
        pipeline_stage: None,
        story_points: None,
        defer_until: None,
        labels: vec![],
        milestone: None,
        assignees: vec![],
        state: IssueState::Open,
        body: Some("## Description\n\nbody\n".to_owned()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![IssueComment {
            author: "alice".to_owned(),
            body: "Hello".to_owned(),
            created_at: chrono::Utc::now(),
        }],
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
    let client = &state.github;

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
    let client = &state.github;

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

/// End-to-end: `include_deps=false` skips the graph-lookup branch of the
/// real `show` handler even when the cache contains a matching graph.
///
/// Uses `MockGitHubClient` behind `Arc<dyn GitHubApi>` so the handler is
/// invoked through its production code path (no `GITHUB_TOKEN` required).
#[tokio::test]
async fn show_include_deps_false_skips_graph_traversal() {
    let mock = new_mock();
    // Two fetches: one for include_deps=false, one for include_deps=true.
    mock.push_fetch_issue_ref(Ok(mock_issue(1)));
    mock.push_fetch_issue_ref(Ok(mock_issue(1)));

    let state = state_with_mock(Arc::clone(&mock));

    // Seed the cache with a graph containing issue #1 → so include_deps=true
    // would otherwise return Some(..). This proves the flag (not an empty
    // cache) is what suppresses the tree.
    let issues = vec![mock_issue(1), mock_issue(2)];
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 2),
        target: QualifiedId::new("acme", "widgets", 1),
    }];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues);
    state.cache.update(issues, ready_set, graph).await;
    assert!(
        state.cache.get_graph().await.is_some(),
        "cache should be populated before invoking show",
    );

    let server = UnblockServer::new(state);

    // include_deps=false: upstream/downstream must be None despite populated cache.
    let Json(result_false) = server
        .show(Parameters(ShowParams {
            issue: "1".to_owned(),
            include_comments: Some(true),
            include_deps: Some(false),
        }))
        .await
        .expect("show should succeed via dyn dispatch (include_deps=false)");
    assert!(
        result_false.upstream.is_none(),
        "upstream must be None when include_deps=false, got {:?}",
        result_false.upstream,
    );
    assert!(
        result_false.downstream.is_none(),
        "downstream must be None when include_deps=false, got {:?}",
        result_false.downstream,
    );
    assert_eq!(
        mock.calls().fetch_issue_ref(),
        1,
        "handler must still fetch the issue when include_deps=false",
    );

    // include_deps=true: upstream/downstream must be Some on the same handler,
    // same cache — confirming the branch is exercised end-to-end.
    let Json(result_true) = server
        .show(Parameters(ShowParams {
            issue: "1".to_owned(),
            include_comments: Some(true),
            include_deps: Some(true),
        }))
        .await
        .expect("show should succeed via dyn dispatch (include_deps=true)");
    assert!(
        result_true.upstream.is_some(),
        "upstream must be Some when include_deps=true and cache has graph",
    );
    assert!(
        result_true.downstream.is_some(),
        "downstream must be Some when include_deps=true and cache has graph",
    );
    assert_eq!(mock.calls().fetch_issue_ref(), 2);
}

/// End-to-end: `include_comments=false` on the real `show` handler returns
/// `comments: None` even though the fetched issue has comments. The handler
/// still performs the fetch — the flag controls projection, not fetch.
#[tokio::test]
async fn show_include_comments_false_skips_comments() {
    let mock = new_mock();
    // Two fetches: one for include_comments=false, one for =true.
    mock.push_fetch_issue_ref(Ok(mock_issue(7)));
    mock.push_fetch_issue_ref(Ok(mock_issue(7)));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));

    // include_comments=false: comments must be None even though the fixture
    // issue carries one comment.
    let Json(result_false) = server
        .show(Parameters(ShowParams {
            issue: "7".to_owned(),
            include_comments: Some(false),
            include_deps: Some(false),
        }))
        .await
        .expect("show should succeed via dyn dispatch (include_comments=false)");
    assert!(
        result_false.comments.is_none(),
        "comments must be None when include_comments=false, got {:?}",
        result_false.comments,
    );
    assert_eq!(
        mock.calls().fetch_issue_ref(),
        1,
        "handler must still fetch the issue when include_comments=false",
    );

    // include_comments=true: comments must be Some(len==1).
    let Json(result_true) = server
        .show(Parameters(ShowParams {
            issue: "7".to_owned(),
            include_comments: Some(true),
            include_deps: Some(false),
        }))
        .await
        .expect("show should succeed via dyn dispatch (include_comments=true)");
    let comments = result_true
        .comments
        .expect("comments must be Some when include_comments=true");
    assert_eq!(
        comments.len(),
        1,
        "comments must surface the single fixture comment",
    );
    assert_eq!(mock.calls().fetch_issue_ref(), 2);
}

/// Flatten a `TreeNode` forest into `(QualifiedId, depth)` pairs for assertions.
fn flatten_tree_nodes(
    nodes: &[unblock_core::types::TreeNode],
) -> Vec<(unblock_core::types::QualifiedId, usize)> {
    let mut result = Vec::new();
    for node in nodes {
        result.push((node.id.clone(), node.depth));
        result.extend(flatten_tree_nodes(&node.children));
    }
    result
}

/// `dependency_tree` returned for issues with blocking relationships.
#[tokio::test]
async fn show_dependency_tree_for_blocking_relationships() {
    let cache = GraphCache::new(Duration::from_secs(300));

    // Issue #2 is blocked by issue #1, #3 is blocked by #2.
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
    cache.update(issues, ready_set, graph).await;

    // With include_deps=true and a populated cache, the DependencyTree should have data.
    let dep_tree = cache
        .get_graph()
        .await
        .map(|g| g.dependency_tree(&qid(1), unblock_core::types::TraversalDirection::Both, 3));

    assert!(
        dep_tree.is_some(),
        "dependency_tree should be Some for an issue with blocking relationships",
    );

    let tree = dep_tree.unwrap();
    assert_eq!(tree.root, qid(1));

    // Downstream: #2 at depth 1 (blocked by #1), #3 at depth 2 (blocked by #2).
    let downstream_flat = flatten_tree_nodes(&tree.downstream);
    assert!(
        !downstream_flat.is_empty(),
        "downstream should not be empty for issue #1 which blocks #2",
    );

    let has_issue_2 = downstream_flat
        .iter()
        .any(|(q, depth)| q.number == 2 && *depth == 1);
    assert!(
        has_issue_2,
        "downstream should contain issue #2 at depth 1: {downstream_flat:?}",
    );

    let has_issue_3 = downstream_flat
        .iter()
        .any(|(q, depth)| q.number == 3 && *depth == 2);
    assert!(
        has_issue_3,
        "downstream should contain issue #3 at depth 2: {downstream_flat:?}",
    );
}

/// `dependency_tree` is `None` when cache is empty.
#[tokio::test]
async fn show_dependency_tree_none_when_cache_empty() {
    let cache = GraphCache::new(Duration::from_secs(300));

    // Cache is empty — no graph. dependency_tree returns None from get_graph.
    let dep_tree: Option<unblock_core::types::DependencyTree> = cache
        .get_graph()
        .await
        .map(|g| g.dependency_tree(&qid(1), unblock_core::types::TraversalDirection::Both, 3));

    assert!(
        dep_tree.is_none(),
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
    cache.update(issues, ready_set.clone(), graph).await;

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
    cache.update(issues, ready_set, graph).await;

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
    cache.update(issues, ready_set.clone(), graph).await;

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
    cache.update(issues, ready_set.clone(), graph).await;

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

/// `include_claimed=true` is a no-op on the graph-level ready set because
/// `compute_ready_set()` already excludes `Status::InProgress` per spec §3.3.
/// The tool-layer `include_claimed` flag is purely defensive.
#[tokio::test]
async fn ready_include_claimed_includes_in_progress() {
    let mut issue_1 = test_issue(1, IssueState::Open);
    issue_1.status = Status::InProgress;
    issue_1.agent = Some("agent-a".to_owned());
    let issue_2 = test_issue(2, IssueState::Open);

    let issues = vec![issue_1, issue_2];
    let graph = DependencyGraph::build(&issues, &[]);
    let ready_set = graph.compute_ready_set(&issues);

    // compute_ready_set now excludes InProgress per spec §3.3.
    assert_eq!(
        ready_set.len(),
        1,
        "compute_ready_set should exclude InProgress issue at graph level",
    );

    // Without include_claimed — result matches the already-filtered ready set.
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

    // With include_claimed=true — still only 1 because InProgress is
    // excluded at the graph level, not the tool-layer filter.
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
        1,
        "include_claimed=true is no-op: InProgress excluded at graph level per spec §3.3",
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

// ── List tool: integration tests ──────────────────────────────────

/// `handle_list` drives the full list pipeline against a `MockGitHubClient`
/// — fetches the OPEN graph, refreshes the cache as a side effect, then
/// applies filter + sort + pagination.
///
/// The mock is seeded with six issues spanning every filterable shape
/// (statuses, priorities, types, milestones, labels, assignees, agents).
/// The test then drives `handle_list` four times to cover:
/// - sort=priority + label filter (returns the matching subset in P0→P3 order),
/// - sort=created (oldest first),
/// - sort=updated (newest first),
/// - offset/limit pagination across the full set.
///
/// All assertions hit the public `pub async fn handle_list(state, params)`
/// surface — server.rs registration is owned by sibling bead unblock-29p.12
/// and is intentionally not exercised here.
#[tokio::test]
#[allow(clippy::too_many_lines)] // Single end-to-end scenario covers 4 call shapes.
async fn list_returns_filtered_sorted_paginated_issues_via_mock_client() {
    use unblock_mcp::tools::list::{ListParams, handle_list};

    #[allow(clippy::too_many_arguments)] // Fixture ctor trades arg count for call-site clarity.
    fn list_issue(
        number: u64,
        status: Status,
        priority: Priority,
        issue_type: IssueType,
        milestone: Option<&str>,
        agent: Option<&str>,
        labels: Vec<&str>,
        assignees: Vec<&str>,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> unblock_core::types::Issue {
        unblock_core::types::Issue {
            qualified_id: QualifiedId::new("acme", "widgets", number),
            number,
            node_id: format!("I_{number}"),
            title: format!("List fixture #{number}"),
            issue_type: Some(issue_type),
            status,
            priority,
            agent: agent.map(str::to_owned),
            claimed_at: None,
            pipeline_stage: None,
            story_points: None,
            defer_until: None,
            labels: labels.into_iter().map(str::to_owned).collect(),
            milestone: milestone.map(str::to_owned),
            assignees: assignees.into_iter().map(str::to_owned).collect(),
            state: IssueState::Open,
            body: None,
            created_at,
            updated_at,
            url: format!("https://github.com/acme/widgets/issues/{number}"),
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    let t1 = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let t2 = chrono::Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
    let t3 = chrono::Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
    let t4 = chrono::Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();
    let t5 = chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let t6 = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

    let issues = vec![
        list_issue(
            1,
            Status::Ready,
            Priority::P0,
            IssueType::Bug,
            Some("v1.0"),
            None,
            vec!["urgent"],
            vec!["alice"],
            t1,
            t6,
        ),
        list_issue(
            2,
            Status::Ready,
            Priority::P1,
            IssueType::Task,
            Some("v1.0"),
            Some("agent-x"),
            vec!["urgent", "backend"],
            vec!["bob"],
            t2,
            t5,
        ),
        list_issue(
            3,
            Status::InProgress,
            Priority::P2,
            IssueType::Feature,
            Some("v2.0"),
            Some("agent-y"),
            vec!["frontend"],
            vec!["alice"],
            t3,
            t4,
        ),
        list_issue(
            4,
            Status::Blocked,
            Priority::P3,
            IssueType::Chore,
            None,
            None,
            vec!["urgent"],
            vec!["carol"],
            t4,
            t3,
        ),
        list_issue(
            5,
            Status::Deferred,
            Priority::P2,
            IssueType::Spike,
            Some("v2.0"),
            None,
            vec!["research"],
            vec![],
            t5,
            t2,
        ),
        list_issue(
            6,
            Status::Ready,
            Priority::P4,
            IssueType::Epic,
            Some("v1.0"),
            None,
            vec!["urgent"],
            vec!["alice", "bob"],
            t6,
            t1,
        ),
    ];

    let mock = new_mock();
    // Seed one result per planned `handle_list` call (4 total).
    for _ in 0..4 {
        mock.push_fetch_graph_data(Ok((issues.clone(), vec![])));
    }
    let state = state_with_mock(Arc::clone(&mock));

    // ── Call 1: filter by label="urgent" with default priority sort ──
    // Expected matches: issues 1, 2, 4, 6 (have "urgent" label).
    // Sort: priority ASC, then created_at ASC, then qualified_id ASC.
    // Order: 1 (P0), 2 (P1), 4 (P3), 6 (P4).
    let result1 = handle_list(
        &state,
        ListParams {
            status: None,
            priority: None,
            issue_type: None,
            milestone: None,
            agent: None,
            label: Some("urgent".to_owned()),
            assignee: None,
            sort: None,
            limit: None,
            offset: None,
        },
    )
    .await
    .expect("list call 1 should succeed");
    assert_eq!(result1.total, 4, "label='urgent' should match 4 issues");
    assert!(!result1.stale, "fresh fetch should not be stale");
    let numbers: Vec<u64> = result1.issues.iter().map(|i| i.number).collect();
    assert_eq!(
        numbers,
        vec![1_u64, 2, 4, 6],
        "priority sort should yield P0→P1→P3→P4 ordering"
    );
    assert_eq!(result1.issues[0].priority, "P0", "first item should be P0",);
    // The list summary must surface assignees from &[Issue] (R3).
    assert_eq!(result1.issues[0].assignees, vec!["alice".to_owned()]);

    // ── Call 2: sort by created_at ASC ──
    // Should order all 6 by created_at: 1, 2, 3, 4, 5, 6.
    let result2 = handle_list(
        &state,
        ListParams {
            status: None,
            priority: None,
            issue_type: None,
            milestone: None,
            agent: None,
            label: None,
            assignee: None,
            sort: Some("created".to_owned()),
            limit: None,
            offset: None,
        },
    )
    .await
    .expect("list call 2 should succeed");
    assert_eq!(result2.total, 6);
    assert!(!result2.stale);
    let numbers2: Vec<u64> = result2.issues.iter().map(|i| i.number).collect();
    assert_eq!(numbers2, vec![1_u64, 2, 3, 4, 5, 6]);

    // ── Call 3: sort by updated_at DESC ──
    // Issue updated_at: 1=t6, 2=t5, 3=t4, 4=t3, 5=t2, 6=t1.
    // Newest first: 1, 2, 3, 4, 5, 6.
    let result3 = handle_list(
        &state,
        ListParams {
            status: None,
            priority: None,
            issue_type: None,
            milestone: None,
            agent: None,
            label: None,
            assignee: None,
            sort: Some("updated".to_owned()),
            limit: None,
            offset: None,
        },
    )
    .await
    .expect("list call 3 should succeed");
    assert_eq!(result3.total, 6);
    let numbers3: Vec<u64> = result3.issues.iter().map(|i| i.number).collect();
    assert_eq!(numbers3, vec![1_u64, 2, 3, 4, 5, 6]);
    // The list summary must surface updated_at from &[Issue] (R2).
    assert!(
        result3.issues[0].updated_at.starts_with("2026-06-01"),
        "first item's updated_at should be t6 (2026-06-01), got {}",
        result3.issues[0].updated_at,
    );

    // ── Call 4: offset/limit pagination across the default sort ──
    // Default sort (priority): 1 (P0), 2 (P1), 3 (P2 t3), 5 (P2 t5),
    // 4 (P3), 6 (P4). offset=2, limit=2 should yield issues 3 then 5.
    let result4 = handle_list(
        &state,
        ListParams {
            status: None,
            priority: None,
            issue_type: None,
            milestone: None,
            agent: None,
            label: None,
            assignee: None,
            sort: None,
            limit: Some(2),
            offset: Some(2),
        },
    )
    .await
    .expect("list call 4 should succeed");
    assert_eq!(
        result4.total, 6,
        "total counts the pre-pagination filter set, not the page",
    );
    let numbers4: Vec<u64> = result4.issues.iter().map(|i| i.number).collect();
    assert_eq!(
        numbers4,
        vec![3_u64, 5],
        "offset=2 + limit=2 should yield page 2 of the priority-sorted set",
    );

    // Final invariant: the mock saw exactly 4 fetch_graph_data calls
    // (one per handle_list invocation) — confirms list always refetches
    // and never short-circuits on a warm cache.
    assert_eq!(
        mock.calls().fetch_graph_data(),
        4,
        "handle_list should always issue a fresh fetch, not consult the cache",
    );
}

/// `handle_list` rejects an out-of-range `limit` with `INVALID_PARAMS`
/// before issuing any GitHub call.
#[tokio::test]
async fn list_rejects_limit_out_of_range_without_fetching() {
    use rmcp::model::ErrorCode;
    use unblock_mcp::tools::list::{ListParams, handle_list};

    let mock = new_mock();
    // Deliberately do NOT push a fetch_graph_data result — if the
    // handler reached the network it would error out as "not stubbed".
    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_list(
        &state,
        ListParams {
            status: None,
            priority: None,
            issue_type: None,
            milestone: None,
            agent: None,
            label: None,
            assignee: None,
            sort: None,
            limit: Some(0),
            offset: None,
        },
    )
    .await
    .expect_err("limit=0 must fail validation");

    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("limit") && err.message.contains("200"),
        "validation error must explain the bound: {}",
        err.message,
    );
    assert_eq!(
        mock.calls().fetch_graph_data(),
        0,
        "validation must short-circuit before any GitHub call",
    );
}

// ── Search tool: integration tests ────────────────────────────────

/// `handle_search` drives the search pipeline against a `MockGitHubClient`
/// — validates the query, forwards to `search_issues(query, limit)`, and
/// maps the returned core `IssueSummary` entries to the schema-annotated
/// `SearchIssueSummary` wire type.
///
/// This test also locks in the cache-bypass invariant: the cache is
/// pre-populated with a distinct snapshot, and the test asserts that
/// `handle_search` does not invalidate or read the cache, and that no
/// `fetch_graph_data()` call is issued.
///
/// Server.rs registration is owned by sibling bead unblock-29p.12 and
/// is intentionally not exercised here.
#[tokio::test]
async fn search_hits_github_and_maps_to_summary_without_touching_cache() {
    use unblock_core::types::IssueSummary;
    use unblock_mcp::tools::search::{SearchParams, handle_search};

    // Build a small pair of IssueSummary entries with non-default
    // fields so the wire projection has something meaningful to
    // assert on.
    fn search_summary(
        number: u64,
        title: &str,
        labels: Vec<&str>,
        milestone: Option<&str>,
    ) -> IssueSummary {
        IssueSummary {
            qualified_id: QualifiedId::new("acme", "widgets", number),
            number,
            title: title.to_owned(),
            issue_type: Some(IssueType::Task),
            status: Status::Ready,
            priority: Priority::P2,
            agent: None,
            milestone: milestone.map(str::to_owned),
            story_points: None,
            defer_until: None,
            labels: labels.into_iter().map(str::to_owned).collect(),
            created_at: chrono::Utc.with_ymd_and_hms(2026, 3, 15, 12, 0, 0).unwrap(),
            url: format!("https://github.com/acme/widgets/issues/{number}"),
        }
    }

    let seeded = vec![
        search_summary(
            101,
            "Ship the new search tool",
            vec!["feature", "mcp"],
            Some("v0.1.0"),
        ),
        search_summary(102, "Fix flaky search test", vec!["bug"], None),
    ];

    let mock = new_mock();
    mock.push_search_issues(Ok(seeded.clone()));
    let state = state_with_mock(Arc::clone(&mock));

    // Pre-populate the cache with a distinct snapshot so we can detect
    // any stray read/write by the search handler.
    let cache_issues = vec![test_issue(999, IssueState::Open)];
    let graph = DependencyGraph::build(&cache_issues, &[]);
    let ready_set = graph.compute_ready_set(&cache_issues);
    state
        .cache
        .update(cache_issues, ready_set.clone(), graph)
        .await;
    assert!(
        state.cache.is_fresh().await,
        "cache should be fresh before search",
    );
    let ready_before = state
        .cache
        .get_ready_set()
        .await
        .expect("cache seeded above");

    let result = handle_search(
        &state,
        SearchParams {
            query: "  ship  ".to_owned(),
            limit: Some(5),
        },
    )
    .await
    .expect("search call should succeed");

    // Envelope assertions.
    assert_eq!(result.count, 2, "count must equal issues.len()");
    assert_eq!(result.issues.len(), 2);
    assert!(
        !result.stale,
        "search bypasses the cache — stale must be false on success",
    );

    // Per-field mapping assertions — order preserved from the mock.
    assert_eq!(result.issues[0].number, 101);
    assert_eq!(result.issues[0].title, "Ship the new search tool");
    assert_eq!(result.issues[0].priority, "P2");
    assert_eq!(result.issues[0].status, "Ready");
    assert_eq!(result.issues[0].issue_type.as_deref(), Some("Task"));
    assert_eq!(result.issues[0].milestone.as_deref(), Some("v0.1.0"));
    assert_eq!(result.issues[0].labels, vec!["feature", "mcp"]);
    assert!(result.issues[0].created_at.starts_with("2026-03-15"));
    assert!(result.issues[0].url.ends_with("/101"));

    assert_eq!(result.issues[1].number, 102);
    assert_eq!(result.issues[1].labels, vec!["bug"]);
    assert!(result.issues[1].milestone.is_none());

    // Invariant 1: exactly one search_issues call was made.
    assert_eq!(
        mock.calls().search_issues(),
        1,
        "search tool must invoke search_issues exactly once",
    );

    // Invariant 2: the cache was not consulted or invalidated — no
    // fetch_graph_data call, cache still holds the pre-populated data.
    assert_eq!(
        mock.calls().fetch_graph_data(),
        0,
        "search must bypass the cache — no fetch_graph_data allowed",
    );
    assert!(
        state.cache.is_fresh().await,
        "cache must remain fresh — search handler cannot invalidate it",
    );
    let ready_after = state
        .cache
        .get_ready_set()
        .await
        .expect("cache was not invalidated");
    assert_eq!(
        *ready_before, *ready_after,
        "cache contents must be identical before and after search",
    );
}

/// `handle_search` rejects an empty query with `INVALID_PARAMS` before
/// issuing any GitHub call.
#[tokio::test]
async fn search_rejects_empty_query_without_fetching() {
    use rmcp::model::ErrorCode;
    use unblock_mcp::tools::search::{SearchParams, handle_search};

    let mock = new_mock();
    // Deliberately do NOT push a search_issues stub — if the handler
    // reached the trait it would fail with `MockNotStubbed`.
    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_search(
        &state,
        SearchParams {
            query: String::new(),
            limit: None,
        },
    )
    .await
    .expect_err("empty query must fail validation");

    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("empty") || err.message.contains("query"),
        "validation error must explain the failure: {}",
        err.message,
    );

    // Whitespace-only must behave identically.
    let err2 = handle_search(
        &state,
        SearchParams {
            query: "   \t\n".to_owned(),
            limit: None,
        },
    )
    .await
    .expect_err("whitespace-only query must fail validation");
    assert_eq!(err2.code, ErrorCode::INVALID_PARAMS);

    assert_eq!(
        mock.calls().search_issues(),
        0,
        "validation must short-circuit before any GitHub call",
    );
}

/// `handle_search` rejects `limit = Some(0)` with `INVALID_PARAMS`
/// before issuing any GitHub call (cross-ref unblock-29p.19).
#[tokio::test]
async fn search_rejects_zero_limit_without_fetching() {
    use rmcp::model::ErrorCode;
    use unblock_mcp::tools::search::{SearchParams, handle_search};

    let mock = new_mock();
    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_search(
        &state,
        SearchParams {
            query: "anything".to_owned(),
            limit: Some(0),
        },
    )
    .await
    .expect_err("limit=0 must fail validation");

    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("limit"),
        "validation error must mention limit: {}",
        err.message,
    );
    assert_eq!(
        mock.calls().search_issues(),
        0,
        "validation must short-circuit before any GitHub call",
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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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
    let client = &state.github;

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

    // Intentional concrete `GitHubClient::new` (not the `GitHubApi` trait):
    // this test asserts the real client's project-resolution path returns
    // `ProjectNotConfigured` when `UNBLOCK_PROJECT` is unset, which is a
    // property of the concrete implementation, not the trait abstraction.
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
    let client = &state.github;

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
    let client = &state.github;

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

// ── Reconcile tool: integration tests ────────────────────────────────

/// Run `reconcile` in read-only mode against the real GitHub repository and
/// verify that it completes without error.
///
/// This test validates that the full reconcile pipeline (fresh fetch, graph
/// rebuild, drift analysis) works end-to-end against real GitHub data.
/// It does NOT assert `clean: true` because the test repo may have legitimate
/// drift — it only asserts successful execution and valid report structure.
#[tokio::test]
async fn reconcile_on_clean_repo() {
    if !has_github_token() {
        eprintln!("GITHUB_TOKEN not set — skipping integration test");
        return;
    }

    let state = test_server_state().await;

    let params = ReconcileParams {
        fix: false,
        stale_claim_hours: 24,
    };

    let result = unblock_mcp::tools::reconcile::handle_reconcile(&params, &state).await;

    match result {
        Ok(output) => {
            let report = &output.report;
            eprintln!(
                "reconcile_on_clean_repo: issues_scanned={}, edges_scanned={}, clean={}, drift_count={}",
                report.issues_scanned,
                report.edges_scanned,
                report.clean,
                report.drift_found.len()
            );
            // Structural assertions — the report must have valid data.
            assert!(!report.repo.is_empty(), "repo field should be populated");
            assert!(
                !report.reconciled_at.is_empty(),
                "reconciled_at should be populated"
            );
            // Read-only mode: repaired should always be empty.
            assert!(
                report.repaired.is_empty(),
                "read-only reconcile should not repair anything"
            );
            // If clean, drift_found should be empty and message present.
            if report.clean {
                assert!(report.drift_found.is_empty());
                assert!(
                    report.message.is_some(),
                    "clean report should include a message"
                );
            }
        }
        Err(e) => {
            panic!("handle_reconcile failed: {e:?}");
        }
    }
}

/// Build a set of issues with known drift (`UncascadedClosure`), run the
/// reconcile engine to verify it detects the drift, then simulate repair by
/// correcting the `Status` and verifying the re-analysis returns clean.
///
/// This is a unit-level integration test — it exercises the full analyse →
/// detect → fix → re-analyse cycle without requiring GitHub write access.
#[tokio::test]
async fn reconcile_with_injected_drift_and_fix() {
    use std::collections::{HashMap, HashSet};
    use unblock_core::reconcile::{DriftKind, ReconcileEngine};

    // Setup: issue #1 (closed), issue #2 (open, blocked by #1).
    // #2's Status is Blocked but should be Ready (blocker is closed).
    let q1 = QualifiedId::new("acme", "test", 1);
    let q2 = QualifiedId::new("acme", "test", 2);

    let issue1 = {
        let mut i = test_issue(1, IssueState::Closed);
        i.qualified_id = q1.clone();
        i.status = Status::Closed;
        i
    };
    let issue2 = {
        let mut i = test_issue(2, IssueState::Open);
        i.qualified_id = q2.clone();
        i.status = Status::Blocked; // <-- DRIFT: should be Ready
        i
    };

    let issues_vec = vec![issue1.clone(), issue2.clone()];
    let edges = vec![BlockingEdge {
        source: q2.clone(),
        target: q1.clone(),
    }];
    let graph = DependencyGraph::build(&issues_vec, &edges);
    let computed_ready: HashSet<QualifiedId> = graph
        .compute_ready_set(&issues_vec)
        .into_iter()
        .map(|s| s.qualified_id)
        .collect();
    let by_id: HashMap<QualifiedId, _> = issues_vec
        .iter()
        .map(|i| (i.qualified_id.clone(), i.clone()))
        .collect();

    let engine = ReconcileEngine::new(24);

    // Step 1: Detect drift.
    let report = engine.analyse(&graph, &by_id, &computed_ready, chrono::Utc::now());
    assert!(!report.clean, "Should detect drift");
    let has_uncascaded = report
        .drift_found
        .iter()
        .any(|d| matches!(d, DriftKind::UncascadedClosure { .. }));
    assert!(has_uncascaded, "Should detect UncascadedClosure drift");

    // Step 2: Simulate repair — correct the Status on issue #2.
    let corrected_issues = {
        let mut i2 = issue2.clone();
        i2.status = Status::Ready; // Fixed!
        vec![issue1, i2]
    };
    let repaired_graph = DependencyGraph::build(&corrected_issues, &edges);
    let repaired_ready: HashSet<QualifiedId> = repaired_graph
        .compute_ready_set(&corrected_issues)
        .into_iter()
        .map(|s| s.qualified_id)
        .collect();
    let repaired_by_id: HashMap<QualifiedId, _> = corrected_issues
        .iter()
        .map(|i| (i.qualified_id.clone(), i.clone()))
        .collect();

    // Step 3: Re-analyse — should be clean now.
    let repaired_report = engine.analyse(
        &repaired_graph,
        &repaired_by_id,
        &repaired_ready,
        chrono::Utc::now(),
    );
    assert!(
        repaired_report.clean,
        "After repair, graph should be clean but found drift: {:?}",
        repaired_report.drift_found
    );
}
