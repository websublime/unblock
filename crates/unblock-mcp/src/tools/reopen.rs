//! Reopen tool — reopens a closed issue and re-evaluates its blocking
//! status against the freshly rebuilt graph.
//!
//! Per spec §8.7 this is a write tool that turns a Closed GitHub issue
//! back to Open, rebuilds the dependency graph, computes whether the
//! re-opened issue still has any open blockers, and updates its Projects
//! V2 Status field accordingly:
//!
//! - **Blocked** (at least one open blocker) → `Status → blocked`
//! - **Unblocked** (no open blockers) → `Status → ready`
//!
//! The wire format for `status` is the lowercase Projects V2 option slug
//! (`"blocked"` or `"ready"`) so it round-trips safely with the option
//! names created by the setup flow (see `unblock-github/src/projects.rs`)
//! and with the `by_status` keys returned by `list` and `stats` (R8
//! decision).
//!
//! ## Flow
//!
//! 1. **Validate** `id` — reject `0` with a `Validation` error before any
//!    network call (R1 decision; mirrors `search`'s `limit=Some(0)` fast
//!    fail at `search.rs:204-214`). `u64` inherently rejects negatives,
//!    so positivity collapses to `id >= 1`.
//! 2. **Fetch** the issue via [`GitHubApi::fetch_issue`] and assert the
//!    returned [`IssueState`] is [`Closed`](IssueState::Closed). A
//!    non-Closed state maps to [`IssueAlreadyOpenSnafu`] (R2 decision —
//!    see caveat below).
//! 3. **Reopen** via [`GitHubApi::reopen_issue`]. On success the GitHub
//!    mutation is durable: subsequent failures never rewind it.
//! 4. **Rebuild cache** via the crate-internal `execute_write_tool` in
//!    [`crate::tools`]. The write-tool helper invalidates, re-fetches
//!    graph data, and repopulates the cache; a rebuild failure leaves
//!    the cache empty and is surfaced to the caller (R3 decision).
//! 5. **Compute blocked** against the rebuilt graph using the local
//!    `has_open_blockers` helper (R4 — intentionally duplicated from
//!    `stats.rs` and `prime.rs`; a shared helper is deferred to a
//!    follow-up cleanup bead).
//! 6. **Best-effort Projects V2 Status update** — set the Status field
//!    to `blocked` or `ready` via the Projects V2 field ladder. Failures
//!    are logged and swallowed; the cache already holds the fresh issue
//!    view, so the agent can re-observe the status via `show` if needed.
//!    (The TODO for extracting the shared field-update helper is
//!    tracked by `unblock-b6b.79` / `unblock-29p.24` — do NOT refactor
//!    here.)
//! 7. **Return** [`ReopenResult`] with the issue number, the `blocked`
//!    flag, and the lowercase status slug.
//!
//! ## R2 caveat — `IssueAlreadyOpen` vs `IssueNotClosed`
//!
//! Spec §8.7 step 1 lists both `IssueNotClosed` and `IssueAlreadyOpen`
//! as possible validation errors. In practice
//! [`GitHubApi::fetch_issue`] only ever parses the upstream state as
//! [`IssueState::Open`] or [`IssueState::Closed`] (see
//! `unblock-github/src/graphql.rs` — any non-`CLOSED` string maps to
//! `Open`). That means the reopen handler can only observe the Open
//! branch today, so `IssueNotClosed` is effectively unreachable from
//! this path. We keep the domain variant [`IssueNotClosed`] defined in
//! `unblock-core/src/errors.rs` as defensive forward-compatibility
//! (future states like `ReadOnly` or third-party forks could legitimately
//! surface it), but the live reopen flow only emits
//! [`IssueAlreadyOpen`](unblock_core::errors::DomainError::IssueAlreadyOpen).
//!
//! ## R5 caveat — Status downgrade from `InProgress`
//!
//! Spec §8.7 prescribes the target status transitions
//! (`blocked | ready`) without any "keep `InProgress` if it was already
//! `InProgress`" guard (contrast with the close cascade at
//! `server.rs:1126`). Reopening an issue that was `InProgress` before
//! close (rare — close normally goes from Done) therefore resets Status
//! to `ready | blocked`. This is spec-correct and intentional.
//!
//! ## R3 caveat — cannot compute `blocked` after rebuild
//!
//! Two failure modes share a single posture:
//!
//! 1. **Empty cache.** If `fetch_graph_data()` inside
//!    [`crate::tools::rebuild_cache`] fails (e.g. transient GitHub 503),
//!    the cache is left invalidated.
//! 2. **Missing issue (race).** If the rebuild succeeds but the
//!    reopened issue is absent from the returned set — e.g. another
//!    agent re-closed it between our `reopen_issue` mutation and the
//!    `fetch_graph_data` rebuild — we cannot locate it in the cache.
//!
//! In both cases the reopen itself has already succeeded server-side,
//! so the mutation is not lost. The handler surfaces a 503-class
//! [`GitHubApi`](unblock_github::errors::Error::GitHubApi) error with a
//! message instructing the caller to re-run `show` to observe the final
//! status, rather than returning a fabricated `blocked = false`
//! envelope. Matches the stats R6 posture: propagate real errors rather
//! than silently returning a degraded-but-plausible envelope. Preserves
//! spec §14 invariants 8 and 13 (no fictional Status/`blocked` claims
//! when the graph cannot be consulted).
//!
//! ## Cache contract
//!
//! Invalidates the cache, rebuilds via `execute_write_tool`, then reads
//! the fresh issue set from the cache to locate the reopened issue for
//! the blocker evaluation. No separate network fetch is issued for the
//! blocker evaluation — everything rides on the single rebuild fetch
//! (spec "API calls: 1 (fetch) + 1 (reopen) + 1-2 (fields) + 1+
//! (rebuild)").
//!
//! ## Server registration
//!
//! The `#[tool]` registration on `UnblockServer` is **deliberately out
//! of scope** for this module — it is tracked by sibling bead
//! `unblock-29p.12`. This file only exposes the handler function and
//! data types.
//!
//! [`GitHubApi::fetch_issue`]: unblock_github::GitHubApi::fetch_issue
//! [`GitHubApi::reopen_issue`]: unblock_github::GitHubApi::reopen_issue
//! [`GitHubApi::fetch_graph_data`]: unblock_github::GitHubApi::fetch_graph_data
//! [`IssueNotClosed`]: unblock_core::errors::DomainError::IssueNotClosed
//! [`IssueAlreadyOpen`]: unblock_core::errors::DomainError::IssueAlreadyOpen
//! [`IssueAlreadyOpenSnafu`]: unblock_core::errors::IssueAlreadyOpenSnafu

use std::sync::Arc;

use rmcp::model::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument, warn};
use unblock_core::errors::{IssueAlreadyOpenSnafu, ValidationSnafu};
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{Issue, IssueState};
use unblock_github::GitHubApi;
use unblock_github::projects::FieldValue;

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;
use crate::tools::execute_write_tool;

/// Input parameters for the `reopen` MCP tool.
///
/// Per spec §8.7. `id` must be a positive integer (>= 1); `0` is rejected
/// up front with a [`DomainError::Validation`](unblock_core::errors::DomainError::Validation)
/// error before any GitHub call is issued (R1 decision).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReopenParams {
    /// Issue number to reopen. Must be a positive integer (>= 1).
    pub id: u64,
}

/// Result returned by the `reopen` MCP tool.
///
/// Per spec §8.7. The `status` field is the lowercase Projects V2 option
/// slug (`"blocked"` or `"ready"`), matching the by-status keys returned
/// by `list` and `stats` (R8 decision). It is never the
/// [`Display`](std::fmt::Display) form of
/// [`Status`](unblock_core::types::Status).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReopenResult {
    /// Issue number that was reopened.
    pub issue: u64,
    /// `true` if the reopened issue still has at least one open blocker
    /// in the freshly rebuilt dependency graph. When `true`, the
    /// Projects V2 Status field was set to `blocked`; otherwise `ready`.
    pub blocked: bool,
    /// Lowercase Projects V2 option slug for the new Status — either
    /// `"blocked"` or `"ready"`. Derived directly from `blocked`.
    pub status: String,
}

/// Return `true` when `issue` has at least one blocker that is still
/// OPEN.
///
/// Mirrors the identical helper in
/// [`prime`](crate::tools::prime) (`prime.rs:649`) and
/// [`stats`](crate::tools::stats) (`stats.rs:187`): outgoing edges in
/// the dependency graph point from the blocked issue to its blockers,
/// so we walk the outgoing neighbours and stop at the first one whose
/// [`IssueState`] is [`Open`](IssueState::Open).
///
/// Issues absent from the graph (no blockers recorded at build time)
/// are treated as unblocked — matching the stats and prime posture.
///
/// **R4 note:** this is the third copy of this helper in the crate. A
/// shared extraction is deferred to a follow-up cleanup bead (analogous
/// to `unblock-29p.33` for `refresh_cache_from`). Do NOT extract in
/// this bead.
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

/// Build a domain `Validation` error and lift it through the GitHub
/// error mapping so the resulting [`ErrorData`] mirrors the rest of the
/// MCP surface (HTTP 400 → `INVALID_PARAMS`). Same shape as the helper
/// in `search.rs:180-187`.
fn validation_error(message: impl Into<String>) -> ErrorData {
    let domain = ValidationSnafu {
        message: message.into(),
    }
    .build();
    let github = unblock_github::errors::Error::from(domain);
    github_error_to_mcp(github)
}

/// Validate the `id` parameter: reject `0` up front with
/// `INVALID_PARAMS`. Any `id >= 1` is accepted without issuing any
/// network call (R1 decision). `u64` already rejects negatives at the
/// type level, so we only need to guard the zero case.
fn validate_id(id: u64) -> Result<(), ErrorData> {
    if id == 0 {
        return Err(validation_error("id must be >= 1"));
    }
    Ok(())
}

/// Best-effort Projects V2 Status field update for the reopened issue.
///
/// Mirrors the close/claim/depends field-update ladder
/// (`server.rs:1046-1080` and siblings). Each level of the ladder is
/// defensive: a missing field cache, an unresolved project, or an
/// absent project item all degrade to a `tracing::warn!` rather than an
/// error — the reopen has already succeeded server-side and the caller
/// can observe the final Status via a follow-up `show` if the Projects
/// V2 integration is misconfigured.
///
/// The `slug` argument must be `"blocked"` or `"ready"`; other values
/// would simply not match any option in `field_ids.status.options` and
/// log a warning.
///
/// (R7 import path: `FieldValue` lives under `unblock_github::projects`,
/// same as the close handler uses.)
async fn update_status_field(client: &dyn GitHubApi, issue_node_id: &str, slug: &str) {
    // TODO(unblock-b6b.79 / unblock-29p.24): Extract shared project field
    // update helper to deduplicate this if-let ladder across close,
    // claim, depends, and now reopen. Not in scope for this bead.
    let Some(field_ids) = client.field_ids().await else {
        tracing::debug!(
            slug,
            "No field IDs cached — run setup first to enable project Status updates after reopen"
        );
        return;
    };

    let project_info = match client.resolve_project_info().await {
        Ok(info) => info,
        Err(err) => {
            warn!(error = %err, "Failed to resolve project info — reopened issue Status field will not be set");
            return;
        }
    };

    let item_id = match client
        .get_project_item_id(issue_node_id, &project_info.id)
        .await
    {
        Ok(id) => id,
        Err(err) => {
            warn!(error = %err, "Failed to get project item ID for reopened issue — Status field will not be set");
            return;
        }
    };

    let Some(option_id) = field_ids.status.options.get(slug) else {
        warn!(
            slug,
            "Projects V2 Status field has no option matching slug — skipping Status update after reopen"
        );
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
        warn!(error = %err, slug, "Failed to set Status field on reopened issue");
    }
}

/// Execute the `reopen` tool handler.
///
/// See the module-level docs for the full spec contract and the
/// R1..R8 design decisions. Flow outline:
///
/// 1. Validate `params.id` (reject `0` fast).
/// 2. Fetch + validate + reopen inside `execute_write_tool` so the
///    mutation is followed by a cache rebuild atomically.
/// 3. Pull the fresh graph + issue vector from the cache; if either is
///    absent propagate a clear error (R3).
/// 4. Locate the reopened issue in the rebuilt cache and compute
///    `blocked` via the local `has_open_blockers` helper. If the issue
///    is missing from the rebuilt set (short race window — another
///    agent re-closed it between steps 2 and 3) surface a 503-class
///    error instead of defaulting `blocked = false` (R3).
/// 5. Best-effort Projects V2 Status update.
/// 6. Return [`ReopenResult`].
///
/// # Errors
///
/// Returns [`ErrorData`] in the following cases:
/// - `id == 0` → `INVALID_PARAMS` ("id must be >= 1").
/// - `fetch_issue` fails (e.g. 404) → mapped via `github_error_to_mcp`.
/// - The fetched issue is not Closed → `IssueAlreadyOpen` (R2).
/// - `reopen_issue` fails → mapped via `github_error_to_mcp` (e.g. 404
///   maps to `INVALID_PARAMS`).
/// - Cache rebuild fails and leaves the cache empty, or the rebuild
///   succeeds but the reopened issue is absent from the rebuilt set
///   (concurrent re-close race) → a 503-class
///   [`GitHubApi`](unblock_github::errors::Error::GitHubApi) error is
///   surfaced so the caller re-runs `show` to observe the final state
///   (R3).
#[instrument(
    skip(state, params),
    name = "handle_reopen",
    fields(
        agent.kind = state.agent_kind_str(),
        issue_number = params.id,
    ),
)]
pub async fn handle_reopen(
    state: &ServerState,
    params: ReopenParams,
) -> Result<ReopenResult, ErrorData> {
    let issue_number = params.id;
    info!("Reopen tool invoked");

    // Step 1: fast-fail validation before any network call (R1).
    validate_id(issue_number)?;

    // Step 2: fetch + validate + reopen + rebuild (via execute_write_tool).
    // The closure returns the reopened issue's `node_id` so Phase 2 (the
    // best-effort Projects V2 Status update) can reuse it without issuing
    // a second `fetch_issue` call. Threading the node_id through the
    // closure's Ok value keeps the data flow linear — no shared-state
    // locks across await points.
    let node_id: String = {
        let client = Arc::clone(&state.github);
        execute_write_tool(state, || async move {
            // 2a. Fetch and assert state == Closed. `IssueNotClosed` is
            // defensive-only (see module R2 note); the live path only
            // emits `IssueAlreadyOpen`.
            let issue = client.fetch_issue(issue_number).await?;

            if issue.state != IssueState::Closed {
                return Err(IssueAlreadyOpenSnafu {
                    number: issue_number,
                }
                .build()
                .into());
            }

            let node_id = issue.node_id.clone();

            // 2b. Reopen (REST PATCH state=open). IssueNotFound on 404
            // propagates via github_error_to_mcp.
            client.reopen_issue(issue_number).await?;

            Ok(node_id)
        })
        .await?
    };

    // Step 3: read the freshly rebuilt cache. If the rebuild inside
    // execute_write_tool failed, the cache is empty — the reopen has
    // already succeeded server-side but we cannot compute `blocked`
    // here. Surface a real error instructing the caller to re-run
    // `show` (R3 — honest partial state; do NOT default to
    // `blocked=false`).
    let graph_arc = state.cache.get_graph().await;
    let issues_arc = state.cache.get_issues().await;

    let (Some(graph), Some(issues)) = (graph_arc, issues_arc) else {
        warn!(
            issue_number,
            "Cache empty after reopen — rebuild failed; caller must re-run `show` to observe final status"
        );
        // Surface as a 503-class error so MCP clients see INTERNAL_ERROR
        // and can retry or fall back to a `show` call. The mutation is
        // durable on GitHub regardless of this failure.
        return Err(github_error_to_mcp(
            unblock_github::errors::GitHubApiSnafu {
                status: 503_u16,
                message: format!(
                    "Issue #{issue_number} reopened successfully, but cache rebuild failed — please re-run `show` to observe the final blocked status"
                ),
            }
            .build(),
        ));
    };

    // Step 4: locate the reopened issue in the rebuilt cache and
    // compute `blocked` against the new graph. The fetch_graph_data
    // call already re-fetched the issue as Open (since reopen_issue
    // succeeded), so the cache view is canonical on the happy path.
    //
    // If the issue is missing from the rebuilt set — a short race
    // window where another agent re-closed it between our
    // `reopen_issue` mutation and the `fetch_graph_data` rebuild — we
    // cannot compute `blocked` without consulting the graph. Surface a
    // 503-class error (mirroring the empty-cache arm above) rather
    // than silently defaulting to `blocked = false`, which would
    // fabricate a ready/unblocked claim without actually evaluating
    // the graph. Preserves spec §14 invariants 8 and 13. The reopen
    // mutation remains durable on GitHub regardless of this failure.
    let Some(issue) = issues.iter().find(|i| i.number == issue_number) else {
        warn!(
            issue_number,
            "Reopened issue not present in rebuilt cache (possible concurrent re-close) — surfacing partial-state error"
        );
        return Err(github_error_to_mcp(
            unblock_github::errors::GitHubApiSnafu {
                status: 503_u16,
                message: format!(
                    "Issue #{issue_number} reopened successfully, but the rebuilt cache does not contain it (possible concurrent close by another agent) — please re-run `show` to observe the final blocked status"
                ),
            }
            .build(),
        ));
    };

    let blocked = has_open_blockers(issue, graph.as_ref());

    let slug = if blocked { "blocked" } else { "ready" };

    // Step 5: best-effort Projects V2 Status field update. Uses the
    // node_id captured and returned by the write-tool closure above —
    // avoids a second fetch_issue call.
    update_status_field(state.github.as_ref(), &node_id, slug).await;

    Ok(ReopenResult {
        issue: issue_number,
        blocked,
        status: slug.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use rmcp::model::ErrorCode;
    use unblock_core::types::{BlockingEdge, IssueType, Priority, QualifiedId, Status};

    use super::*;

    // ── Test helpers ──────────────────────────────────────────────────

    fn qid(number: u64) -> QualifiedId {
        QualifiedId::new("acme", "widgets", number)
    }

    fn reopen_issue(number: u64, status: Status, state: IssueState) -> Issue {
        Issue {
            qualified_id: qid(number),
            number,
            node_id: format!("I_{number}"),
            title: format!("Reopen fixture #{number}"),
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
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            url: format!("https://github.com/acme/widgets/issues/{number}"),
            comments: vec![],
            blocked_by: vec![],
            blocking: vec![],
            parent: None,
            sub_issues: vec![],
        }
    }

    // ── validate_id ──────────────────────────────────────────────────

    #[test]
    fn validate_id_rejects_zero() {
        let err = validate_id(0).expect_err("id=0 must fail validation");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("id"),
            "validation message should mention id: {}",
            err.message,
        );
    }

    #[test]
    fn validate_id_accepts_positive() {
        validate_id(1).expect("id=1 should be accepted");
        validate_id(42).expect("id=42 should be accepted");
        validate_id(u64::MAX).expect("u64::MAX should be accepted");
    }

    // ── has_open_blockers ───────────────────────────────────────────

    #[test]
    fn has_open_blockers_false_when_issue_absent_from_graph() {
        let g = DependencyGraph::build(&[], &[]);
        let orphan = reopen_issue(42, Status::Ready, IssueState::Open);
        assert!(!has_open_blockers(&orphan, &g));
    }

    #[test]
    fn has_open_blockers_true_for_downstream_of_open_upstream() {
        let upstream = reopen_issue(1, Status::Ready, IssueState::Open);
        let downstream = reopen_issue(2, Status::Ready, IssueState::Open);
        let issues = vec![upstream.clone(), downstream.clone()];
        // Edge: downstream (2) source -> upstream (1) target.
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
        let upstream = reopen_issue(1, Status::Closed, IssueState::Closed);
        let downstream = reopen_issue(2, Status::Ready, IssueState::Open);
        let issues = vec![upstream.clone(), downstream.clone()];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];
        let g = DependencyGraph::build(&issues, &edges);
        // Upstream blocker is Closed → downstream looks unblocked.
        assert!(!has_open_blockers(&downstream, &g));
    }

    // ── ReopenResult serialization shape ────────────────────────────

    #[test]
    fn reopen_result_serializes_status_as_lowercase_slug() {
        // R8: the wire format is the lowercase Projects V2 option slug,
        // not the Display form of Status.
        let ready = ReopenResult {
            issue: 7,
            blocked: false,
            status: "ready".to_owned(),
        };
        let json = serde_json::to_value(&ready).expect("ReopenResult should serialize");
        assert_eq!(json["issue"], 7);
        assert_eq!(json["blocked"], false);
        assert_eq!(json["status"], "ready");

        let blocked = ReopenResult {
            issue: 9,
            blocked: true,
            status: "blocked".to_owned(),
        };
        let json = serde_json::to_value(&blocked).expect("ReopenResult should serialize");
        assert_eq!(json["issue"], 9);
        assert_eq!(json["blocked"], true);
        assert_eq!(json["status"], "blocked");
    }
}
