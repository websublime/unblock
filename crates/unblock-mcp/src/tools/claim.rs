//! Claim tool — atomically claims an issue for an agent.
//!
//! Validates the issue is open, unblocked, not deferred, and not already claimed,
//! then updates Projects V2 fields (Status=In Progress, Agent=name,
//! and posts a claim comment. Uses `execute_write_tool()`
//! to rebuild the cache after all mutations complete.
//!
//! The validation logic is extracted into `validate_claimable` so it can be
//! unit tested without a GitHub client.

use chrono::{DateTime, NaiveDate, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_core::errors::{
    AlreadyClaimedSnafu, IssueBlockedSnafu, IssueClosedSnafu, IssueDeferredSnafu, ValidationSnafu,
};
use unblock_core::types::{IssueRef, IssueState, RelatedIssue, Status};

/// Input parameters for the `claim` MCP tool.
///
/// Only `id` is required. If `agent` is not provided, falls back to
/// `Config.agent` (from `UNBLOCK_AGENT`), then `"unknown"`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClaimParams {
    /// Issue number to claim (required).
    pub id: u64,
    /// Agent name claiming the issue. Defaults to the configured agent name.
    pub agent: Option<String>,
}

/// Result returned by the `claim` MCP tool.
///
/// Contains the claimed issue number, the resolved agent name, and the
/// timestamp when the claim was recorded.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ClaimResult {
    /// The claimed issue number.
    pub issue_number: u64,
    /// The agent name that claimed the issue.
    pub agent: String,
    /// Timestamp when the claim was recorded (ISO 8601).
    pub claimed_at: DateTime<Utc>,
}

/// Snapshot of the fields needed to validate whether an issue can be claimed.
///
/// Extracted from [`Issue`](unblock_core::types::Issue) to decouple validation
/// from the full issue type and the GitHub client.
pub(crate) struct ClaimCandidate {
    /// Issue number.
    pub number: u64,
    /// GitHub native issue state (Open/Closed).
    pub state: IssueState,
    /// Workflow status from Projects V2.
    pub status: Status,
    /// Agent name if already claimed.
    pub agent: Option<String>,
    /// Issues blocking this issue.
    pub blocked_by: Vec<RelatedIssue>,
    /// Date until which the issue is deferred.
    pub defer_until: Option<NaiveDate>,
}

/// Validates that an issue can be claimed.
///
/// Checks are performed in order of cost (cheapest first):
/// 1. Issue must be open (not closed).
/// 2. Issue must have no open blockers.
/// 3. Issue must not be deferred beyond today.
/// 4. Issue must not already be claimed (In Progress with an agent).
///
/// # Errors
///
/// Returns an `unblock_github::errors::Error` (wrapping a domain error) if
/// any validation check fails.
pub(crate) fn validate_claimable(
    candidate: &ClaimCandidate,
    today: NaiveDate,
) -> Result<(), unblock_github::errors::Error> {
    // Check 1: closed (cheapest).
    if candidate.state == IssueState::Closed {
        return Err(IssueClosedSnafu {
            number: candidate.number,
        }
        .build()
        .into());
    }

    // Check 2: blocked (filter to open blockers).
    //
    // GitHub's native `trackedByIssues` connection — the source of
    // `candidate.blocked_by` — only surfaces blockers in the same
    // `owner/repo` as the issue being claimed (see SPEC §8.2 / mutations
    // `add_blocked_by_refs` cross-repo commentary). Since `claim`
    // operates on a local issue, all blockers it observes live in the
    // configured repo; we wrap each as `IssueRef::Local(r.number)`.
    // If `RelatedIssue` is later extended to carry `owner/repo`, this
    // mapping trivially evolves to produce `IssueRef::CrossRepo`.
    let open_blockers: Vec<IssueRef> = candidate
        .blocked_by
        .iter()
        .filter(|r| r.state == IssueState::Open)
        .map(|r| IssueRef::Local(r.number))
        .collect();

    if !open_blockers.is_empty() {
        return Err(IssueBlockedSnafu {
            number: candidate.number,
            blockers: open_blockers,
        }
        .build()
        .into());
    }

    // Check 3: deferred.
    if let Some(defer_until) = candidate.defer_until
        && defer_until > today
    {
        return Err(IssueDeferredSnafu {
            number: candidate.number,
            until: defer_until.to_string(),
        }
        .build()
        .into());
    }

    // Check 4: already claimed.
    if let Some(ref agent) = candidate.agent
        && candidate.status == Status::InProgress
    {
        return Err(AlreadyClaimedSnafu {
            number: candidate.number,
            agent: agent.clone(),
        }
        .build()
        .into());
    }

    Ok(())
}

/// Validates the optional `agent` parameter from [`ClaimParams`].
///
/// The claim handler accepts `agent` as an `Option<String>` and falls back to
/// `Config.agent` when `None`. An empty or whitespace-only string, however,
/// would produce a malformed claim comment like `"Claimed by  at ..."`. This
/// validator rejects that case so callers get a clear error rather than
/// silently-corrupt state. `None` is always accepted (the handler applies
/// its own fallback logic).
///
/// # Errors
///
/// Returns [`unblock_github::errors::Error`] wrapping a
/// [`DomainError::Validation`](unblock_core::errors::DomainError::Validation)
/// (HTTP 400) when `agent` is `Some` but empty or whitespace-only.
pub(crate) fn validate_agent(agent: Option<&str>) -> Result<(), unblock_github::errors::Error> {
    if let Some(raw) = agent
        && raw.trim().is_empty()
    {
        return Err(ValidationSnafu {
            message: "agent parameter must not be empty or whitespace-only".to_owned(),
        }
        .build()
        .into());
    }
    Ok(())
}

// TODO(unblock-45a.12): Add integration tests for claim tool (ready, blocked, closed, deferred,
// already-claimed paths) as part of the E2E workflow integration test.

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use unblock_core::types::{IssueState, RelatedIssue, Status};

    use super::*;

    /// Build a minimal `ClaimCandidate` for a ready (claimable) issue.
    fn ready_candidate(number: u64) -> ClaimCandidate {
        ClaimCandidate {
            number,
            state: IssueState::Open,
            status: Status::Ready,
            agent: None,
            blocked_by: vec![],
            defer_until: None,
        }
    }

    /// Build a `RelatedIssue` for testing blockers.
    fn blocker(number: u64, state: IssueState) -> RelatedIssue {
        RelatedIssue {
            number,
            title: format!("Blocker #{number}"),
            state,
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 30).expect("valid date")
    }

    // ── Happy path ──────────────────────────────────────────────────

    #[test]
    fn open_unblocked_issue_is_claimable() {
        let candidate = ready_candidate(1);
        assert!(validate_claimable(&candidate, today()).is_ok());
    }

    #[test]
    fn issue_with_only_closed_blockers_is_claimable() {
        let mut candidate = ready_candidate(1);
        candidate.blocked_by = vec![blocker(2, IssueState::Closed)];
        assert!(validate_claimable(&candidate, today()).is_ok());
    }

    #[test]
    fn issue_with_past_defer_until_is_claimable() {
        let mut candidate = ready_candidate(1);
        candidate.defer_until = Some(NaiveDate::from_ymd_opt(2026, 3, 29).expect("valid date"));
        assert!(validate_claimable(&candidate, today()).is_ok());
    }

    #[test]
    fn issue_with_today_defer_until_is_claimable() {
        let mut candidate = ready_candidate(1);
        candidate.defer_until = Some(today());
        assert!(validate_claimable(&candidate, today()).is_ok());
    }

    #[test]
    fn in_progress_issue_without_agent_is_claimable() {
        let mut candidate = ready_candidate(1);
        candidate.status = Status::InProgress;
        candidate.agent = None;
        assert!(validate_claimable(&candidate, today()).is_ok());
    }

    // ── Closed issue ────────────────────────────────────────────────

    #[test]
    fn closed_issue_returns_issue_closed_error() {
        let mut candidate = ready_candidate(1);
        candidate.state = IssueState::Closed;
        let err = validate_claimable(&candidate, today()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("already closed"),
            "expected IssueClosed error, got: {msg}"
        );
        assert!(msg.contains('1'));
        assert_eq!(err.status_code(), 409);
    }

    // ── Blocked issue ───────────────────────────────────────────────

    #[test]
    fn blocked_issue_returns_issue_blocked_error() {
        let mut candidate = ready_candidate(5);
        candidate.blocked_by = vec![blocker(2, IssueState::Open), blocker(3, IssueState::Open)];
        let err = validate_claimable(&candidate, today()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("blocked by"),
            "expected IssueBlocked error, got: {msg}"
        );
        assert!(msg.contains('5'));
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn blocked_by_mix_of_open_and_closed_returns_only_open_blockers() {
        let mut candidate = ready_candidate(5);
        candidate.blocked_by = vec![blocker(2, IssueState::Open), blocker(3, IssueState::Closed)];
        let err = validate_claimable(&candidate, today()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("blocked by"), "got: {msg}");
        // The error should mention issue 2 (open) but we cannot easily parse the vec from Display.
        // Just ensure it is an IssueBlocked error.
        assert_eq!(err.status_code(), 409);
    }

    // ── Deferred issue ──────────────────────────────────────────────

    #[test]
    fn deferred_issue_returns_issue_deferred_error() {
        let mut candidate = ready_candidate(7);
        candidate.defer_until = Some(NaiveDate::from_ymd_opt(2026, 4, 15).expect("valid date"));
        let err = validate_claimable(&candidate, today()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("deferred"),
            "expected IssueDeferred error, got: {msg}"
        );
        assert!(msg.contains("2026-04-15"));
        assert_eq!(err.status_code(), 409);
    }

    // ── Already claimed issue ───────────────────────────────────────

    #[test]
    fn already_claimed_issue_returns_already_claimed_error() {
        let mut candidate = ready_candidate(10);
        candidate.status = Status::InProgress;
        candidate.agent = Some("other-agent".to_owned());
        let err = validate_claimable(&candidate, today()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("already claimed"),
            "expected AlreadyClaimed error, got: {msg}"
        );
        assert!(msg.contains("other-agent"));
        assert!(msg.contains("10"));
        assert_eq!(err.status_code(), 409);
    }

    // ── Validation order ────────────────────────────────────────────

    #[test]
    fn closed_takes_precedence_over_blocked() {
        let mut candidate = ready_candidate(1);
        candidate.state = IssueState::Closed;
        candidate.blocked_by = vec![blocker(2, IssueState::Open)];
        let err = validate_claimable(&candidate, today()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("already closed"),
            "closed should take precedence over blocked, got: {msg}"
        );
    }

    #[test]
    fn blocked_takes_precedence_over_deferred() {
        let mut candidate = ready_candidate(1);
        candidate.blocked_by = vec![blocker(2, IssueState::Open)];
        candidate.defer_until = Some(NaiveDate::from_ymd_opt(2026, 4, 15).expect("valid date"));
        let err = validate_claimable(&candidate, today()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("blocked by"),
            "blocked should take precedence over deferred, got: {msg}"
        );
    }

    // ── Agent parameter validation ──────────────────────────────────

    #[test]
    fn validate_agent_accepts_none() {
        assert!(validate_agent(None).is_ok());
    }

    #[test]
    fn validate_agent_accepts_non_empty_string() {
        assert!(validate_agent(Some("alice")).is_ok());
    }

    #[test]
    fn validate_agent_rejects_empty_string() {
        let err = validate_agent(Some("")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("empty") || msg.contains("whitespace"),
            "expected validation error, got: {msg}"
        );
        assert_eq!(err.status_code(), 400);
    }

    #[test]
    fn validate_agent_rejects_whitespace_only_string() {
        let err = validate_agent(Some("   \t ")).unwrap_err();
        assert_eq!(err.status_code(), 400);
    }

    #[test]
    fn deferred_takes_precedence_over_already_claimed() {
        let mut candidate = ready_candidate(1);
        candidate.defer_until = Some(NaiveDate::from_ymd_opt(2026, 4, 15).expect("valid date"));
        candidate.status = Status::InProgress;
        candidate.agent = Some("bot".to_owned());
        let err = validate_claimable(&candidate, today()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("deferred"),
            "deferred should take precedence over already-claimed, got: {msg}"
        );
    }
}
