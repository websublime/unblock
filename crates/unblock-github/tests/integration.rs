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

use std::sync::Arc;

use unblock_core::config::Config;
use unblock_core::types::{IssueState, IssueType, Status};
use unblock_github::client::GitHubClient;
use unblock_github::mutations::CreateIssueParams;
use unblock_github::projects::{CreateViewParams, FieldValue, OwnerType, ViewLayout};

/// Drop guard that closes a GitHub issue on scope exit, even during a panic
/// unwind. This ensures integration tests do not leave orphaned open issues
/// when an assertion fails before the explicit cleanup call.
///
/// The guard captures a shared [`Arc<GitHubClient>`] and the issue number at
/// creation time. On drop it uses [`tokio::spawn`] to fire-and-forget the
/// async `close_issue` call on the current runtime handle, which keeps the
/// destructor runtime-agnostic — it works on both `current_thread` and
/// `multi_thread` tokio flavors. The previous `block_in_place` strategy
/// required a multi-threaded runtime and aborted the process via SIGABRT
/// when an assertion panicked under `#[tokio::test]` (`current_thread` default
/// — bead `unblock-ekf`).
///
/// Caveat: because the close runs as a detached task, it is not awaited
/// before the test process exits. If the runtime is torn down before the
/// task is polled, the close is silently skipped — acceptable for cleanup
/// in a test harness, and the alternative (blocking the destructor) is
/// strictly worse. The same trade-off is documented on
/// [`crate::CloseIssuesGuard`] in `unblock-mcp/tests/e2e_workflow.rs`.
#[derive(Debug)]
struct CloseIssueGuard {
    client: Arc<GitHubClient>,
    issue_number: u64,
    /// Set to `true` once the test completes successfully and the caller has
    /// already cleaned up (or does not need cleanup). When `true`, the guard
    /// skips the `close_issue` call in `Drop`.
    disarmed: bool,
}

impl CloseIssueGuard {
    /// Creates an armed guard that will close `issue_number` on drop.
    fn new(client: &Arc<GitHubClient>, issue_number: u64) -> Self {
        Self {
            client: Arc::clone(client),
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

impl Drop for CloseIssueGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        // Spawn a detached cleanup task on the current runtime. This avoids
        // `block_in_place`, which requires the multi-threaded runtime flavor
        // and aborts the process when invoked under `#[tokio::test]`'s
        // `current_thread` default during a panic-driven unwind.
        let number = self.issue_number;
        let client = Arc::clone(&self.client);
        tokio::spawn(async move {
            if let Err(e) = client
                .close_issue(
                    number,
                    Some("Automated test cleanup (drop guard)".to_owned()),
                )
                .await
            {
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

// ── Live test fixture-label contract (bead `unblock-1hz`) ──────────
//
// Every issue created by a live test MUST be tagged with the canonical
// [`FIXTURE_LABEL`] (`unblock-fixture`) so the
// `scripts/setup-test-project.sh --wipe-issues` cleanup mode can identify
// orphaned fixtures across runs. The wipe script enumerates issues with
// this label exactly — never include a timestamp or run id here, otherwise
// the wipe loses its anchor.
//
// Cycle 1 of bead `unblock-1hz` also added a per-run `unblock-run-<millis>`
// discriminator label, but cycle 2 (Miguel decision 2026-05-05) dropped it:
// the per-run label has the same accumulation problem that views had before
// the stable-name refactor. The repo accumulated tens of `unblock-run-*`
// labels across CI runs, contradicting the bead's purpose. The canonical
// `unblock-fixture` label alone is sufficient for wipe selection; forensic
// correlation across runs is recovered from the issue's timestamp instead.
//
// Tests that already attach a domain label such as `"test"` keep it —
// fixture labels are additive.

/// Canonical fixture marker label. Applied by every live test that creates
/// an issue. The `--wipe-issues` mode in `scripts/setup-test-project.sh`
/// selects issues by this exact name.
pub(crate) const FIXTURE_LABEL: &str = "unblock-fixture";

/// Returns the canonical fixture label set: the stable `unblock-fixture`
/// marker plus any extras. Use as the `labels` field on `CreateIssueParams`
/// to make a live-test-created issue eligible for the `--wipe-issues`
/// cleanup path.
///
/// `extra` is appended after the canonical fixture label so domain-specific
/// labels such as `"test"` stay attached to the created issue.
pub(crate) fn fixture_labels(extra: &[&str]) -> Vec<String> {
    let mut labels = Vec::with_capacity(1 + extra.len());
    labels.push(FIXTURE_LABEL.to_owned());
    labels.extend(extra.iter().map(|s| (*s).to_owned()));
    labels
}

/// Bead `unblock-q1c`: live test fixture project-field populator.
///
/// Idempotently runs `setup_fields` (caches `field_ids` on the client),
/// resolves the project's item ID for the just-created issue, and writes
/// Priority + Status (+ optional Agent / `PipelineStage`) via
/// [`unblock_github::projects::set_project_fields`]. Mirrors the canonical
/// exemplar in `crates/unblock-mcp/tests/e2e_workflow.rs` so live integration
/// fixtures populate the canonical Projects V2 fields after `create_issue`,
/// instead of leaving the live board's Status / Priority / Agent / Pipeline
/// columns empty (defeats the "live documentation" goal of `unblock-wgj.22`).
///
/// Best-effort by design: per-field failures inside `set_project_fields`
/// are logged via `tracing::warn!` and do not return errors. If the issue
/// has no `ProjectV2Item` yet (cross-repo, project not configured), the
/// function returns `None` so the caller can either skip the populate step
/// or treat the missing item as a soft failure.
///
/// **Gating contract.** Callers MUST gate on
/// [`require_github_token_and_project`] before invoking this helper —
/// `setup_fields` requires `UNBLOCK_PROJECT` to be exported and panics in
/// the `resolve_project_info` step otherwise.
async fn populate_project_fields(
    client: &Arc<GitHubClient>,
    issue_node_id: &str,
    priority: &str,
    status_option_name: &str,
    agent: Option<&str>,
    pipeline_stage: Option<&str>,
) -> Option<String> {
    use unblock_github::GitHubApi;

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info should succeed (UNBLOCK_PROJECT exported)");

    // Idempotent — safe to call once per test even if a sibling test in the
    // same process already ran setup_fields. Cache hit short-circuits the
    // per-field round-trips; cache miss does the canonical 7-field setup.
    if client.field_ids().await.is_none() {
        let report = client
            .setup_fields(&project_info.id)
            .await
            .expect("setup_fields should succeed");
        client.set_field_ids(report.field_ids).await;
    }

    let field_ids = client
        .field_ids()
        .await
        .expect("field_ids should be cached after setup_fields + set_field_ids");

    let item_id = client
        .get_project_item_id(issue_node_id, &project_info.id)
        .await
        .ok()?;

    let api: &dyn GitHubApi = client.as_ref();
    unblock_github::projects::set_project_fields(
        api,
        &project_info.id,
        &item_id,
        &field_ids,
        priority,
        status_option_name,
        agent,
        pipeline_stage,
        None,
        None,
    )
    .await;

    Some(item_id)
}

// ── Canonical view names for the create_view live tests ────────────
//
// Per bead `unblock-1hz` decision D3 (option y — reuse stable-name refactor),
// the three create_view tests use fixed names instead of timestamp-suffixed
// unique ones. Project views cannot be deleted via the GitHub API
// (verified against the v2 GraphQL schema and 2026-03-10 REST OpenAPI;
// `docs/archive/research/github-projectsv2-views-api-findings.md`), so
// every fresh-name run was permanently polluting the live test project.
// Each test now uses `list_views` to check for pre-existence and creates
// only when missing — view count stays at exactly 3 fixtures across all
// runs.

const FIXTURE_VIEW_BOARD: &str = "test-board-fixture";
const FIXTURE_VIEW_TABLE: &str = "test-table-fixture";
const FIXTURE_VIEW_ROADMAP: &str = "test-roadmap-fixture";

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
    // This test provisions its own fixture issue rather than assuming any
    // pre-existing issue number exists in the test repo. It creates a fresh
    // issue via `create_issue`, fetches it back via `fetch_issue`, asserts
    // round-trip equality on the observable fields, and unconditionally
    // closes the issue on exit (via `CloseIssueGuard`, which fires even on
    // assertion-driven panic unwinds). The previous form hardcoded
    // `fetch_issue(1)` and broke against repos whose lowest issue number is
    // greater than 1, or whose issues have been deleted between live runs
    // (see beads `unblock-mwg`, `unblock-741`).
    //
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Provision a fresh fixture issue. Mirrors the canonical create-style
    // template at `create_issue_returns_issue_with_correct_fields` so that
    // operators reading the board see a uniform "Automated integration test"
    // body and `test` label across every live-required test that creates
    // issues.
    let create_params = CreateIssueParams {
        title: "Fixture for fetch_issue round-trip integration test".to_owned(),
        body: Some(
            "Automated integration test for the live `fetch_issue` path \
             — safe to close. Provisioned by \
             `fetch_issue_returns_full_details_for_existing_issue`."
                .to_owned(),
        ),
        labels: fixture_labels(&["test"]),
        milestone: None,
        assignees: vec![],
        issue_type: Some(IssueType::Bug.canonical_name().to_owned()),
    };

    let created = client
        .create_issue(create_params)
        .await
        .expect("create_issue() fixture provision should succeed");

    // Arm the drop guard *after* create succeeds (so a create failure does
    // not panic the guard) but *before* the asserts (so an assertion panic
    // still triggers cleanup).
    let mut guard = CloseIssueGuard::new(&client, created.number);

    // Bead `unblock-q1c`: populate Projects V2 fields so the live board
    // shows clean rows. Bug fixture (Appendix B.3) — P0 default.
    populate_project_fields(
        &client,
        &created.node_id,
        "P0",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

    let issue = client
        .fetch_issue(created.number)
        .await
        .expect("fetch_issue() should succeed for the just-created fixture");

    // Round-trip equality on the observable surface of `Issue`.
    assert_eq!(
        issue.number, created.number,
        "fetched issue number should match the created fixture"
    );
    assert_eq!(
        issue.title, "Fixture for fetch_issue round-trip integration test",
        "fetched title should round-trip the create input verbatim"
    );
    assert!(!issue.node_id.is_empty(), "node_id should be non-empty");
    assert!(!issue.url.is_empty(), "url should be non-empty");
    assert!(
        issue.state == IssueState::Open || issue.state == IssueState::Closed,
        "state should be Open or Closed"
    );

    // Explicit cleanup on the happy path; disarm the guard so it does not
    // double-close.
    client
        .close_issue(created.number, Some("Automated test cleanup".to_owned()))
        .await
        .expect("close_issue() cleanup should succeed");
    guard.disarm();

    eprintln!(
        "fetch_issue test: created, fetched, and closed issue #{}",
        created.number
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
        // Status must be one of the known variants. Backlog was added
        // by `unblock-1zj` as the create-time default and is included
        // in the valid set.
        let valid = matches!(
            issue.status,
            Status::Backlog
                | Status::Ready
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
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board AND so the representative pin
    // assertion below (`fetch_field_value` re-fetch) has a project to
    // query.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Realistic Bug fixture (spec Appendix B.3 — unblock-wgj.22).
    // Exercises the `Bug` IssueType + canonical Bug body shape that
    // operators see on the live board.
    let params = CreateIssueParams {
        title: "Fix authentication bypass in /login endpoint".to_owned(),
        body: Some(
            "Automated integration test for the live `create_issue` path \
             — safe to close. Models a high-severity auth regression."
                .to_owned(),
        ),
        labels: fixture_labels(&["test"]),
        milestone: None,
        assignees: vec![],
        issue_type: Some(
            unblock_core::types::IssueType::Bug
                .canonical_name()
                .to_owned(),
        ),
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
        issue.title, "Fix authentication bypass in /login endpoint",
        "title should match what was provided"
    );
    assert!(!issue.node_id.is_empty(), "node_id should be non-empty");
    assert!(!issue.url.is_empty(), "url should be non-empty");
    assert_eq!(
        issue.state,
        IssueState::Open,
        "newly created issue should be Open"
    );

    // Bead `unblock-q1c`: populate Projects V2 fields so the live board
    // shows clean rows AND so this test acts as the representative
    // fields-populated invariant pin point for the `unblock-github` live
    // suite. Bug fixture (Appendix B.3) → P0 + Backlog (canonical
    // create-time Status per spec §8.3) + integration-fixture agent.
    let item_id = populate_project_fields(
        &client,
        &issue.node_id,
        "P0",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await
    .expect("populate_project_fields should resolve a ProjectV2Item ID");

    // Re-fetch the field values via the read path used by the
    // `update_field_changes_value_on_project_item` test. Asserts the
    // populate path actually wrote each field — `set_project_fields` is
    // best-effort and only logs warnings on per-field failures, so the
    // round-trip read is the only way to fail-fast on a regression that
    // breaks the populate flow (bead `unblock-q1c` Risks).
    let resolved_field_ids = client
        .field_ids()
        .await
        .expect("field_ids should be cached after populate_project_fields");

    let priority_value =
        fetch_field_value(&client, &item_id, &resolved_field_ids.priority.field_id).await;
    assert_eq!(
        priority_value.as_deref(),
        Some("P0 - Critical"),
        "Priority should be populated to canonical 'P0 - Critical' after populate_project_fields"
    );

    let status_value =
        fetch_field_value(&client, &item_id, &resolved_field_ids.status.field_id).await;
    assert_eq!(
        status_value.as_deref(),
        Some(Status::Backlog.option_name()),
        "Status should be populated to canonical Backlog after populate_project_fields"
    );

    let agent_value = fetch_field_value(&client, &item_id, &resolved_field_ids.agent).await;
    assert_eq!(
        agent_value.as_deref(),
        Some("integration-fixture"),
        "Agent should be populated to 'integration-fixture' after populate_project_fields"
    );

    // Explicit cleanup on the happy path; disarm the guard so it does not
    // double-close.
    client
        .close_issue(issue.number, Some("Automated test cleanup".to_owned()))
        .await
        .expect("close_issue() cleanup should succeed");
    guard.disarm();

    eprintln!(
        "create_issue test: created, populated fields, asserted re-fetch, and closed issue #{}",
        issue.number
    );
}

// ── mutations: close_issue ──────────────────────────────────────────

#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn close_issue_closes_issue_and_refetch_confirms() {
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Create a Refactor fixture to close (spec Appendix B.3 —
    // unblock-wgj.22). Exercises the `Refactor` IssueType.
    let params = CreateIssueParams {
        title: "Migrate auth middleware to async".to_owned(),
        body: Some(
            "Automated integration test for the live `close_issue` \
             path — will be closed."
                .to_owned(),
        ),
        labels: fixture_labels(&["test"]),
        milestone: None,
        assignees: vec![],
        issue_type: Some(IssueType::Refactor.canonical_name().to_owned()),
    };

    let issue = client
        .create_issue(params)
        .await
        .expect("create_issue() should succeed");

    // Arm the drop guard so the issue is closed even if an assertion panics.
    let mut guard = CloseIssueGuard::new(&client, issue.number);

    // Bead `unblock-q1c`: populate Projects V2 fields. Refactor fixture
    // (Appendix B.3) → P1 + Backlog + integration-fixture agent.
    populate_project_fields(
        &client,
        &issue.node_id,
        "P1",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

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
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Create a Spike fixture to close with a reason (spec Appendix
    // B.3 — unblock-wgj.22). Exercises the `Spike` IssueType +
    // canonical Priority default (P2 — Medium).
    //
    // Bead `unblock-q1c` DRIFT-D closure: `issue_type` populated with
    // canonical Spike (was `None`, contradicting Appendix B.3 "all 8
    // IssueType variants exercised").
    let params = CreateIssueParams {
        title: "Investigate flaky checkout test".to_owned(),
        body: Some(
            "Automated integration test for the live `close_issue` \
             path — will be closed with a reason. Models a time-boxed \
             investigation of an intermittent checkout test failure."
                .to_owned(),
        ),
        labels: fixture_labels(&["test"]),
        milestone: None,
        assignees: vec![],
        issue_type: Some(IssueType::Spike.canonical_name().to_owned()),
    };

    let issue = client
        .create_issue(params)
        .await
        .expect("create_issue() should succeed");

    // Arm the drop guard so the issue is closed even if an assertion panics.
    let mut guard = CloseIssueGuard::new(&client, issue.number);

    // Bead `unblock-q1c`: populate Projects V2 fields. Spike fixture
    // (Appendix B.3) → P2 + Backlog + integration-fixture agent.
    populate_project_fields(
        &client,
        &issue.node_id,
        "P2",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

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
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Create a Docs fixture to comment on (spec Appendix B.3 —
    // unblock-wgj.22). Exercises the `Docs` IssueType.
    let params = CreateIssueParams {
        title: "Document Projects V2 setup workflow".to_owned(),
        body: Some(
            "Automated integration test for the live `add_comment` path \
             — will receive a comment. Models a documentation task."
                .to_owned(),
        ),
        labels: fixture_labels(&["test"]),
        milestone: None,
        assignees: vec![],
        issue_type: Some(IssueType::Docs.canonical_name().to_owned()),
    };

    let issue = client
        .create_issue(params)
        .await
        .expect("create_issue() should succeed");

    // Arm the drop guard so the issue is closed even if an assertion panics.
    let mut guard = CloseIssueGuard::new(&client, issue.number);

    // Bead `unblock-q1c`: populate Projects V2 fields. Docs fixture
    // (Appendix B.3) → P3 + Backlog. Agent intentionally `None` to
    // exercise the §8.3 / §8.6 absence-leaves-unmodified edge case
    // (matches the canonical e2e_workflow.rs pattern for issue C).
    populate_project_fields(
        &client,
        &issue.node_id,
        "P3",
        Status::Backlog.option_name(),
        None,
        None,
    )
    .await;

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
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Create two `Task` fixtures from the OAuth Epic family
    // (spec Appendix B.3 — unblock-wgj.22). A will be blocked by B.
    let issue_a = client
        .create_issue(CreateIssueParams {
            title: "Add OAuth callback handler".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Task.canonical_name().to_owned()),
        })
        .await
        .expect("create_issue A should succeed");

    let mut guard_a = CloseIssueGuard::new(&client, issue_a.number);

    // Bead `unblock-q1c`: populate Projects V2 fields. Task fixture
    // (Appendix B.3) → P2 + Backlog + integration-fixture agent.
    populate_project_fields(
        &client,
        &issue_a.node_id,
        "P2",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

    let issue_b = client
        .create_issue(CreateIssueParams {
            title: "Add OAuth token validation".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Task.canonical_name().to_owned()),
        })
        .await
        .expect("create_issue B should succeed");

    let mut guard_b = CloseIssueGuard::new(&client, issue_b.number);

    populate_project_fields(
        &client,
        &issue_b.node_id,
        "P2",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

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
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Create a `Chore` and a `Refactor` fixture (spec Appendix B.3 —
    // unblock-wgj.22). Exercises both NEW IssueType variants.
    // A will be blocked by B, then unblocked.
    let issue_a = client
        .create_issue(CreateIssueParams {
            title: "Bump dependency versions".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Chore.canonical_name().to_owned()),
        })
        .await
        .expect("create_issue A should succeed");

    let mut guard_a = CloseIssueGuard::new(&client, issue_a.number);

    // Bead `unblock-q1c`: populate Projects V2 fields. Chore fixture
    // (Appendix B.3) → P4 + Backlog + integration-fixture agent.
    populate_project_fields(
        &client,
        &issue_a.node_id,
        "P4",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

    let issue_b = client
        .create_issue(CreateIssueParams {
            title: "Migrate auth middleware to async".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Refactor.canonical_name().to_owned()),
        })
        .await
        .expect("create_issue B should succeed");

    let mut guard_b = CloseIssueGuard::new(&client, issue_b.number);

    // Refactor fixture (Appendix B.3) → P1 + Backlog.
    populate_project_fields(
        &client,
        &issue_b.node_id,
        "P1",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

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
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Create two realistic fixtures (spec Appendix B.3 — unblock-wgj.22).
    let issue_a = client
        .create_issue(CreateIssueParams {
            title: "Investigate flaky checkout test".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Spike.canonical_name().to_owned()),
        })
        .await
        .expect("create_issue A should succeed");

    let mut guard_a = CloseIssueGuard::new(&client, issue_a.number);

    // Bead `unblock-q1c`: populate Projects V2 fields. Spike fixture
    // (Appendix B.3) → P2 + Backlog + integration-fixture agent.
    populate_project_fields(
        &client,
        &issue_a.node_id,
        "P2",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

    let issue_b = client
        .create_issue(CreateIssueParams {
            title: "Fix authentication bypass in /login endpoint".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Bug.canonical_name().to_owned()),
        })
        .await
        .expect("create_issue B should succeed");

    let mut guard_b = CloseIssueGuard::new(&client, issue_b.number);

    // Bug fixture (Appendix B.3) → P0 + Backlog.
    populate_project_fields(
        &client,
        &issue_b.node_id,
        "P0",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

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
    // Bead `unblock-q1c`: gates on `require_github_token_and_project` so
    // the post-create `populate_project_fields` step can write Status /
    // Priority / Agent on the live board.
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    // Spec Appendix B.3 (unblock-wgj.22): Epic + Task hierarchy
    // exemplar — the canonical OAuth login flow Epic with one
    // sub-Task. Exercises the `Epic` and `Task` IssueType variants
    // and the `add_sub_issue` API path.
    let parent = client
        .create_issue(CreateIssueParams {
            title: "Implement OAuth login flow".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Epic.canonical_name().to_owned()),
        })
        .await
        .expect("create parent should succeed");

    let mut guard_parent = CloseIssueGuard::new(&client, parent.number);

    // Bead `unblock-q1c`: populate Projects V2 fields. Epic parent
    // (Appendix B.3 — Implement OAuth login flow) → P1 + Backlog +
    // integration-fixture agent.
    populate_project_fields(
        &client,
        &parent.node_id,
        "P1",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

    let child = client
        .create_issue(CreateIssueParams {
            title: "Add OAuth callback handler".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Task.canonical_name().to_owned()),
        })
        .await
        .expect("create child should succeed");

    let mut guard_child = CloseIssueGuard::new(&client, child.number);

    // Task child (Appendix B.3) → P2 + Backlog.
    populate_project_fields(
        &client,
        &child.node_id,
        "P2",
        Status::Backlog.option_name(),
        Some("integration-fixture"),
        None,
    )
    .await;

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

    // Verify single-select fields have the correct options. Post-
    // `unblock-1zj` the canonical Status options are 6 TitleCase
    // strings sourced from `Status::option_name`, in board order.
    assert_eq!(
        field_ids.status.options.len(),
        Status::ALL.len(),
        "Status should have {} options, got: {:?}",
        Status::ALL.len(),
        field_ids.status.options.keys().collect::<Vec<_>>()
    );
    for variant in Status::ALL {
        let expected = variant.option_name();
        assert!(
            field_ids.status.options.contains_key(expected),
            "Status should have option {expected:?}, got: {:?}",
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

// This test intentionally exercises `GitHubClient::update_field` directly
// rather than going through the `populate_project_fields` helper introduced
// in unblock-q1c. `update_field` is the unit-under-test here: this is the
// only live coverage that pins its single-field write contract (Priority
// SingleSelect + Agent Text re-fetch round-trip, including the
// `option_id_by_prefix` resolution path documented in unblock-ekf). The
// q1c migration (set_project_fields → populate_project_fields wrapper)
// targeted FIXTURE sites that create an issue and then leave its custom
// fields blank — this test is not such a site; it is the dedicated
// `update_field` regression test and must keep calling `update_field`
// directly so a regression there fails this assertion rather than being
// masked by the higher-level helper. Do NOT migrate this call to
// `populate_project_fields`.
#[tokio::test]
#[ignore = "live GitHub API — opt-in via `cargo test --workspace -- --ignored` with GITHUB_TOKEN set"]
async fn update_field_changes_value_on_project_item() {
    if !require_github_token_and_project() {
        return;
    }

    let config = test_config();
    let client = Arc::new(
        GitHubClient::new(&config)
            .await
            .expect("GitHubClient::new() should succeed"),
    );

    let project_info = client
        .resolve_project_info()
        .await
        .expect("resolve_project_info() should succeed");

    let report = client
        .setup_fields(&project_info.id)
        .await
        .expect("setup_fields() should succeed");
    let field_ids = &report.field_ids;

    // Create a `Feature` fixture to test field updates on
    // (spec Appendix B.3 — unblock-wgj.22). The OAuth login flow Epic
    // counterpart, here written as a Feature for variety so the
    // corpus exercises every IssueType variant at least once.
    let issue = client
        .create_issue(CreateIssueParams {
            title: "Implement OAuth login flow".to_owned(),
            body: Some("Automated test — safe to close.".to_owned()),
            labels: fixture_labels(&["test"]),
            milestone: None,
            assignees: vec![],
            issue_type: Some(IssueType::Feature.canonical_name().to_owned()),
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
    //
    // The HashMap on `FieldMeta::options` is keyed by the canonical option
    // name from `REQUIRED_FIELDS` (e.g. `"P1 - High"`), so a bare `"P1"`
    // lookup never matches. Use `option_id_by_prefix` which falls back to a
    // prefix match — robust to future REQUIRED_FIELDS option renames
    // (bead `unblock-ekf` Bug #1, decision option (b)).
    let p1_option_id = field_ids
        .priority
        .option_id_by_prefix("P1")
        .expect("P1 option should exist (prefix match against REQUIRED_FIELDS)");

    client
        .update_field(
            &project_info.id,
            &item_id,
            &field_ids.priority.field_id,
            &FieldValue::SingleSelectOption(p1_option_id.clone()),
        )
        .await
        .expect("update_field(Priority=P1) should succeed");

    // Re-fetch to confirm the Priority value was persisted. `fetch_field_value`
    // returns the canonical option NAME from GraphQL — `"P1 - High"` per the
    // REQUIRED_FIELDS spec — not the short code (bead `unblock-ekf` Bug #2).
    let priority_value = fetch_field_value(&client, &item_id, &field_ids.priority.field_id).await;
    assert_eq!(
        priority_value.as_deref(),
        Some("P1 - High"),
        "Re-fetched Priority should be the canonical name 'P1 - High'"
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
//
// View accumulation is an upstream constraint, not a bug we can fix:
// GitHub Projects V2 has no public API to delete a project view. The
// GraphQL schema exposes `deleteProjectV2Item` but no `deleteProjectV2View`
// (verified against `docs/archive/research/schema.docs.graphql`), and the
// REST API has no `DELETE /views` endpoint (see
// `docs/archive/research/github-projectsv2-views-api-findings.md`). Views
// can only be removed via the GitHub Web UI — manual operator action.
//
// To keep the live test project from drowning in `test-board-<ts>` /
// `test-table-<ts>` / `test-roadmap-<ts>` orphans, the three tests below
// reuse fixed canonical fixture names ([`FIXTURE_VIEW_BOARD`],
// [`FIXTURE_VIEW_TABLE`], [`FIXTURE_VIEW_ROADMAP`]) and call `list_views`
// to detect pre-existence. On the first run the fixture views are created;
// on every subsequent run the tests verify the existing fixture's layout
// and filter (the `create_view` call is skipped). View count stays at
// exactly 3 fixtures across all runs (bead `unblock-1hz` decision D3,
// option y — reuse stable-name refactor; bead Risk R1).
//
// Operators: if a fixture view drifts (wrong layout, accidental rename),
// delete it manually via the GitHub Web UI and the next live test run
// will recreate it from canonical params.

/// Looks up an existing view by name on the live test project. Returns
/// `Some(layout)` if the view exists, `None` otherwise. Used by the three
/// `create_view_*` tests to decide whether to create the canonical
/// fixture view or assert against the already-present one.
async fn lookup_fixture_view_layout(
    client: &GitHubClient,
    owner_type: OwnerType,
    name: &str,
) -> Option<ViewLayout> {
    let views = client
        .list_views(owner_type)
        .await
        .expect("list_views() should succeed");
    views.iter().find(|v| v.name == name).map(|v| v.layout)
}

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

    let view_name = FIXTURE_VIEW_BOARD;

    // Create the canonical board fixture view only when it is missing.
    // Subsequent runs reuse the existing one so view count stays bounded
    // (the GitHub API has no delete-view path; see module-level comment
    // and bead `unblock-1hz` decision D3).
    if let Some(existing_layout) = lookup_fixture_view_layout(&client, owner_type, view_name).await
    {
        assert_eq!(
            existing_layout,
            ViewLayout::Board,
            "fixture view '{view_name}' should be a Board view; if it has \
             drifted, delete it manually via the GitHub Web UI and re-run"
        );
        eprintln!("create_view_board_and_list_views: reusing existing fixture view '{view_name}'");
    } else {
        let params = CreateViewParams {
            name: view_name.to_owned(),
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
            "Created board fixture view: name={}, number={}, id={:?}",
            view.name, view.number, view.id
        );
    }

    // Verify the view appears in list_views (post-condition holds whether
    // we just created it or reused the existing fixture).
    let views = client
        .list_views(owner_type)
        .await
        .expect("list_views() should succeed");

    let found = views.iter().any(|v| v.name == view_name);
    assert!(
        found,
        "list_views should include the canonical fixture view '{view_name}'"
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

    let view_name = FIXTURE_VIEW_TABLE;

    if let Some(existing_layout) = lookup_fixture_view_layout(&client, owner_type, view_name).await
    {
        assert_eq!(
            existing_layout,
            ViewLayout::Table,
            "fixture view '{view_name}' should be a Table view; if it has \
             drifted, delete it manually via the GitHub Web UI and re-run"
        );
        eprintln!("create_view_table_layout: reusing existing fixture view '{view_name}'");
        return;
    }

    let params = CreateViewParams {
        name: view_name.to_owned(),
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
        "Created table fixture view: name={}, number={}",
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

    let view_name = FIXTURE_VIEW_ROADMAP;

    if let Some(existing_layout) = lookup_fixture_view_layout(&client, owner_type, view_name).await
    {
        assert_eq!(
            existing_layout,
            ViewLayout::Roadmap,
            "fixture view '{view_name}' should be a Roadmap view; if it \
             has drifted, delete it manually via the GitHub Web UI and re-run"
        );
        eprintln!("create_view_roadmap_layout: reusing existing fixture view '{view_name}'");
        return;
    }

    let params = CreateViewParams {
        name: view_name.to_owned(),
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
        "Created roadmap fixture view: name={}, number={}",
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
