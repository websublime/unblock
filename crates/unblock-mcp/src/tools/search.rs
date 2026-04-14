//! Search tool — full-text issue search via the GitHub REST Search API.
//!
//! Per spec §7.6 this tool forwards the caller-supplied query string to
//! the GitHub Search API, scoped to the configured `owner/repo`. It is
//! a read-only tool that returns a lightweight projection of each
//! matching issue.
//!
//! ## Cache bypass (spec §7.6, §9.1 invariant 10)
//!
//! Unlike [`ready`](crate::tools::ready) (cache-aware) and
//! [`list`](crate::tools::list) (refreshes the cache as a side effect),
//! `search` **bypasses the cache entirely** on every invocation:
//!
//! - No cache read — each call issues exactly one GitHub HTTP request.
//! - No cache write — the cache is not refreshed as a side effect.
//!
//! This is a spec-level invariant: search must always observe the
//! authoritative state at GitHub, independent of any cached graph
//! snapshot. The [`SearchResult::stale`] field therefore surfaces as
//! `false` for every successful response (the data is freshly fetched)
//! and network failures surface as `ErrorData` rather than a degraded
//! `stale = true` envelope.
//!
//! ## Projects V2 defaults (unblock-29p.5 decision)
//!
//! The GitHub REST `/search/issues` endpoint does not return Projects V2
//! custom fields. The trait implementation populates each returned
//! [`IssueSummary`] with `Status::Ready`, `Priority::P2`, and `None` for
//! `agent`, `story_points`, `defer_until`, `issue_type`. Consumers that
//! need authoritative Projects V2 values should follow up with a
//! [`show`](crate::tools::show) call — the search result is a lookup
//! pointer, not a full issue snapshot.
//!
//! ## Local-repo scope (spec §5.6)
//!
//! Search is local-only. The `repo:{owner}/{repo}` qualifier is
//! composed inside
//! [`GitHubApi::search_issues`](unblock_github::GitHubApi::search_issues),
//! so this tool forwards the user-supplied query unchanged — it does
//! not pre-pend any repo filter, and it does not accept an owner/repo
//! parameter.
//!
//! ## Validation
//!
//! - `query` — must be present and non-empty **after trimming**. A
//!   whitespace-only query is rejected with `INVALID_PARAMS`.
//! - `limit` — optional. `Some(0)` is rejected with `INVALID_PARAMS`
//!   (cross-ref unblock-29p.19 for the 0-clamp discussion). `None`
//!   defaults to 20 inside the transport layer; values above GitHub's
//!   per-page maximum are clamped to 100 by the transport layer.
//!
//! ## Logging
//!
//! The raw query string is never emitted to the tracing subscriber —
//! it may carry project-sensitive terms. The handler records
//! `query_len`, `limit`, and `agent.kind` only (see
//! [`handle_search`]).

use rmcp::model::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use unblock_core::errors::ValidationSnafu;
use unblock_core::types::IssueSummary;

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;

/// Input parameters for the `search` MCP tool.
///
/// The spec (§7.6) requires a non-empty query string and an optional
/// `limit` (default 20). Empty or whitespace-only queries are rejected
/// before any GitHub call is made.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Free-text search query forwarded to GitHub's Search API. Must
    /// be present and non-empty after trimming. Supports GitHub's
    /// native search qualifiers (e.g. `label:bug`, `author:octocat`)
    /// in addition to raw keywords; the `repo:{owner}/{repo} is:issue`
    /// scope is prepended automatically by the transport layer.
    pub query: String,
    /// Maximum number of matches to return. Defaults to 20 when
    /// omitted. A value of `0` is rejected with `INVALID_PARAMS`. The
    /// transport layer clamps the upper bound to GitHub's per-page
    /// maximum of 100.
    pub limit: Option<u32>,
}

/// Result returned by the `search` MCP tool.
///
/// `count` mirrors `issues.len()` for clients that need the match
/// count without traversing the `issues` array. `stale` is included
/// for envelope-level uniformity with the other read tools; because
/// the search handler bypasses the cache entirely, it is always
/// `false` on a successful response — the data is freshly fetched on
/// every call. A network failure surfaces as an `ErrorData` rather
/// than a degraded `stale = true` envelope.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SearchResult {
    /// Issues matching the search query, in the order returned by
    /// GitHub's relevance ranking.
    pub issues: Vec<SearchIssueSummary>,
    /// Number of issues returned (same as `issues.len()`).
    pub count: usize,
    /// Always `false` on a successful search. Included for
    /// consistency with the other read-tool envelopes — search
    /// bypasses the cache, so every successful call observes freshly
    /// fetched data.
    pub stale: bool,
}

/// Lightweight issue summary for the search result.
///
/// Mirrors [`IssueSummary`] with a `JsonSchema` derive (core types do
/// not depend on `schemars`). Fields are normalised to strings where
/// the wire format benefits from it (`status`, `priority`,
/// `issue_type`, timestamps, dates). The Projects V2 fields
/// (`status`, `priority`, `agent`, `story_points`, `defer_until`,
/// `issue_type`) default to their `Default`-equivalent values for
/// search responses — see the module-level docs for the lookup-pointer
/// semantics.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SearchIssueSummary {
    /// GitHub issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Issue type classification (e.g. `"Task"`, `"Bug"`). `None`
    /// from the search tool — Projects V2 fields are not returned by
    /// `/search/issues`.
    pub issue_type: Option<String>,
    /// Workflow status from Projects V2. Defaults to `"Ready"` on
    /// search responses.
    pub status: String,
    /// Priority level from Projects V2. Defaults to `"P2"` on search
    /// responses.
    pub priority: String,
    /// Agent name if claimed. `None` from the search tool.
    pub agent: Option<String>,
    /// Milestone title, if the issue is attached to a milestone.
    pub milestone: Option<String>,
    /// Story points estimate. `None` from the search tool.
    pub story_points: Option<i32>,
    /// Date until which the issue is deferred (ISO 8601 date).
    /// `None` from the search tool.
    pub defer_until: Option<String>,
    /// Labels attached to the issue.
    pub labels: Vec<String>,
    /// Timestamp when the issue was created (RFC 3339).
    pub created_at: String,
    /// HTML URL for linking back to GitHub.
    pub url: String,
}

impl SearchIssueSummary {
    /// Convert from a core [`IssueSummary`] to a schema-annotated MCP
    /// result type.
    #[must_use]
    pub fn from_core(summary: &IssueSummary) -> Self {
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

/// Validate the query string: reject empty or whitespace-only input
/// with `INVALID_PARAMS`, returning the trimmed query on success.
///
/// Trimming before the emptiness check mirrors the
/// [`normalize_filter`](crate::tools::normalize_filter) convention
/// applied elsewhere in the tool surface — `{"query": "   "}` behaves
/// identically to `{"query": ""}`.
fn validate_query(raw: &str) -> Result<&str, ErrorData> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(validation_error("query must not be empty"));
    }
    Ok(trimmed)
}

/// Validate the `limit` parameter: reject `Some(0)` with
/// `INVALID_PARAMS`. `None` and any `Some(n)` with `n >= 1` are
/// accepted; the transport layer clamps the upper bound to 100.
fn validate_limit(limit: Option<u32>) -> Result<(), ErrorData> {
    match limit {
        Some(0) => Err(validation_error(
            "limit must be at least 1 when provided — omit to use the default (20)",
        )),
        _ => Ok(()),
    }
}

/// Execute the `search` tool handler.
///
/// # Flow
///
/// 1. Validate `query` (non-empty after trim) and `limit` (reject
///    `Some(0)`) — returns `INVALID_PARAMS` on bad input without
///    issuing any GitHub call.
/// 2. Forward the trimmed query and raw `limit` to
///    [`GitHubApi::search_issues`](unblock_github::GitHubApi::search_issues).
///    The trait implementation prepends `repo:{owner}/{repo} is:issue`
///    and clamps `limit` to GitHub's per-page maximum (100) before
///    issuing exactly one REST call against `/search/issues`.
/// 3. Map each [`IssueSummary`] to a schema-annotated
///    [`SearchIssueSummary`] and wrap in a [`SearchResult`] with
///    `stale = false` and `count = issues.len()`.
///
/// # Cache invariant
///
/// The handler never reads from or writes to `state.cache`. Search is
/// the only read tool today that bypasses the cache entirely (spec
/// §7.6, §9.1 invariant 10).
///
/// # Errors
///
/// Returns [`ErrorData`] with code `INVALID_PARAMS` for validation
/// failures (empty/whitespace query, `limit = Some(0)`). Network or
/// upstream errors propagate via the crate-internal
/// `github_error_to_mcp` helper and surface as `INTERNAL_ERROR` or
/// `INVALID_PARAMS` depending on the HTTP status.
#[instrument(
    skip(state, params),
    name = "handle_search",
    fields(
        agent.kind = state.agent_kind_str(),
        query_len = params.query.len(),
        limit = ?params.limit,
    ),
)]
pub async fn handle_search(
    state: &ServerState,
    params: SearchParams,
) -> Result<SearchResult, ErrorData> {
    info!("Search tool invoked");

    // Step 1: validate BEFORE any network call so we fail fast on bad
    // input. Empty/whitespace query and limit = Some(0) are rejected
    // with INVALID_PARAMS.
    validate_limit(params.limit)?;
    let trimmed = validate_query(&params.query)?;

    // Step 2: forward to the trait implementation. This issues exactly
    // one REST call; the cache is intentionally untouched.
    let summaries = state
        .github
        .search_issues(trimmed, params.limit)
        .await
        .map_err(github_error_to_mcp)?;

    // Step 3: project to the schema-annotated wire type.
    let issues: Vec<SearchIssueSummary> = summaries
        .iter()
        .map(SearchIssueSummary::from_core)
        .collect();
    let count = issues.len();

    Ok(SearchResult {
        issues,
        count,
        stale: false,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, Utc};
    use rmcp::model::ErrorCode;
    use unblock_core::types::{IssueSummary, IssueType, Priority, QualifiedId, Status};

    use super::*;

    // ── Test helpers ────────────────────────────────────────────────

    fn sample_summary(number: u64) -> IssueSummary {
        IssueSummary {
            qualified_id: QualifiedId::new("acme", "widgets", number),
            number,
            title: format!("Search fixture #{number}"),
            issue_type: Some(IssueType::Task),
            status: Status::Ready,
            priority: Priority::P2,
            agent: None,
            milestone: None,
            story_points: None,
            defer_until: None,
            labels: vec![],
            created_at: Utc::now(),
            url: format!("https://github.com/acme/widgets/issues/{number}"),
        }
    }

    // ── validate_query tests ────────────────────────────────────────

    #[test]
    fn validate_query_rejects_empty() {
        let err = validate_query("").expect_err("empty query must fail");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn validate_query_rejects_whitespace_only() {
        let err = validate_query("   \t\n").expect_err("whitespace-only query must fail");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn validate_query_trims_leading_and_trailing_whitespace() {
        let trimmed = validate_query("  ship  ").expect("padded query should succeed");
        assert_eq!(trimmed, "ship");
    }

    #[test]
    fn validate_query_preserves_internal_whitespace() {
        let trimmed = validate_query("foo bar baz").expect("internal whitespace is preserved");
        assert_eq!(trimmed, "foo bar baz");
    }

    // ── validate_limit tests ────────────────────────────────────────

    #[test]
    fn validate_limit_accepts_none() {
        validate_limit(None).expect("None limit should be accepted");
    }

    #[test]
    fn validate_limit_accepts_positive() {
        validate_limit(Some(1)).expect("limit = 1 should be accepted");
        validate_limit(Some(50)).expect("limit = 50 should be accepted");
        validate_limit(Some(1_000)).expect("limit = 1000 should be accepted (clamped upstream)");
    }

    #[test]
    fn validate_limit_rejects_zero() {
        let err = validate_limit(Some(0)).expect_err("limit = 0 must fail");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("limit"),
            "validation message should explain the bound: {}",
            err.message,
        );
    }

    // ── SearchIssueSummary::from_core tests ─────────────────────────

    #[test]
    fn from_core_maps_defaults_for_projects_v2_fields() {
        let s = sample_summary(42);
        let wire = SearchIssueSummary::from_core(&s);
        assert_eq!(wire.number, 42);
        assert_eq!(wire.title, "Search fixture #42");
        // The transport layer populates Projects V2 fields with defaults
        // for search responses; the wire type must reflect that.
        assert_eq!(wire.status, "Ready");
        assert_eq!(wire.priority, "P2");
        assert_eq!(wire.issue_type.as_deref(), Some("Task"));
        assert!(wire.agent.is_none());
        assert!(wire.story_points.is_none());
        assert!(wire.defer_until.is_none());
        assert!(wire.milestone.is_none());
        assert!(wire.labels.is_empty());
        assert!(
            wire.created_at.contains('T'),
            "created_at should be RFC 3339"
        );
        assert!(wire.url.ends_with("/42"));
    }

    #[test]
    fn from_core_serializes_defer_until_as_iso_date() {
        let mut s = sample_summary(7);
        s.defer_until = Some(NaiveDate::from_ymd_opt(2099, 12, 31).unwrap());
        let wire = SearchIssueSummary::from_core(&s);
        assert_eq!(wire.defer_until.as_deref(), Some("2099-12-31"));
    }

    #[test]
    fn from_core_passes_through_labels_and_milestone() {
        let mut s = sample_summary(3);
        s.labels = vec!["bug".to_owned(), "p0".to_owned()];
        s.milestone = Some("v1.0".to_owned());
        let wire = SearchIssueSummary::from_core(&s);
        assert_eq!(wire.labels, vec!["bug".to_owned(), "p0".to_owned()]);
        assert_eq!(wire.milestone.as_deref(), Some("v1.0"));
    }
}
