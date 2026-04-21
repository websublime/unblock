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
//!    ("Validation before mutation") on ALL paths. Branching, per
//!    `unblock-29p.43`:
//!    - **Warm cache AND both endpoints `Local`** — fast path: look up
//!      the edge in the in-memory graph. When absent, surface
//!      `INVALID_PARAMS` (spec §8.5 literal behaviour). No extra RTT.
//!    - **Cold cache OR at least one endpoint cross-repo** — fetch the
//!      source issue via [`GitHubApi::fetch_issue_ref`] (1 GraphQL RTT)
//!      and scan its `trackedByIssues` list for the target. The
//!      `FETCH_ISSUE_QUERY` trackedBy subselection carries
//!      `repository { owner { login } name }` (see `graphql.rs`) so
//!      cross-repo blockers are correctly disambiguated. When the
//!      target is NOT present, early-return `DepRemoveResult { removed:
//!      false, ... }` WITHOUT issuing the mutation — honours Invariant
//!      11 (validated-absent + mutation would contradict) and saves one
//!      wasted RTT.
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
//!    proved the edge did not exist (cold-cache / cross-repo path
//!    only; warm-local-local surfaces `INVALID_PARAMS` instead — see
//!    step 3). The source/target are rendered in canonical
//!    [`IssueRef`] form and `message` documents what happened.
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
//! ## R6 caveat — closed-blocker blind spot
//!
//! [`GitHubApi::fetch_graph_data`] returns OPEN issues only today
//! (tracked by bead `unblock-a36`), so a source whose only remaining
//! blocker is already Closed looks unblocked *before* `dep_remove` runs;
//! this tool will then classify as ready and flip Status accordingly.
//! Same quirk as `reopen` — not a new divergence.
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
use unblock_core::errors::{InvalidIssueRefSnafu, ValidationSnafu};
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
    /// `true` when the blocking edge existed and was removed;
    /// `false` when the cold-cache / cross-repo single-issue edge-
    /// existence check proved the edge did not exist and the
    /// mutation was skipped (no-op early return). The warm + both-
    /// `Local` path surfaces a `Validation` error instead of
    /// `removed = false` — see the module-level docs.
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
/// `INVALID_PARAMS` on the warm-cache fast path.
#[derive(Debug, PartialEq, Eq)]
enum EdgePresence {
    /// The edge was confirmed present — proceed to
    /// `remove_blocked_by_refs`.
    Present,
    /// The edge was proved absent via the single-issue probe
    /// (cold-cache / cross-repo path). Return
    /// `DepRemoveResult { removed: false, ... }` WITHOUT calling the
    /// mutation (spec §14 Invariant 11 — validation before mutation;
    /// calling the mutation after proving absence would contradict
    /// the validation outcome).
    MissingSkipMutation,
}

/// Warm-cache edge-existence guard: reject when the edge from
/// `source_qid` → `target_qid` is absent from the currently cached
/// graph. Only invoked when the cache is warm AND both endpoints are
/// `Local` (see [`probe_edge_presence`]).
///
/// Returns `Ok(EdgePresence::Present)` when the edge exists, and a
/// `Validation` [`ErrorData`] when the edge is absent — preserves the
/// existing spec §8.5 "Validate edge exists in graph" literal behaviour
/// (caller sees `INVALID_PARAMS`, no `removed: false` wire signal on the
/// warm-local-local path).
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
    let node_map = graph.node_map();
    match (node_map.get(source_qid), node_map.get(target_qid)) {
        (Some(&s_idx), Some(&t_idx)) if graph.inner_graph().contains_edge(s_idx, t_idx) => {
            Ok(EdgePresence::Present)
        }
        _ => {
            // At least one endpoint is missing from the cached graph OR
            // the directed edge is not present. `fetch_graph_data`
            // returns OPEN issues only (R6), so this branch also fires
            // when either side is Closed in the current cache view.
            // Matches the spec §8.5 contract: the edge, if any, is not
            // in the active graph.
            Err(validation_error(format!(
                "dep_remove: no blocking edge exists from {source_qid} to {target_qid}"
            )))
        }
    }
}

/// Single-issue edge-existence probe used by the cold-cache and cross-
/// repo paths: call [`GitHubApi::fetch_issue_ref`] on `source_ref` and
/// scan the returned `blocked_by` list for a blocker matching
/// `target_qid`. The `FETCH_ISSUE_QUERY` trackedBy subselection carries
/// `repository { owner { login } name }` so a cross-repo blocker is
/// disambiguated from a same-repo blocker (see `unblock-29p.43`).
///
/// Returns:
/// - `Ok(EdgePresence::Present)` — edge confirmed present; caller
///   proceeds to mutation.
/// - `Ok(EdgePresence::MissingSkipMutation)` — edge proved absent;
///   caller MUST NOT call the mutation and instead returns
///   `DepRemoveResult { removed: false, ... }` to honour Invariant 11.
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
    let present = issue.blocked_by.iter().any(|blocker| {
        // When the GraphQL selection omitted `repository { ... }` (e.g.
        // same-repo blocker default today), `repo_owner` / `repo_name`
        // are `None` — treat `None` as "same repo as the fetched source"
        // per `RelatedIssue` contract.
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
    if present {
        Ok(EdgePresence::Present)
    } else {
        Ok(EdgePresence::MissingSkipMutation)
    }
}

/// Pre-mutation edge-existence probe. Honours spec §14 Invariant 11 on
/// every path (see module-level docs for the decision tree):
///
/// - Warm cache AND both endpoints `Local` — cache-lookup fast path
///   via [`guard_edge_exists`]. Missing edge → `INVALID_PARAMS`.
/// - Cold cache OR at least one endpoint cross-repo — single-issue
///   GraphQL probe via [`probe_edge_via_fetch`]. Missing edge →
///   `EdgePresence::MissingSkipMutation` (caller early-returns
///   `removed: false`).
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
/// `unblock-29p.43`):
///
/// 1. Parse and normalize both references against the configured repo.
/// 2. Reject `source == target` defensively on resolved
///    [`QualifiedId`]s.
/// 3. Pre-mutation edge-existence probe:
///    - Warm cache AND both endpoints `Local` — in-memory graph
///      lookup (fast path, 0 extra RTT). Missing edge →
///      `INVALID_PARAMS`.
///    - Cold cache OR cross-repo endpoints — single-issue
///      [`GitHubApi::fetch_issue_ref`] probe (1 extra RTT). Missing
///      edge → early-return `DepRemoveResult { removed: false, ... }`
///      without calling the mutation.
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
/// - Warm cache AND both `Local` AND edge absent from the cached graph
///   → `INVALID_PARAMS` with a `Validation` message (spec §8.5 literal
///   behaviour preserved).
/// - Cold cache / cross-repo single-issue probe fails (network, 403,
///   GraphQL error) → propagated via `github_error_to_mcp`.
/// - `remove_blocked_by_refs` fails → mapped via `github_error_to_mcp`
///   (e.g. 404 maps to `INVALID_PARAMS`).
/// - Cache rebuild fails and leaves the cache empty AND the source is
///   `Local` → a 503-class error is surfaced so the caller re-runs
///   `show` to observe the final state (R3).
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
    // (§14 "Validation before mutation") on every path — warm cache +
    // both-Local uses the in-memory cache lookup (preserves the spec
    // §8.5 INVALID_PARAMS behaviour on a missing edge); cold cache OR
    // cross-repo endpoints use a single-issue GraphQL probe via
    // `fetch_issue_ref` on the source and scans its trackedBy list for
    // the target (the query carries repository identity per
    // unblock-29p.43).
    match probe_edge_presence(state, &source_ref, &target_ref, &source_qid, &target_qid).await? {
        EdgePresence::Present => {
            // Fall through to the mutation in step 4.
        }
        EdgePresence::MissingSkipMutation => {
            // Cold-cache / cross-repo probe proved the edge absent.
            // Invariant 11 forbids mutating after validation failed, so
            // we SKIP `remove_blocked_by_refs` entirely and surface a
            // truthful `removed: false` on the wire. The warm + both-
            // Local fast path never reaches this arm — it surfaces
            // `INVALID_PARAMS` via `guard_edge_exists` instead.
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
        // Upstream blocker is Closed → downstream looks unblocked. This
        // is the R6 blind spot documented at the module level.
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
