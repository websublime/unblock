//! Runtime coverage for the `Arc<dyn GitHubApi>` dispatch layer.
//!
//! These tests back [`ServerState`] with [`MockGitHubClient`] stored as an
//! `Arc<dyn GitHubApi>` trait object and invoke every tool handler on
//! [`UnblockServer`] through that vtable. Each test asserts both the handler
//! result and that the mock's per-method call counter incremented, proving
//! the call traversed the dyn-dispatch layer rather than being monomorphized
//! away.
//!
//! Unlike [`integration.rs`](integration.rs) and
//! [`e2e_workflow.rs`](e2e_workflow.rs), these tests do **not** require a
//! `GITHUB_TOKEN` — they run unconditionally in CI and are the sole runtime
//! signal that `Arc<dyn GitHubApi>` dispatch works end-to-end.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::Utc;
use rmcp::handler::server::wrapper::{Json, Parameters};
use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_core::types::{
    Issue, IssueState, IssueType, Priority, QualifiedId, ReadyState, Status,
};
use unblock_github::GitHubApi;
use unblock_github::mock::MockGitHubClient;
use unblock_github::projects::{
    CreatedProject, FieldMeta, OwnerType, ProjectFieldIds, ProjectInfo, ProjectView, SetupStatus,
    ViewLayout,
};
use unblock_mcp::server::{ServerState, UnblockServer};
use unblock_mcp::tools::claim::ClaimParams;
use unblock_mcp::tools::close::CloseParams;
use unblock_mcp::tools::comment::CommentParams;
use unblock_mcp::tools::create::CreateParams;
use unblock_mcp::tools::depends::DependsParams;
use unblock_mcp::tools::init::InitParams;
use unblock_mcp::tools::prime::PrimeParams;
use unblock_mcp::tools::ready::ReadyParams;
use unblock_mcp::tools::reconcile::ReconcileParams;
use unblock_mcp::tools::setup::SetupParams;
use unblock_mcp::tools::show::ShowParams;
use unblock_mcp::tools::update::UpdateParams;

// ── Test fixtures ──────────────────────────────────────────────────────

/// Build a minimal, deterministic `Config` without touching the environment.
fn test_config() -> Config {
    Config::load_from(|key| match key {
        "GITHUB_TOKEN" => Ok("ghp_mock_token_for_dyn_dispatch_tests".to_owned()),
        "UNBLOCK_REPO" => Ok("acme/widgets".to_owned()),
        _ => Err(std::env::VarError::NotPresent),
    })
    .expect("mock test config should load")
}

/// Build a fresh `MockGitHubClient` with the same coordinates as `test_config`.
fn new_mock() -> Arc<MockGitHubClient> {
    Arc::new(MockGitHubClient::new("acme", "widgets", Some(1)))
}

/// Wrap a mock in a `ServerState` whose `client` is typed as
/// `Arc<dyn GitHubApi>`. Every handler call therefore goes through the
/// dyn-dispatch vtable.
fn state_with_mock(mock: Arc<MockGitHubClient>) -> ServerState {
    let config = test_config();
    let client: Arc<dyn GitHubApi> = mock;
    ServerState {
        config: Arc::new(config),
        github: client,
        cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
        agent_kind: OnceLock::new(),
        agent_client: OnceLock::new(),
        connected_at: OnceLock::new(),
    }
}

/// Build a minimal, open `Issue` suitable for most handler stubs.
fn make_issue(number: u64) -> Issue {
    Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_{number}"),
        title: format!("Issue #{number}"),
        issue_type: Some(IssueType::Task),
        status: Status::Open,
        priority: Priority::P2,
        agent: None,
        claimed_at: None,
        ready_state: ReadyState::Ready,
        story_points: None,
        defer_until: None,
        labels: vec![],
        milestone: None,
        assignees: vec![],
        state: IssueState::Open,
        body: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        url: format!("https://github.com/acme/widgets/issues/{number}"),
        comments: vec![],
        blocked_by: vec![],
        blocking: vec![],
        parent: None,
        sub_issues: vec![],
    }
}

/// Helper: a minimal `ProjectFieldIds` with empty option maps. Not currently
/// used by these tests because handlers gracefully skip project-field updates
/// when `field_ids()` returns `None`, which keeps mocks small.
#[allow(dead_code)]
fn empty_field_ids() -> ProjectFieldIds {
    let meta = || FieldMeta {
        field_id: "f".to_owned(),
        options: HashMap::new(),
    };
    ProjectFieldIds {
        status: meta(),
        priority: meta(),
        issue_type: meta(),
        agent: "agent".to_owned(),
        story_points: "sp".to_owned(),
        defer_until: "du".to_owned(),
        ready_state: meta(),
    }
}

// ── init ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn init_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_detect_owner_type(Ok(OwnerType::Org));
    mock.push_resolve_owner_node_id(Ok("O_acme".to_owned()));
    mock.push_list_owner_projects(Ok(vec![])); // no existing match
    mock.push_create_project(Ok(CreatedProject {
        number: 42,
        url: "https://github.com/orgs/acme/projects/42".to_owned(),
    }));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .init(Parameters(InitParams {
            scope: None,
            title: Some("Widgets Tasks".to_owned()),
            description: None,
            public: None,
        }))
        .await
        .expect("init should succeed via dyn dispatch");

    assert_eq!(result.project_number, 42);
    assert!(result.created);
    assert_eq!(mock.calls().detect_owner_type(), 1);
    assert_eq!(mock.calls().resolve_owner_node_id(), 1);
    assert_eq!(mock.calls().list_owner_projects(), 1);
    assert_eq!(mock.calls().create_project(), 1);
}

// ── setup ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn setup_dry_run_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_detect_owner_type(Ok(OwnerType::Org));
    mock.push_query_setup_status(Ok(SetupStatus {
        existing: vec![],
        missing: vec![],
    }));
    mock.push_list_views(Ok(vec![ProjectView {
        id: Some(1),
        number: 1,
        name: "://ready".to_owned(),
        layout: ViewLayout::Board,
        node_id: None,
        filter: None,
        visible_fields: vec![],
    }]));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .setup(Parameters(SetupParams {
            project: None,
            dry_run: Some(true),
        }))
        .await
        .expect("setup dry-run should succeed via dyn dispatch");

    assert_eq!(result.project_number, 1);
    assert!(result.dry_run);
    assert_eq!(mock.calls().resolve_project_info(), 1);
    assert_eq!(mock.calls().detect_owner_type(), 1);
    assert_eq!(mock.calls().query_setup_status(), 1);
    assert_eq!(mock.calls().list_views(), 1);
}

// ── show ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn show_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_fetch_issue_ref(Ok(make_issue(10)));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .show(Parameters(ShowParams {
            issue: "10".to_owned(),
            include_comments: Some(false),
            include_deps: Some(false),
        }))
        .await
        .expect("show should succeed via dyn dispatch");

    assert_eq!(result.issue.number, 10);
    assert_eq!(mock.calls().fetch_issue_ref(), 1);
}

/// Show accepts a cross-repo `owner/repo#number` reference and dispatches
/// `fetch_issue_ref` through the trait-object vtable, confirming alignment
/// with ARCH §10.6 (`IssueRef` input). Mirrors the cross-repo pattern used
/// in the `depends` / `create.blocked_by` integration tests.
#[tokio::test]
async fn show_accepts_cross_repo_issue_ref() {
    let mock = new_mock();
    mock.push_fetch_issue_ref(Ok(make_issue(42)));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .show(Parameters(ShowParams {
            issue: "acme/widgets#42".to_owned(),
            include_comments: Some(false),
            include_deps: Some(false),
        }))
        .await
        .expect("show should accept owner/repo#number and dispatch through dyn vtable");

    assert_eq!(result.issue.number, 42);
    assert_eq!(mock.calls().fetch_issue_ref(), 1);
    // Cross-repo issues are not in the local cache graph — dependency tree
    // is always absent here because include_deps=false, and additionally the
    // handler must not have attempted a local-repo fetch_issue call.
    assert_eq!(mock.calls().fetch_issue(), 0);
}

/// Negative-path coverage: prove that an `Err` returned by the mock also
/// crosses the `Arc<dyn GitHubApi>` vtable boundary and propagates out as an
/// MCP `ErrorData`. Without this, only the Ok branch is exercised through
/// dyn dispatch and a regression in the error-path call site could pass.
#[tokio::test]
async fn show_err_propagates_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_fetch_issue_ref(Err(unblock_github::errors::Error::GitHubApi {
        status: 404,
        message: "Not Found".to_owned(),
    }));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let result = server
        .show(Parameters(ShowParams {
            issue: "999".to_owned(),
            include_comments: Some(false),
            include_deps: Some(false),
        }))
        .await;
    let Err(err) = result else {
        panic!("show must surface mock Err through dyn dispatch");
    };

    // The vtable call must have happened exactly once even on the Err path.
    assert_eq!(mock.calls().fetch_issue_ref(), 1);
    // Sanity: the propagated MCP error carries the upstream message.
    assert!(
        err.message.contains("Not Found") || err.message.contains("404"),
        "expected propagated GitHub error message, got: {}",
        err.message
    );
}

// ── ready ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn ready_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    // ready triggers a lazy rebuild when cache is stale (empty).
    mock.push_fetch_graph_data(Ok((vec![make_issue(1)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .ready(Parameters(ReadyParams {
            limit: None,
            issue_type: None,
            priority: None,
            milestone: None,
            agent: None,
            label: None,
            include_claimed: None,
        }))
        .await
        .expect("ready should succeed via dyn dispatch");

    assert!(!result.stale);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

// ── claim ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn claim_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    // claim uses execute_write_tool → fetch_issue, add_comment, rebuild via
    // fetch_graph_data. field_ids returns None so project-field updates are
    // skipped, keeping the stub set minimal.
    mock.push_fetch_issue(Ok(make_issue(7)));
    mock.push_add_comment(Ok(
        "https://github.com/acme/widgets/issues/7#comment-1".to_owned()
    ));
    mock.push_fetch_graph_data(Ok((vec![make_issue(7)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .claim(Parameters(ClaimParams {
            id: 7,
            agent: Some("alice".to_owned()),
        }))
        .await
        .expect("claim should succeed via dyn dispatch");

    assert_eq!(result.issue_number, 7);
    assert_eq!(result.agent, "alice");
    assert_eq!(mock.calls().fetch_issue(), 1);
    assert_eq!(mock.calls().add_comment(), 1);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
    assert_eq!(mock.calls().field_ids(), 1);
}

#[tokio::test]
async fn claim_with_empty_agent_returns_invalid_params() {
    // Regression for unblock-b6b.80 follow-up: Some("") and whitespace-only
    // agent must be rejected at the handler level with INVALID_PARAMS (HTTP
    // 400) BEFORE any GitHub API call is made.
    let mock = new_mock();
    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));

    for bad in ["", "   ", "\t \n"] {
        let err = server
            .claim(Parameters(ClaimParams {
                id: 7,
                agent: Some(bad.to_owned()),
            }))
            .await
            .map(|_| ())
            .expect_err(&format!(
                "empty/whitespace agent must be rejected for {bad:?}"
            ));
        assert_eq!(
            err.code,
            rmcp::model::ErrorCode::INVALID_PARAMS,
            "expected INVALID_PARAMS for agent={bad:?}"
        );
    }

    // Crucially: no GitHub API calls should have been made.
    assert_eq!(mock.calls().fetch_issue(), 0);
    assert_eq!(mock.calls().add_comment(), 0);
    assert_eq!(mock.calls().fetch_graph_data(), 0);
}

// ── close ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn close_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_fetch_issue(Ok(make_issue(8)));
    mock.push_close_issue(Ok(()));
    mock.push_fetch_graph_data(Ok((vec![make_issue(8)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .close(Parameters(CloseParams {
            id: 8,
            reason: None,
        }))
        .await
        .expect("close should succeed via dyn dispatch");

    assert_eq!(result.issue, 8);
    assert!(result.unblocked.is_empty());
    assert_eq!(mock.calls().fetch_issue(), 1);
    assert_eq!(mock.calls().close_issue(), 1);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

#[tokio::test]
async fn close_with_empty_reason_skips_comment() {
    // Regression for unblock-b6b.85: Some("") and whitespace-only reason
    // should be treated as None and NOT post an empty comment before closing.
    let mock = new_mock();
    mock.push_fetch_issue(Ok(make_issue(9)));
    mock.push_close_issue(Ok(()));
    mock.push_fetch_graph_data(Ok((vec![make_issue(9)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .close(Parameters(CloseParams {
            id: 9,
            reason: Some("   ".to_owned()),
        }))
        .await
        .expect("close should succeed via dyn dispatch");

    assert_eq!(result.issue, 9);
    assert_eq!(mock.calls().close_issue(), 1);
    // Crucially: no comment should have been posted.
    assert_eq!(
        mock.calls().add_comment(),
        0,
        "empty/whitespace reason must not post a comment"
    );
}

// ── depends ────────────────────────────────────────────────────────────

#[tokio::test]
async fn depends_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_fetch_issue(Ok(make_issue(3))); // source
    mock.push_add_blocked_by_ref(Ok(()));
    mock.push_fetch_graph_data(Ok((vec![make_issue(3), make_issue(5)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .depends(Parameters(DependsParams {
            source: 3,
            target: "5".to_owned(),
        }))
        .await
        .expect("depends should succeed via dyn dispatch");

    assert_eq!(result.source, 3);
    assert_eq!(result.target, "5");
    assert_eq!(mock.calls().fetch_issue(), 1);
    assert_eq!(mock.calls().add_blocked_by_ref(), 1);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

// ── create ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_create_issue(Ok(make_issue(100)));
    mock.push_fetch_graph_data(Ok((vec![make_issue(100)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .create(Parameters(CreateParams {
            title: "Test issue".to_owned(),
            issue_type: None,
            priority: None,
            body: None,
            labels: None,
            milestone: None,
            blocked_by: None,
            parent: None,
            story_points: None,
            defer_until: None,
        }))
        .await
        .expect("create should succeed via dyn dispatch");

    assert_eq!(result.number, 100);
    assert_eq!(mock.calls().create_issue(), 1);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

// ── comment ────────────────────────────────────────────────────────────

#[tokio::test]
async fn comment_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_fetch_issue(Ok(make_issue(11)));
    mock.push_add_comment(Ok(
        "https://github.com/acme/widgets/issues/11#comment-9".to_owned()
    ));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .comment(Parameters(CommentParams {
            id: 11,
            body: "hello".to_owned(),
        }))
        .await
        .expect("comment should succeed via dyn dispatch");

    assert_eq!(result.issue_number, 11);
    assert_eq!(mock.calls().fetch_issue(), 1);
    assert_eq!(mock.calls().add_comment(), 1);
}

// ── update ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    // update handler fetches the issue twice (step 1 validate + step 9 re-fetch)
    // and triggers a cache rebuild via execute_write_tool.
    mock.push_fetch_issue(Ok(make_issue(12)));
    mock.push_fetch_issue(Ok(make_issue(12)));
    mock.push_fetch_graph_data(Ok((vec![make_issue(12)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .update(Parameters(UpdateParams {
            id: 12,
            priority: None,
            status: None,
            labels_add: None,
            labels_remove: None,
            assignees_add: None,
            assignees_remove: None,
            body_section: None,
            milestone: None,
            story_points: None,
            defer_until: None,
        }))
        .await
        .expect("update should succeed via dyn dispatch");

    assert_eq!(result.number, 12);
    assert_eq!(mock.calls().fetch_issue(), 2);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

// ── prime ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn prime_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    // prime spawns a background reconcile concurrently (which also calls
    // fetch_graph_data) and itself fetches once directly. The background
    // JoinHandle is awaited inside prime via resolve_drift_warnings(), so by
    // the time prime returns BOTH fetches have completed deterministically.
    // Queue two stubs and assert exactly two calls to prove both vtable
    // traversals occurred (direct + background).
    mock.push_fetch_graph_data(Ok((vec![make_issue(1)], vec![])));
    mock.push_fetch_graph_data(Ok((vec![make_issue(1)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .prime(Parameters(PrimeParams {
            stale_threshold_hours: None,
            max_per_category: None,
            agent: None,
        }))
        .await
        .expect("prime should succeed via dyn dispatch");

    // Both the direct fetch AND the awaited background reconcile fetch must
    // have traversed the dyn vtable by now.
    assert_eq!(mock.calls().fetch_graph_data(), 2);
    // Basic sanity on result shape.
    let _ = result.counts.ready;
}

// ── reconcile ──────────────────────────────────────────────────────────

#[tokio::test]
async fn reconcile_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_fetch_graph_data(Ok((vec![make_issue(1)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(_result) = server
        .reconcile(Parameters(ReconcileParams {
            fix: false,
            stale_claim_hours: 24,
        }))
        .await
        .expect("reconcile should succeed via dyn dispatch");

    assert_eq!(mock.calls().fetch_graph_data(), 1);
}

// ── Compile-time witness ───────────────────────────────────────────────

/// Compile-time proof that `Arc<MockGitHubClient>` coerces to
/// `Arc<dyn GitHubApi>`. A future dyn-incompatible change to the trait
/// would break this line at build time.
#[allow(dead_code)]
fn _dyn_object_safety_witness() {
    let _: Arc<dyn GitHubApi> = new_mock();
}
