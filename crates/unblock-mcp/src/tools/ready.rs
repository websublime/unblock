//! Ready tool — finds issues with no active blockers that can be worked on now.
//!
//! This is the primary tool agents call to find work. It reads from the
//! in-memory cache (rebuilding lazily if stale) and applies optional filters
//! for priority, issue type, milestone, agent, and label.
//!
//! This is a read tool with cache-aware logic — it checks cache freshness
//! and triggers a rebuild if stale, but does not mutate GitHub state.

use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_core::types::IssueSummary;

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
/// Contains the filtered, sorted list of ready issues, a count, and
/// a staleness indicator.
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
            issue_type: summary.issue_type.map(|it| format!("{it:?}")),
            status: format!("{:?}", summary.status),
            priority: format!("{:?}", summary.priority),
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

    ready_set
        .iter()
        // Filter by issue_type (case-insensitive Debug match).
        .filter(|s| {
            params.issue_type.as_ref().is_none_or(|filter| {
                s.issue_type
                    .is_some_and(|it| format!("{it:?}").eq_ignore_ascii_case(filter))
            })
        })
        // Filter by priority (case-insensitive Debug match).
        .filter(|s| {
            params
                .priority
                .as_ref()
                .is_none_or(|filter| format!("{:?}", s.priority).eq_ignore_ascii_case(filter))
        })
        // Filter by milestone (exact match).
        .filter(|s| {
            params
                .milestone
                .as_ref()
                .is_none_or(|filter| s.milestone.as_deref() == Some(filter.as_str()))
        })
        // Filter by agent (exact match).
        .filter(|s| {
            params
                .agent
                .as_ref()
                .is_none_or(|filter| s.agent.as_deref() == Some(filter.as_str()))
        })
        // Filter by label (case-insensitive, any match).
        .filter(|s| {
            params
                .label
                .as_ref()
                .is_none_or(|filter| s.labels.iter().any(|l| l.eq_ignore_ascii_case(filter)))
        })
        // Exclude deferred issues (defer_until > today).
        .filter(|s| s.defer_until.is_none_or(|d| d <= today))
        // Exclude InProgress (claimed) unless include_claimed is true.
        .filter(|s| include_claimed || s.status != unblock_core::types::Status::InProgress)
        .take(limit)
        .map(ReadyIssueSummary::from_core)
        .collect()
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
            status: Status::Open,
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
}
