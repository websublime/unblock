//! # unblock-mcp
//!
//! MCP server binary for dependency-aware task tracking powered by GitHub.
//!
//! Connects to GitHub via `unblock-github`, builds a dependency graph via `unblock-core`,
//! and exposes MCP tools over stdio transport.

use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt as _;
use snafu::ResultExt as _;
use tracing::info;
use tracing_subscriber::EnvFilter;
use unblock_core::cache::GraphCache;
use unblock_core::config::Config;
use unblock_github::client::GitHubClient;
use unblock_mcp::errors::{ClientInitSnafu, ConfigLoadSnafu, RuntimeSnafu, TransportSnafu};
use unblock_mcp::server::{ServerState, UnblockServer};

#[tokio::main]
async fn main() -> Result<(), unblock_mcp::errors::BootstrapError> {
    // 1. Load configuration from environment variables.
    let config = Config::load().context(ConfigLoadSnafu)?;

    // 2. Initialize tracing subscriber: JSON format, output to stderr, level from config.
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::new(&config.log_level))
        .with_writer(std::io::stderr)
        .init();

    // 3. Create GitHub client — resolves repo and project, exits on failure.
    let client = GitHubClient::new(&config).await.context(ClientInitSnafu)?;

    // 4. Create graph cache with TTL from configuration.
    let cache = GraphCache::new(Duration::from_secs(config.cache_ttl));

    // 5. Build server state.
    let state = ServerState {
        config: Arc::new(config),
        client: Arc::new(client),
        cache: Arc::new(cache),
        agent_kind: std::sync::OnceLock::new(),
    };

    let repo_slug = format!("{}/{}", state.client.owner(), state.client.repo());
    info!(
        server_name = "unblock",
        version = env!("CARGO_PKG_VERSION"),
        repo = %repo_slug,
        project = ?state.client.project_number(),
        "Starting unblock MCP server"
    );

    // 6. Build and serve the MCP server on stdio.
    let server = UnblockServer::new(state);
    let running = server
        .serve(rmcp::transport::io::stdio())
        .await
        .context(TransportSnafu)?;

    // 7. Block until the client disconnects.
    running.waiting().await.context(RuntimeSnafu)?;

    Ok(())
}
