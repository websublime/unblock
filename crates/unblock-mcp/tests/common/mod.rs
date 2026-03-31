//! Shared test helpers for `unblock-mcp` integration tests.
//!
//! Extracted to avoid duplication across `integration.rs` and `e2e_workflow.rs`.

use std::sync::Arc;
use std::time::Duration;

use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_github::client::GitHubClient;
use unblock_mcp::server::ServerState;

/// Returns `true` if the `GITHUB_TOKEN` env var is set and non-empty.
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
pub fn test_config() -> Config {
    Config::load().expect("Config::load() should succeed when GITHUB_TOKEN is set")
}

/// Creates a [`ServerState`] with a real client and empty cache.
pub async fn test_server_state() -> ServerState {
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
