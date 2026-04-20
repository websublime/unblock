//! Ready tool — finds issues with no active blockers that can be worked on now.
//!
//! This is the primary tool agents call to find work. It reads from the
//! in-memory cache (rebuilding lazily if stale) and applies optional filters
//! for priority, issue type, milestone, agent, and label.
//!
//! This is a read tool with cache-aware logic — it checks cache freshness
//! and triggers a rebuild if stale, but does not mutate GitHub state.
//!
//! ## Cross-Repo Response Contract (SPEC §11.4)
//!
//! The ready-set computation silently drops local issues held out of the ready
//! set by cross-repo OPEN blockers — the agent cannot see those blockers in
//! the returned [`IssueSummary`] projection (which is scoped to the configured
//! repository). Per SPEC §11.4 the handler surfaces those cross-repo
//! [`QualifiedId`] nodes via [`ReadyResult::cross_repo_refs`].
//!
//! Population rules (SPEC §11.4 / §7.1 flow step 9, Invariant 14):
//!
//! - A local issue `L` (whose `(owner, repo)` matches the configured repo) is
//!   "filtered out by step 6 of §3.3" iff it has at least one non-closed
//!   blocker. Only such `L` issues contribute to `cross_repo_refs`.
//! - Issues dropped by earlier filters (`IssueState::Closed`, `Status::InProgress`,
//!   `Status::Deferred`, `Status::Closed`) are NOT "step-6-filtered" — they MUST
//!   NOT contribute cross-repo refs (otherwise a claimed issue whose blocker is
//!   cross-repo would spuriously populate the ref set).
//! - Cross-repo issues (source issues themselves whose `(owner, repo)` differs
//!   from the configured repo) are NOT in the local ready-set projection in
//!   the first place and thus MUST NOT seed cross-repo refs.
//! - For each contributing `L`, every OPEN (non-closed) cross-repo blocker of
//!   `L` is recorded in a [`BTreeSet<String>`](std::collections::BTreeSet)
//!   keyed by [`QualifiedId::Display`](unblock_core::types::QualifiedId)
//!   (`"owner/repo#number"`). The `BTreeSet` yields free de-duplication and a
//!   lexicographically-sorted output — Invariant 14 holds without an explicit
//!   `sort()` call.
//! - The field is `Some` iff the accumulator is non-empty; otherwise `None`
//!   and elided from JSON via `#[serde(skip_serializing_if = "Option::is_none")]`.
//!
//! The computation runs BEFORE the tool-layer [`filter_ready_set`] call: SPEC
//! §7.1 flow step 9 explicitly scopes the refs to the §3.3 step-6 filter, NOT
//! to the later tool-layer filters (priority, milestone, label, agent,
//! `issue_type`, `defer_until`). See `compute_cross_repo_refs` (crate-internal)
//! for the per-issue classification.

use std::collections::{BTreeSet, HashMap};

use chrono::Utc;
use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};
use rmcp::model::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{CrossRepoRefs, Issue, IssueState, IssueSummary, QualifiedId, Status};

use crate::server::ServerState;
use crate::tools::cross_repo;

/// Default number of issues returned when `limit` is not specified.
const DEFAULT_LIMIT: usize = 10;

/// Input parameters for the `ready` MCP tool.
///
/// All parameters are optional. With no parameters, returns the top 10
/// open, unblocked, non-deferred, non-claimed issues sorted by priority
/// then creation date.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadyParams {
    /// Maximum number of issues to return. Defaults to 10.
    pub limit: Option<usize>,
    /// Filter by issue type (e.g. "Task", "Bug", "Feature", "Epic", "Chore", "Spike").
    /// Case-insensitive.
    pub issue_type: Option<String>,
    /// Filter by priority level (e.g. "P0", "P1", "P2", "P3", "P4").
    /// Case-insensitive.
    pub priority: Option<String>,
    /// Filter by milestone title. Exact match.
    pub milestone: Option<String>,
    /// Filter by agent name. Exact match.
    pub agent: Option<String>,
    /// Filter by label. Returns issues that have this label (any match).
    /// Case-insensitive.
    pub label: Option<String>,
    /// If `true`, include issues with `Status::InProgress` (already claimed).
    /// Defaults to `false`.
    pub include_claimed: Option<bool>,
}

/// Result returned by the `ready` MCP tool.
///
/// Contains the filtered, sorted list of ready issues, a count, a
/// staleness indicator, and the SPEC §11.4 `cross_repo_refs` envelope
/// carrying cross-repo blockers that silently excluded local issues from
/// the ready set.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadyResult {
    /// Ready issues matching the filter criteria, sorted by priority ASC
    /// then `created_at` ASC, truncated to `limit`.
    pub issues: Vec<ReadyIssueSummary>,
    /// Number of issues returned (same as `issues.len()`).
    pub count: usize,
    /// `true` if the cache was empty or stale even after a rebuild attempt.
    /// Results may be incomplete or empty when stale.
    pub stale: bool,
    /// Cross-repo `QualifiedId` blockers that held local issues out of the
    /// ready set (SPEC §11.4, §7.1 flow step 9).
    ///
    /// `Some` iff at least one local issue would have entered the ready set
    /// but was filtered by §3.3 step 6 and at least one of its OPEN blockers
    /// was cross-repo. Otherwise `None`, in which case the field is elided
    /// from the JSON envelope via `skip_serializing_if`. Never `Some` with
    /// an empty `omitted` vector (Invariant 14, SPEC §14).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_repo_refs: Option<CrossRepoRefs>,
}

/// Lightweight issue summary for the ready result.
///
/// Re-declared from [`IssueSummary`] with `JsonSchema` derive, since core
/// types do not depend on `schemars`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadyIssueSummary {
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Issue type classification (e.g. "Task", "Bug").
    pub issue_type: Option<String>,
    /// Workflow status from Projects V2.
    pub status: String,
    /// Priority level from Projects V2.
    pub priority: String,
    /// Agent name if claimed.
    pub agent: Option<String>,
    /// Milestone title.
    pub milestone: Option<String>,
    /// Story points estimate.
    pub story_points: Option<i32>,
    /// Date until which the issue is deferred (ISO 8601 date), if set.
    pub defer_until: Option<String>,
    /// Labels attached to the issue.
    pub labels: Vec<String>,
    /// Timestamp when the issue was created (ISO 8601 / RFC 3339).
    pub created_at: String,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

impl ReadyIssueSummary {
    /// Convert from a core [`IssueSummary`] to a schema-annotated MCP result type.
    fn from_core(summary: &IssueSummary) -> Self {
        Self {
            number: summary.number,
            title: summary.title.clone(),
            issue_type: summary.issue_type.map(|it| it.to_string()),
            status: summary.status.to_string(),
            priority: summary.priority.to_string(),
            agent: summary.agent.clone(),
            milestone: summary.milestone.clone(),
            story_points: summary.story_points,
            defer_until: summary.defer_until.map(|d| d.to_string()),
            labels: summary.labels.clone(),
            created_at: summary.created_at.to_rfc3339(),
            url: summary.url.clone(),
        }
    }
}

/// Apply all ready-tool filters to a ready set.
///
/// Filters are applied in order: `issue_type`, priority, milestone, agent,
/// label, deferred exclusion, claimed exclusion. The result preserves
/// the input sort order (priority ASC, `created_at` ASC).
pub fn filter_ready_set(
    ready_set: &[IssueSummary],
    params: &ReadyParams,
) -> Vec<ReadyIssueSummary> {
    let include_claimed = params.include_claimed.unwrap_or(false);
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    let today = Utc::now().date_naive();

    // Normalize empty/whitespace string filters to None so they behave as
    // "no filter" rather than silently matching nothing. Serde deserializes
    // `"agent": ""` as `Some("")`, not `None`.
    let issue_type = crate::tools::normalize_filter(params.issue_type.as_deref());
    let priority = crate::tools::normalize_filter(params.priority.as_deref());
    let milestone = crate::tools::normalize_filter(params.milestone.as_deref());
    let agent = crate::tools::normalize_filter(params.agent.as_deref());
    let label = crate::tools::normalize_filter(params.label.as_deref());

    ready_set
        .iter()
        // Filter by issue_type (case-insensitive match).
        .filter(|s| {
            issue_type.is_none_or(|filter| {
                s.issue_type
                    .is_some_and(|it| it.to_string().eq_ignore_ascii_case(filter))
            })
        })
        // Filter by priority (case-insensitive Display match).
        .filter(|s| {
            priority.is_none_or(|filter| s.priority.to_string().eq_ignore_ascii_case(filter))
        })
        // Filter by milestone (exact match).
        .filter(|s| milestone.is_none_or(|filter| s.milestone.as_deref() == Some(filter)))
        // Filter by agent (exact match).
        .filter(|s| agent.is_none_or(|filter| s.agent.as_deref() == Some(filter)))
        // Filter by label (case-insensitive, any match).
        .filter(|s| {
            label.is_none_or(|filter| s.labels.iter().any(|l| l.eq_ignore_ascii_case(filter)))
        })
        // Exclude deferred issues (defer_until > today).
        .filter(|s| s.defer_until.is_none_or(|d| d <= today))
        // Exclude InProgress (claimed) unless include_claimed is true.
        .filter(|s| include_claimed || s.status != unblock_core::types::Status::InProgress)
        .take(limit)
        .map(ReadyIssueSummary::from_core)
        .collect()
}

/// Classify the outgoing (blocker) edges of a single node for the §11.4
/// ready cross-repo computation.
///
/// Extracted so the missing-state fallback contract can be unit-tested
/// directly. [`DependencyGraph::build`] (graph.rs:83-123) currently populates
/// `issue_state` in lock-step with `node_map` for every issue, which makes
/// the missing-state branch unreachable via the public API — but aligning
/// the predicate with the §3.3 canonical impl in
/// [`DependencyGraph::compute_ready_set`](unblock_core::graph::DependencyGraph::compute_ready_set)
/// (graph.rs:171-179) pins the contract so future relaxations of that build
/// invariant do NOT silently flip "missing-state blocker" from non-blocking
/// (graph engine) to blocking (old ready.rs fallback). See bead
/// `unblock-eos.1`.
///
/// ## Returns
///
/// - `any_open_blocker`: `true` iff at least one outgoing neighbour has
///   `issue_state == Some(IssueState::Open)`. Missing or `Closed` state ⇒
///   not blocking — mirrors `is_some_and(== Open)` at graph.rs:176-178.
/// - `cross_repo_blockers`: for every OPEN blocker whose `(owner, repo)`
///   differs from `(configured_owner, configured_repo)`, its
///   [`QualifiedId::Display`](unblock_core::types::QualifiedId) rendering.
///   Order is the petgraph iteration order — [`compute_cross_repo_refs`]
///   feeds this into a [`BTreeSet`] which provides the §11.4 dedup + lex
///   sort (Invariant 14).
///
/// Caller MUST commit `cross_repo_blockers` to the aggregate accumulator
/// ONLY when `any_open_blocker` is `true`; a local issue whose blockers are
/// all closed is not step-6-filtered and its (non-existent) cross-repo
/// blockers must not pollute the refs. Pre-inserting inside this helper
/// would regress the
/// `cross_repo_refs_closed_cross_repo_blocker_returns_none` invariant.
fn classify_ready_blockers(
    node_idx: NodeIndex,
    inner: &DiGraph<QualifiedId, ()>,
    issue_state_map: &HashMap<QualifiedId, IssueState>,
    configured_owner: &str,
    configured_repo: &str,
) -> (bool, Vec<String>) {
    let mut any_open_blocker = false;
    let mut cross_repo_blockers: Vec<String> = Vec::new();
    for blocker_idx in inner.neighbors_directed(node_idx, Direction::Outgoing) {
        let blocker_qid = &inner[blocker_idx];
        // Canonical §3.3 semantic (graph.rs:176-178): missing state is
        // treated as NOT blocking, same as Closed. Keeps the tool-layer
        // and graph-engine interpretations of "blocked" in lock-step.
        let blocker_is_open = issue_state_map
            .get(blocker_qid)
            .is_some_and(|state| *state == IssueState::Open);
        if !blocker_is_open {
            continue;
        }
        any_open_blocker = true;
        if blocker_qid.owner != configured_owner || blocker_qid.repo != configured_repo {
            cross_repo_blockers.push(blocker_qid.to_string());
        }
    }
    (any_open_blocker, cross_repo_blockers)
}

/// Compute [`CrossRepoRefs`] for the `ready` response per SPEC §11.4.
///
/// Walks the full open-issue set and surfaces every cross-repo OPEN
/// [`QualifiedId`](unblock_core::types::QualifiedId) that held a LOCAL issue
/// out of the ready set at §3.3 step 6. See the module docs for the full
/// contract; summary below.
///
/// ## Parameters
///
/// - `issues`: the UNFILTERED full issue slice as returned by
///   [`GraphCache::get_issues`](unblock_core::cache::GraphCache::get_issues) —
///   i.e. every open issue the cache knows about, in both the configured repo
///   AND any cross-repo nodes the graph references. This is NOT the
///   post-§3.3 [`IssueSummary`] ready-set projection returned by
///   [`GraphCache::get_ready_set`](unblock_core::cache::GraphCache::get_ready_set):
///   passing the already-filtered ready set here would silently drop the
///   §3.3 step-6-filtered issues this helper is designed to capture,
///   collapsing the output to `None` in every realistic scenario.
///   The SPEC §7.1 flow step 3 / step 9 split mandates this input-shape:
///   refs are scoped to step 6 ONLY, before the tool-layer `filter_ready_set`
///   call narrows the projection. See also the module-level docs.
/// - `graph`: the cached [`DependencyGraph`] — blocker edges are read from
///   its internal petgraph.
/// - `configured_owner`, `configured_repo`: the `(owner, repo)` tuple of the
///   MCP server's configured repository, used to partition local vs
///   cross-repo nodes at both source and target of each blocker edge.
///
/// ## Algorithm
///
/// For each `issue` in `issues`:
///
/// 1. Skip if `issue.state == IssueState::Closed` (filter 1 of §3.3).
/// 2. Skip if `issue.status` is `InProgress | Deferred | Closed` (filter 2
///    of §3.3). These issues are NOT "step-6-filtered" — they are filtered
///    by steps 4–5 — so their blockers do not contribute to the refs.
/// 3. Skip if `issue.qualified_id` is cross-repo (i.e. `(owner, repo)` differs
///    from the configured repo). Cross-repo issues cannot appear in the
///    bare-`u64` local ready-set projection regardless of their blockers.
/// 4. Look up the node in the graph. If absent, the issue has zero blockers
///    and is ready — skip.
/// 5. Walk outgoing edges (blockers). Partition open blockers into local
///    vs cross-repo. If at least one OPEN blocker exists (the issue would
///    be dropped by step 6), add every OPEN cross-repo blocker to the
///    accumulator. Closed OR missing-state blockers do not contribute —
///    mirrors the §3.3 canonical `is_some_and(== Open)` semantic (see
///    `unblock_core::graph::DependencyGraph::compute_ready_set`,
///    graph.rs:171-179) so the tool-layer and graph-engine interpretations
///    of "blocked" stay in lock-step. Under the current
///    [`DependencyGraph::build`] invariant missing-state is unreachable via
///    the public API (graph.rs:83-123 populates `issue_state` in lock-step
///    with `node_map`), but the alignment pins the contract so future
///    relaxations of that invariant cannot silently flip the classification.
///
/// The accumulator is a [`BTreeSet<String>`] keyed by
/// [`QualifiedId::Display`](unblock_core::types::QualifiedId). De-duplication
/// and lex ordering are free (Invariant 14, SPEC §14).
///
/// Returns `Some(CrossRepoRefs { omitted, summary })` iff the accumulator is
/// non-empty; otherwise `None` — which makes the field elide from the JSON
/// envelope via `#[serde(skip_serializing_if = "Option::is_none")]` on
/// [`ReadyResult::cross_repo_refs`].
#[must_use]
pub(crate) fn compute_cross_repo_refs(
    issues: &[Issue],
    graph: &DependencyGraph,
    configured_owner: &str,
    configured_repo: &str,
) -> Option<CrossRepoRefs> {
    let mut accum: BTreeSet<String> = BTreeSet::new();
    let node_map = graph.node_map();
    let issue_state_map = graph.issue_state();
    let inner = graph.inner_graph();

    for issue in issues {
        // Filter 1: must be open in GitHub (§3.3).
        if issue.state == IssueState::Closed {
            continue;
        }
        // Filter 2: skip preserved states (§3.3) — these are NOT step-6-filtered.
        match issue.status {
            Status::InProgress | Status::Deferred | Status::Closed => continue,
            Status::Ready | Status::Blocked => {}
        }
        // Only LOCAL source issues can be held out of the LOCAL ready set in a
        // meaningful cross-repo sense. Skip cross-repo sources.
        if issue.qualified_id.owner != configured_owner
            || issue.qualified_id.repo != configured_repo
        {
            continue;
        }

        let Some(node_idx) = node_map.get(&issue.qualified_id).copied() else {
            // Issue not in the graph: zero blockers → would be ready → not
            // filtered by step 6, contributes nothing.
            continue;
        };

        // Two-pass classification of the outgoing (blocker) edges via the
        // crate-internal helper: collect cross-repo blockers AND determine
        // whether any open blocker exists. Commit to the accumulator only
        // when the "any open blocker" predicate holds (→ the issue would be
        // step-6-filtered). See [`classify_ready_blockers`] for the
        // semantic contract.
        let (any_open_blocker, cross_repo_blockers_for_issue) = classify_ready_blockers(
            node_idx,
            inner,
            issue_state_map,
            configured_owner,
            configured_repo,
        );
        if any_open_blocker {
            for qid_display in cross_repo_blockers_for_issue {
                accum.insert(qid_display);
            }
        }
    }

    // Delegate the final envelope + summary grammar to the shared
    // primitive so all three cross-repo consumers (`ready`, `dep_cycles`,
    // `prime`) share byte-for-byte-identical empty-set and
    // singular/plural branches. See
    // [`crate::tools::cross_repo::build_cross_repo_refs_with_summary`] +
    // [`crate::tools::cross_repo::ready_summary`].
    cross_repo::build_cross_repo_refs_with_summary(accum, cross_repo::ready_summary)
}

/// Execute the `ready` tool handler.
///
/// See the module-level docs for the full contract. Flow (mirrors SPEC §7.1):
///
/// 1. Check cache freshness — lazy rebuild via [`crate::tools::rebuild_cache`]
///    when stale.
/// 2. Pull the cached ready set, full issue slice, and dependency graph.
/// 3. Compute [`CrossRepoRefs`] via the crate-internal
///    `compute_cross_repo_refs` helper using the full issue slice and the
///    graph — this captures only the §3.3 step-6 filter, NOT the tool-layer
///    filters that run next.
/// 4. Apply the tool-layer filters via [`filter_ready_set`].
/// 5. Return [`ReadyResult`] with `count = issues.len()`, `stale` set per
///    post-rebuild cache state, and the computed `cross_repo_refs`.
///
/// Extraction rationale: the tool's integration tests cannot drive the
/// `#[tool]`-wrapped entry point directly without booting
/// [`UnblockServer`](crate::server::UnblockServer); the extracted helper
/// mirrors the [`handle_dep_cycles`](crate::tools::dep_cycles::handle_dep_cycles)
/// house style adopted by sibling tools (unblock-29p.11).
///
/// # Errors
///
/// Returns [`ErrorData`] only for surfaced upstream failures. The current
/// implementation never returns an error — cache-rebuild failures degrade to
/// `stale = true` per SPEC §7.1 rather than surfacing an error. The return
/// type is kept as `Result` so future spec revisions can surface errors
/// without a breaking signature change.
#[instrument(
    skip(state, params),
    name = "handle_ready",
    fields(
        agent.kind = state.agent_kind_str(),
        limit = params.limit,
        issue_type = params.issue_type.as_deref(),
        priority = params.priority.as_deref(),
        milestone = params.milestone.as_deref(),
        agent = params.agent.as_deref(),
        label = params.label.as_deref(),
        include_claimed = params.include_claimed,
    ),
)]
pub async fn handle_ready(
    state: &ServerState,
    params: ReadyParams,
) -> Result<ReadyResult, ErrorData> {
    info!("Ready tool invoked");

    // Step 1: warm the cache lazily if stale. On rebuild failure the cache
    // stays invalidated — the read-side logic below will observe `stale=true`
    // and return an empty ready set (SPEC §7.1 cache-failure posture).
    if !state.cache.is_fresh().await {
        tracing::debug!("Cache is stale — triggering lazy rebuild");
        crate::tools::rebuild_cache(state).await;
    }

    // Step 2: pull ready_set, issues, and graph from the cache. All three
    // are written atomically by `GraphCache::update`, so observing `ready_set
    // = Some(_)` but `issues = None` (or vice versa) would indicate a
    // cache-layer bug — defensively treat it as stale.
    let configured_owner = state.github.owner().to_owned();
    let configured_repo = state.github.repo().to_owned();

    let ready_set_opt = state.cache.get_ready_set().await;
    let issues_opt = state.cache.get_issues().await;
    let graph_opt = state.cache.get_graph().await;

    let (is_stale, filtered_issues, cross_repo_refs) = match (ready_set_opt, issues_opt, graph_opt)
    {
        (Some(ready_set), Some(issues), Some(graph)) => {
            // Step 3: compute cross-repo refs against the FULL issue slice
            // and graph — scoped to §3.3 step 6 only.
            let refs =
                compute_cross_repo_refs(&issues, &graph, &configured_owner, &configured_repo);
            // Step 4: tool-layer filters over the already-filtered ready set.
            let filtered = filter_ready_set(&ready_set, &params);
            (false, filtered, refs)
        }
        (Some(ready_set), _, _) => {
            // Partial cache — should not happen in practice (see cache.rs
            // atomicity guarantees). Surface filtered issues without refs.
            tracing::warn!(
                "Cache returned ready_set but not issues/graph — omitting cross_repo_refs"
            );
            let filtered = filter_ready_set(&ready_set, &params);
            (false, filtered, None)
        }
        _ => {
            // Cache still empty after rebuild attempt (e.g. fetch failed).
            tracing::warn!("Cache still empty after rebuild — returning stale=true");
            (true, Vec::new(), None)
        }
    };

    let count = filtered_issues.len();
    Ok(ReadyResult {
        issues: filtered_issues,
        count,
        stale: is_stale,
        cross_repo_refs,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};
    use unblock_core::types::{IssueSummary, IssueType, Priority, QualifiedId, Status};

    use super::*;

    /// Build a minimal `IssueSummary` for testing.
    fn test_summary(number: u64, priority: Priority) -> IssueSummary {
        IssueSummary {
            qualified_id: QualifiedId::new("test", "repo", number),
            number,
            title: format!("Issue #{number}"),
            issue_type: Some(IssueType::Task),
            status: Status::Ready,
            priority,
            agent: None,
            milestone: None,
            story_points: None,
            defer_until: None,
            labels: vec![],
            created_at: Utc::now(),
            url: format!("https://github.com/test/repo/issues/{number}"),
        }
    }

    fn default_params() -> ReadyParams {
        ReadyParams {
            limit: None,
            issue_type: None,
            priority: None,
            milestone: None,
            agent: None,
            label: None,
            include_claimed: None,
        }
    }

    // ── Basic filtering ──────────────────────────────────────────────

    #[test]
    fn empty_ready_set_returns_empty() {
        let result = filter_ready_set(&[], &default_params());
        assert!(result.is_empty());
    }

    #[test]
    fn returns_all_when_no_filters() {
        let set = vec![test_summary(1, Priority::P0), test_summary(2, Priority::P1)];
        let result = filter_ready_set(&set, &default_params());
        assert_eq!(result.len(), 2);
    }

    // ── Limit ────────────────────────────────────────────────────────

    #[test]
    fn limit_truncates_results() {
        let set: Vec<_> = (1..=20).map(|n| test_summary(n, Priority::P2)).collect();
        let params = ReadyParams {
            limit: Some(5),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn default_limit_is_10() {
        let set: Vec<_> = (1..=15).map(|n| test_summary(n, Priority::P2)).collect();
        let result = filter_ready_set(&set, &default_params());
        assert_eq!(result.len(), 10);
    }

    // ── Priority filter ──────────────────────────────────────────────

    #[test]
    fn filter_by_priority() {
        let set = vec![
            test_summary(1, Priority::P0),
            test_summary(2, Priority::P1),
            test_summary(3, Priority::P0),
        ];
        let params = ReadyParams {
            priority: Some("P0".to_owned()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|r| r.priority == "P0"));
    }

    #[test]
    fn filter_by_priority_case_insensitive() {
        let set = vec![test_summary(1, Priority::P0)];
        let params = ReadyParams {
            priority: Some("p0".to_owned()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 1);
    }

    // ── Issue type filter ────────────────────────────────────────────

    #[test]
    fn filter_by_issue_type() {
        let mut set = vec![test_summary(1, Priority::P1)];
        set[0].issue_type = Some(IssueType::Bug);
        let mut s2 = test_summary(2, Priority::P1);
        s2.issue_type = Some(IssueType::Task);
        set.push(s2);

        let params = ReadyParams {
            issue_type: Some("Bug".to_owned()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].number, 1);
    }

    #[test]
    fn filter_by_issue_type_case_insensitive() {
        let mut set = vec![test_summary(1, Priority::P1)];
        set[0].issue_type = Some(IssueType::Feature);
        let params = ReadyParams {
            issue_type: Some("feature".to_owned()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 1);
    }

    // ── Label filter ─────────────────────────────────────────────────

    #[test]
    fn filter_by_label() {
        let mut s1 = test_summary(1, Priority::P1);
        s1.labels = vec!["urgent".to_owned(), "backend".to_owned()];
        let mut s2 = test_summary(2, Priority::P1);
        s2.labels = vec!["frontend".to_owned()];
        let set = vec![s1, s2];

        let params = ReadyParams {
            label: Some("urgent".to_owned()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].number, 1);
    }

    #[test]
    fn filter_by_label_case_insensitive() {
        let mut s1 = test_summary(1, Priority::P1);
        s1.labels = vec!["Urgent".to_owned()];
        let set = vec![s1];

        let params = ReadyParams {
            label: Some("urgent".to_owned()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 1);
    }

    // ── Milestone filter ─────────────────────────────────────────────

    #[test]
    fn filter_by_milestone() {
        let mut s1 = test_summary(1, Priority::P1);
        s1.milestone = Some("v1.0".to_owned());
        let mut s2 = test_summary(2, Priority::P1);
        s2.milestone = Some("v2.0".to_owned());
        let set = vec![s1, s2];

        let params = ReadyParams {
            milestone: Some("v1.0".to_owned()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].number, 1);
    }

    // ── Agent filter ─────────────────────────────────────────────────

    #[test]
    fn filter_by_agent() {
        let mut s1 = test_summary(1, Priority::P1);
        s1.agent = Some("agent-x".to_owned());
        s1.status = Status::InProgress;
        let s2 = test_summary(2, Priority::P1);
        let set = vec![s1, s2];

        let params = ReadyParams {
            agent: Some("agent-x".to_owned()),
            include_claimed: Some(true),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].number, 1);
    }

    // ── Empty string filter normalization ──────────────────────────────

    #[test]
    fn empty_agent_string_treated_as_no_filter() {
        let set = vec![test_summary(1, Priority::P0), test_summary(2, Priority::P1)];
        let params = ReadyParams {
            agent: Some(String::new()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(
            result.len(),
            2,
            "empty agent string should not filter out any issues"
        );
    }

    #[test]
    fn whitespace_agent_string_treated_as_no_filter() {
        let set = vec![test_summary(1, Priority::P0)];
        let params = ReadyParams {
            agent: Some("   ".to_owned()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(
            result.len(),
            1,
            "whitespace-only agent string should not filter out any issues"
        );
    }

    #[test]
    fn empty_issue_type_string_treated_as_no_filter() {
        let set = vec![test_summary(1, Priority::P0)];
        let params = ReadyParams {
            issue_type: Some(String::new()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(
            result.len(),
            1,
            "empty issue_type string should not filter out any issues"
        );
    }

    #[test]
    fn empty_priority_string_treated_as_no_filter() {
        let set = vec![test_summary(1, Priority::P0)];
        let params = ReadyParams {
            priority: Some(String::new()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(
            result.len(),
            1,
            "empty priority string should not filter out any issues"
        );
    }

    #[test]
    fn empty_milestone_string_treated_as_no_filter() {
        let set = vec![test_summary(1, Priority::P0)];
        let params = ReadyParams {
            milestone: Some(String::new()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(
            result.len(),
            1,
            "empty milestone string should not filter out any issues"
        );
    }

    #[test]
    fn empty_label_string_treated_as_no_filter() {
        let mut s1 = test_summary(1, Priority::P0);
        s1.labels = vec!["backend".to_owned()];
        let set = vec![s1];
        let params = ReadyParams {
            label: Some(String::new()),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(
            result.len(),
            1,
            "empty label string should not filter out any issues"
        );
    }

    // ── Deferred exclusion ───────────────────────────────────────────

    #[test]
    fn excludes_deferred_issues() {
        let mut s1 = test_summary(1, Priority::P1);
        // Defer until far future.
        s1.defer_until = Some(NaiveDate::from_ymd_opt(2099, 12, 31).unwrap());
        let s2 = test_summary(2, Priority::P1);
        let set = vec![s1, s2];

        let result = filter_ready_set(&set, &default_params());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].number, 2);
    }

    #[test]
    fn includes_past_deferred_issues() {
        let mut s1 = test_summary(1, Priority::P1);
        // Defer until yesterday — should be included.
        s1.defer_until = Some(
            Utc::now()
                .date_naive()
                .pred_opt()
                .unwrap_or(Utc::now().date_naive()),
        );
        let set = vec![s1];

        let result = filter_ready_set(&set, &default_params());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn includes_today_deferred_issues() {
        let mut s1 = test_summary(1, Priority::P1);
        // Defer until today — should be included (defer_until <= today).
        s1.defer_until = Some(Utc::now().date_naive());
        let set = vec![s1];

        let result = filter_ready_set(&set, &default_params());
        assert_eq!(result.len(), 1);
    }

    // ── InProgress exclusion ─────────────────────────────────────────

    #[test]
    fn excludes_in_progress_by_default() {
        let mut s1 = test_summary(1, Priority::P1);
        s1.status = Status::InProgress;
        s1.agent = Some("agent-a".to_owned());
        let s2 = test_summary(2, Priority::P1);
        let set = vec![s1, s2];

        let result = filter_ready_set(&set, &default_params());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].number, 2);
    }

    #[test]
    fn include_claimed_includes_in_progress() {
        let mut s1 = test_summary(1, Priority::P1);
        s1.status = Status::InProgress;
        s1.agent = Some("agent-a".to_owned());
        let s2 = test_summary(2, Priority::P1);
        let set = vec![s1, s2];

        let params = ReadyParams {
            include_claimed: Some(true),
            ..default_params()
        };
        let result = filter_ready_set(&set, &params);
        assert_eq!(result.len(), 2);
    }

    // ── Sort order preserved ─────────────────────────────────────────

    #[test]
    fn sort_order_p0_before_p1() {
        let earlier = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        let mut s1 = test_summary(1, Priority::P1);
        s1.created_at = earlier;
        let mut s2 = test_summary(2, Priority::P0);
        s2.created_at = later;
        // Input is already sorted by compute_ready_set: P0 first.
        let set = vec![s2, s1];

        let result = filter_ready_set(&set, &default_params());
        assert_eq!(result[0].number, 2); // P0
        assert_eq!(result[1].number, 1); // P1
    }

    #[test]
    fn sort_order_same_priority_by_created_at() {
        let earlier = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        let mut s1 = test_summary(1, Priority::P1);
        s1.created_at = earlier;
        let mut s2 = test_summary(2, Priority::P1);
        s2.created_at = later;
        // Input sorted by created_at ASC.
        let set = vec![s1, s2];

        let result = filter_ready_set(&set, &default_params());
        assert_eq!(result[0].number, 1); // earlier
        assert_eq!(result[1].number, 2); // later
    }

    // ── compute_cross_repo_refs — hermetic unit tests (SPEC §11.4) ─────

    use unblock_core::graph::DependencyGraph;
    use unblock_core::types::{BlockingEdge, Issue, IssueState};

    /// Build a full [`Issue`] for graph construction. `owner`/`repo` are
    /// configurable so tests can mix local and cross-repo issues.
    fn issue_at(owner: &str, repo: &str, number: u64, state: IssueState, status: Status) -> Issue {
        Issue {
            qualified_id: QualifiedId::new(owner, repo, number),
            number,
            node_id: format!("I_{owner}_{repo}_{number}"),
            title: format!("{owner}/{repo}#{number}"),
            issue_type: Some(IssueType::Task),
            status,
            priority: Priority::P2,
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
            url: format!("https://github.com/{owner}/{repo}/issues/{number}"),
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    /// Case (i): empty issue slice → `None`, never `Some` with empty omitted.
    #[test]
    fn cross_repo_refs_empty_issues_returns_none() {
        let issues: Vec<Issue> = vec![];
        let graph = DependencyGraph::build(&issues, &[]);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets");
        assert!(refs.is_none(), "empty issue slice → None");
    }

    /// Case (ii): local issue blocked by local open blocker → no cross-repo
    /// member → `None`. The step-6 filter fires but contributes nothing.
    #[test]
    fn cross_repo_refs_all_local_blockers_returns_none() {
        let issues = vec![
            // Local issue #1 is open and ready but blocked by local #2 (open).
            issue_at("acme", "widgets", 1, IssueState::Open, Status::Ready),
            issue_at("acme", "widgets", 2, IssueState::Open, Status::Ready),
        ];
        let edges = vec![BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 1),
            target: QualifiedId::new("acme", "widgets", 2),
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets");
        assert!(refs.is_none(), "all-local blockers → None");
    }

    /// Case (iii): local issue blocked by cross-repo OPEN blocker → `Some`,
    /// with `"other/repo#99"` in `omitted` and a populated summary.
    #[test]
    fn cross_repo_refs_single_cross_repo_open_blocker_returns_some() {
        let issues = vec![
            issue_at("acme", "widgets", 1, IssueState::Open, Status::Ready),
            // Cross-repo OPEN blocker.
            issue_at("other", "repo", 99, IssueState::Open, Status::Ready),
        ];
        let edges = vec![BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 1),
            target: QualifiedId::new("other", "repo", 99),
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets")
            .expect("SPEC §11.4: cross-repo OPEN blocker → Some");
        assert_eq!(refs.omitted, vec!["other/repo#99".to_owned()]);
        let summary = refs
            .summary
            .as_deref()
            .expect("summary populated when omitted non-empty");
        assert!(
            summary.contains("cross-repo"),
            "summary must describe cross-repo: {summary}"
        );
        assert!(
            summary.contains("ready set"),
            "summary must reference `ready set`: {summary}"
        );
    }

    /// Case (iv): the cross-repo blocker is already CLOSED → does not hold
    /// the issue out of the ready set → refs empty → `None`.
    #[test]
    fn cross_repo_refs_closed_cross_repo_blocker_returns_none() {
        let issues = vec![
            issue_at("acme", "widgets", 1, IssueState::Open, Status::Ready),
            // Cross-repo CLOSED blocker — does not filter out #1.
            issue_at("other", "repo", 99, IssueState::Closed, Status::Closed),
        ];
        let edges = vec![BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 1),
            target: QualifiedId::new("other", "repo", 99),
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets");
        assert!(
            refs.is_none(),
            "closed cross-repo blocker cannot hold issue out of ready set → None",
        );
    }

    /// Case (v): the SAME cross-repo blocker participates in filtering two
    /// distinct local issues → `BTreeSet` de-dupes → single entry in `omitted`.
    #[test]
    fn cross_repo_refs_duplicate_blocker_across_issues_dedupes() {
        let issues = vec![
            issue_at("acme", "widgets", 1, IssueState::Open, Status::Ready),
            issue_at("acme", "widgets", 2, IssueState::Open, Status::Ready),
            issue_at("other", "repo", 42, IssueState::Open, Status::Ready),
        ];
        let edges = vec![
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 1),
                target: QualifiedId::new("other", "repo", 42),
            },
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 2),
                target: QualifiedId::new("other", "repo", 42),
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets")
            .expect("two local filter-outs → Some");
        assert_eq!(
            refs.omitted,
            vec!["other/repo#42".to_owned()],
            "same cross-repo blocker must dedupe to a single omitted entry",
        );
        let summary = refs.summary.as_deref().expect("summary populated");
        assert!(
            summary.starts_with("1 "),
            "singular-noun summary for single omitted entry: {summary}"
        );
        assert!(
            summary.contains("blocker") && !summary.contains("blockers"),
            "singular noun only: {summary}"
        );
    }

    /// Case (vi): multiple distinct cross-repo blockers → `omitted` emerges
    /// lex-sorted from the `BTreeSet` (Invariant 14, no explicit `sort()`).
    #[test]
    fn cross_repo_refs_multiple_blockers_sorted_lexicographically() {
        let issues = vec![
            issue_at("acme", "widgets", 1, IssueState::Open, Status::Ready),
            issue_at("zeta", "repo", 3, IssueState::Open, Status::Ready),
            issue_at("alpha", "repo", 1, IssueState::Open, Status::Ready),
            issue_at("mid", "repo", 2, IssueState::Open, Status::Ready),
        ];
        let edges = vec![
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 1),
                target: QualifiedId::new("zeta", "repo", 3),
            },
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 1),
                target: QualifiedId::new("alpha", "repo", 1),
            },
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 1),
                target: QualifiedId::new("mid", "repo", 2),
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets")
            .expect("three cross-repo blockers → Some");
        assert_eq!(
            refs.omitted,
            vec![
                "alpha/repo#1".to_owned(),
                "mid/repo#2".to_owned(),
                "zeta/repo#3".to_owned(),
            ],
            "Invariant 14: omitted MUST emerge lexicographically sorted from BTreeSet",
        );
        let summary = refs.summary.as_deref().expect("summary populated");
        assert!(summary.contains("3 "));
        assert!(
            summary.contains("blockers"),
            "plural-noun summary for multiple omitted: {summary}"
        );
    }

    /// Risk: `InProgress` local issue MUST NOT contribute to refs even when
    /// it has a cross-repo open blocker — such issues are filtered by step 5
    /// (claimed), NOT step 6 (blocked). This guards against a regression
    /// that spuriously reports blockers of claimed work.
    #[test]
    fn cross_repo_refs_in_progress_local_issue_does_not_contribute() {
        let issues = vec![
            // InProgress local issue — filtered by §3.3 step 5 (claimed),
            // not step 6 (blocked). Its cross-repo blocker must NOT surface.
            issue_at("acme", "widgets", 1, IssueState::Open, Status::InProgress),
            issue_at("other", "repo", 99, IssueState::Open, Status::Ready),
        ];
        let edges = vec![BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 1),
            target: QualifiedId::new("other", "repo", 99),
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets");
        assert!(
            refs.is_none(),
            "InProgress local issue is filtered by step 5, not step 6 → no cross-repo refs",
        );
    }

    /// Risk: `Deferred` and `Closed` local statuses follow the same rule as
    /// `InProgress` — filtered by §3.3 step 2, NOT step 6. Their cross-repo
    /// blockers must not appear.
    #[test]
    fn cross_repo_refs_deferred_and_closed_status_do_not_contribute() {
        let issues = vec![
            issue_at("acme", "widgets", 1, IssueState::Open, Status::Deferred),
            issue_at("acme", "widgets", 2, IssueState::Open, Status::Closed),
            issue_at("other", "repo", 50, IssueState::Open, Status::Ready),
            issue_at("other", "repo", 51, IssueState::Open, Status::Ready),
        ];
        let edges = vec![
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 1),
                target: QualifiedId::new("other", "repo", 50),
            },
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 2),
                target: QualifiedId::new("other", "repo", 51),
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets");
        assert!(
            refs.is_none(),
            "Deferred/Closed-status issues are step-2-filtered, not step-6 → no refs",
        );
    }

    /// Risk: a cross-repo SOURCE issue with a cross-repo blocker MUST NOT
    /// contribute — it is not a member of the local ready-set projection.
    /// Without this guard, cross-repo chains would pollute the refs.
    #[test]
    fn cross_repo_refs_cross_repo_source_issue_does_not_contribute() {
        let issues = vec![
            // Source issue lives in other/repo — not part of acme/widgets ready set.
            issue_at("other", "repo", 10, IssueState::Open, Status::Ready),
            // Its blocker is also cross-repo.
            issue_at("third", "party", 20, IssueState::Open, Status::Ready),
        ];
        let edges = vec![BlockingEdge {
            source: QualifiedId::new("other", "repo", 10),
            target: QualifiedId::new("third", "party", 20),
        }];
        let graph = DependencyGraph::build(&issues, &edges);
        let refs = compute_cross_repo_refs(&issues, &graph, "acme", "widgets");
        assert!(
            refs.is_none(),
            "cross-repo source issue cannot populate local-ready-set cross-repo refs",
        );
    }

    /// Missing-state fallback contract (bead `unblock-eos.1`): when the
    /// graph holds a blocker edge to a node that is absent from
    /// `issue_state`, [`classify_ready_blockers`] MUST treat that blocker as
    /// NOT blocking — mirroring the §3.3 canonical predicate
    /// `is_some_and(== Open)` at `crates/unblock-core/src/graph.rs:176-178`
    /// used by [`DependencyGraph::compute_ready_set`]. The old fallback
    /// (`unwrap_or(IssueState::Open)`) treated missing as blocking and
    /// would have spuriously surfaced the cross-repo blocker in
    /// `cross_repo_refs.omitted`.
    ///
    /// Under the current [`DependencyGraph::build`] invariant
    /// (`crates/unblock-core/src/graph.rs:83-123` populates `issue_state`
    /// and `node_map` in lock-step for every issue, and edges to unknown
    /// [`QualifiedId`]s are dropped at graph.rs:107-114), this branch is
    /// unreachable via the public API. The test constructs a divergent
    /// state map directly to pin the contract: if the build invariant ever
    /// loosens, this test prevents a silent flip of the classification.
    #[test]
    fn classify_ready_blockers_missing_blocker_state_treated_as_not_blocking() {
        // Build a well-formed graph: issue #1 blocked by cross-repo #99,
        // both registered as Open. Under DependencyGraph::build the
        // issue_state map contains BOTH entries in lock-step with node_map.
        let issues = vec![
            issue_at("acme", "widgets", 1, IssueState::Open, Status::Ready),
            issue_at("other", "repo", 99, IssueState::Open, Status::Ready),
        ];
        let edges = vec![BlockingEdge {
            source: QualifiedId::new("acme", "widgets", 1),
            target: QualifiedId::new("other", "repo", 99),
        }];
        let graph = DependencyGraph::build(&issues, &edges);

        // Sanity check: with the full state map, #99 IS classified as an
        // open cross-repo blocker (baseline behaviour — same as the
        // `cross_repo_refs_single_cross_repo_open_blocker_returns_some`
        // test at ready.rs:913).
        let node_idx_1 = graph
            .node_map()
            .get(&QualifiedId::new("acme", "widgets", 1))
            .copied()
            .expect("issue #1 must be in node_map");
        let inner = graph.inner_graph();
        let full_state = graph.issue_state().clone();
        let (any_blocker_full, cross_repo_full) =
            classify_ready_blockers(node_idx_1, inner, &full_state, "acme", "widgets");
        assert!(
            any_blocker_full,
            "baseline: with complete state map, the Open cross-repo blocker holds #1 out of the ready set"
        );
        assert_eq!(
            cross_repo_full,
            vec!["other/repo#99".to_owned()],
            "baseline: cross-repo blocker is captured in the per-issue vec"
        );

        // Now construct a DIVERGENT state map with the blocker key
        // removed. This simulates a future relaxation of the build
        // invariant where an edge may target a node whose state has not
        // been recorded. Per §3.3 canonical semantics, the blocker MUST
        // NOT count as holding #1 out of the ready set.
        let mut divergent_state = full_state.clone();
        let removed = divergent_state.remove(&QualifiedId::new("other", "repo", 99));
        assert!(
            removed.is_some(),
            "pre-condition: full state map must contain the blocker entry before we strip it"
        );

        let (any_blocker_missing, cross_repo_missing) =
            classify_ready_blockers(node_idx_1, inner, &divergent_state, "acme", "widgets");
        assert!(
            !any_blocker_missing,
            "§3.3 canonical contract: missing-state blocker is NOT blocking (graph.rs:176-178). Old ready.rs fallback would have flipped this to true and spuriously surfaced #99 in cross_repo_refs.omitted."
        );
        assert!(
            cross_repo_missing.is_empty(),
            "missing-state blocker MUST NOT be recorded as a cross-repo blocker: found {cross_repo_missing:?}"
        );
    }

    /// Determinism: re-running `compute_cross_repo_refs` on the same inputs
    /// produces byte-identical output. `BTreeSet` iteration is stable; this
    /// is the property-test goal (SPEC §13.3) reduced to an example-based
    /// smoke check.
    #[test]
    fn cross_repo_refs_output_is_deterministic_across_runs() {
        let issues = vec![
            issue_at("acme", "widgets", 1, IssueState::Open, Status::Ready),
            issue_at("z", "r", 3, IssueState::Open, Status::Ready),
            issue_at("a", "r", 1, IssueState::Open, Status::Ready),
        ];
        let edges = vec![
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 1),
                target: QualifiedId::new("z", "r", 3),
            },
            BlockingEdge {
                source: QualifiedId::new("acme", "widgets", 1),
                target: QualifiedId::new("a", "r", 1),
            },
        ];
        let graph = DependencyGraph::build(&issues, &edges);
        let first = compute_cross_repo_refs(&issues, &graph, "acme", "widgets");
        let second = compute_cross_repo_refs(&issues, &graph, "acme", "widgets");
        assert_eq!(
            first, second,
            "compute_cross_repo_refs must be deterministic (Invariant 14)"
        );
    }
}
