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
use std::sync::Arc;

use chrono::Utc;
use rmcp::handler::server::wrapper::{Json, Parameters};
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{
    BlockingEdge, Issue, IssueState, IssueType, Priority, QualifiedId, Status,
};
use unblock_github::GitHubApi;
use unblock_github::projects::{
    CreatedProject, FieldMeta, OwnerType, ProjectFieldIds, ProjectInfo, ProjectView, SetupStatus,
    ViewLayout,
};
use unblock_mcp::server::UnblockServer;
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

mod common;
use common::{new_mock, state_with_mock};

// ── Test fixtures ──────────────────────────────────────────────────────

/// Build a minimal, open `Issue` suitable for most handler stubs.
fn make_issue(number: u64) -> Issue {
    Issue {
        qualified_id: QualifiedId::new("acme", "widgets", number),
        number,
        node_id: format!("I_{number}"),
        title: format!("Issue #{number}"),
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

/// A minimal `ProjectFieldIds` with empty option maps.
///
/// When pushed via `mock.push_field_ids(Some(empty_field_ids()))`, handlers
/// enter the `field_ids = Some(...)` branch and call `resolve_project_info`
/// and `get_project_item_id`. Because the option maps are empty, no
/// `update_field` calls are made for select-option fields (`status`,
/// `priority`, `pipeline_stage`). Plain-text, numeric, and date fields
/// (`agent`, `claimed_at`, `story_points`, `defer_until`) bypass the option
/// map and DO call `update_field`.
fn empty_field_ids() -> ProjectFieldIds {
    let meta = || FieldMeta {
        field_id: "f".to_owned(),
        options: HashMap::new(),
    };
    ProjectFieldIds {
        status: meta(),
        priority: meta(),
        pipeline_stage: meta(),
        agent: "agent".to_owned(),
        claimed_at: "ca".to_owned(),
        story_points: "sp".to_owned(),
        defer_until: "du".to_owned(),
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
    // fetch_graph_data. field_ids returns Some so the handler enters the
    // project-field update branch. With empty option maps, only the
    // unconditional Agent text-field update calls update_field.
    mock.push_fetch_issue(Ok(make_issue(7)));
    mock.push_add_comment(Ok(
        "https://github.com/acme/widgets/issues/7#comment-1".to_owned()
    ));
    mock.push_field_ids(Some(empty_field_ids()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_claim".to_owned()));
    // Agent field update is unconditional (not gated by options map).
    mock.push_update_field(Ok(()));
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
    assert_eq!(mock.calls().resolve_project_info(), 1);
    assert_eq!(mock.calls().get_project_item_id(), 1);
    // Only the Agent text-field update fires; option-gated fields (Status)
    // are skipped because empty_field_ids() has empty option maps.
    assert_eq!(mock.calls().update_field(), 1);
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
    // field_ids=Some enters the project-field update branch. With empty
    // option maps, no update_field calls are made (Status=Done is
    // gated by options.get()).
    mock.push_field_ids(Some(empty_field_ids()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_close".to_owned()));
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
    assert_eq!(mock.calls().field_ids(), 1);
    assert_eq!(mock.calls().resolve_project_info(), 1);
    assert_eq!(mock.calls().get_project_item_id(), 1);
    assert_eq!(mock.calls().update_field(), 0);
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
    mock.push_fetch_issue_ref(Ok(make_issue(3))); // source (local)
    mock.push_add_blocked_by_refs(Ok(()));
    // field_ids=Some enters the project-field update branch. With empty
    // option maps, no update_field calls are made (Status=Blocked is
    // gated by options.get()).
    mock.push_field_ids(Some(empty_field_ids()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_depends".to_owned()));
    mock.push_fetch_graph_data(Ok((vec![make_issue(3), make_issue(5)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .depends(Parameters(DependsParams {
            source: "3".to_owned(),
            target: "5".to_owned(),
        }))
        .await
        .expect("depends should succeed via dyn dispatch");

    assert!(result.created, "created flag must be true on success");
    // Both source and target render in canonical Display form: local refs
    // use the hash-prefix shape (e.g. "#3"), matching IssueRef::Local Display.
    assert_eq!(result.source, "#3");
    assert_eq!(result.target, "#5");
    assert_eq!(mock.calls().fetch_issue_ref(), 1);
    assert_eq!(mock.calls().add_blocked_by_refs(), 1);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
    assert_eq!(mock.calls().field_ids(), 1);
    assert_eq!(mock.calls().resolve_project_info(), 1);
    assert_eq!(mock.calls().get_project_item_id(), 1);
    assert_eq!(mock.calls().update_field(), 0);
    // Old local-source entry points must NOT be called.
    assert_eq!(mock.calls().fetch_issue(), 0);
    assert_eq!(mock.calls().add_blocked_by_ref(), 0);
}

/// Exercises the cross-repo source path (`source: "other-owner/other-repo#7"`).
///
/// Verifies:
/// - `fetch_issue_ref` is called for source (not `fetch_issue`, which is
///   local-only).
/// - `add_blocked_by_refs` is the mutation entry point used.
/// - Projects V2 field update on source is SKIPPED (source is outside the
///   configured project), so `field_ids`, `resolve_project_info`,
///   `get_project_item_id`, and `update_field` are not called.
/// - Cache rebuild (`fetch_graph_data`) still runs.
/// - The result renders `source` and `target` in their canonical `IssueRef`
///   `Display` forms.
#[tokio::test]
async fn depends_accepts_cross_repo_source() {
    let mock = new_mock();
    // Source fetch uses fetch_issue_ref for the cross-repo source.
    // Build a shaped issue — number is 7 in other-owner/other-repo.
    let mut source_issue = make_issue(7);
    source_issue.qualified_id = QualifiedId::new("other-owner", "other-repo", 7);
    source_issue.url = "https://github.com/other-owner/other-repo/issues/7".to_owned();
    mock.push_fetch_issue_ref(Ok(source_issue));
    mock.push_add_blocked_by_refs(Ok(()));
    mock.push_fetch_graph_data(Ok((vec![make_issue(5)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .depends(Parameters(DependsParams {
            source: "other-owner/other-repo#7".to_owned(),
            target: "5".to_owned(),
        }))
        .await
        .expect("depends should succeed for cross-repo source");

    assert!(result.created);
    assert_eq!(result.source, "other-owner/other-repo#7");
    assert_eq!(result.target, "#5");
    assert_eq!(mock.calls().fetch_issue_ref(), 1);
    assert_eq!(mock.calls().add_blocked_by_refs(), 1);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
    // Field update path MUST be skipped for cross-repo source.
    assert_eq!(mock.calls().field_ids(), 0);
    assert_eq!(mock.calls().resolve_project_info(), 0);
    assert_eq!(mock.calls().get_project_item_id(), 0);
    assert_eq!(mock.calls().update_field(), 0);
}

/// Rejects `source == target` (spec §8.4 validation requirement).
///
/// The check operates on the resolved [`QualifiedId`], so local variants
/// `"42"` and `"#42"` collapse to the same identity and are rejected. No
/// network calls should be issued.
#[tokio::test]
async fn depends_rejects_self_reference() {
    let mock = new_mock();

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let err = server
        .depends(Parameters(DependsParams {
            source: "42".to_owned(),
            target: "#42".to_owned(),
        }))
        .await
        .map(|_| ())
        .expect_err("source == target must be rejected as a validation error");

    // Mapped to INVALID_PARAMS by github_error_to_mcp (HTTP 400).
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.contains("source and target"),
        "message should mention the self-ref validation: {}",
        err.message
    );

    // No outbound calls should have been made.
    assert_eq!(mock.calls().fetch_issue(), 0);
    assert_eq!(mock.calls().fetch_issue_ref(), 0);
    assert_eq!(mock.calls().add_blocked_by_ref(), 0);
    assert_eq!(mock.calls().add_blocked_by_refs(), 0);
}

/// Exercises the aliasing case for `source`: when the caller spells the
/// configured repo explicitly as a cross-repo ref (`acme/widgets#N` where
/// the configured repo IS `acme/widgets`), the handler must treat the ref
/// as local for every downstream guard.
///
/// Verifies the normalization fix for the review [WARNING]s:
/// 1. Cycle detection runs: a pre-populated cache with an edge
///    `target -> source` causes `would_create_cycle` to fire, and the
///    handler returns a `CircularDependency` error — proving it took the
///    `(Local, Local)` arm rather than the cross-repo skip-warn arm.
/// 2. Result rendering is the canonical Local form (`"#N"`), matching
///    what `"N"` and `"#N"` inputs produce, fulfilling the "stable
///    output regardless of input form" contract.
///
/// Notes: this test is scoped to cycle rejection, so it does NOT invoke
/// the mutation, field update, or graph rebuild paths. A sibling test
/// below covers the happy path and asserts the field-update call
/// counters.
#[tokio::test]
async fn depends_aliased_configured_repo_source_cycle_detection() {
    let mock = new_mock();
    // Source (issue #3) is in the configured repo (acme/widgets). The
    // handler must fetch it via fetch_issue_ref(Local(3)) after
    // normalization.
    mock.push_fetch_issue_ref(Ok(make_issue(3)));

    // Pre-populate the cache with a graph containing a cycle that
    // would_create_cycle(source=3, target=5) will detect:
    // path target(5) -> source(3) already exists.
    let issues = vec![make_issue(3), make_issue(5)];
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 5),
        target: QualifiedId::new("acme", "widgets", 3),
    }];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues, "acme", "widgets");

    let state = state_with_mock(Arc::clone(&mock));
    let cache = Arc::clone(&state.cache);
    cache.update(issues, ready_set, graph).await;

    let server = UnblockServer::new(state);

    // Caller passes source as CrossRepo form pointing at the configured
    // repo. Without normalization, the handler would take the skip-warn
    // cross-repo arm and the mutation would proceed. With normalization,
    // the handler enters the (Local, Local) arm, runs would_create_cycle,
    // and rejects.
    let err = server
        .depends(Parameters(DependsParams {
            source: "acme/widgets#3".to_owned(),
            target: "5".to_owned(),
        }))
        .await
        .map(|_| ())
        .expect_err("cycle must be detected for aliased-configured-repo source");

    // CircularDependency maps to 422 → INVALID_PARAMS.
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.to_lowercase().contains("circular"),
        "error message should reference a circular dependency: {}",
        err.message
    );

    // Source fetch happened (step 1 of the handler).
    assert_eq!(mock.calls().fetch_issue_ref(), 1);
    // Mutation MUST NOT run when cycle is rejected.
    assert_eq!(mock.calls().add_blocked_by_refs(), 0);
    assert_eq!(mock.calls().add_blocked_by_ref(), 0);
    // Rebuild does not run when we rejected before the mutation.
    assert_eq!(mock.calls().fetch_graph_data(), 0);
}

/// Exercises the aliasing-case happy path for `source`: `acme/widgets#N`
/// where the configured repo IS `acme/widgets`.
///
/// Verifies the normalization fix for the review [WARNING]s (happy path):
/// - The Projects V2 field update ladder runs (`field_ids`,
///   `resolve_project_info`, `get_project_item_id` each called once). Under
///   the aliasing bug this was skipped.
/// - The mutation entry point is still `add_blocked_by_refs` (the
///   `GitHubApi` trait method), invoked exactly once.
/// - Result rendering: `source` is the canonical Local form (`"#3"`),
///   matching the `"3"` and `"#3"` input forms — fulfills the
///   "stable output regardless of input form" contract.
#[tokio::test]
async fn depends_aliased_configured_repo_source_happy_path() {
    let mock = new_mock();
    mock.push_fetch_issue_ref(Ok(make_issue(3))); // source (aliased as cross-repo)
    mock.push_add_blocked_by_refs(Ok(()));
    mock.push_field_ids(Some(empty_field_ids()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_aliased".to_owned()));
    mock.push_fetch_graph_data(Ok((vec![make_issue(3), make_issue(5)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .depends(Parameters(DependsParams {
            source: "acme/widgets#3".to_owned(),
            target: "5".to_owned(),
        }))
        .await
        .expect("depends should succeed for aliased-configured-repo source");

    assert!(result.created);
    // Source renders as the canonical Local form "#3", matching the
    // unaliased "3" and "#3" inputs.
    assert_eq!(result.source, "#3");
    assert_eq!(result.target, "#5");

    // Source fetch path (step 1 of the handler) ran once.
    assert_eq!(mock.calls().fetch_issue_ref(), 1);
    // Mutation dispatch (step 3).
    assert_eq!(mock.calls().add_blocked_by_refs(), 1);
    // Cache rebuild (wrap in execute_write_tool).
    assert_eq!(mock.calls().fetch_graph_data(), 1);
    // Projects V2 field update ladder MUST run for aliased local source.
    // Under the aliasing bug these counters would all be 0.
    assert_eq!(mock.calls().field_ids(), 1);
    assert_eq!(mock.calls().resolve_project_info(), 1);
    assert_eq!(mock.calls().get_project_item_id(), 1);
    // Empty option maps gate the SingleSelectOption update, so
    // update_field is not called — identical to the canonical-form test.
    assert_eq!(mock.calls().update_field(), 0);
    // The local-only fallbacks must NOT be called.
    assert_eq!(mock.calls().fetch_issue(), 0);
    assert_eq!(mock.calls().add_blocked_by_ref(), 0);
}

/// Mirrors `depends_aliased_configured_repo_source_cycle_detection` with
/// the alias on `target` instead of `source`. Source is local (`"3"`);
/// target is `"acme/widgets#5"` (aliased cross-repo form of the
/// configured repo's issue #5).
///
/// Without normalization on `target_ref`, the cycle-detection `match` arm
/// would skip with a warn because the `(Local, CrossRepo)` pattern falls
/// through. With normalization, both refs are `Local`, the arm runs, and
/// the pre-seeded cycle is detected.
#[tokio::test]
async fn depends_aliased_configured_repo_target_cycle_detection() {
    let mock = new_mock();
    mock.push_fetch_issue_ref(Ok(make_issue(3)));

    let issues = vec![make_issue(3), make_issue(5)];
    let edges = vec![BlockingEdge {
        source: QualifiedId::new("acme", "widgets", 5),
        target: QualifiedId::new("acme", "widgets", 3),
    }];
    let graph = DependencyGraph::build(&issues, &edges);
    let ready_set = graph.compute_ready_set(&issues, "acme", "widgets");

    let state = state_with_mock(Arc::clone(&mock));
    let cache = Arc::clone(&state.cache);
    cache.update(issues, ready_set, graph).await;

    let server = UnblockServer::new(state);

    let err = server
        .depends(Parameters(DependsParams {
            source: "3".to_owned(),
            target: "acme/widgets#5".to_owned(),
        }))
        .await
        .map(|_| ())
        .expect_err("cycle must be detected for aliased-configured-repo target");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    assert!(
        err.message.to_lowercase().contains("circular"),
        "error message should reference a circular dependency: {}",
        err.message
    );

    assert_eq!(mock.calls().fetch_issue_ref(), 1);
    assert_eq!(mock.calls().add_blocked_by_refs(), 0);
    assert_eq!(mock.calls().add_blocked_by_ref(), 0);
    assert_eq!(mock.calls().fetch_graph_data(), 0);
}

// ── create ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    mock.push_create_issue(Ok(make_issue(100)));
    // field_ids=Some enters the project-field update branch. With empty
    // option maps, no update_field calls are made (all create fields —
    // Priority, IssueType, Status — are gated by options.get()).
    mock.push_field_ids(Some(empty_field_ids()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_create".to_owned()));
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
    assert_eq!(mock.calls().field_ids(), 1);
    assert_eq!(mock.calls().resolve_project_info(), 1);
    assert_eq!(mock.calls().get_project_item_id(), 1);
    assert_eq!(mock.calls().update_field(), 0);
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

/// Variant of the update test that passes `story_points=Some(5.0)` so
/// `has_project_updates=true` and the handler enters the `field_ids=Some`
/// branch. `story_points` bypasses the option map and calls `update_field`
/// unconditionally, proving the full project-field vtable path is exercised.
#[tokio::test]
async fn update_with_project_fields_dispatches_through_dyn_vtable() {
    let mock = new_mock();
    // update handler fetches the issue twice (step 1 validate + step 9 re-fetch)
    // and triggers a cache rebuild via execute_write_tool.
    mock.push_fetch_issue(Ok(make_issue(13)));
    mock.push_field_ids(Some(empty_field_ids()));
    mock.push_resolve_project_info(Ok(ProjectInfo {
        id: "PVT_1".to_owned(),
        number: 1,
    }));
    mock.push_get_project_item_id(Ok("PVTI_update".to_owned()));
    // story_points update_field call (bypasses option map).
    mock.push_update_field(Ok(()));
    mock.push_fetch_issue(Ok(make_issue(13)));
    mock.push_fetch_graph_data(Ok((vec![make_issue(13)], vec![])));

    let server = UnblockServer::new(state_with_mock(Arc::clone(&mock)));
    let Json(result) = server
        .update(Parameters(UpdateParams {
            id: 13,
            priority: None,
            status: None,
            labels_add: None,
            labels_remove: None,
            assignees_add: None,
            assignees_remove: None,
            body_section: None,
            milestone: None,
            story_points: Some(5.0),
            defer_until: None,
        }))
        .await
        .expect("update with story_points should succeed via dyn dispatch");

    assert_eq!(result.number, 13);
    assert_eq!(mock.calls().fetch_issue(), 2);
    assert_eq!(mock.calls().fetch_graph_data(), 1);
    assert_eq!(mock.calls().field_ids(), 1);
    assert_eq!(mock.calls().resolve_project_info(), 1);
    assert_eq!(mock.calls().get_project_item_id(), 1);
    // story_points calls update_field directly (no option map gating).
    assert_eq!(mock.calls().update_field(), 1);
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
