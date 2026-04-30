//! End-to-end workflow integration test exercising all 10 Phase 1 MCP tools.
//!
//! This test creates real GitHub Issues and a real GitHub Project, running
//! through the full tool lifecycle: `init` → `setup` → `create` → `ready` →
//! `depends` → `claim` → `update` → `comment` → `show` → `close`.
//!
//! # Status — live-required, opt-in via `--ignored`
//!
//! The single test in this file is tagged `#[ignore]`. It only runs when
//! invoked explicitly via `cargo test --workspace -- --ignored` with all
//! the env vars below exported. Default `cargo test` runs do NOT execute it.
//! See bead `unblock-3lb` for the rationale (silent skips with `eprintln!`
//! were being counted as PASS by Cargo, masking that this test had never
//! actually executed in CI).
//!
//! # Prerequisites
//!
//! - `GITHUB_TOKEN` — a valid GitHub PAT with repo and project scopes.
//! - `UNBLOCK_REPO` — the `owner/repo` slug of the test repository.
//! - `UNBLOCK_PROJECT` — the project number on the test repository.
//!
//! The test calls `require_github_token_and_project()` and exits cleanly
//! if any of `GITHUB_TOKEN` / `UNBLOCK_PROJECT` are missing.
//!
//! # Cleanup
//!
//! All created issues are closed via a drop guard (`CloseIssuesGuard`) that
//! fires on both success and panic unwind. This ensures the test repository is
//! not polluted with orphaned open issues.

use unblock_github::GitHubApi;
use unblock_github::mutations::CreateIssueParams;
use unblock_github::projects::{CreateViewParams, FieldValue, ViewLayout};
use unblock_mcp::server::set_project_fields;
use unblock_mcp::tools::ready::{ReadyParams, filter_ready_set};
use unblock_mcp::tools::rebuild_cache;
use unblock_mcp::tools::setup::REQUIRED_VIEWS;

mod common;
use common::{require_github_token_and_project, test_server_state};

/// Drop guard that closes multiple GitHub issues on scope exit, even during a
/// panic unwind. Adapted from the single-issue guard in
/// `crates/unblock-github/tests/integration.rs`.
///
/// On drop, iterates over all tracked issue numbers and closes each one.
/// Individual close failures are logged but do not abort the remaining cleanup.
struct CloseIssuesGuard<'a> {
    client: &'a dyn GitHubApi,
    issue_numbers: Vec<u64>,
    /// Set to `true` once the test completes successfully and the caller has
    /// already cleaned up. When `true`, the guard skips cleanup in `Drop`.
    disarmed: bool,
}

impl<'a> CloseIssuesGuard<'a> {
    /// Creates an armed guard with no issues tracked.
    fn new(client: &'a dyn GitHubApi) -> Self {
        Self {
            client,
            issue_numbers: Vec::new(),
            disarmed: false,
        }
    }

    /// Adds an issue number to the cleanup list.
    fn track(&mut self, issue_number: u64) {
        self.issue_numbers.push(issue_number);
    }

    /// Disarms the guard so that `Drop` becomes a no-op.
    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for CloseIssuesGuard<'_> {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        let numbers = self.issue_numbers.clone();
        let client = self.client;
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            for number in &numbers {
                if let Err(e) = handle.block_on(
                    client.close_issue(*number, Some("E2E test cleanup (drop guard)".to_owned())),
                ) {
                    eprintln!("CloseIssuesGuard: failed to close issue #{number}: {e}");
                } else {
                    eprintln!("CloseIssuesGuard: cleaned up issue #{number}");
                }
            }
        });
    }
}

// ── E2E Workflow Test ───────────────────────────────────────────────

/// Full end-to-end workflow test exercising all 10 Phase 1 MCP tools in
/// sequence against a real GitHub repository.
///
/// Test sequence:
///  1. `init`    — create (or reuse) a Projects V2 project
///  2. `setup`   — create fields + views on the project
///  3. `create`  — create issue A (no deps) with priority P1
///  4. `create`  — create issue B (blocked by A) with priority P2
///  5. `create`  — create issue C (no deps) with priority P3
///  6. `ready`   — verify: A and C in ready set, B not in ready set
///  7. `depends` — add B blocked by C explicitly (B now blocked by A and C)
///  8. `claim`   — claim issue A for agent 'test-agent'
///  9. `ready`   — verify: A no longer in ready set (claimed), C in ready set
/// 10. `update`  — update A's priority to P0, add label 'urgent'
/// 11. `comment` — post comment on A
/// 12. `show`    — show A, verify comment and label appear
/// 13. `close` A — verify: A closed, B still blocked (still has C)
/// 14. `close` C — verify: C closed, B now in ready set (cascade)
/// 15. `ready`   — final verify: B in ready set, A and C not present
/// 16. Cleanup: close B
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live GitHub API + Projects V2 — opt-in via cargo test --workspace -- --ignored with GITHUB_TOKEN + UNBLOCK_REPO + UNBLOCK_PROJECT"]
async fn e2e_workflow_all_10_tools() {
    // ── Gate: skip if required env vars are not set ──────────────────
    if !require_github_token_and_project() {
        return;
    }

    let state = test_server_state().await;
    let client = &state.github;

    // Unique label to isolate test issues from other issues in the repo.
    let test_label = format!("e2e-test-{}", chrono::Utc::now().timestamp());

    // Drop guard for cleanup — tracks all created issues.
    let mut guard = CloseIssuesGuard::new(client.as_ref());

    // ── Step 1: init — create or reuse project ──────────────────────
    eprintln!("=== Step 1: init ===");
    let owner_type = client
        .detect_owner_type()
        .await
        .expect("detect_owner_type should succeed");

    let existing_projects = client
        .list_owner_projects(owner_type)
        .await
        .expect("list_owner_projects should succeed");

    eprintln!(
        "init: found {} existing projects for owner '{}'",
        existing_projects.len(),
        client.owner()
    );

    // init is idempotent — we just verify the project listing works.
    // The project is already configured via UNBLOCK_PROJECT, so we verify
    // resolve_project_info succeeds.
    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info should succeed — project exists");

    eprintln!(
        "init: project #{} (id={}) verified",
        project_info.number, project_info.id
    );

    // ── Step 2: setup — create fields + views ───────────────────────
    eprintln!("=== Step 2: setup ===");
    let report = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields should succeed");

    let total_fields = report.created.len() + report.skipped.len();
    assert_eq!(
        total_fields, 7,
        "setup should resolve exactly 7 fields, got {total_fields}"
    );

    // Cache the resolved field IDs on the client.
    client.set_field_ids(report.field_ids).await;

    // Fail early if field_ids were not resolved — subsequent project field
    // assignments would silently no-op and produce false-positive assertions.
    assert!(
        client.field_ids().await.is_some(),
        "field_ids should be Some after setup_fields + set_field_ids"
    );

    eprintln!(
        "setup: fields created={:?}, skipped={:?}",
        report.created, report.skipped
    );

    // Create missing views — mirrors the setup tool handler in server.rs.
    let existing_views = client
        .list_views(owner_type)
        .await
        .expect("list_views should succeed");

    let existing_view_names: std::collections::HashSet<&str> =
        existing_views.iter().map(|v| v.name.as_str()).collect();

    // Fetch REST field IDs needed for visible_fields on non-Roadmap views.
    let rest_fields = client
        .list_rest_fields(owner_type)
        .await
        .expect("list_rest_fields should succeed");
    let all_field_ids: Vec<u64> = rest_fields.iter().map(|f| f.id).collect();

    let mut views_created: Vec<String> = Vec::new();
    let mut views_existing: Vec<String> = Vec::new();

    for spec in REQUIRED_VIEWS {
        if existing_view_names.contains(spec.name) {
            views_existing.push(spec.name.to_owned());
            continue;
        }

        // Roadmap views do not support visible_fields (ARCH S8.5).
        let visible_fields = if spec.layout == ViewLayout::Roadmap {
            None
        } else {
            Some(all_field_ids.clone())
        };

        let view_params = CreateViewParams {
            name: spec.name.to_owned(),
            layout: spec.layout,
            filter: spec.filter.map(String::from),
            visible_fields,
        };

        client
            .create_view(owner_type, &view_params)
            .await
            .unwrap_or_else(|e| panic!("create_view({}) should succeed: {e}", spec.name));

        views_created.push(spec.name.to_owned());
    }

    eprintln!(
        "setup: views created={views_created:?}, views existing={views_existing:?} ({} total)",
        views_created.len() + views_existing.len()
    );

    // ── Step 3: create issue A (no deps, P1) ────────────────────────
    eprintln!("=== Step 3: create issue A ===");
    let issue_a = client
        .create_issue(CreateIssueParams {
            title: format!("[e2e] Issue A (no deps, P1) {test_label}"),
            body: Some("## Description\n\nE2E test issue A.".to_owned()),
            labels: vec![test_label.clone()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create issue A should succeed");
    guard.track(issue_a.number);

    // Add to project and set fields.
    if let Ok(item_id) = client
        .get_project_item_id(&issue_a.node_id, &project_info.id)
        .await
        && let Some(field_ids) = client.field_ids().await
    {
        // NOTE: story_points / defer_until (the trailing `None, None` args here
        // and at the analogous call sites for issues B and C below) are not
        // exercised by this e2e test — only the priority/status axes are
        // validated end-to-end against the live GitHub Project.
        set_project_fields(
            client.as_ref(),
            &project_info.id,
            &item_id,
            &field_ids,
            "P1",
            "ready",
            None,
            None,
        )
        .await;
    }

    rebuild_cache(&state).await;
    eprintln!("create A: #{} '{}'", issue_a.number, issue_a.title);

    // ── Step 4: create issue B (blocked by A, P2) ───────────────────
    eprintln!("=== Step 4: create issue B ===");
    let issue_b = client
        .create_issue(CreateIssueParams {
            title: format!("[e2e] Issue B (blocked by A, P2) {test_label}"),
            body: Some("## Description\n\nE2E test issue B.".to_owned()),
            labels: vec![test_label.clone()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create issue B should succeed");
    guard.track(issue_b.number);

    // Add blocking relationship: B blocked by A.
    client
        .add_blocked_by(issue_b.number, issue_a.number)
        .await
        .expect("add_blocked_by(B, A) should succeed");

    // Add to project and set fields (Blocked because has blocker).
    if let Ok(item_id) = client
        .get_project_item_id(&issue_b.node_id, &project_info.id)
        .await
        && let Some(field_ids) = client.field_ids().await
    {
        set_project_fields(
            client.as_ref(),
            &project_info.id,
            &item_id,
            &field_ids,
            "P2",
            "blocked",
            None,
            None,
        )
        .await;
    }

    rebuild_cache(&state).await;
    eprintln!("create B: #{} '{}'", issue_b.number, issue_b.title);

    // ── Step 5: create issue C (no deps, P3) ────────────────────────
    eprintln!("=== Step 5: create issue C ===");
    let issue_c = client
        .create_issue(CreateIssueParams {
            title: format!("[e2e] Issue C (no deps, P3) {test_label}"),
            body: Some("## Description\n\nE2E test issue C.".to_owned()),
            labels: vec![test_label.clone()],
            milestone: None,
            assignees: Vec::new(),
        })
        .await
        .expect("create issue C should succeed");
    guard.track(issue_c.number);

    // Add to project and set fields.
    if let Ok(item_id) = client
        .get_project_item_id(&issue_c.node_id, &project_info.id)
        .await
        && let Some(field_ids) = client.field_ids().await
    {
        set_project_fields(
            client.as_ref(),
            &project_info.id,
            &item_id,
            &field_ids,
            "P3",
            "ready",
            None,
            None,
        )
        .await;
    }

    rebuild_cache(&state).await;
    eprintln!("create C: #{} '{}'", issue_c.number, issue_c.title);

    // ── Step 6: ready — verify A and C in ready set, B not ──────────
    eprintln!("=== Step 6: ready (first check) ===");
    let ready_set = state
        .cache
        .get_ready_set()
        .await
        .expect("cache should have a ready set after rebuild");

    let ready_params = ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: Some(test_label.clone()),
        include_claimed: None,
    };
    let ready_issues = filter_ready_set(&ready_set, &ready_params);
    let ready_numbers: Vec<u64> = ready_issues.iter().map(|i| i.number).collect();

    eprintln!("ready (step 6): numbers={ready_numbers:?}");

    assert!(
        ready_numbers.contains(&issue_a.number),
        "Issue A (#{}) should be in ready set: {ready_numbers:?}",
        issue_a.number,
    );
    assert!(
        ready_numbers.contains(&issue_c.number),
        "Issue C (#{}) should be in ready set: {ready_numbers:?}",
        issue_c.number,
    );
    assert!(
        !ready_numbers.contains(&issue_b.number),
        "Issue B (#{}) should NOT be in ready set (blocked by A): {ready_numbers:?}",
        issue_b.number,
    );

    // ── Step 7: depends — add B blocked by C ────────────────────────
    eprintln!("=== Step 7: depends (B blocked by C) ===");
    client
        .add_blocked_by(issue_b.number, issue_c.number)
        .await
        .expect("add_blocked_by(B, C) should succeed");

    rebuild_cache(&state).await;

    // Verify B is still blocked (now by both A and C).
    let ready_set = state
        .cache
        .get_ready_set()
        .await
        .expect("cache should have a ready set");
    let ready_issues = filter_ready_set(&ready_set, &ready_params);
    let ready_numbers: Vec<u64> = ready_issues.iter().map(|i| i.number).collect();

    assert!(
        !ready_numbers.contains(&issue_b.number),
        "Issue B (#{}) should NOT be in ready set (blocked by A and C): {ready_numbers:?}",
        issue_b.number,
    );
    eprintln!("depends: B now blocked by both A and C");

    // ── Step 8: claim — claim issue A for 'test-agent' ──────────────
    eprintln!("=== Step 8: claim issue A ===");
    let agent_name = "test-agent";

    // Post claim comment.
    let claim_time = chrono::Utc::now();
    let claim_comment = format!(
        "\u{1F916} Claimed by {agent_name} at {}",
        claim_time.to_rfc3339()
    );
    client
        .add_comment(issue_a.number, claim_comment)
        .await
        .expect("claim comment should succeed");

    // Update project fields: Status -> In Progress, Agent -> test-agent.
    if let Some(field_ids) = client.field_ids().await
        && let Ok(item_id) = client
            .get_project_item_id(&issue_a.node_id, &project_info.id)
            .await
    {
        // Status -> in_progress
        if let Some(option_id) = field_ids.status.options.get("in_progress") {
            let _ = client
                .update_field(
                    &project_info.id,
                    &item_id,
                    &field_ids.status.field_id,
                    &FieldValue::SingleSelectOption(option_id.clone()),
                )
                .await;
        }

        // Agent -> test-agent
        let _ = client
            .update_field(
                &project_info.id,
                &item_id,
                &field_ids.agent,
                &FieldValue::Text(agent_name.to_owned()),
            )
            .await;

        // (ReadyState field removed — Status field now covers ready/blocked state)
    }

    rebuild_cache(&state).await;
    eprintln!("claim: A claimed by {agent_name}");

    // ── Step 9: ready — A no longer in ready set (claimed), C still ─
    eprintln!("=== Step 9: ready (after claim) ===");
    let ready_set = state
        .cache
        .get_ready_set()
        .await
        .expect("cache should have a ready set");
    let ready_issues = filter_ready_set(&ready_set, &ready_params);
    let ready_numbers: Vec<u64> = ready_issues.iter().map(|i| i.number).collect();

    eprintln!("ready (step 9): numbers={ready_numbers:?}");

    assert!(
        !ready_numbers.contains(&issue_a.number),
        "Issue A (#{}) should NOT be in ready set (claimed/InProgress): {ready_numbers:?}",
        issue_a.number,
    );
    assert!(
        ready_numbers.contains(&issue_c.number),
        "Issue C (#{}) should still be in ready set: {ready_numbers:?}",
        issue_c.number,
    );

    // ── Step 10: update — change A's priority to P0, add 'urgent' label ─
    eprintln!("=== Step 10: update issue A ===");

    // Ensure the 'urgent' label exists on the repo.
    client
        .ensure_labels(&["urgent".to_owned()])
        .await
        .expect("ensure_labels(urgent) should succeed");

    // Add 'urgent' label to issue A.
    client
        .add_labels_to_issue(issue_a.number, vec!["urgent".to_owned()])
        .await
        .expect("add_labels_to_issue should succeed");

    // Update priority to P0 via project fields.
    if let Some(field_ids) = client.field_ids().await
        && let Ok(item_id) = client
            .get_project_item_id(&issue_a.node_id, &project_info.id)
            .await
        && let Some(option_id) = field_ids.priority.option_id_by_prefix("P0")
    {
        let _ = client
            .update_field(
                &project_info.id,
                &item_id,
                &field_ids.priority.field_id,
                &FieldValue::SingleSelectOption(option_id.clone()),
            )
            .await;
    }

    rebuild_cache(&state).await;
    eprintln!("update: A priority=P0, label=urgent");

    // ── Step 11: comment — post comment on A ────────────────────────
    eprintln!("=== Step 11: comment on issue A ===");
    let comment_body = format!("E2E test comment — {test_label}");
    let comment_url = client
        .add_comment(issue_a.number, comment_body.clone())
        .await
        .expect("add_comment should succeed");

    eprintln!("comment: posted on A, url={comment_url}");

    // ── Step 12: show — verify A has comment and label ──────────────
    eprintln!("=== Step 12: show issue A ===");
    let shown_issue = client
        .fetch_issue(issue_a.number)
        .await
        .expect("fetch_issue(A) should succeed");

    // Verify label 'urgent' is present.
    assert!(
        shown_issue.labels.contains(&"urgent".to_owned()),
        "Issue A should have 'urgent' label: {:?}",
        shown_issue.labels,
    );

    // Verify the E2E test comment is present.
    let has_e2e_comment = shown_issue
        .comments
        .iter()
        .any(|c| c.body.contains(&test_label));
    assert!(
        has_e2e_comment,
        "Issue A should have the E2E test comment: {:?}",
        shown_issue
            .comments
            .iter()
            .map(|c| &c.body)
            .collect::<Vec<_>>(),
    );

    eprintln!(
        "show: A verified — labels={:?}, comments={}",
        shown_issue.labels,
        shown_issue.comments.len()
    );

    // ── Step 13: close A — verify B still blocked (has C) ───────────
    eprintln!("=== Step 13: close issue A ===");
    client
        .close_issue(issue_a.number, Some("E2E test: closing A".to_owned()))
        .await
        .expect("close_issue(A) should succeed");

    rebuild_cache(&state).await;

    // Verify B is still blocked (still has C as blocker).
    let ready_set = state
        .cache
        .get_ready_set()
        .await
        .expect("cache should have a ready set");
    let ready_issues = filter_ready_set(&ready_set, &ready_params);
    let ready_numbers: Vec<u64> = ready_issues.iter().map(|i| i.number).collect();

    eprintln!("ready (after close A): numbers={ready_numbers:?}");

    assert!(
        !ready_numbers.contains(&issue_b.number),
        "Issue B (#{}) should NOT be in ready set yet (still blocked by C): {ready_numbers:?}",
        issue_b.number,
    );
    assert!(
        !ready_numbers.contains(&issue_a.number),
        "Issue A (#{}) should NOT be in ready set (closed): {ready_numbers:?}",
        issue_a.number,
    );

    // ── Step 14: close C — verify B now in ready set (cascade) ──────
    eprintln!("=== Step 14: close issue C ===");
    client
        .close_issue(issue_c.number, Some("E2E test: closing C".to_owned()))
        .await
        .expect("close_issue(C) should succeed");

    rebuild_cache(&state).await;

    // Verify B is now in the ready set (all blockers A and C are closed).
    let ready_set = state
        .cache
        .get_ready_set()
        .await
        .expect("cache should have a ready set");

    // For step 15, B may have project fields set to Blocked/Not Ready from step 4,
    // but the graph engine computes readiness from issue state (open/closed) and
    // blocking edges, not from project field values. So B should appear in the
    // ready set even though its project fields say "blocked".
    let ready_params_with_claimed = ReadyParams {
        limit: None,
        issue_type: None,
        priority: None,
        milestone: None,
        agent: None,
        label: Some(test_label.clone()),
        include_claimed: Some(true),
    };
    let ready_issues = filter_ready_set(&ready_set, &ready_params_with_claimed);
    let ready_numbers: Vec<u64> = ready_issues.iter().map(|i| i.number).collect();

    eprintln!("ready (after close C): numbers={ready_numbers:?}");

    assert!(
        ready_numbers.contains(&issue_b.number),
        "Issue B (#{}) should be in ready set (all blockers closed): {ready_numbers:?}",
        issue_b.number,
    );

    // ── Step 15: ready — final verify ───────────────────────────────
    eprintln!("=== Step 15: ready (final check) ===");
    assert!(
        !ready_numbers.contains(&issue_a.number),
        "Issue A (#{}) should NOT be in ready set (closed): {ready_numbers:?}",
        issue_a.number,
    );
    assert!(
        !ready_numbers.contains(&issue_c.number),
        "Issue C (#{}) should NOT be in ready set (closed): {ready_numbers:?}",
        issue_c.number,
    );
    assert!(
        ready_numbers.contains(&issue_b.number),
        "Issue B (#{}) should be in ready set: {ready_numbers:?}",
        issue_b.number,
    );
    eprintln!("ready (final): B in ready set, A and C not — cascade verified!");

    // ── Step 16: Cleanup ────────────────────────────────────────────
    eprintln!("=== Step 16: cleanup ===");
    // Close B explicitly (A and C already closed).
    client
        .close_issue(issue_b.number, Some("E2E test cleanup".to_owned()))
        .await
        .expect("close_issue(B) should succeed for cleanup");

    eprintln!("cleanup: B closed");

    // Disarm the guard — we cleaned up manually.
    guard.disarm();

    eprintln!("=== E2E workflow test PASSED ===");
}
