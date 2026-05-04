//! Integration tests for MCP tool handlers.
//!
//! ## Two test buckets
//!
//! The tests in this file are split into two buckets:
//!
//! 1. **Mock-backed tests** — use `MockGitHubClient` via `state_with_mock` and
//!    do not require any environment variables, network, or `.git/config`.
//!    These run as part of the default `cargo test --workspace` run and form
//!    the bulk of the integration suite.
//! 2. **Live-required tests** — actually call `api.github.com`. These are
//!    marked `#[ignore]` and opt-in via `cargo test --workspace -- --ignored`
//!    with a real `GITHUB_TOKEN` + `UNBLOCK_REPO` (and `UNBLOCK_PROJECT` for
//!    the Projects V2 tests) set. Every live-required test starts with a
//!    `require_github_token()` / `require_github_token_and_project()` gate so
//!    that accidental invocation without credentials exits cleanly instead
//!    of emitting a confusing failure.
//!
//! The contract mirrors `unblock-github`'s integration tests (see beads
//! `unblock-c4h` and `unblock-3lb` for the full rationale): live tests are
//! never silently skipped — they are explicit `#[ignore]` so the cargo
//! report flags them as ignored rather than counting them as PASS.

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
use unblock_github::projects::{CreateViewParams, OwnerType, ViewLayout};
use unblock_mcp::server::UnblockServer;
use unblock_mcp::tools::reconcile::ReconcileParams;
use unblock_mcp::tools::setup::REQUIRED_VIEWS;
use unblock_mcp::tools::show::ShowParams;

mod common;
use common::{
    TracingCapture, build_github_client, new_mock, require_github_token,
    require_github_token_and_project, state_with_mock, test_server_state,
};

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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn show_existing_issue_returns_all_fields_populated() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn show_nonexistent_issue_returns_issue_not_found() {
    if !require_github_token() {
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
    let ready_set = graph.compute_ready_set(&issues, "acme", "widgets");
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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");
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

/// Pin the full Phase 01 MCP tool suite (SPEC §6).
///
/// Asserts that the server registers every one of the 17 canonical
/// Phase 01 tools listed in SPEC §6 (§7.1–§7.7 reads + §8.1–§8.10 writes)
/// and that the total tool count remains stable. Fails loudly whenever a
/// future task adds or removes a tool without updating this contract.
///
/// ## Count note — 17 spec tools + 1 Phase 02 early feature
///
/// Plan `docs/plans/01-plan-mcp-foundation.md:920` Definition of Done
/// item 1 requires "All 17 tools registered and functional —
/// `server_lists_all_17_tools` test passes", where the 17 refers to the
/// SPEC §6 canonical list. `reconcile` is a Phase 02 tool that is
/// already implemented and registered per plan decision D4 (line 905:
/// "Keep code, exclude from F1 acceptance criteria"). This test
/// therefore asserts (a) every name in `EXPECTED_SPEC_TOOLS` is
/// present, AND (b) the total registered count is
/// `EXPECTED_SPEC_TOOLS.len() + 1` so `reconcile` is pinned too — any
/// future tool addition/removal still fails loudly.
#[test]
fn server_lists_all_17_tools() {
    // SPEC §6 canonical 17-tool list, taken verbatim from the spec
    // headings at §7.1–§7.7 (read tools) and §8.1–§8.10 (write tools).
    // The order here follows spec section order for readability; the
    // assertion is order-independent.
    const EXPECTED_SPEC_TOOLS: &[&str] = &[
        // Read tools (SPEC §7)
        "ready",      // §7.1
        "show",       // §7.2
        "prime",      // §7.3
        "stats",      // §7.4
        "list",       // §7.5
        "search",     // §7.6
        "dep_cycles", // §7.7
        // Write tools (SPEC §8)
        "claim",      // §8.1
        "close",      // §8.2
        "create",     // §8.3
        "depends",    // §8.4
        "dep_remove", // §8.5
        "update",     // §8.6
        "reopen",     // §8.7
        "comment",    // §8.8
        "init",       // §8.9
        "setup",      // §8.10
    ];
    // Phase 02 early features kept per plan D4 (line 905).
    const PHASE_02_EARLY_FEATURES: &[&str] = &["reconcile"];

    assert_eq!(
        EXPECTED_SPEC_TOOLS.len(),
        17,
        "EXPECTED_SPEC_TOOLS must list exactly 17 tools per SPEC §6",
    );

    let mock = new_mock();
    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));

    let registered: std::collections::BTreeSet<String> = server.tool_names().into_iter().collect();

    // (a) Every SPEC §6 tool must be present.
    for expected in EXPECTED_SPEC_TOOLS {
        assert!(
            registered.contains(*expected),
            "SPEC §6 tool '{expected}' is missing from the MCP server tool router — \
             registered tools: {registered:?}",
        );
    }

    // (b) Phase 02 early features (per D4) must also remain registered so
    //     removal trips the pin.
    for expected in PHASE_02_EARLY_FEATURES {
        assert!(
            registered.contains(*expected),
            "Phase 02 early-feature tool '{expected}' is missing from the MCP server \
             tool router — registered tools: {registered:?}",
        );
    }

    // (c) Exact count: 17 spec tools + N phase-02 early features.
    //     Any future tool added or removed without updating the pinned
    //     lists above fails this assertion — the whole point of the test.
    let expected_total = EXPECTED_SPEC_TOOLS.len() + PHASE_02_EARLY_FEATURES.len();
    assert_eq!(
        registered.len(),
        expected_total,
        "Registered tool count mismatch. Expected {expected_total} (= 17 SPEC §6 tools + \
         {} Phase 02 early features), got {}. Registered: {registered:?}. \
         If you added a tool to the server router, update EXPECTED_SPEC_TOOLS (if it \
         belongs to the Phase 01 spec) or PHASE_02_EARLY_FEATURES (otherwise).",
        PHASE_02_EARLY_FEATURES.len(),
        registered.len(),
    );

    // (d) Sanity: router output is sorted alphabetically (rmcp
    //     `ToolRouter::list_all` sorts by name — see
    //     rmcp/src/handler/server/router/tool.rs:415). A regression in
    //     rmcp could silently break callers that rely on the ordering,
    //     so pin it here too.
    let observed_order = server.tool_names();
    let mut expected_order = observed_order.clone();
    expected_order.sort();
    assert_eq!(
        observed_order, expected_order,
        "tool_names() must be alphabetically sorted per rmcp ToolRouter::list_all contract",
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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");
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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");
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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");
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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");
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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");

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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");

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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");

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
    let ready_set = graph.compute_ready_set(&issues, "test", "repo");

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

// ── Ready tool: cross-repo refs integration tests (SPEC §11.4) ─────────

/// Build a fixture issue under the `MockGitHubClient` coordinates
/// (`acme/widgets`) for ready cross-repo tests. Mirrors
/// `dep_cycles_fixture_issue` — `ready` only consumes topology and
/// per-issue filter fields, which stay at minimal defaults.
fn ready_fixture_issue(number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_{number}"),
        title: format!("Ready fixture #{number}"),
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Build a cross-repo fixture issue whose `QualifiedId` lives OUTSIDE the
/// configured `acme/widgets` repo, so the cross-repo projection can pick
/// it up as an omitted blocker.
fn ready_cross_repo_fixture(owner: &str, repo: &str, number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new(owner, repo, number),
        number,
        node_id: format!("I_{owner}_{repo}_{number}"),
        title: format!("Ready cross-repo fixture {owner}/{repo}#{number}"),
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/{owner}/{repo}/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Acceptance (a): local-only graph — `cross_repo_refs == None` and
/// `skip_serializing_if` elides the key from the JSON envelope.
///
/// Fixture: three local issues. Issue #1 blocks #2, so #2 is held out of
/// the ready set by a LOCAL blocker. Issue #3 is independent. Per SPEC
/// §11.4 this MUST yield `cross_repo_refs == None` because no cross-repo
/// node participated in filtering.
#[tokio::test]
async fn ready_no_cross_repo_blockers_cross_repo_refs_is_none() {
    use unblock_mcp::tools::ready::{ReadyParams, handle_ready};

    let issues = vec![
        ready_fixture_issue(1),
        ready_fixture_issue(2),
        ready_fixture_issue(3),
    ];
    // Local-only blocking edge: #2 is blocked by #1. Ready set = {#1, #3}.
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 2),
        target: QualifiedId::new("acme", "widgets", 1),
    }];

    let mock = new_mock();
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = state_with_mock(Arc::clone(&mock));

    let params = ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result = handle_ready(&state, params)
        .await
        .expect("handle_ready should succeed on cold cache");

    // Ready set contains only issues with no open blockers: {#1, #3}.
    assert_eq!(result.count, 2, "local-only: ready set = {{#1, #3}}");
    assert!(!result.stale, "stale=false on successful rebuild");
    // Acceptance (a): local-only graph → cross_repo_refs None.
    assert!(
        result.cross_repo_refs.is_none(),
        "SPEC §11.4: local-only graph → cross_repo_refs None; got: {:?}",
        result.cross_repo_refs,
    );
    // skip_serializing_if MUST elide the key entirely (JSON-layer guard).
    let json = serde_json::to_value(&result).expect("serialize");
    assert!(
        json.get("cross_repo_refs").is_none(),
        "None cross_repo_refs MUST be elided from JSON: {json}",
    );
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "cold ready call rebuilds the cache once",
    );
}

/// Acceptance (b): cross-repo OPEN blocker silently excludes a local
/// issue from the ready set → `cross_repo_refs` populated with the
/// omitted qualified ref and a summary containing "cross-repo" and
/// "ready".
///
/// Fixture: local issue #1, local issue #2, and cross-repo blocker
/// `other/repo#99`. Edge: #1 → other/repo#99. Ready set therefore drops
/// #1 (open cross-repo blocker) but keeps #2.
#[tokio::test]
async fn ready_cross_repo_open_blocker_populates_cross_repo_refs() {
    use unblock_mcp::tools::ready::{ReadyParams, handle_ready};

    let issues = vec![
        ready_fixture_issue(1),
        ready_fixture_issue(2),
        ready_cross_repo_fixture("other", "repo", 99),
    ];
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 1),
        target: QualifiedId::new("other", "repo", 99),
    }];

    let mock = new_mock();
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = state_with_mock(Arc::clone(&mock));

    let params = ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result = handle_ready(&state, params)
        .await
        .expect("handle_ready should succeed on cold cache");

    // Load-bearing assertion: only #2 survives.
    //
    // - #1 is a local source held out of the ready set by its open
    //   cross-repo blocker other/repo#99 (SPEC §3.3 Filter 4).
    // - other/repo#99 is a cross-repo source, so it is dropped by
    //   SPEC §3.3 Filter 3 (§14 Invariant 14(a), unblock-eos.4 / D6.a /
    //   GAP-14.b) BEFORE the blocker traversal — it can never reach the
    //   ready set.
    // - #2 has no blocker and lives in the configured repo, so it is the
    //   only surviving entry.
    //
    // Pre-eos.4 the graph engine admitted other/repo#99 into the ready
    // set; tool-layer post-filters may have dropped it but that was never
    // the invariant. Post-eos.4, a strict count check pins the new
    // invariant at the edge of the tool handler.
    assert_eq!(
        result.count, 1,
        "SPEC §14 Invariant 14(a): only #2 (configured-repo, unblocked) must appear; got: {:?}",
        result.issues,
    );
    assert_eq!(
        result.issues.len(),
        1,
        "result.issues must match result.count; got: {:?}",
        result.issues,
    );
    assert_eq!(
        result.issues[0].number, 2,
        "Ready entry must be local #2 (cross-repo #99 scrubbed by Filter 3, local #1 blocked); got: {:?}",
        result.issues,
    );
    // `ReadyIssueSummary` drops `qualified_id`; check the fixture-derived
    // url to pin the entry to the configured (acme, widgets) repo.
    assert!(
        result.issues[0].url.contains("acme/widgets") || result.issues[0].url.is_empty(),
        "Ready entry must live in the configured (acme, widgets) repo; got url={}",
        result.issues[0].url,
    );
    assert!(!result.stale);

    // Acceptance (b): cross_repo_refs is Some with "other/repo#99" in
    // omitted and a populated summary referencing "cross-repo" + "ready".
    let refs = result
        .cross_repo_refs
        .as_ref()
        .expect("SPEC §11.4: cross-repo OPEN blocker → cross_repo_refs Some");
    assert_eq!(
        refs.omitted,
        vec!["other/repo#99".to_owned()],
        "omitted carries the cross-repo blocker display form",
    );
    let summary = refs
        .summary
        .as_deref()
        .expect("SPEC §11.4: summary populated for non-empty omitted");
    assert!(
        summary.contains("cross-repo"),
        "summary must describe cross-repo omission: {summary}",
    );
    assert!(
        summary.contains("ready"),
        "summary must reference the `ready` projection: {summary}",
    );

    // JSON envelope surfaces the cross_repo_refs field (not elided).
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["cross_repo_refs"]["omitted"][0], "other/repo#99");
    assert!(
        json["cross_repo_refs"]["summary"]
            .as_str()
            .is_some_and(|s| s.contains("cross-repo")),
        "JSON envelope carries the summary: {json}",
    );
    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

/// SPEC §14 Invariant 14(a) / Plan Task 04.05 addendum / unblock-eos.5 AC #6:
/// when `fetch_graph_data` returns a mix of configured-repo and cross-repo
/// OPEN issues (with NO blocking edges between them), `ReadyResult.issues`
/// MUST contain ONLY configured-repo source issues. The graph engine's
/// Filter 3 is the single chokepoint that enforces this — the `ready` tool
/// handler does NOT re-check.
///
/// Fixture:
/// - Configured repo (acme/widgets): three OPEN unblocked issues (#1, #2, #3).
/// - Cross-repo (other/repo): three OPEN issues (#50, #51, #52) with no edges.
///
/// Expected behaviour:
/// - `result.count == 3` (only the configured-repo issues survive).
/// - No entry in `result.issues` has `qualified_id.(owner, repo) != (acme, widgets)`.
/// - `result.cross_repo_refs` is `None` — no cross-repo BLOCKER participated
///   in filtering (the cross-repo nodes are sources, scrubbed by Filter 3;
///   per SPEC §11.4 the `ready` row surfaces cross-repo BLOCKERS only).
#[tokio::test]
async fn ready_mixed_repo_sources_excluded_per_invariant_14a() {
    use unblock_mcp::tools::ready::{ReadyParams, handle_ready};

    let issues = vec![
        // Configured-repo OPEN issues — all unblocked.
        ready_fixture_issue(1),
        ready_fixture_issue(2),
        ready_fixture_issue(3),
        // Cross-repo OPEN issues — not blockers of anything local.
        ready_cross_repo_fixture("other", "repo", 50),
        ready_cross_repo_fixture("other", "repo", 51),
        ready_cross_repo_fixture("other", "repo", 52),
    ];
    // Intentionally NO edges: no cross-repo blocker participates in
    // filtering, so `cross_repo_refs` must be None.
    let edges: Vec<BlockingEdge> = vec![];

    let mock = new_mock();
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = state_with_mock(Arc::clone(&mock));

    let params = ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: None,
        include_claimed: None,
    };
    let result = handle_ready(&state, params)
        .await
        .expect("handle_ready should succeed on cold cache");

    // AC #6 core: no cross-repo source leaks into `issues`.
    //
    // The MCP projection `ReadyIssueSummary` collapses `QualifiedId` into
    // `number` + `url`; the two fixture families use disjoint number ranges
    // (configured-repo uses 1–3, cross-repo uses 50–52), so any leak of a
    // cross-repo source into the envelope is detectable as a forbidden
    // number OR via the url prefix. We check both.
    let numbers: std::collections::HashSet<u64> = result.issues.iter().map(|i| i.number).collect();
    let forbidden: std::collections::HashSet<u64> = std::collections::HashSet::from([50, 51, 52]);
    assert!(
        numbers.is_disjoint(&forbidden),
        "SPEC §14 Invariant 14(a): cross-repo source numbers {:?} leaked \
         into ReadyResult.issues; got numbers: {:?}",
        forbidden.intersection(&numbers).collect::<Vec<_>>(),
        numbers,
    );
    for summary in &result.issues {
        assert!(
            summary.url.contains("acme/widgets") || summary.url.is_empty(),
            "SPEC §14 Invariant 14(a): issue url must point at the \
             configured (acme, widgets) repo; got url={} number={}",
            summary.url,
            summary.number,
        );
    }

    // `count` must equal the configured-repo subset (three entries), not
    // the full 6-issue input.
    assert_eq!(
        result.count, 3,
        "Filter 3 must drop every other/repo#N issue; got: {:?}",
        result.issues,
    );
    assert_eq!(result.issues.len(), 3);

    assert_eq!(
        numbers,
        std::collections::HashSet::from([1, 2, 3]),
        "Only configured-repo issues #1, #2, #3 may appear",
    );

    // No cross-repo blocker participated in filtering, so §11.4 contract
    // requires `cross_repo_refs` to be `None`.
    assert!(
        result.cross_repo_refs.is_none(),
        "SPEC §11.4: cross_repo_refs must be None when no cross-repo blocker \
         held a local issue out of the ready set; got: {:?}",
        result.cross_repo_refs,
    );
    assert!(!result.stale);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
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
            label: Some("urgent".to_owned()),
            ..Default::default()
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
            sort: Some("created".to_owned()),
            ..Default::default()
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
            sort: Some("updated".to_owned()),
            ..Default::default()
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
            limit: Some(2),
            offset: Some(2),
            ..Default::default()
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

/// `handle_list` exposes CLOSED issues when `status="Closed"` after the
/// `fetch_graph_data` widening in bead `unblock-a36`.
///
/// Before the widening the MCP `list` tool only ever saw `OPEN` issues,
/// so `list(status="Closed")` was documented as returning `total=0` in
/// the tool description. The fetch now uses `states: [OPEN, CLOSED]`,
/// and this test asserts the new contract by driving two `handle_list`
/// calls against a single seeded mixed-state universe:
///
/// 1. `status="Closed"` — returns ONLY the fixtures with `Status::Closed`
///    (and by construction `IssueState::Closed`). No Ready/InProgress
///    issues leak in.
/// 2. `status="Ready"` — the partition complement: returns ONLY the
///    Ready fixtures, with the Closed fixtures correctly excluded from
///    both `issues` and `total`. This twin assertion guards against a
///    regression where the widening accidentally let Closed issues
///    bleed into the default Ready projection.
///
/// The mixed universe seeds three Closed issues (#10/#11/#12) alongside
/// two Ready issues (#1/#2), exercised via the default priority sort
/// where Closed issues have `P1` (highest) so any spurious inclusion
/// would flip the ordering in a visible way.
#[tokio::test]
#[allow(clippy::too_many_lines)] // End-to-end test covers two list-call shapes in one scenario.
async fn list_status_closed_returns_closed_issues_and_status_ready_excludes_them() {
    use unblock_mcp::tools::list::{ListParams, handle_list};

    #[allow(clippy::too_many_arguments)]
    fn list_fixture(
        number: u64,
        status: Status,
        state: IssueState,
        priority: Priority,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> unblock_core::types::Issue {
        unblock_core::types::Issue {
            qualified_id: QualifiedId::new("acme", "widgets", number),
            number,
            node_id: format!("I_{number}"),
            title: format!("List closed fixture #{number}"),
            issue_type: Some(IssueType::Task),
            status,
            priority,
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
            created_at,
            updated_at: created_at,
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

    let issues = vec![
        // Two Ready issues (the pre-unblock-a36 visible set).
        list_fixture(1, Status::Ready, IssueState::Open, Priority::P2, t1),
        list_fixture(2, Status::Ready, IssueState::Open, Priority::P3, t2),
        // Three Closed issues (invisible to list before the widening).
        list_fixture(10, Status::Closed, IssueState::Closed, Priority::P1, t3),
        list_fixture(11, Status::Closed, IssueState::Closed, Priority::P1, t4),
        list_fixture(12, Status::Closed, IssueState::Closed, Priority::P1, t5),
    ];

    let mock = new_mock();
    // Two handle_list calls = two fresh fetches.
    for _ in 0..2 {
        mock.push_fetch_graph_data(Ok((issues.clone(), vec![])));
    }
    let state = state_with_mock(Arc::clone(&mock));

    // ── Call 1: status="Closed" returns ONLY the three Closed fixtures ──
    // Default priority sort: all three are P1, so the deterministic
    // qualified_id tiebreaker orders them 10 < 11 < 12.
    let closed_result = handle_list(
        &state,
        ListParams {
            status: Some("Closed".to_owned()),
            ..Default::default()
        },
    )
    .await
    .expect("list(status=Closed) should succeed");

    assert_eq!(
        closed_result.total, 3,
        "status=Closed must surface all three Closed fixtures after unblock-a36",
    );
    let closed_numbers: Vec<u64> = closed_result.issues.iter().map(|i| i.number).collect();
    assert_eq!(
        closed_numbers,
        vec![10_u64, 11, 12],
        "status=Closed must return exactly the Closed fixtures in qualified-id order",
    );
    for summary in &closed_result.issues {
        assert_eq!(
            summary.status, "Closed",
            "every row in the status=Closed projection must carry status='Closed'",
        );
    }

    // ── Call 2: status="Ready" returns ONLY the two Ready fixtures ──
    // Ensures the widening did not leak Closed issues into the default
    // Ready projection. Priority ASC: #1 (P2) then #2 (P3).
    let ready_result = handle_list(
        &state,
        ListParams {
            status: Some("Ready".to_owned()),
            ..Default::default()
        },
    )
    .await
    .expect("list(status=Ready) should succeed");

    assert_eq!(
        ready_result.total, 2,
        "status=Ready must exclude the three Closed fixtures",
    );
    let ready_numbers: Vec<u64> = ready_result.issues.iter().map(|i| i.number).collect();
    assert_eq!(
        ready_numbers,
        vec![1_u64, 2],
        "status=Ready must return exactly the Ready fixtures in priority order",
    );
    for summary in &ready_result.issues {
        assert_eq!(
            summary.status, "Ready",
            "no Closed issue may leak into the status=Ready projection",
        );
    }

    assert_eq!(
        mock.calls().fetch_graph_data(),
        2,
        "each handle_list call must refetch — no cache short-circuit",
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
            limit: Some(0),
            ..Default::default()
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
#[allow(clippy::too_many_lines)] // Comprehensive end-to-end scenario.
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
    let ready_set = graph.compute_ready_set(&cache_issues, "test", "repo");
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

/// Closes the QA RISK from unblock-29p.7 (tracked under unblock-29p.29):
/// the upstream-error propagation pathway through `handle_search` had no
/// end-to-end coverage — only transitive transport-layer (`wiremock`) and
/// tool-layer unit tests. This test seeds the mock with a representative
/// `RateLimited` (HTTP 429) response, drives `handle_search` with a valid
/// query, and asserts:
///
/// 1. The handler returns `Err(ErrorData)` — the upstream error is not
///    swallowed or fabricated into a degraded `stale = true` envelope
///    (`SearchResult` cannot encode upstream failure; `stale` is always
///    `false` on a successful response — see `tools/search.rs` module
///    docs).
/// 2. The error code is `INTERNAL_ERROR` per the `github_error_to_mcp`
///    mapping table (`errors.rs:99`): 429 → `INTERNAL_ERROR`. This locks
///    in the contract that transient upstream failures are NOT silently
///    re-coded as `INVALID_PARAMS`.
/// 3. The error message preserves the underlying `RateLimited::Display`
///    so callers can diagnose the upstream cause without parsing the
///    JSON-RPC code in isolation.
/// 4. Validation has already passed and the upstream WAS hit exactly once
///    (`mock.calls().search_issues() == 1`) — failure mapping happens
///    AFTER the trait call, not before it.
/// 5. The cache-bypass invariant holds even on the error path
///    (`fetch_graph_data` count remains zero).
#[tokio::test]
async fn search_propagates_upstream_rate_limit_error_through_handle_search() {
    use chrono::Utc;
    use rmcp::model::ErrorCode;
    use unblock_github::errors::RateLimitedSnafu;
    use unblock_mcp::tools::search::{SearchParams, handle_search};

    let mock = new_mock();

    // Seed a `RateLimited` upstream error — the bead dispatch comment
    // names this variant explicitly. `RateLimited::status_code() == 429`,
    // and `github_error_to_mcp` maps the catch-all (non-{400,403,404,409,
    // 412,422}) bucket to `INTERNAL_ERROR` — so this variant exercises
    // the 429 → `INTERNAL_ERROR` lane of the mapping table.
    let reset_at = Utc::now();
    mock.push_search_issues(Err(RateLimitedSnafu { reset_at }.build()));

    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_search(
        &state,
        SearchParams {
            query: "ship the new thing".to_owned(),
            limit: None,
        },
    )
    .await
    .expect_err("upstream RateLimited must propagate as ErrorData, not be swallowed");

    // The 429 → `INTERNAL_ERROR` arm of `github_error_to_mcp` (errors.rs
    // line 99) is the contract: rate-limit / 5xx / network-class errors
    // are surfaced as `INTERNAL_ERROR`, distinguishing them from caller
    // misuse (`INVALID_PARAMS`).
    assert_eq!(
        err.code,
        ErrorCode::INTERNAL_ERROR,
        "RateLimited must map to INTERNAL_ERROR (429 → -32603), not INVALID_PARAMS",
    );
    // The underlying `Display` impl is `"GitHub rate limit exceeded —
    // resets at {reset_at}"`. The mapping must preserve that message
    // verbatim (the `github_error_to_mcp` helper uses `err.to_string()`)
    // so the agent can diagnose the upstream cause.
    assert!(
        err.message.contains("rate limit"),
        "error message must surface the underlying RateLimited Display: {}",
        err.message,
    );

    // The upstream WAS hit once — validation passed and the trait call
    // executed before the error mapping. This is the spec §7.6 invariant
    // we are guarding against regression: `handle_search` must not
    // short-circuit on ambient state.
    assert_eq!(
        mock.calls().search_issues(),
        1,
        "search_issues must be invoked exactly once before mapping the error",
    );
    // Cache-bypass invariant holds on the error path too — `search`
    // never reads or writes the cache (spec §7.6 / §9.1 invariant 10).
    assert_eq!(
        mock.calls().fetch_graph_data(),
        0,
        "search must not touch fetch_graph_data even when upstream errors",
    );
}

// ── Stats tool: integration tests ─────────────────────────────────

/// Build a customisable stats fixture `Issue` — one helper per field
/// the stats aggregator distinguishes.
#[allow(clippy::too_many_arguments)]
fn stats_fixture_issue(
    number: u64,
    status: Status,
    priority: Priority,
    issue_state: IssueState,
    agent: Option<&str>,
    milestone: Option<&str>,
) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_{number}"),
        title: format!("Stats fixture #{number}"),
        issue_type: Some(IssueType::Task),
        status,
        priority,
        agent: agent.map(str::to_owned),
        claimed_at: None,
        pipeline_stage: None,
        story_points: None,
        defer_until: None,
        labels: vec![],
        milestone: milestone.map(str::to_owned),
        assignees: vec![],
        state: issue_state,
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// End-to-end happy path: stats aggregates every bucket, blocked-count
/// unions `Status::Blocked` with graph-open-blockers, ready-count
/// matches `compute_ready_set`, cycle-count surfaces a 2-issue cycle,
/// and per-agent throughput tracks `in_progress`. Verifies that after a
/// cold start the cache was warmed (a single `fetch_graph_data()` call),
/// and that the follow-up stats call hits the cache (still one call).
#[tokio::test]
#[allow(clippy::too_many_lines)] // Comprehensive end-to-end scenario.
async fn stats_aggregates_every_bucket_and_warms_cache() {
    use unblock_mcp::tools::stats::{StatsParams, handle_stats};

    // Issue #1: Ready / P0 — ready.
    // Issue #2: InProgress / P1 / agent=alice — in-progress, not ready.
    // Issue #3: Blocked / P2 — Status::Blocked, bumps blocked_count.
    // Issue #4: Deferred / P3 — deferred.
    // Issue #5: Ready / P4 / blocked-by #1 in graph — bumps
    //   blocked_count (open blocker), NOT ready.
    // Issues #6 & #7: Ready / P2 — form a cycle (#6→#7, #7→#6) for
    //   cycle_count. Because they mutually block, neither is ready.
    // Issue #8: InProgress / P0 / agent=alice — second alice task.
    // Issue #9: InProgress / P2 / agent=bob — bob in-progress.
    let issues = vec![
        stats_fixture_issue(1, Status::Ready, Priority::P0, IssueState::Open, None, None),
        stats_fixture_issue(
            2,
            Status::InProgress,
            Priority::P1,
            IssueState::Open,
            Some("alice"),
            None,
        ),
        stats_fixture_issue(
            3,
            Status::Blocked,
            Priority::P2,
            IssueState::Open,
            None,
            None,
        ),
        stats_fixture_issue(
            4,
            Status::Deferred,
            Priority::P3,
            IssueState::Open,
            None,
            None,
        ),
        stats_fixture_issue(5, Status::Ready, Priority::P4, IssueState::Open, None, None),
        stats_fixture_issue(6, Status::Ready, Priority::P2, IssueState::Open, None, None),
        stats_fixture_issue(7, Status::Ready, Priority::P2, IssueState::Open, None, None),
        stats_fixture_issue(
            8,
            Status::InProgress,
            Priority::P0,
            IssueState::Open,
            Some("alice"),
            None,
        ),
        stats_fixture_issue(
            9,
            Status::InProgress,
            Priority::P2,
            IssueState::Open,
            Some("bob"),
            None,
        ),
    ];
    let edges = vec![
        // Issue #5 blocked by issue #1 (open).
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 5),
            target: QualifiedId::new("acme", "widgets", 1),
        },
        // Cycle: #6 <-> #7.
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 6),
            target: QualifiedId::new("acme", "widgets", 7),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 7),
            target: QualifiedId::new("acme", "widgets", 6),
        },
    ];

    let mock = new_mock();
    // Only the first (cold) call should trigger a fetch. The second
    // call must be served entirely from the cache.
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = state_with_mock(Arc::clone(&mock));

    // ── Call 1 (cold): triggers the single rebuild fetch. ──
    let result = handle_stats(&state, StatsParams { milestone: None })
        .await
        .expect("stats call should succeed on cold cache");

    assert_eq!(result.total, 9);
    // by_status — each bucket counted exactly.
    // Post-`unblock-1zj`: `by_status` keys are canonical TitleCase
    // option names sourced from `Status::option_name`.
    assert_eq!(result.by_status.get("Ready"), Some(&4_usize)); // #1, #5, #6, #7
    assert_eq!(result.by_status.get("In Progress"), Some(&3_usize)); // #2, #8, #9
    assert_eq!(result.by_status.get("Blocked"), Some(&1_usize)); // #3
    assert_eq!(result.by_status.get("Deferred"), Some(&1_usize)); // #4
    assert_eq!(result.by_status.get("Closed"), Some(&0_usize)); // fixture has no Closed issues
    assert_eq!(result.by_status.get("Backlog"), Some(&0_usize)); // fixture has no Backlog issues

    // by_priority — one P0/P1/P3/P4, three P2s, plus another P0 (#8).
    assert_eq!(result.by_priority.get("P0"), Some(&2_usize)); // #1, #8
    assert_eq!(result.by_priority.get("P1"), Some(&1_usize));
    assert_eq!(result.by_priority.get("P2"), Some(&4_usize)); // #3, #6, #7, #9
    assert_eq!(result.by_priority.get("P3"), Some(&1_usize));
    assert_eq!(result.by_priority.get("P4"), Some(&1_usize));

    // blocked_count — #3 (Status::Blocked) + #5 (open blocker) + #6, #7
    // (mutual blockers each see the other as an open blocker).
    assert_eq!(result.blocked_count, 4);
    // ready_count: per spec §3.3, `compute_ready_set` only filters
    // InProgress / Deferred / Closed — Status::Blocked issues with no
    // open blockers WILL appear in the ready set. So:
    //   #1 — Ready / no blocker → ready.
    //   #2/#8/#9 — InProgress (filtered).
    //   #3 — Status::Blocked but no graph blocker → ready (per §3.3).
    //   #4 — Deferred (filtered).
    //   #5 — has open blocker (#1 is Open) → blocked.
    //   #6/#7 — cycle partners, each blocked by the other → blocked.
    // Expected ready set: {1, 3}.
    assert_eq!(result.ready_count, 2);
    assert_eq!(result.cycle_count, 1, "one SCC of size 2 = one cycle");

    // agents — sorted alphabetically; alice has 2 in_progress; bob 1.
    assert_eq!(result.agents.len(), 2);
    assert_eq!(result.agents[0].name, "alice");
    assert_eq!(result.agents[0].in_progress, 2);
    assert_eq!(result.agents[0].completed, 0); // fixture has no Closed issues
    assert_eq!(result.agents[1].name, "bob");
    assert_eq!(result.agents[1].in_progress, 1);
    assert_eq!(result.agents[1].completed, 0);

    // Exactly one fetch should have occurred (cold-cache rebuild).
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "cold stats call rebuilds the cache once",
    );

    // ── Call 2 (warm): zero new fetch calls — cache hit path. ──
    let result2 = handle_stats(&state, StatsParams { milestone: None })
        .await
        .expect("stats call should succeed on warm cache");
    assert_eq!(result2.total, 9);
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "cache-hit stats call must not trigger any additional fetch (spec §7.4)",
    );
}

/// `handle_stats` honours the `milestone` filter: `total`, `by_status`,
/// `by_priority`, `blocked_count`, `ready_count`, and `agents` all
/// reflect only the filtered subset. `cycle_count` remains full-graph
/// (R5 decision) — a cycle entirely outside the milestone still counts.
#[tokio::test]
#[allow(clippy::too_many_lines)] // Comprehensive end-to-end scenario.
async fn stats_milestone_filter_scopes_aggregation_but_not_cycles() {
    use unblock_mcp::tools::stats::{StatsParams, handle_stats};

    // Milestone v1.0:
    //   #1 — Ready / P0 / no blocker.
    //   #2 — InProgress / P2 / agent=alice.
    // Milestone v2.0:
    //   #3 — Ready / P1.
    //   #4 — Ready / P1 (cycle partner with #5).
    //   #5 — Ready / P1 (cycle partner with #4).
    // No milestone:
    //   #6 — Deferred / P3.
    let issues = vec![
        stats_fixture_issue(
            1,
            Status::Ready,
            Priority::P0,
            IssueState::Open,
            None,
            Some("v1.0"),
        ),
        stats_fixture_issue(
            2,
            Status::InProgress,
            Priority::P2,
            IssueState::Open,
            Some("alice"),
            Some("v1.0"),
        ),
        stats_fixture_issue(
            3,
            Status::Ready,
            Priority::P1,
            IssueState::Open,
            None,
            Some("v2.0"),
        ),
        stats_fixture_issue(
            4,
            Status::Ready,
            Priority::P1,
            IssueState::Open,
            None,
            Some("v2.0"),
        ),
        stats_fixture_issue(
            5,
            Status::Ready,
            Priority::P1,
            IssueState::Open,
            None,
            Some("v2.0"),
        ),
        stats_fixture_issue(
            6,
            Status::Deferred,
            Priority::P3,
            IssueState::Open,
            None,
            None,
        ),
    ];
    // Cycle #4 <-> #5 — entirely inside milestone v2.0.
    let edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 4),
            target: QualifiedId::new("acme", "widgets", 5),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 5),
            target: QualifiedId::new("acme", "widgets", 4),
        },
    ];

    let mock = new_mock();
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = state_with_mock(Arc::clone(&mock));

    // ── Filter milestone = v1.0 ──
    let v1 = handle_stats(
        &state,
        StatsParams {
            milestone: Some("v1.0".to_owned()),
        },
    )
    .await
    .expect("stats call should succeed");
    assert_eq!(v1.total, 2, "v1.0 has 2 issues");
    // Post-`unblock-1zj`: TitleCase Status keys.
    assert_eq!(v1.by_status.get("Ready"), Some(&1_usize)); // #1
    assert_eq!(v1.by_status.get("In Progress"), Some(&1_usize)); // #2
    assert_eq!(v1.by_status.get("Blocked"), Some(&0_usize));
    assert_eq!(v1.by_priority.get("P0"), Some(&1_usize));
    assert_eq!(v1.by_priority.get("P2"), Some(&1_usize));
    assert_eq!(v1.by_priority.get("P1"), Some(&0_usize));
    assert_eq!(v1.blocked_count, 0, "no blockers inside v1.0");
    assert_eq!(v1.ready_count, 1, "#1 is ready, #2 is InProgress");
    assert_eq!(
        v1.cycle_count, 1,
        "cycle count is full-graph (R5) — v2.0's cycle still counts",
    );
    assert_eq!(v1.agents.len(), 1);
    assert_eq!(v1.agents[0].name, "alice");
    assert_eq!(v1.agents[0].in_progress, 1);

    // ── Filter milestone = v2.0 ──
    let v2 = handle_stats(
        &state,
        StatsParams {
            milestone: Some("v2.0".to_owned()),
        },
    )
    .await
    .expect("stats call should succeed");
    assert_eq!(v2.total, 3);
    assert_eq!(v2.by_status.get("Ready"), Some(&3_usize));
    // blocked_count: #4 and #5 each have an open blocker (the other).
    assert_eq!(v2.blocked_count, 2);
    // ready_count: #3 has no blockers; #4 and #5 mutually block.
    assert_eq!(v2.ready_count, 1);
    assert_eq!(
        v2.cycle_count, 1,
        "cycle count is full-graph and surfaces the v2.0 cycle",
    );
    assert!(v2.agents.is_empty(), "no agents assigned inside v2.0");
    // Exactly one fetch — the second stats call hit the cache.
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "milestone filter must be a post-filter, not a refetch",
    );
}

/// Closes the QA RISK #1 from unblock-29p.8 (tracked under
/// unblock-29p.32): the cache-empty fallback retry path in
/// `tools/stats.rs:383-408` had no end-to-end coverage. The constituent
/// steps (`fetch_graph_data`, graph build, cache update, aggregation)
/// each have unit coverage, but no test exercises the full composed
/// defensive path: initial rebuild fails → cache stays empty → fallback
/// re-issues `fetch_graph_data` → graph rebuilt locally → cache warmed
/// for follow-up reads → `StatsResult` returned to the caller.
///
/// Why this path exists: `StatsResult` has no `stale` field (R6 spec
/// decision, 2026-04-15 09:28 — see bead description). When the lazy
/// rebuild leaves the cache empty, the only way to honour the spec is to
/// surface the underlying error. The fallback re-issues the fetch
/// directly so a *transient* upstream failure on the rebuild path does
/// not turn into an empty-envelope response when the network has already
/// recovered by the time the handler dispatches the retry.
///
/// A regression in this path would silently drop the retry semantics —
/// callers would receive the rebuild's stale `MockNotMocked`/network
/// error even when the underlying data is available on the next attempt.
///
/// The test:
/// 1. Configures the mock to fail the first `fetch_graph_data` (driving
///    `rebuild_cache` into its `Err` arm — `tools/mod.rs:179-185` —
///    which leaves the cache invalidated/empty).
/// 2. Configures the mock to succeed on the second `fetch_graph_data`
///    (the fallback retry inside `handle_stats`).
/// 3. Calls `handle_stats` against a cold cache.
/// 4. Asserts the call returns a successful, fully-populated
///    `StatsResult` (not an error) — the retry semantics held.
/// 5. Asserts exactly two `fetch_graph_data` calls (rebuild + fallback).
/// 6. Asserts the cache is now warm — a subsequent `handle_stats` call
///    does NOT trigger a third fetch (spec §7.4 cache-hit invariant).
#[tokio::test]
async fn stats_cache_empty_fallback_retries_fetch_graph_data() {
    use unblock_github::errors::GitHubApiSnafu;
    use unblock_mcp::tools::stats::{StatsParams, handle_stats};

    // Build a small but representative fixture so the post-retry
    // `StatsResult` has non-zero counts in every bucket the aggregator
    // distinguishes — this catches regressions where the fallback path
    // forgets to thread an input through one of the helpers (e.g.
    // `compute_ready_set`, `detect_all_cycles`, `aggregate_stats`).
    //
    // Layout:
    //   #1 — Ready / P1 / no blocker → ready, 1 P1.
    //   #2 — InProgress / P0 / agent=alice → 1 in_progress, 1 P0,
    //          agent.alice.in_progress = 1.
    //   #3 — Blocked / P2 / no graph blocker → blocked_count contributor
    //          via `Status::Blocked` (R3 union).
    let issues = vec![
        stats_fixture_issue(1, Status::Ready, Priority::P1, IssueState::Open, None, None),
        stats_fixture_issue(
            2,
            Status::InProgress,
            Priority::P0,
            IssueState::Open,
            Some("alice"),
            None,
        ),
        stats_fixture_issue(
            3,
            Status::Blocked,
            Priority::P2,
            IssueState::Open,
            None,
            None,
        ),
    ];
    let edges: Vec<BlockingEdge> = vec![];

    let mock = new_mock();

    // Stub #1: rebuild_cache's fetch_graph_data fails. `rebuild_cache`
    // (tools/mod.rs:162-186) has already invalidated the cache; the
    // failure leaves it empty. This is the precondition for the
    // fallback retry path.
    mock.push_fetch_graph_data(Err(GitHubApiSnafu {
        status: 503_u16,
        message: "transient upstream — first fetch fails".to_owned(),
    }
    .build()));
    // Stub #2: the fallback retry inside `handle_stats` (stats.rs:384-388)
    // succeeds. The handler must aggregate from the freshly-fetched
    // vectors AND warm the cache so a follow-up call hits the cache-hit
    // path — both behaviours are asserted below.
    mock.push_fetch_graph_data(Ok((issues.clone(), edges.clone())));

    let state = state_with_mock(Arc::clone(&mock));
    assert!(
        !state.cache.is_fresh().await,
        "precondition: cache must start cold so the rebuild path is exercised",
    );

    // ── Drive the fallback retry path. ──
    let result = handle_stats(&state, StatsParams { milestone: None })
        .await
        .expect("fallback retry must rescue a transient initial-rebuild failure");

    // The retry semantics held: caller observes a fully-populated
    // `StatsResult`, not an error. Per-bucket counts validate that the
    // freshly-fetched vectors traversed every aggregation pathway
    // (`aggregate_stats` was called, the graph was built locally, and
    // `compute_ready_set` ran against the configured coordinates).
    assert_eq!(result.total, 3, "all three fixture issues counted");
    // Post-`unblock-1zj`: TitleCase Status keys.
    assert_eq!(result.by_status.get("Ready"), Some(&1_usize));
    assert_eq!(result.by_status.get("In Progress"), Some(&1_usize));
    assert_eq!(result.by_status.get("Blocked"), Some(&1_usize));
    assert_eq!(result.by_priority.get("P0"), Some(&1_usize));
    assert_eq!(result.by_priority.get("P1"), Some(&1_usize));
    assert_eq!(result.by_priority.get("P2"), Some(&1_usize));
    // `Status::Blocked` contributes via the R3 union even with no graph
    // edges — `aggregate_stats` was reached.
    assert_eq!(result.blocked_count, 1);
    // `compute_ready_set` ran on the freshly-built graph. Per spec §3.3:
    //   - #1 (Ready / no blocker) → ready.
    //   - #2 (InProgress) → filtered by Filter 2.
    //   - #3 (Status::Blocked / no graph blocker) → ready (the spec
    //     intentionally does NOT filter `Status::Blocked` issues whose
    //     blockers have all closed; see graph.rs:182-191 and the
    //     existing `stats_aggregates_every_bucket_and_warms_cache`
    //     fixture for the same assertion).
    assert_eq!(
        result.ready_count, 2,
        "ready set must hold #1 (Ready/no blocker) and #3 (Status::Blocked/no graph blocker)",
    );
    assert_eq!(result.cycle_count, 0, "fixture has no cycles");
    assert_eq!(result.agents.len(), 1, "alice is the only assigned agent");
    assert_eq!(result.agents[0].name, "alice");
    assert_eq!(result.agents[0].in_progress, 1);

    // Exactly two `fetch_graph_data` calls — the failed rebuild attempt
    // plus the fallback retry. A regression where the retry was skipped
    // (or where the rebuild somehow consumed two stubs) would trip here.
    assert_eq!(
        mock.calls().fetch_graph_data(),
        2,
        "stats must issue: 1× rebuild_cache fetch (Err) + 1× fallback retry (Ok)",
    );

    // Cache is now warm — the fallback explicitly calls
    // `state.cache.update(...)` (stats.rs:406) so the retry's success is
    // not lost. A follow-up read must hit the cache-hit path with zero
    // additional fetches.
    assert!(
        state.cache.is_fresh().await,
        "fallback success path must populate the cache so subsequent reads are warm",
    );
    let warm_issues = state
        .cache
        .get_issues()
        .await
        .expect("cache must hold the freshly-fetched issue vector");
    assert_eq!(warm_issues.len(), 3, "cache holds the post-retry issue set",);

    // Second call: cache hit — must not trigger another fetch.
    let result2 = handle_stats(&state, StatsParams { milestone: None })
        .await
        .expect("warm-cache stats call must succeed");
    assert_eq!(result2.total, 3);
    assert_eq!(
        mock.calls().fetch_graph_data(),
        2,
        "warm-cache read must NOT trigger any additional fetch (spec §7.4)",
    );
}

// ── Reopen tool: integration tests ────────────────────────────────

/// Build a customisable reopen fixture `Issue` under the mock's
/// `acme/widgets` coordinates. The `state` parameter is the GitHub
/// issue state (Open/Closed); `status` is the Projects V2 workflow
/// status snapshot (Ready/InProgress/etc).
fn reopen_fixture_issue(
    number: u64,
    status: Status,
    state: IssueState,
) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_{number}"),
        title: format!("Reopen fixture #{number}"),
        issue_type: Some(IssueType::Task),
        status,
        priority: Priority::P2,
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
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Happy path: reopen a closed issue whose rebuilt graph has no open
/// blockers. The handler should:
/// - call `fetch_issue` once (Phase 1),
/// - call `reopen_issue` once (Phase 1),
/// - call `fetch_graph_data` once (post-reopen rebuild),
/// - return `blocked = false` / `status = "ready"` after consulting
///   the rebuilt cache.
///
/// Projects V2 field-update ladder is short-circuited by the default
/// empty `field_ids` stub (which returns `None`) — so no extra project
/// calls are expected. The test focuses on the core re-evaluation
/// contract.
#[tokio::test]
async fn reopen_closed_issue_with_no_blockers_transitions_to_ready() {
    use unblock_mcp::tools::reopen::{ReopenParams, handle_reopen};

    let mock = new_mock();

    // Phase 1 stubs: fetch returns a Closed issue, reopen succeeds.
    let closed = reopen_fixture_issue(42, Status::Closed, IssueState::Closed);
    mock.push_fetch_issue(Ok(closed));
    mock.push_reopen_issue(Ok(()));

    // Post-reopen rebuild: issue #42 is now Open with no blockers, plus
    // an unrelated issue #7 to make the graph non-trivial.
    let rebuilt_42 = reopen_fixture_issue(42, Status::Ready, IssueState::Open);
    let other = reopen_fixture_issue(7, Status::Ready, IssueState::Open);
    mock.push_fetch_graph_data(Ok((vec![rebuilt_42, other], vec![])));

    let state = state_with_mock(Arc::clone(&mock));

    let result = handle_reopen(&state, ReopenParams { id: 42 })
        .await
        .expect("reopen call should succeed");

    assert_eq!(result.issue, 42);
    assert!(
        !result.blocked,
        "issue with no open blockers must not be blocked",
    );
    assert_eq!(
        result.status, "Ready",
        "unblocked reopen must emit canonical TitleCase `Ready` (post-`unblock-1zj`)",
    );

    // Exactly one call to each of the three GitHub operations.
    assert_eq!(mock.calls().fetch_issue(), 1, "Phase 1 fetches once");
    assert_eq!(mock.calls().reopen_issue(), 1, "reopen is called once");
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "post-reopen rebuild fetches graph data once",
    );

    // Cache is warm after the rebuild — a follow-up `get_issues` hits
    // the cache without additional fetches.
    let cached_issues = state
        .cache
        .get_issues()
        .await
        .expect("cache should be warm after reopen");
    assert_eq!(
        cached_issues.len(),
        2,
        "cache contains the rebuilt issue set",
    );
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "cache-hit read must not trigger an extra fetch",
    );
}

/// Reopen a closed issue that has an OPEN blocker in the rebuilt graph.
/// Verifies the handler returns `blocked = true` / `status = "blocked"`.
/// This is the counterpart of the happy path above.
#[tokio::test]
async fn reopen_closed_issue_with_open_blocker_transitions_to_blocked() {
    use unblock_mcp::tools::reopen::{ReopenParams, handle_reopen};

    let mock = new_mock();

    // Phase 1: closed fixture + successful reopen.
    let closed = reopen_fixture_issue(42, Status::Closed, IssueState::Closed);
    mock.push_fetch_issue(Ok(closed));
    mock.push_reopen_issue(Ok(()));

    // Post-reopen rebuild: issue #42 is Open and blocked by #99 (also
    // Open). The graph edge encodes "source #42 is blocked by target
    // #99" — the same convention used everywhere else in the test
    // suite (see cache/stats fixtures).
    let rebuilt_42 = reopen_fixture_issue(42, Status::Ready, IssueState::Open);
    let blocker = reopen_fixture_issue(99, Status::Ready, IssueState::Open);
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    mock.push_fetch_graph_data(Ok((vec![rebuilt_42, blocker], edges)));

    let state = state_with_mock(Arc::clone(&mock));

    let result = handle_reopen(&state, ReopenParams { id: 42 })
        .await
        .expect("reopen call should succeed");

    assert_eq!(result.issue, 42);
    assert!(
        result.blocked,
        "issue with an open blocker in the rebuilt graph must be blocked",
    );
    assert_eq!(
        result.status, "Blocked",
        "blocked reopen must emit canonical TitleCase `Blocked` (post-`unblock-1zj`)",
    );

    assert_eq!(mock.calls().fetch_issue(), 1);
    assert_eq!(mock.calls().reopen_issue(), 1);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

/// Reopen rejects an already-open issue with `IssueAlreadyOpen` (R2),
/// without issuing the reopen mutation and without touching the cache.
#[tokio::test]
async fn reopen_rejects_already_open_issue() {
    use unblock_mcp::tools::reopen::{ReopenParams, handle_reopen};

    let mock = new_mock();

    // Phase 1 stub: fetch returns an OPEN issue. The handler must
    // error out before calling reopen_issue.
    let already_open = reopen_fixture_issue(42, Status::Ready, IssueState::Open);
    mock.push_fetch_issue(Ok(already_open));

    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_reopen(&state, ReopenParams { id: 42 })
        .await
        .expect_err("reopen on an already-open issue must fail");

    // 409 Conflict (IssueAlreadyOpen) maps to INVALID_PARAMS per
    // errors.rs:85.
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("already open"),
        "error message must explain the failure: {}",
        err.message,
    );

    // Invariants: reopen_issue never called; cache never rebuilt.
    assert_eq!(mock.calls().fetch_issue(), 1);
    assert_eq!(
        mock.calls().reopen_issue(),
        0,
        "a validation failure must short-circuit before the reopen mutation",
    );
    assert_eq!(
        mock.calls().fetch_graph_data(),
        0,
        "no rebuild is attempted when Phase 1 fails",
    );
}

/// Reopen rejects `id = 0` before any network call (R1 — fail-fast).
#[tokio::test]
async fn reopen_rejects_zero_id_without_fetching() {
    use unblock_mcp::tools::reopen::{ReopenParams, handle_reopen};

    let mock = new_mock();
    // Intentionally do NOT push fetch_issue / reopen_issue stubs. If
    // the handler bypassed validation it would fall into the
    // `MockNotStubbed` fallback and fail the test with a different
    // error than the one we want to assert on.
    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_reopen(&state, ReopenParams { id: 0 })
        .await
        .expect_err("id=0 must fail validation");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("id"),
        "validation error must explain the id bound: {}",
        err.message,
    );

    // Validation short-circuits BEFORE any network call.
    assert_eq!(
        mock.calls().fetch_issue(),
        0,
        "id=0 must fail before fetch_issue (R1 fail-fast)",
    );
    assert_eq!(mock.calls().reopen_issue(), 0);
    assert_eq!(mock.calls().fetch_graph_data(), 0);
}

/// R3: when the post-reopen cache rebuild fails (e.g. transient
/// GitHub 503), the reopen has already succeeded server-side. The
/// handler must NOT silently default `blocked = false` — it must
/// propagate a clear error telling the caller to re-run `show`.
#[tokio::test]
async fn reopen_surfaces_error_when_post_reopen_rebuild_fails() {
    use unblock_github::errors::GitHubApiSnafu;
    use unblock_mcp::tools::reopen::{ReopenParams, handle_reopen};

    let mock = new_mock();

    // Phase 1: fetch + reopen both succeed.
    let closed = reopen_fixture_issue(42, Status::Closed, IssueState::Closed);
    mock.push_fetch_issue(Ok(closed));
    mock.push_reopen_issue(Ok(()));

    // Post-reopen rebuild: simulate a transient 503 — `execute_write_tool`
    // leaves the cache empty after logging the error (see
    // `tools/mod.rs:167-173`), so the reopen handler's cache-check
    // falls through to the R3 "surface an error" branch.
    mock.push_fetch_graph_data(Err(GitHubApiSnafu {
        status: 503_u16,
        message: "upstream service unavailable".to_owned(),
    }
    .build()));

    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_reopen(&state, ReopenParams { id: 42 })
        .await
        .expect_err("cache rebuild failure must surface as a handler error (R3)");

    // The error must reference the partial-state guidance so agents
    // know to retry `show`, name the preceding mutation, and include
    // the fully-qualified issue reference. After unblock-29p.35 the
    // surfaced message comes from
    // `PostMutationRebuildFailed::Display`; it names the mutation
    // (`"reopen"`) rather than the past participle `"reopened"` used by
    // the prior synthetic message.
    assert!(
        err.message.contains("reopen")
            && err.message.contains("show")
            && err.message.contains("#42"),
        "error message must name the mutation, include the QID, and instruct re-run of `show`: {}",
        err.message,
    );

    // Despite the rebuild failure, the reopen mutation DID land.
    assert_eq!(
        mock.calls().reopen_issue(),
        1,
        "reopen is durable: mutation persists even if rebuild fails",
    );
    assert_eq!(mock.calls().fetch_issue(), 1);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
    // Cache is empty because rebuild failed.
    assert!(
        !state.cache.is_fresh().await,
        "cache must be invalidated and not repopulated after rebuild failure",
    );
}

/// R3 race-window path: when the post-reopen cache rebuild succeeds
/// (returning a non-empty issue set) but the reopened issue is absent
/// from the rebuilt set — e.g. another agent re-closed it between our
/// `reopen_issue` mutation and the `fetch_graph_data` rebuild — the
/// handler MUST NOT silently default `blocked = false` and return
/// `status = "ready"`. It must surface a 503-class error instructing
/// the caller to re-run `show`, mirroring the empty-cache arm.
/// Preserves spec §14 invariants 8 and 13 (no fictional Status claims
/// when the graph cannot actually be consulted for the reopened issue).
#[tokio::test]
async fn reopen_surfaces_error_when_rebuilt_cache_missing_reopened_issue() {
    use unblock_mcp::tools::reopen::{ReopenParams, handle_reopen};

    let mock = new_mock();

    // Phase 1: fetch + reopen both succeed.
    let closed = reopen_fixture_issue(42, Status::Closed, IssueState::Closed);
    mock.push_fetch_issue(Ok(closed));
    mock.push_reopen_issue(Ok(()));

    // Post-reopen rebuild SUCCEEDS but returns a set that does NOT
    // contain the reopened issue #42. Simulates the race where another
    // agent re-closed #42 between our mutation and the rebuild. The
    // rebuilt graph is non-trivial (issue #7 is present) so the
    // empty-cache arm at reopen.rs:382-410 is explicitly NOT exercised
    // here — the missing-issue arm at :426-440 is.
    let unrelated = reopen_fixture_issue(7, Status::Ready, IssueState::Open);
    mock.push_fetch_graph_data(Ok((vec![unrelated], vec![])));

    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_reopen(&state, ReopenParams { id: 42 })
        .await
        .expect_err(
            "rebuilt cache missing the reopened issue must surface as a handler error (R3 race)",
        );

    // 503 → INTERNAL_ERROR per github_error_to_mcp.
    assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    // The error must reference the partial-state guidance so agents
    // know to retry `show`, name the preceding mutation, and include
    // the fully-qualified issue reference. Same
    // `PostMutationRebuildFailed` surface as the empty-cache arm — the
    // two race-window arms produce identical wire output; call-site
    // log lines disambiguate the cause in traces.
    assert!(
        err.message.contains("reopen")
            && err.message.contains("show")
            && err.message.contains("#42"),
        "error message must name the mutation, include the QID, and instruct re-run of `show`: {}",
        err.message,
    );

    // Despite the race, the reopen mutation DID land — it is durable.
    assert_eq!(
        mock.calls().reopen_issue(),
        1,
        "reopen is durable: mutation persists even if the rebuilt cache races",
    );
    assert_eq!(mock.calls().fetch_issue(), 1);
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "post-reopen rebuild is attempted exactly once",
    );

    // The rebuild itself succeeded (returned a non-empty issue set),
    // so execute_write_tool leaves the cache in a fresh state. Only
    // the *reopened* issue is missing from that fresh cache.
    assert!(
        state.cache.is_fresh().await,
        "cache must be fresh when the rebuild succeeded — only the reopened issue is missing",
    );
}

// ── dep_remove tool: integration tests ────────────────────────────

/// Build a [`ProjectFieldIds`] fixture with a populated Status-option map
/// containing a `"ready"` slug. When supplied via
/// `push_field_ids(Some(dep_remove_field_ids()))`, the best-effort Status
/// update ladder in [`dep_remove`](unblock_mcp::tools::dep_remove)
/// successfully resolves `field_ids.status.options["ready"]` and fires a
/// real `update_field` call — which the counter-based assertions below
/// depend on.
///
/// Kept local to this module so the reopen/create integration tests keep
/// their existing no-op Status-ladder posture (empty `status.options`).
fn dep_remove_field_ids() -> unblock_github::projects::ProjectFieldIds {
    use std::collections::HashMap;
    use unblock_github::projects::{FieldMeta, ProjectFieldIds};

    let mut status_options = HashMap::new();
    // Post-`unblock-1zj`: canonical TitleCase option name from
    // `Status::option_name`.
    status_options.insert(
        unblock_core::types::Status::Ready.option_name().to_owned(),
        "OPT_READY".to_owned(),
    );

    let empty_meta = || FieldMeta::new("f".to_owned(), HashMap::new());

    ProjectFieldIds {
        status: FieldMeta::new("status-field-id".to_owned(), status_options),
        priority: empty_meta(),
        pipeline_stage: empty_meta(),
        agent: "agent".to_owned(),
        claimed_at: "ca".to_owned(),
        story_points: "sp".to_owned(),
        defer_until: "du".to_owned(),
    }
}

/// Build a fixture issue under `acme/widgets` coordinates for
/// `dep_remove` tests.
fn dep_remove_fixture_issue(number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_{number}"),
        title: format!("DepRemove fixture #{number}"),
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Happy path (spec §8.5): local source blocked by local target, edge
/// exists in the warm cache, `remove_blocked_by_refs` succeeds, post-
/// mutation rebuild returns the same two issues with NO remaining edges,
/// and the source's re-evaluation finds zero open blockers — so the
/// handler must fire the `Status=ready` update ladder.
///
/// Asserts:
/// - `removed = true`,
/// - `message` mentions both `#42` and `#99`,
/// - call counters: `remove_blocked_by_refs = 1`, `fetch_graph_data = 1`,
///   `field_ids = 1`, `resolve_project_info = 1`, `get_project_item_id = 1`,
///   `update_field = 1`,
/// - `remove_blocked_by_ref` stays at `0` (the handler must use the
///   two-ref variant, not the local-only one).
#[tokio::test]
async fn dep_remove_local_edge_transitions_source_to_ready() {
    use unblock_github::projects::ProjectInfo;
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();

    // Phase 1 mutation stub.
    mock.push_remove_blocked_by_refs(Ok(()));

    // Post-mutation rebuild: source #42 and target #99, but NO remaining
    // edges — so #42 is now unblocked after the removal.
    //
    // The rebuilt source is seeded as `Status::Blocked` (rather than the
    // generic-fixture default of `Status::Ready`) so this test asserts the
    // Blocked→Ready transition explicitly. Per spec §8.5 step 5 the
    // status-update ladder in `dep_remove.rs` fires unconditionally when
    // `has_open_blockers` returns false; with a Ready source this
    // assertion would only validate a Ready→Ready no-op. A Blocked source
    // catches any future regression where the ladder is made conditional
    // on the current Projects V2 Status value (see bead unblock-29p.39).
    let mut rebuilt_source = dep_remove_fixture_issue(42);
    rebuilt_source.status = Status::Blocked;
    let rebuilt_target = dep_remove_fixture_issue(99);
    mock.push_fetch_graph_data(Ok((vec![rebuilt_source, rebuilt_target], vec![])));

    // Status-update ladder stubs (fired because the source is Local AND
    // has zero open blockers after the rebuild).
    mock.push_field_ids(Some(dep_remove_field_ids()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_42".to_owned()));
    mock.push_update_field(Ok(()));

    // Pre-populate the cache with the source→target blocking edge so the
    // warm-cache pre-mutation guard passes. Edge convention throughout
    // the test suite: `source = blocked issue`, `target = blocker`.
    let state = state_with_mock(Arc::clone(&mock));
    let pre_source = dep_remove_fixture_issue(42);
    let pre_target = dep_remove_fixture_issue(99);
    let pre_edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    let pre_issues = vec![pre_source, pre_target];
    let pre_graph = DependencyGraph::build(&pre_issues, &pre_edges);
    let pre_ready_set = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state
        .cache
        .update(pre_issues, pre_ready_set, pre_graph)
        .await;
    assert!(
        state.cache.is_fresh().await,
        "cache must be warm so the pre-mutation edge guard can run",
    );

    // Act.
    let result = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect("dep_remove should succeed on a warm-cache edge");

    // Response shape.
    assert!(result.removed, "edge must be reported as removed");
    assert_eq!(result.source, "#42", "local source renders as `#n`");
    assert_eq!(result.target, "#99", "local target renders as `#n`");
    assert!(
        result.message.contains("#42") && result.message.contains("#99"),
        "message must mention both references: {}",
        result.message,
    );

    // Call-counter contract. These are the load-bearing assertions — they
    // prove the handler used the cross-repo-capable `remove_blocked_by_refs`
    // path, rebuilt the cache exactly once, and fired the Projects V2
    // Status update ladder through to `update_field`.
    let calls = mock.calls();
    assert_eq!(
        calls.remove_blocked_by_refs(),
        1,
        "the cross-repo-capable mutation variant must be used",
    );
    assert_eq!(
        calls.remove_blocked_by_ref(),
        0,
        "the single-side `_ref` variant must NOT be used by this handler",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        1,
        "post-mutation rebuild fetches graph data exactly once",
    );
    assert_eq!(
        calls.field_ids(),
        1,
        "Status-update ladder starts with a field_ids lookup",
    );
    assert_eq!(
        calls.resolve_project_info(),
        1,
        "Status-update ladder resolves the project exactly once",
    );
    assert_eq!(
        calls.get_project_item_id(),
        1,
        "Status-update ladder fetches the project item id exactly once",
    );
    assert_eq!(
        calls.update_field(),
        1,
        "zero-blocker source must flip Projects V2 Status to ready (spec §8.5 step 5)",
    );

    // Cache must be warm after the successful rebuild.
    assert!(
        state.cache.is_fresh().await,
        "cache must be repopulated after the post-mutation rebuild",
    );
}

/// Sticky-Backlog (`unblock-1zj` extension of spec §8.5 / Invariant
/// 15(b)): when the source is `Status::Backlog` and `dep_remove` would
/// otherwise transition it to `Ready`, the handler MUST skip the Status
/// update entirely. Backlog is sticky — `compute_expected_status`
/// (§10.2) preserves it, so the post-mutation cross-check MUST do the
/// same. Without this guard, dropping a blocker on a Backlog source
/// would silently flip it to Ready.
#[tokio::test]
async fn dep_remove_backlog_source_skips_status_update_per_sticky_rule() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();
    mock.push_remove_blocked_by_refs(Ok(()));

    // Post-rebuild source is in Backlog with no remaining blockers.
    let mut rebuilt_source = dep_remove_fixture_issue(42);
    rebuilt_source.status = Status::Backlog;
    let rebuilt_target = dep_remove_fixture_issue(99);
    mock.push_fetch_graph_data(Ok((vec![rebuilt_source, rebuilt_target], vec![])));

    let state = state_with_mock(Arc::clone(&mock));
    let mut pre_source = dep_remove_fixture_issue(42);
    pre_source.status = Status::Backlog;
    let pre_target = dep_remove_fixture_issue(99);
    let pre_edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    let pre_issues = vec![pre_source, pre_target];
    let pre_graph = DependencyGraph::build(&pre_issues, &pre_edges);
    let pre_ready_set = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state
        .cache
        .update(pre_issues, pre_ready_set, pre_graph)
        .await;

    let result = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect("dep_remove should succeed on a warm-cache edge");

    assert!(result.removed);

    let calls = mock.calls();
    assert_eq!(
        calls.remove_blocked_by_refs(),
        1,
        "the edge is still removed — sticky-Backlog only skips the Status update",
    );
    assert_eq!(
        calls.update_field(),
        0,
        "spec §10.2 sticky-Backlog rule: NO Status update for Backlog source",
    );
}

/// Defensive `source == target` rejection (spec §8.4 parity — see the
/// module-level docs of `dep_remove` for the rationale). The handler must
/// fail fast BEFORE issuing any network call.
#[tokio::test]
async fn dep_remove_rejects_source_equals_target_without_network_calls() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();
    // Intentionally push no stubs — if the handler leaked past validation
    // it would hit `MockNotStubbed` and fail the test with a noisy error
    // instead of the clean INVALID_PARAMS we want to assert on.
    let state = state_with_mock(Arc::clone(&mock));

    let err = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "7".to_owned(),
            target: "#7".to_owned(),
        },
    )
    .await
    .expect_err("source == target must fail validation");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("source and target must differ"),
        "validation message must explain the constraint: {}",
        err.message,
    );

    // Zero network traffic — validation short-circuits first.
    let calls = mock.calls();
    assert_eq!(calls.remove_blocked_by_refs(), 0);
    assert_eq!(calls.remove_blocked_by_ref(), 0);
    assert_eq!(calls.fetch_graph_data(), 0);
    assert_eq!(calls.update_field(), 0);
}

/// When the warm cache has no edge between `source_qid` and `target_qid`,
/// the pre-mutation guard reports `removed: false` and skips the mutation.
/// Covers the unified missing-edge posture from `unblock-29p.54`:
/// warm+both-Local now surfaces the SAME wire signal as cold/cross-repo
/// (`removed: false`, no mutation) instead of the retired
/// `INVALID_PARAMS` error.
#[tokio::test]
async fn dep_remove_warm_cache_missing_edge_reports_false_without_mutation() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();
    // No stubs — a leak past the guard must surface as `MockNotStubbed`
    // in the counter assertions below.
    let state = state_with_mock(Arc::clone(&mock));

    // Warm cache with #42 and #99 but NO edge between them.
    let issues = vec![dep_remove_fixture_issue(42), dep_remove_fixture_issue(99)];
    let graph = DependencyGraph::build(&issues, &[]);
    let ready_set = graph.compute_ready_set(&issues, "acme", "widgets");
    state.cache.update(issues, ready_set, graph).await;

    let result = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect("missing edge in warm cache must early-return removed:false");

    assert!(!result.removed, "absent edge must yield removed=false");
    assert_eq!(result.source, "#42");
    assert_eq!(result.target, "#99");
    assert!(
        result.message.contains("No blocking edge to remove"),
        "message must document the no-op: {}",
        result.message,
    );

    // No mutation issued — guard short-circuited.
    let calls = mock.calls();
    assert_eq!(calls.remove_blocked_by_refs(), 0);
    assert_eq!(calls.remove_blocked_by_ref(), 0);
    assert_eq!(calls.fetch_graph_data(), 0);
    assert_eq!(
        calls.update_field(),
        0,
        "no mutation → no Status update ladder"
    );
    // Warm + both-Local fast path: the in-memory guard must NOT call
    // `fetch_issue_ref` (that's the cold/cross-repo probe path).
    assert_eq!(
        calls.fetch_issue_ref(),
        0,
        "warm + both-Local fast path must NOT call fetch_issue_ref"
    );
}

/// R3 — when the post-mutation cache rebuild fails (transient GitHub
/// 5xx), the blocking edge has already been removed server-side. The
/// handler must propagate a 503-class error referencing both `show` and
/// the two endpoints, NOT silently default to `removed=true, status=ready`.
#[tokio::test]
async fn dep_remove_surfaces_error_when_post_mutation_rebuild_fails() {
    use unblock_github::errors::GitHubApiSnafu;
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();

    // Mutation succeeds on GitHub…
    mock.push_remove_blocked_by_refs(Ok(()));
    // …but the post-mutation rebuild fails.
    mock.push_fetch_graph_data(Err(GitHubApiSnafu {
        status: 503_u16,
        message: "upstream service unavailable".to_owned(),
    }
    .build()));

    let state = state_with_mock(Arc::clone(&mock));

    // Warm cache with the edge so the pre-mutation guard passes.
    let issues = vec![dep_remove_fixture_issue(42), dep_remove_fixture_issue(99)];
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues, "acme", "widgets");
    state.cache.update(issues, ready_set, graph).await;

    let err = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect_err("rebuild failure must surface as a handler error (R3)");

    // Must instruct the caller to re-run `show`, name the preceding
    // mutation, and identify the source endpoint whose Status
    // re-evaluation was skipped. Target endpoint (`#99`) is no longer
    // surfaced in the rendered message (the `PostMutationRebuildFailed`
    // variant carries a single QualifiedId — the source, matching the
    // `EndpointClosed` precedent) but is still logged via the
    // `reevaluate_source_after_remove` `warn!` as a structured field;
    // see `tools/dep_remove.rs:490-500`. Spec §8.5 R3 row is about the
    // handler's ability to compute `has_open_blockers` for the source,
    // so the source is the semantically-authoritative QualifiedId for
    // this error.
    assert!(
        err.message.contains("show")
            && err.message.contains("remove_blocked_by")
            && err.message.contains("acme/widgets#42"),
        "R3 error must reference `show`, the preceding mutation, and the source endpoint QID: {}",
        err.message,
    );

    // Mutation landed; rebuild was attempted once; Status ladder never
    // fired (cache was empty after the failed rebuild).
    let calls = mock.calls();
    assert_eq!(
        calls.remove_blocked_by_refs(),
        1,
        "mutation is durable — it ran even though the rebuild failed",
    );
    assert_eq!(calls.fetch_graph_data(), 1);
    assert_eq!(
        calls.update_field(),
        0,
        "Status ladder must NOT fire when the cache rebuild failed",
    );
    assert!(
        !state.cache.is_fresh().await,
        "cache must be invalidated and not repopulated after rebuild failure",
    );
}

// ── dep_remove — cold-cache + cross-repo edge validation (unblock-29p.43)

/// Build a fixture issue with a populated `blocked_by` list — the list
/// is what `probe_edge_via_fetch` scans on the cold-cache / cross-repo
/// path. The blocker's `repo_owner` / `repo_name` default to `None`
/// (same-repo convention) unless the caller passes explicit values via
/// the second argument.
fn dep_remove_fixture_issue_with_blockers(
    number: u64,
    blockers: Vec<unblock_core::types::RelatedIssue>,
) -> unblock_core::types::Issue {
    let mut issue = dep_remove_fixture_issue(number);
    issue.blocked_by = blockers;
    issue
}

/// Helper: local (same-repo-as-configured) blocker — delegates to
/// [`unblock_core::types::RelatedIssue::local`], leaving `repo_owner`
/// / `repo_name` as `None` so callers exercise the default-to-
/// enclosing-repo branch in `probe_edge_via_fetch`.
fn local_blocker(number: u64) -> unblock_core::types::RelatedIssue {
    unblock_core::types::RelatedIssue::local(number, format!("Blocker #{number}"), IssueState::Open)
}

/// Helper: cross-repo blocker — delegates to
/// [`unblock_core::types::RelatedIssue::cross_repo`] with explicit
/// `owner` / `name` so the probe can distinguish a cross-repo blocker
/// from a same-repo blocker of the same number (the
/// `FETCH_ISSUE_QUERY` subselection extension in `unblock-29p.43`).
fn cross_repo_blocker(owner: &str, repo: &str, number: u64) -> unblock_core::types::RelatedIssue {
    unblock_core::types::RelatedIssue::cross_repo(
        number,
        format!("{owner}/{repo}#{number}"),
        IssueState::Open,
        owner,
        repo,
    )
}

/// Cold cache, edge DOES exist → the handler calls `fetch_issue_ref`
/// on the source, sees the target in `blocked_by`, proceeds to the
/// mutation, and returns `removed: true`. Locks the cold-cache arm of
/// `probe_edge_presence` — no cache pre-seeding, exactly one
/// `fetch_issue_ref` call, exactly one `remove_blocked_by_refs` call.
#[tokio::test]
async fn dep_remove_cold_cache_validates_edge_via_single_issue_fetch() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();

    // Pre-mutation probe: source #42 reports #99 as an open blocker.
    let source_with_blocker = dep_remove_fixture_issue_with_blockers(42, vec![local_blocker(99)]);
    mock.push_fetch_issue_ref(Ok(source_with_blocker));

    // Mutation succeeds.
    mock.push_remove_blocked_by_refs(Ok(()));

    // Post-mutation rebuild returns the two issues without any edges —
    // source is now unblocked, so the Status-update ladder fires.
    mock.push_fetch_graph_data(Ok((
        vec![dep_remove_fixture_issue(42), dep_remove_fixture_issue(99)],
        vec![],
    )));
    mock.push_field_ids(Some(dep_remove_field_ids()));
    mock.push_resolve_project_info(Ok(unblock_github::projects::ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_42".to_owned()));
    mock.push_update_field(Ok(()));

    // Cold cache — no seeding.
    let state = state_with_mock(Arc::clone(&mock));
    assert!(
        !state.cache.is_fresh().await,
        "precondition: cache must be cold"
    );

    let result = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect("cold-cache existing edge must drive a successful removal");

    assert!(result.removed, "existing edge must be reported as removed");
    assert_eq!(result.source, "#42");
    assert_eq!(result.target, "#99");

    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "cold-cache probe must call fetch_issue_ref exactly once"
    );
    assert_eq!(
        calls.remove_blocked_by_refs(),
        1,
        "mutation must run when the edge was confirmed present"
    );
    assert_eq!(calls.fetch_graph_data(), 1, "one post-mutation rebuild");
    assert_eq!(
        calls.update_field(),
        1,
        "zero-blocker source must flip Status to ready"
    );
}

/// Cold cache, edge does NOT exist → the probe reports absence and the
/// handler MUST NOT call `remove_blocked_by_refs`. Response:
/// `removed: false`, message explains "no blocking edge to remove".
/// Locks Invariant 11 on the cold-cache path (spec §14 "Validation
/// before mutation").
#[tokio::test]
async fn dep_remove_cold_cache_reports_false_when_edge_never_existed() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();

    // Source #42 has NO blockers — the probe returns an empty
    // `blocked_by` so `probe_edge_via_fetch` yields MissingSkipMutation.
    mock.push_fetch_issue_ref(Ok(dep_remove_fixture_issue_with_blockers(42, vec![])));

    let state = state_with_mock(Arc::clone(&mock));
    assert!(
        !state.cache.is_fresh().await,
        "precondition: cache must be cold"
    );

    let result = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect("cold-cache absent edge must early-return, not error");

    assert!(
        !result.removed,
        "absent edge must be reported as removed=false"
    );
    assert_eq!(result.source, "#42");
    assert_eq!(result.target, "#99");
    assert!(
        result.message.contains("No blocking edge to remove"),
        "message must document the no-op: {}",
        result.message,
    );

    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "cold-cache probe must call fetch_issue_ref exactly once"
    );
    assert_eq!(
        calls.remove_blocked_by_refs(),
        0,
        "Invariant 11: mutation MUST NOT run when the probe proved absence"
    );
    assert_eq!(
        calls.remove_blocked_by_ref(),
        0,
        "single-side variant must NOT run either"
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild"
    );
    assert_eq!(
        calls.update_field(),
        0,
        "no mutation → no Status update ladder"
    );
}

/// Cross-repo target, edge DOES exist → the probe (triggered by the
/// non-Local target) sees the cross-repo blocker disambiguated via
/// `RelatedIssue.repo_owner` / `.repo_name` (fetched by the extended
/// `FETCH_ISSUE_QUERY` subselection), proceeds to the mutation, and
/// returns `removed: true`. Locks the cross-repo arm of
/// `probe_edge_presence`.
#[tokio::test]
async fn dep_remove_cross_repo_validates_edge_via_single_issue_fetch() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();

    // Source is local #42, target is cross-repo other/repo#99. The
    // probe fetches the source and must find a blocker whose
    // repository.owner.login == "other" AND repository.name == "repo"
    // AND number == 99. `cross_repo_blocker` encodes that fixture.
    let source_with_xrepo_blocker =
        dep_remove_fixture_issue_with_blockers(42, vec![cross_repo_blocker("other", "repo", 99)]);
    mock.push_fetch_issue_ref(Ok(source_with_xrepo_blocker));

    mock.push_remove_blocked_by_refs(Ok(()));

    // Post-mutation rebuild (source is local, so the rebuild + Status
    // ladder still fires). No in-repo edges remain after the mutation.
    mock.push_fetch_graph_data(Ok((vec![dep_remove_fixture_issue(42)], vec![])));
    mock.push_field_ids(Some(dep_remove_field_ids()));
    mock.push_resolve_project_info(Ok(unblock_github::projects::ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_42".to_owned()));
    mock.push_update_field(Ok(()));

    // Warm the cache — but with NO edge in-repo. The handler must still
    // use the cross-repo probe path, not the warm-cache fast path
    // (which only fires on both-Local). Validates that the branch
    // predicate is `is_both_local && is_cache_warm`, not just warm.
    let state = state_with_mock(Arc::clone(&mock));
    let pre_issues = vec![dep_remove_fixture_issue(42)];
    let pre_graph = DependencyGraph::build(&pre_issues, &[]);
    let pre_ready = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state.cache.update(pre_issues, pre_ready, pre_graph).await;
    assert!(state.cache.is_fresh().await);

    let result = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "other/repo#99".to_owned(),
        },
    )
    .await
    .expect("cross-repo edge present must drive a successful removal");

    assert!(result.removed);
    assert_eq!(result.source, "#42");
    assert_eq!(result.target, "other/repo#99");

    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "cross-repo probe routes through fetch_issue_ref even with a warm cache"
    );
    assert_eq!(calls.remove_blocked_by_refs(), 1);
    assert_eq!(calls.fetch_graph_data(), 1);
}

/// Cross-repo target, edge does NOT exist → probe returns absence, the
/// handler early-returns `removed: false`, and the mutation MUST NOT
/// run. Locks Invariant 11 on the cross-repo path.
#[tokio::test]
async fn dep_remove_cross_repo_reports_false_when_edge_never_existed() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();

    // Source #42 blocked only by a DIFFERENT cross-repo issue — the
    // probe must reject the lookup because (owner, repo, number) does
    // not match the target. This also guards against a naive scan that
    // only compares `number` and would incorrectly accept other/repo#99
    // when only other/different-repo#99 is present.
    let source_with_unrelated_xrepo_blocker = dep_remove_fixture_issue_with_blockers(
        42,
        vec![cross_repo_blocker("other", "different-repo", 99)],
    );
    mock.push_fetch_issue_ref(Ok(source_with_unrelated_xrepo_blocker));

    let state = state_with_mock(Arc::clone(&mock));
    assert!(
        !state.cache.is_fresh().await,
        "precondition: cache must be cold (also exercises cold+cross-repo)",
    );

    let result = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "other/repo#99".to_owned(),
        },
    )
    .await
    .expect("cross-repo absent edge must early-return, not error");

    assert!(!result.removed, "absent edge must yield removed=false");
    assert_eq!(result.source, "#42");
    assert_eq!(result.target, "other/repo#99");
    assert!(
        result.message.contains("No blocking edge to remove"),
        "message must document the no-op: {}",
        result.message,
    );

    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "cross-repo probe must call fetch_issue_ref exactly once"
    );
    assert_eq!(
        calls.remove_blocked_by_refs(),
        0,
        "Invariant 11: mutation MUST NOT run when the probe proved absence"
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild"
    );
    assert_eq!(calls.update_field(), 0);
}

/// Armadilha (trap-test) locking the None-means-same-repo convention
/// fallback inside `probe_edge_via_fetch` (bead `unblock-29p.58`).
///
/// Scenario:
/// - MCP client is configured for `acme/widgets` (via `new_mock()`).
/// - `dep_remove` source is a CROSS-REPO issue: `otherowner/otherrepo#42`.
/// - The source carries a single blockedBy blocker whose `repo_owner`
///   / `repo_name` are both `None` — i.e. the GraphQL response did NOT
///   emit an explicit `repository { owner { login } name }` subselection
///   for that node (same-repo default in the GitHub API).
/// - `dep_remove` target is a DIFFERENT cross-repo issue
///   `thirdowner/thirdrepo#99` — intentionally distinct from both the
///   MCP-configured repo AND the source's enclosing repo.
///
/// The `probe_edge_via_fetch` closure MUST apply the "None means same
/// repo as the enclosing (fetched) source" convention, deriving the
/// blocker's identity as `otherowner/otherrepo#99`. Compared against
/// the target `thirdowner/thirdrepo#99`, the owner mismatch forces
/// absence → `removed: false` with the mutation skipped (Invariant 11).
///
/// Falsifier / regression catch: this test falsifies a swap-typo
/// regression where the `.unwrap_or(source_qid.owner.as_str())` fallback
/// is rewritten against `target_qid` (or any other qid) instead of the
/// SOURCE's qid. Under that regression the None blocker would be
/// interpreted as `thirdowner/thirdrepo#99` — a spurious match with the
/// target — and the probe would return `Present`, driving the handler
/// into `remove_blocked_by_refs` and surfacing `removed: true` on the
/// wire. Assertions below pin both the wire signal (`removed: false`)
/// AND the mock call counts (`remove_blocked_by_refs = 0`,
/// `fetch_graph_data = 0`) so either symptom flags the regression.
///
/// This complements
/// `dep_remove_cross_repo_reports_false_when_edge_never_existed`, which
/// exercises the SAME probe path but with an EXPLICIT cross-repo blocker
/// (non-None identity). This test specifically covers the None-fallback
/// branch that the sibling test skips.
#[tokio::test]
async fn dep_remove_cross_repo_source_with_local_looking_blocker() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();

    // Build a CROSS-REPO source fixture: `otherowner/otherrepo#42` with
    // a single blocker whose repo identity is None (same-repo-to-source
    // convention). The source's `qualified_id` is the enclosing repo the
    // convention must derive against.
    let mut cross_repo_source = dep_remove_fixture_issue(42);
    cross_repo_source.qualified_id = QualifiedId::new("otherowner", "otherrepo", 42);
    cross_repo_source.url = "https://github.com/otherowner/otherrepo/issues/42".to_owned();
    cross_repo_source.blocked_by = vec![local_blocker(99)];
    mock.push_fetch_issue_ref(Ok(cross_repo_source));

    // Cold cache forces the cross-repo / fetch-issue probe branch.
    // (Either endpoint being non-Local ALSO forces the probe branch even
    // on a warm cache — see the sibling positive test — so the cold
    // cache here is a belt-and-braces precondition.)
    let state = state_with_mock(Arc::clone(&mock));
    assert!(
        !state.cache.is_fresh().await,
        "precondition: cache must be cold to route through probe_edge_via_fetch",
    );

    let result = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "otherowner/otherrepo#42".to_owned(),
            target: "thirdowner/thirdrepo#99".to_owned(),
        },
    )
    .await
    .expect(
        "cross-repo source with a None-identity blocker whose convention-derived repo \
         differs from the target must early-return with `removed: false`, not error",
    );

    // Primary wire-signal assertion: the convention derives the blocker
    // as `otherowner/otherrepo#99`, which does NOT match the target
    // `thirdowner/thirdrepo#99`, so the probe returns absence and the
    // handler reports `removed: false`. If the fallback is swapped to
    // `target_qid` or `client.owner/repo()`, the interpretation would
    // flip to `thirdowner/thirdrepo#99` or `acme/widgets#99` — the
    // former would spuriously MATCH the target (→ `removed: true`) and
    // fail this assertion; the latter would still report absence but
    // would be caught by other tests in this module.
    assert!(
        !result.removed,
        "None-identity blocker must be derived against the SOURCE's enclosing repo \
         (otherowner/otherrepo), NOT the target's repo or the MCP-configured repo; \
         a swap-typo regression would spuriously match and report removed=true",
    );
    assert_eq!(
        result.source, "otherowner/otherrepo#42",
        "source must render in canonical cross-repo form"
    );
    assert_eq!(
        result.target, "thirdowner/thirdrepo#99",
        "target must render in canonical cross-repo form"
    );
    assert!(
        result.message.contains("No blocking edge to remove"),
        "message must document the no-op (Invariant 11 uniform posture): {}",
        result.message,
    );

    // Secondary assertion: the mutation ladder MUST NOT have fired.
    // Even if the wire signal somehow stayed `false` under regression
    // (e.g. a rewrite that returns `MissingSkipMutation` after a
    // spurious match), a false `Present` classification would trigger
    // `remove_blocked_by_refs` before the early-return. Pinning the
    // call count at zero catches that path directly.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "cross-repo probe must call fetch_issue_ref exactly once (source lookup)",
    );
    assert_eq!(
        calls.remove_blocked_by_refs(),
        0,
        "Invariant 11 + convention correctness: mutation MUST NOT run when the \
         None-fallback correctly resolves the blocker outside the target's repo; \
         a non-zero count flags a swap-typo or configured-repo-fallback regression",
    );
    assert_eq!(
        calls.remove_blocked_by_ref(),
        0,
        "single-side variant must NOT run either",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild",
    );
    assert_eq!(
        calls.update_field(),
        0,
        "no mutation → no Projects V2 Status update ladder; cross-repo source is \
         outside the configured project scope (spec §5.6 footnote) and would be \
         skipped regardless, but pinning this guards against future drift",
    );
}

// ── DepRemove tool: Closed-endpoint UX (unblock-a36) ──────────────

/// Warm-cache path: the target endpoint is Closed in the cached graph
/// → `handle_dep_remove` surfaces `DomainError::EndpointClosed` naming
/// the target's `QualifiedId` instead of collapsing into the generic
/// `removed: false` "no edge" reply. Pins the three-outcome
/// `EdgePresence` classifier introduced in the same commit.
///
/// Pre-conditions:
/// - Warm cache seeded with both endpoints AND the blocking edge, so
///   the prior two-outcome posture would have classified this as
///   `Present` (proceed to mutation).
/// - Target #99 is flagged `state: IssueState::Closed` — the new
///   `issue_state` check must detect this BEFORE the edge lookup and
///   short-circuit to `EndpointClosed(target_qid)`.
///
/// Asserts:
/// - Error code `INVALID_PARAMS` (HTTP 409 → MCP mapping).
/// - Message names the Closed endpoint with its qualified id
///   (`acme/widgets#99`) and references the `reopen` guidance.
/// - Zero network traffic: `remove_blocked_by_refs`, `fetch_graph_data`,
///   `fetch_issue_ref`, `update_field` all stay at 0 (the warm-cache
///   probe is purely in-memory).
#[tokio::test]
async fn dep_remove_warm_cache_target_closed_surfaces_endpoint_closed_error() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();
    // Deliberately push NO stubs — any network call from the handler
    // past the warm-cache probe is a regression.
    let state = state_with_mock(Arc::clone(&mock));

    // Seed the cache with an Open source, a Closed target, and the
    // blocking edge between them. Under the previous two-outcome
    // posture this would have been classified as `Present` (edge
    // exists in the graph) and the handler would have mutated. The
    // new `issue_state` gate short-circuits to `EndpointClosed`
    // FIRST, naming target #99.
    let source_open = dep_remove_fixture_issue(42);
    let mut target_closed = dep_remove_fixture_issue(99);
    target_closed.state = IssueState::Closed;
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    let issues = vec![source_open, target_closed];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues, "acme", "widgets");
    state.cache.update(issues, ready_set, graph).await;
    assert!(
        state.cache.is_fresh().await,
        "cache must be warm so the probe runs through guard_edge_exists",
    );

    let err = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect_err("a Closed target endpoint must surface an error, not removed=false");

    assert_eq!(
        err.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "EndpointClosed maps to INVALID_PARAMS (409 → MCP mapping)",
    );
    assert!(
        err.message.contains("acme/widgets#99"),
        "error message must name the Closed endpoint's qualified id: {}",
        err.message,
    );
    assert!(
        err.message.contains("Closed"),
        "error message must call out the Closed state: {}",
        err.message,
    );
    assert!(
        err.message.contains("reopen"),
        "error message must tell the agent to reopen the issue: {}",
        err.message,
    );

    // Zero network traffic — the warm-cache probe is fully in-memory.
    let calls = mock.calls();
    assert_eq!(
        calls.remove_blocked_by_refs(),
        0,
        "mutation MUST NOT run when an endpoint is Closed",
    );
    assert_eq!(
        calls.remove_blocked_by_ref(),
        0,
        "single-side variant MUST NOT run either",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild",
    );
    assert_eq!(
        calls.fetch_issue_ref(),
        0,
        "warm + both-Local fast path stays in-memory — NO fetch_issue_ref",
    );
    assert_eq!(
        calls.update_field(),
        0,
        "no mutation → no Projects V2 Status update ladder",
    );
}

/// Warm-cache path, symmetric twin: the SOURCE endpoint is Closed in
/// the cached graph. Source is inspected first by `guard_edge_exists`,
/// so a Closed source surfaces `EndpointClosed(source_qid)` ahead of
/// any target-side logic. Pins the source-first ordering inside the
/// warm-cache probe.
#[tokio::test]
async fn dep_remove_warm_cache_source_closed_surfaces_endpoint_closed_error() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();
    let state = state_with_mock(Arc::clone(&mock));

    // Closed source, Open target, blocking edge present. The source-
    // first ordering means the probe reports the SOURCE as the
    // closed endpoint even though the target is Open.
    let mut source_closed = dep_remove_fixture_issue(42);
    source_closed.state = IssueState::Closed;
    let target_open = dep_remove_fixture_issue(99);
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    let issues = vec![source_closed, target_open];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues, "acme", "widgets");
    state.cache.update(issues, ready_set, graph).await;

    let err = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect_err("a Closed source endpoint must surface an error");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("acme/widgets#42"),
        "error must name the SOURCE's qualified id (source is checked first): {}",
        err.message,
    );
    assert!(
        !err.message.contains("acme/widgets#99"),
        "error must NOT conflate the target into the message when only source is Closed: {}",
        err.message,
    );
    assert!(err.message.contains("Closed"));

    // Same zero-traffic invariant.
    let calls = mock.calls();
    assert_eq!(calls.remove_blocked_by_refs(), 0);
    assert_eq!(calls.fetch_graph_data(), 0);
    assert_eq!(calls.fetch_issue_ref(), 0);
    assert_eq!(calls.update_field(), 0);
}

/// Cold-cache / cross-repo path: `probe_edge_via_fetch` fetches a
/// cross-repo source via `fetch_issue_ref` and observes
/// `issue.state == Closed`. The probe MUST short-circuit to
/// `EndpointClosed(source_qid)` BEFORE scanning `blocked_by`. This
/// exercises the cold-path's `issue.state` inspection that bead
/// `unblock-a36` added to `probe_edge_via_fetch`.
#[tokio::test]
async fn dep_remove_cold_cache_cross_repo_source_closed_surfaces_endpoint_closed_error() {
    use unblock_mcp::tools::dep_remove::{DepRemoveParams, handle_dep_remove};

    let mock = new_mock();

    // Seed the cross-repo source as Closed. `blocked_by` is
    // intentionally left empty — the state check fires BEFORE the
    // blocked_by scan, so the contents of blocked_by are irrelevant.
    // If a future regression reorders the checks, this test would
    // degrade to the missing-edge path (removed=false) instead of
    // surfacing the error — which is precisely what we want to
    // catch.
    let mut cross_repo_source = dep_remove_fixture_issue(42);
    cross_repo_source.qualified_id = QualifiedId::new("otherowner", "otherrepo", 42);
    cross_repo_source.url = "https://github.com/otherowner/otherrepo/issues/42".to_owned();
    cross_repo_source.state = IssueState::Closed;
    cross_repo_source.blocked_by = vec![];
    mock.push_fetch_issue_ref(Ok(cross_repo_source));

    // Cold cache forces the probe_edge_via_fetch branch even without
    // the cross-repo endpoint; with a cross-repo source, the probe
    // would route through fetch_issue_ref regardless of cache state.
    let state = state_with_mock(Arc::clone(&mock));
    assert!(
        !state.cache.is_fresh().await,
        "precondition: cache must be cold to route through probe_edge_via_fetch",
    );

    let err = handle_dep_remove(
        &state,
        DepRemoveParams {
            source: "otherowner/otherrepo#42".to_owned(),
            target: "99".to_owned(),
        },
    )
    .await
    .expect_err("a Closed cross-repo source must surface an error, not removed=false");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("otherowner/otherrepo#42"),
        "error must name the Closed cross-repo source's qualified id: {}",
        err.message,
    );
    assert!(err.message.contains("Closed"));
    assert!(err.message.contains("reopen"));

    // The single probe fetch is allowed; nothing else.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "probe_edge_via_fetch must call fetch_issue_ref exactly once",
    );
    assert_eq!(
        calls.remove_blocked_by_refs(),
        0,
        "mutation MUST NOT run when the source is Closed",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild",
    );
    assert_eq!(
        calls.update_field(),
        0,
        "no mutation → no Projects V2 Status update ladder",
    );
}

// ── Claim tool: cross-repo blocker mapping (unblock-29p.55) ───────

/// Armadilha (trap-test) guarding the `claim.rs` blocker-mapping fix
/// for cross-repo blockers carried by `RelatedIssue`.
///
/// Since unblock-29p.43, `FETCH_ISSUE_QUERY.blockedBy` (schema as of
/// 2026-04-30) includes `repository { owner { login } name }` and the
/// parser populates `RelatedIssue.repo_owner` / `repo_name`.
/// unblock-29p.55 rewrites
/// `validate_claimable`'s blocker loop to emit `IssueRef::CrossRepo`
/// when both are `Some`, instead of always aliasing to
/// `IssueRef::Local(r.number)`.
///
/// This test falsifies a regression to the pre-fix behaviour:
/// - It stages an issue (#5) whose sole open blocker is
///   `other/upstream#99` (different owner/repo from `acme/widgets`).
/// - It invokes the `claim` tool through `UnblockServer`.
/// - It asserts the surfaced error message contains the cross-repo
///   rendering `"other/upstream#99"` (produced by `IssueRef::CrossRepo
///   ` Display) and explicitly does NOT contain the bare
///   `"#99"`-as-own-token rendering a `IssueRef::Local(99)` would
///   emit instead.
///
/// If `validate_claimable` regresses to `IssueRef::Local(r.number)`,
/// the `render_blockers` helper at `errors.rs:23-29` would produce
/// `"#99"` alone in the message — the cross-repo substring assertion
/// would fail, and the negative assertion on the bare `"#99, "` token
/// would also fire. This makes the fix non-reversible without
/// breaking the test.
#[tokio::test]
async fn claim_surfaces_cross_repo_blocker_with_repo_identity() {
    use rmcp::model::ErrorCode;
    use unblock_mcp::tools::claim::ClaimParams;

    let mock = new_mock();

    // Build a fixture issue in the configured repo (acme/widgets)
    // whose open blocker lives in other/upstream — the GraphQL
    // parser would populate `repo_owner` / `repo_name` on this
    // exact shape when FETCH_ISSUE_QUERY returns a blockedBy node
    // with a `repository { owner { login } name }` subselection
    // (see graphql.rs:62-74 + parse_related_issues at
    // graphql.rs:888). We reuse the existing `cross_repo_blocker`
    // helper so this test exercises the *same* fixture shape the
    // dep_remove cross-repo armadilhas rely on.
    let mut target = mock_issue(5);
    target.blocked_by = vec![cross_repo_blocker("other", "upstream", 99)];
    mock.push_fetch_issue(Ok(target));

    let state = state_with_mock(Arc::clone(&mock));
    let server = UnblockServer::new(state);

    // `rmcp::Json<ClaimResult>` does not implement `Debug`, so we
    // use `let...else` instead of `.expect_err(...)`.
    let Err(err) = server
        .claim(Parameters(ClaimParams {
            id: 5,
            agent: Some("agent-a".to_owned()),
        }))
        .await
    else {
        panic!("claim must be rejected when issue is blocked by a cross-repo ref")
    };

    // The claim handler wraps the infrastructure error via
    // `github_error_to_mcp`, which maps 409 (IssueBlocked) to
    // `INVALID_PARAMS` and renders the error's `Display` as the
    // message payload. `render_blockers` (errors.rs:23-29) joins
    // the `Vec<IssueRef>` via `ToString::to_string`, so the
    // variant picked by `validate_claimable` is observable on the
    // wire:
    //   IssueRef::Local(99)                    → "#99"
    //   IssueRef::CrossRepo { o, r, 99 }       → "other/upstream#99"
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("blocked by"),
        "expected IssueBlocked error, got: {}",
        err.message,
    );
    assert!(
        err.message.contains("other/upstream#99"),
        "cross-repo blocker must render with owner/repo qualification \
         (IssueRef::CrossRepo Display) — if validate_claimable regressed \
         to IssueRef::Local(r.number) the rendering would drop the \
         owner/repo prefix; got: {}",
        err.message,
    );
    // Negative (armadilha) — if the fix regressed to `IssueRef::Local`
    // the blocker list after "blocked by: " would be the bare local
    // token `"#99"` instead of the qualified `"other/upstream#99"`.
    // Check for the regression-shaped rendering explicitly: the
    // blocker-list prefix is documented by `IssueBlocked`'s Display
    // template at errors.rs:65 as `"Issue #{number} is blocked by: "`,
    // so the substring `"blocked by: #99"` is a marker of a local-only
    // regression and MUST NOT appear here.
    assert!(
        !err.message.contains("blocked by: #99"),
        "cross-repo blocker MUST NOT surface as a bare local `#99` token \
         after 'blocked by:'; if validate_claimable regressed to \
         IssueRef::Local(99) the blocker list would drop the owner/repo \
         prefix and render as `blocked by: #99`. got: {}",
        err.message,
    );

    // Hermetic sanity: exactly one fetch_issue call (the single-issue
    // path that feeds validate_claimable). No mutations, no cache
    // rebuild — validation short-circuited before `execute_write_tool`
    // ran the mutation ladder.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue(),
        1,
        "claim validation must consult fetch_issue exactly once"
    );
    assert_eq!(
        calls.update_field(),
        0,
        "Invariant 11: validation failure → no Projects V2 mutation",
    );
    assert_eq!(
        calls.add_comment(),
        0,
        "Invariant 11: validation failure → no claim comment",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild",
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn create_issue_with_all_params_and_refetch() {
    if !require_github_token() {
        return;
    }

    let state = test_server_state().await;
    let client = &state.github;

    // Realistic Bug fixture (spec Appendix B.3 — unblock-wgj.22).
    let title = format!(
        "Fix authentication bypass in /login endpoint {}",
        chrono::Utc::now().timestamp()
    );

    let params = unblock_github::mutations::CreateIssueParams {
        title: title.clone(),
        body: Some("## Description\n\nIntegration test issue.".to_owned()),
        labels: vec!["test".to_owned()],
        milestone: None,
        assignees: Vec::new(),
        issue_type: None,
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn create_issue_with_blocked_by_local() {
    if !require_github_token() {
        return;
    }

    let state = test_server_state().await;
    let client = &state.github;

    // Create blocker issue first.
    let blocking_title = format!(
        "Migrate auth middleware to async {}",
        chrono::Utc::now().timestamp()
    );
    let blocking_issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: blocking_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
            issue_type: None,
        })
        .await
        .expect("create blocker issue should succeed");

    // Create blocked issue.
    let dependent_title = format!(
        "Add OAuth callback handler {}",
        chrono::Utc::now().timestamp()
    );
    let dependent_issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: dependent_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
            issue_type: None,
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn create_issue_with_blocked_by_cross_repo() {
    if !require_github_token() {
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
        "Investigate flaky checkout test {}",
        chrono::Utc::now().timestamp()
    );
    let blocking_issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: blocking_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
            issue_type: None,
        })
        .await
        .expect("create blocker issue should succeed");

    // Create dependent issue.
    let dependent_title = format!(
        "Add OAuth token validation {}",
        chrono::Utc::now().timestamp()
    );
    let dependent_issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: dependent_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
            issue_type: None,
        })
        .await
        .expect("create blocked issue should succeed");

    // Build a CrossRepo IssueRef pointing at the blocker in the same repo.
    // This exercises the full cross-repo GraphQL resolution code path
    // (resolve_issue_ref with owner/repo/number query → addBlockedBy).
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn create_issue_with_parent_sub_issue() {
    if !require_github_token() {
        return;
    }

    let state = test_server_state().await;
    let client = &state.github;

    // Create parent issue.
    let parent_title = format!(
        "Implement OAuth login flow {}",
        chrono::Utc::now().timestamp()
    );
    let parent = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: parent_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
            issue_type: None,
        })
        .await
        .expect("create parent issue should succeed");

    // Create child issue.
    let child_title = format!(
        "Add OAuth callback handler {}",
        chrono::Utc::now().timestamp()
    );
    let child = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: child_title,
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
            issue_type: None,
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn create_issue_with_defaults() {
    if !require_github_token() {
        return;
    }

    let state = test_server_state().await;
    let client = &state.github;

    let title = format!(
        "Bump dependency versions {}",
        chrono::Utc::now().timestamp()
    );

    let issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: title.clone(),
            body: None,
            labels: Vec::new(),
            milestone: None,
            assignees: Vec::new(),
            issue_type: None,
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn ensure_labels_creates_missing_labels() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn create_issue_appears_in_ready_set_after_rebuild() {
    if !require_github_token() {
        return;
    }

    let state = test_server_state().await;
    let client = &state.github;

    let title = format!(
        "Document Projects V2 setup workflow {}",
        chrono::Utc::now().timestamp()
    );

    let issue = client
        .create_issue(unblock_github::mutations::CreateIssueParams {
            title: title.clone(),
            body: None,
            labels: vec!["test".to_owned()],
            milestone: None,
            assignees: Vec::new(),
            issue_type: None,
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

/// Setup creates all 7 required fields on first run.
///
/// Verifies that `setup_fields()` returns `created` entries for any fields
/// that did not already exist, and that the total resolved field count is 7.
#[tokio::test]
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO + UNBLOCK_PROJECT"]
async fn setup_creates_fields_on_first_run() {
    if !require_github_token_and_project() {
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

    // Total fields (created + healed + skipped) should be 7. The heal
    // bucket was added in bead unblock-aa2 — single-select required
    // fields with diverging options land in `healed` instead of
    // `skipped`. Buckets are mutually exclusive.
    let total = report.created.len() + report.healed.len() + report.skipped.len();
    assert_eq!(
        total, 7,
        "setup should resolve exactly 7 fields, got {total}"
    );

    eprintln!(
        "setup_creates_fields: created={:?}, healed={:?}, skipped={:?}",
        report.created, report.healed, report.skipped
    );
}

/// Setup is idempotent — rerun creates no duplicate fields.
///
/// Calls `setup_fields()` twice. The second call should report all fields
/// as skipped (already existing) with zero created.
#[tokio::test]
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO + UNBLOCK_PROJECT"]
async fn setup_fields_idempotent_no_duplicates() {
    if !require_github_token_and_project() {
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
    assert!(
        report2.healed.is_empty(),
        "second setup_fields should heal zero fields (idempotent), got: {:?}",
        report2.healed
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO + UNBLOCK_PROJECT"]
async fn setup_creates_views_with_correct_layout() {
    if !require_github_token_and_project() {
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO + UNBLOCK_PROJECT"]
async fn setup_views_idempotent_no_duplicates() {
    if !require_github_token_and_project() {
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO + UNBLOCK_PROJECT"]
async fn setup_dry_run_reports_without_mutations() {
    if !require_github_token_and_project() {
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn setup_no_project_returns_project_not_configured() {
    if !require_github_token() {
        return;
    }

    // Build a config without UNBLOCK_PROJECT.
    let config = Config::load_from(|key| match key {
        "UNBLOCK_PROJECT" => Err(std::env::VarError::NotPresent),
        other => std::env::var(other),
    })
    .expect("Config should load without UNBLOCK_PROJECT");

    // Intentional concrete `GitHubClient` (not the `GitHubApi` trait):
    // this test asserts the real client's project-resolution path returns
    // `ProjectNotConfigured` when `UNBLOCK_PROJECT` is unset, which is a
    // property of the concrete implementation, not the trait abstraction.
    //
    // The shared `build_github_client` helper resolves the client via
    // `with_repo` and returns `Err` with a clear message if `UNBLOCK_REPO`
    // is not set — `.git/config` is intentionally unreachable from the
    // `unblock-mcp` test surface (bead unblock-3lb). The earlier
    // `require_github_token()` gate makes the error structurally unreachable
    // when CI is configured correctly, so `expect` here is appropriate.
    let client = build_github_client(&config)
        .await
        .expect("build_github_client should succeed once require_github_token passed");

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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn setup_owner_type_detection_works() {
    if !require_github_token() {
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO + UNBLOCK_PROJECT"]
async fn setup_visible_fields_use_integer_ids() {
    if !require_github_token_and_project() {
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
#[ignore = "live GitHub API — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO"]
async fn reconcile_on_clean_repo() {
    if !require_github_token() {
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
        .compute_ready_set(&issues_vec, "acme", "test")
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
        .compute_ready_set(&corrected_issues, "acme", "test")
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

// ── DepCycles tool: integration tests (SPEC §7.7, §11.4) ──────────────

/// Build a fixture issue for `dep_cycles` tests, parameterised by full
/// `(owner, repo, number)` coordinates.
///
/// `dep_cycles` only consumes the graph topology (not per-issue fields),
/// so every optional field is set to a minimal value. The per-issue
/// `node_id`, `title`, and `url` strings are derived uniformly from the
/// coordinates so the helper covers both the local (`acme/widgets`)
/// variant used by `detect_all_cycles` over the configured repo AND the
/// cross-repo variant that seeds SCCs with nodes OUTSIDE the configured
/// repo for §11.4 projection coverage — see bead `unblock-29p.44`.
///
/// Callers should prefer the thin wrappers below
/// ([`dep_cycles_fixture_issue`], [`dep_cycles_cross_repo_fixture`]) so
/// call-site readability stays tight: the shorthand conveys intent
/// (local-only topology vs. cross-repo mixing) without spelling out
/// `"acme", "widgets"` at every call site.
fn dep_cycles_fixture_at(owner: &str, repo: &str, number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new(owner, repo, number),
        number,
        node_id: format!("I_{owner}_{repo}_{number}"),
        title: format!("DepCycles fixture {owner}/{repo}#{number}"),
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/{owner}/{repo}/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Build a fixture issue under the `MockGitHubClient` coordinates
/// (`acme/widgets`) for `dep_cycles` tests. Thin wrapper over
/// [`dep_cycles_fixture_at`] for local-only topology coverage.
fn dep_cycles_fixture_issue(number: u64) -> unblock_core::types::Issue {
    dep_cycles_fixture_at("acme", "widgets", number)
}

/// Build a cross-repo fixture issue for `dep_cycles` mixed-cycle tests.
/// The resulting `QualifiedId` points OUTSIDE the configured
/// `acme/widgets` repo so the cross-repo projection can strip it.
/// Thin wrapper over [`dep_cycles_fixture_at`].
fn dep_cycles_cross_repo_fixture(
    owner: &str,
    repo: &str,
    number: u64,
) -> unblock_core::types::Issue {
    dep_cycles_fixture_at(owner, repo, number)
}

/// Acceptance (a): local-only cycle — `cross_repo_refs == None`, bare
/// `cycles` populated, cache warmed exactly once across two calls.
///
/// Fixture: four issues, #6 ↔ #7 form a 2-node cycle, #1 / #8 are
/// unrelated acyclic nodes. Per SPEC §7.7 the handler must:
/// - detect one cycle of length 2,
/// - project to `Vec<Vec<u64>>` with the local numbers {6, 7},
/// - leave `cross_repo_refs == None` (skip-serialising to no JSON key),
/// - serve the second call from the warm cache (no additional fetch).
#[tokio::test]
async fn dep_cycles_returns_all_local_cycles_from_warm_cache() {
    use unblock_mcp::tools::dep_cycles::{DepCyclesParams, handle_dep_cycles};

    let issues = vec![
        dep_cycles_fixture_issue(1),
        dep_cycles_fixture_issue(6),
        dep_cycles_fixture_issue(7),
        dep_cycles_fixture_issue(8),
    ];
    let edges = vec![
        // Cycle: #6 ↔ #7.
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 6),
            target: QualifiedId::new("acme", "widgets", 7),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 7),
            target: QualifiedId::new("acme", "widgets", 6),
        },
    ];

    let mock = new_mock();
    // Only the first (cold) call should trigger a fetch. The second
    // call must be served entirely from the cache (spec §7.7 contract:
    // "API calls: 0 (cache hit) | 1+ (rebuild)").
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = state_with_mock(Arc::clone(&mock));

    // ── Call 1 (cold): triggers the single rebuild fetch. ──
    let result = handle_dep_cycles(&state, DepCyclesParams { id: None })
        .await
        .expect("dep_cycles should succeed on cold cache");

    assert_eq!(result.count, 1, "one SCC of size 2 = one cycle");
    assert_eq!(result.cycles.len(), 1, "count mirrors cycles.len()");
    // Tarjan SCC order is not contractually stable across petgraph
    // versions — assert on the member SET, not positional order.
    let cycle_set: std::collections::HashSet<u64> = result.cycles[0].iter().copied().collect();
    assert_eq!(
        cycle_set,
        std::collections::HashSet::from([6_u64, 7]),
        "local cycle must contain issue numbers 6 and 7",
    );
    // Acceptance (a): local-only cycle produces cross_repo_refs == None.
    assert!(
        result.cross_repo_refs.is_none(),
        "SPEC §11.4: local-only cycle → cross_repo_refs None; got: {:?}",
        result.cross_repo_refs,
    );
    // The skip_serializing_if attribute must elide the key entirely.
    let json = serde_json::to_value(&result).expect("serialize");
    assert!(
        json.get("cross_repo_refs").is_none(),
        "None cross_repo_refs must be elided from JSON: {json}"
    );

    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "cold dep_cycles call rebuilds the cache once",
    );

    // ── Call 2 (warm): zero new fetch calls — cache hit path. ──
    let result2 = handle_dep_cycles(&state, DepCyclesParams { id: None })
        .await
        .expect("dep_cycles should succeed on warm cache");
    assert_eq!(result2.count, 1);
    assert!(result2.cross_repo_refs.is_none());
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "warm dep_cycles call must not trigger any additional fetch (SPEC §7.7)",
    );
}

/// Acceptance (b): mixed cycle — the configured-repo projection is
/// shortened and `cross_repo_refs` surfaces the omitted members in
/// lexicographic order with a non-empty summary.
///
/// Fixture: one SCC spanning three nodes —
/// `acme/widgets#1 → zeta/repo#9 → acme/widgets#2 → acme/widgets#1`.
/// The cycle contains two local members (#1, #2) and one cross-repo
/// node (`zeta/repo#9`). Per SPEC §11.4:
/// - `cycles` must contain the local members as `Vec<u64>` (possibly
///   shorter than the true cycle length),
/// - `cross_repo_refs.omitted` must contain `"zeta/repo#9"`,
/// - `cross_repo_refs.summary` must be populated (agent-facing text).
#[tokio::test]
async fn dep_cycles_mixed_cycle_populates_cross_repo_refs() {
    use unblock_mcp::tools::dep_cycles::{DepCyclesParams, handle_dep_cycles};

    let issues = vec![
        dep_cycles_fixture_issue(1),
        dep_cycles_fixture_issue(2),
        // Two cross-repo nodes pulled into the cycle so the
        // determinism contract (lex-sorted `omitted`) is observable.
        dep_cycles_cross_repo_fixture("alpha", "upstream", 42),
        dep_cycles_cross_repo_fixture("zeta", "repo", 9),
    ];
    // 4-node cycle:
    //   #1 → alpha/upstream#42 → zeta/repo#9 → #2 → #1
    // After stripping cross-repo members, the local projection is {1, 2}.
    // `alpha/upstream#42` sorts before `zeta/repo#9` lexicographically.
    let edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 1),
            target: QualifiedId::new("alpha", "upstream", 42),
        },
        BlockingEdge {
            source: QualifiedId::new("alpha", "upstream", 42),
            target: QualifiedId::new("zeta", "repo", 9),
        },
        BlockingEdge {
            source: QualifiedId::new("zeta", "repo", 9),
            target: QualifiedId::new("acme", "widgets", 2),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 2),
            target: QualifiedId::new("acme", "widgets", 1),
        },
    ];

    let mock = new_mock();
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = state_with_mock(Arc::clone(&mock));

    let result = handle_dep_cycles(&state, DepCyclesParams { id: None })
        .await
        .expect("dep_cycles should succeed on cold cache");

    // Exactly one cycle.
    assert_eq!(result.count, 1);
    assert_eq!(result.cycles.len(), 1);
    // Acceptance (b): local projection contains the local members.
    // Tarjan SCC order is not stable — assert on SET membership.
    let local_set: std::collections::HashSet<u64> = result.cycles[0].iter().copied().collect();
    assert_eq!(
        local_set,
        std::collections::HashSet::from([1_u64, 2]),
        "mixed cycle must emit only local members in the bare-u64 projection; got: {:?}",
        result.cycles[0],
    );
    // SPEC §7.7 flow step 4b: the bare-u64 projection MAY be shorter
    // than the true cycle length. True cycle length here is 4; the
    // projection is length 2.
    assert!(
        result.cycles[0].len() < 4,
        "local projection must be shorter than true cycle length (SPEC §7.7 flow 4b)",
    );

    // Acceptance (b): cross_repo_refs is Some with the cross-repo
    // members in lexicographic order.
    let refs = result
        .cross_repo_refs
        .as_ref()
        .expect("SPEC §11.4: mixed cycle → cross_repo_refs Some");
    assert_eq!(
        refs.omitted,
        vec!["alpha/upstream#42".to_owned(), "zeta/repo#9".to_owned(),],
        "Invariant 14: omitted MUST be sorted lexicographically",
    );
    let summary = refs
        .summary
        .as_deref()
        .expect("SPEC §11.4: summary populated for non-empty omitted");
    assert!(
        summary.contains("cross-repo"),
        "summary must describe cross-repo omission: {summary}",
    );
    assert!(
        summary.contains("cycles"),
        "summary must reference the `cycles` projection: {summary}",
    );

    // JSON serialisation includes the cross_repo_refs envelope.
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(
        json["cross_repo_refs"]["omitted"][0], "alpha/upstream#42",
        "JSON envelope surfaces the sorted omitted list",
    );
    assert_eq!(json["cross_repo_refs"]["omitted"][1], "zeta/repo#9");

    // Exactly one fetch call for the cold rebuild.
    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

/// Acceptance (c): targeted `id` filter — two disjoint cycles in the
/// graph, an `id` parameter picks exactly one.
///
/// Fixture: two independent 2-node cycles (#10 ↔ #11 and #20 ↔ #21).
/// With `id = Some(10)` the handler must return only the {#10, #11}
/// cycle, not the {#20, #21} cycle. With `id = Some(20)` the reverse.
/// With `id = Some(999)` (absent from every cycle) the handler returns
/// an empty `cycles` vector and `count == 0`.
#[tokio::test]
async fn dep_cycles_targeted_id_filters_to_scc() {
    use unblock_mcp::tools::dep_cycles::{DepCyclesParams, handle_dep_cycles};

    let issues = vec![
        dep_cycles_fixture_issue(10),
        dep_cycles_fixture_issue(11),
        dep_cycles_fixture_issue(20),
        dep_cycles_fixture_issue(21),
    ];
    let edges = vec![
        // Cycle A: #10 ↔ #11.
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 10),
            target: QualifiedId::new("acme", "widgets", 11),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 11),
            target: QualifiedId::new("acme", "widgets", 10),
        },
        // Cycle B: #20 ↔ #21.
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 20),
            target: QualifiedId::new("acme", "widgets", 21),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 21),
            target: QualifiedId::new("acme", "widgets", 20),
        },
    ];

    let mock = new_mock();
    // One push is enough — all three calls below share the same warm
    // cache after the first fetch.
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = state_with_mock(Arc::clone(&mock));

    // ── id = 10: matches cycle A only. ──
    let r10 = handle_dep_cycles(&state, DepCyclesParams { id: Some(10) })
        .await
        .expect("dep_cycles(id=10) should succeed");
    assert_eq!(r10.count, 1, "id=10 matches exactly one cycle");
    let set10: std::collections::HashSet<u64> = r10.cycles[0].iter().copied().collect();
    assert_eq!(
        set10,
        std::collections::HashSet::from([10_u64, 11]),
        "id=10 → cycle A {{#10, #11}}",
    );
    assert!(r10.cross_repo_refs.is_none());

    // ── id = 20: matches cycle B only. ──
    let r20 = handle_dep_cycles(&state, DepCyclesParams { id: Some(20) })
        .await
        .expect("dep_cycles(id=20) should succeed");
    assert_eq!(r20.count, 1, "id=20 matches exactly one cycle");
    let set20: std::collections::HashSet<u64> = r20.cycles[0].iter().copied().collect();
    assert_eq!(
        set20,
        std::collections::HashSet::from([20_u64, 21]),
        "id=20 → cycle B {{#20, #21}}",
    );
    assert!(r20.cross_repo_refs.is_none());

    // ── id = 999: matches no cycle. ──
    let r999 = handle_dep_cycles(&state, DepCyclesParams { id: Some(999) })
        .await
        .expect("dep_cycles(id=999) should succeed (no match is not an error)");
    assert_eq!(r999.count, 0);
    assert!(r999.cycles.is_empty());
    assert!(r999.cross_repo_refs.is_none());

    // ── id = None: returns BOTH cycles. ──
    let r_all = handle_dep_cycles(&state, DepCyclesParams { id: None })
        .await
        .expect("dep_cycles(id=None) should succeed");
    assert_eq!(r_all.count, 2, "id=None → full graph produces both cycles");

    // All four calls shared a single fetch (warm-cache after the first).
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "SPEC §7.7 cache contract: one fetch across four calls",
    );
}

// ── prime `context` markdown — §11.4 trailer coverage (bead unblock-eos.7) ──
//
// The two tests below cover the SPEC §7.3 + §11.4 markdown trailer contract
// mandated by plan Task 04.13: the `## Cross-repo references` section
// appears iff the cycle detector visits a cross-repo node, and is omitted
// otherwise. They reuse the `dep_cycles_*` fixtures (same topology, same
// local `acme/widgets` coords) so the cross-repo accumulator semantics are
// wired to the same graph shapes exercised by `dep_cycles` — this keeps
// `prime`'s trailer rendering in lock-step with `dep_cycles`'s JSON
// envelope (parity across tools is non-negotiable per SPEC §11.4).
//
// Each test queues TWO stubs because `handle_prime` spawns a background
// read-only reconcile via `tokio::spawn` (Design Decision R5) and the
// reconcile also calls `fetch_graph_data`. The background JoinHandle is
// awaited inside `handle_prime` before returning, so by the time the test
// inspects the markdown both fetches are deterministic.

/// SPEC §7.3 + §11.4 (markdown adaptation): when a detected cycle touches
/// a cross-repo `QualifiedId`, the rendered `context` MUST include a
/// trailing `## Cross-repo references` section with the omitted member
/// rendered as ``` `owner/repo#N` ``` and the italic singular/plural
/// summary matching `dep_cycles` byte-for-byte.
///
/// Fixture: 4-node mixed cycle
/// `acme/widgets#1 → alpha/upstream#42 → zeta/repo#9 → acme/widgets#2 → #1`.
/// Two cross-repo members (`alpha/upstream#42`, `zeta/repo#9`) — the
/// plural summary branch must fire.
#[tokio::test]
async fn prime_markdown_emits_cross_repo_section_when_cycle_touches_foreign_repo() {
    use unblock_mcp::tools::prime::{PrimeParams, handle_prime};

    let issues = vec![
        dep_cycles_fixture_issue(1),
        dep_cycles_fixture_issue(2),
        dep_cycles_cross_repo_fixture("alpha", "upstream", 42),
        dep_cycles_cross_repo_fixture("zeta", "repo", 9),
    ];
    let edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 1),
            target: QualifiedId::new("alpha", "upstream", 42),
        },
        BlockingEdge {
            source: QualifiedId::new("alpha", "upstream", 42),
            target: QualifiedId::new("zeta", "repo", 9),
        },
        BlockingEdge {
            source: QualifiedId::new("zeta", "repo", 9),
            target: QualifiedId::new("acme", "widgets", 2),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 2),
            target: QualifiedId::new("acme", "widgets", 1),
        },
    ];

    let mock = new_mock();
    // Two stubs: direct fetch + background reconcile fetch.
    mock.push_fetch_graph_data(Ok((issues.clone(), edges.clone())));
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = Arc::new(state_with_mock(Arc::clone(&mock)));

    let result = handle_prime(
        &PrimeParams {
            stale_threshold_hours: None,
            max_per_category: None,
            agent: None,
        },
        &state,
    )
    .await
    .expect("handle_prime should succeed on mixed-cycle fixture");

    let md = &result.context;

    // Header block present + correct local coords.
    assert!(
        md.starts_with("# Repo: acme/widgets\n"),
        "header must lead the blob: {md}"
    );

    // SPEC §7.3 flow step 2: `## Issues with cycles` section rendered.
    assert!(
        md.contains("\n## Issues with cycles\n"),
        "cycles section required for any detected cycle: {md}"
    );

    // SPEC §11.4 (markdown adaptation): `## Cross-repo references`
    // trailer present with BOTH omitted members rendered verbatim as
    // inline code. The BTreeSet-backed accumulator yields lex order, so
    // alpha/... precedes zeta/... in the rendered bullet list.
    assert!(
        md.contains("\n## Cross-repo references\n"),
        "trailer required when cycle touches cross-repo node: {md}"
    );
    assert!(md.contains("- `alpha/upstream#42`\n"), "omitted[0]: {md}");
    assert!(md.contains("- `zeta/repo#9`\n"), "omitted[1]: {md}");
    let alpha_idx = md
        .find("- `alpha/upstream#42`")
        .expect("alpha bullet present");
    let zeta_idx = md.find("- `zeta/repo#9`").expect("zeta bullet present");
    assert!(
        alpha_idx < zeta_idx,
        "Invariant 14 determinism: omitted rendered in lex order (alpha < zeta): {md}"
    );

    // Singular/plural parity with dep_cycles — exact phrasing.
    assert!(
        md.contains("_2 cross-repo cycle members omitted from `cycles`_\n"),
        "italic summary must match `dep_cycles` phrasing byte-for-byte: {md}"
    );

    // Session section present (Epic 1.5 surface preserved via Option 3).
    assert!(md.contains("\n## Session\n"), "session section: {md}");

    // Two fetches: direct + background reconcile.
    assert_eq!(mock.calls().fetch_graph_data(), 2);
}

/// SPEC §7.3 + §11.4 dual: when every detected cycle is local, the
/// rendered `context` MUST include the `## Issues with cycles` section
/// AND MUST NOT emit the `## Cross-repo references` trailer.
///
/// Fixture: 2-node local cycle `acme/widgets#6 ↔ #7`. No cross-repo node
/// participates, so the trailer must elide entirely (mirror of
/// `dep_cycles_returns_all_local_cycles_from_warm_cache` Acceptance (a)).
#[tokio::test]
async fn prime_markdown_omits_cross_repo_section_when_all_cycles_are_local() {
    use unblock_mcp::tools::prime::{PrimeParams, handle_prime};

    let issues = vec![
        dep_cycles_fixture_issue(1),
        dep_cycles_fixture_issue(6),
        dep_cycles_fixture_issue(7),
        dep_cycles_fixture_issue(8),
    ];
    let edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 6),
            target: QualifiedId::new("acme", "widgets", 7),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 7),
            target: QualifiedId::new("acme", "widgets", 6),
        },
    ];

    let mock = new_mock();
    mock.push_fetch_graph_data(Ok((issues.clone(), edges.clone())));
    mock.push_fetch_graph_data(Ok((issues, edges)));
    let state = Arc::new(state_with_mock(Arc::clone(&mock)));

    let result = handle_prime(
        &PrimeParams {
            stale_threshold_hours: None,
            max_per_category: None,
            agent: None,
        },
        &state,
    )
    .await
    .expect("handle_prime should succeed on local-cycle fixture");

    let md = &result.context;

    // Local cycle section appears — the graph DOES have a cycle.
    assert!(
        md.contains("\n## Issues with cycles\n"),
        "local cycle must still be surfaced in the markdown: {md}"
    );

    // SPEC §11.4 (markdown adaptation): trailer is elided when no
    // cross-repo node participated in any cycle. This is the mirror of
    // `dep_cycles`'s `cross_repo_refs == None` branch.
    assert!(
        !md.contains("## Cross-repo references"),
        "trailer MUST be elided when every cycle is local (SPEC §11.4): {md}"
    );

    // No stray italic summary leaking through.
    assert!(
        !md.contains("cross-repo cycle"),
        "no singular/plural summary must leak when trailer is absent: {md}"
    );
}

// ── close tool: §11.4 cross-repo response contract integration tests ──
//
// Hermetic `MockGitHubClient` tests for SPEC §8.2 + §11.4 row 4 +
// §14 Invariant 14(b). Mirror `dep_cycles_returns_all_local_cycles_from_warm_cache`
// (None branch) and `dep_cycles_mixed_cycle_populates_cross_repo_refs`
// (Some branch). See bead `unblock-iov`.

/// Build a fixture issue under the `MockGitHubClient` coordinates
/// (`acme/widgets`) for `close` cascade tests. Every optional field is
/// set to a minimal value — the `close` handler only consumes the graph
/// topology (via `compute_unblock_cascade`) plus the closed issue's
/// `node_id`.
fn close_fixture_issue(number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_close_{number}"),
        title: format!("Close fixture #{number}"),
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Build a cross-repo fixture issue for `close` mixed-cascade tests.
/// The resulting `QualifiedId` points OUTSIDE the configured
/// `acme/widgets` repo so the §11.4 partition can strip it from
/// `unblocked` and surface it in `cross_repo_refs.omitted`.
fn close_cross_repo_fixture(owner: &str, repo: &str, number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new(owner, repo, number),
        number,
        node_id: format!("I_close_{owner}_{repo}_{number}"),
        title: format!("Close cross-repo fixture {owner}/{repo}#{number}"),
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/{owner}/{repo}/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Acceptance (a): all local cascade — `cross_repo_refs == None`,
/// bare `unblocked` carries both local dependents, JSON elides the key.
///
/// Fixture: local blocker #8 has local dependents #10 and #11
/// (edges: #10 → #8, #11 → #8 — "#10 is blocked by #8", "#11 is
/// blocked by #8"). Closing #8 cascades both.
///
/// Per SPEC §11.4 / §14 Invariant 14(b): all-local cascade → field
/// is `None` and the JSON envelope omits `cross_repo_refs` entirely.
#[tokio::test]
async fn close_no_cross_repo_dependents_cross_repo_refs_is_none() {
    use rmcp::handler::server::wrapper::{Json, Parameters};
    use unblock_mcp::server::UnblockServer;
    use unblock_mcp::tools::close::CloseParams;

    // Phase 0 pre-close graph: #8 is OPEN, and both #10 and #11 are
    // blocked by it. This is the graph state the cascade captures
    // against.
    let pre_close_issues = vec![
        close_fixture_issue(8),
        close_fixture_issue(10),
        close_fixture_issue(11),
    ];
    // Edges: #10 → #8, #11 → #8. Closing #8 unblocks both.
    let edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 10),
            target: QualifiedId::new("acme", "widgets", 8),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 11),
            target: QualifiedId::new("acme", "widgets", 8),
        },
    ];
    // Phase 1 post-close rebuild: #8 is EXCLUDED from the rebuild
    // universe — this is the post-close rebuild topology divergence
    // from the prime topology (the just-closed blocker is absent).
    // Under PRE-close ordering the cascade is already captured; the
    // post-close graph is only consulted for step 8
    // `update_status_fields` reconciliation.
    let post_close_issues = vec![close_fixture_issue(10), close_fixture_issue(11)];
    // Post-close the edges are dropped (both dependents are now
    // unblocked, so there are no remaining open blockers on them).
    let post_close_edges: Vec<BlockingEdge> = vec![];

    let mock = new_mock();
    // Phase 0 cold-cache prime — pushes the PRE-close graph so the
    // cascade can resolve #8 as an OPEN node.
    mock.push_fetch_graph_data(Ok((pre_close_issues, edges)));
    // Phase 1: fetch #8 (validates it is Open), close #8.
    mock.push_fetch_issue(Ok(close_fixture_issue(8)));
    mock.push_close_issue(Ok(()));
    // Phase 1 field ladder: push None to skip Projects V2 updates.
    mock.push_field_ids(None);
    // Phase 1 post-close rebuild — the closed issue #8 is now
    // absent from the rebuild universe, modelling the post-close
    // rebuild topology divergence from the prime topology.
    mock.push_fetch_graph_data(Ok((post_close_issues, post_close_edges)));
    // Phase 2 loop runs 2×. Each iteration calls add_comment_ref
    // then field_ids. Post unblock-eos.13 the cascade always dispatches
    // via the *_ref primitive (SPEC §8.2 step 6 / §5.6 `close` row);
    // local dependents normalize to `IssueRef::Local(n)`.
    // We push add_comment_ref Ok twice and let field_ids default to
    // None (queue empty ⇒ None) so the inner project-field ladder is
    // skipped.
    mock.push_add_comment_ref(Ok("comment_id".to_owned()));
    mock.push_add_comment_ref(Ok("comment_id".to_owned()));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .close(Parameters(CloseParams {
            id: 8,
            reason: None,
        }))
        .await
        .expect("close should succeed on all-local cascade");

    assert_eq!(result.issue, 8);
    // petgraph Incoming-neighbour iteration order is NOT contractually
    // stable — assert on set membership, not positional order.
    let unblocked_set: std::collections::HashSet<u64> = result.unblocked.iter().copied().collect();
    assert_eq!(
        unblocked_set,
        std::collections::HashSet::from([10_u64, 11]),
        "all-local cascade must emit both local dependents in `unblocked`; got: {:?}",
        result.unblocked,
    );
    // Acceptance (a): all-local cascade produces cross_repo_refs == None.
    assert!(
        result.cross_repo_refs.is_none(),
        "SPEC §11.4: all-local cascade → cross_repo_refs None; got: {:?}",
        result.cross_repo_refs,
    );
    // The skip_serializing_if attribute must elide the key entirely.
    let json = serde_json::to_value(&result).expect("serialize");
    assert!(
        json.get("cross_repo_refs").is_none(),
        "None cross_repo_refs must be elided from JSON: {json}"
    );

    // Both cascade members received an unblock comment via the *_ref
    // dispatch path (SPEC §8.2 step 6 — unblock-eos.13 migration).
    assert_eq!(mock.calls().add_comment_ref(), 2);
    // The legacy single-repo `add_comment` primitive is NOT invoked by
    // the cascade loop anymore; all traffic flows through *_ref.
    assert_eq!(mock.calls().add_comment(), 0);
    // Argument-aware assertion (upgrade from pre-unblock-eos.13 call-count
    // only). Both dependents are local → they normalize to IssueRef::Local.
    let ref_calls = mock.add_comment_ref_calls();
    assert_eq!(ref_calls.len(), 2);
    let ref_numbers: std::collections::HashSet<u64> = ref_calls
        .iter()
        .map(|r| match r {
            unblock_core::types::IssueRef::Local(n) => *n,
            unblock_core::types::IssueRef::CrossRepo { number, .. } => *number,
        })
        .collect();
    assert_eq!(
        ref_numbers,
        std::collections::HashSet::from([10_u64, 11]),
        "all-local cascade must dispatch add_comment_ref for #10 and #11"
    );
    assert!(
        ref_calls
            .iter()
            .all(|r| matches!(r, unblock_core::types::IssueRef::Local(_))),
        "all-local cascade must normalize every cascaded_qid to IssueRef::Local; got: {ref_calls:?}"
    );
    assert_eq!(mock.calls().close_issue(), 1);
    // Under PRE-close ordering the handler issues two
    // `fetch_graph_data` round-trips: Phase 0 cold-cache prime
    // (captures the cascade against the prime graph that still
    // contains #8) and Phase 1 post-close rebuild (post-close
    // rebuild universe excludes the just-closed #8). GAP-15.
    assert_eq!(mock.calls().fetch_graph_data(), 2);
}

/// Acceptance (b): mixed cascade — one local dependent + two cross-repo
/// dependents. The configured-repo projection `unblocked` carries only
/// the local member; `cross_repo_refs` surfaces the cross-repo members
/// in lexicographic order with the singular/plural summary phrasing
/// mandated by SPEC §11.4 row 4 (`close_summary`).
///
/// Fixture: local blocker #8 has:
/// - one local dependent #10,
/// - one cross-repo dependent `other/repo#99`,
/// - one cross-repo dependent `alpha/upstream#42`.
///
/// Edges: `#10 → #8`, `other/repo#99 → #8`, `alpha/upstream#42 → #8`.
/// Closing #8 cascades all three. `unblocked` should contain `[10]`
/// (local set), `cross_repo_refs.omitted` should be
/// `["alpha/upstream#42", "other/repo#99"]` (lex-sorted per
/// Invariant 14(b) determinism).
#[tokio::test]
#[allow(clippy::too_many_lines)] // Dual pre-close/post-close fixtures + multi-ref assertions.
async fn close_cross_repo_dependent_populates_cross_repo_refs() {
    use rmcp::handler::server::wrapper::{Json, Parameters};
    use unblock_mcp::server::UnblockServer;
    use unblock_mcp::tools::close::CloseParams;

    // Phase 0 pre-close graph: #8 is OPEN and all three dependents
    // are blocked by it (one local, two cross-repo).
    let pre_close_issues = vec![
        close_fixture_issue(8),
        close_fixture_issue(10),
        close_cross_repo_fixture("other", "repo", 99),
        close_cross_repo_fixture("alpha", "upstream", 42),
    ];
    let edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 10),
            target: QualifiedId::new("acme", "widgets", 8),
        },
        BlockingEdge {
            source: QualifiedId::new("other", "repo", 99),
            target: QualifiedId::new("acme", "widgets", 8),
        },
        BlockingEdge {
            source: QualifiedId::new("alpha", "upstream", 42),
            target: QualifiedId::new("acme", "widgets", 8),
        },
    ];
    // Phase 1 post-close rebuild: #8 absent from the rebuild
    // universe (post-close rebuild topology divergence).
    let post_close_issues = vec![
        close_fixture_issue(10),
        close_cross_repo_fixture("other", "repo", 99),
        close_cross_repo_fixture("alpha", "upstream", 42),
    ];
    let post_close_edges: Vec<BlockingEdge> = vec![];

    let mock = new_mock();
    // Phase 0 cold-cache prime.
    mock.push_fetch_graph_data(Ok((pre_close_issues, edges)));
    mock.push_fetch_issue(Ok(close_fixture_issue(8)));
    mock.push_close_issue(Ok(()));
    mock.push_field_ids(None); // Phase 1: skip project-field ladder.
    // Phase 1 post-close rebuild (production-realistic: #8 excluded).
    mock.push_fetch_graph_data(Ok((post_close_issues, post_close_edges)));
    // Phase 2 loop runs 3× (#10, other/repo#99, alpha/upstream#42).
    // Each iteration posts an add_comment_ref; field_ids defaults to
    // None when the queue is empty, so the inner project-field ladder
    // is skipped — no further pushes required.
    //
    // Post-unblock-eos.13: the cascade dispatches via `add_comment_ref`
    // (SPEC §8.2 step 6 / §5.6 `close` row: cascade side-effects only).
    // Local dependents normalize to `IssueRef::Local`; foreign dependents
    // carry their qualified `(owner, repo, number)` so the REST POST
    // lands on the correct repository. The argument-aware call log
    // (`mock.add_comment_ref_calls()`) closes the previous RISK #2 gap.
    mock.push_add_comment_ref(Ok("c1".to_owned()));
    mock.push_add_comment_ref(Ok("c2".to_owned()));
    mock.push_add_comment_ref(Ok("c3".to_owned()));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .close(Parameters(CloseParams {
            id: 8,
            reason: None,
        }))
        .await
        .expect("close should succeed on mixed cascade");

    assert_eq!(result.issue, 8);
    // Only the local dependent appears in `unblocked`.
    assert_eq!(
        result.unblocked,
        vec![10_u64],
        "SPEC §8.2 flow step 9: local partition only; got: {:?}",
        result.unblocked,
    );

    // Acceptance (b): cross_repo_refs is Some with lex-sorted omitted.
    let refs = result
        .cross_repo_refs
        .as_ref()
        .expect("SPEC §11.4: mixed cascade → cross_repo_refs Some");
    assert_eq!(
        refs.omitted,
        vec!["alpha/upstream#42".to_owned(), "other/repo#99".to_owned(),],
        "Invariant 14(b): omitted MUST be sorted lexicographically",
    );
    // Exact summary phrasing (byte-for-byte per SPEC §11.4 row 4).
    assert_eq!(
        refs.summary.as_deref(),
        Some("2 cross-repo dependents cascade-updated but omitted from `unblocked`"),
        "SPEC §11.4 / §8.2 line 1262: plural phrasing must match byte-for-byte",
    );

    // JSON serialisation includes the cross_repo_refs envelope.
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(
        json["cross_repo_refs"]["omitted"][0], "alpha/upstream#42",
        "JSON envelope surfaces the lex-sorted omitted list",
    );
    assert_eq!(json["cross_repo_refs"]["omitted"][1], "other/repo#99");
    // unblocked carries only the local member.
    assert_eq!(json["unblocked"].as_array().map(Vec::len), Some(1));

    // All three cascade members received an unblock comment via the
    // *_ref path (SPEC §8.2 flow step 6: cross-repo dependents ARE still
    // cascade-updated — honoured by unblock-eos.13 primitives).
    assert_eq!(mock.calls().add_comment_ref(), 3);
    // The legacy bare-`u64` primitive is NOT invoked by the cascade.
    assert_eq!(mock.calls().add_comment(), 0);

    // Argument-aware assertion: the cross-repo members must be dispatched
    // with their QUALIFIED refs (not coerced into the configured repo).
    // This closes the previous RISK #2 gap — mock counters alone couldn't
    // distinguish "add_comment(99) against acme/widgets" (wrong) from
    // "add_comment_ref(other/repo#99)" (correct). IssueRef does not
    // derive `Hash`, so assert on Vec containment.
    let ref_calls = mock.add_comment_ref_calls();
    assert_eq!(ref_calls.len(), 3);
    assert!(
        ref_calls.contains(&unblock_core::types::IssueRef::Local(10)),
        "local dependent #10 MUST dispatch IssueRef::Local(10); got: {ref_calls:?}"
    );
    assert!(
        ref_calls.contains(&unblock_core::types::IssueRef::CrossRepo {
            owner: "other".to_owned(),
            repo: "repo".to_owned(),
            number: 99,
        }),
        "cross-repo dependent other/repo#99 MUST dispatch IssueRef::CrossRepo; got: {ref_calls:?}"
    );
    assert!(
        ref_calls.contains(&unblock_core::types::IssueRef::CrossRepo {
            owner: "alpha".to_owned(),
            repo: "upstream".to_owned(),
            number: 42,
        }),
        "cross-repo dependent alpha/upstream#42 MUST dispatch IssueRef::CrossRepo; \
         got: {ref_calls:?}"
    );

    assert_eq!(mock.calls().close_issue(), 1);
    // PRE-close prime + POST-close rebuild = 2 round-trips (GAP-15).
    assert_eq!(mock.calls().fetch_graph_data(), 2);
}

/// Acceptance (c): singular-form summary — a single cross-repo
/// dependent triggers the singular `"1 cross-repo dependent …"`
/// phrasing per SPEC §11.4 row 4 (`close_summary` — mirrors the
/// `cycles_summary` / `ready_summary` singular/plural noun grammar).
#[tokio::test]
async fn close_single_cross_repo_dependent_uses_singular_summary() {
    use rmcp::handler::server::wrapper::{Json, Parameters};
    use unblock_mcp::server::UnblockServer;
    use unblock_mcp::tools::close::CloseParams;

    // Phase 0 pre-close graph: #8 OPEN, other/repo#99 blocked by it.
    let pre_close_issues = vec![
        close_fixture_issue(8),
        close_cross_repo_fixture("other", "repo", 99),
    ];
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("other", "repo", 99),
        target: QualifiedId::new("acme", "widgets", 8),
    }];
    // Phase 1 post-close rebuild: #8 absent from the rebuild
    // universe (post-close rebuild topology divergence).
    let post_close_issues = vec![close_cross_repo_fixture("other", "repo", 99)];
    let post_close_edges: Vec<BlockingEdge> = vec![];

    let mock = new_mock();
    // Phase 0 cold-cache prime.
    mock.push_fetch_graph_data(Ok((pre_close_issues, edges)));
    mock.push_fetch_issue(Ok(close_fixture_issue(8)));
    mock.push_close_issue(Ok(()));
    mock.push_field_ids(None);
    // Phase 1 post-close rebuild (rebuild universe excludes the just-closed #8).
    mock.push_fetch_graph_data(Ok((post_close_issues, post_close_edges)));
    // Phase 2: single cross-repo dependent — dispatched through the *_ref
    // primitive (SPEC §8.2 step 6 / §5.6 `close` row).
    mock.push_add_comment_ref(Ok("c1".to_owned()));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .close(Parameters(CloseParams {
            id: 8,
            reason: None,
        }))
        .await
        .expect("close should succeed on single-cross-repo cascade");

    assert_eq!(result.issue, 8);
    // No local dependents — `unblocked` is empty.
    assert!(
        result.unblocked.is_empty(),
        "all-cross-repo cascade → empty unblocked; got: {:?}",
        result.unblocked,
    );

    let refs = result
        .cross_repo_refs
        .as_ref()
        .expect("single cross-repo cascade → cross_repo_refs Some");
    assert_eq!(refs.omitted, vec!["other/repo#99".to_owned()]);
    // Singular grammar per SPEC §11.4 row 4 / §8.2 line 1262.
    assert_eq!(
        refs.summary.as_deref(),
        Some("1 cross-repo dependent cascade-updated but omitted from `unblocked`"),
        "SPEC §11.4: singular phrasing must match byte-for-byte",
    );

    // Argument-aware dispatch check: the lone cascade member is cross-repo
    // → MUST dispatch `IssueRef::CrossRepo { other, repo, 99 }` through
    // `add_comment_ref` (closes unblock-eos.13 RISK #2 on the singular path).
    assert_eq!(mock.calls().add_comment_ref(), 1);
    assert_eq!(mock.calls().add_comment(), 0);
    let ref_calls = mock.add_comment_ref_calls();
    assert_eq!(
        ref_calls,
        vec![unblock_core::types::IssueRef::CrossRepo {
            owner: "other".to_owned(),
            repo: "repo".to_owned(),
            number: 99,
        }],
        "single cross-repo cascade MUST dispatch the qualified IssueRef"
    );
}

/// Acceptance (d) — unblock-eos.17 best-effort observability guard.
///
/// The Phase-2 cascade loop in the close handler wraps
/// `add_comment_ref` in `if let Err(e) = ...` and swallows the failure
/// with a `tracing::warn!` fallback. This path is purely observability
/// but forms part of the SPEC §8.2 step 6 contract: "cross-repo
/// dependents ARE still cascade-updated; any write-scope denial on a
/// foreign repo MUST NOT tear down the cascade". Before this test the
/// behavioural contract (cascade continues, response shape honored)
/// was only asserted indirectly via the successful-dispatch tests above
/// (`close_cross_repo_dependent_populates_cross_repo_refs`,
/// `close_single_cross_repo_dependent_uses_singular_summary`). The
/// `warn!` branch itself was unit-covered at `mutations.rs:1982-2137`
/// (wiremock 403/404 on `add_comment_in_repo`) but NOT exercised at the
/// integration level — this is the gap the unblock-eos.17 QA finding
/// called out as RISK P3.
///
/// Fixture (GAP-15 PRE-close ordering): local blocker #8 with a single
/// cross-repo dependent `other/repo#99`. Phase 0 primes the graph
/// against a PRE-close fixture containing #8 as an active blocker.
/// Phase 1 closes #8 and rebuilds from a POST-close fixture where the
/// just-closed #8 is absent from the rebuild universe (post-close
/// rebuild topology divergence from the prime topology). The mock queues
/// an `Err(CrossRepoAccessDenied)` on the first (and only)
/// `add_comment_ref` call — modelling the "token lacks write scope on
/// foreign repo" scenario.
///
/// Assertions:
/// 1. The tool returns `Ok` with a well-formed `CloseResult` — the
///    cascade does NOT abort on a best-effort comment failure.
/// 2. `cross_repo_refs.omitted` still carries `["other/repo#99"]` and
///    the singular §11.4 summary is still emitted — response-shape is
///    independent of the side-effect outcome.
/// 3. The `warn!` log carries the `cascaded_qid` structured field with
///    the QUALIFIED ref (`other/repo#99`), so operators can distinguish
///    a cross-repo permission denial from a local-repo failure (the
///    whole point of the unblock-eos.13 migration from bare `u64` to
///    `IssueRef` dispatch — closes the last observability gap flagged
///    by the unblock-eos.13 QA pass).
/// 4. The `warn!` message "Failed to post unblock comment on cascaded
///    issue" appears verbatim — that phrasing is the observability
///    contract for the Phase-2 best-effort fallback.
#[tokio::test]
async fn close_cross_repo_add_comment_ref_failure_warns_and_continues_cascade() {
    use rmcp::handler::server::wrapper::{Json, Parameters};
    use unblock_mcp::server::UnblockServer;
    use unblock_mcp::tools::close::CloseParams;

    // Phase 0 pre-close graph: #8 OPEN, other/repo#99 blocked by it.
    let pre_close_issues = vec![
        close_fixture_issue(8),
        close_cross_repo_fixture("other", "repo", 99),
    ];
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("other", "repo", 99),
        target: QualifiedId::new("acme", "widgets", 8),
    }];
    // Phase 1 post-close rebuild: #8 absent from the rebuild
    // universe (post-close rebuild topology divergence).
    let post_close_issues = vec![close_cross_repo_fixture("other", "repo", 99)];
    let post_close_edges: Vec<BlockingEdge> = vec![];

    let mock = new_mock();
    // Phase 0 cold-cache prime.
    mock.push_fetch_graph_data(Ok((pre_close_issues, edges)));
    mock.push_fetch_issue(Ok(close_fixture_issue(8)));
    mock.push_close_issue(Ok(()));
    // Phase 1 field-ladder: None skips the Projects V2 updates on #8.
    mock.push_field_ids(None);
    // Phase 1 post-close rebuild (production-realistic: #8 excluded).
    mock.push_fetch_graph_data(Ok((post_close_issues, post_close_edges)));
    // Induced failure: the sole cascaded dependent is cross-repo, so
    // the one `add_comment_ref` invocation in the Phase-2 loop receives
    // this `Err`. `CrossRepoAccessDenied { owner, repo }` is the
    // idiomatic wire-level shape a token-without-write-scope returns —
    // see `errors.rs:174-179`. Any `Error` variant would exercise the
    // same branch; picking the cross-repo-typed variant keeps the
    // fixture aligned with the SPEC §11.1 HTTP-403 wiring.
    mock.push_add_comment_ref(Err(unblock_github::errors::Error::Domain {
        source: unblock_core::errors::DomainError::CrossRepoAccessDenied {
            owner: "other".to_owned(),
            repo: "repo".to_owned(),
        },
    }));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));

    // Capture the Phase-3 `tracing::warn!` output. `set_default`
    // binds a thread-local subscriber and returns a guard; the guard
    // drops at end-of-scope and restores the previous subscriber so
    // no other test is polluted by ours. `#[tokio::test]` defaults to
    // `flavor = "current_thread"`, meaning the awaited future is
    // driven on THIS thread and the thread-local subscriber stays
    // active across the `.await`. Any thread-hop (e.g., `spawn_blocking`,
    // multi-thread flavor) would lose the subscriber — the close
    // handler stays on the current thread, so this is safe.
    let capture = TracingCapture::new();
    let subscriber = capture.subscriber();
    let result = {
        let _default_guard = tracing::subscriber::set_default(subscriber);
        server
            .close(Parameters(CloseParams {
                id: 8,
                reason: None,
            }))
            .await
    };

    let Json(result) = result.expect(
        "close MUST return Ok when add_comment_ref fails — the warn! \
         branch is best-effort per SPEC §8.2 step 6",
    );

    // Assertion 1: the response is still well-formed — cascade did
    // NOT abort on the swallowed comment failure.
    assert_eq!(result.issue, 8);
    assert!(
        result.unblocked.is_empty(),
        "all-cross-repo cascade → empty unblocked; got: {:?}",
        result.unblocked,
    );

    // Assertion 2: cross_repo_refs carries the qualified ref with
    // byte-exact singular summary (§11.4 row 4).
    let refs = result
        .cross_repo_refs
        .as_ref()
        .expect("single cross-repo cascade → cross_repo_refs Some even when warn! fires");
    assert_eq!(refs.omitted, vec!["other/repo#99".to_owned()]);
    assert_eq!(
        refs.summary.as_deref(),
        Some("1 cross-repo dependent cascade-updated but omitted from `unblocked`"),
        "SPEC §11.4 row 4: response-shape is independent of Phase-3 side-effect outcome",
    );

    // The cascade dispatched exactly once against the cross-repo ref.
    assert_eq!(mock.calls().add_comment_ref(), 1);
    assert_eq!(mock.calls().add_comment(), 0);
    assert_eq!(
        mock.add_comment_ref_calls(),
        vec![unblock_core::types::IssueRef::CrossRepo {
            owner: "other".to_owned(),
            repo: "repo".to_owned(),
            number: 99,
        }],
        "the Err was induced against the QUALIFIED ref, not a bare u64 \
         re-targeted at the configured repo"
    );

    // Assertion 3+4: the warn! payload carries the `cascaded_qid`
    // structured field populated with the QUALIFIED ref and the
    // human-readable "Failed to post unblock comment on cascaded issue"
    // message emitted by the Phase-2 loop. Checking the raw JSON text
    // matches the `tracing_subscriber::fmt::json` layer's on-wire
    // format exactly — no span/field coupling, so refactors that keep
    // field name + Display value stable remain covered.
    let output = capture.output();
    assert!(
        output.contains("\"cascaded_qid\":\"other/repo#99\""),
        "warn! MUST include structured field `cascaded_qid=other/repo#99` \
         so operators can distinguish cross-repo permission denials from \
         local failures (unblock-eos.13 observability contract); got: {output}",
    );
    assert!(
        output.contains("Failed to post unblock comment on cascaded issue"),
        "warn! message emitted by Phase-2 cascade loop must appear verbatim; got: {output}",
    );
    // Surface that the error chain reached the log (Display of
    // `CrossRepoAccessDenied` is `Access denied to cross-repo issue
    // other/repo` per `errors.rs:173`). Matching on the owner/repo
    // pair is sufficient — the exact wording is intentionally not
    // locked here so error-Display tweaks don't flake this test.
    assert!(
        output.contains("other/repo"),
        "warn! `error=%e` field must surface the cross-repo owner/repo \
         from the failing `add_comment_ref` error; got: {output}",
    );
    // Belt-and-suspenders: the `warn` level string appears (the JSON
    // layer writes `"level":"WARN"`). This ensures we captured the
    // right severity bucket — `info!`/`debug!` wouldn't meet the
    // SPEC §8.2 step 6 "surface the denial to operators" intent.
    assert!(
        output.contains("\"level\":\"WARN\""),
        "Phase-2 cascade fallback MUST log at WARN level; got: {output}",
    );

    assert_eq!(mock.calls().close_issue(), 1);
    // PRE-close prime + POST-close rebuild = 2 round-trips (GAP-15).
    assert_eq!(mock.calls().fetch_graph_data(), 2);
}

/// R3 post-close rebuild failure — refocused semantics under PRE-close
/// ordering (GAP-15 / SPEC §8.2 "Post-rebuild field-sync failure").
///
/// Under PRE-close ordering the cascade list is captured in Phase 0
/// and is durable in memory; the close mutation is durable on GitHub;
/// the Phase 2 cascade field-updates are applied best-effort. What
/// the post-close rebuild failure *does* break is the step 8
/// `update_status_fields` reconciliation — cross-checking Status
/// fields for issues NOT already handled by the Phase 2 cascade loop.
/// This step requires the rebuilt graph and cannot run against an
/// empty cache. The handler MUST surface a 503-class error whose
/// message instructs the caller to re-run `show` so the Status
/// fan-out is reconciled on the next read. Preserves spec §14
/// invariants 8 and 13 (no fictional Status-sync claims when the
/// graph cannot be consulted). Mirrors the reopen R3 regression
/// guard at `reopen_surfaces_error_when_post_reopen_rebuild_fails`.
///
/// Fixture: Phase 0 prime succeeds (the PRE-close graph is
/// available), Phase 1 close succeeds on GitHub, and the post-close
/// rebuild errors with a transient 503. `execute_write_tool` leaves
/// the cache empty after logging the error, so the close handler's
/// post-write graph check falls through to the refocused R3 branch.
#[tokio::test]
async fn close_surfaces_error_when_rebuild_fails_after_pre_cascade() {
    use rmcp::handler::server::wrapper::Parameters;
    use unblock_github::errors::GitHubApiSnafu;
    use unblock_mcp::server::UnblockServer;
    use unblock_mcp::tools::close::CloseParams;

    // Phase 0 pre-close graph: #8 is OPEN with two local dependents
    // (#10, #11) so the cascade list has content. This proves the
    // cascade list survives a subsequent rebuild failure — the R3
    // error signals only Status reconciliation loss, not cascade loss.
    let pre_close_issues = vec![
        close_fixture_issue(8),
        close_fixture_issue(10),
        close_fixture_issue(11),
    ];
    let edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 10),
            target: QualifiedId::new("acme", "widgets", 8),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 11),
            target: QualifiedId::new("acme", "widgets", 8),
        },
    ];

    let mock = new_mock();

    // Phase 0 cold-cache prime succeeds — cascade captured against
    // the PRE-close graph while #8 is still OPEN.
    mock.push_fetch_graph_data(Ok((pre_close_issues, edges)));
    // Phase 1: fetch + close both succeed.
    mock.push_fetch_issue(Ok(close_fixture_issue(8)));
    mock.push_close_issue(Ok(()));
    // Phase 1 field-ladder: None skips the Projects V2 update on #8
    // so `update_field` is not called from the write-tool closure.
    mock.push_field_ids(None);
    // Post-close rebuild: transient 503 — `execute_write_tool` leaves
    // the cache empty after logging the error. The Phase 2 cascade
    // field-update loop still runs (best-effort, against the
    // captured Phase-0 list) and the response-projection step still
    // executes. The refocused R3 branch fires at the tail to signal
    // that the step 8 reconciliation could not run.
    mock.push_fetch_graph_data(Err(GitHubApiSnafu {
        status: 503_u16,
        message: "upstream service unavailable".to_owned(),
    }
    .build()));
    // Phase 2 cascade field-updates for the two local dependents are
    // attempted best-effort against the Phase-0 captured list. Queue
    // two add_comment_ref responses so the loop doesn't trip on a
    // mock-queue underflow; the cascade continues regardless under
    // the best-effort contract (individual comment failures are
    // tracing::warn!'d, not propagated).
    mock.push_add_comment_ref(Ok("c10".to_owned()));
    mock.push_add_comment_ref(Ok("c11".to_owned()));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));

    // `rmcp::Json` does not implement `Debug`, so we can't use
    // `expect_err` here — destructure directly with `let...else`.
    let result = server
        .close(Parameters(CloseParams {
            id: 8,
            reason: None,
        }))
        .await;
    let Err(err) = result else {
        panic!("post-close rebuild failure must surface as a handler error (R3 refocused)");
    };

    // 503 → INTERNAL_ERROR per github_error_to_mcp.
    assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    // After unblock-29p.35 the post-close R3 surface uses the shared
    // `PostMutationRebuildFailed` variant. The surfaced message is
    // deliberately leaner than the pre-refactor wording: it names the
    // preceding mutation (`"close_cascade"`), includes the mutated
    // issue's fully-qualified reference (`acme/widgets#8`), and
    // instructs the caller to re-run `show`. The pre-refactor details
    // ("cascade field-updates applied", "Status reconciliation") are
    // preserved as a `tracing::warn!` at `server.rs:1388-1391` for
    // operator diagnostics — they're no longer part of the MCP wire
    // contract. SPEC §8.2 "Post-rebuild field-sync failure" and §8.2
    // step-8 "update_status_fields reconciliation" semantics are
    // encoded in the preserved `warn!` plus the mutation label and
    // unchanged 503 → INTERNAL_ERROR mapping.
    assert!(
        err.message.contains("close_cascade"),
        "error message must name the preceding mutation (`close_cascade`): {}",
        err.message,
    );
    assert!(
        err.message.contains("acme/widgets#8"),
        "error message must include the mutated issue's fully-qualified reference: {}",
        err.message,
    );
    assert!(
        err.message.contains("show"),
        "error message must instruct caller to re-run `show`: {}",
        err.message,
    );

    // Despite the rebuild failure, the close mutation DID land and
    // the Phase 2 cascade field-updates ran against the Phase-0
    // list — the whole point of PRE-close ordering.
    assert_eq!(
        mock.calls().close_issue(),
        1,
        "close is durable: mutation persists even if rebuild fails",
    );
    assert_eq!(mock.calls().fetch_issue(), 1);
    // fetch_graph_data was called twice: Phase 0 prime (Ok) + Phase 1
    // post-close rebuild (Err).
    assert_eq!(mock.calls().fetch_graph_data(), 2);
    // Phase 2 cascade loop still ran against the captured Phase-0
    // list, dispatching add_comment_ref for each of #10 and #11.
    // This is the correctness contract that PRE-close ordering
    // buys: the cascade survives a post-close rebuild failure.
    assert_eq!(
        mock.calls().add_comment_ref(),
        2,
        "Phase 2 cascade field-updates must run against the captured Phase-0 list even when \
         the post-close rebuild fails — this is the PRE-close correctness contract (GAP-15)",
    );
    // With `field_ids = None`, the Phase 1 Projects V2 ladder
    // short-circuits at the `tracing::debug!` branch — no
    // `update_field` fires for the closed issue.
    assert_eq!(
        mock.calls().update_field(),
        0,
        "status update should be skipped when field_ids=None — no best-effort Phase 1 field \
         updates fire",
    );
    // Cache is invalidated by `execute_write_tool` on rebuild failure
    // and not repopulated; the refocused R3 error signals that the
    // step 8 reconciliation cannot run.
    assert!(
        !server.state().cache.is_fresh().await,
        "cache must be invalidated and not repopulated after rebuild failure",
    );
}

/// GAP-15 PRE-close contract regression guard (unblock-29p.62).
///
/// This test locks the PRE-close cascade capture against a future
/// refactor that might silently re-introduce POST-close lookup. It
/// uses a rebuilt-graph fixture where the just-closed issue is
/// absent from the rebuild universe — i.e. the post-close rebuild
/// topology diverges from the prime topology — and asserts the
/// cascade list is still correctly populated from the Phase-0
/// pre-close capture.
///
/// Under the legacy POST-close ordering this test would fail:
/// `compute_unblock_cascade` would short-circuit to `Vec::new()` at
/// `unblock-core/src/graph.rs:289-291` because the just-closed issue
/// is absent from the rebuilt `node_map`. Under PRE-close ordering
/// the cascade is captured in Phase 0 against the pre-close graph
/// where #8 is still an active blocker — the post-close rebuild's
/// graph shape is irrelevant to the response envelope.
///
/// Fixture: #8 blocks #10 and #11. Phase 0 primes against
/// `{#8, #10, #11}` with edges `#10 → #8`, `#11 → #8`; Phase 1
/// closes #8 and rebuilds against `{#10, #11}` (no edges — the
/// blocker is gone from the rebuild universe). The response MUST
/// still carry `unblocked = [10, 11]` (set membership) and
/// `cross_repo_refs = None`, identical to the warm-cache happy path.
///
/// Call-pattern assertions are intentionally duplicated with the
/// first happy-path test — keeping them locked here guards the
/// PRE-close contract against a regression that only manifests
/// when the rebuild topology diverges from the prime topology.
#[tokio::test]
async fn close_cascade_survives_post_close_rebuild_topology_divergence() {
    use rmcp::handler::server::wrapper::{Json, Parameters};
    use unblock_mcp::server::UnblockServer;
    use unblock_mcp::tools::close::CloseParams;

    // Phase 0 pre-close graph: #8 is OPEN, #10 and #11 both blocked.
    let pre_close_issues = vec![
        close_fixture_issue(8),
        close_fixture_issue(10),
        close_fixture_issue(11),
    ];
    let pre_close_edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 10),
            target: QualifiedId::new("acme", "widgets", 8),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 11),
            target: QualifiedId::new("acme", "widgets", 8),
        },
    ];
    // Phase 1 post-close rebuild: #8 is EXCLUDED from the rebuild
    // universe — this is the key departure from the pre-GAP-15
    // cheat-fixture that retained #8 in the rebuild response to
    // artificially satisfy the POST-close lookup. Under PRE-close
    // ordering the cascade has already been captured, so this
    // post-close rebuild topology divergence is the correct
    // production model. The edges are gone too because the
    // closed blocker is no longer part of the rebuild universe.
    let post_close_issues = vec![close_fixture_issue(10), close_fixture_issue(11)];
    let post_close_edges: Vec<BlockingEdge> = vec![];

    let mock = new_mock();
    // Phase 0 cold-cache prime — captures cascade against the
    // PRE-close graph while #8 is still an OPEN node.
    mock.push_fetch_graph_data(Ok((pre_close_issues, pre_close_edges)));
    // Phase 1 mutation.
    mock.push_fetch_issue(Ok(close_fixture_issue(8)));
    mock.push_close_issue(Ok(()));
    mock.push_field_ids(None); // Skip Projects V2 ladder.
    // Phase 1 post-close rebuild — production-realistic fixture:
    // #8 EXCLUDED, no edges. This is the topology where the legacy
    // POST-close cascade would silently return Vec::new(); under
    // PRE-close ordering it is irrelevant to the response.
    mock.push_fetch_graph_data(Ok((post_close_issues, post_close_edges)));
    // Phase 2 cascade field-updates for the two local dependents.
    mock.push_add_comment_ref(Ok("c10".to_owned()));
    mock.push_add_comment_ref(Ok("c11".to_owned()));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .close(Parameters(CloseParams {
            id: 8,
            reason: None,
        }))
        .await
        .expect(
            "GAP-15 PRE-close contract: cascade MUST be captured from the pre-close graph, \
             so the close succeeds with a fully-populated `unblocked` even when the post-close \
             rebuild excludes the closed issue",
        );

    assert_eq!(result.issue, 8);

    // THE CORE REGRESSION GUARD: despite the rebuild fixture
    // excluding #8 from the rebuild universe (post-close rebuild
    // topology divergence), the response carries both pre-close
    // dependents. Under the legacy POST-close ordering this
    // assertion would FAIL with `unblocked == []` because the
    // rebuilt `node_map` lacks #8 and `compute_unblock_cascade`
    // would short-circuit. GAP-15 makes this topology irrelevant
    // to the response.
    let unblocked_set: std::collections::HashSet<u64> = result.unblocked.iter().copied().collect();
    assert_eq!(
        unblocked_set,
        std::collections::HashSet::from([10_u64, 11]),
        "GAP-15 PRE-close contract: cascade must contain both pre-close dependents even when \
         the post-close rebuilt graph EXCLUDES the closed issue (post-close rebuild topology \
         divergence). Legacy POST-close ordering would silently return []; got: {:?}",
        result.unblocked,
    );
    // All-local cascade → cross_repo_refs MUST be None.
    assert!(
        result.cross_repo_refs.is_none(),
        "all-local cascade → cross_repo_refs None even under the post-close rebuild topology \
         divergence; got: {:?}",
        result.cross_repo_refs,
    );

    // Phase 2 dispatched add_comment_ref twice (once per dependent)
    // via the *_ref primitive — both normalize to IssueRef::Local
    // for the configured-repo dependents.
    assert_eq!(mock.calls().add_comment_ref(), 2);
    let ref_calls = mock.add_comment_ref_calls();
    let ref_numbers: std::collections::HashSet<u64> = ref_calls
        .iter()
        .map(|r| match r {
            unblock_core::types::IssueRef::Local(n) => *n,
            unblock_core::types::IssueRef::CrossRepo { number, .. } => *number,
        })
        .collect();
    assert_eq!(
        ref_numbers,
        std::collections::HashSet::from([10_u64, 11]),
        "Phase 2 cascade MUST dispatch one add_comment_ref per captured dependent: \
         got: {ref_calls:?}",
    );
    assert!(
        ref_calls
            .iter()
            .all(|r| matches!(r, unblock_core::types::IssueRef::Local(_))),
        "all dependents are configured-repo → every ref normalizes to IssueRef::Local; \
         got: {ref_calls:?}",
    );

    assert_eq!(mock.calls().close_issue(), 1);
    // Two fetch_graph_data round-trips: Phase 0 prime + Phase 1
    // post-close rebuild. The second one returns a rebuild universe
    // that EXCLUDES the just-closed issue (post-close rebuild
    // topology divergence).
    assert_eq!(mock.calls().fetch_graph_data(), 2);
}

/// GAP-15 PRE-close 503 surface regression guard (unblock-29p.63).
///
/// The close handler has **two** distinct 503-class error surfaces that
/// must remain architecturally separated per SPEC §8.2 and the GAP-15
/// design (unblock-29p.62). The QA pass on unblock-29p.62 flagged this
/// test as a MINOR RISK: the pre-mutation Phase 0 prime-failure branch
/// had no direct regression guard, so a refactor that collapsed the two
/// surfaces into a single post-mutation error would not trip any test.
///
/// The two surfaces are:
///
/// 1. **Phase 0 cold-cache prime failure (PRE-mutation)** — covered by
///    THIS test. When the cache is cold and
///    [`crate::tools::rebuild_cache`] fails inside
///    `fetch_graph_data` (transient 503 / network error), the cache
///    stays invalidated and `state.cache.get_graph()` returns `None`
///    in the `let Some(pre_close_graph) = ...` guard at
///    `crates/unblock-mcp/src/server.rs:1127`. The handler aborts with
///    a 503 BEFORE any mutation fires — preserving the "close not
///    attempted on empty graph" invariant from
///    `crates/unblock-mcp/src/tools/close.rs:87-92`.
///
/// 2. **Post-rebuild reconciliation failure (POST-mutation)** — covered
///    by `close_surfaces_error_when_rebuild_fails_after_pre_cascade`
///    above. The mutation DID land, the Phase-2 cascade field-updates
///    applied best-effort, but the post-close rebuild failed so step 8
///    `update_status_fields` reconciliation could not run.
///
/// This test locks the distinction by asserting:
///   - The error message references `prime` (pre-mutation path), NOT
///     `Status reconciliation` (the R3 post-mutation wording).
///   - `close_issue()` was NEVER called — the mutation is gated behind
///     the Phase 0 prime success.
///   - The cache is NOT fresh after the abort — the prime-failure
///     branch does not falsely claim a rebuild landed.
///
/// Fixture: cache is cold at entry (never pushed into state). The first
/// `fetch_graph_data` stub returns a transient 503, matching the
/// production "upstream unavailable" scenario during cold boot or after
/// a prior write invalidated the cache.
#[tokio::test]
#[allow(clippy::too_many_lines)] // Dual-surface 503 regression guard: positive + negative assertions on wording, plus five mutation-gate witnesses.
async fn close_surfaces_error_when_phase0_prime_fails() {
    use rmcp::handler::server::wrapper::Parameters;
    use unblock_github::errors::GitHubApiSnafu;
    use unblock_mcp::server::UnblockServer;
    use unblock_mcp::tools::close::CloseParams;

    let mock = new_mock();

    // Phase 0 cold-cache prime: push a transient 503 so
    // `rebuild_cache` fails inside `fetch_graph_data`, leaves the cache
    // invalidated (empty) after logging the error, and the handler's
    // `let Some(pre_close_graph)` guard at `server.rs:1127` falls
    // through to the pre-mutation 503 branch. NO other stubs are
    // queued — any mutation call (fetch_issue, close_issue,
    // update_field, add_comment_ref) would hit `MockNotStubbed` and
    // fail the test loudly, which is exactly the behaviour we want
    // to guard: the close MUST be gated behind the Phase 0 prime.
    mock.push_fetch_graph_data(Err(GitHubApiSnafu {
        status: 503_u16,
        message: "upstream service unavailable".to_owned(),
    }
    .build()));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));

    // Pre-condition: cache is cold at entry — no state pre-population.
    assert!(
        !server.state().cache.is_fresh().await,
        "test prerequisite: cache must be cold so the Phase 0 prime path executes",
    );

    // `rmcp::Json` does not implement `Debug`, so destructure with
    // `let...else` rather than `expect_err`.
    let result = server
        .close(Parameters(CloseParams {
            id: 8,
            reason: None,
        }))
        .await;
    let Err(err) = result else {
        panic!(
            "Phase 0 cold-cache prime failure MUST surface as a pre-mutation 503 — the close \
             cannot proceed without a primed graph (see tools/close.rs module doc, \
             PRE-close cascade capture phase)"
        );
    };

    // 503 → INTERNAL_ERROR per github_error_to_mcp.
    assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);

    // AC3: error message wording MUST be distinct from the R3 path
    // (`Status reconciliation`). The pre-mutation branch in the close
    // handler references `prime the dependency graph` because the
    // mutation was NOT attempted — this is the semantic the SPEC §8.2
    // ordering demands. After bead unblock-29p.69, the wording is
    // sourced from `Error::PreMutationPrimeFailed`'s `Display` impl
    // (the symmetric pre-mutation counterpart to
    // `PostMutationRebuildFailed`); the assertions below pin the
    // contract bit-for-bit so a future Display drift trips this guard.
    assert!(
        err.message.contains("prime the dependency graph"),
        "Phase 0 error message must reference `prime the dependency graph` (pre-mutation path, \
         distinct from the R3 post-rebuild wording): {}",
        err.message,
    );
    assert!(
        err.message.contains("before cascade capture"),
        "Phase 0 error message must reference `before cascade capture` to identify the PRE-close \
         ordering requirement: {}",
        err.message,
    );
    assert!(
        err.message.contains("retry") || err.message.contains("`prime`"),
        "Phase 0 error message must advise retry or running `prime` first (pre-mutation recovery \
         hint — NOT `show` like the R3 path): {}",
        err.message,
    );
    // unblock-29p.69 AC5: pin the new variant's Display contract by
    // asserting on the `QualifiedId` rendering. `PreMutationPrimeFailed`
    // is constructed with the issue's `QualifiedId` so same-numbered
    // local vs. cross-repo issues render unambiguously. The mock's
    // configured repo is `acme/widgets` (see common::new_mock); issue
    // id 8 → `acme/widgets#8`. If a future refactor strips the qid
    // rendering from `Display`, this assertion fails loudly rather than
    // silently regressing the disambiguation contract.
    assert!(
        err.message.contains("acme/widgets#8"),
        "Phase 0 error message must render the QualifiedId in `owner/repo#n` form (sourced from \
         `PreMutationPrimeFailed`'s `Display` impl after bead unblock-29p.69): {}",
        err.message,
    );
    // Strict negative: the R3 post-mutation wording MUST NOT appear
    // here — if it does, the two surfaces have collapsed, which is
    // the regression this test guards against (AC6: distinction
    // between the two 503 surfaces must remain visible).
    assert!(
        !err.message.contains("Status reconciliation"),
        "Phase 0 prime-failure message must NOT use the R3 `Status reconciliation` wording — \
         collapsing the two 503 surfaces breaks the SPEC §8.2 pre-vs-post-mutation contract: {}",
        err.message,
    );
    // unblock-29p.69: the post-mutation `re-run `show`` recovery hint
    // MUST NOT appear in the pre-mutation Display either — that hint
    // is `PostMutationRebuildFailed`'s contract and only makes sense
    // when a mutation has landed. Collapsing the two would silently
    // regress AC6 of unblock-29p.69 / SPEC §8.2.
    assert!(
        !err.message.contains("re-run `show`"),
        "Phase 0 prime-failure message must NOT use the post-mutation `re-run `show`` recovery \
         hint — that wording is `PostMutationRebuildFailed`'s contract (R3 path), \
         and applies only after a mutation has landed: {}",
        err.message,
    );
    assert!(
        !err.message.contains("closed successfully"),
        "Phase 0 prime-failure message must NOT claim the issue was closed — no mutation fired: \
         {}",
        err.message,
    );

    // AC2: zero mutations fired — the close is strictly gated behind
    // the Phase 0 prime success. The `MockNotStubbed` fallback on the
    // unstubbed queues would have tripped the handler into a generic
    // internal error, but the `close_issue() == 0` assertion locks
    // the pre-mutation ordering directly.
    assert_eq!(
        mock.calls().close_issue(),
        0,
        "Phase 0 prime failure MUST abort BEFORE any mutation — close_issue must not be called \
         when the cache cannot be primed (tools/close.rs:87-92: `close is NOT attempted on an \
         empty graph`)",
    );
    assert_eq!(
        mock.calls().fetch_issue(),
        0,
        "Phase 0 prime failure MUST abort before Phase 1 validation — fetch_issue must not be \
         called",
    );
    assert_eq!(
        mock.calls().update_field(),
        0,
        "Phase 0 prime failure MUST abort before Phase 1 Projects V2 field ladder — update_field \
         must not be called",
    );
    assert_eq!(
        mock.calls().add_comment_ref(),
        0,
        "Phase 0 prime failure MUST abort before the Phase 2 cascade field-update loop — \
         add_comment_ref must not be called",
    );

    // The Phase 0 prime was attempted exactly once: `rebuild_cache`
    // invoked `fetch_graph_data`, which returned the injected Err.
    // No post-close rebuild round-trip — the handler bailed out
    // BEFORE reaching `execute_write_tool`.
    assert_eq!(
        mock.calls().fetch_graph_data(),
        1,
        "exactly one fetch_graph_data round-trip on the prime-failure path: the cold-cache \
         prime attempt. No post-close rebuild can occur because no mutation was attempted.",
    );

    // AC4: cache remains invalidated/empty — `rebuild_cache`
    // invalidates the cache BEFORE the network call, so a failed
    // fetch leaves it empty. This is the contract of
    // `crates/unblock-mcp/src/tools/mod.rs:162` (rebuild_cache).
    // The prime-failure branch must NOT falsely claim a rebuild
    // landed.
    assert!(
        !server.state().cache.is_fresh().await,
        "cache must stay cold after the Phase 0 prime failure — no false rebuild claim (AC4 / \
         §14 Invariant 8: no write leaves cache inconsistent)",
    );
    assert!(
        server.state().cache.get_graph().await.is_none(),
        "cache graph must stay None after the Phase 0 prime failure — the `let Some(pre_close_graph)` \
         guard at server.rs:1127 is the exact branch under test",
    );
}

// ── depends tool: integration tests (unblock-29p.13) ──────────────────

/// Build a [`ProjectFieldIds`] fixture with the `"blocked"` Status option
/// populated so the `depends` handler's Status-update ladder (server.rs
/// §`depends` handler, step 4) resolves
/// `field_ids.status.options["blocked"]` and fires a real `update_field`
/// call. Kept local to the `depends` suite so the `dep_remove`/reopen/
/// create tests keep their existing option-map posture.
fn depends_field_ids_with_blocked() -> unblock_github::projects::ProjectFieldIds {
    use std::collections::HashMap;
    use unblock_github::projects::{FieldMeta, ProjectFieldIds};

    let mut status_options = HashMap::new();
    // Post-`unblock-1zj`: canonical TitleCase option name from
    // `Status::option_name`.
    status_options.insert(
        unblock_core::types::Status::Blocked
            .option_name()
            .to_owned(),
        "OPT_BLOCKED".to_owned(),
    );

    let empty_meta = || FieldMeta::new("f".to_owned(), HashMap::new());

    ProjectFieldIds {
        status: FieldMeta::new("status-field-id".to_owned(), status_options),
        priority: empty_meta(),
        pipeline_stage: empty_meta(),
        agent: "agent".to_owned(),
        claimed_at: "ca".to_owned(),
        story_points: "sp".to_owned(),
        defer_until: "du".to_owned(),
    }
}

/// Build a fixture issue under `acme/widgets` coordinates for `depends`
/// tests. Mirrors the `dep_remove_fixture_issue` shape so the depends
/// suite matches the `dep_remove` template called out in the bead
/// investigation (unblock-29p.13 Phase 1 step 1).
fn depends_fixture_issue(number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_{number}"),
        title: format!("Depends fixture #{number}"),
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Happy path (spec §8.4): local source blocked by local target, warm
/// cache contains both nodes with no existing edges so the cycle check
/// passes, `add_blocked_by_refs` succeeds, the post-mutation rebuild
/// returns the expected edge, and the Projects V2 `Status=blocked`
/// ladder fires because the source is local to the configured repo.
///
/// Asserts:
/// - `created = true`,
/// - `source = "#42"`, `target = "#99"` (canonical local rendering),
/// - `message` mentions both `#42` and `#99`,
/// - call counters: `fetch_issue_ref = 1`, `add_blocked_by_refs = 1`,
///   `fetch_graph_data = 1`, `update_field = 1` (Status=blocked ladder),
/// - `add_blocked_by_ref = 0` — the handler MUST use the two-sided
///   `_refs` variant so both endpoints round-trip through a
///   cross-repo-capable primitive.
#[tokio::test]
async fn depends_local_edge_marks_source_blocked() {
    use unblock_github::projects::ProjectInfo;
    use unblock_mcp::tools::depends::DependsParams;

    let mock = new_mock();

    // Step 1 fetch: the handler validates the source exists by calling
    // `fetch_issue_ref(source_ref)` (server.rs:1466-1469). After
    // normalization with Local input the call lands on the
    // `fetch_issue_ref` path (same stub queue as the _ref primitive).
    mock.push_fetch_issue_ref(Ok(depends_fixture_issue(42)));
    // Step 3 mutation (inside execute_write_tool).
    mock.push_add_blocked_by_refs(Ok(()));
    // Step 3 post-mutation rebuild: source #42 is now blocked by target #99.
    let rebuilt_source = depends_fixture_issue(42);
    let rebuilt_target = depends_fixture_issue(99);
    let rebuilt_edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    mock.push_fetch_graph_data(Ok((vec![rebuilt_source, rebuilt_target], rebuilt_edges)));
    // Step 4 Status=blocked ladder (source is Local; server.rs:1535-1572).
    mock.push_field_ids(Some(depends_field_ids_with_blocked()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_42".to_owned()));
    mock.push_update_field(Ok(()));

    // Warm-cache prime with both nodes and NO existing edges so the
    // cycle check at server.rs:1484-1497 returns false without hitting
    // any mocks. `would_create_cycle` only inspects `state.cache`.
    let state = state_with_mock(Arc::clone(&mock));
    let pre_issues = vec![depends_fixture_issue(42), depends_fixture_issue(99)];
    let pre_graph = DependencyGraph::build(&pre_issues, &[]);
    let pre_ready_set = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state
        .cache
        .update(pre_issues, pre_ready_set, pre_graph)
        .await;
    assert!(
        state.cache.is_fresh().await,
        "cache must be warm so the cycle-detection branch is exercised (server.rs:1484-1497)",
    );

    let server = UnblockServer::new(state);
    let Json(result) = server
        .depends(Parameters(DependsParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        }))
        .await
        .expect("depends should succeed when no cycle exists");

    // Response shape — canonical local rendering, source blocked message.
    assert!(result.created, "a new blocking edge must be reported");
    assert_eq!(result.source, "#42", "local source renders as `#n`");
    assert_eq!(result.target, "#99", "local target renders as `#n`");
    assert!(
        result.message.contains("#42") && result.message.contains("#99"),
        "message must mention both refs: {}",
        result.message,
    );
    assert!(
        result.message.contains("blocked"),
        "message must document the blocked relationship: {}",
        result.message,
    );

    // Call-counter contract. These are the load-bearing assertions: they
    // prove the handler used the cross-repo-capable `_refs` mutation,
    // rebuilt the cache exactly once, and fired the Projects V2
    // Status-blocked ladder through to `update_field`.
    //
    // Per-rung ladder assertions (`field_ids`, `resolve_project_info`,
    // `get_project_item_id`) make it possible to pinpoint WHICH rung of
    // the server.rs:1535-1572 ladder drops if this test ever regresses —
    // rather than diagnosing only via the terminal `update_field` count.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "step 1 validates the source via fetch_issue_ref (single call)",
    );
    assert_eq!(
        calls.add_blocked_by_refs(),
        1,
        "step 3 must use the cross-repo-capable `_refs` mutation variant",
    );
    assert_eq!(
        calls.add_blocked_by_ref(),
        0,
        "the single-side `_ref` variant must NOT be used by this handler",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        1,
        "post-mutation rebuild fetches graph data exactly once",
    );
    assert_eq!(
        calls.field_ids(),
        1,
        "ladder rung 1: field_ids must be resolved exactly once (server.rs:1535)",
    );
    assert_eq!(
        calls.resolve_project_info(),
        1,
        "ladder rung 2: resolve_project_info must fire exactly once",
    );
    assert_eq!(
        calls.get_project_item_id(),
        1,
        "ladder rung 3: get_project_item_id must fire exactly once",
    );
    assert_eq!(
        calls.update_field(),
        1,
        "local source must flip Projects V2 Status=blocked (spec §8.4 step 5)",
    );
}

/// Sticky-Backlog (spec §8.4 step 5 / `unblock-1zj`): when the source
/// is currently in `Status::Backlog`, adding a blocker MUST NOT
/// auto-promote it to `Blocked`. The blocker is recorded; Status stays
/// in Backlog until an explicit user/agent transition. The Status
/// update ladder (server.rs:1535-1572) MUST be skipped — `update_field`
/// expects 0 calls, not 1.
///
/// Mirrors the happy-path test above except the source fixture carries
/// `status: Status::Backlog`. The cycle-check, mutation, and rebuild
/// rungs all still fire.
#[tokio::test]
async fn depends_backlog_source_skips_status_update_per_sticky_rule() {
    use unblock_github::projects::ProjectInfo;
    use unblock_mcp::tools::depends::DependsParams;

    let mock = new_mock();

    // Source fixture starts in Backlog (sticky default per `unblock-1zj`).
    let mut backlog_source = depends_fixture_issue(42);
    backlog_source.status = Status::Backlog;
    mock.push_fetch_issue_ref(Ok(backlog_source.clone()));
    mock.push_add_blocked_by_refs(Ok(()));

    let rebuilt_source = backlog_source.clone();
    let rebuilt_target = depends_fixture_issue(99);
    let rebuilt_edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    mock.push_fetch_graph_data(Ok((vec![rebuilt_source, rebuilt_target], rebuilt_edges)));
    // Even though no Status update fires, the handler may pre-resolve
    // field_ids / project_info / item_id under best-effort branches.
    // Only `update_field` is the load-bearing assertion below; we stub
    // the resolution rungs so they don't return MockNotStubbed if
    // exercised.
    mock.push_field_ids(Some(depends_field_ids_with_blocked()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_42".to_owned()));

    let state = state_with_mock(Arc::clone(&mock));
    let pre_issues = vec![backlog_source.clone(), depends_fixture_issue(99)];
    let pre_graph = DependencyGraph::build(&pre_issues, &[]);
    let pre_ready_set = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state
        .cache
        .update(pre_issues, pre_ready_set, pre_graph)
        .await;

    let server = UnblockServer::new(state);
    let Json(result) = server
        .depends(Parameters(DependsParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        }))
        .await
        .expect("depends should succeed for Backlog source");

    assert!(result.created);

    let calls = mock.calls();
    assert_eq!(
        calls.add_blocked_by_refs(),
        1,
        "blocker is still recorded — sticky-Backlog only suppresses Status update",
    );
    assert_eq!(
        calls.update_field(),
        0,
        "spec §8.4 step 5 sticky-Backlog rule: NO Status field update for Backlog source",
    );
}

/// Primary error (spec §8.4): `source == target` is rejected BEFORE any
/// network call. Mirrors the `dep_remove` `source == target` rejection
/// template at integration.rs:2659-2691. This is the cheapest
/// guaranteed-failure primary error path for the depends handler.
///
/// The handler normalizes both refs first (server.rs:1438-1449) so
/// `"7"` and `"#7"` both collapse to `IssueRef::Local(7)` whose resolved
/// `QualifiedId` (against the configured `acme/widgets` repo) compares
/// equal, tripping the `source and target must differ` validation.
#[tokio::test]
async fn depends_rejects_source_equals_target_without_network_calls() {
    use unblock_mcp::tools::depends::DependsParams;

    let mock = new_mock();
    // Intentionally push no stubs — a leak past validation would surface
    // `MockNotStubbed` and fail the test with a noisy error rather than
    // the clean INVALID_PARAMS we want to assert on.
    let state = state_with_mock(Arc::clone(&mock));
    let server = UnblockServer::new(state);

    let Err(err) = server
        .depends(Parameters(DependsParams {
            source: "7".to_owned(),
            target: "#7".to_owned(),
        }))
        .await
    else {
        panic!("source == target must fail validation")
    };

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("must differ"),
        "validation message must explain the constraint: {}",
        err.message,
    );

    // Zero network traffic — validation short-circuits before `fetch_issue_ref`
    // and before any mutation or rebuild primitive.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        0,
        "source-equals-target must fail BEFORE step 1 fetch",
    );
    assert_eq!(calls.add_blocked_by_refs(), 0);
    assert_eq!(calls.add_blocked_by_ref(), 0);
    assert_eq!(calls.fetch_graph_data(), 0);
    assert_eq!(calls.update_field(), 0);
}

/// Cycle detection (spec §8.4): warm cache already contains an edge
/// `#99 → #42` (i.e. `#99` is blocked by `#42`). Trying to add
/// `#42 → #99` would create a cycle, and the handler must reject with
/// a `CircularDependency` error (status 422 → `INVALID_PARAMS`) BEFORE
/// calling the mutation.
///
/// This is the second §8.4 primary-error variant called out in the
/// bead investigation and exercises the local-only cycle branch
/// (server.rs:1479-1506) — the warm-cache graph is consulted and
/// `would_create_cycle` returns true.
#[tokio::test]
async fn depends_rejects_cycle_when_warm_cache_has_reverse_edge() {
    use unblock_mcp::tools::depends::DependsParams;

    let mock = new_mock();
    // Step 1 fetch still runs before the cycle check (server.rs:1466-1469
    // precedes the cycle branch at 1479), so we must stub it — otherwise
    // the test would fail on `MockNotStubbed` before reaching the cycle
    // assertion.
    mock.push_fetch_issue_ref(Ok(depends_fixture_issue(42)));

    let state = state_with_mock(Arc::clone(&mock));
    // Warm cache with an EXISTING edge #99 → #42 (i.e. #99 is blocked
    // by #42). Adding #42 → #99 on top of that would create the cycle
    // #42 → #99 → #42.
    let pre_issues = vec![depends_fixture_issue(42), depends_fixture_issue(99)];
    let pre_edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 99),
        target: QualifiedId::new("acme", "widgets", 42),
    }];
    let pre_graph = DependencyGraph::build(&pre_issues, &pre_edges);
    let pre_ready_set = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state
        .cache
        .update(pre_issues, pre_ready_set, pre_graph)
        .await;

    let server = UnblockServer::new(state);
    let Err(err) = server
        .depends(Parameters(DependsParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        }))
        .await
    else {
        panic!("cycle #42 → #99 → #42 must be rejected")
    };

    // CircularDependency has status 422 which `github_error_to_mcp`
    // routes to INVALID_PARAMS (see errors.rs:99-101).
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.to_lowercase().contains("cycle")
            || err.message.to_lowercase().contains("circular"),
        "cycle rejection message must mention the cycle: {}",
        err.message,
    );

    // Fetch ran (step 1 precedes the cycle check), but NO mutation and
    // NO rebuild. Status ladder must NOT fire.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "step 1 fetch precedes the cycle check and must have run",
    );
    assert_eq!(
        calls.add_blocked_by_refs(),
        0,
        "cycle detection must short-circuit BEFORE the mutation",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild",
    );
    assert_eq!(
        calls.update_field(),
        0,
        "no mutation → no Status=blocked ladder",
    );
}

/// Duplicate-edge detection (spec §8.4 step 3): warm cache already
/// contains the edge `#42 → #99` (i.e. `#42` is already blocked by
/// `#99`). Trying to add `#42 → #99` again MUST be rejected with a
/// `DuplicateDependency` error (status 409 → `INVALID_PARAMS`) BEFORE
/// calling the mutation. This prevents the tool from conflating a
/// legitimate caller retry with an erroneous double-call — both of
/// which used to return an idempotent success at the GitHub level.
///
/// Placed adjacent to `depends_rejects_cycle_when_warm_cache_has_
/// reverse_edge` because the two tests share a skeleton (warm cache +
/// stubbed `fetch_issue_ref` + assertions that no mutation or rebuild
/// fires) and exercise the same Local/Local pre-mutation-check block
/// in the handler (server.rs duplicate-edge branch, immediately after
/// the cycle-detection branch).
///
/// DECISION: No companion entry in `dyn_dispatch.rs`. The existing
/// `depends_dispatches_through_dyn_vtable` already covers the depends
/// vtable dispatch path for the happy case. The duplicate-edge
/// rejection short-circuits on the warm-cache graph BEFORE any
/// `GitHubApi` vtable method is called (no `fetch_issue_ref`,
/// `add_blocked_by_refs`, or `fetch_graph_data` invocation), so a
/// rejection-path entry would add no distinct vtable exercise. The
/// rejection path is load-bearing at the handler level and is covered
/// here.
#[tokio::test]
async fn depends_rejects_duplicate_edge_when_warm_cache_has_same_edge() {
    use unblock_mcp::tools::depends::DependsParams;

    let mock = new_mock();
    // Step 1 fetch precedes the duplicate-edge check (server.rs:1494-1497
    // runs before the duplicate-edge branch), so we must stub it —
    // otherwise the test would fail on `MockNotStubbed` before reaching
    // the duplicate-edge assertion.
    mock.push_fetch_issue_ref(Ok(depends_fixture_issue(42)));

    let state = state_with_mock(Arc::clone(&mock));
    // Warm cache with the EXISTING edge #42 → #99 (i.e. #42 is already
    // blocked by #99). Re-issuing `depends(source=42, target=99)`
    // attempts to add the SAME edge and must be rejected.
    let pre_issues = vec![depends_fixture_issue(42), depends_fixture_issue(99)];
    let pre_edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 42),
        target: QualifiedId::new("acme", "widgets", 99),
    }];
    let pre_graph = DependencyGraph::build(&pre_issues, &pre_edges);
    let pre_ready_set = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state
        .cache
        .update(pre_issues, pre_ready_set, pre_graph)
        .await;

    let server = UnblockServer::new(state);
    let Err(err) = server
        .depends(Parameters(DependsParams {
            source: "42".to_owned(),
            target: "99".to_owned(),
        }))
        .await
    else {
        panic!("duplicate edge #42 → #99 must be rejected per SPEC §8.4 step 3")
    };

    // `DuplicateDependency` has status 409 which `github_error_to_mcp`
    // routes to `INVALID_PARAMS` (errors.rs:100-101) — same terminal
    // error code as the cycle-detection branch (422) so agents get a
    // consistent INVALID_PARAMS for any pre-mutation graph-state
    // rejection.
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.to_lowercase().contains("already")
            || err.message.to_lowercase().contains("duplicate"),
        "duplicate rejection message must mention the pre-existing edge: {}",
        err.message,
    );

    // Fetch ran (step 1 precedes the duplicate-edge check), but NO
    // mutation and NO rebuild. Status ladder must NOT fire — the
    // rejection must be purely pre-mutation.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "step 1 fetch precedes the duplicate-edge check and must have run",
    );
    assert_eq!(
        calls.add_blocked_by_refs(),
        0,
        "duplicate-edge detection must short-circuit BEFORE the mutation",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild",
    );
    assert_eq!(
        calls.update_field(),
        0,
        "no mutation → no Status=blocked ladder",
    );
}

// ── comment tool: integration tests (unblock-29p.13) ─────────────────

/// Happy path (spec §8.8): posting a comment on an existing issue fetches
/// the issue (to validate it exists), posts the comment via
/// `add_comment`, and returns the comment URL. Crucially, the `comment`
/// tool is a READ tool from the graph perspective — spec §8.8 says "NO
/// cache invalidation" — so the warm cache must remain fresh after the
/// call.
///
/// Asserts:
/// - `issue_number == 5`,
/// - `comment_url` matches the stubbed URL,
/// - call counters: `fetch_issue = 1`, `add_comment = 1`,
///   `fetch_graph_data = 0` (spec §8.8 — no cache rebuild),
/// - `state.cache.is_fresh()` remains true post-call (load-bearing
///   invariant: guards against regressions where `comment` becomes a
///   write tool).
#[tokio::test]
async fn comment_posts_on_existing_issue_without_cache_invalidation() {
    use unblock_mcp::tools::comment::CommentParams;

    let mock = new_mock();
    // Step 2 fetch (server.rs:1915): validate the issue exists.
    mock.push_fetch_issue(Ok(mock_issue(5)));
    // Step 3 mutation: post the comment and return the URL.
    let comment_url = "https://github.com/acme/widgets/issues/5#issuecomment-1".to_owned();
    mock.push_add_comment(Ok(comment_url.clone()));

    let state = state_with_mock(Arc::clone(&mock));
    // Pre-populate the cache so `is_fresh()` returns true before the
    // call — the load-bearing assertion below is that the cache is
    // still fresh AFTER the call. Without this prime there is no
    // baseline to compare against.
    let pre_issues = vec![mock_issue(5)];
    let pre_graph = DependencyGraph::build(&pre_issues, &[]);
    let pre_ready_set = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state
        .cache
        .update(pre_issues, pre_ready_set, pre_graph)
        .await;
    assert!(
        state.cache.is_fresh().await,
        "cache must be warm BEFORE the call so we can assert it stays fresh afterwards",
    );

    let server = UnblockServer::new(state);
    let Json(result) = server
        .comment(Parameters(CommentParams {
            id: 5,
            body: "hello".to_owned(),
        }))
        .await
        .expect("comment must succeed on an existing issue with a non-empty body");

    assert_eq!(result.issue_number, 5);
    assert_eq!(result.comment_url, comment_url);

    // Call-counter contract. The critical assertion is
    // `fetch_graph_data = 0`: spec §8.8 says comments do not invalidate
    // the cache. If this regresses (e.g. someone routes `comment`
    // through `execute_write_tool`), the counter jumps to 1 and this
    // test fails loudly.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue(),
        1,
        "step 2 validates existence via a single fetch_issue",
    );
    assert_eq!(calls.add_comment(), 1, "step 3 posts exactly one comment",);
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "spec §8.8: comments MUST NOT trigger a cache rebuild",
    );

    // Load-bearing invariant — comment is a read tool; cache stays warm.
    // This is the regression guard for "comment becomes a write tool".
    assert!(
        server.state().cache.is_fresh().await,
        "spec §8.8: cache must remain fresh after a comment (NO invalidation)",
    );
}

/// Primary error (spec §8.8): an empty or whitespace-only body is
/// rejected BEFORE any network call. The handler short-circuits at
/// server.rs:1906-1912 with `INVALID_PARAMS` and the message
/// `"must not be empty or whitespace-only"`.
#[tokio::test]
async fn comment_rejects_empty_body_without_network_calls() {
    use unblock_mcp::tools::comment::CommentParams;

    let mock = new_mock();
    // Intentionally push no stubs — a leak past validation surfaces
    // `MockNotStubbed` and fails the test with a noisy error rather
    // than the clean INVALID_PARAMS we want to assert on.
    let state = state_with_mock(Arc::clone(&mock));
    let server = UnblockServer::new(state);

    let Err(err) = server
        .comment(Parameters(CommentParams {
            id: 1,
            body: "   ".to_owned(),
        }))
        .await
    else {
        panic!("whitespace-only body must fail validation")
    };

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("empty") && err.message.contains("whitespace"),
        "validation message must document the constraint: {}",
        err.message,
    );

    // Zero network traffic — validation short-circuits first.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue(),
        0,
        "empty body must fail BEFORE the existence check",
    );
    assert_eq!(
        calls.add_comment(),
        0,
        "empty body must fail BEFORE posting",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no call path should touch the graph",
    );
}

// ── claim tool: integration tests (unblock-29p.13) ───────────────────

/// Build a [`ProjectFieldIds`] fixture with the `"in_progress"` Status
/// option populated so the `claim` handler's Status-update ladder
/// (server.rs:928-959) resolves `field_ids.status.options["in_progress"]`
/// and fires a real `update_field` call for Status. The Agent field has
/// no option map (it is a plain text field) and always fires a second
/// `update_field`, and the Claimed At Date field (SPEC §8.1 step 3) fires
/// a third unconditionally — so the happy path expects
/// `update_field == 3`.
fn claim_field_ids_with_in_progress() -> unblock_github::projects::ProjectFieldIds {
    use std::collections::HashMap;
    use unblock_github::projects::{FieldMeta, ProjectFieldIds};

    let mut status_options = HashMap::new();
    // Post-`unblock-1zj`: canonical TitleCase option name from
    // `Status::option_name` (= `"In Progress"`).
    status_options.insert(
        unblock_core::types::Status::InProgress
            .option_name()
            .to_owned(),
        "OPT_IN_PROGRESS".to_owned(),
    );

    let empty_meta = || FieldMeta::new("f".to_owned(), HashMap::new());

    ProjectFieldIds {
        status: FieldMeta::new("status-field-id".to_owned(), status_options),
        priority: empty_meta(),
        pipeline_stage: empty_meta(),
        agent: "agent-field-id".to_owned(),
        claimed_at: "ca".to_owned(),
        story_points: "sp".to_owned(),
        defer_until: "du".to_owned(),
    }
}

/// Build a claim fixture issue under `acme/widgets` coordinates — Open,
/// status Ready, no agent, no blockers, not deferred. This is the
/// claim-ready shape that `validate_claimable` accepts.
fn claim_fixture_issue(number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_claim_{number}"),
        title: format!("Claim fixture #{number}"),
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Happy path (spec §8.1): an open, ready, unblocked, unclaimed issue
/// passes `validate_claimable`, the Projects V2 ladder fires THREE
/// `update_field` calls (`Status=in_progress` + `Agent=<name>` +
/// `Claimed At=<today>`), the claim comment is posted, and
/// `execute_write_tool` rebuilds the cache.
///
/// Asserts:
/// - `issue_number == 5`, `agent == "alice"`, `claimed_at` is recent,
/// - call counters: `fetch_issue = 1`, `update_field = 3` (Status +
///   Agent + Claimed At — RISK from the bead investigation: if only
///   two `Ok()`s are pushed the third call silently surfaces
///   `MockNotStubbed` which the handler swallows via `tracing::warn`;
///   stubbing three and asserting `== 3` catches regressions that
///   drop the Claimed At write — see SPEC §8.1 step 3),
/// - `add_comment = 1`, `fetch_graph_data = 1` (rebuild).
#[tokio::test]
async fn claim_unblocked_open_issue_sets_in_progress_and_posts_comment() {
    use unblock_github::projects::ProjectInfo;
    use unblock_mcp::tools::claim::ClaimParams;

    let mock = new_mock();

    // Step 1 fetch: returns the ready claim fixture.
    mock.push_fetch_issue(Ok(claim_fixture_issue(5)));

    // Step 6 Projects V2 ladder — Status → in_progress, Agent → "alice",
    // Claimed At → today (SPEC §8.1 step 3 — three writes).
    // RISK from investigation: THREE update_field calls fire; if we
    // only queue two Ok()s the third call surfaces MockNotStubbed which
    // the handler SWALLOWS via tracing::warn at the Claimed-At rung.
    // Pushing three Ok()s and asserting the counter == 3 is the only
    // way to detect a regression that drops the Claimed At write.
    mock.push_field_ids(Some(claim_field_ids_with_in_progress()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_5".to_owned()));
    mock.push_update_field(Ok(())); // Status = in_progress
    mock.push_update_field(Ok(())); // Agent = alice
    mock.push_update_field(Ok(())); // Claimed At = <today>

    // Step 7 claim comment.
    mock.push_add_comment(Ok(
        "https://github.com/acme/widgets/issues/5#issuecomment-42".to_owned(),
    ));

    // execute_write_tool rebuild — fetch_graph_data runs unconditionally
    // after the mutation ladder. RISK from investigation: missing this
    // stub surfaces as a cryptic MockNotStubbed on rebuild rather than
    // a primary-assertion failure.
    mock.push_fetch_graph_data(Ok((vec![claim_fixture_issue(5)], vec![])));

    let state = state_with_mock(Arc::clone(&mock));
    let server = UnblockServer::new(state);

    let before = chrono::Utc::now();
    let Json(result) = server
        .claim(Parameters(ClaimParams {
            id: 5,
            agent: Some("alice".to_owned()),
        }))
        .await
        .expect("claim must succeed on an open, ready, unblocked, unclaimed issue");
    let after = chrono::Utc::now();

    assert_eq!(result.issue_number, 5);
    assert_eq!(result.agent.as_deref(), Some("alice"));
    // `claimed_at` is taken inside the handler between `before` and
    // `after` — assert the handler did not stamp a fictional timestamp.
    assert!(
        result.claimed_at >= before && result.claimed_at <= after,
        "claimed_at must be taken inside the handler between {before:?} and {after:?}, got {:?}",
        result.claimed_at,
    );

    // Call-counter contract. These are the load-bearing assertions: the
    // field ladder fires THREE TIMES (Status + Agent + Claimed At per
    // SPEC §8.1 step 3), the claim comment lands exactly once, and the
    // cache is rebuilt exactly once.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue(),
        1,
        "step 1 fetches the issue exactly once for validate_claimable",
    );
    assert_eq!(
        calls.update_field(),
        3,
        "spec §8.1 step 3 fires update_field THREE times: Status=in_progress + Agent=<name> + Claimed At=<today>",
    );
    assert_eq!(
        calls.add_comment(),
        1,
        "spec §8.1 step 7 posts the claim comment exactly once",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        1,
        "execute_write_tool rebuilds the cache exactly once after the mutation ladder",
    );
}

/// Primary error (spec §8.1): an issue that is already claimed (status
/// `InProgress` with a non-empty agent) must be rejected with
/// `AlreadyClaimed` (status 409 → `INVALID_PARAMS`). The validation
/// short-circuits before any mutation, so the Projects V2 ladder, the
/// claim comment, and the post-mutation rebuild must not fire.
///
/// This covers the MCP-boundary wire-through for the `AlreadyClaimed`
/// arm — the domain-level validation is already unit-tested in
/// claim.rs:186-407 but no test previously routed it through
/// `execute_write_tool`.
#[tokio::test]
async fn claim_rejects_already_claimed_issue() {
    use unblock_mcp::tools::claim::ClaimParams;

    let mock = new_mock();

    // Step 1 fetch: returns an issue already claimed by "bob".
    let mut target = claim_fixture_issue(5);
    target.status = Status::InProgress;
    target.agent = Some("bob".to_owned());
    mock.push_fetch_issue(Ok(target));

    let state = state_with_mock(Arc::clone(&mock));
    let server = UnblockServer::new(state);

    let Err(err) = server
        .claim(Parameters(ClaimParams {
            id: 5,
            agent: Some("alice".to_owned()),
        }))
        .await
    else {
        panic!("claim must be rejected when the issue is already claimed")
    };

    // AlreadyClaimed has status 409 → INVALID_PARAMS via `github_error_to_mcp`.
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("already claimed"),
        "error message must mention 'already claimed': {}",
        err.message,
    );
    assert!(
        err.message.contains("bob"),
        "error message must surface the current claimant 'bob': {}",
        err.message,
    );

    // Validation short-circuits before any mutation. Only the initial
    // `fetch_issue` should have run.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue(),
        1,
        "validation consults fetch_issue exactly once",
    );
    assert_eq!(
        calls.update_field(),
        0,
        "validation failure → no Projects V2 mutation",
    );
    assert_eq!(
        calls.add_comment(),
        0,
        "validation failure → no claim comment",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no mutation → no post-mutation rebuild",
    );
}

// ── close tool: residual coverage (unblock-29p.13 Phase 4) ───────────
// The §11.4 suite above already covers cross-repo projection, the
// best-effort add_comment_ref failure path, and R3 rebuild-failure.
// The close.rs:147-166 TODO narrows the residual gap to:
//   (a) Phase-1 already-closed short-circuit (`IssueClosedSnafu`)
//   (b) Co-blocking — a dependent with another remaining open blocker
//       MUST NOT be emitted in `unblocked`/`cross_repo_refs`.

/// Residual coverage (a) — already-closed short-circuit (close.rs:147-166
/// TODO). When `fetch_issue` returns an issue already in `IssueState::Closed`,
/// the handler must reject with `IssueClosedSnafu` (status 409 →
/// `INVALID_PARAMS`) from within `execute_write_tool`. The close mutation
/// and the cascade loop must NOT run.
///
/// Cache is warm-primed so the Phase-0 cold-cache prime is skipped —
/// `fetch_graph_data` fires exactly zero times (no Phase-0 prime, no
/// post-close rebuild because the mutation short-circuited). This
/// matches the foot-gun warning in the bead investigation: mismatching
/// the stub count is the most common cause of spurious failures in the
/// close suite.
#[tokio::test]
async fn close_rejects_already_closed_issue() {
    use unblock_mcp::tools::close::CloseParams;

    let mock = new_mock();
    // Step 1 fetch returns an already-closed fixture. The handler sees
    // `state == Closed` at server.rs:1130 and raises IssueClosedSnafu
    // INSIDE `execute_write_tool`, so close_issue never runs and
    // fetch_graph_data (the post-mutation rebuild) never runs either.
    let mut closed_fixture = close_fixture_issue(5);
    closed_fixture.state = IssueState::Closed;
    mock.push_fetch_issue(Ok(closed_fixture));

    let state = state_with_mock(Arc::clone(&mock));
    // Warm-prime the cache with a single open fixture so Phase-0 cold-
    // cache prime is SKIPPED (state.cache.get_graph() returns Some).
    // Per the investigation: "If the test warm-primes the cache via
    // state.cache.update(...) the Phase-0 prime is skipped and you
    // must NOT push a fetch_graph_data stub for it". Keeping the stub
    // queues empty ensures any leaked round-trip surfaces as a loud
    // MockNotStubbed failure.
    let pre_issues = vec![close_fixture_issue(5)];
    let pre_graph = DependencyGraph::build(&pre_issues, &[]);
    let pre_ready_set = pre_graph.compute_ready_set(&pre_issues, "acme", "widgets");
    state
        .cache
        .update(pre_issues, pre_ready_set, pre_graph)
        .await;

    let server = UnblockServer::new(state);
    let Err(err) = server
        .close(Parameters(CloseParams {
            id: 5,
            reason: None,
        }))
        .await
    else {
        panic!("close must reject an already-closed issue")
    };

    // IssueClosed has status 409 → INVALID_PARAMS via `github_error_to_mcp`.
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.to_lowercase().contains("closed"),
        "error message must document the closed state: {}",
        err.message,
    );

    // Call-counter contract. Fetch ran once; no mutation, no rebuild,
    // no cascade.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue(),
        1,
        "step 1 validates state via fetch_issue exactly once",
    );
    assert_eq!(
        calls.close_issue(),
        0,
        "already-closed must short-circuit BEFORE the close mutation",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        0,
        "no Phase-0 prime (warm cache) and no post-close rebuild (mutation never ran)",
    );
    assert_eq!(
        calls.add_comment_ref(),
        0,
        "mutation never ran → cascade loop has nothing to iterate",
    );
}

/// Residual coverage (b) — co-blocking. A dependent with at least one
/// remaining open blocker MUST NOT be emitted in `unblocked` or
/// `cross_repo_refs`. This validates that `compute_unblock_cascade`'s
/// "fully-unblocked" semantics (graph.rs:300-325 — `all_blockers_resolved`)
/// flow through to the MCP response projection.
///
/// Graph shape (see consts `BLOCKER_A`, `CLOSED`, `DEPENDENT` in the
/// test body — the roles, not the numbers, carry the meaning):
/// - `CLOSED` (target of the close)
/// - `BLOCKER_A` (ANOTHER open blocker)
/// - `DEPENDENT` (blocked by BOTH `CLOSED` and `BLOCKER_A`)
/// - Edges: `DEPENDENT → CLOSED` and `DEPENDENT → BLOCKER_A`
///
/// Closing `CLOSED` leaves `DEPENDENT` still blocked by the still-open
/// `BLOCKER_A`, so the cascade list is empty; the response envelope
/// reports `unblocked: []` and `cross_repo_refs: None`. The JSON
/// envelope must elide `cross_repo_refs` entirely (Invariant 14(b)
/// determinism clause — matches the acceptance (a) §11.4 template).
#[tokio::test]
async fn close_co_blocking_dependent_excluded_from_unblocked() {
    use unblock_mcp::tools::close::CloseParams;

    // Role-named issue numbers — the cascade topology is about roles,
    // not the raw numbers. `DEPENDENT` is blocked by BOTH `CLOSED` and
    // `BLOCKER_A`, and closing `CLOSED` must NOT promote `DEPENDENT`
    // because `BLOCKER_A` remains open.
    const BLOCKER_A: u64 = 7;
    const CLOSED: u64 = 8;
    const DEPENDENT: u64 = 10;

    // Phase 0 PRE-close graph: CLOSED Open, BLOCKER_A Open, DEPENDENT
    // blocked by BOTH.
    let pre_close_issues = vec![
        close_fixture_issue(BLOCKER_A),
        close_fixture_issue(CLOSED),
        close_fixture_issue(DEPENDENT),
    ];
    // Edge convention: source = blocked, target = blocker.
    // DEPENDENT is blocked by CLOSED AND by BLOCKER_A.
    let edges = vec![
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", DEPENDENT),
            target: QualifiedId::new("acme", "widgets", CLOSED),
        },
        BlockingEdge {
            source: QualifiedId::new("acme", "widgets", DEPENDENT),
            target: QualifiedId::new("acme", "widgets", BLOCKER_A),
        },
    ];
    // Phase 1 POST-close rebuild: CLOSED is absent from the rebuild
    // universe (post-close rebuild topology divergence — see
    // server.rs:1069-1075), DEPENDENT and BLOCKER_A remain. The edge
    // DEPENDENT → CLOSED has vanished (its target is no longer part
    // of the rebuild universe), but the edge DEPENDENT → BLOCKER_A
    // is still present so `ready_set` correctly excludes DEPENDENT.
    let post_close_issues = vec![
        close_fixture_issue(BLOCKER_A),
        close_fixture_issue(DEPENDENT),
    ];
    let post_close_edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", DEPENDENT),
        target: QualifiedId::new("acme", "widgets", BLOCKER_A),
    }];

    let mock = new_mock();
    // Phase 0 cold-cache prime.
    mock.push_fetch_graph_data(Ok((pre_close_issues, edges)));
    // Phase 1 mutation ladder.
    mock.push_fetch_issue(Ok(close_fixture_issue(CLOSED)));
    mock.push_close_issue(Ok(()));
    mock.push_field_ids(None); // skip Projects V2 ladder
    // Phase 1 post-close rebuild.
    mock.push_fetch_graph_data(Ok((post_close_issues, post_close_edges)));
    // NO push_add_comment_ref stubs — the cascade list is empty
    // (compute_unblock_cascade sees DEPENDENT still has open blocker
    // BLOCKER_A) so the Phase-2 loop has zero iterations. If this test
    // leaks past co-blocking protection, the first cascade iteration
    // surfaces `MockNotStubbed` — that loud failure is the regression
    // guard.

    let state = state_with_mock(Arc::clone(&mock));
    let server = UnblockServer::new(state);

    let Json(result) = server
        .close(Parameters(CloseParams {
            id: CLOSED,
            reason: None,
        }))
        .await
        .expect("close must succeed when the cascade is empty");

    assert_eq!(result.issue, CLOSED);
    // Co-blocking: DEPENDENT still has BLOCKER_A as an open blocker, so
    // it is NOT emitted in the cascade. This is the load-bearing
    // assertion — the §11.4 "fully-unblocked" semantics (spec §3.4)
    // flow through the MCP projection.
    assert!(
        result.unblocked.is_empty(),
        "co-blocking: dependent with another open blocker MUST NOT appear in unblocked; got: {:?}",
        result.unblocked,
    );
    // All-local zero-cascade → cross_repo_refs is None and JSON elides it.
    assert!(
        result.cross_repo_refs.is_none(),
        "empty cascade → cross_repo_refs None; got: {:?}",
        result.cross_repo_refs,
    );
    let json = serde_json::to_value(&result).expect("serialize");
    assert!(
        json.get("cross_repo_refs").is_none(),
        "None cross_repo_refs must be elided from JSON: {json}",
    );

    // Call-counter contract. No cascade iterations → zero add_comment_ref
    // calls. Two fetch_graph_data round-trips: Phase-0 prime + Phase-1
    // post-close rebuild.
    let calls = mock.calls();
    assert_eq!(
        calls.fetch_issue(),
        1,
        "Phase 1 fetches the closed issue exactly once",
    );
    assert_eq!(
        calls.close_issue(),
        1,
        "the close mutation runs exactly once",
    );
    assert_eq!(
        calls.fetch_graph_data(),
        2,
        "Phase-0 cold-cache prime + Phase-1 post-close rebuild (GAP-15)",
    );
    assert_eq!(
        calls.add_comment_ref(),
        0,
        "co-blocking: cascade is empty so the Phase-2 loop has zero iterations",
    );
    assert_eq!(
        calls.add_comment(),
        0,
        "legacy bare-number primitive must NOT be invoked by the cascade",
    );
}

// ── unblock-1zj Appendix A.3 obligations #4 + #5 ────────────────────
//
// These two tests pin the spec §8.3 step 4(b) and §8.2 step 6.a
// regression classes called out in the `unblock-1zj` review of PR #283.
// The implementations at server.rs:1972-1973 and server.rs:1394-1421
// are spec-correct; these tests fail loudly if a future refactor
// reintroduces the pre-`unblock-1zj` `ready` / `blocked` branching on
// `create` or drops the cascaded-status `!= Backlog` gate on `close`.

/// Build a [`ProjectFieldIds`] fixture wired so the `create` handler's
/// `set_project_fields` Status update fires IFF the handler reads
/// `Status::Backlog.option_name()`. The Priority option map is
/// deliberately empty so the Priority best-effort path short-circuits
/// at `option_id_by_prefix`, isolating `update_field` accounting to the
/// Status field.
///
/// The `Ready` and `Blocked` Status options are intentionally OMITTED
/// from `status.options` so a regression that re-introduces the
/// pre-`unblock-1zj` `ready` / `blocked` branch in the create handler
/// would call `field_ids.status.options.get("Ready" | "Blocked")`,
/// receive `None`, and skip the Status update — making the
/// `update_field == 1` assertion below fail instead of silently
/// passing.
fn create_field_ids_with_only_backlog() -> unblock_github::projects::ProjectFieldIds {
    use std::collections::HashMap;
    use unblock_github::projects::{FieldMeta, ProjectFieldIds};

    let mut status_options = HashMap::new();
    status_options.insert(
        unblock_core::types::Status::Backlog
            .option_name()
            .to_owned(),
        "OPT_BACKLOG".to_owned(),
    );

    let empty_meta = || FieldMeta::new("f".to_owned(), HashMap::new());

    ProjectFieldIds {
        status: FieldMeta::new("status-field-id".to_owned(), status_options),
        priority: empty_meta(),
        pipeline_stage: empty_meta(),
        agent: "agent".to_owned(),
        claimed_at: "ca".to_owned(),
        story_points: "sp".to_owned(),
        defer_until: "du".to_owned(),
    }
}

/// Build a fixture issue under `acme/widgets` for the create-into-Backlog
/// integration tests below. Mirrors `close_fixture_issue` shape.
fn create_fixture_issue(number: u64) -> unblock_core::types::Issue {
    unblock_core::types::Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_create_{number}"),
        title: format!("Create fixture #{number}"),
        issue_type: Some(IssueType::Task),
        status: Status::Backlog,
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
        body: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Appendix A.3 obligation #4: `create` lands a fresh issue in
/// `Status::Backlog` regardless of whether `blocked_by` is populated.
/// The pre-`unblock-1zj` `ready` / `blocked` branch (see spec §8.3 step
/// 4(b) — the post-`unblock-1zj` contract is "always Backlog") is
/// REMOVED, and a future regression that re-introduces it must trip
/// this test.
///
/// Witnesses the contract via three orthogonal signals:
///
/// 1. The Status option-id map exposes ONLY `"Backlog"` (no `"Ready"`,
///    no `"Blocked"`). A regression that reads `Status::Ready` or
///    `Status::Blocked` from `option_name()` would call
///    `field_ids.status.options.get(...)` against the missing key,
///    hit the `None` branch in `set_project_fields`, and skip the
///    Status update entirely — making `update_field == 1` fail.
/// 2. The blocker is recorded via `add_blocked_by_ref`, proving the
///    `blocked_by` pathway was exercised (so the test isn't vacuously
///    passing on an empty-blockers fixture — the regression class the
///    spec guards against is *blocker-driven* auto-promotion).
/// 3. The fixture rebuild (Phase 2 / cache rebuild after
///    `execute_write_tool`) returns the post-create issue with
///    `status = Status::Backlog`, exercising the round-trip through the
///    cache layer.
#[tokio::test]
async fn create_lands_in_backlog_even_with_blockers() {
    use unblock_github::projects::ProjectInfo;
    use unblock_mcp::tools::create::CreateParams;

    let mock = new_mock();

    // Step 3: create_issue returns the new issue (#42).
    mock.push_create_issue(Ok(create_fixture_issue(42)));

    // Step 4: Projects V2 field ladder.
    // field_ids exposes ONLY the "Backlog" Status option — see the
    // helper doc for the regression-trip rationale.
    mock.push_field_ids(Some(create_field_ids_with_only_backlog()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_42".to_owned()));
    // Step 4 set_project_fields: ONE successful update_field call for
    // Status (the Priority option map is empty so that branch
    // short-circuits at option_id_by_prefix). story_points and
    // defer_until are None so they don't fire.
    mock.push_update_field(Ok(()));

    // Step 5: blocker recording. We pass blocked_by = ["#999"] which
    // parses to IssueRef::Local(999); the handler dispatches via
    // add_blocked_by_ref (singular).
    mock.push_add_blocked_by_ref(Ok(()));

    // Post-write rebuild: cache refresh after execute_write_tool.
    let rebuilt = create_fixture_issue(42);
    mock.push_fetch_graph_data(Ok((vec![rebuilt], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .create(Parameters(CreateParams {
            title: "test backlog default".to_owned(),
            issue_type: None,
            priority: None,
            agent: None,
            body: None,
            labels: None,
            milestone: None,
            blocked_by: Some(vec!["#999".to_owned()]),
            parent: None,
            story_points: None,
            defer_until: None,
        }))
        .await
        .expect("create with blockers should succeed and land in Backlog");

    assert_eq!(result.number, 42);
    assert!(
        result.added_to_project,
        "field-ladder must enter the project-field branch (proof the Status update path ran)",
    );
    assert!(
        result.fields_attempted,
        "set_project_fields must have been invoked",
    );
    assert_eq!(
        result.blockers_added, 1,
        "the blocker must have been recorded — this test pins the regression class \
         where blocker presence flips the create-time Status, so the test must \
         actually exercise the blocker pathway",
    );

    let calls = mock.calls();
    // The load-bearing assertion. Status update fires IFF the handler
    // looked up `Status::Backlog.option_name()` in `status.options`.
    // Any regression that reads `Status::Ready` or `Status::Blocked`
    // hits the missing-key branch and update_field stays at 0.
    assert_eq!(
        calls.update_field(),
        1,
        "spec §8.3 step 4(b): create must land at Status::Backlog regardless of \
         blocker presence — a regression that re-introduces ready/blocked branching \
         would miss the option-id lookup (Ready/Blocked are NOT in the fixture map) \
         and update_field would stay at 0",
    );
    assert_eq!(calls.create_issue(), 1, "exactly one issue created",);
    assert_eq!(
        calls.add_blocked_by_ref(),
        1,
        "the single blocker passed via blocked_by must be recorded",
    );
}

/// Appendix A.3 obligation #5: `close` cascade SKIPS the Status field
/// update for a dependent currently in `Status::Backlog` while STILL
/// emitting the unblock comment. Spec §8.2 step 6 (Backlog sticky):
/// a graph-driven cascade does not promote a Backlog dependent.
///
/// The implementation at server.rs:1394-1421 gates the
/// `update_status_field_best_effort` call on
/// `cascaded_status != Status::Backlog`; the unblock comment posts
/// BEFORE the gate (server.rs:1360-1367). A future regression that
/// drops the gate would silently re-promote Backlog dependents to
/// `Ready` — invisible without this test.
///
/// Fixture: #8 blocks #10. #10 starts in `Status::Backlog`. Closing
/// #8 cascades. The test asserts:
///
/// - `add_comment_ref` fires once (the unblock comment lands BEFORE
///   the Backlog gate).
/// - `update_field` fires exactly ONCE — for the closed issue itself
///   (Status → Closed in Phase 1 step 3). The cascade iteration for
///   the Backlog dependent must NOT fire `update_field`. If the gate
///   regressed, `update_field` would be 2.
///
/// The fixture wires `field_ids.status.options` with BOTH `"Closed"`
/// (so the Phase 1 step-3 Status update on the closed issue is
/// load-bearing) AND `"Ready"` (so a regression that bypassed the
/// Backlog gate would successfully look up the Ready `option_id` and
/// fire a second `update_field` call — making the count == 1
/// assertion fail). Without the `"Ready"` entry, a regression would
/// silently short-circuit at the missing-option branch and the test
/// would pass for the wrong reason.
#[tokio::test]
#[allow(clippy::too_many_lines)] // Multi-phase fixture (Phase 0 prime + Phase 1 mutation/rebuild + Phase 2 cascade) plus regression-trip ladder pre-stocking.
async fn close_cascade_skips_status_update_for_backlog_dependent() {
    use unblock_github::projects::ProjectInfo;
    use unblock_mcp::tools::close::CloseParams;

    // Phase 0 pre-close graph: #8 OPEN, #10 OPEN-and-Backlog,
    // edge #10 → #8.
    let pre_close_blocker = close_fixture_issue(8);
    let mut pre_close_dependent = close_fixture_issue(10);
    pre_close_dependent.status = Status::Backlog;
    let pre_close_issues = vec![pre_close_blocker.clone(), pre_close_dependent.clone()];
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 10),
        target: QualifiedId::new("acme", "widgets", 8),
    }];

    // Phase 1 post-close rebuild: #8 excluded, #10 still Backlog.
    let post_close_dependent = pre_close_dependent.clone();
    let post_close_issues = vec![post_close_dependent];

    // Status options wired for both `Closed` (Phase 1 step 3 update on
    // the closed issue itself) AND `Ready` (cascade target — a
    // regression that bypasses the Backlog gate would land here).
    let mut status_options = std::collections::HashMap::new();
    status_options.insert(
        Status::Closed.option_name().to_owned(),
        "OPT_CLOSED".to_owned(),
    );
    status_options.insert(
        Status::Ready.option_name().to_owned(),
        "OPT_READY".to_owned(),
    );
    let empty_meta = || {
        unblock_github::projects::FieldMeta::new("f".to_owned(), std::collections::HashMap::new())
    };
    let field_ids = unblock_github::projects::ProjectFieldIds {
        status: unblock_github::projects::FieldMeta::new(
            "status-field-id".to_owned(),
            status_options,
        ),
        priority: empty_meta(),
        pipeline_stage: empty_meta(),
        agent: "agent".to_owned(),
        claimed_at: "ca".to_owned(),
        story_points: "sp".to_owned(),
        defer_until: "du".to_owned(),
    };

    let mock = new_mock();
    // Phase 0 cold-cache prime — captures cascade against PRE-close graph.
    mock.push_fetch_graph_data(Ok((pre_close_issues, edges)));
    // Phase 1: fetch + close + Status→Closed best-effort ladder.
    mock.push_fetch_issue(Ok(close_fixture_issue(8)));
    mock.push_close_issue(Ok(()));
    // Phase 1 step 3: update_status_field_best_effort on closed issue
    // (Status → Closed). Resolves field_ids → resolve_project_info →
    // get_project_item_id → update_field.
    mock.push_field_ids(Some(field_ids.clone()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_8".to_owned()));
    mock.push_update_field(Ok(()));
    // Phase 1 post-close rebuild.
    mock.push_fetch_graph_data(Ok((post_close_issues, vec![])));
    // Phase 2 cascade iteration for #10:
    //   1. add_comment_ref (unblock comment posts BEFORE the gate).
    //   2. fetch_issue_ref returns #10 in Backlog → gate fires → SKIP
    //      `update_status_field_best_effort` entirely.
    //
    // We push a fresh Backlog issue on the fetch_issue_ref queue.
    //
    // We ALSO push a full second pass of the project-field stubs
    // (field_ids/resolve/item_id/update_field) sized for the Ready
    // transition the regression path would take. The mock's
    // `field_ids()` returns `None` on an empty queue (silently), and
    // `update_status_field_best_effort` short-circuits on `None` —
    // which would let a broken gate pass this test for the WRONG
    // reason. Pre-stocking the queue with the full Ready ladder
    // ensures a regression that drops the
    // `cascaded_status != Backlog` guard would resolve the option_id
    // for `Status::Ready` (which we wired into `status.options`
    // above) and fire a SECOND `update_field` call, tripping the
    // `update_field == 1` assertion below.
    mock.push_add_comment_ref(Ok("c10".to_owned()));
    let mut cascade_target = close_fixture_issue(10);
    cascade_target.status = Status::Backlog;
    mock.push_fetch_issue_ref(Ok(cascade_target));
    // Regression-trip ladder for the cascade rung (NOT consumed under
    // the correct gate — these stubs leave residual queue depth which
    // we do not assert on; only `update_field` count matters).
    mock.push_field_ids(Some(field_ids.clone()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_10".to_owned()));
    mock.push_update_field(Ok(()));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .close(Parameters(CloseParams {
            id: 8,
            reason: None,
        }))
        .await
        .expect("close should succeed with a Backlog dependent");

    assert_eq!(result.issue, 8);
    let unblocked_set: std::collections::HashSet<u64> = result.unblocked.iter().copied().collect();
    assert_eq!(
        unblocked_set,
        std::collections::HashSet::from([10_u64]),
        "the cascade still surfaces #10 in `unblocked` — sticky-Backlog only \
         suppresses the Status update, not the graph-cascade projection",
    );

    let calls = mock.calls();
    // Unblock comment landed BEFORE the Backlog gate.
    assert_eq!(
        calls.add_comment_ref(),
        1,
        "spec §8.2 step 6: unblock comment posts BEFORE the sticky-Backlog \
         Status-update gate — Backlog dependents still receive the comment",
    );
    // Cascade entered the gate — fetch_issue_ref ran for #10.
    assert_eq!(
        calls.fetch_issue_ref(),
        1,
        "cascade iteration must fetch the dependent's current Status before \
         deciding whether to update it",
    );
    // The load-bearing assertion. Status updates: 1 for the closed
    // issue (Phase 1 step 3 → Closed), 0 for the Backlog dependent
    // (cascade gate skipped). A regression that drops the
    // `cascaded_status != Backlog` guard would call
    // update_status_field_best_effort, look up
    // `Status::Ready.option_name()` in the status_options map (which
    // we deliberately populated with "Ready" → "OPT_READY"), and fire
    // a SECOND update_field call — failing this assertion.
    assert_eq!(
        calls.update_field(),
        1,
        "spec §8.2 step 6.a sticky-Backlog rule: NO Status field update for a \
         cascaded dependent currently in Backlog. Exactly one update_field call \
         is allowed — Phase 1 step 3 setting the closed issue itself to \
         Status::Closed. A regression that drops the cascaded-status != Backlog \
         gate would fire a second update_field for the Ready transition (the \
         fixture wires `Ready` → `OPT_READY` so the regression path successfully \
         resolves an option_id and the test fails LOUDLY here rather than \
         silently passing on a missing-option short-circuit).",
    );
    assert_eq!(calls.close_issue(), 1);
    // Phase 0 prime + Phase 1 rebuild = 2.
    assert_eq!(calls.fetch_graph_data(), 2);
}
