//! Stats tool — aggregate counts and metrics across the issue set.
//!
//! Per spec §7.4 this is a read-only tool that aggregates the open issue
//! set (and, once [`unblock-a36`] lands, the closed set too) into a
//! compact snapshot: totals by status, totals by priority, how many
//! issues are blocked vs ready, how many cycles exist in the dependency
//! graph, and per-agent throughput.
//!
//! ## Cache-aware read path (spec §7.4)
//!
//! The handler observes the spec's "API calls: 0 (cache hit) | 1+
//! (rebuild)" contract:
//!
//! 1. If the cache is stale or empty, call
//!    [`crate::tools::rebuild_cache`] to refresh every cached artefact
//!    via a single `fetch_graph_data()` round-trip.
//! 2. Read [`GraphCache::get_issues`] and [`GraphCache::get_graph`] —
//!    both O(1) `Arc` clones — and aggregate entirely from cached
//!    state. This is why [`GraphCache`] stores the full issue vector
//!    (see the cache-extension commit for unblock-29p.8): without it,
//!    stats would have to re-fetch on every call.
//! 3. If the cache is still empty after a rebuild attempt (the network
//!    call failed), propagate the underlying GitHub error via the
//!    crate-internal `github_error_to_mcp` helper instead of returning
//!    a degraded `stale = true` envelope. The spec's `StatsResult` has
//!    no `stale` field — this mirrors [`search`](crate::tools::search),
//!    and the R6 decision on the bead binds this choice.
//!
//! ## OPEN-only scope (unblock-a36 follow-up)
//!
//! `fetch_graph_data` only returns OPEN GitHub issues today (tracked
//! separately by bead `unblock-a36`). That has two visible consequences
//! for stats:
//!
//! - `by_status["closed"]` is always `0`.
//! - Every [`AgentStats::completed`] counter is always `0`.
//!
//! Both fields are still populated (pre-seeded) in the response so
//! callers can differentiate "unknown" from "no data". Once `unblock-a36`
//! extends the fetch to include closed issues, stats picks up the new
//! data with no code change.
//!
//! ## Aggregation semantics
//!
//! - **`total`** — number of issues after applying the optional
//!   `milestone` filter.
//! - **`by_status`** — count per [`Status`] variant, keyed by the
//!   lowercase Projects V2 option slug (`ready`, `in_progress`,
//!   `blocked`, `deferred`, `closed`). Every key is pre-seeded with `0`
//!   so callers can read `map["ready"]` without null-checking.
//! - **`by_priority`** — count per [`Priority`] variant, keyed by the
//!   [`Display`](Priority) form (`"P0"`..`"P4"`). All five keys are
//!   pre-seeded with `0`.
//! - **`blocked_count`** — number of issues that are effectively
//!   blocked, i.e. `status == Status::Blocked` OR have at least one
//!   open blocker in the graph (per the R3 decision on the bead —
//!   mirrors [`prime`](crate::tools::prime) categorisation semantics).
//! - **`ready_count`** — `compute_ready_set(&issues).len()` after the
//!   milestone filter is applied (per the R4 decision). This is the
//!   count of issues that would appear in a fresh `ready` call scoped
//!   to the same milestone.
//! - **`cycle_count`** — `DependencyGraph::detect_all_cycles().len()`
//!   over the **full** graph (per the R5 decision). The milestone
//!   filter never scopes cycle detection because an edge can cross
//!   milestones and the spec does not define a scoped cycle semantic.
//! - **`agents`** — one [`AgentStats`] per distinct non-empty agent
//!   value observed in the (optionally milestone-filtered) issue set.
//!   `in_progress` counts issues whose `status == Status::InProgress`;
//!   `completed` counts issues with `state == IssueState::Closed`
//!   (always `0` today — see the OPEN-only scope note above).
//!
//! ## Validation
//!
//! The only parameter is `milestone: Option<String>`; there is no
//! numeric limit, no required field, and empty / whitespace-only
//! strings collapse to `None` via the crate-internal
//! `normalize_filter` helper, matching every other filter-accepting
//! read tool.
//!
//! [`unblock-a36`]: https://example.invalid/bd/unblock-a36
//! [`GraphCache`]: unblock_core::cache::GraphCache
//! [`GraphCache::get_issues`]: unblock_core::cache::GraphCache::get_issues
//! [`GraphCache::get_graph`]: unblock_core::cache::GraphCache::get_graph

use std::collections::HashMap;

use rmcp::model::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{Issue, IssueState, Priority, Status};

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;

/// Input parameters for the `stats` MCP tool.
///
/// Per spec §7.4. The `milestone` filter is applied before aggregation
/// so `by_status`, `by_priority`, `ready_count`, and `agents` all reflect
/// the scoped subset. `cycle_count` is intentionally computed on the
/// full graph (see module docs, R5 decision).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatsParams {
    /// Exact-match milestone title filter. When omitted (or empty /
    /// whitespace-only, which collapses to `None`), the response
    /// aggregates across every open issue.
    pub milestone: Option<String>,
}

/// Result returned by the `stats` MCP tool.
///
/// Per spec §7.4. Note the absence of a `stale` field: stats propagates
/// fetch failures as an `ErrorData` rather than a degraded envelope
/// (R6 decision).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StatsResult {
    /// Number of issues counted (after the optional milestone filter).
    pub total: usize,
    /// Count per [`Status`] variant, keyed by lowercase Projects V2
    /// option slug (`ready`, `in_progress`, `blocked`, `deferred`,
    /// `closed`). All five keys are always present — absent keys are
    /// pre-seeded to `0`.
    pub by_status: HashMap<String, usize>,
    /// Count per [`Priority`] variant, keyed by the [`Display`](Priority)
    /// form (`"P0"`..`"P4"`). All five keys are always present.
    pub by_priority: HashMap<String, usize>,
    /// Number of issues effectively blocked — either
    /// `status == Status::Blocked` OR at least one open blocker in the
    /// graph (R3 decision).
    pub blocked_count: usize,
    /// Number of issues that would appear in a `ready` call scoped to
    /// the same milestone (R4 decision).
    pub ready_count: usize,
    /// Number of cycles in the **full** dependency graph (R5 decision).
    /// The milestone filter never scopes cycle detection.
    pub cycle_count: usize,
    /// Per-agent throughput. One entry per distinct non-empty agent
    /// value in the (optionally milestone-filtered) issue set.
    pub agents: Vec<AgentStats>,
}

/// Per-agent throughput snapshot.
///
/// Per spec §7.4. `completed` is always `0` today because
/// `fetch_graph_data` returns OPEN issues only (see module-level docs
/// and `unblock-a36`).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentStats {
    /// Agent name as stored on `Issue.agent`.
    pub name: String,
    /// Number of issues assigned to this agent with
    /// `status == Status::InProgress`.
    pub in_progress: usize,
    /// Number of issues assigned to this agent with
    /// `state == IssueState::Closed`. Always `0` today — see module docs.
    pub completed: usize,
}

/// Lowercase Projects V2 slug for a [`Status`] variant.
///
/// Uses the same option names as the GraphQL `parse_status_field`
/// parser in `unblock-github` (and the `projects.rs` setup helper),
/// so the wire format is round-trip safe with the field values the
/// server itself creates.
fn status_slug(status: Status) -> &'static str {
    match status {
        Status::Ready => "ready",
        Status::InProgress => "in_progress",
        Status::Blocked => "blocked",
        Status::Deferred => "deferred",
        Status::Closed => "closed",
    }
}

/// Return `true` when `issue` has at least one blocker that is still
/// OPEN.
///
/// Mirrors `has_open_blockers` in
/// [`prime`](crate::tools::prime) — outgoing edges in the dependency
/// graph point from the blocked issue to its blockers, so we walk the
/// outgoing neighbours and stop at the first one whose
/// [`IssueState`] is [`Open`](IssueState::Open).
///
/// Issues absent from the graph (edges absent, no blockers recorded)
/// are treated as unblocked.
fn has_open_blockers(issue: &Issue, graph: &DependencyGraph) -> bool {
    let Some(&node_idx) = graph.node_map().get(&issue.qualified_id) else {
        return false;
    };
    let inner = graph.inner_graph();
    let issue_state = graph.issue_state();
    inner
        .neighbors_directed(node_idx, petgraph::Direction::Outgoing)
        .any(|neighbor_idx| {
            let neighbor_qid = &inner[neighbor_idx];
            issue_state
                .get(neighbor_qid)
                .is_some_and(|state| *state == IssueState::Open)
        })
}

/// Pre-seed the `by_status` bucket with `0` for every [`Status`]
/// variant so callers can read any key unconditionally.
fn seed_status_buckets() -> HashMap<String, usize> {
    let mut out = HashMap::with_capacity(5);
    for status in [
        Status::Ready,
        Status::InProgress,
        Status::Blocked,
        Status::Deferred,
        Status::Closed,
    ] {
        out.insert(status_slug(status).to_owned(), 0);
    }
    out
}

/// Pre-seed the `by_priority` bucket with `0` for every [`Priority`]
/// variant so callers can read any key unconditionally.
fn seed_priority_buckets() -> HashMap<String, usize> {
    let mut out = HashMap::with_capacity(5);
    for priority in [
        Priority::P0,
        Priority::P1,
        Priority::P2,
        Priority::P3,
        Priority::P4,
    ] {
        out.insert(priority.to_string(), 0);
    }
    out
}

/// Aggregate a fully-resolved issue / graph snapshot into the spec
/// [`StatsResult`] shape.
///
/// Pure function — takes pre-filtered issues (milestone already
/// applied) plus the unfiltered graph. Returning a dedicated helper
/// keeps [`handle_stats`] focused on the cache orchestration and makes
/// the aggregation logic trivially unit-testable without touching the
/// cache layer.
///
/// `configured_owner` / `configured_repo` are forwarded to
/// [`DependencyGraph::compute_ready_set`] so SPEC §3.3 Filter 3
/// (§14 Invariant 14(a)) scopes `ready_count` to the configured
/// repository. Every caller already holds these from
/// [`crate::server::ServerState::github`].
fn aggregate_stats(
    filtered: &[&Issue],
    graph: &DependencyGraph,
    configured_owner: &str,
    configured_repo: &str,
) -> StatsResult {
    let mut by_status = seed_status_buckets();
    let mut by_priority = seed_priority_buckets();
    let mut blocked_count: usize = 0;
    // Map of agent name → (in_progress, completed).
    let mut agent_map: HashMap<String, (usize, usize)> = HashMap::new();

    for issue in filtered {
        // by_status — lowercase slug keys match the Projects V2 option
        // names the project is created with (unblock-github/src/projects.rs).
        if let Some(bucket) = by_status.get_mut(status_slug(issue.status)) {
            *bucket += 1;
        }

        // by_priority — "P0".."P4" Display form.
        if let Some(bucket) = by_priority.get_mut(&issue.priority.to_string()) {
            *bucket += 1;
        }

        // blocked_count — union of Status::Blocked OR open blockers in
        // the graph (R3).
        if issue.status == Status::Blocked || has_open_blockers(issue, graph) {
            blocked_count += 1;
        }

        // Agents — one entry per distinct non-empty `Issue.agent`. Empty
        // strings collapse to None the same way `normalize_filter` does
        // elsewhere, so a trailing-space agent is not split from the
        // same name without the space (defensive — agents today are
        // assigned via claim which already writes trimmed values).
        if let Some(agent) = issue
            .agent
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let slot = agent_map
                .entry(agent.to_owned())
                .or_insert((0_usize, 0_usize));
            if issue.status == Status::InProgress {
                slot.0 += 1;
            }
            if issue.state == IssueState::Closed {
                slot.1 += 1;
            }
        }
    }

    // Ready set is computed on the filtered slice — R4: the ready_count
    // surfaces what `ready(milestone=…)` would return. `compute_ready_set`
    // needs `&[Issue]`, so project the `&Issue` refs back into a Vec of
    // owned clones. This allocation is bounded by `total` and only runs
    // on the cache-warm read path (once per stats call).
    let filtered_owned: Vec<Issue> = filtered.iter().copied().cloned().collect();
    let ready_count = graph
        .compute_ready_set(&filtered_owned, configured_owner, configured_repo)
        .len();

    // Cycle count is always full-graph (R5). Milestone does not scope
    // cycle detection — the spec does not define a milestone-aware
    // cycle semantic, and an edge can cross milestones.
    let cycle_count = graph.detect_all_cycles().len();

    // Stable agent order so test assertions (and clients) do not depend
    // on HashMap iteration order.
    let mut agents: Vec<AgentStats> = agent_map
        .into_iter()
        .map(|(name, (in_progress, completed))| AgentStats {
            name,
            in_progress,
            completed,
        })
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));

    StatsResult {
        total: filtered.len(),
        by_status,
        by_priority,
        blocked_count,
        ready_count,
        cycle_count,
        agents,
    }
}

/// Execute the `stats` tool handler.
///
/// See the module-level docs for the cache-aware contract and the
/// aggregation semantics that R3/R4/R5/R6/R7 pin down.
///
/// # Flow
///
/// 1. If the cache is stale, lazily rebuild via
///    [`crate::tools::rebuild_cache`] (single `fetch_graph_data()`).
/// 2. Read the full issue vector and the dependency graph from the
///    cache — both O(1) `Arc` clones. If either is still absent after
///    the rebuild attempt, propagate a real GitHub error so the caller
///    sees the underlying cause (R6).
/// 3. Apply the optional `milestone` filter to produce the aggregation
///    slice.
/// 4. Delegate to the crate-internal `aggregate_stats` helper.
///
/// # Errors
///
/// Returns [`ErrorData`] when the cache cannot be warmed (e.g. GitHub
/// network failure). The error is the same one the crate-internal
/// `github_error_to_mcp` helper surfaces for a direct
/// `fetch_graph_data()` call.
#[instrument(
    skip(state, params),
    name = "handle_stats",
    fields(
        agent.kind = state.agent_kind_str(),
        milestone = params.milestone.as_deref(),
    ),
)]
pub async fn handle_stats(
    state: &ServerState,
    params: StatsParams,
) -> Result<StatsResult, ErrorData> {
    info!("Stats tool invoked");

    // Step 1: warm the cache if needed. Zero work on the cache-hit path.
    if !state.cache.is_fresh().await {
        tracing::debug!("Stats cache is stale — triggering lazy rebuild");
        crate::tools::rebuild_cache(state).await;
    }

    // Step 2: pull the full issue set and graph from cache. Both are
    // O(1) Arc clones; the cache lock is released as soon as both
    // handles are captured.
    let issues_arc = state.cache.get_issues().await;
    let graph_arc = state.cache.get_graph().await;

    let (Some(issues), Some(graph)) = (issues_arc, graph_arc) else {
        // The rebuild attempt above could not populate the cache (e.g.
        // `fetch_graph_data` returned an error inside `rebuild_cache`).
        // Re-issue the fetch locally so the caller receives the real
        // underlying error rather than an empty-envelope response.
        // `StatsResult` has no `stale` field, so error propagation is
        // the only surface for failures (R6 decision).
        tracing::warn!("Cache empty after rebuild — retrying fetch to surface the error");
        let (issues_vec, edges_vec) = state
            .github
            .fetch_graph_data()
            .await
            .map_err(github_error_to_mcp)?;
        // The retry unexpectedly succeeded — populate the cache so a
        // follow-up call is warm, then aggregate against the freshly
        // fetched vectors without re-reading the cache.
        let graph_built = DependencyGraph::build(&issues_vec, &edges_vec);
        // SPEC §3.3 Filter 3 / §14 Invariant 14(a): pass configured coords
        // so the cached ready set is local-only and `ready_count` in the
        // stats envelope matches what `ready(milestone=…)` would return.
        let ready_set = graph_built.compute_ready_set(
            &issues_vec,
            state.github.owner(),
            state.github.repo(),
        );
        let milestone = crate::tools::normalize_filter(params.milestone.as_deref());
        let filtered = filter_by_milestone(&issues_vec, milestone);
        let result = aggregate_stats(
            &filtered,
            &graph_built,
            state.github.owner(),
            state.github.repo(),
        );
        state.cache.update(issues_vec, ready_set, graph_built).await;
        return Ok(result);
    };

    // Step 3: apply the milestone filter (R2 decision — `milestone` is
    // normalised so empty/whitespace inputs collapse to None).
    let milestone = crate::tools::normalize_filter(params.milestone.as_deref());
    let filtered = filter_by_milestone(issues.as_ref(), milestone);

    // Step 4: aggregate. SPEC §3.3 Filter 3 / §14 Invariant 14(a) —
    // `aggregate_stats` threads configured coords into the embedded
    // `compute_ready_set` call.
    Ok(aggregate_stats(
        &filtered,
        graph.as_ref(),
        state.github.owner(),
        state.github.repo(),
    ))
}

/// Filter an issue slice by exact-match milestone title.
///
/// When `milestone` is `None`, every issue is retained (zero-cost
/// borrow-through via `Iterator::collect`). Extracted so both the
/// happy path and the fetch-retry fallback share a single filter
/// implementation.
fn filter_by_milestone<'a>(issues: &'a [Issue], milestone: Option<&str>) -> Vec<&'a Issue> {
    issues
        .iter()
        .filter(|issue| milestone.is_none_or(|m| issue.milestone.as_deref() == Some(m)))
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use unblock_core::types::{BlockingEdge, IssueType, QualifiedId};

    use super::*;

    // ── Test helpers ────────────────────────────────────────────────────

    /// Owner/repo used by stats test fixtures. Must match the values passed
    /// to `aggregate_stats` so SPEC §3.3 Filter 3 (§14 Invariant 14(a))
    /// admits the local issues.
    const TEST_OWNER: &str = "acme";
    const TEST_REPO: &str = "widgets";

    fn qid(number: u64) -> QualifiedId {
        QualifiedId::new(TEST_OWNER, TEST_REPO, number)
    }

    /// Minimal open [`Issue`] populated enough for aggregation tests.
    fn stats_issue(
        number: u64,
        status: Status,
        priority: Priority,
        state: IssueState,
        agent: Option<&str>,
        milestone: Option<&str>,
    ) -> Issue {
        Issue {
            qualified_id: qid(number),
            number,
            node_id: format!("NODE_{number}"),
            title: format!("Issue #{number}"),
            issue_type: Some(IssueType::Task),
            status,
            priority,
            agent: agent.map(str::to_owned),
            claimed_at: None,
            pipeline_stage: None,
            story_points: None,
            defer_until: None,
            labels: vec![],
            milestone: milestone.map(str::to_owned),
            assignees: vec![],
            state,
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

    // ── status_slug / seeding ───────────────────────────────────────────

    #[test]
    fn status_slug_maps_every_variant_to_lowercase() {
        assert_eq!(status_slug(Status::Ready), "ready");
        assert_eq!(status_slug(Status::InProgress), "in_progress");
        assert_eq!(status_slug(Status::Blocked), "blocked");
        assert_eq!(status_slug(Status::Deferred), "deferred");
        assert_eq!(status_slug(Status::Closed), "closed");
    }

    #[test]
    fn seed_status_buckets_contains_every_slug_at_zero() {
        let seeded = seed_status_buckets();
        assert_eq!(seeded.len(), 5);
        for key in ["ready", "in_progress", "blocked", "deferred", "closed"] {
            assert_eq!(
                seeded.get(key),
                Some(&0_usize),
                "missing pre-seeded slug {key}",
            );
        }
    }

    #[test]
    fn seed_priority_buckets_contains_every_priority_at_zero() {
        let seeded = seed_priority_buckets();
        assert_eq!(seeded.len(), 5);
        for key in ["P0", "P1", "P2", "P3", "P4"] {
            assert_eq!(
                seeded.get(key),
                Some(&0_usize),
                "missing pre-seeded priority {key}",
            );
        }
    }

    // ── has_open_blockers ───────────────────────────────────────────────

    #[test]
    fn has_open_blockers_false_when_issue_not_in_graph() {
        let g = DependencyGraph::build(&[], &[]);
        let orphan = stats_issue(
            42,
            Status::Ready,
            Priority::P2,
            IssueState::Open,
            None,
            None,
        );
        assert!(!has_open_blockers(&orphan, &g));
    }

    #[test]
    fn has_open_blockers_true_for_downstream_of_open_upstream() {
        let upstream = stats_issue(1, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let downstream = stats_issue(2, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let issues = vec![upstream.clone(), downstream.clone()];
        // Edge: downstream (2) source -> upstream (1) target. Matches
        // the existing `BlockingEdge` convention in cache/rebuild tests.
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];
        let g = DependencyGraph::build(&issues, &edges);
        assert!(has_open_blockers(&downstream, &g));
        assert!(!has_open_blockers(&upstream, &g));
    }

    #[test]
    fn has_open_blockers_false_when_upstream_is_closed() {
        let upstream = stats_issue(
            1,
            Status::Closed,
            Priority::P2,
            IssueState::Closed,
            None,
            None,
        );
        let downstream = stats_issue(2, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let issues = vec![upstream.clone(), downstream.clone()];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];
        let g = DependencyGraph::build(&issues, &edges);
        // The upstream blocker is Closed — the downstream issue should
        // look unblocked.
        assert!(!has_open_blockers(&downstream, &g));
    }

    // ── aggregate_stats ─────────────────────────────────────────────────

    #[test]
    fn aggregate_stats_empty_input_yields_zeroed_envelope() {
        let g = DependencyGraph::build(&[], &[]);
        let result = aggregate_stats(&[], &g, TEST_OWNER, TEST_REPO);
        assert_eq!(result.total, 0);
        assert_eq!(result.blocked_count, 0);
        assert_eq!(result.ready_count, 0);
        assert_eq!(result.cycle_count, 0);
        assert!(result.agents.is_empty());
        for key in ["ready", "in_progress", "blocked", "deferred", "closed"] {
            assert_eq!(result.by_status.get(key), Some(&0_usize));
        }
        for key in ["P0", "P1", "P2", "P3", "P4"] {
            assert_eq!(result.by_priority.get(key), Some(&0_usize));
        }
    }

    #[test]
    fn aggregate_stats_counts_one_per_variant() {
        // One issue in each Status × Priority combination we care about.
        let issues = vec![
            stats_issue(1, Status::Ready, Priority::P0, IssueState::Open, None, None),
            stats_issue(
                2,
                Status::InProgress,
                Priority::P1,
                IssueState::Open,
                Some("alice"),
                None,
            ),
            stats_issue(
                3,
                Status::Blocked,
                Priority::P2,
                IssueState::Open,
                None,
                None,
            ),
            stats_issue(
                4,
                Status::Deferred,
                Priority::P3,
                IssueState::Open,
                None,
                None,
            ),
            // Note: state Open because fetch_graph_data is OPEN-only today.
            stats_issue(5, Status::Ready, Priority::P4, IssueState::Open, None, None),
        ];
        let g = DependencyGraph::build(&issues, &[]);
        let refs: Vec<&Issue> = issues.iter().collect();
        let result = aggregate_stats(&refs, &g, TEST_OWNER, TEST_REPO);

        assert_eq!(result.total, 5);
        assert_eq!(result.by_status.get("ready"), Some(&2_usize));
        assert_eq!(result.by_status.get("in_progress"), Some(&1_usize));
        assert_eq!(result.by_status.get("blocked"), Some(&1_usize));
        assert_eq!(result.by_status.get("deferred"), Some(&1_usize));
        assert_eq!(result.by_status.get("closed"), Some(&0_usize));

        for key in ["P0", "P1", "P2", "P3", "P4"] {
            assert_eq!(result.by_priority.get(key), Some(&1_usize));
        }
    }

    #[test]
    fn aggregate_stats_blocked_count_unions_status_and_graph() {
        // Issue 1 is Status::Blocked (no graph edge).
        let a = stats_issue(
            1,
            Status::Blocked,
            Priority::P2,
            IssueState::Open,
            None,
            None,
        );
        // Issue 2 is Status::Ready but blocked-by issue 3 in the graph.
        let b = stats_issue(2, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let c = stats_issue(3, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let issues = vec![a.clone(), b.clone(), c.clone()];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(3),
        }];
        let g = DependencyGraph::build(&issues, &edges);
        let refs: Vec<&Issue> = issues.iter().collect();
        let result = aggregate_stats(&refs, &g, TEST_OWNER, TEST_REPO);

        // Both #1 (Status::Blocked) and #2 (has open blocker) count.
        assert_eq!(result.blocked_count, 2);
    }

    #[test]
    fn aggregate_stats_ready_count_matches_graph_compute_ready_set() {
        let a = stats_issue(1, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let b = stats_issue(2, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let issues = vec![a.clone(), b.clone()];
        // Issue 2 is blocked by issue 1, so only issue 1 is ready.
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];
        let g = DependencyGraph::build(&issues, &edges);
        let refs: Vec<&Issue> = issues.iter().collect();
        let result = aggregate_stats(&refs, &g, TEST_OWNER, TEST_REPO);
        assert_eq!(result.ready_count, 1);
    }

    #[test]
    fn aggregate_stats_cycle_count_detects_mutual_blockers() {
        let a = stats_issue(1, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let b = stats_issue(2, Status::Ready, Priority::P2, IssueState::Open, None, None);
        let issues = vec![a.clone(), b.clone()];
        let edges = vec![
            BlockingEdge {
                source: qid(1),
                target: qid(2),
            },
            BlockingEdge {
                source: qid(2),
                target: qid(1),
            },
        ];
        let g = DependencyGraph::build(&issues, &edges);
        let refs: Vec<&Issue> = issues.iter().collect();
        let result = aggregate_stats(&refs, &g, TEST_OWNER, TEST_REPO);
        assert_eq!(result.cycle_count, 1, "one SCC of size 2 = one cycle");
    }

    #[test]
    fn aggregate_stats_agents_grouped_by_name_with_in_progress_counts() {
        let issues = vec![
            stats_issue(
                1,
                Status::InProgress,
                Priority::P0,
                IssueState::Open,
                Some("alice"),
                None,
            ),
            stats_issue(
                2,
                Status::InProgress,
                Priority::P1,
                IssueState::Open,
                Some("alice"),
                None,
            ),
            stats_issue(
                3,
                Status::Ready,
                Priority::P2,
                IssueState::Open,
                Some("alice"),
                None,
            ),
            stats_issue(
                4,
                Status::InProgress,
                Priority::P0,
                IssueState::Open,
                Some("bob"),
                None,
            ),
            // Unassigned issue — should not contribute any AgentStats.
            stats_issue(5, Status::Ready, Priority::P2, IssueState::Open, None, None),
        ];
        let g = DependencyGraph::build(&issues, &[]);
        let refs: Vec<&Issue> = issues.iter().collect();
        let result = aggregate_stats(&refs, &g, TEST_OWNER, TEST_REPO);

        assert_eq!(result.agents.len(), 2);
        // Sort is by name ascending (alice, bob).
        assert_eq!(result.agents[0].name, "alice");
        assert_eq!(result.agents[0].in_progress, 2);
        assert_eq!(result.agents[0].completed, 0);
        assert_eq!(result.agents[1].name, "bob");
        assert_eq!(result.agents[1].in_progress, 1);
        assert_eq!(result.agents[1].completed, 0);
    }

    #[test]
    fn aggregate_stats_total_equals_sum_of_status_buckets() {
        // Property: when no milestone filter applies, the sum of every
        // status bucket must equal `total`.
        let issues = vec![
            stats_issue(1, Status::Ready, Priority::P2, IssueState::Open, None, None),
            stats_issue(
                2,
                Status::InProgress,
                Priority::P2,
                IssueState::Open,
                None,
                None,
            ),
            stats_issue(
                3,
                Status::Blocked,
                Priority::P2,
                IssueState::Open,
                None,
                None,
            ),
            stats_issue(
                4,
                Status::Deferred,
                Priority::P2,
                IssueState::Open,
                None,
                None,
            ),
        ];
        let g = DependencyGraph::build(&issues, &[]);
        let refs: Vec<&Issue> = issues.iter().collect();
        let result = aggregate_stats(&refs, &g, TEST_OWNER, TEST_REPO);
        let status_sum: usize = result.by_status.values().sum();
        assert_eq!(status_sum, result.total);
    }

    #[test]
    fn aggregate_stats_empty_agent_string_collapses_to_none() {
        // A whitespace-only agent string should not create an entry.
        let issues = vec![stats_issue(
            1,
            Status::InProgress,
            Priority::P2,
            IssueState::Open,
            Some("   "),
            None,
        )];
        let g = DependencyGraph::build(&issues, &[]);
        let refs: Vec<&Issue> = issues.iter().collect();
        let result = aggregate_stats(&refs, &g, TEST_OWNER, TEST_REPO);
        assert!(result.agents.is_empty());
    }

    // ── filter_by_milestone ─────────────────────────────────────────────

    #[test]
    fn filter_by_milestone_none_returns_everything() {
        let issues = vec![
            stats_issue(
                1,
                Status::Ready,
                Priority::P2,
                IssueState::Open,
                None,
                Some("v1.0"),
            ),
            stats_issue(
                2,
                Status::Ready,
                Priority::P2,
                IssueState::Open,
                None,
                Some("v2.0"),
            ),
        ];
        assert_eq!(filter_by_milestone(&issues, None).len(), 2);
    }

    #[test]
    fn filter_by_milestone_some_exact_match_only() {
        let issues = vec![
            stats_issue(
                1,
                Status::Ready,
                Priority::P2,
                IssueState::Open,
                None,
                Some("v1.0"),
            ),
            stats_issue(
                2,
                Status::Ready,
                Priority::P2,
                IssueState::Open,
                None,
                Some("v2.0"),
            ),
            stats_issue(3, Status::Ready, Priority::P2, IssueState::Open, None, None),
        ];
        let filtered = filter_by_milestone(&issues, Some("v1.0"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].number, 1);
    }
}
