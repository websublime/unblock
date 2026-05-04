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
pub(crate) mod cross_repo;
pub mod dep_cycles;
pub mod dep_remove;
pub mod depends;
pub mod init;
pub mod list;
pub mod prime;
pub mod ready;
pub mod reconcile;
pub mod reopen;
pub mod search;
pub mod setup;
pub mod show;
pub mod stats;
pub mod update;

use std::future::Future;

use rmcp::model::ErrorData;
use tracing::instrument;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{BlockingEdge, Issue, IssueSummary};
use unblock_github::GitHubApi;
use unblock_github::projects::FieldValue;

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

/// Validates an `issue_type` parameter against the canonical
/// [`unblock_core::types::IssueType`] taxonomy.
///
/// Returns the resolved [`IssueType`](unblock_core::types::IssueType)
/// on success, or an [`ErrorData`] with code `INVALID_PARAMS` whose
/// message lists every accepted canonical name verbatim. Matching is
/// case-insensitive + byte-trim per the §5.7 normaliser, routed
/// through `IssueType::from_canonical_name`.
///
/// Used by both the `create` and `update` tool handlers so the
/// rejection message stays uniform across the two surfaces (parent
/// bead unblock-wgj review SUGGESTION 2 — previously the message was
/// duplicated ~14 lines each at server.rs:1893-1918 and 2305-2326).
///
/// Spec §8.3 + §8.6: the eight canonical names are `Task`, `Bug`,
/// `Feature`, `Spike`, `Epic`, `Chore`, `Refactor`, `Docs`. Adding a
/// variant to `IssueType` does not require touching this helper —
/// the message is built from `IssueType::ALL` at call time, so the
/// list extends automatically.
///
/// # Errors
///
/// Returns an `ErrorData` with code `INVALID_PARAMS` when `raw` does
/// not normalise to one of the canonical names. The message is
/// agent-actionable: it names the offending value AND lists every
/// accepted alternative.
pub(crate) fn validate_issue_type_param(
    raw: &str,
) -> Result<unblock_core::types::IssueType, ErrorData> {
    unblock_core::types::IssueType::from_canonical_name(raw).ok_or_else(|| ErrorData {
        code: rmcp::model::ErrorCode::INVALID_PARAMS,
        message: format!(
            "Invalid issue_type '{raw}' — must be one of {}",
            unblock_core::types::IssueType::ALL
                .iter()
                .map(|v| v.canonical_name())
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into(),
        data: None,
    })
}

/// Build a fresh [`DependencyGraph`] and ready-set projection from raw
/// `fetch_graph_data()` outputs.
///
/// Centralises the build → `compute_ready_set` pair that every
/// cache-population site (the [`rebuild_cache`] success branch and the
/// fresh-fetch sequences in `prime`, `reconcile`, `list`, and the `stats`
/// retry path) would otherwise duplicate verbatim.
///
/// SPEC §3.3 Filter 3 / §14 Invariant 14(a): forwarding the configured
/// `(owner, repo)` to [`DependencyGraph::compute_ready_set`] is the single
/// chokepoint that scopes the cached ready set to local-only source issues.
/// Concentrating the call here means `(owner, repo)` is threaded through
/// the engine in exactly one place per fresh-fetch flow, making the
/// scoping invariant easy to audit and impossible to bypass by accident.
///
/// Pure / synchronous: no I/O, no allocations beyond the `Vec<IssueSummary>`
/// and the underlying `DiGraph`. Inputs are borrowed so callers retain
/// ownership of the issue/edge slices when they need to use them again
/// (e.g. for downstream categorisation, drift analysis, or aggregation
/// before the cache is updated).
#[must_use]
pub(crate) fn build_graph_and_ready_set(
    issues: &[Issue],
    edges: &[BlockingEdge],
    configured_owner: &str,
    configured_repo: &str,
) -> (DependencyGraph, Vec<IssueSummary>) {
    let graph = DependencyGraph::build(issues, edges);
    let ready_set = graph.compute_ready_set(issues, configured_owner, configured_repo);
    (graph, ready_set)
}

/// Ergonomic overload of [`build_graph_and_ready_set`] that pulls the
/// configured `(owner, repo)` from `state` and forwards to the 4-arg form.
///
/// Used by call sites that retain intermediate references to the freshly
/// built graph and ready set between [`compute_ready_set`] and
/// [`unblock_core::cache::GraphCache::update`] — i.e. they CANNOT delegate
/// to [`refresh_cache_from`] because they need to consume the local
/// references first (categorisation, drift analysis, aggregation, cycle
/// detection). Today that is `prime`, `reconcile`, the `stats` retry
/// path, and the `dep_cycles` retry path.
///
/// **Chokepoint contract is unchanged.** The configured `(owner, repo)`
/// still flows through the same single
/// [`DependencyGraph::compute_ready_set`] chokepoint that enforces SPEC
/// §3.3 Filter 3 / §14 Invariant 14(a). This overload is purely
/// ergonomic: it removes the four-line `(state.github.owner(),
/// state.github.repo())` boilerplate in favour of the resolved values
/// being threaded through `state` once. Auditors can still trace
/// `(owner, repo)` to a single helper line — now in
/// [`build_graph_and_ready_set`] itself, called by both forms.
///
/// Source of `(owner, repo)`: [`unblock_github::GitHubApi::owner`] /
/// [`unblock_github::GitHubApi::repo`] on `state.github`, matching the
/// sibling [`refresh_cache_from`] helper. The `Config.repo`
/// `Option<String>` carries the unparsed `"owner/repo"` form, so using
/// the resolved accessors avoids a parsing path the call sites do not
/// currently exercise.
///
/// Pure / synchronous wrapper — no I/O of its own; defers to the 4-arg
/// helper for all allocations and graph construction.
///
/// [`compute_ready_set`]: DependencyGraph::compute_ready_set
#[must_use]
pub(crate) fn build_graph_and_ready_set_in(
    state: &ServerState,
    issues: &[Issue],
    edges: &[BlockingEdge],
) -> (DependencyGraph, Vec<IssueSummary>) {
    build_graph_and_ready_set(issues, edges, state.github.owner(), state.github.repo())
}

/// Refresh the cache from a freshly-fetched `(issues, edges)` pair.
///
/// Builds a [`DependencyGraph`] and ready set via
/// [`build_graph_and_ready_set`] (using `state.github.owner()` /
/// `state.github.repo()` as the SPEC §3.3 Filter 3 chokepoint) and stores
/// all three artefacts in [`crate::server::ServerState::cache`] via
/// [`unblock_core::cache::GraphCache::update`].
///
/// Used by call sites that **fully delegate** the build → compute → store
/// sequence — i.e. they do not need the freshly-built graph or ready set
/// for any intermediate work between `compute_ready_set` and
/// `cache.update`. Those are:
///
/// - [`rebuild_cache`] — post-mutation cache rebuild after `execute_write_tool`.
/// - The success branch of the `list` handler — refreshes the cache as a
///   side effect of the fresh fetch the handler already needed.
///
/// **Sites that retain intermediate references** (`prime`, `reconcile`, the
/// `stats` retry path, the `dep_cycles` retry path) instead call
/// [`build_graph_and_ready_set_in`] (or the 4-arg
/// [`build_graph_and_ready_set`] form) directly, then invoke
/// `state.cache.update(...)` once they have finished consuming the local
/// references. Coupling the build/compute pair into a shared helper while
/// leaving the cache-update line at the call site avoids both (a)
/// double-builds (calling the full helper after a local build) and (b)
/// post-update cache re-reads (which would introduce a new race window
/// with concurrent invalidators relative to today's behaviour).
///
/// **Observably equivalent.** Callers that adopt this helper match the
/// pre-refactor sequence one-for-one: same allocations, same lock
/// acquisition order, same SPEC scoping. The site-specific `tracing::debug!`
/// message that previously followed `cache.update` is preserved at the call
/// site so log telemetry is unchanged.
pub(crate) async fn refresh_cache_from(
    state: &ServerState,
    issues: Vec<Issue>,
    edges: &[BlockingEdge],
) {
    let (graph, ready_set) =
        build_graph_and_ready_set(&issues, edges, state.github.owner(), state.github.repo());
    state.cache.update(issues, ready_set, graph).await;
}

/// Site-specific log messages for [`update_status_field_best_effort`].
///
/// Each adopting call site declares a `static` config literal naming the
/// exact log message strings used pre-refactor. Optional fields (`None`)
/// instruct the helper to silently skip the corresponding rung — preserving
/// the byte-for-byte observability of sites that intentionally swallowed a
/// failure mode (e.g. the close-cascade loop, which iterates over many
/// dependents and would otherwise emit per-iteration spam when the project
/// is misconfigured).
///
/// **Telemetry contract.** The helper emits at most one log record per rung
/// of the if-let ladder. Severity (`debug!` vs `warn!`) and structured
/// fields (`error = %e`, `slug`) are fixed by the helper; only the message
/// text is configurable via this struct. Adopting sites that need
/// additional structured context (e.g. the close-cascade site's
/// `cascaded_qid`) wrap the helper call in a `tracing::Span` so the field
/// merges into every log record automatically.
pub(crate) struct StatusUpdateLogConfig {
    /// `tracing::debug!` message when [`GitHubApi::field_ids`] returns
    /// `None` (setup not run / cache cleared). `None` ⇒ silently skip the
    /// rung — preserves the close-cascade outer-`&&`-chain behaviour where
    /// the missing-setup case was swallowed without per-iteration log spam.
    pub no_field_ids_debug: Option<&'static str>,
    /// `tracing::warn!` (with `error = %e` field) message when
    /// [`GitHubApi::resolve_project_info`] returns `Err`. `None` ⇒ silently
    /// skip — same cascade rationale as `no_field_ids_debug`.
    pub resolve_project_warn: Option<&'static str>,
    /// `tracing::warn!` (with `error = %e` field) message when
    /// [`GitHubApi::get_project_item_id`] returns `Err`. Always emitted —
    /// no pre-refactor site silently swallowed this rung.
    pub item_id_warn: &'static str,
    /// `tracing::warn!` (with `slug` field) message when the configured
    /// status slug is absent from `field_ids.status.options`. `None` ⇒
    /// silently skip — preserves the pre-refactor `if let Some(option_id)
    /// = ... && let Err(e) = ...` shape used by close, depends, and the
    /// close-cascade Status=ready rung, which all silently no-op when the
    /// option is missing rather than warn.
    pub option_missing_warn: Option<&'static str>,
    /// `tracing::warn!` (with `error = %e` field) message when
    /// [`GitHubApi::update_field`] itself returns `Err`. Always emitted —
    /// every pre-refactor site warned on this rung.
    pub update_field_warn: &'static str,
}

/// Best-effort Projects V2 Status field update — single chokepoint for the
/// "set Status to slug X" if-let ladder.
///
/// Five call sites in the workspace pre-refactor open-coded the same
/// sequence: cached `field_ids` ⇒ `resolve_project_info` ⇒
/// `get_project_item_id` ⇒ `field_ids.status.options.get(slug)` ⇒
/// `update_field`. The pattern was tracked by `unblock-29p.24` (parent
/// finding from `unblock-b6b.79`, kept open as the authoritative
/// consolidation bead). The five sites are:
///
/// 1. **`server::close` handler** — Status → `closed` after
///    [`GitHubApi::close_issue`]. Configured-repo only (caller already
///    validated). Silent on missing-option (pre-refactor behaviour).
/// 2. **`server::close` cascade loop** — Status → `ready` per cascaded
///    dependent (gated outside the helper on
///    `cascaded_issue.status != Status::InProgress` so the helper
///    semantics stay simple). Cross-repo dependents naturally degrade via
///    [`GitHubApi::get_project_item_id`] returning `Err` for issues that
///    are not project items on the configured board (per spec §5.6
///    cross-repo scope). Silent on missing field-IDs / project resolution
///    so the per-iteration loop does not spam logs.
/// 3. **`server::depends` handler** — Status → `blocked` on the source
///    issue after
///    [`GitHubApi::add_blocked_by_refs`]. Gated outside the helper on
///    `matches!(source_ref, IssueRef::Local(_))` because the configured
///    project's `ProjectInfo` cannot host cross-repo source items per
///    spec §5.6.
/// 4. **`tools::reopen::update_status_field`** (deleted; consolidated under
///    `update_status_field_best_effort`) — Status → caller-supplied
///    slug (`"ready"` or `"blocked"`) after
///    [`GitHubApi::reopen_issue`]. Caller-supplied slug.
/// 5. **`tools::dep_remove::update_status_to_ready`** (deleted; consolidated
///    under `update_status_field_best_effort`) — Status → `ready`
///    when [`GitHubApi::remove_blocked_by_refs`] left the source with
///    zero open blockers. Hardcoded slug per spec §8.5.
///
/// **(`owner`, `repo`) chokepoint propagation (SPEC §3.3 Filter 3 / §14
/// Invariant 14(a)).** Projects V2 field updates are scoped to the
/// configured project, not to the issue's home repo. The configured
/// `(owner, repo)` is enforced upstream by the
/// [`GitHubApi::resolve_project_info`] / [`GitHubApi::get_project_item_id`]
/// pair: `resolve_project_info` resolves the configured project (single
/// project per server instance), and `get_project_item_id(node_id, project_id)`
/// looks up the issue's project item on that exact project — returning
/// `Err(IssueNotFound)` for any cross-repo issue that is not a member of
/// the configured project. The helper therefore inherits the scoping
/// invariant transparently: cross-repo nodes (e.g. cascaded dependents in
/// site 2) degrade to a naturally-warned `item_id_warn` rather than
/// silently writing to the wrong project. Caller-side gating (sites 2 and
/// 3 above) is preserved because it short-circuits the no-op early and
/// avoids an unnecessary GraphQL round-trip on the cross-repo path.
///
/// **Best-effort posture.** Every rung swallows its `Err`/`None` outcome
/// and continues — no rung returns a `Result`. This matches the pre-
/// refactor behaviour at every site: the underlying state-changing
/// mutation (`close` / `claim` / `depends` / `reopen` / `dep_remove`) has
/// already succeeded server-side, and the Projects V2 Status field is a
/// reconciliation surface (`reconcile` tool re-asserts it on demand).
///
/// **Observable equivalence.** The helper logs the same number of records
/// at the same severities as the pre-refactor sites, with the exact
/// message strings supplied by the [`StatusUpdateLogConfig`] literal at
/// the call site. Structured fields (`error`, `slug`) are uniform across
/// adopters; site-specific structured context (e.g. `cascaded_qid`)
/// merges in via [`tracing::Span`] propagation when the caller wraps the
/// helper invocation in a span.
///
/// **Out of scope.** The `claim` handler's three-write ladder (Status +
/// Agent + Claimed At) and the `update` handler's four-write ladder with
/// success-tracking accumulator are NOT consolidated here — both have
/// shapes meaningfully different from this single-Status best-effort
/// helper. They remain candidates for a separate multi-field consolidation
/// if uniformity across mutation surfaces is later desired.
pub(crate) async fn update_status_field_best_effort(
    client: &dyn GitHubApi,
    issue_node_id: &str,
    target_status_slug: &str,
    log: &StatusUpdateLogConfig,
) {
    let Some(field_ids) = client.field_ids().await else {
        if let Some(msg) = log.no_field_ids_debug {
            tracing::debug!(slug = target_status_slug, "{msg}");
        }
        return;
    };

    let project_info = match client.resolve_project_info().await {
        Ok(info) => info,
        Err(err) => {
            if let Some(msg) = log.resolve_project_warn {
                tracing::warn!(error = %err, slug = target_status_slug, "{msg}");
            }
            return;
        }
    };

    let item_id = match client
        .get_project_item_id(issue_node_id, &project_info.id)
        .await
    {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!(error = %err, slug = target_status_slug, "{}", log.item_id_warn);
            return;
        }
    };

    let Some(option_id) = field_ids.status.options.get(target_status_slug) else {
        if let Some(msg) = log.option_missing_warn {
            tracing::warn!(slug = target_status_slug, "{msg}");
        }
        return;
    };

    if let Err(err) = client
        .update_field(
            &project_info.id,
            &item_id,
            &field_ids.status.field_id,
            &FieldValue::SingleSelectOption(option_id.clone()),
        )
        .await
    {
        tracing::warn!(error = %err, slug = target_status_slug, "{}", log.update_field_warn);
    }
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
            // SPEC §3.3 Filter 3 / §14 Invariant 14(a): `refresh_cache_from`
            // forwards the configured (owner, repo) to the engine — the
            // canonical chokepoint that scopes the cached ready set to
            // local-only sources. Downstream consumers (ready, prime,
            // update_status_fields) inherit the guarantee without
            // re-checking. (unblock-eos.4 / D6.a / GAP-14.b)
            refresh_cache_from(state, issues, &edges).await;
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
        // Fixture lives in ("test", "repo") — match the configured coords
        // so SPEC §3.3 Filter 3 admits the issues.
        let ready_set = graph.compute_ready_set(&issues, "test", "repo");
        cache.update(issues, ready_set, graph).await;
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

    // ── validate_issue_type_param tests
    //     (parent bead unblock-wgj review SUGGESTION 2) ────────────────

    #[test]
    fn validate_issue_type_param_accepts_every_canonical_variant() {
        // Every member of `IssueType::ALL` MUST round-trip through the
        // shared helper. Adding a variant to the enum auto-extends
        // this assertion (the loop iterates the enum, not a literal
        // list).
        for &variant in &IssueType::ALL {
            let resolved =
                validate_issue_type_param(variant.canonical_name()).unwrap_or_else(|err| {
                    panic!(
                        "canonical name {:?} should round-trip through validator: {:?}",
                        variant.canonical_name(),
                        err.message
                    )
                });
            assert_eq!(resolved, variant);
        }
    }

    #[test]
    fn validate_issue_type_param_normalises_case_and_trim() {
        // §5.7 normaliser: case-insensitive + byte-trim.
        assert_eq!(validate_issue_type_param("bug").unwrap(), IssueType::Bug);
        assert_eq!(validate_issue_type_param("BUG").unwrap(), IssueType::Bug);
        assert_eq!(
            validate_issue_type_param("  Refactor  ").unwrap(),
            IssueType::Refactor
        );
        assert_eq!(validate_issue_type_param("ePiC").unwrap(), IssueType::Epic);
    }

    #[test]
    fn validate_issue_type_param_rejects_unknown_with_actionable_message() {
        let err = validate_issue_type_param("Sprinkles")
            .expect_err("non-canonical name should be rejected");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        // Message names the offending value AND lists every accepted
        // alternative — the contract that lets agents recover without
        // a separate round-trip.
        assert!(
            err.message.contains("Sprinkles"),
            "message must echo the bad value: {}",
            err.message
        );
        for variant in IssueType::ALL {
            assert!(
                err.message.contains(variant.canonical_name()),
                "message must list canonical name {:?}: {}",
                variant.canonical_name(),
                err.message
            );
        }
    }

    #[test]
    fn validate_issue_type_param_rejects_empty_string() {
        let err = validate_issue_type_param("")
            .expect_err("empty string is not a canonical IssueType name");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
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
