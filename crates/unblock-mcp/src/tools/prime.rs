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

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{Issue, IssueState, IssueSummary, QualifiedId, Status};

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;

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

/// Session metadata stub.
///
/// Will be populated by Epic 1.5 (Agent Client Detection) with real client
/// information. Until then, reports `Unknown`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SessionMeta {
    /// The kind of AI agent connected (e.g., "claude-code", "copilot", "unknown").
    pub agent_kind: String,
    /// Timestamp when the session started (ISO 8601 / RFC 3339).
    pub connected_at: String,
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self {
            agent_kind: "unknown".to_owned(),
            connected_at: Utc::now().to_rfc3339(),
        }
    }
}

/// Execute the prime tool handler.
///
/// # Flow
///
/// 1. Fresh fetch via `fetch_graph_data()` — bypasses cache entirely.
/// 2. Rebuild `DependencyGraph` from scratch.
/// 3. Categorise all issues into `in_progress`, `blocked`, `ready`, `completed`, `hotspots`, `stale`.
/// 4. Update cache with the fresh graph already fetched.
/// 5. Return `PrimeResult` with stub session and no drift warnings.
///
/// # Errors
///
/// Returns [`rmcp::model::ErrorData`] with `INVALID_PARAMS` if `stale_threshold_hours`
/// or `max_per_category` is below its minimum (currently 1), or if the GitHub fetch fails.
pub async fn handle_prime(
    params: &PrimeParams,
    state: &ServerState,
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

    // 1. Always fresh fetch — bypasses cache entirely.
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

    // 5. Build result with truncation.
    let counts = PrimeCounts {
        in_progress: categories.in_progress.len(),
        ready: categories.ready.len(),
        blocked: categories.blocked.len(),
        completed: categories.completed.len(),
        hotspots: categories.hotspots.len(),
        stale: categories.stale.len(),
    };

    Ok(PrimeResult {
        in_progress: categories
            .in_progress
            .iter()
            .take(max_per_category)
            .map(PrimeIssueSummary::from_core)
            .collect(),
        ready: categories
            .ready
            .iter()
            .take(max_per_category)
            .map(PrimeIssueSummary::from_core)
            .collect(),
        blocked: categories
            .blocked
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
        stale: categories
            .stale
            .into_iter()
            .take(max_per_category)
            .collect(),
        session: SessionMeta::default(),
        drift_warnings: None,
        counts,
    })
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

    #[test]
    fn session_meta_default_is_unknown() {
        let meta = SessionMeta::default();
        assert_eq!(meta.agent_kind, "unknown");
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
        let state = test_state().await;
        let params = PrimeParams {
            stale_threshold_hours: Some(0),
            max_per_category: None,
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
        let state = test_state().await;
        let params = PrimeParams {
            stale_threshold_hours: None,
            max_per_category: Some(0),
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
        let result = PrimeResult {
            in_progress: vec![],
            ready: vec![],
            blocked: vec![],
            completed: vec![],
            hotspots: vec![],
            stale: vec![],
            session: SessionMeta::default(),
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
        assert_eq!(json["counts"]["in_progress"], 0);
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
}
