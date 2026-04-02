//! Prime tool — session entry point for every agent session.
//!
//! Aggregates issue state into categorised lists (`in_progress`, `ready`, `blocked`,
//! `completed`, `hotspots`, `stale`) so the agent can orient itself at the start of
//! a session without calling multiple individual tools.
//!
//! This is a read tool that always performs a fresh fetch from GitHub (bypasses
//! cache) because the cache only stores the ready set — categorising `in_progress`,
//! `blocked`, and `stale` requires the full `Issue` list with status and `claimed_at`
//! fields. After the fetch, the cache is updated with the fresh graph data.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{Issue, IssueState, IssueSummary, QualifiedId, Status};

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;
use crate::tools::reconcile::{ReconcileParams, ReconcileReport, handle_reconcile};

/// Minimum allowed value for `stale_threshold_hours` (must be at least 1).
const MIN_STALE_THRESHOLD_HOURS: u64 = 1;

/// Minimum allowed value for `max_per_category` (must be at least 1).
const MIN_MAX_PER_CATEGORY: usize = 1;

/// Default number of hours before a claim is considered stale.
const DEFAULT_STALE_THRESHOLD_HOURS: u64 = 24;

/// Default maximum number of items per category in the output.
const DEFAULT_MAX_PER_CATEGORY: usize = 10;

/// Input parameters for the `prime` MCP tool.
///
/// All parameters are optional. With no parameters, returns up to 10 items
/// per category with a 24-hour stale claim threshold.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PrimeParams {
    /// Hours without activity before considering a claim stale. Default: 24.
    /// Must be at least 1; zero is rejected with an `INVALID_PARAMS` error.
    pub stale_threshold_hours: Option<u64>,
    /// Maximum number of items per category to return. Controls output size.
    /// Default: 10. Must be at least 1; zero is rejected with an
    /// `INVALID_PARAMS` error.
    pub max_per_category: Option<usize>,
    /// Filter all categories by agent name. Exact match. When set, only
    /// issues claimed by this agent appear in `in_progress`, `ready`,
    /// `blocked`, and `stale`. The `completed` and `hotspots` categories
    /// are never filtered (global continuity context and structural graph
    /// properties respectively).
    pub agent: Option<String>,
}

/// Output from the `prime` MCP tool.
///
/// Provides a rich session context with smart prioritisation so agents can
/// orient themselves at the start of every session.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PrimeResult {
    /// Issues currently being worked on (`Status::InProgress`, `IssueState::Open`).
    pub in_progress: Vec<PrimeIssueSummary>,
    /// Issues with no active blockers that can be picked up.
    pub ready: Vec<PrimeIssueSummary>,
    /// Issues that have at least one open blocker.
    pub blocked: Vec<PrimeIssueSummary>,
    /// Issues closed within the recent window (default 24h) for continuity.
    /// Lets agents see what was recently shipped before picking up new work.
    pub completed: Vec<CompletedIssueSummary>,
    /// Issues that block the most other issues (most-blocking first).
    pub hotspots: Vec<HotspotSummary>,
    /// In-progress issues with `claimed_at` older than the stale threshold.
    pub stale: Vec<StaleIssueSummary>,
    /// Session metadata (populated by Epic 1.5; stub `Unknown` until then).
    pub session: SessionMeta,
    /// Drift warnings from background reconciliation (populated by Epic 1.6;
    /// `None` until then).
    pub drift_warnings: Option<Vec<String>>,
    /// Summary counts for quick orientation.
    pub counts: PrimeCounts,
}

/// Summary counts for the prime result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PrimeCounts {
    /// Total number of in-progress issues (before truncation).
    pub in_progress: usize,
    /// Total number of ready issues (before truncation).
    pub ready: usize,
    /// Total number of blocked issues (before truncation).
    pub blocked: usize,
    /// Total number of recently completed issues (before truncation).
    pub completed: usize,
    /// Total number of hotspot issues (before truncation).
    pub hotspots: usize,
    /// Total number of stale claims (before truncation).
    pub stale: usize,
}

/// Lightweight issue summary for prime result categories.
///
/// Re-declared from [`IssueSummary`] with `JsonSchema` derive, since core
/// types do not depend on `schemars`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PrimeIssueSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub qualified_id: String,
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
    /// Labels attached to the issue.
    pub labels: Vec<String>,
    /// Timestamp when the issue was created (ISO 8601 / RFC 3339).
    pub created_at: String,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

impl PrimeIssueSummary {
    /// Convert from a core [`IssueSummary`] to a schema-annotated MCP result type.
    fn from_core(summary: &IssueSummary) -> Self {
        Self {
            qualified_id: summary.qualified_id.to_string(),
            number: summary.number,
            title: summary.title.clone(),
            issue_type: summary.issue_type.map(|it| format!("{it:?}")),
            status: format!("{:?}", summary.status),
            priority: format!("{:?}", summary.priority),
            agent: summary.agent.clone(),
            milestone: summary.milestone.clone(),
            labels: summary.labels.clone(),
            created_at: summary.created_at.to_rfc3339(),
            url: summary.url.clone(),
        }
    }
}

/// A hotspot: an issue that blocks many other issues.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HotspotSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub qualified_id: String,
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Workflow status from Projects V2.
    pub status: String,
    /// Priority level from Projects V2.
    pub priority: String,
    /// Number of issues this issue is blocking.
    pub blocking_count: usize,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

/// A stale claim: an in-progress issue with `claimed_at` older than threshold.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StaleIssueSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub qualified_id: String,
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Agent name that claimed the issue.
    pub agent: Option<String>,
    /// Timestamp when the issue was claimed (ISO 8601 / RFC 3339).
    pub claimed_at: String,
    /// Hours since the issue was claimed.
    pub hours_stale: u64,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

/// A recently completed issue: closed within the configurable time window.
///
/// Provides continuity context so agents can see what was recently shipped
/// before picking up new work (PRD §6.3).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CompletedIssueSummary {
    /// Fully qualified issue identifier (`owner/repo#number`).
    pub qualified_id: String,
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Issue type classification (e.g. "Task", "Bug").
    pub issue_type: Option<String>,
    /// Priority level from Projects V2.
    pub priority: String,
    /// Approximate close time (derived from `updated_at` since GitHub
    /// `closedAt` is not currently fetched).
    pub closed_at: String,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

/// Session metadata populated from [`ServerState`] during each `prime` call.
///
/// Surfaces the connected MCP client identity, the resolved agent kind,
/// an optional operator-defined agent field (`UNBLOCK_AGENT` env var),
/// and the session start timestamp.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SessionMeta {
    /// Raw MCP `clientInfo.name` (e.g., "Claude Code", "GitHub Copilot Chat").
    pub agent_client: String,
    /// Normalised agent kind string (e.g., "claude-code", "copilot", "unknown").
    pub agent_kind: String,
    /// Value of the `UNBLOCK_AGENT` env var if set by the operator.
    /// `None` when the variable is not present in the environment.
    pub agent_field: Option<String>,
    /// UTC timestamp when the MCP session was initialised (ISO 8601 / RFC 3339).
    pub connected_at: DateTime<Utc>,
}

impl SessionMeta {
    /// Build [`SessionMeta`] from the live [`ServerState`].
    ///
    /// Reads `agent_kind` and `agent_client` from their respective
    /// [`OnceLock`](std::sync::OnceLock) fields (lock-free). Falls back to
    /// `"unknown"` when the locks have not been set (e.g., in tests where
    /// `initialize()` is not called).
    ///
    /// `agent_field` is read from the `UNBLOCK_AGENT` environment variable
    /// on every call (not cached), returning `None` when unset.
    ///
    /// `connected_at` falls back to `Utc::now()` when the `OnceLock` has not
    /// been set, ensuring tests that skip `initialize()` still get a valid
    /// timestamp.
    #[must_use]
    pub fn from_state(state: &ServerState) -> Self {
        let agent_client = state
            .agent_client
            .get()
            .map_or_else(|| "unknown".to_owned(), |c| c.name.clone());

        let agent_kind = state.agent_kind_str().to_owned();

        let agent_field = std::env::var("UNBLOCK_AGENT").ok();

        let connected_at = state.connected_at.get().copied().unwrap_or_else(Utc::now);

        Self {
            agent_client,
            agent_kind,
            agent_field,
            connected_at,
        }
    }
}

/// Execute the prime tool handler.
///
/// # Flow
///
/// 1. Spawn a background read-only reconcile via `tokio::spawn` (Design Decision R5).
/// 2. Fresh fetch via `fetch_graph_data()` — bypasses cache entirely.
/// 3. Rebuild `DependencyGraph` from scratch.
/// 4. Categorise all issues into `in_progress`, `blocked`, `ready`, `completed`, `hotspots`, `stale`.
/// 5. Update cache with the fresh graph already fetched.
/// 6. Await the drift check — if drift is found, populate `drift_warnings`.
/// 7. Return `PrimeResult` with session metadata and drift warnings.
///
/// The background reconcile runs concurrently with the prime fetch and does not
/// block the response path. If it fails or panics, `drift_warnings` is simply
/// `None` — prime never fails due to reconcile errors.
///
/// # Errors
///
/// Returns [`rmcp::model::ErrorData`] with `INVALID_PARAMS` if `stale_threshold_hours`
/// or `max_per_category` is below its minimum (currently 1), or if the GitHub fetch fails.
#[allow(clippy::too_many_lines)]
pub async fn handle_prime(
    params: &PrimeParams,
    state: &Arc<ServerState>,
) -> Result<PrimeResult, rmcp::model::ErrorData> {
    // Validate boundary values before proceeding.
    if let Some(hours) = params.stale_threshold_hours
        && hours < MIN_STALE_THRESHOLD_HOURS
    {
        return Err(rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_PARAMS,
            message: format!(
                "stale_threshold_hours must be at least {MIN_STALE_THRESHOLD_HOURS}, got {hours}"
            )
            .into(),
            data: None,
        });
    }
    if let Some(max) = params.max_per_category
        && max < MIN_MAX_PER_CATEGORY
    {
        return Err(rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_PARAMS,
            message: format!("max_per_category must be at least {MIN_MAX_PER_CATEGORY}, got {max}")
                .into(),
            data: None,
        });
    }

    let stale_threshold_hours = params
        .stale_threshold_hours
        .unwrap_or(DEFAULT_STALE_THRESHOLD_HOURS);
    let max_per_category = params.max_per_category.unwrap_or(DEFAULT_MAX_PER_CATEGORY);

    info!(
        stale_threshold_hours,
        max_per_category, "Prime tool invoked"
    );

    // 1. Spawn background read-only reconcile (Design Decision R5).
    //    Runs concurrently with the prime fetch. If it fails or panics,
    //    drift_warnings is simply None — prime never fails due to reconcile.
    let drift_check = tokio::spawn({
        let state = Arc::clone(state);
        async move {
            let reconcile_params = ReconcileParams {
                fix: false,
                stale_claim_hours: 24,
            };
            handle_reconcile(&reconcile_params, &state).await
        }
    });

    // 2. Always fresh fetch — bypasses cache entirely.
    let (issues_vec, edges) = state
        .client
        .fetch_graph_data()
        .await
        .map_err(github_error_to_mcp)?;

    // 2. Build graph and compute ready set.
    let graph = DependencyGraph::build(&issues_vec, &edges);
    let ready_summaries = graph.compute_ready_set(&issues_vec);

    let now = Utc::now();

    // 3. Categorise issues.
    let categories = categorise_issues(
        &issues_vec,
        &graph,
        &ready_summaries,
        stale_threshold_hours,
        now,
    );

    // 4. Update cache with the fresh graph already fetched.
    state.cache.update(ready_summaries, graph).await;
    tracing::debug!("Cache updated with fresh graph from prime");

    // 5. Apply agent filter to relevant categories (PRD §6.3).
    //    `completed` and `hotspots` are NOT filtered — completed provides
    //    global continuity context and hotspots are structural graph properties.
    let mut filtered_ip = categories.in_progress;
    let mut filtered_ready = categories.ready;
    let mut filtered_blocked = categories.blocked;
    let mut filtered_stale = categories.stale;

    // Normalize empty/whitespace agent strings to None so they don't silently
    // filter out all results (serde deserializes "" as Some("")).
    let agent_filter = crate::tools::normalize_filter(params.agent.as_deref());
    if let Some(agent_filter) = agent_filter {
        let matches_agent = |s: &IssueSummary| s.agent.as_deref() == Some(agent_filter);
        filtered_ip.retain(matches_agent);
        filtered_ready.retain(matches_agent);
        filtered_blocked.retain(matches_agent);
        filtered_stale.retain(|s| s.agent.as_deref() == Some(agent_filter));
    }

    // 6. Build result with counts computed AFTER filtering.
    let counts = PrimeCounts {
        in_progress: filtered_ip.len(),
        ready: filtered_ready.len(),
        blocked: filtered_blocked.len(),
        completed: categories.completed.len(),
        hotspots: categories.hotspots.len(),
        stale: filtered_stale.len(),
    };

    Ok(PrimeResult {
        in_progress: filtered_ip
            .iter()
            .take(max_per_category)
            .map(PrimeIssueSummary::from_core)
            .collect(),
        ready: filtered_ready
            .iter()
            .take(max_per_category)
            .map(PrimeIssueSummary::from_core)
            .collect(),
        blocked: filtered_blocked
            .iter()
            .take(max_per_category)
            .map(PrimeIssueSummary::from_core)
            .collect(),
        completed: categories
            .completed
            .into_iter()
            .take(max_per_category)
            .collect(),
        hotspots: categories
            .hotspots
            .into_iter()
            .take(max_per_category)
            .collect(),
        stale: filtered_stale.into_iter().take(max_per_category).collect(),
        session: SessionMeta::from_state(state),
        drift_warnings: resolve_drift_warnings(drift_check).await,
        counts,
    })
}

/// Await the background drift check and convert to `drift_warnings`.
///
/// Returns `None` if the reconcile task panicked, returned an error, or
/// found no drift (`report.clean == true`). Returns `Some(warnings)` when
/// drift is detected, with human-readable summary strings.
async fn resolve_drift_warnings(
    drift_check: tokio::task::JoinHandle<
        Result<crate::tools::reconcile::ReconcileOutput, rmcp::model::ErrorData>,
    >,
) -> Option<Vec<String>> {
    match drift_check.await {
        Ok(Ok(reconcile_out)) if !reconcile_out.report.clean => {
            Some(summarise_drift(&reconcile_out.report))
        }
        _ => None,
    }
}

/// Convert a [`ReconcileReport`] into human-readable drift warning strings.
///
/// Groups drift items by type tag and produces one summary line per drift
/// type, e.g. `"3 stale ready states"`, `"1 uncascaded closure"`.
fn summarise_drift(report: &ReconcileReport) -> Vec<String> {
    // Count occurrences of each drift type from the serialised JSON values.
    // Each drift_found entry is an externally-tagged serde enum: `{"VariantName": {...}}`.
    let mut counts: HashMap<String, usize> = HashMap::new();

    for drift in &report.drift_found {
        if let Some(obj) = drift.as_object()
            && let Some(key) = obj.keys().next()
        {
            *counts.entry(key.clone()).or_insert(0) += 1;
        }
    }

    let mut warnings: Vec<String> = counts
        .into_iter()
        .map(|(kind, count)| {
            let label = match kind.as_str() {
                "StaleReadyState" => "stale ready state",
                "UncascadedClosure" => "uncascaded closure",
                "OrphanedBlockingEdge" => "orphaned blocking edge",
                "MalformedAgentField" => "malformed agent field",
                "MissingProjectField" => "missing project field",
                "CycleDetected" => "cycle detected",
                "StaleClaim" => "stale claim",
                other => other,
            };
            if count == 1 {
                format!("1 {label}")
            } else {
                format!("{count} {label}s")
            }
        })
        .collect();

    // Sort for deterministic output in tests.
    warnings.sort();
    warnings
}

/// Intermediate result holding categorised issue lists.
struct CategorisedIssues {
    in_progress: Vec<IssueSummary>,
    ready: Vec<IssueSummary>,
    blocked: Vec<IssueSummary>,
    completed: Vec<CompletedIssueSummary>,
    hotspots: Vec<HotspotSummary>,
    stale: Vec<StaleIssueSummary>,
}

/// Categorise issues into the prime result categories.
///
/// - `in_progress`: `Status::InProgress` and `IssueState::Open`
/// - `blocked`: open issues that have at least one open blocker in the graph
/// - `ready`: from `compute_ready_set()` (open, unblocked)
/// - `completed`: closed issues with `updated_at` within the stale threshold window
/// - `hotspots`: issues that block the most other issues (descending by count)
/// - `stale`: in-progress issues with `claimed_at` older than the threshold
fn categorise_issues(
    issues: &[Issue],
    graph: &DependencyGraph,
    ready_summaries: &[IssueSummary],
    stale_threshold_hours: u64,
    now: DateTime<Utc>,
) -> CategorisedIssues {
    let mut in_progress = Vec::new();
    let mut blocked = Vec::new();
    let mut completed = Vec::new();
    let mut stale = Vec::new();

    // Build a set of ready QualifiedIds for quick lookup.
    let ready_set: std::collections::HashSet<&QualifiedId> =
        ready_summaries.iter().map(|s| &s.qualified_id).collect();

    // Build issue lookup by QualifiedId.
    let issue_map: HashMap<&QualifiedId, &Issue> =
        issues.iter().map(|i| (&i.qualified_id, i)).collect();

    // The stale threshold doubles as the "recently completed" window.
    let completed_cutoff = now
        - chrono::Duration::hours(i64::from(
            u32::try_from(stale_threshold_hours).unwrap_or(u32::MAX),
        ));

    for issue in issues {
        // Collect recently-closed issues into the completed category.
        if issue.state != IssueState::Open {
            if issue.updated_at >= completed_cutoff {
                completed.push(CompletedIssueSummary {
                    qualified_id: issue.qualified_id.to_string(),
                    number: issue.number,
                    title: issue.title.clone(),
                    issue_type: issue.issue_type.map(|it| format!("{it:?}")),
                    priority: format!("{:?}", issue.priority),
                    closed_at: issue.updated_at.to_rfc3339(),
                    url: issue.url.clone(),
                });
            }
            continue;
        }

        let summary = issue_to_summary(issue);

        if issue.status == Status::InProgress {
            in_progress.push(summary.clone());

            // Check for staleness. Log if claimed_at is missing — may indicate
            // a data quality issue (agent claimed work but no timestamp recorded).
            if issue.claimed_at.is_none() {
                tracing::debug!(
                    number = issue.number,
                    qualified_id = %issue.qualified_id,
                    "InProgress issue has no claimed_at — skipped for stale detection"
                );
            }
            if let Some(claimed_at) = issue.claimed_at {
                let hours_elapsed = (now - claimed_at).num_hours().unsigned_abs();
                if hours_elapsed > stale_threshold_hours {
                    stale.push(StaleIssueSummary {
                        qualified_id: issue.qualified_id.to_string(),
                        number: issue.number,
                        title: issue.title.clone(),
                        agent: issue.agent.clone(),
                        claimed_at: claimed_at.to_rfc3339(),
                        hours_stale: hours_elapsed,
                        url: issue.url.clone(),
                    });
                }
            }
        } else if !ready_set.contains(&issue.qualified_id) {
            // Not in_progress and not ready — check if blocked.
            // Exclude Deferred issues: they were intentionally deferred, not
            // dependency-blocked, so showing them as "blocked" confuses agents.
            if issue.status != Status::Deferred
                && (issue.status == Status::Blocked || has_open_blockers(issue, graph))
            {
                blocked.push(summary);
            }
        }
    }

    // Sort in_progress by priority ASC, then created_at ASC.
    in_progress.sort_by(|a, b| {
        a.priority
            .as_sort_key()
            .cmp(&b.priority.as_sort_key())
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    // Sort blocked by priority ASC, then created_at ASC.
    blocked.sort_by(|a, b| {
        a.priority
            .as_sort_key()
            .cmp(&b.priority.as_sort_key())
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    // Sort completed by closed_at DESC (most recently closed first).
    completed.sort_by(|a, b| b.closed_at.cmp(&a.closed_at));

    // Sort stale by hours_stale DESC (most stale first).
    stale.sort_by(|a, b| b.hours_stale.cmp(&a.hours_stale));

    // Compute hotspots from the graph edges.
    let hotspots = compute_hotspots(graph, &issue_map);

    // Filter InProgress issues out of the ready list — an issue already being
    // worked on should not appear as "ready to pick up".
    let in_progress_ids: std::collections::HashSet<&QualifiedId> =
        in_progress.iter().map(|s| &s.qualified_id).collect();
    let filtered_ready: Vec<IssueSummary> = ready_summaries
        .iter()
        .filter(|s| !in_progress_ids.contains(&s.qualified_id))
        .cloned()
        .collect();

    CategorisedIssues {
        in_progress,
        ready: filtered_ready,
        blocked,
        completed,
        hotspots,
        stale,
    }
}

/// Check if an issue has at least one open blocker in the graph.
///
/// Uses `all_edges()` to find edges where this issue is the `source`
/// (blocked by target), then checks if any target is open.
fn has_open_blockers(issue: &Issue, graph: &DependencyGraph) -> bool {
    let issue_state = graph.issue_state();

    // Look up this issue's node in the graph.
    if let Some(&node_idx) = graph.node_map().get(&issue.qualified_id) {
        // Outgoing edges point to blockers.
        let inner = graph.inner_graph();
        inner
            .neighbors_directed(node_idx, petgraph::Direction::Outgoing)
            .any(|neighbor_idx| {
                let neighbor_qid = &inner[neighbor_idx];
                issue_state
                    .get(neighbor_qid)
                    .is_some_and(|state| *state == IssueState::Open)
            })
    } else {
        false
    }
}

/// Compute hotspots: issues that block the most other issues.
///
/// An issue is a hotspot if it appears as the `target` of blocking edges
/// (other issues depend on it). Returns sorted by `blocking_count` descending.
fn compute_hotspots(
    graph: &DependencyGraph,
    issue_map: &HashMap<&QualifiedId, &Issue>,
) -> Vec<HotspotSummary> {
    // Count how many issues each node blocks (incoming edges = dependents).
    let edges = graph.all_edges();
    let mut blocking_counts: HashMap<QualifiedId, usize> = HashMap::new();

    for edge in &edges {
        // edge.source is blocked by edge.target
        // So edge.target is the blocker — count how many things it blocks.
        *blocking_counts.entry(edge.target.clone()).or_insert(0) += 1;
    }

    let mut hotspots: Vec<HotspotSummary> = blocking_counts
        .into_iter()
        .filter_map(|(qid, count)| {
            // Only include open issues as hotspots.
            let issue = issue_map.get(&qid)?;
            if issue.state != IssueState::Open {
                return None;
            }
            Some(HotspotSummary {
                qualified_id: qid.to_string(),
                number: issue.number,
                title: issue.title.clone(),
                status: format!("{:?}", issue.status),
                priority: format!("{:?}", issue.priority),
                blocking_count: count,
                url: issue.url.clone(),
            })
        })
        .collect();

    // Sort by blocking_count DESC, then number ASC (stable tiebreaker).
    hotspots.sort_by(|a, b| {
        b.blocking_count
            .cmp(&a.blocking_count)
            .then_with(|| a.number.cmp(&b.number))
    });

    hotspots
}

/// Convert an [`Issue`] to an [`IssueSummary`] for categorisation.
fn issue_to_summary(issue: &Issue) -> IssueSummary {
    IssueSummary {
        qualified_id: issue.qualified_id.clone(),
        number: issue.number,
        title: issue.title.clone(),
        issue_type: issue.issue_type,
        status: issue.status,
        priority: issue.priority,
        agent: issue.agent.clone(),
        milestone: issue.milestone.clone(),
        story_points: issue.story_points,
        defer_until: issue.defer_until,
        labels: issue.labels.clone(),
        created_at: issue.created_at,
        url: issue.url.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::similar_names)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::{TimeZone, Utc};
    use unblock_core::cache::GraphCache;
    use unblock_core::config::Config;
    use unblock_core::graph::DependencyGraph;
    use unblock_core::types::{
        BlockingEdge, Issue, IssueState, IssueType, Priority, QualifiedId, ReadyState, Status,
    };

    use super::*;
    use crate::server::ServerState;

    // ── Test helpers ───────────────────────────────────────────────────

    /// Helper to create a `QualifiedId` for tests.
    fn qid(number: u64) -> QualifiedId {
        QualifiedId::new("test-owner", "test-repo", number)
    }

    /// Build a minimal `Issue` for testing.
    fn test_issue(number: u64, state: IssueState, status: Status) -> Issue {
        Issue {
            qualified_id: qid(number),
            number,
            node_id: format!("NODE_{number}"),
            title: format!("Issue #{number}"),
            issue_type: Some(IssueType::Task),
            status,
            priority: Priority::P1,
            agent: None,
            claimed_at: None,
            ready_state: ReadyState::Ready,
            story_points: None,
            defer_until: None,
            labels: vec![],
            milestone: None,
            assignees: vec![],
            state,
            body: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            url: format!("https://github.com/test-owner/test-repo/issues/{number}"),
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    /// Create a `ServerState` for unit tests.
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
            client: Arc::new(client),
            cache: Arc::new(GraphCache::new(Duration::from_secs(300))),
            agent_kind: std::sync::OnceLock::new(),
            agent_client: std::sync::OnceLock::new(),
            connected_at: std::sync::OnceLock::new(),
        }
    }

    // ── Categorisation tests ──────────────────────────────────────────

    #[test]
    fn categorise_empty_issues_returns_empty() {
        let graph = DependencyGraph::build(&[], &[]);
        let ready = graph.compute_ready_set(&[]);
        let result = categorise_issues(&[], &graph, &ready, 24, Utc::now());

        assert!(result.in_progress.is_empty());
        assert!(result.ready.is_empty());
        assert!(result.blocked.is_empty());
        assert!(result.completed.is_empty());
        assert!(result.hotspots.is_empty());
        assert!(result.stale.is_empty());
    }

    #[test]
    fn categorise_in_progress_issues() {
        let mut issue = test_issue(1, IssueState::Open, Status::InProgress);
        issue.agent = Some("agent-x".to_owned());
        issue.claimed_at = Some(Utc::now());
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.in_progress.len(), 1);
        assert_eq!(result.in_progress[0].number, 1);
        assert!(
            result.stale.is_empty(),
            "recently claimed should not be stale"
        );
    }

    #[test]
    fn categorise_stale_claims() {
        let mut issue = test_issue(1, IssueState::Open, Status::InProgress);
        issue.agent = Some("agent-x".to_owned());
        // Claimed 48 hours ago.
        issue.claimed_at = Some(Utc::now() - chrono::Duration::hours(48));
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.in_progress.len(), 1);
        assert_eq!(result.stale.len(), 1);
        assert_eq!(result.stale[0].number, 1);
        assert!(result.stale[0].hours_stale >= 47); // at least 47 hours
    }

    #[test]
    fn categorise_blocked_issues() {
        // Issue #1 blocks issue #2.
        let issue1 = test_issue(1, IssueState::Open, Status::Open);
        let mut issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        issue2.ready_state = ReadyState::Blocked;
        let issues = vec![issue1, issue2];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Issue #1 is ready (no blockers), issue #2 is blocked.
        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].number, 1);
        assert_eq!(result.blocked.len(), 1);
        assert_eq!(result.blocked[0].number, 2);
    }

    #[test]
    fn categorise_hotspots() {
        // Issue #1 blocks issues #2 and #3.
        let issue1 = test_issue(1, IssueState::Open, Status::Open);
        let issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        let issue3 = test_issue(3, IssueState::Open, Status::Blocked);
        let issues = vec![issue1, issue2, issue3];
        let edges = vec![
            BlockingEdge {
                source: qid(2),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(3),
                target: qid(1),
            },
        ];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);
        let issue_map: HashMap<&QualifiedId, &Issue> =
            issues.iter().map(|i| (&i.qualified_id, i)).collect();
        let hotspots = compute_hotspots(&graph, &issue_map);

        assert_eq!(hotspots.len(), 1);
        assert_eq!(hotspots[0].number, 1);
        assert_eq!(hotspots[0].blocking_count, 2);

        // Also verify via full categorise.
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());
        assert_eq!(result.hotspots.len(), 1);
        assert_eq!(result.hotspots[0].blocking_count, 2);
    }

    #[test]
    fn hotspots_excludes_closed_issues() {
        // Issue #1 blocks #2, but #1 is closed.
        let issue1 = test_issue(1, IssueState::Closed, Status::Closed);
        let issue2 = test_issue(2, IssueState::Open, Status::Open);
        let issues = vec![issue1, issue2];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let issue_map: HashMap<&QualifiedId, &Issue> =
            issues.iter().map(|i| (&i.qualified_id, i)).collect();
        let hotspots = compute_hotspots(&graph, &issue_map);

        assert!(
            hotspots.is_empty(),
            "closed issues should not appear as hotspots"
        );
    }

    #[test]
    fn hotspots_sorted_by_blocking_count_desc() {
        // #1 blocks 3 issues, #4 blocks 1 issue.
        let issue1 = test_issue(1, IssueState::Open, Status::Open);
        let issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        let issue3 = test_issue(3, IssueState::Open, Status::Blocked);
        let issue4 = test_issue(4, IssueState::Open, Status::Open);
        let issue5 = test_issue(5, IssueState::Open, Status::Blocked);
        let issues = vec![issue1, issue2, issue3, issue4, issue5];
        let edges = vec![
            BlockingEdge {
                source: qid(2),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(3),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(5),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(5),
                target: qid(4),
            },
        ];

        let graph = DependencyGraph::build(&issues, &edges);
        let issue_map: HashMap<&QualifiedId, &Issue> =
            issues.iter().map(|i| (&i.qualified_id, i)).collect();
        let hotspots = compute_hotspots(&graph, &issue_map);

        assert_eq!(hotspots.len(), 2);
        assert_eq!(hotspots[0].number, 1);
        assert_eq!(hotspots[0].blocking_count, 3);
        assert_eq!(hotspots[1].number, 4);
        assert_eq!(hotspots[1].blocking_count, 1);
    }

    #[test]
    fn closed_issues_excluded_from_open_categories() {
        let issue = test_issue(1, IssueState::Closed, Status::Closed);
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert!(result.in_progress.is_empty());
        assert!(result.ready.is_empty());
        assert!(result.blocked.is_empty());
        assert!(result.stale.is_empty());
        // Recently closed issue should appear in completed (updated_at is "now").
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.completed[0].number, 1);
    }

    #[test]
    fn completed_excludes_old_closed_issues() {
        let mut issue = test_issue(1, IssueState::Closed, Status::Closed);
        // Updated 48 hours ago — outside the default 24h window.
        issue.updated_at = Utc::now() - chrono::Duration::hours(48);
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert!(
            result.completed.is_empty(),
            "issues closed more than 24h ago should not appear in completed"
        );
    }

    #[test]
    fn completed_sorted_by_closed_at_desc() {
        let mut issue1 = test_issue(1, IssueState::Closed, Status::Closed);
        issue1.updated_at = Utc::now() - chrono::Duration::hours(2);

        let mut issue2 = test_issue(2, IssueState::Closed, Status::Closed);
        issue2.updated_at = Utc::now() - chrono::Duration::hours(1);

        let issues = vec![issue1, issue2];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.completed.len(), 2);
        assert_eq!(
            result.completed[0].number, 2,
            "most recently closed should come first"
        );
        assert_eq!(
            result.completed[1].number, 1,
            "older closed should come second"
        );
    }

    #[test]
    fn completed_respects_custom_threshold() {
        let mut issue = test_issue(1, IssueState::Closed, Status::Closed);
        // Updated 30 hours ago — outside 24h but inside 48h.
        issue.updated_at = Utc::now() - chrono::Duration::hours(30);
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);

        // With default 24h window: excluded.
        let result_24 = categorise_issues(&issues, &graph, &ready, 24, Utc::now());
        assert!(
            result_24.completed.is_empty(),
            "30h-old closure should not appear with 24h window"
        );

        // With 48h window: included.
        let result_48 = categorise_issues(&issues, &graph, &ready, 48, Utc::now());
        assert_eq!(
            result_48.completed.len(),
            1,
            "30h-old closure should appear with 48h window"
        );
    }

    #[test]
    fn deferred_issues_excluded_from_blocked() {
        // Issue #1 blocks issue #2 (deferred). Deferred should not appear as blocked.
        let issue1 = test_issue(1, IssueState::Open, Status::Open);
        let mut issue2 = test_issue(2, IssueState::Open, Status::Deferred);
        issue2.ready_state = ReadyState::Blocked;
        let issues = vec![issue1, issue2];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert!(
            result.blocked.is_empty(),
            "deferred issue should not appear in blocked list"
        );
    }

    #[test]
    fn in_progress_excluded_from_ready() {
        // An InProgress issue with no blockers should only appear in in_progress, not ready.
        let mut issue = test_issue(1, IssueState::Open, Status::InProgress);
        issue.agent = Some("agent-x".to_owned());
        issue.claimed_at = Some(Utc::now());
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.in_progress.len(), 1);
        assert!(
            result.ready.is_empty(),
            "InProgress issues should not appear in the ready list"
        );
    }

    #[test]
    fn in_progress_sorted_by_priority_then_created_at() {
        let earlier = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();

        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.priority = Priority::P2;
        issue1.created_at = later;
        issue1.claimed_at = Some(Utc::now());

        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.priority = Priority::P0;
        issue2.created_at = earlier;
        issue2.claimed_at = Some(Utc::now());

        let issues = vec![issue1, issue2];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.in_progress.len(), 2);
        assert_eq!(result.in_progress[0].number, 2, "P0 should come first");
        assert_eq!(result.in_progress[1].number, 1, "P2 should come second");
    }

    #[test]
    fn stale_sorted_by_hours_desc() {
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("a".to_owned());
        issue1.claimed_at = Some(Utc::now() - chrono::Duration::hours(30));

        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("b".to_owned());
        issue2.claimed_at = Some(Utc::now() - chrono::Duration::hours(72));

        let issues = vec![issue1, issue2];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(result.stale.len(), 2);
        assert_eq!(
            result.stale[0].number, 2,
            "most stale (72h) should come first"
        );
        assert_eq!(
            result.stale[1].number, 1,
            "less stale (30h) should come second"
        );
    }

    // ── SessionMeta tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn session_meta_from_state_defaults_when_uninitialised() {
        let state = test_state().await;
        let meta = SessionMeta::from_state(&state);
        assert_eq!(meta.agent_client, "unknown");
        assert_eq!(meta.agent_kind, "unknown");
        // agent_field depends on UNBLOCK_AGENT env var — not asserted here
        // connected_at falls back to Utc::now(), just verify it's recent
        let elapsed = Utc::now() - meta.connected_at;
        assert!(
            elapsed.num_seconds() < 5,
            "connected_at should be recent, got {elapsed}"
        );
    }

    #[tokio::test]
    async fn session_meta_from_state_populated() {
        use unblock_core::client::{AgentClient, AgentKind};

        let state = test_state().await;
        let _ = state.agent_kind.set(AgentKind::ClaudeCode);
        let _ = state.agent_client.set(AgentClient {
            name: "Claude Code".to_owned(),
            version: "1.2.3".to_owned(),
        });
        let connected = Utc::now();
        let _ = state.connected_at.set(connected);

        let meta = SessionMeta::from_state(&state);
        assert_eq!(meta.agent_client, "Claude Code");
        assert_eq!(meta.agent_kind, "claude-code");
        assert_eq!(meta.connected_at, connected);
    }

    /// Helper test invoked by subprocess tests below. Prints the `agent_field`
    /// value from `SessionMeta::from_state` so the parent process can assert it.
    ///
    /// Protocol: prints `AGENT_FIELD=<value>` or `AGENT_FIELD=NONE` to stdout.
    #[ignore = "invoked by subprocess tests, not meant to run directly"]
    #[tokio::test]
    async fn subprocess_helper_print_agent_field() {
        let state = test_state().await;
        let meta = SessionMeta::from_state(&state);
        match meta.agent_field {
            Some(val) => println!("AGENT_FIELD={val}"),
            None => println!("AGENT_FIELD=NONE"),
        }
    }

    /// Spawns a child process *with* `UNBLOCK_AGENT=test-supervisor` set and
    /// asserts that `SessionMeta.agent_field` is `Some("test-supervisor")`.
    #[test]
    fn session_meta_agent_field_set_via_subprocess() {
        let test_bin = std::env::current_exe().expect("should resolve test binary path");
        let output = std::process::Command::new(&test_bin)
            .arg("--exact")
            .arg("tools::prime::tests::subprocess_helper_print_agent_field")
            .arg("--include-ignored")
            .arg("--nocapture")
            .env("UNBLOCK_AGENT", "test-supervisor")
            // Clear detection env vars to avoid side effects on other tests.
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env_remove("GITHUB_COPILOT_TOKEN")
            .env_remove("CURSOR_TRACE_ID")
            .output()
            .expect("failed to spawn subprocess");

        assert!(
            output.status.success(),
            "subprocess exited with non-zero status: {output:?}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("AGENT_FIELD=test-supervisor"),
            "expected AGENT_FIELD=test-supervisor in subprocess output, got:\n{stdout}"
        );
    }

    /// Spawns a child process *without* `UNBLOCK_AGENT` and asserts that
    /// `SessionMeta.agent_field` is `None`.
    #[test]
    fn session_meta_agent_field_unset_via_subprocess() {
        let test_bin = std::env::current_exe().expect("should resolve test binary path");
        let output = std::process::Command::new(&test_bin)
            .arg("--exact")
            .arg("tools::prime::tests::subprocess_helper_print_agent_field")
            .arg("--include-ignored")
            .arg("--nocapture")
            .env_remove("UNBLOCK_AGENT")
            // Clear detection env vars to avoid side effects on other tests.
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .env_remove("GITHUB_COPILOT_TOKEN")
            .env_remove("CURSOR_TRACE_ID")
            .output()
            .expect("failed to spawn subprocess");

        assert!(
            output.status.success(),
            "subprocess exited with non-zero status: {output:?}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("AGENT_FIELD=NONE"),
            "expected AGENT_FIELD=NONE in subprocess output, got:\n{stdout}"
        );
    }

    // ── PrimeParams deserialization tests ──────────────────────────────

    #[test]
    fn prime_params_defaults() {
        let json = r"{}";
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert!(params.stale_threshold_hours.is_none());
        assert!(params.max_per_category.is_none());
    }

    #[test]
    fn prime_params_zero_stale_threshold_deserializes() {
        let json = r#"{"stale_threshold_hours": 0}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(params.stale_threshold_hours, Some(0));
    }

    #[test]
    fn prime_params_zero_max_per_category_deserializes() {
        let json = r#"{"max_per_category": 0}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(params.max_per_category, Some(0));
    }

    #[tokio::test]
    async fn handle_prime_rejects_zero_stale_threshold() {
        let state = Arc::new(test_state().await);
        let params = PrimeParams {
            stale_threshold_hours: Some(0),
            max_per_category: None,
            agent: None,
        };

        let err = handle_prime(&params, &state)
            .await
            .expect_err("stale_threshold_hours=0 should be rejected");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("stale_threshold_hours"),
            "error should mention the parameter name: {}",
            err.message,
        );
    }

    #[tokio::test]
    async fn handle_prime_rejects_zero_max_per_category() {
        let state = Arc::new(test_state().await);
        let params = PrimeParams {
            stale_threshold_hours: None,
            max_per_category: Some(0),
            agent: None,
        };

        let err = handle_prime(&params, &state)
            .await
            .expect_err("max_per_category=0 should be rejected");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("max_per_category"),
            "error should mention the parameter name: {}",
            err.message,
        );
    }

    #[test]
    fn prime_params_explicit_values() {
        let json = r#"{"stale_threshold_hours": 48, "max_per_category": 5}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(params.stale_threshold_hours, Some(48));
        assert_eq!(params.max_per_category, Some(5));
    }

    // ── PrimeResult serialization tests ───────────────────────────────

    #[test]
    fn prime_result_serializes_clean() {
        let connected = Utc::now();
        let result = PrimeResult {
            in_progress: vec![],
            ready: vec![],
            blocked: vec![],
            completed: vec![],
            hotspots: vec![],
            stale: vec![],
            session: SessionMeta {
                agent_client: "unknown".to_owned(),
                agent_kind: "unknown".to_owned(),
                agent_field: None,
                connected_at: connected,
            },
            drift_warnings: None,
            counts: PrimeCounts {
                in_progress: 0,
                ready: 0,
                blocked: 0,
                completed: 0,
                hotspots: 0,
                stale: 0,
            },
        };

        let json = serde_json::to_value(&result).expect("should serialize");
        assert!(json["drift_warnings"].is_null());
        assert_eq!(json["session"]["agent_kind"], "unknown");
        assert_eq!(json["session"]["agent_client"], "unknown");
        assert!(json["session"]["agent_field"].is_null());
        assert!(json["session"]["connected_at"].is_string());
        assert_eq!(json["counts"]["in_progress"], 0);
    }

    #[test]
    fn prime_result_session_all_fields_present_in_json() {
        let connected = Utc::now();
        let result = PrimeResult {
            in_progress: vec![],
            ready: vec![],
            blocked: vec![],
            completed: vec![],
            hotspots: vec![],
            stale: vec![],
            session: SessionMeta {
                agent_client: "Claude Code".to_owned(),
                agent_kind: "claude-code".to_owned(),
                agent_field: Some("rust-supervisor".to_owned()),
                connected_at: connected,
            },
            drift_warnings: None,
            counts: PrimeCounts {
                in_progress: 0,
                ready: 0,
                blocked: 0,
                completed: 0,
                hotspots: 0,
                stale: 0,
            },
        };

        let json = serde_json::to_value(&result).expect("should serialize");
        let session = &json["session"];
        assert_eq!(session["agent_client"], "Claude Code");
        assert_eq!(session["agent_kind"], "claude-code");
        assert_eq!(session["agent_field"], "rust-supervisor");
        // connected_at should be a valid RFC 3339 string
        let ts_str = session["connected_at"].as_str().expect("should be string");
        let parsed = DateTime::parse_from_rfc3339(ts_str);
        assert!(
            parsed.is_ok(),
            "connected_at should be valid RFC 3339: {ts_str}"
        );
    }

    // ── Integration test: full categorise pipeline ────────────────────

    #[test]
    fn integration_mixed_issues_categorised_correctly() {
        // Setup: 4 issues in different states.
        // #1: open, ready (no blockers)
        // #2: open, blocked by #1
        // #3: in_progress (claimed 48h ago — stale)
        // #4: closed
        let issue1 = test_issue(1, IssueState::Open, Status::Open);
        let mut issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        issue2.ready_state = ReadyState::Blocked;
        let mut issue3 = test_issue(3, IssueState::Open, Status::InProgress);
        issue3.agent = Some("agent-z".to_owned());
        issue3.claimed_at = Some(Utc::now() - chrono::Duration::hours(48));
        let issue4 = test_issue(4, IssueState::Closed, Status::Closed);

        let issues = vec![issue1, issue2, issue3, issue4];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // #1 is ready (#3 is InProgress so excluded from ready list).
        assert_eq!(
            result.ready.len(),
            1,
            "ready should include only #1 (InProgress #3 is excluded)"
        );
        assert_eq!(result.ready[0].number, 1);
        // #2 is blocked.
        assert_eq!(result.blocked.len(), 1);
        assert_eq!(result.blocked[0].number, 2);
        // #3 is in_progress + stale.
        assert_eq!(result.in_progress.len(), 1);
        assert_eq!(result.in_progress[0].number, 3);
        assert_eq!(result.stale.len(), 1);
        assert_eq!(result.stale[0].number, 3);
        // #4 is closed recently — should appear in completed.
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.completed[0].number, 4);
        // #1 is a hotspot (blocks #2).
        assert_eq!(result.hotspots.len(), 1);
        assert_eq!(result.hotspots[0].number, 1);
        assert_eq!(result.hotspots[0].blocking_count, 1);
    }

    // ── Cache update integration test ─────────────────────────────────

    #[tokio::test]
    async fn cache_updated_after_prime() {
        let state = test_state().await;
        assert!(
            !state.cache.is_fresh().await,
            "cache should be empty initially"
        );

        // Manually update cache (simulating what handle_prime does after fetch).
        let issues = vec![
            test_issue(1, IssueState::Open, Status::Open),
            test_issue(2, IssueState::Open, Status::Open),
        ];
        let graph = DependencyGraph::build(&issues, &[]);
        let ready_set = graph.compute_ready_set(&issues);
        state.cache.update(ready_set, graph).await;

        assert!(
            state.cache.is_fresh().await,
            "cache should be fresh after update"
        );
        let cached_ready = state.cache.get_ready_set().await;
        assert!(cached_ready.is_some());
        assert_eq!(cached_ready.unwrap().len(), 2);
    }

    // ── Max per category truncation test ──────────────────────────────

    #[test]
    fn max_per_category_truncates_results() {
        let mut issues = Vec::new();
        for i in 1..=20 {
            issues.push(test_issue(i, IssueState::Open, Status::Open));
        }

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        assert_eq!(
            result.ready.len(),
            20,
            "all 20 should be ready before truncation"
        );

        // Simulate truncation as handle_prime does it.
        let max = 5;
        let truncated: Vec<_> = result
            .ready
            .iter()
            .take(max)
            .map(PrimeIssueSummary::from_core)
            .collect();
        assert_eq!(truncated.len(), 5);
    }

    // ── Agent filter tests ───────────────────────────────────────────

    /// Helper: apply agent filter the same way `handle_prime` does, returning
    /// filtered category lengths for assertions.
    fn apply_agent_filter(
        categories: &CategorisedIssues,
        agent: Option<&str>,
    ) -> (usize, usize, usize, usize) {
        let count_summary = |items: &[IssueSummary]| -> usize {
            items
                .iter()
                .filter(|s| agent.is_none_or(|a| s.agent.as_deref() == Some(a)))
                .count()
        };
        let count_stale = categories
            .stale
            .iter()
            .filter(|s| agent.is_none_or(|a| s.agent.as_deref() == Some(a)))
            .count();
        (
            count_summary(&categories.in_progress),
            count_summary(&categories.ready),
            count_summary(&categories.blocked),
            count_stale,
        )
    }

    #[test]
    fn agent_filter_none_returns_all() {
        // Two in-progress issues with different agents.
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now());
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now());
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let (ip, _r, _b, _s) = apply_agent_filter(&result, None);
        assert_eq!(ip, 2, "None agent should return all in_progress issues");
    }

    #[test]
    fn agent_filter_matches_in_progress() {
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now());
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now());
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let (ip, _, _, _) = apply_agent_filter(&result, Some("agent-x"));
        assert_eq!(ip, 1, "should filter in_progress to agent-x only");
    }

    #[test]
    fn agent_filter_matches_ready() {
        let mut issue1 = test_issue(1, IssueState::Open, Status::Open);
        issue1.agent = Some("agent-x".to_owned());
        let mut issue2 = test_issue(2, IssueState::Open, Status::Open);
        issue2.agent = Some("agent-y".to_owned());
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let (_, r, _, _) = apply_agent_filter(&result, Some("agent-x"));
        assert_eq!(r, 1, "should filter ready to agent-x only");
    }

    #[test]
    fn agent_filter_matches_blocked() {
        // #1 blocks #2 and #3. Agents assigned to the blocked issues.
        let issue1 = test_issue(1, IssueState::Open, Status::Open);
        let mut issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        issue2.agent = Some("agent-x".to_owned());
        issue2.ready_state = ReadyState::Blocked;
        let mut issue3 = test_issue(3, IssueState::Open, Status::Blocked);
        issue3.agent = Some("agent-y".to_owned());
        issue3.ready_state = ReadyState::Blocked;
        let issues = vec![issue1, issue2, issue3];
        let edges = vec![
            BlockingEdge {
                source: qid(2),
                target: qid(1),
            },
            BlockingEdge {
                source: qid(3),
                target: qid(1),
            },
        ];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let (_, _, b, _) = apply_agent_filter(&result, Some("agent-x"));
        assert_eq!(b, 1, "should filter blocked to agent-x only");
    }

    #[test]
    fn agent_filter_matches_stale() {
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now() - chrono::Duration::hours(48));
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now() - chrono::Duration::hours(48));
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        let (_, _, _, s) = apply_agent_filter(&result, Some("agent-x"));
        assert_eq!(s, 1, "should filter stale to agent-x only");
    }

    #[test]
    fn agent_filter_does_not_affect_completed() {
        // Completed issues should not be filtered by agent.
        let mut issue = test_issue(1, IssueState::Closed, Status::Closed);
        issue.agent = Some("agent-x".to_owned());
        let issues = vec![issue];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Completed has no agent field — it should always appear regardless
        // of what agent filter we would apply.
        assert_eq!(
            result.completed.len(),
            1,
            "completed should not be filtered by agent"
        );
    }

    #[test]
    fn agent_filter_does_not_affect_hotspots() {
        // Hotspots are structural — should not be filtered by agent.
        let mut issue1 = test_issue(1, IssueState::Open, Status::Open);
        issue1.agent = Some("agent-x".to_owned());
        let mut issue2 = test_issue(2, IssueState::Open, Status::Blocked);
        issue2.agent = Some("agent-y".to_owned());
        issue2.ready_state = ReadyState::Blocked;
        let issues = vec![issue1, issue2];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];

        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Hotspot #1 is agent-x, but even filtering for agent-y should not
        // remove hotspots (they are not filtered).
        assert_eq!(
            result.hotspots.len(),
            1,
            "hotspots should not be filtered by agent"
        );
    }

    #[test]
    fn agent_filter_counts_reflect_filtered_totals() {
        // Two in_progress issues, two ready, filter to one agent.
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now());
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now());
        let mut issue3 = test_issue(3, IssueState::Open, Status::Open);
        issue3.agent = Some("agent-x".to_owned());
        let mut issue4 = test_issue(4, IssueState::Open, Status::Open);
        issue4.agent = Some("agent-y".to_owned());
        let issues = vec![issue1, issue2, issue3, issue4];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let categories = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Simulate the filtering handle_prime does.
        let agent_filter = Some("agent-x".to_owned());
        let mut in_progress = categories.in_progress;
        let mut ready_list = categories.ready;
        if let Some(ref f) = agent_filter {
            let matches_agent = |s: &IssueSummary| s.agent.as_deref() == Some(f.as_str());
            in_progress.retain(matches_agent);
            ready_list.retain(matches_agent);
        }

        assert_eq!(in_progress.len(), 1, "filtered in_progress count");
        assert_eq!(ready_list.len(), 1, "filtered ready count");
    }

    #[test]
    fn prime_params_agent_deserializes() {
        let json = r#"{"agent": "agent-x"}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(params.agent.as_deref(), Some("agent-x"));
    }

    #[test]
    fn prime_params_agent_defaults_to_none() {
        let json = r"{}";
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert!(params.agent.is_none());
    }

    #[test]
    fn prime_params_empty_agent_deserializes_as_some_empty() {
        // Demonstrates the serde behavior this fix addresses: `""` becomes
        // `Some("")`, not `None`. The normalize_filter call in handle_prime
        // collapses this to None before filtering.
        let json = r#"{"agent": ""}"#;
        let params: PrimeParams = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(
            params.agent.as_deref(),
            Some(""),
            "serde should deserialize empty string as Some(\"\")"
        );
    }

    #[test]
    fn empty_agent_filter_returns_all_categories() {
        // Regression test: empty string agent filter should behave as no filter.
        let mut issue1 = test_issue(1, IssueState::Open, Status::InProgress);
        issue1.agent = Some("agent-x".to_owned());
        issue1.claimed_at = Some(Utc::now());
        let mut issue2 = test_issue(2, IssueState::Open, Status::InProgress);
        issue2.agent = Some("agent-y".to_owned());
        issue2.claimed_at = Some(Utc::now());
        let issues = vec![issue1, issue2];

        let graph = DependencyGraph::build(&issues, &[]);
        let ready = graph.compute_ready_set(&issues);
        let result = categorise_issues(&issues, &graph, &ready, 24, Utc::now());

        // Simulate what handle_prime does: normalize then filter.
        let agent_filter = crate::tools::normalize_filter(Some(""));
        assert!(
            agent_filter.is_none(),
            "empty string should normalize to None"
        );

        let (ip, _, _, _) = apply_agent_filter(&result, agent_filter);
        assert_eq!(
            ip, 2,
            "empty agent string should return all in_progress issues"
        );
    }

    // ── summarise_drift tests ────────────────────────────────────────

    /// Build a [`ReconcileReport`] with the given `drift_found` values.
    fn make_report(drift_found: Vec<serde_json::Value>) -> ReconcileReport {
        ReconcileReport {
            repo: "test-owner/test-repo".to_owned(),
            reconciled_at: Utc::now().to_rfc3339(),
            issues_scanned: 10,
            edges_scanned: 5,
            clean: drift_found.is_empty(),
            drift_found,
            repaired: vec![],
            errors: vec![],
            message: None,
        }
    }

    #[test]
    fn summarise_drift_empty_report_returns_empty() {
        let report = make_report(vec![]);
        let warnings = summarise_drift(&report);
        assert!(warnings.is_empty());
    }

    #[test]
    fn summarise_drift_single_stale_ready_state() {
        let drift = serde_json::json!({
            "StaleReadyState": {
                "issue": "owner/repo#1",
                "field_says": "Ready",
                "graph_says": "Blocked"
            }
        });
        let report = make_report(vec![drift]);
        let warnings = summarise_drift(&report);
        assert_eq!(warnings, vec!["1 stale ready state"]);
    }

    #[test]
    fn summarise_drift_multiple_of_same_type_uses_plural() {
        let drift1 = serde_json::json!({
            "StaleReadyState": { "issue": "o/r#1", "field_says": "Ready", "graph_says": "Blocked" }
        });
        let drift2 = serde_json::json!({
            "StaleReadyState": { "issue": "o/r#2", "field_says": "Ready", "graph_says": "Blocked" }
        });
        let drift3 = serde_json::json!({
            "StaleReadyState": { "issue": "o/r#3", "field_says": "Blocked", "graph_says": "Ready" }
        });
        let report = make_report(vec![drift1, drift2, drift3]);
        let warnings = summarise_drift(&report);
        assert_eq!(warnings, vec!["3 stale ready states"]);
    }

    #[test]
    fn summarise_drift_mixed_types_sorted() {
        let stale_rs = serde_json::json!({
            "StaleReadyState": { "issue": "o/r#1", "field_says": "Ready", "graph_says": "Blocked" }
        });
        let uncascaded = serde_json::json!({
            "UncascadedClosure": { "closed_issue": "o/r#2", "should_have_unblocked": ["o/r#3"] }
        });
        let stale_claim = serde_json::json!({
            "StaleClaim": { "issue": "o/r#4", "claimed_at": "2026-01-01T00:00:00Z", "hours_stale": 48 }
        });
        let report = make_report(vec![stale_rs, uncascaded, stale_claim]);
        let warnings = summarise_drift(&report);
        // Sorted alphabetically.
        assert_eq!(
            warnings,
            vec![
                "1 stale claim",
                "1 stale ready state",
                "1 uncascaded closure",
            ]
        );
    }

    #[test]
    fn summarise_drift_all_seven_types() {
        let drifts = vec![
            serde_json::json!({"StaleReadyState": {}}),
            serde_json::json!({"UncascadedClosure": {}}),
            serde_json::json!({"OrphanedBlockingEdge": {}}),
            serde_json::json!({"MalformedAgentField": {}}),
            serde_json::json!({"MissingProjectField": {}}),
            serde_json::json!({"CycleDetected": {}}),
            serde_json::json!({"StaleClaim": {}}),
        ];
        let report = make_report(drifts);
        let warnings = summarise_drift(&report);
        assert_eq!(warnings.len(), 7);
        // All present with count 1.
        for w in &warnings {
            assert!(w.starts_with("1 "), "each should start with '1 ': {w}");
        }
    }

    // ── resolve_drift_warnings tests ─────────────────────────────────

    #[tokio::test]
    async fn resolve_drift_warnings_clean_report_returns_none() {
        let handle = tokio::spawn(async {
            Ok(crate::tools::reconcile::ReconcileOutput {
                report: make_report(vec![]),
            })
        });
        let result = resolve_drift_warnings(handle).await;
        assert!(result.is_none(), "clean report should produce None");
    }

    #[tokio::test]
    async fn resolve_drift_warnings_with_drift_returns_some() {
        let drift = serde_json::json!({
            "StaleReadyState": { "issue": "o/r#1", "field_says": "Ready", "graph_says": "Blocked" }
        });
        let handle = tokio::spawn(async {
            Ok(crate::tools::reconcile::ReconcileOutput {
                report: ReconcileReport {
                    repo: "o/r".to_owned(),
                    reconciled_at: Utc::now().to_rfc3339(),
                    issues_scanned: 5,
                    edges_scanned: 2,
                    clean: false,
                    drift_found: vec![drift],
                    repaired: vec![],
                    errors: vec![],
                    message: None,
                },
            })
        });
        let result = resolve_drift_warnings(handle).await;
        assert!(result.is_some(), "dirty report should produce Some");
        let warnings = result.unwrap();
        assert_eq!(warnings, vec!["1 stale ready state"]);
    }

    #[tokio::test]
    async fn resolve_drift_warnings_error_returns_none() {
        let handle = tokio::spawn(async {
            Err(rmcp::model::ErrorData {
                code: rmcp::model::ErrorCode::INTERNAL_ERROR,
                message: "boom".into(),
                data: None,
            })
        });
        let result = resolve_drift_warnings(handle).await;
        assert!(result.is_none(), "error should produce None");
    }

    #[tokio::test]
    async fn resolve_drift_warnings_panic_returns_none() {
        let handle: tokio::task::JoinHandle<
            Result<crate::tools::reconcile::ReconcileOutput, rmcp::model::ErrorData>,
        > = tokio::spawn(async { panic!("simulated reconcile panic") });
        let result = resolve_drift_warnings(handle).await;
        assert!(result.is_none(), "panic should produce None");
    }
}
