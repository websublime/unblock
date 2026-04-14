//! MCP tool handlers and shared execution helpers.
//!
//! Each tool is a function that validates input, executes the operation,
//! and returns an MCP result. Write tools invalidate the cache and rebuild
//! the graph after mutation.
//!
//! ## Execution Helpers
//!
//! All tool handlers use the shared execution pattern defined here:
//!
//! - `execute_read_tool` — for tools that only read data (e.g., `ready`, `show`, `depends`).
//!   Simply calls the operation and maps errors to `ErrorData`.
//!
//! - `execute_write_tool` — for tools that mutate state (e.g., `create`, `close`, `update`).
//!   After a successful mutation: invalidates the cache, fetches fresh graph data,
//!   rebuilds the dependency graph and ready set, and updates the cache.
//!
//! No tool should implement its own rebuild/invalidate logic.
//!
//! ## Tool Categories
//!
//! ### Core Workflow (6)
//! `ready`, `claim`, `create`, `update`, `close`, `reopen`
//!
//! ### Dependencies (4)
//! `depends`, `dep_remove`, `dep_cycles`, `comment`
//!
//! ### Query (4)
//! `show`, `list`, `search`, `stats`
//!
//! ### Setup & Diagnostics (5)
//! `init`, `setup`, `doctor`, `prime`, `reconcile`

pub mod claim;
pub mod close;
pub mod comment;
pub mod create;
pub mod depends;
pub mod init;
pub mod list;
pub mod prime;
pub mod ready;
pub mod reconcile;
pub mod search;
pub mod setup;
pub mod show;
pub mod update;

use std::future::Future;

use rmcp::model::ErrorData;
use tracing::instrument;
use unblock_core::graph::DependencyGraph;

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;

/// Normalize an optional string filter: empty or whitespace-only values become `None`.
///
/// Serde deserializes `"agent": ""` as `Some("")`, not `None`. Without
/// normalization, an empty string silently matches nothing and returns
/// empty results. This helper collapses empty and whitespace-only strings
/// to `None` so filters behave as if the parameter was omitted.
#[must_use]
pub(crate) fn normalize_filter(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.trim().is_empty())
}

/// Executes a read-only MCP tool operation.
///
/// Calls `op` and maps any `unblock_github::errors::Error` to an
/// `ErrorData` via `github_error_to_mcp`.
///
/// Read tools do not touch the cache — they return data directly from GitHub
/// or from the cached graph state.
///
/// # Parameters
///
/// - `_state`: currently unused, but retained intentionally as part of the
///   stable read-tool signature. Future read handlers will need access to
///   [`ServerState`] to consult the cached [`DependencyGraph`] (e.g. for
///   `ready`, `show`, and `blocked` tools that should serve cached results
///   rather than re-fetching from GitHub). Keeping the parameter in place
///   now avoids a breaking signature change to every read tool call site
///   when cache-backed reads land. Do not remove it.
/// - `op`: the async operation producing the tool result.
///
/// # Errors
///
/// Returns [`ErrorData`] if the operation fails.
#[allow(dead_code)] // Used by tool handlers added in beads 45a.3–45a.11.
#[instrument(skip_all, name = "execute_read_tool")]
pub(crate) async fn execute_read_tool<F, Fut, R>(
    _state: &ServerState,
    op: F,
) -> Result<R, ErrorData>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<R, unblock_github::errors::Error>>,
{
    op().await.map_err(github_error_to_mcp)
}

/// Executes a write MCP tool operation with cache invalidation and rebuild.
///
/// After a successful mutation:
/// 1. Invalidates the cache (sets it to `None`).
/// 2. Fetches fresh graph data from GitHub via `client.fetch_graph_data()`.
/// 3. Builds a new [`DependencyGraph`] from the fetched issues and edges.
/// 4. Computes the new ready set from the graph.
/// 5. Updates the cache with the new ready set and graph.
/// 6. Returns the operation result.
///
/// If the operation fails, the cache is left untouched.
///
/// If the cache rebuild fails (e.g., network error fetching graph data), the
/// cache remains invalidated (empty) and the operation result is still returned.
/// This ensures the mutation is not lost — the cache will be rebuilt on the next
/// read.
///
/// # Design Invariant
///
/// The cache write lock is never held across GitHub API calls. The sequence is:
/// `invalidate()` (quick lock/release) -> `fetch_graph_data()` (network) ->
/// `update()` (quick lock/release).
///
/// # Errors
///
/// Returns [`ErrorData`] if the mutation operation itself fails.
#[allow(dead_code)] // Used by tool handlers added in beads 45a.3–45a.11.
#[instrument(skip_all, name = "execute_write_tool")]
pub(crate) async fn execute_write_tool<F, Fut, R>(
    state: &ServerState,
    op: F,
) -> Result<R, ErrorData>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<R, unblock_github::errors::Error>>,
{
    let result = op().await.map_err(github_error_to_mcp)?;

    // Operation succeeded — invalidate and rebuild.
    rebuild_cache(state).await;

    Ok(result)
}

/// Invalidates the cache, fetches fresh graph data, and rebuilds the cache.
///
/// If the fetch or rebuild fails, the cache is left invalidated (empty). This is
/// intentional — the mutation already succeeded, and the cache will be rebuilt on
/// the next read. The error is logged but not propagated.
///
/// The cache write lock is never held across the GitHub API call:
/// `invalidate()` releases the lock, then `fetch_graph_data()` runs (network),
/// then `update()` acquires the lock again.
pub async fn rebuild_cache(state: &ServerState) {
    state.cache.invalidate().await;

    match state.github.fetch_graph_data().await {
        Ok((issues, edges)) => {
            let graph = DependencyGraph::build(&issues, &edges);
            let ready_set = graph.compute_ready_set(&issues);
            state.cache.update(ready_set, graph).await;
            tracing::debug!("Cache rebuilt after write tool execution");
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "Failed to rebuild cache after write tool — cache left invalidated"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::Utc;
    use rmcp::model::ErrorCode;
    use unblock_core::cache::GraphCache;
    use unblock_core::config::Config;
    use unblock_core::graph::DependencyGraph;
    use unblock_core::types::{BlockingEdge, Issue, IssueState, IssueType, Priority, Status};
    use unblock_github::errors::{GitHubApiSnafu, ProjectNotConfiguredSnafu};

    use super::*;

    // ── Test helpers ───────────────────────────────────────────────────

    use unblock_core::types::QualifiedId;

    /// Helper to create a `QualifiedId` for tests.
    fn qid(number: u64) -> QualifiedId {
        QualifiedId::new("test", "repo", number)
    }

    /// Build a minimal `Issue` for testing.
    fn test_issue(number: u64, state: IssueState) -> Issue {
        Issue {
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
            url: format!("https://github.com/test/repo/issues/{number}"),
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    /// Populate a cache with test data (2 issues, 1 blocking edge).
    /// Issue #1 blocks issue #2, so only issue #1 is in the ready set.
    async fn populate_cache(cache: &GraphCache) {
        let issues = vec![
            test_issue(1, IssueState::Open),
            test_issue(2, IssueState::Open),
        ];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let ready_set = graph.compute_ready_set(&issues);
        cache.update(ready_set, graph).await;
    }

    /// Create a `ServerState` with a real cache but a client that points
    /// at a non-existent host. Tests that need the rebuild path should
    /// test the individual rebuild steps, not call the full `execute_write_tool`.
    async fn test_state() -> ServerState {
        let config = Config::load_from(|key| match key {
            "GITHUB_TOKEN" => Ok("ghp_test_token_for_unit_tests".to_owned()),
            "UNBLOCK_REPO" => Ok("test-owner/test-repo".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        })
        .expect("test config should load");

        let client = unblock_github::client::GitHubClient::new(&config)
            .await
            .expect("test client should initialize");

        ServerState {
            config: Arc::new(config),
            github: Arc::new(client) as Arc<dyn unblock_github::GitHubApi>,
            cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
            agent_kind: std::sync::OnceLock::new(),
            agent_client: std::sync::OnceLock::new(),
            connected_at: std::sync::OnceLock::new(),
        }
    }

    // ── normalize_filter tests ──────────────────────────────────────────

    #[test]
    fn normalize_filter_none_stays_none() {
        assert_eq!(normalize_filter(None), None);
    }

    #[test]
    fn normalize_filter_non_empty_preserved() {
        assert_eq!(normalize_filter(Some("agent-x")), Some("agent-x"));
    }

    #[test]
    fn normalize_filter_empty_string_becomes_none() {
        assert_eq!(normalize_filter(Some("")), None);
    }

    #[test]
    fn normalize_filter_whitespace_only_becomes_none() {
        assert_eq!(normalize_filter(Some("   ")), None);
        assert_eq!(normalize_filter(Some("\t\n")), None);
    }

    #[test]
    fn normalize_filter_preserves_whitespace_padded_value() {
        // A value with surrounding whitespace is preserved (not trimmed) —
        // only purely-whitespace strings are collapsed to None.
        assert_eq!(normalize_filter(Some(" agent-x ")), Some(" agent-x "));
    }

    // ── execute_read_tool tests ────────────────────────────────────────

    #[tokio::test]
    async fn read_tool_success_returns_result() {
        let state = test_state().await;
        let result = execute_read_tool(&state, || async {
            Ok::<_, unblock_github::errors::Error>(42)
        })
        .await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn read_tool_error_maps_to_error_data() {
        let state = test_state().await;
        let result = execute_read_tool(&state, || async {
            Err::<u32, _>(ProjectNotConfiguredSnafu.build())
        })
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("setup"),
            "message should mention setup: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn read_tool_preserves_error_message() {
        let state = test_state().await;
        let result = execute_read_tool(&state, || async {
            Err::<u32, _>(
                GitHubApiSnafu {
                    status: 404_u16,
                    message: "Not Found".to_owned(),
                }
                .build(),
            )
        })
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("Not Found"),
            "message should contain API error: {}",
            err.message
        );
    }

    // ── execute_write_tool tests ───────────────────────────────────────
    //
    // These test the write tool flow using the full `execute_write_tool`
    // function. The rebuild step will fail (the test client points at a
    // fake host), which exercises the "rebuild failure" path: cache is
    // invalidated, rebuild fails, result is still returned.

    #[tokio::test]
    async fn write_tool_success_returns_result_and_invalidates_cache() {
        let state = test_state().await;
        populate_cache(&state.cache).await;
        assert!(
            state.cache.is_fresh().await,
            "cache should be fresh before write"
        );

        // The op succeeds. The rebuild will fail (no real GitHub server),
        // so the cache is left invalidated.
        let result = execute_write_tool(&state, || async {
            Ok::<_, unblock_github::errors::Error>("created")
        })
        .await;

        assert_eq!(result.unwrap(), "created");
        // Cache was invalidated by rebuild_cache, and since fetch_graph_data
        // will fail against the test client, it stays invalidated.
        assert!(
            !state.cache.is_fresh().await,
            "cache should be invalidated after write with failed rebuild"
        );
    }

    #[tokio::test]
    async fn write_tool_failure_does_not_touch_cache() {
        let state = test_state().await;
        populate_cache(&state.cache).await;
        let original_ready_set = state.cache.get_ready_set().await.unwrap();
        assert!(
            state.cache.is_fresh().await,
            "cache should be fresh before write"
        );

        let result = execute_write_tool(&state, || async {
            Err::<String, _>(
                GitHubApiSnafu {
                    status: 500_u16,
                    message: "Internal Server Error".to_owned(),
                }
                .build(),
            )
        })
        .await;

        // Operation should have failed.
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);

        // Cache should still contain the original data — untouched.
        assert!(state.cache.is_fresh().await, "cache should still be fresh");
        let current_ready_set = state.cache.get_ready_set().await.unwrap();
        assert_eq!(*current_ready_set, *original_ready_set);
    }

    // ── rebuild_cache tests ────────────────────────────────────────────

    #[tokio::test]
    async fn rebuild_cache_invalidates_first() {
        let state = test_state().await;
        populate_cache(&state.cache).await;
        assert!(state.cache.is_fresh().await);

        // After rebuild_cache, since the test client can't reach GitHub,
        // the cache is left invalidated.
        rebuild_cache(&state).await;

        assert!(
            !state.cache.is_fresh().await,
            "cache should be invalidated after failed rebuild"
        );
        assert!(state.cache.get_ready_set().await.is_none());
    }
}
