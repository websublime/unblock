//! List tool — filtered, sorted, paginated view over the open issue set.
//!
//! Returns issues matching the requested filters in the requested sort
//! order, with offset/limit pagination. Per spec §7.5 this is a read tool
//! over the **full open issue set** (not just the ready set), so unlike
//! [`ready`](crate::tools::ready) the cache is not directly consulted —
//! the cache stores only the ready subset and lacks the per-issue
//! `updated_at` and `assignees` fields the list contract advertises.
//!
//! Each invocation issues exactly one `fetch_graph_data()` call against
//! GitHub. The fresh graph and ready set are written to the cache as a
//! useful side-effect so that subsequent ready/show/list calls observe a
//! consistent snapshot. The cache is "read-only" from the spec's
//! perspective in the sense that callers cannot observe a difference
//! between a list call that hit the cache and one that missed it — the
//! data is reconstructable from a single GitHub fetch (spec §4.5).
//!
//! ## OPEN-only scope
//!
//! `fetch_graph_data()` only returns OPEN GitHub issues today, so a
//! `list(status="Closed")` call always yields `total = 0`. The
//! [`ListParams::status`] doc-comment and the tool description document
//! this gap. Extending the GraphQL query to fetch CLOSED issues is
//! tracked by a follow-up bead (R1 decision recorded on bead
//! unblock-29p.6).

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::model::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use unblock_core::errors::ValidationSnafu;
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{Issue, Priority, QualifiedId, Status};

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;

/// Default number of issues returned when `limit` is not specified.
const DEFAULT_LIMIT: usize = 50;

/// Maximum permitted value for `limit` (spec §7.5).
const MAX_LIMIT: usize = 200;

/// Canonical sort identifier for "priority" (default ordering).
const SORT_PRIORITY: &str = "priority";

/// Canonical sort identifier for "created" (`created_at` ASC).
const SORT_CREATED: &str = "created";

/// Canonical sort identifier for "updated" (`updated_at` DESC).
const SORT_UPDATED: &str = "updated";

/// Input parameters for the `list` MCP tool.
///
/// All parameters are optional. With no parameters set, returns the first
/// 50 open issues sorted by priority (`P0` first), then `created_at`
/// ascending, then `qualified_id` ascending as a deterministic
/// tiebreaker.
///
/// String filters that are empty or whitespace-only are treated as
/// absent — `{"agent": ""}` behaves identically to `{"agent": null}`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListParams {
    /// Filter by workflow status (case-insensitive). Accepts `"Ready"`,
    /// `"InProgress"`, `"Blocked"`, `"Deferred"`, or `"Closed"`.
    ///
    /// **Open-only scope:** the underlying graph fetch returns only OPEN
    /// GitHub issues, so `status="Closed"` always yields `total = 0`
    /// today.
    pub status: Option<String>,
    /// Filter by priority level (case-insensitive). Accepts `"P0"`,
    /// `"P1"`, `"P2"`, `"P3"`, or `"P4"`.
    pub priority: Option<String>,
    /// Filter by issue type (case-insensitive). Accepts `"Task"`,
    /// `"Bug"`, `"Feature"`, `"Epic"`, `"Chore"`, or `"Spike"`.
    pub issue_type: Option<String>,
    /// Filter by milestone title. Exact match.
    pub milestone: Option<String>,
    /// Filter by agent name. Exact match.
    pub agent: Option<String>,
    /// Filter by label. Returns issues that have this label (any match).
    /// Case-insensitive.
    pub label: Option<String>,
    /// Filter by assignee. Returns issues that have this GitHub login as
    /// an assignee (any match). Exact match.
    pub assignee: Option<String>,
    /// Sort order. One of `"priority"` (default), `"created"`, or
    /// `"updated"`. Empty/whitespace-only strings normalise to the
    /// default. Any other value is rejected with `INVALID_PARAMS`.
    ///
    /// - `"priority"`: priority ascending (`P0` first), then `created_at`
    ///   ascending.
    /// - `"created"`: `created_at` ascending.
    /// - `"updated"`: `updated_at` descending.
    ///
    /// Every sort is finalised with a deterministic `qualified_id`
    /// tiebreaker so pagination is stable across calls.
    pub sort: Option<String>,
    /// Maximum number of issues to return (after sorting). Must be in
    /// `1..=200` if present; defaults to 50.
    pub limit: Option<usize>,
    /// Number of leading sorted issues to skip before applying `limit`.
    /// Defaults to 0. An offset past `total` returns an empty
    /// `issues` array with `total > 0` (Postgres-style; not an error).
    pub offset: Option<usize>,
}

/// Result returned by the `list` MCP tool.
///
/// `issues` holds the sorted, paginated slice; `total` is the count of
/// matches **before** pagination so clients can compute page counts.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListResult {
    /// Issues matching all filter criteria, sorted as requested,
    /// truncated to `[offset, offset + limit)`.
    pub issues: Vec<ListIssueSummary>,
    /// Total number of matches across all pages — the length of the
    /// filtered+sorted set before pagination.
    pub total: usize,
    /// `true` if the GitHub fetch failed and the cache had no data to
    /// fall back on. `issues` will be empty and `total` will be 0 in
    /// that case.
    pub stale: bool,
}

/// Lightweight issue summary for the list result.
///
/// Re-declared here (rather than re-exporting
/// [`unblock_core::types::IssueSummary`]) so it can derive `JsonSchema`
/// without coupling the core crate to `schemars` and so it can carry
/// `updated_at` and `assignees` (which are absent from `IssueSummary`)
/// without expanding the core type. Sourced directly from
/// [`unblock_core::types::Issue`] via the internal `build_list_rows`
/// projection.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListIssueSummary {
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Issue type classification (e.g. `"Task"`, `"Bug"`).
    pub issue_type: Option<String>,
    /// Workflow status from Projects V2 (e.g. `"Ready"`, `"InProgress"`).
    pub status: String,
    /// Priority level from Projects V2 (`"P0"`..`"P4"`).
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
    /// GitHub usernames of human assignees on the issue.
    ///
    /// Present on this list-specific summary (and not on the core
    /// `IssueSummary`) so the `assignee` filter has a meaningful field
    /// to match against and so consumers can render the assignment
    /// without a follow-up `show` call.
    pub assignees: Vec<String>,
    /// Timestamp when the issue was created (ISO 8601 / RFC 3339).
    pub created_at: String,
    /// Timestamp when the issue was last updated (ISO 8601 / RFC 3339).
    ///
    /// Present here (and not on `IssueSummary`) so the `sort=updated`
    /// branch has data to render and so consumers can interpret the
    /// resulting order.
    pub updated_at: String,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

/// Internal projection over an [`Issue`] holding only the fields needed
/// for filtering and sorting in [`filter_sort_paginate`].
///
/// Built once per call to avoid repeating allocations and `.to_string()`
/// calls inside the filter and sort closures. `qualified_id` is carried
/// through so the sort tiebreaker is deterministic without needing to
/// recompute the `Display` form of each id.
#[derive(Debug, Clone)]
struct ListRow {
    qualified_id: QualifiedId,
    number: u64,
    title: String,
    issue_type: Option<String>,
    status: Status,
    priority: Priority,
    agent: Option<String>,
    milestone: Option<String>,
    story_points: Option<i32>,
    defer_until: Option<NaiveDate>,
    labels: Vec<String>,
    assignees: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    url: String,
}

impl ListRow {
    /// Project an [`Issue`] down to the minimal data needed for filter,
    /// sort, and the eventual [`ListIssueSummary`].
    fn from_issue(issue: &Issue) -> Self {
        Self {
            qualified_id: issue.qualified_id.clone(),
            number: issue.number,
            title: issue.title.clone(),
            issue_type: issue.issue_type.map(|it| it.to_string()),
            status: issue.status,
            priority: issue.priority,
            agent: issue.agent.clone(),
            milestone: issue.milestone.clone(),
            story_points: issue.story_points,
            defer_until: issue.defer_until,
            labels: issue.labels.clone(),
            assignees: issue.assignees.clone(),
            created_at: issue.created_at,
            updated_at: issue.updated_at,
            url: issue.url.clone(),
        }
    }

    /// Convert the row into the schemars-friendly wire type.
    fn into_summary(self) -> ListIssueSummary {
        ListIssueSummary {
            number: self.number,
            title: self.title,
            issue_type: self.issue_type,
            status: self.status.to_string(),
            priority: self.priority.to_string(),
            agent: self.agent,
            milestone: self.milestone,
            story_points: self.story_points,
            defer_until: self.defer_until.map(|d| d.to_string()),
            labels: self.labels,
            assignees: self.assignees,
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
            url: self.url,
        }
    }
}

/// Build the list-row projection from a slice of issues.
///
/// Kept private because the only callers are the [`handle_list`] entry
/// point and the in-module unit tests. Integration tests drive the
/// public [`handle_list`] surface via [`crate::server::ServerState`]
/// and a [`MockGitHubClient`](unblock_github::mock::MockGitHubClient).
#[must_use]
fn build_list_rows(issues: &[Issue]) -> Vec<ListRow> {
    issues.iter().map(ListRow::from_issue).collect()
}

/// Apply filter, sort, and pagination to a pre-projected row set.
///
/// Returns the page of summaries to send back **and** the pre-pagination
/// `total` so the caller can populate [`ListResult::total`] without
/// re-counting.
///
/// Filters are AND-combined: an issue must match every supplied filter
/// to appear in the output. Empty/whitespace filter strings have already
/// been collapsed to `None` by [`handle_list`] before this helper runs.
///
/// Sort order is fully deterministic: the requested ordering key is
/// followed by a final `qualified_id` tiebreaker so paginating clients
/// see a stable sequence across calls even when timestamps collide.
fn filter_sort_paginate(
    rows: &[ListRow],
    params: &ListParams,
    sort: SortKey,
    offset: usize,
    limit: usize,
) -> (Vec<ListIssueSummary>, usize) {
    let status_filter = crate::tools::normalize_filter(params.status.as_deref());
    let priority_filter = crate::tools::normalize_filter(params.priority.as_deref());
    let issue_type_filter = crate::tools::normalize_filter(params.issue_type.as_deref());
    let milestone_filter = crate::tools::normalize_filter(params.milestone.as_deref());
    let agent_filter = crate::tools::normalize_filter(params.agent.as_deref());
    let label_filter = crate::tools::normalize_filter(params.label.as_deref());
    let assignee_filter = crate::tools::normalize_filter(params.assignee.as_deref());

    // Collect references first so the sort runs over &ListRow without
    // cloning each row twice (once for filter, once for sort).
    let mut filtered: Vec<&ListRow> = rows
        .iter()
        // status: case-insensitive Display match (e.g. "ready"=="Ready").
        .filter(|r| status_filter.is_none_or(|f| r.status.to_string().eq_ignore_ascii_case(f)))
        // priority: case-insensitive Display match (e.g. "p0"=="P0").
        .filter(|r| priority_filter.is_none_or(|f| r.priority.to_string().eq_ignore_ascii_case(f)))
        // issue_type: case-insensitive Display match.
        .filter(|r| {
            issue_type_filter.is_none_or(|f| {
                r.issue_type
                    .as_deref()
                    .is_some_and(|it| it.eq_ignore_ascii_case(f))
            })
        })
        // milestone: exact match (matching ready.rs:149).
        .filter(|r| milestone_filter.is_none_or(|f| r.milestone.as_deref() == Some(f)))
        // agent: exact match (matching ready.rs:151).
        .filter(|r| agent_filter.is_none_or(|f| r.agent.as_deref() == Some(f)))
        // label: any label case-insensitive match (matching ready.rs:153).
        .filter(|r| label_filter.is_none_or(|f| r.labels.iter().any(|l| l.eq_ignore_ascii_case(f))))
        // assignee: any assignee exact match — GitHub stores logins as
        // canonical strings, and exact match mirrors the agent filter.
        .filter(|r| assignee_filter.is_none_or(|f| r.assignees.iter().any(|a| a == f)))
        .collect();

    sort.apply(&mut filtered);

    let total = filtered.len();

    let issues = filtered
        .into_iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .map(ListRow::into_summary)
        .collect();

    (issues, total)
}

/// Resolved sort key with the comparator the call selects between.
///
/// Parsed once in [`handle_list`] so the comparator decision happens
/// outside the hot filter path. The `apply` method finishes every sort
/// with a deterministic `qualified_id` tiebreaker so pagination is
/// stable across calls (Sherlock R7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    /// Priority ascending, then `created_at` ascending.
    Priority,
    /// `created_at` ascending.
    Created,
    /// `updated_at` descending.
    Updated,
}

impl SortKey {
    /// Parse a normalised sort string. `None` (or an empty/whitespace
    /// string already collapsed by the caller) selects [`Self::Priority`].
    /// Any other value is rejected with a validation error.
    ///
    /// # Errors
    ///
    /// Returns the `Err` arm of a [`Result`] carrying an `ErrorData`
    /// with code `INVALID_PARAMS` when `raw` is not one of the
    /// canonical sort identifiers.
    fn parse(raw: Option<&str>) -> Result<Self, ErrorData> {
        match raw {
            None | Some(SORT_PRIORITY) => Ok(Self::Priority),
            Some(SORT_CREATED) => Ok(Self::Created),
            Some(SORT_UPDATED) => Ok(Self::Updated),
            Some(other) => Err(validation_error(format!(
                "sort must be one of \"{SORT_PRIORITY}\", \"{SORT_CREATED}\", or \
                 \"{SORT_UPDATED}\" — got \"{other}\""
            ))),
        }
    }

    /// Sort `rows` in place using the resolved comparator and a
    /// deterministic `qualified_id` tiebreaker.
    fn apply(self, rows: &mut Vec<&ListRow>) {
        match self {
            Self::Priority => {
                rows.sort_by(|a, b| {
                    a.priority
                        .as_sort_key()
                        .cmp(&b.priority.as_sort_key())
                        .then_with(|| a.created_at.cmp(&b.created_at))
                        .then_with(|| qid_cmp(&a.qualified_id, &b.qualified_id))
                });
            }
            Self::Created => {
                rows.sort_by(|a, b| {
                    a.created_at
                        .cmp(&b.created_at)
                        .then_with(|| qid_cmp(&a.qualified_id, &b.qualified_id))
                });
            }
            Self::Updated => {
                rows.sort_by(|a, b| {
                    b.updated_at
                        .cmp(&a.updated_at)
                        .then_with(|| qid_cmp(&a.qualified_id, &b.qualified_id))
                });
            }
        }
    }
}

/// Lexicographic ordering on `(owner, repo, number)` so the tiebreaker
/// is consistent with the [`std::fmt::Display`] form
/// (`owner/repo#number`) without going through string formatting on
/// every comparison.
fn qid_cmp(a: &QualifiedId, b: &QualifiedId) -> std::cmp::Ordering {
    a.owner
        .cmp(&b.owner)
        .then_with(|| a.repo.cmp(&b.repo))
        .then_with(|| a.number.cmp(&b.number))
}

/// Validate the runtime constraints `JsonSchema` cannot enforce
/// (`limit in 1..=200`).
fn validate_limit(limit: Option<usize>) -> Result<(), ErrorData> {
    match limit {
        None => Ok(()),
        Some(value) if (1..=MAX_LIMIT).contains(&value) => Ok(()),
        Some(value) => Err(validation_error(format!(
            "limit must be in 1..={MAX_LIMIT} — got {value}"
        ))),
    }
}

/// Build a domain `Validation` error and lift it through the GitHub
/// error mapping so the resulting [`ErrorData`] mirrors the rest of the
/// MCP surface (HTTP 400 → `INVALID_PARAMS`).
fn validation_error(message: impl Into<String>) -> ErrorData {
    let domain = ValidationSnafu {
        message: message.into(),
    }
    .build();
    let github = unblock_github::errors::Error::from(domain);
    github_error_to_mcp(github)
}

/// Execute the `list` tool handler.
///
/// # Flow
///
/// 1. Validate `limit` and parse `sort` — returns `INVALID_PARAMS` on
///    bad input.
/// 2. Always issue a fresh `fetch_graph_data()` so the result is
///    derived from the current open issue set (the cache holds only
///    the ready subset and lacks `updated_at`/`assignees`).
/// 3. On a successful fetch, rebuild the dependency graph + ready set
///    and refresh the cache with the new snapshot — a useful side
///    effect that keeps subsequent ready/show calls warm without
///    changing what `list` itself observes.
/// 4. Project the issues to the internal `ListRow`, then run
///    `filter_sort_paginate` to produce the page of
///    [`ListIssueSummary`] plus the pre-pagination `total`.
/// 5. On a fetch failure, fall back to an empty result with `stale =
///    true` (mirrors the ready handler at server.rs:847-852).
///
/// # Errors
///
/// Returns [`ErrorData`] with code `INVALID_PARAMS` when validation
/// fails. Network/transport errors do **not** propagate — they are
/// surfaced via `stale = true` so callers receive a structured response
/// instead of a tool error.
#[instrument(skip_all, name = "handle_list")]
pub async fn handle_list(state: &ServerState, params: ListParams) -> Result<ListResult, ErrorData> {
    let kind = state.agent_kind_str();
    info!(
        agent.kind = %kind,
        status = params.status.as_deref(),
        priority = params.priority.as_deref(),
        issue_type = params.issue_type.as_deref(),
        milestone = params.milestone.as_deref(),
        agent = params.agent.as_deref(),
        label = params.label.as_deref(),
        assignee = params.assignee.as_deref(),
        sort = params.sort.as_deref(),
        limit = params.limit,
        offset = params.offset,
        "List tool invoked"
    );

    // Step 1: validate `limit` and resolve `sort` before any network
    // call so we fail fast on bad inputs.
    validate_limit(params.limit)?;
    let sort_raw = crate::tools::normalize_filter(params.sort.as_deref());
    let sort = SortKey::parse(sort_raw)?;
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    let offset = params.offset.unwrap_or(0);

    // Step 2: always fresh fetch — the cache only carries the ready
    // subset and lacks updated_at/assignees, so a list response cannot
    // be derived from cached state alone.
    let fetch_result = state.github.fetch_graph_data().await;

    let issues = match fetch_result {
        Ok((issues, edges)) => {
            // Step 3: refresh the cache as a useful side effect. The
            // operation is read-only from the contract perspective —
            // callers cannot tell from the response whether the cache
            // was warm or cold.
            let graph = DependencyGraph::build(&issues, &edges);
            let ready_set = graph.compute_ready_set(&issues);
            state.cache.update(ready_set, graph).await;
            tracing::debug!("Cache refreshed by list handler");
            issues
        }
        Err(err) => {
            // Step 5: degrade gracefully — stale=true with empty issues,
            // mirroring the ready handler.
            tracing::warn!(
                error = %err,
                "fetch_graph_data failed during list — returning stale=true"
            );
            return Ok(ListResult {
                issues: Vec::new(),
                total: 0,
                stale: true,
            });
        }
    };

    // Step 4: project, filter, sort, paginate.
    let rows = build_list_rows(&issues);
    let (issues_out, total) = filter_sort_paginate(&rows, &params, sort, offset, limit);

    Ok(ListResult {
        issues: issues_out,
        total,
        stale: false,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use rmcp::model::ErrorCode;
    use unblock_core::types::{Issue, IssueState, IssueType, Priority, QualifiedId, Status};

    use super::*;

    // ── Test fixtures ────────────────────────────────────────────────

    /// Build a minimal `Issue` for tests. Defaults: open, Ready, P2,
    /// no labels, no assignees, no milestone, fixed timestamps.
    fn test_issue(number: u64) -> Issue {
        let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        Issue {
            qualified_id: QualifiedId::new("test", "repo", number),
            number,
            node_id: format!("NODE_{number}"),
            title: format!("Issue #{number}"),
            issue_type: Some(IssueType::Task),
            status: Status::Ready,
            priority: Priority::P2,
            agent: None,
            claimed_at: None,
            pipeline_stage: None,
            story_points: None,
            defer_until: None,
            labels: vec![],
            milestone: None,
            assignees: vec![],
            state: IssueState::Open,
            body: None,
            created_at: ts,
            updated_at: ts,
            url: format!("https://github.com/test/repo/issues/{number}"),
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    fn default_params() -> ListParams {
        ListParams {
            status: None,
            priority: None,
            issue_type: None,
            milestone: None,
            agent: None,
            label: None,
            assignee: None,
            sort: None,
            limit: None,
            offset: None,
        }
    }

    /// Helper: drive the full filter+sort+paginate pipeline through
    /// the same projection `handle_list` uses, with explicit sort and
    /// pagination inputs.
    fn run(
        issues: &[Issue],
        params: &ListParams,
        sort: SortKey,
        offset: usize,
        limit: usize,
    ) -> (Vec<ListIssueSummary>, usize) {
        let rows = build_list_rows(issues);
        filter_sort_paginate(&rows, params, sort, offset, limit)
    }

    // ── Empty input ──────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty_and_zero_total() {
        let (out, total) = run(&[], &default_params(), SortKey::Priority, 0, DEFAULT_LIMIT);
        assert!(out.is_empty());
        assert_eq!(total, 0);
    }

    // ── No-filter pass-through ───────────────────────────────────────

    #[test]
    fn returns_all_with_no_filters() {
        let issues = vec![test_issue(1), test_issue(2), test_issue(3)];
        let (out, total) = run(&issues, &default_params(), SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 3);
        assert_eq!(total, 3);
    }

    // ── Status filter ────────────────────────────────────────────────

    #[test]
    fn filter_by_status_case_insensitive() {
        let mut i1 = test_issue(1);
        i1.status = Status::InProgress;
        let mut i2 = test_issue(2);
        i2.status = Status::Ready;
        let issues = vec![i1, i2];
        let params = ListParams {
            status: Some("inprogress".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn filter_by_status_no_match_returns_empty() {
        let issues = vec![test_issue(1)]; // Ready by default
        let params = ListParams {
            status: Some("Closed".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 0);
        assert_eq!(total, 0);
    }

    // ── Priority filter ──────────────────────────────────────────────

    #[test]
    fn filter_by_priority_case_insensitive() {
        let mut i1 = test_issue(1);
        i1.priority = Priority::P0;
        let mut i2 = test_issue(2);
        i2.priority = Priority::P3;
        let issues = vec![i1, i2];
        let params = ListParams {
            priority: Some("p0".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, 1);
        assert_eq!(total, 1);
    }

    // ── Issue type filter ────────────────────────────────────────────

    #[test]
    fn filter_by_issue_type_case_insensitive() {
        let mut i1 = test_issue(1);
        i1.issue_type = Some(IssueType::Bug);
        let mut i2 = test_issue(2);
        i2.issue_type = Some(IssueType::Task);
        let issues = vec![i1, i2];
        let params = ListParams {
            issue_type: Some("bug".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, 1);
        assert_eq!(total, 1);
    }

    // ── Milestone filter ─────────────────────────────────────────────

    #[test]
    fn filter_by_milestone_exact_match() {
        let mut i1 = test_issue(1);
        i1.milestone = Some("v1.0".to_owned());
        let mut i2 = test_issue(2);
        i2.milestone = Some("v2.0".to_owned());
        let issues = vec![i1, i2];
        let params = ListParams {
            milestone: Some("v1.0".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, 1);
        assert_eq!(total, 1);
    }

    // ── Agent filter ─────────────────────────────────────────────────

    #[test]
    fn filter_by_agent_exact_match() {
        let mut i1 = test_issue(1);
        i1.agent = Some("agent-x".to_owned());
        let i2 = test_issue(2);
        let issues = vec![i1, i2];
        let params = ListParams {
            agent: Some("agent-x".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, 1);
        assert_eq!(total, 1);
    }

    // ── Label filter ─────────────────────────────────────────────────

    #[test]
    fn filter_by_label_case_insensitive() {
        let mut i1 = test_issue(1);
        i1.labels = vec!["Urgent".to_owned(), "backend".to_owned()];
        let mut i2 = test_issue(2);
        i2.labels = vec!["frontend".to_owned()];
        let issues = vec![i1, i2];
        let params = ListParams {
            label: Some("urgent".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, 1);
        assert_eq!(total, 1);
    }

    // ── Assignee filter ──────────────────────────────────────────────

    #[test]
    fn filter_by_assignee_exact_match() {
        let mut i1 = test_issue(1);
        i1.assignees = vec!["alice".to_owned(), "bob".to_owned()];
        let mut i2 = test_issue(2);
        i2.assignees = vec!["carol".to_owned()];
        let issues = vec![i1, i2];
        let params = ListParams {
            assignee: Some("bob".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn assignee_filter_is_case_sensitive() {
        let mut i1 = test_issue(1);
        i1.assignees = vec!["Alice".to_owned()];
        let issues = vec![i1];
        let params = ListParams {
            assignee: Some("alice".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 0);
        assert_eq!(total, 0);
    }

    // ── AND-combined filters ─────────────────────────────────────────

    #[test]
    fn all_filters_combined_use_and_logic() {
        let mut i1 = test_issue(1);
        i1.status = Status::Ready;
        i1.priority = Priority::P0;
        i1.issue_type = Some(IssueType::Bug);
        i1.milestone = Some("v1.0".to_owned());
        i1.agent = Some("agent-x".to_owned());
        i1.labels = vec!["urgent".to_owned()];
        i1.assignees = vec!["alice".to_owned()];

        let mut i2 = test_issue(2);
        // Same status/priority/type/milestone/agent/label as i1 but
        // different assignee — should be filtered out.
        i2.status = Status::Ready;
        i2.priority = Priority::P0;
        i2.issue_type = Some(IssueType::Bug);
        i2.milestone = Some("v1.0".to_owned());
        i2.agent = Some("agent-x".to_owned());
        i2.labels = vec!["urgent".to_owned()];
        i2.assignees = vec!["bob".to_owned()];

        let issues = vec![i1, i2];
        let params = ListParams {
            status: Some("Ready".to_owned()),
            priority: Some("P0".to_owned()),
            issue_type: Some("Bug".to_owned()),
            milestone: Some("v1.0".to_owned()),
            agent: Some("agent-x".to_owned()),
            label: Some("urgent".to_owned()),
            assignee: Some("alice".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].number, 1);
        assert_eq!(total, 1);
    }

    // ── Sort: priority ───────────────────────────────────────────────

    #[test]
    fn sort_priority_p0_before_p1_then_created_asc() {
        let earlier = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let mut i1 = test_issue(1);
        i1.priority = Priority::P1;
        i1.created_at = later;
        let mut i2 = test_issue(2);
        i2.priority = Priority::P0;
        i2.created_at = later;
        let mut i3 = test_issue(3);
        i3.priority = Priority::P0;
        i3.created_at = earlier;
        let issues = vec![i1, i2, i3];

        let (out, _) = run(&issues, &default_params(), SortKey::Priority, 0, 50);
        // P0 earliest, P0 later, then P1.
        assert_eq!(out[0].number, 3);
        assert_eq!(out[1].number, 2);
        assert_eq!(out[2].number, 1);
    }

    // ── Sort: created ────────────────────────────────────────────────

    #[test]
    fn sort_created_ascending() {
        let early = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let late = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let mut i1 = test_issue(1);
        i1.created_at = late;
        let mut i2 = test_issue(2);
        i2.created_at = early;
        let issues = vec![i1, i2];

        let (out, _) = run(&issues, &default_params(), SortKey::Created, 0, 50);
        assert_eq!(out[0].number, 2);
        assert_eq!(out[1].number, 1);
    }

    // ── Sort: updated ────────────────────────────────────────────────

    #[test]
    fn sort_updated_descending() {
        let early = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let late = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let mut i1 = test_issue(1);
        i1.updated_at = early;
        let mut i2 = test_issue(2);
        i2.updated_at = late;
        let issues = vec![i1, i2];

        let (out, _) = run(&issues, &default_params(), SortKey::Updated, 0, 50);
        assert_eq!(out[0].number, 2);
        assert_eq!(out[1].number, 1);
    }

    // ── Sort tiebreaker: qualified_id ─────────────────────────────────

    #[test]
    fn sort_priority_tiebreaks_on_qualified_id_when_timestamps_match() {
        let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut i1 = test_issue(2);
        i1.priority = Priority::P0;
        i1.created_at = ts;
        let mut i2 = test_issue(1);
        i2.priority = Priority::P0;
        i2.created_at = ts;
        // Input ordered i1 (number=2) before i2 (number=1) so a stable
        // sort would keep that order — but the qualified_id tiebreaker
        // must produce 1, 2.
        let issues = vec![i1, i2];

        let (out, _) = run(&issues, &default_params(), SortKey::Priority, 0, 50);
        assert_eq!(out[0].number, 1);
        assert_eq!(out[1].number, 2);
    }

    #[test]
    fn sort_created_tiebreaks_on_qualified_id() {
        let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut i1 = test_issue(2);
        i1.created_at = ts;
        let mut i2 = test_issue(1);
        i2.created_at = ts;
        let issues = vec![i1, i2];

        let (out, _) = run(&issues, &default_params(), SortKey::Created, 0, 50);
        assert_eq!(out[0].number, 1);
        assert_eq!(out[1].number, 2);
    }

    #[test]
    fn sort_updated_tiebreaks_on_qualified_id() {
        let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut i1 = test_issue(2);
        i1.updated_at = ts;
        let mut i2 = test_issue(1);
        i2.updated_at = ts;
        let issues = vec![i1, i2];

        let (out, _) = run(&issues, &default_params(), SortKey::Updated, 0, 50);
        assert_eq!(out[0].number, 1);
        assert_eq!(out[1].number, 2);
    }

    // ── Pagination ───────────────────────────────────────────────────

    #[test]
    fn pagination_offset_zero_returns_first_page() {
        let issues: Vec<_> = (1..=10).map(test_issue).collect();
        let (out, total) = run(&issues, &default_params(), SortKey::Priority, 0, 3);
        assert_eq!(out.len(), 3);
        assert_eq!(total, 10);
        assert_eq!(out[0].number, 1);
        assert_eq!(out[1].number, 2);
        assert_eq!(out[2].number, 3);
    }

    #[test]
    fn pagination_offset_in_middle_returns_correct_page() {
        let issues: Vec<_> = (1..=10).map(test_issue).collect();
        let (out, total) = run(&issues, &default_params(), SortKey::Priority, 5, 3);
        assert_eq!(out.len(), 3);
        assert_eq!(total, 10);
        assert_eq!(out[0].number, 6);
    }

    #[test]
    fn pagination_offset_past_total_returns_empty_with_total_intact() {
        let issues: Vec<_> = (1..=5).map(test_issue).collect();
        let (out, total) = run(&issues, &default_params(), SortKey::Priority, 100, 50);
        assert!(out.is_empty());
        assert_eq!(total, 5, "total counts the filter set, not the page");
    }

    #[test]
    fn pagination_limit_smaller_than_total_truncates_page() {
        let issues: Vec<_> = (1..=20).map(test_issue).collect();
        let (out, total) = run(&issues, &default_params(), SortKey::Priority, 0, 5);
        assert_eq!(out.len(), 5);
        assert_eq!(total, 20);
    }

    #[test]
    fn pagination_limit_larger_than_total_returns_all() {
        let issues: Vec<_> = (1..=3).map(test_issue).collect();
        let (out, total) = run(&issues, &default_params(), SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 3);
        assert_eq!(total, 3);
    }

    // ── Empty-string filter normalisation ────────────────────────────

    #[test]
    fn empty_status_string_is_no_filter() {
        let issues = vec![test_issue(1), test_issue(2)];
        let params = ListParams {
            status: Some(String::new()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 2);
        assert_eq!(total, 2);
    }

    #[test]
    fn empty_priority_string_is_no_filter() {
        let issues = vec![test_issue(1)];
        let params = ListParams {
            priority: Some(String::new()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn empty_issue_type_string_is_no_filter() {
        let issues = vec![test_issue(1)];
        let params = ListParams {
            issue_type: Some(String::new()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn empty_milestone_string_is_no_filter() {
        let issues = vec![test_issue(1)];
        let params = ListParams {
            milestone: Some(String::new()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn empty_agent_string_is_no_filter() {
        let issues = vec![test_issue(1)];
        let params = ListParams {
            agent: Some(String::new()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn empty_label_string_is_no_filter() {
        let mut i1 = test_issue(1);
        i1.labels = vec!["backend".to_owned()];
        let issues = vec![i1];
        let params = ListParams {
            label: Some(String::new()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn empty_assignee_string_is_no_filter() {
        let mut i1 = test_issue(1);
        i1.assignees = vec!["alice".to_owned()];
        let issues = vec![i1];
        let params = ListParams {
            assignee: Some(String::new()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn whitespace_filter_string_is_no_filter() {
        let issues = vec![test_issue(1)];
        let params = ListParams {
            status: Some("   ".to_owned()),
            ..default_params()
        };
        let (out, total) = run(&issues, &params, SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(total, 1);
    }

    // ── Sort key parsing ─────────────────────────────────────────────

    #[test]
    fn sort_parse_default_is_priority() {
        assert_eq!(SortKey::parse(None).unwrap(), SortKey::Priority);
    }

    #[test]
    fn sort_parse_canonical_values_match() {
        assert_eq!(SortKey::parse(Some("priority")).unwrap(), SortKey::Priority);
        assert_eq!(SortKey::parse(Some("created")).unwrap(), SortKey::Created);
        assert_eq!(SortKey::parse(Some("updated")).unwrap(), SortKey::Updated);
    }

    #[test]
    fn sort_parse_unknown_value_errors_with_invalid_params() {
        let err = SortKey::parse(Some("foo")).unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("foo"),
            "error message should reference the bad value: {}",
            err.message
        );
    }

    #[test]
    fn sort_parse_case_sensitive_uppercase_rejected() {
        // Spec says canonical values are lowercase. "Priority" is rejected.
        let err = SortKey::parse(Some("Priority")).unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    }

    // ── Limit validation ─────────────────────────────────────────────

    #[test]
    fn limit_validation_none_is_ok() {
        assert!(validate_limit(None).is_ok());
    }

    #[test]
    fn limit_validation_one_is_ok() {
        assert!(validate_limit(Some(1)).is_ok());
    }

    #[test]
    fn limit_validation_max_is_ok() {
        assert!(validate_limit(Some(MAX_LIMIT)).is_ok());
    }

    #[test]
    fn limit_validation_zero_errors() {
        let err = validate_limit(Some(0)).unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("limit") && err.message.contains("200"),
            "error message should mention the limit bound: {}",
            err.message
        );
    }

    #[test]
    fn limit_validation_above_max_errors() {
        let err = validate_limit(Some(MAX_LIMIT + 1)).unwrap_err();
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    }

    // ── Summary projection contract ──────────────────────────────────

    #[test]
    fn summary_carries_assignees_and_updated_at() {
        let mut i1 = test_issue(1);
        i1.assignees = vec!["alice".to_owned(), "bob".to_owned()];
        i1.updated_at = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
        let issues = vec![i1];
        let (out, _) = run(&issues, &default_params(), SortKey::Priority, 0, 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].assignees, vec!["alice".to_owned(), "bob".to_owned()]);
        assert!(
            out[0].updated_at.starts_with("2026-07-15"),
            "updated_at should be ISO-8601, got {}",
            out[0].updated_at
        );
    }
}
