//! Shared test helpers for `unblock-mcp` integration tests.
//!
//! Extracted to avoid duplication across `integration.rs`, `dyn_dispatch.rs`,
//! and `e2e_workflow.rs`.

use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_github::GitHubApi;
use unblock_github::client::GitHubClient;
use unblock_github::mock::MockGitHubClient;
use unblock_mcp::server::ServerState;

// ── Live test fixture-label contract (bead `unblock-1hz`) ──────────
//
// Every issue created by a live test MUST be tagged with the canonical
// [`FIXTURE_LABEL`] (`unblock-fixture`) so the
// `scripts/setup-test-project.sh --wipe-issues` cleanup mode can identify
// orphaned fixtures across runs. See the parallel definition in
// `crates/unblock-github/tests/integration.rs` for the full rationale —
// this module duplicates the helper because Cargo integration test
// binaries cannot share private modules across crates without a workspace
// dev-dependency, and a bespoke "test-utils" crate would be heavier than
// the handful of lines duplicated here.
//
// Cycle 1 of bead `unblock-1hz` also added a per-run `unblock-run-<millis>`
// discriminator label, but cycle 2 (Miguel decision 2026-05-05) dropped
// it: the per-run label was accumulating in the test repo (~7 occurrences
// per CI run, no API to delete labels in bulk except through this script's
// new `--wipe-labels` mode), defeating the bead's purpose. The canonical
// `unblock-fixture` label alone is sufficient for wipe selection.

/// Canonical fixture marker label. Applied by every live test that creates
/// an issue. The `--wipe-issues` mode in `scripts/setup-test-project.sh`
/// selects issues by this exact name.
#[allow(dead_code)] // Not every test binary creates issues
pub const FIXTURE_LABEL: &str = "unblock-fixture";

/// Canonical per-test discriminator label. Replaces the per-run
/// `e2e-test-<millis>` and `test-label-<ts>` labels that accumulated across
/// CI runs (cycle 2 of bead `unblock-1hz`). Tests that need a label distinct
/// from the wipe anchor (e.g. to filter ready/list results to fixtures
/// created by the current process or to exercise `ensure_labels`) reuse
/// this single canonical name. The pre-test wipe in
/// `scripts/setup-test-project.sh --wipe-issues` already guarantees the
/// project is clean of prior fixture issues, so a stable label per-run is
/// sufficient — there is nothing for it to collide with.
#[allow(dead_code)] // Used by e2e_workflow.rs and the ensure_labels integration test
pub const TEST_LABEL: &str = "unblock-test-label";

/// Returns the canonical fixture label set: the stable `unblock-fixture`
/// marker plus any extras. Use as the `labels` field on `CreateIssueParams`
/// to make a live-test-created issue eligible for the `--wipe-issues`
/// cleanup path.
///
/// `extra` is appended after the canonical fixture label so domain-specific
/// labels such as `"test"` or [`TEST_LABEL`] stay attached to the created
/// issue.
#[allow(dead_code)]
#[must_use]
pub fn fixture_labels(extra: &[&str]) -> Vec<String> {
    let mut labels = Vec::with_capacity(1 + extra.len());
    labels.push(FIXTURE_LABEL.to_owned());
    labels.extend(extra.iter().map(|s| (*s).to_owned()));
    labels
}

/// Returns `true` if the `GITHUB_TOKEN` env var is set and non-empty.
#[allow(dead_code)] // Not every test binary uses this
pub fn has_github_token() -> bool {
    std::env::var("GITHUB_TOKEN")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Returns `true` if the `UNBLOCK_PROJECT` env var is set and non-empty.
#[allow(dead_code)] // Used by e2e_workflow.rs but not integration.rs
pub fn has_project_number() -> bool {
    std::env::var("UNBLOCK_PROJECT")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Gate for live-required integration tests: returns `true` when
/// `GITHUB_TOKEN` is set, otherwise prints a clear opt-in hint and returns
/// `false` so the caller can early-return cleanly.
///
/// Live-required tests are tagged `#[ignore]` and opt-in via
/// `cargo test --workspace -- --ignored` with `GITHUB_TOKEN` + `UNBLOCK_REPO`
/// set. When a user explicitly passes `--ignored` without a token this helper
/// keeps the test exit status clean instead of hitting a confusing assertion
/// failure.
///
/// Mirrors [`unblock_github::tests::integration::require_github_token`] —
/// see bead `unblock-c4h` for the full rationale.
#[allow(dead_code)] // Not every test binary uses this
pub fn require_github_token() -> bool {
    if has_github_token() {
        true
    } else {
        eprintln!(
            "GITHUB_TOKEN not set — skipping live integration test (re-run with \
             `GITHUB_TOKEN=... UNBLOCK_REPO=owner/repo cargo test --workspace -- --ignored`)"
        );
        false
    }
}

/// Same as [`require_github_token`] but also requires `UNBLOCK_PROJECT`. Used
/// by Projects V2 live tests that cannot run without a configured project.
#[allow(dead_code)] // Used by e2e_workflow.rs and the project-gated tests in integration.rs
pub fn require_github_token_and_project() -> bool {
    if has_github_token() && has_project_number() {
        true
    } else {
        eprintln!(
            "GITHUB_TOKEN or UNBLOCK_PROJECT not set — skipping live project \
             integration test (re-run with both env vars set and \
             `UNBLOCK_REPO=owner/repo cargo test --workspace -- --ignored`)"
        );
        false
    }
}

/// Builds a [`Config`] from the process environment for integration tests.
#[allow(dead_code)]
pub fn test_config() -> Config {
    Config::load().expect("Config::load() should succeed when GITHUB_TOKEN is set")
}

/// Creates a [`ServerState`] with a real client and empty cache.
///
/// Constructs the underlying [`GitHubClient`] via [`GitHubClient::with_repo`]
/// using the explicit `owner/repo` from `config.repo` (i.e. `UNBLOCK_REPO`).
/// This avoids any dependency on a reachable `.git/config` so that
/// `cargo test -p unblock-mcp` runs cleanly from the member crate directory.
///
/// **Panics** if `UNBLOCK_REPO` is not set on the loaded config or if the
/// underlying [`GitHubClient::with_repo`] call fails. Live integration tests
/// must export `UNBLOCK_REPO=owner/repo` alongside `GITHUB_TOKEN` and gate on
/// [`require_github_token`] / [`require_github_token_and_project`] before
/// calling this helper, which makes the panic structurally unreachable from
/// the test surface. The production `GitHubClient::new` git-remote resolution
/// path remains covered by `unblock-github`'s own integration tests
/// (`github_client_new_connects_to_real_repo`) — it is intentionally
/// unreachable from `unblock-mcp`'s test surface to remove the
/// `.git/config` relative-path footgun.
#[allow(dead_code)]
pub async fn test_server_state() -> ServerState {
    let config = test_config();
    let client = build_github_client(&config)
        .await
        .unwrap_or_else(|e| panic!("test_server_state: build_github_client failed: {e}"));
    ServerState {
        config: Arc::new(config),
        github: Arc::new(client),
        cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
        agent_kind: OnceLock::new(),
        agent_client: OnceLock::new(),
        connected_at: OnceLock::new(),
    }
}

/// Build a real [`GitHubClient`] from `config` via [`GitHubClient::with_repo`].
///
/// Returns [`Err`] with a descriptive message when `config.repo` is not set
/// (i.e. `UNBLOCK_REPO=owner/repo` was not exported) or when the underlying
/// [`GitHubClient::with_repo`] call fails. This mirrors the boolean-gate
/// ergonomic of [`require_github_token`] / [`require_github_token_and_project`]
/// — callers can short-circuit cleanly without a panic-unwind.
///
/// Live `unblock-mcp` tests must not fall back to the `.git/config`-reading
/// [`GitHubClient::new`] path because `cargo test -p unblock-mcp` runs the
/// test binaries from the member crate directory where `.git/config` is not
/// reachable. The production `GitHubClient::new` resolution path is covered
/// end-to-end by `unblock-github`'s integration tests instead.
///
/// The `owner/repo` parsing here intentionally matches
/// [`unblock_core::config::Config::repo`]'s validation invariant — the
/// string is guaranteed to contain exactly one `/` with non-empty
/// segments on each side at config-load time, so `split_once('/')` is
/// total here without further validation.
#[allow(dead_code)]
pub async fn build_github_client(config: &Config) -> Result<GitHubClient, String> {
    let repo = config.repo.as_deref().ok_or_else(|| {
        "UNBLOCK_REPO must be set for unblock-mcp live integration tests \
         (export UNBLOCK_REPO=owner/repo alongside GITHUB_TOKEN)"
            .to_owned()
    })?;
    let (owner, name) = repo.split_once('/').ok_or_else(|| {
        "UNBLOCK_REPO must be in owner/repo form (validated at Config::load)".to_owned()
    })?;
    GitHubClient::with_repo(config, owner, name)
        .await
        .map_err(|e| format!("GitHubClient::with_repo() failed for {owner}/{name}: {e}"))
}

// ── Mock helpers ──────────────────────────────────────────────────────

/// Build a minimal, deterministic [`Config`] without touching the
/// environment. Suitable for tests that use [`MockGitHubClient`].
#[allow(dead_code)] // Not every test file uses every helper
pub fn mock_test_config() -> Config {
    Config::load_from(|key| match key {
        "GITHUB_TOKEN" => Ok("ghp_mock_token".to_owned()),
        "UNBLOCK_REPO" => Ok("acme/widgets".to_owned()),
        _ => Err(std::env::VarError::NotPresent),
    })
    .expect("mock test config should load")
}

/// Build a fresh [`MockGitHubClient`] with fixed coordinates `acme/widgets`.
#[allow(dead_code)]
pub fn new_mock() -> Arc<MockGitHubClient> {
    Arc::new(MockGitHubClient::new("acme", "widgets", Some(1)))
}

/// Wrap a mock in a [`ServerState`] whose `github` field is typed as
/// `Arc<dyn GitHubApi>`, so handler calls traverse the dyn-dispatch vtable.
#[allow(dead_code)]
pub fn state_with_mock(mock: Arc<MockGitHubClient>) -> ServerState {
    let config = mock_test_config();
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

// ── Tracing-capture helper ─────────────────────────────────────────────
//
// Mirrors the private `TracingCapture` inside `server.rs`'s unit-test
// module. Lifted here so integration tests that need to inspect the
// `tracing::warn!` contract (structured field names, specifically
// `cascaded_qid` on the Phase-3 cascade best-effort paths — see
// `server.rs:1114-1118`) can capture output without duplicating the
// machinery at every call site.

/// Shared buffer for capturing tracing output in tests.
///
/// Wraps an `Arc<Mutex<Vec<u8>>>` and implements [`io::Write`] so it can
/// be used as a `tracing_subscriber` writer. Call [`TracingCapture::new`]
/// to create an instance, [`TracingCapture::subscriber`] to build a JSON
/// subscriber wired to the buffer, and [`TracingCapture::output`] to
/// retrieve a snapshot of the captured output as a `String` with UTF-8
/// validated once at construction time.
#[allow(dead_code)] // Not every test binary uses the capture helper
#[derive(Clone)]
pub struct TracingCapture(Arc<Mutex<Vec<u8>>>);

/// Owned snapshot of the captured tracing output, validated as UTF-8
/// once at construction time.
///
/// Returned by [`TracingCapture::output`]. UTF-8 validation happens
/// exactly once when the snapshot is created; subsequent [`Deref`]
/// calls are zero-cost borrows. The mutex is released immediately after
/// the snapshot is taken.
#[allow(dead_code)]
pub struct CapturedOutput(String);

impl std::ops::Deref for CapturedOutput {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CapturedOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

#[allow(dead_code)]
impl TracingCapture {
    /// Create a new, empty capture buffer.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    /// Build a JSON tracing subscriber that writes to this buffer.
    pub fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync {
        let writer = self.clone();
        tracing_subscriber::registry().with(
            fmt::layer()
                .json()
                .with_writer(move || writer.clone())
                .with_target(false),
        )
    }

    /// Return a snapshot of the captured output as a validated UTF-8
    /// string.
    ///
    /// UTF-8 validation is performed exactly once when the snapshot is
    /// created. The mutex is released immediately after copying the
    /// buffer, so callers can inspect the output without holding the
    /// lock.
    ///
    /// # Panics
    ///
    /// Panics if the mutex is poisoned or the buffer is not valid UTF-8.
    #[must_use]
    pub fn output(&self) -> CapturedOutput {
        let bytes = self
            .0
            .lock()
            .expect("tracing capture mutex poisoned")
            .clone();
        CapturedOutput(String::from_utf8(bytes).expect("captured output is not valid UTF-8"))
    }
}

impl Default for TracingCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl io::Write for TracingCapture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("tracing capture mutex poisoned")
            .write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .expect("tracing capture mutex poisoned")
            .flush()
    }
}
