//! `dep_remove` tool — removes a blocking edge between two issues and,
//! when the source ends up with zero open blockers, flips its Projects
//! V2 Status to `ready`.
//!
//! Per spec §8.5 this is a write tool:
//!
//! 1. Parse + normalize both `source` and `target` against the
//!    configured repo. Cross-repo references are supported on both sides
//!    per spec §5.6 (cross-repo scope table).
//! 2. (Defensive) reject `source == target` on resolved
//!    [`QualifiedId`]s. Spec §8.5 does
//!    not mandate this, but §8.4 (`depends`) does — we mirror the
//!    restriction so the two mutation tools present a symmetric surface
//!    and an edge cannot be "removed" from an issue to itself (which
//!    would never have been creatable via `depends`). See the PR
//!    description for the rationale; this is NOT hidden scope creep.
//! 3. Pre-mutation edge-existence guard — honours spec §14 Invariant 11
//!    ("Validation before mutation") on ALL paths. The probe classifies
//!    the edge into three outcomes:
//!    - **Present** — proceed to step 4 and run the mutation.
//!    - **Missing (no such edge)** — surface `DepRemoveResult { removed:
//!      false, ... }` on the wire without issuing the mutation (unified
//!      across paths by `unblock-29p.54`; the prior warm+both-Local
//!      `INVALID_PARAMS` posture was retired).
//!    - **Endpoint Closed** — surface
//!      [`DomainError::EndpointClosed`][`unblock_core::errors::DomainError::EndpointClosed`]
//!      (HTTP 409 Conflict → MCP `INVALID_PARAMS`) naming the Closed
//!      endpoint's [`QualifiedId`]. Introduced by bead `unblock-a36`
//!      after `fetch_graph_data` was widened to `states: [OPEN, CLOSED]`:
//!      with Closed issues now observable in the cached graph (and via
//!      `fetch_issue_ref` on the cold path, where they were already
//!      visible), the previous "collapse Closed endpoint into `no edge`"
//!      posture produced a misleading message. The error explicitly
//!      instructs the caller to reopen the issue via the `reopen` tool
//!      or accept the dangling edge.
//!
//!    Cache-mode branching (identical to the two-outcome posture the
//!    three-outcome posture replaced):
//!    - **Warm cache AND both endpoints `Local`** — fast path: consult
//!      the in-memory graph. The `issue_state` snapshot disambiguates
//!      Closed endpoints from absent nodes (no extra RTT).
//!    - **Cold cache OR at least one endpoint cross-repo** — fetch the
//!      source issue via [`GitHubApi::fetch_issue_ref`] (1 GraphQL RTT)
//!      and inspect its `state` plus its `trackedByIssues` list. The
//!      `FETCH_ISSUE_QUERY` trackedBy subselection carries both
//!      `repository { owner { login } name }` and `state` (see
//!      `graphql.rs`) so the Closed-endpoint check needs no second
//!      round-trip regardless of whether the source or the target is
//!      the Closed side.
//! 4. Call [`GitHubApi::remove_blocked_by_refs`] inside
//!    `execute_write_tool` so the mutation is followed by an atomic
//!    cache invalidate + rebuild. Only reached when the edge was
//!    confirmed present.
//! 5. Re-evaluate the source's blocker set against the freshly rebuilt
//!    graph. If the source is `Local` AND `has_open_blockers` returns
//!    `false` (zero open blockers remain), issue a best-effort Projects
//!    V2 Status update pinned to the `ready` slug. If the source is
//!    cross-repo, skip the Status update entirely: the configured
//!    project's `ProjectInfo` / `get_project_item_id` ladder is scoped
//!    to the configured project, matching the cross-repo posture of the
//!    `depends` handler (spec §5.6 footnote).
//! 6. Return [`DepRemoveResult`] with `removed = true` when the
//!    mutation ran, or `removed = false` when the pre-mutation guard
//!    proved the edge did not exist (uniform across ALL paths —
//!    warm-local, warm-cross-repo, cold-local, cold-cross-repo — per
//!    `unblock-29p.54`). The source/target are rendered in canonical
//!    [`IssueRef`] form and `message` documents what happened. A Closed
//!    endpoint short-circuits *before* step 4 and returns an error
//!    envelope (not a `DepRemoveResult`) — see step 3 above.
//!
//! ## Status-transition policy when blockers remain
//!
//! Spec §8.5 step 5 reads literally: *"If source now has zero open
//! blockers: Status → ready"* — silent about the non-zero-blockers case.
//! We deliberately leave Status untouched when at least one blocker
//! remains. The source was already `Blocked` before this call (otherwise
//! the edge we removed would not have existed), and any post-removal
//! `InProgress` / `Ready` transitions triggered by other paths must not
//! be clobbered here.
//!
//! ## R3 caveat — cache empty after rebuild
//!
//! If `execute_write_tool` fails to repopulate the cache (e.g.
//! transient GitHub 503 after the mutation landed), the handler cannot
//! compute `has_open_blockers` locally. The removal itself is durable on
//! GitHub's side, so we surface a 503-style
//! [`unblock_github::errors::Error`] error with a
//! message instructing the caller to re-run `show`. This matches
//! `reopen.rs:378-395`.
//!
//! ## R4 caveat — duplicated helpers
//!
//! `has_open_blockers` is the **fourth** local copy of the
//! blocker-walk helper in `unblock-mcp/src/tools`
//! (`prime` / `stats` / `reopen` / `dep_remove`). Extraction is tracked
//! by bead `unblock-29p.33` — do NOT extract here.
//!
//! The Projects V2 Status field update ladder (`update_status_to_ready`)
//! is the **fifth** copy
//! (`close` / `claim` / `depends` / `reopen` / `dep_remove`). Extraction
//! is tracked by bead `unblock-29p.24` — do NOT extract here.
//!
//! ## Server registration
//!
//! The `#[tool]` registration on `UnblockServer` is **deliberately out
//! of scope** for this module — it is tracked by sibling bead
//! `unblock-29p.12`. This file exposes [`handle_dep_remove`] and the
//! data types [`DepRemoveParams`] / [`DepRemoveResult`] so the sibling
//! bead can wire the router without touching mutation logic.
//!
//! [`GitHubApi::remove_blocked_by_refs`]: unblock_github::GitHubApi::remove_blocked_by_refs
//! [`GitHubApi::fetch_graph_data`]: unblock_github::GitHubApi::fetch_graph_data

use std::sync::Arc;

use rmcp::model::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument, warn};
use unblock_core::errors::{EndpointClosedSnafu, InvalidIssueRefSnafu, ValidationSnafu};
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{Issue, IssueRef, IssueState, QualifiedId};
use unblock_github::GitHubApi;
use unblock_github::projects::FieldValue;

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;
use crate::tools::execute_write_tool;

/// Input parameters for the `dep_remove` MCP tool.
///
/// Per spec §8.5. Both `source` and `target` accept an
/// [`IssueRef`]-compatible string: a bare local number (`"42"`), a
/// hash-prefixed local number (`"#42"`), or a cross-repo reference
/// (`"owner/repo#42"`). See [`IssueRef`] for the full grammar.
///
/// Cross-repo references are supported on both sides per spec §5.6.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DepRemoveParams {
    /// Currently-blocked issue (edge source). Accepts `42`, `#42`, or
    /// `owner/repo#42`.
    pub source: String,
    /// Currently-blocking issue (edge target). Accepts `42`, `#42`, or
    /// `owner/repo#42`.
    pub target: String,
}

/// Result returned by the `dep_remove` MCP tool.
///
/// Per spec §8.5. The `source` and `target` fields echo the canonical
/// rendering of the normalized [`IssueRef`]: `#n` for a local reference,
/// `owner/repo#n` for a cross-repo reference.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DepRemoveResult {
    /// Uniform posture across all paths (warm-local, warm-cross-repo,
    /// cold-local, cold-cross-repo): `true` iff the edge existed and
    /// was removed; `false` iff the pre-mutation probe proved the
    /// edge did not exist and the mutation was skipped (no-op early
    /// return). Unified by `unblock-29p.54`; the prior warm +
    /// both-`Local` `INVALID_PARAMS` posture was retired.
    pub removed: bool,
    /// The source issue reference in canonical form.
    pub source: String,
    /// The target issue reference in canonical form.
    pub target: String,
    /// Human-readable confirmation message.
    pub message: String,
}

/// Return `true` when `issue` has at least one blocker that is still
/// OPEN in the dependency graph.
///
/// Mirrors the identical helper in [`reopen`](crate::tools::reopen),
/// [`prime`](crate::tools::prime), and [`stats`](crate::tools::stats):
/// outgoing edges in the dependency graph point from the blocked issue
/// to its blockers, so we walk the outgoing neighbours and stop at the
/// first one whose [`IssueState`] is [`Open`](IssueState::Open). Issues
/// absent from the graph are treated as unblocked — matching the
/// stats/prime/reopen posture.
///
/// **R4 note:** fourth copy of this helper in the crate. Extraction is
/// deferred to `unblock-29p.33`. Do NOT extract in this bead.
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
/// MCP surface (HTTP 400 → `INVALID_PARAMS`). Same shape as `reopen.rs`
/// and `search.rs`.
fn validation_error(message: impl Into<String>) -> ErrorData {
    let domain = ValidationSnafu {
        message: message.into(),
    }
    .build();
    let github = unblock_github::errors::Error::from(domain);
    github_error_to_mcp(github)
}

/// Parse an [`IssueRef`] from a raw string, surfacing a malformed
/// reference as [`InvalidIssueRefSnafu`] lifted through
/// [`github_error_to_mcp`] (HTTP 400 → MCP `-32602`).
///
/// Per SPEC §11.1 / plan Task 02.02 "Error-side wiring" the domain
/// variant carries only the raw `input`, so this helper no longer
/// tags a `source`/`target` field name in the message — the caller
/// already knows which side they passed. Both `handle_dep_remove`
/// call sites parse `source` first, then `target`, so on a malformed
/// input the position (and therefore the field) is implicit in the
/// failure ordering.
fn parse_ref(value: &str) -> Result<IssueRef, ErrorData> {
    value.parse::<IssueRef>().map_err(|_| {
        github_error_to_mcp(unblock_github::errors::Error::from(
            InvalidIssueRefSnafu {
                input: value.to_owned(),
            }
            .build(),
        ))
    })
}

/// Outcome of the pre-mutation edge-existence probe.
///
/// The three variants distinguish the decisions the handler must make:
/// proceed with the mutation, early-return `removed: false`, or surface
/// a Closed-endpoint error.
#[derive(Debug, PartialEq, Eq)]
enum EdgePresence {
    /// The edge was confirmed present — proceed to
    /// `remove_blocked_by_refs`.
    Present,
    /// The edge was proved absent — either via the warm-cache
    /// in-memory lookup (both-`Local` fast path) or via the cold-cache
    /// / cross-repo single-issue probe. Return
    /// `DepRemoveResult { removed: false, ... }` WITHOUT calling the
    /// mutation (spec §14 Invariant 11 — validation before mutation;
    /// calling the mutation after proving absence would contradict
    /// the validation outcome).
    MissingSkipMutation,
    /// At least one endpoint resolves to an issue whose GitHub native
    /// state is [`IssueState::Closed`]. Surface
    /// [`DomainError::EndpointClosed`][`unblock_core::errors::DomainError::EndpointClosed`]
    /// naming the Closed endpoint so the agent can reopen it (via the
    /// `reopen` tool) or accept the dangling edge. Spec §8.5 / bead
    /// `unblock-a36`.
    ///
    /// Carries the [`QualifiedId`] of the endpoint that is closed —
    /// which is the side the probe actually observed as Closed, so the
    /// error message is unambiguous even when both endpoints happen to
    /// be closed (only the first-observed one surfaces).
    EndpointClosed(QualifiedId),
}

/// Warm-cache edge-existence guard: report presence / absence /
/// Closed-endpoint for the edge from `source_qid` → `target_qid` in the
/// currently cached graph. Only invoked when the cache is warm AND both
/// endpoints are `Local` (see [`probe_edge_presence`]).
///
/// Three outcomes:
/// - `Ok(EdgePresence::Present)` — edge confirmed present.
/// - `Ok(EdgePresence::EndpointClosed(qid))` — source or target is
///   observed with [`IssueState::Closed`] in the graph's
///   [`issue_state`](unblock_core::graph::DependencyGraph::issue_state)
///   snapshot. Source is checked before target; only the first-observed
///   Closed endpoint is returned. Added by bead `unblock-a36` once the
///   `fetch_graph_data` widening let Closed issues into the cache.
/// - `Ok(EdgePresence::MissingSkipMutation)` — neither endpoint is
///   Closed but the directed edge is absent (or one endpoint is absent
///   from the graph, which covers issues deleted post-cache-rebuild).
async fn guard_edge_exists(
    state: &ServerState,
    source_qid: &QualifiedId,
    target_qid: &QualifiedId,
) -> Result<EdgePresence, ErrorData> {
    let graph = state.cache.get_graph().await.ok_or_else(|| {
        // Caller contract violation: `probe_edge_presence` MUST route
        // to `fetch_issue_ref` when the cache is cold. Surface an
        // internal error so the bug is visible in logs rather than
        // silently bypassing the guard.
        validation_error(
            "dep_remove: internal invariant violated — guard_edge_exists \
             invoked on a cold cache; expected warm-cache + both-Local fast path",
        )
    })?;

    // Closed-endpoint check (SPEC §8.5 / bead unblock-a36). Inspect the
    // graph's issue_state snapshot BEFORE the edge lookup so a Closed
    // endpoint short-circuits to an informative error rather than
    // collapsing into the generic `missing edge` path. Source is
    // checked first — when both endpoints are Closed, the source-side
    // error is what agents typically need to act on (they own the
    // source that wants to drop the edge).
    let issue_state = graph.issue_state();
    if issue_state
        .get(source_qid)
        .is_some_and(|s| *s == IssueState::Closed)
    {
        return Ok(EdgePresence::EndpointClosed(source_qid.clone()));
    }
    if issue_state
        .get(target_qid)
        .is_some_and(|s| *s == IssueState::Closed)
    {
        return Ok(EdgePresence::EndpointClosed(target_qid.clone()));
    }

    let node_map = graph.node_map();
    match (node_map.get(source_qid), node_map.get(target_qid)) {
        (Some(&s_idx), Some(&t_idx)) if graph.inner_graph().contains_edge(s_idx, t_idx) => {
            Ok(EdgePresence::Present)
        }
        _ => {
            // Neither endpoint is Closed, but at least one is missing
            // from the cached graph OR the directed edge is not
            // present. Unified posture (unblock-29p.54): report
            // absence via `MissingSkipMutation` so the caller
            // early-returns `removed: false` identically to the
            // cold/cross-repo path.
            Ok(EdgePresence::MissingSkipMutation)
        }
    }
}

/// Single-issue edge-existence probe used by the cold-cache and cross-
/// repo paths: call [`GitHubApi::fetch_issue_ref`] on `source_ref` and
/// inspect its state plus its `blocked_by` list for a blocker matching
/// `target_qid`. The `FETCH_ISSUE_QUERY` trackedBy subselection carries
/// both `repository { owner { login } name }` (for cross-repo blocker
/// disambiguation, see `unblock-29p.43`) and `state` (for the
/// Closed-endpoint check added by bead `unblock-a36`) so no extra
/// round-trip is needed to classify either endpoint.
///
/// Returns:
/// - `Ok(EdgePresence::Present)` — edge confirmed present; caller
///   proceeds to mutation.
/// - `Ok(EdgePresence::EndpointClosed(qid))` — the fetched source is
///   `IssueState::Closed`, or the target matches a `blocked_by` entry
///   whose `state` is `Closed`. Source state is checked before the
///   `blocked_by` scan so a Closed source short-circuits before we
///   look at blockers; on the target side only the entry that matches
///   `target_qid` can produce the signal.
/// - `Ok(EdgePresence::MissingSkipMutation)` — source is Open and the
///   target is not in its `blocked_by`. Caller MUST NOT call the
///   mutation and instead returns `DepRemoveResult { removed: false, ... }`
///   to honour Invariant 11.
///
/// Errors propagate through `github_error_to_mcp` so upstream failures
/// (network, rate-limit, cross-repo FORBIDDEN) surface with the same
/// classification used by the `show` tool.
async fn probe_edge_via_fetch(
    state: &ServerState,
    source_ref: &IssueRef,
    source_qid: &QualifiedId,
    target_qid: &QualifiedId,
) -> Result<EdgePresence, ErrorData> {
    let issue = state
        .github
        .fetch_issue_ref(source_ref)
        .await
        .map_err(github_error_to_mcp)?;

    // Closed-source short-circuit (SPEC §8.5 / bead unblock-a36). The
    // fetched `issue.state` is authoritative — check it before scanning
    // blocked_by so a Closed source never falls through to the
    // missing-edge branch. Matches the warm-cache source-first
    // ordering in `guard_edge_exists`.
    if issue.state == IssueState::Closed {
        return Ok(EdgePresence::EndpointClosed(source_qid.clone()));
    }

    let enclosing_repo = (source_qid.owner.as_str(), source_qid.repo.as_str());
    let matched_target = issue.blocked_by.iter().find(|blocker| {
        // When the GraphQL selection omitted `repository { ... }` (e.g.
        // same-repo blocker default today), `repo_owner` / `repo_name`
        // are `None` — treat `None` as "same repo as the fetched source"
        // per `RelatedIssue` contract.
        //
        // Emit a structured debug trace whenever the None-means-same-repo
        // convention is actually invoked (either side missing). This
        // exists so ops can detect GraphQL schema drift or partial-parse
        // failures from log aggregation without requiring a code change —
        // silent misclassification of a cross-repo blocker as local would
        // otherwise cause incorrect `dep_remove` decisions (bead
        // `unblock-29p.57`).
        if blocker.repo_owner.is_none() || blocker.repo_name.is_none() {
            tracing::debug!(
                target: "dep_remove.probe",
                issue_number = blocker.number,
                enclosing_owner = %enclosing_repo.0,
                enclosing_repo = %enclosing_repo.1,
                "trackedBy entry missing repo identity — applying None-means-same-repo convention"
            );
        }
        let owner = blocker
            .repo_owner
            .as_deref()
            .unwrap_or(source_qid.owner.as_str());
        let repo = blocker
            .repo_name
            .as_deref()
            .unwrap_or(source_qid.repo.as_str());
        owner == target_qid.owner && repo == target_qid.repo && blocker.number == target_qid.number
    });

    match matched_target {
        Some(blocker) if blocker.state == IssueState::Closed => {
            // Closed-target signal (SPEC §8.5 / bead unblock-a36). The
            // edge DOES still exist in GitHub's trackedBy projection (a
            // Closed issue can legitimately appear in `blocked_by` when
            // the dependency was never cleaned up) but we refuse the
            // mutation so the caller reopens the target or accepts the
            // dangling edge — uniform with the warm-cache path.
            Ok(EdgePresence::EndpointClosed(target_qid.clone()))
        }
        Some(_) => Ok(EdgePresence::Present),
        None => Ok(EdgePresence::MissingSkipMutation),
    }
}

/// Pre-mutation edge-existence probe. Honours spec §14 Invariant 11 on
/// every path (see module-level docs for the three-outcome decision
/// tree). Uniform missing-edge posture per `unblock-29p.54`: both
/// branches surface `EdgePresence::MissingSkipMutation` so the caller
/// early-returns `removed: false` on the wire regardless of
/// cache/cross-repo state. Both branches also surface
/// `EdgePresence::EndpointClosed(qid)` uniformly when either endpoint
/// is Closed (added by bead `unblock-a36`).
///
/// - Warm cache AND both endpoints `Local` — cache-lookup fast path
///   via [`guard_edge_exists`]. Closed endpoint →
///   `EdgePresence::EndpointClosed`. Missing edge →
///   `EdgePresence::MissingSkipMutation`.
/// - Cold cache OR at least one endpoint cross-repo — single-issue
///   GraphQL probe via [`probe_edge_via_fetch`]. Closed endpoint →
///   `EdgePresence::EndpointClosed`. Missing edge →
///   `EdgePresence::MissingSkipMutation`.
async fn probe_edge_presence(
    state: &ServerState,
    source_ref: &IssueRef,
    target_ref: &IssueRef,
    source_qid: &QualifiedId,
    target_qid: &QualifiedId,
) -> Result<EdgePresence, ErrorData> {
    let is_both_local = matches!(
        (source_ref, target_ref),
        (IssueRef::Local(_), IssueRef::Local(_))
    );
    let is_cache_warm = state.cache.is_fresh().await;

    if is_both_local && is_cache_warm {
        guard_edge_exists(state, source_qid, target_qid).await
    } else {
        probe_edge_via_fetch(state, source_ref, source_qid, target_qid).await
    }
}

/// Post-mutation source re-evaluation: read the freshly rebuilt cache,
/// locate the source issue, and trigger the best-effort Status=ready
/// update when the source has zero open blockers.
///
/// Only invoked when the source is local to the configured project
/// (cross-repo sources are outside the cache and outside the Projects
/// V2 scope; spec §5.6 footnote).
///
/// Returns a 503-style [`ErrorData`] when the cache rebuild failed and
/// left the cache empty — the mutation landed on GitHub but the handler
/// cannot compute `has_open_blockers` locally. Matches the R3 posture
/// in `reopen`.
async fn reevaluate_source_after_remove(
    state: &ServerState,
    source_number: u64,
    source_qid: &QualifiedId,
    target_qid: &QualifiedId,
) -> Result<(), ErrorData> {
    let graph_arc = state.cache.get_graph().await;
    let issues_arc = state.cache.get_issues().await;

    let (Some(graph), Some(issues)) = (graph_arc, issues_arc) else {
        warn!(
            source_number,
            "Cache empty after dep_remove — rebuild failed; caller must re-run `show` to observe final status"
        );
        // R3: surface a 503-class error so MCP clients see INTERNAL_ERROR
        // and can retry or fall back to a `show` call. The mutation is
        // durable on GitHub regardless.
        return Err(github_error_to_mcp(
            unblock_github::errors::GitHubApiSnafu {
                status: 503_u16,
                message: format!(
                    "Blocking edge removed between {source_qid} and {target_qid}, but cache rebuild failed — please re-run `show` to observe the final blocked status"
                ),
            }
            .build(),
        ));
    };

    // Locate the source in the rebuilt cache. The mutation landed, so
    // the graph should reflect the edge removal. A missing source here
    // typically means the issue was closed or otherwise dropped from
    // the Open-issues graph between the mutation and the rebuild — log
    // a warning and skip the Status update.
    let Some(source_issue) = issues
        .iter()
        .find(|i| i.number == source_number && i.qualified_id == *source_qid)
    else {
        warn!(
            source_number,
            "Source issue not present in rebuilt cache after dep_remove — skipping Status update"
        );
        return Ok(());
    };

    if has_open_blockers(source_issue, graph.as_ref()) {
        // Spec §8.5 step 5 is silent about the non-zero-blockers case.
        // Leave Status alone so we do not clobber an InProgress / Ready
        // set by another path. Source stays Blocked (which is what it
        // was before).
        tracing::debug!(
            source_number,
            "Source still has open blockers after dep_remove — Status left untouched per spec §8.5"
        );
    } else {
        // Spec §8.5 step 5: zero open blockers → Status → ready.
        // Best-effort — never surface failures to the caller.
        update_status_to_ready(state.github.as_ref(), &source_issue.node_id).await;
    }
    Ok(())
}

/// Best-effort Projects V2 Status=ready update for the source issue.
///
/// Mirrors the close/claim/depends/reopen field-update ladder. Each
/// level is defensive: a missing field cache, an unresolved project, or
/// an absent project item all degrade to a `tracing::warn!` / `debug!`
/// rather than an error — the `removeIssueDependency` mutation has
/// already succeeded server-side, and the caller can observe the final
/// Status via a follow-up `show` if the Projects V2 integration is
/// misconfigured.
///
/// **R4 note:** fifth copy of the ladder. Extraction is tracked by
/// `unblock-29p.24` — do NOT extract here.
async fn update_status_to_ready(client: &dyn GitHubApi, issue_node_id: &str) {
    // TODO(unblock-29p.24): Extract shared project-field update helper to
    // deduplicate this if-let ladder across close, claim, depends,
    // reopen, and now dep_remove. Not in scope for this bead.
    let Some(field_ids) = client.field_ids().await else {
        tracing::debug!(
            "No field IDs cached — run setup first to enable project Status updates after dep_remove"
        );
        return;
    };

    let project_info = match client.resolve_project_info().await {
        Ok(info) => info,
        Err(err) => {
            warn!(
                error = %err,
                "Failed to resolve project info — source issue Status field will not be set after dep_remove"
            );
            return;
        }
    };

    let item_id = match client
        .get_project_item_id(issue_node_id, &project_info.id)
        .await
    {
        Ok(id) => id,
        Err(err) => {
            warn!(
                error = %err,
                "Failed to get project item ID for source issue — Status field will not be set after dep_remove"
            );
            return;
        }
    };

    let Some(option_id) = field_ids.status.options.get("ready") else {
        warn!(
            "Projects V2 Status field has no `ready` option — skipping Status update after dep_remove"
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
        warn!(error = %err, "Failed to set Status=ready on source issue after dep_remove");
    }
}

/// Execute the `dep_remove` tool handler.
///
/// See the module-level docs for the full spec contract. Flow outline
/// (mirrors spec §8.5 with the Invariant 11 tightening from
/// `unblock-29p.43` and the Closed-endpoint UX from bead
/// `unblock-a36`):
///
/// 1. Parse and normalize both references against the configured repo.
/// 2. Reject `source == target` defensively on resolved
///    [`QualifiedId`]s.
/// 3. Pre-mutation edge-existence probe with three outcomes:
///    - `Present` — proceed to step 4.
///    - `MissingSkipMutation` — early-return
///      `DepRemoveResult { removed: false, ... }` without calling the
///      mutation on ALL paths (unified by `unblock-29p.54`).
///    - `EndpointClosed(qid)` — surface `DomainError::EndpointClosed`
///      (HTTP 409 → MCP `INVALID_PARAMS`) naming the Closed endpoint
///      (added by bead `unblock-a36`).
///
///    Cache-mode branching within step 3:
///    - Warm cache AND both endpoints `Local` — in-memory graph
///      lookup (fast path, 0 extra RTT).
///    - Cold cache OR cross-repo endpoints — single-issue
///      [`GitHubApi::fetch_issue_ref`] probe (1 extra RTT).
/// 4. Run [`GitHubApi::remove_blocked_by_refs`] inside
///    `execute_write_tool`.
/// 5. Re-evaluate blockers on the source via `has_open_blockers` and,
///    when the source is `Local` AND newly has zero open blockers, fire
///    `update_status_to_ready` best-effort.
/// 6. Return [`DepRemoveResult`] with `removed = true`.
///
/// # Errors
///
/// Returns [`ErrorData`] in the following cases:
/// - `source` or `target` fails to parse as an [`IssueRef`] →
///   `INVALID_PARAMS`.
/// - `source == target` on resolved `QualifiedId`s → `INVALID_PARAMS`
///   with a `Validation` message.
/// - Either endpoint is observed as Closed during the pre-mutation
///   probe → `INVALID_PARAMS` carrying
///   [`DomainError::EndpointClosed`][`unblock_core::errors::DomainError::EndpointClosed`]
///   (HTTP 409 status internally; MCP maps 400/403/404/409/412/422 →
///   `INVALID_PARAMS`). Added by bead `unblock-a36`.
/// - Cold cache / cross-repo single-issue probe fails (network, 403,
///   GraphQL error) → propagated via `github_error_to_mcp`.
/// - `remove_blocked_by_refs` fails → mapped via `github_error_to_mcp`
///   (e.g. 404 maps to `INVALID_PARAMS`).
/// - Cache rebuild fails and leaves the cache empty AND the source is
///   `Local` → a 503-class error is surfaced so the caller re-runs
///   `show` to observe the final state (R3).
///
/// Missing-edge absence (with both endpoints Open) is NOT an error on
/// any path: the handler returns `Ok(DepRemoveResult { removed: false, ... })`
/// uniformly (warm-local, warm-cross-repo, cold-local,
/// cold-cross-repo) per `unblock-29p.54`.
///
/// [`GitHubApi::remove_blocked_by_refs`]: unblock_github::GitHubApi::remove_blocked_by_refs
/// [`GitHubApi::fetch_issue_ref`]: unblock_github::GitHubApi::fetch_issue_ref
#[instrument(
    skip(state, params),
    name = "handle_dep_remove",
    fields(
        agent.kind = state.agent_kind_str(),
        source = %params.source,
        target = %params.target,
    ),
)]
pub async fn handle_dep_remove(
    state: &ServerState,
    params: DepRemoveParams,
) -> Result<DepRemoveResult, ErrorData> {
    let client = Arc::clone(&state.github);
    let owner = client.owner().to_owned();
    let repo = client.repo().to_owned();

    info!(
        source = %params.source,
        target = %params.target,
        "DepRemove tool invoked"
    );

    // Step 1: parse + normalize both refs. Normalization collapses a
    // `CrossRepo { owner, repo, .. }` pointing at the configured repo
    // back to `Local`, so all downstream dispatch (edge check, Status
    // update scope) treats aliased and canonical local forms identically.
    let source_raw = parse_ref(&params.source)?;
    let target_raw = parse_ref(&params.target)?;
    let source_ref = source_raw.normalize(&owner, &repo);
    let target_ref = target_raw.normalize(&owner, &repo);

    // Step 2: defensive source != target check on resolved
    // QualifiedIds. Spec §8.5 does not mandate this — we mirror §8.4 so
    // the two mutation tools present a symmetric surface. Compare on the
    // fully-qualified id (resolved against the configured repo) so that
    // `"42"` and `"#42"` collapse to the same identity, as in `depends`.
    let source_qid: QualifiedId = source_ref.resolve(&owner, &repo);
    let target_qid: QualifiedId = target_ref.resolve(&owner, &repo);
    if source_qid == target_qid {
        return Err(validation_error(format!(
            "dep_remove: source and target must differ (both resolved to {source_qid})"
        )));
    }

    // Step 3: pre-mutation edge-existence probe. Honours Invariant 11
    // (§14 "Validation before mutation") on every path with a uniform
    // three-outcome posture (`unblock-29p.54` for the
    // Missing/Present pair; bead `unblock-a36` for the
    // `EndpointClosed` arm): both the warm+both-Local in-memory lookup
    // AND the cold/cross-repo single-issue GraphQL probe return the
    // same `EdgePresence` variant given the same observed state.
    // `MissingSkipMutation` produces a truthful `removed: false` on
    // the wire; `EndpointClosed(qid)` produces
    // `DomainError::EndpointClosed` (HTTP 409 → MCP INVALID_PARAMS)
    // naming the Closed endpoint. The cold/cross-repo probe uses
    // `fetch_issue_ref` on the source and scans its trackedBy list
    // for the target; the query carries repository identity per
    // unblock-29p.43 AND per-entry `state` for the Closed-endpoint
    // check (see `FETCH_ISSUE_QUERY` in graphql.rs).
    match probe_edge_presence(state, &source_ref, &target_ref, &source_qid, &target_qid).await? {
        EdgePresence::Present => {
            // Fall through to the mutation in step 4.
        }
        EdgePresence::MissingSkipMutation => {
            // Pre-mutation probe proved the edge absent. Invariant 11
            // forbids mutating after validation failed, so we SKIP
            // `remove_blocked_by_refs` entirely and surface a truthful
            // `removed: false` on the wire. Unified across all paths
            // (warm-local, warm-cross-repo, cold-local,
            // cold-cross-repo) per `unblock-29p.54`.
            let source_rendered = source_ref.to_string();
            let target_rendered = target_ref.to_string();
            info!(
                source = %source_rendered,
                target = %target_rendered,
                "dep_remove: edge not present per single-issue probe — skipping mutation (removed=false)"
            );
            return Ok(DepRemoveResult {
                removed: false,
                source: source_rendered.clone(),
                target: target_rendered.clone(),
                message: format!(
                    "No blocking edge to remove: {source_rendered} is not currently blocked by {target_rendered}"
                ),
            });
        }
        EdgePresence::EndpointClosed(closed_qid) => {
            // One endpoint is Closed — refuse the mutation and surface
            // `DomainError::EndpointClosed` so the agent can either
            // reopen the issue first (via the `reopen` tool) or accept
            // the dangling edge. Spec §8.5 / bead `unblock-a36`.
            //
            // Uniform posture across the warm-local, warm-cross-repo,
            // cold-local, and cold-cross-repo paths: the probe layer
            // has already selected the correct Closed endpoint's
            // `QualifiedId`, so the handler does not re-derive which
            // side is closed — it just forwards the signal.
            info!(
                source = %source_ref,
                target = %target_ref,
                closed_endpoint = %closed_qid,
                "dep_remove: refusing mutation — endpoint is Closed (surface via DomainError::EndpointClosed)"
            );
            return Err(github_error_to_mcp(unblock_github::errors::Error::from(
                EndpointClosedSnafu { qid: closed_qid }.build(),
            )));
        }
    }

    // Step 4: run the mutation inside execute_write_tool so the cache
    // is invalidated and rebuilt atomically on success. Clone the refs
    // into the closure — execute_write_tool takes ownership.
    {
        let client = Arc::clone(&client);
        let source_ref = source_ref.clone();
        let target_ref = target_ref.clone();
        execute_write_tool(state, || async move {
            client
                .remove_blocked_by_refs(&source_ref, &target_ref)
                .await
        })
        .await?;
    }

    // Step 5: re-evaluate the source's blockers against the freshly
    // rebuilt cache. Only applies when source is Local — cross-repo
    // sources are not in the configured-project cache, and the Projects
    // V2 field update ladder is scoped to the configured project (spec
    // §5.6 footnote).
    if let IssueRef::Local(source_number) = &source_ref {
        reevaluate_source_after_remove(state, *source_number, &source_qid, &target_qid).await?;
    } else {
        tracing::debug!(
            source = %source_ref,
            "Cross-repo source: skipping Projects V2 Status update after dep_remove (source is outside the configured project)."
        );
    }

    // Step 6: render canonical forms of the normalized refs. Matches the
    // posture of the `depends` handler (`server.rs:1380-1381`).
    let source_rendered = source_ref.to_string();
    let target_rendered = target_ref.to_string();

    Ok(DepRemoveResult {
        removed: true,
        source: source_rendered.clone(),
        target: target_rendered.clone(),
        message: format!(
            "Removed blocking edge: {source_rendered} is no longer blocked by {target_rendered}"
        ),
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

    fn dep_remove_issue(number: u64, status: Status, state: IssueState) -> Issue {
        Issue {
            qualified_id: qid(number),
            number,
            node_id: format!("I_{number}"),
            title: format!("DepRemove fixture #{number}"),
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

    // ── parse_ref ────────────────────────────────────────────────────

    #[test]
    fn parse_ref_accepts_bare_local_number() {
        let r = parse_ref("42").expect("bare local number should parse");
        assert_eq!(r, IssueRef::Local(42));
    }

    #[test]
    fn parse_ref_accepts_hash_prefixed_local() {
        let r = parse_ref("#7").expect("hash-prefixed local should parse");
        assert_eq!(r, IssueRef::Local(7));
    }

    #[test]
    fn parse_ref_accepts_cross_repo() {
        let r = parse_ref("acme/widgets#99").expect("cross-repo reference should parse");
        assert_eq!(
            r,
            IssueRef::CrossRepo {
                owner: "acme".to_owned(),
                repo: "widgets".to_owned(),
                number: 99,
            }
        );
    }

    #[test]
    fn parse_ref_rejects_garbage_surfaces_invalid_issue_ref() {
        // Per SPEC §11.1 / plan Task 02.02 "Error-side wiring", a
        // malformed IssueRef at the tool boundary MUST surface as
        // `DomainError::InvalidIssueRef { input }` lifted through
        // `github_error_to_mcp` — i.e. MCP `INVALID_PARAMS` with the
        // raw input preserved in the message. The previous `field`
        // tag (`source`/`target`) was dropped with unblock-6xj because
        // the spec variant only carries `input`; the position is
        // implicit in the caller's parse ordering.
        let err = parse_ref("not-a-ref").expect_err("garbage should not parse");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(
            err.message.contains("not-a-ref"),
            "error should include the raw value: {}",
            err.message,
        );
    }

    // ── has_open_blockers ────────────────────────────────────────────

    #[test]
    fn has_open_blockers_false_when_issue_absent_from_graph() {
        let g = DependencyGraph::build(&[], &[]);
        let orphan = dep_remove_issue(42, Status::Ready, IssueState::Open);
        assert!(!has_open_blockers(&orphan, &g));
    }

    #[test]
    fn has_open_blockers_true_for_downstream_of_open_upstream() {
        let upstream = dep_remove_issue(1, Status::Ready, IssueState::Open);
        let downstream = dep_remove_issue(2, Status::Ready, IssueState::Open);
        let issues = vec![upstream.clone(), downstream.clone()];
        // Edge convention: downstream (2) source -> upstream (1) target
        // means "issue #2 is blocked by issue #1".
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
        let upstream = dep_remove_issue(1, Status::Closed, IssueState::Closed);
        let downstream = dep_remove_issue(2, Status::Ready, IssueState::Open);
        let issues = vec![upstream.clone(), downstream.clone()];
        let edges = vec![BlockingEdge {
            source: qid(2),
            target: qid(1),
        }];
        let g = DependencyGraph::build(&issues, &edges);
        // Upstream blocker is Closed → downstream has no OPEN blockers,
        // so `has_open_blockers` returns `false`. After bead
        // `unblock-a36` widened `fetch_graph_data` to include CLOSED
        // issues, the blocker is now explicitly carried in the graph's
        // `issue_state` snapshot as `IssueState::Closed` — the helper
        // correctly recognises it as not-open rather than relying on
        // the absent-from-graph shortcut.
        assert!(!has_open_blockers(&downstream, &g));
    }

    // ── DepRemoveResult serialization shape ──────────────────────────

    #[test]
    fn dep_remove_result_serializes_expected_shape() {
        let res = DepRemoveResult {
            removed: true,
            source: "#42".to_owned(),
            target: "#99".to_owned(),
            message: "Removed blocking edge: #42 is no longer blocked by #99".to_owned(),
        };
        let json = serde_json::to_value(&res).expect("DepRemoveResult should serialize");
        assert_eq!(json["removed"], true);
        assert_eq!(json["source"], "#42");
        assert_eq!(json["target"], "#99");
        assert!(
            json["message"]
                .as_str()
                .is_some_and(|m| m.contains("#42") && m.contains("#99")),
            "message should mention both refs: {}",
            json["message"],
        );
    }

    #[test]
    fn dep_remove_result_cross_repo_rendering_round_trips() {
        // The handler renders source/target as IssueRef::Display, so
        // cross-repo results should use the `owner/repo#n` canonical
        // form — not the raw input string.
        let res = DepRemoveResult {
            removed: true,
            source: "acme/widgets#42".to_owned(),
            target: "other/repo#9".to_owned(),
            message: "Removed blocking edge: acme/widgets#42 is no longer blocked by other/repo#9"
                .to_owned(),
        };
        let json = serde_json::to_value(&res).expect("serialize");
        assert_eq!(json["source"], "acme/widgets#42");
        assert_eq!(json["target"], "other/repo#9");
    }
}
