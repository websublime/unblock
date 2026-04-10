//! Shared test helpers for `unblock-mcp` integration tests.
//!
//! Extracted to avoid duplication across `integration.rs`, `dyn_dispatch.rs`,
//! and `e2e_workflow.rs`.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_github::GitHubApi;
use unblock_github::client::GitHubClient;
use unblock_github::mock::MockGitHubClient;
use unblock_mcp::server::ServerState;

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

/// Builds a [`Config`] from the process environment for integration tests.
#[allow(dead_code)]
pub fn test_config() -> Config {
    Config::load().expect("Config::load() should succeed when GITHUB_TOKEN is set")
}

/// Creates a [`ServerState`] with a real client and empty cache.
#[allow(dead_code)]
pub async fn test_server_state() -> ServerState {
    let config = test_config();
    let client = GitHubClient::new(&config)
        .await
        .expect("GitHubClient::new() should succeed");
    ServerState {
        config: Arc::new(config),
        github: Arc::new(client),
        cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
        agent_kind: OnceLock::new(),
        agent_client: OnceLock::new(),
        connected_at: OnceLock::new(),
    }
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
