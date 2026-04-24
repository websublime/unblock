//! Close tool — closes an issue and triggers cascade unblock.
//!
//! ## Four-phase execution (SPEC §8.2 + §3.4 Critical, GAP-15 remediation)
//!
//! The close handler runs four explicit phases — ordering is
//! correctness-critical and MUST NOT be reordered:
//!
//! 1. **Phase 0 — PRE-close cascade capture.** Ensure the graph is built
//!    (cold-cache path calls
//!    [`rebuild_cache`][`crate::tools::rebuild_cache`] to issue one
//!    `fetch_graph_data` round-trip), then call
//!    [`compute_unblock_cascade`][`unblock_core::graph::DependencyGraph::compute_unblock_cascade`]
//!    while the closed issue is still an OPEN node in the graph. The
//!    resulting `Vec<QualifiedId>` is captured in a handler-local
//!    binding and used authoritatively by Phases 2 and 3 — it is NOT
//!    re-read from the post-close cache. This ordering is mandatory
//!    per SPEC §3.4 Critical and §8.2 step 2 ("Pre-close cascade MUST
//!    be captured before the mutation"). After bead `unblock-a36`
//!    widened `fetch_graph_data` to `states: [OPEN, CLOSED]`, the
//!    just-closed issue would still appear in the POST-close rebuilt
//!    `node_map` (as `IssueState::Closed`) and the `blocker_qid ==
//!    closed_id` special-case at
//!    `unblock-core/src/graph.rs:312-314` would still allow cascade
//!    participants to resolve — so the "`closed_id` absent from
//!    `node_map`" short-circuit no longer trips from the close path.
//!    PRE-close ordering is nevertheless MANDATORY because a
//!    POST-close rebuild introduces subtle behaviour shifts that the
//!    cascade walk is NOT designed to absorb: (a) already-closed
//!    dependents would show up in the `Incoming` traversal with
//!    `issue_state == Closed`, and `compute_unblock_cascade` does not
//!    filter them out on that axis (contrast with the pseudocode in
//!    SPEC §3.4 "IF `dependent_issue.state` == Closed CONTINUE"); and
//!    (b) any race where a concurrent mutation alters a blocker's
//!    state between close-mutation and rebuild would silently shift
//!    the cascade set. Capturing PRE-close freezes the snapshot
//!    against both risks. The defensive `Vec::new()` short-circuit
//!    at `unblock-core/src/graph.rs:289-291` stays as-is (it is
//!    still correct for create-then-immediately-close races where
//!    `closed_id` legitimately is not yet in the graph).
//! 2. **Phase 1 — MUTATION.** `execute_write_tool` runs `fetch_issue`,
//!    state validation, `close_issue`, the Projects V2 `Status → closed`
//!    field ladder on the closed issue, and a cache rebuild. The close
//!    mutation is durable on GitHub regardless of the rebuild outcome.
//! 3. **Phase 2 — CASCADE FIELD-UPDATE LOOP.** Iterate the cascade
//!    list captured in Phase 0 (not the post-close cache). For each
//!    dependent, dispatch side effects via the `*_ref` primitives —
//!    [`add_comment_ref`][`unblock_github::GitHubApi::add_comment_ref`]
//!    (unblock comment) and
//!    [`fetch_issue_ref`][`unblock_github::GitHubApi::fetch_issue_ref`]
//!    followed by `update_field` (Projects V2 Status → `ready` if the
//!    dependent is not already `InProgress`) — so cross-repo dependents
//!    route to their own `(owner, repo)` rather than silently
//!    retargeting the configured repo. Per-dependent failures are
//!    logged and the cascade continues (best-effort per SPEC §8.2
//!    step 6 / §5.6 `close` row).
//! 4. **Phase 3 — RESPONSE PROJECTION.** Partition the Phase-0 cascade
//!    into `unblocked: Vec<u64>` (local dependents) plus
//!    `cross_repo_refs: Option<CrossRepoRefs>` (cross-repo dependents,
//!    surfaced per SPEC §11.4) via the shared `project_cascade` and
//!    `build_cross_repo_refs_with_summary` helpers in
//!    `crate::tools::cross_repo`.
//!
//! Cross-repo dependents (SPEC §11.4): when the cascade touches a
//! `QualifiedId` whose `(owner, repo)` differs from the configured
//! repo, the dependent is STILL cascade-updated (same Status / comment
//! path) — only the response shape differs. The bare-`u64`
//! [`CloseResult::unblocked`] vector is scoped to the configured repo;
//! cross-repo dependents are surfaced in
//! [`CloseResult::cross_repo_refs`] (`Some` iff at least one cross-repo
//! dependent participated in the cascade, per SPEC §14 Invariant
//! 14(b)).
//!
//! ## R3 caveat — post-rebuild Status reconciliation failure
//!
//! Under PRE-close ordering the Phase-0 cascade list is captured
//! before the mutation, so a post-close rebuild failure no longer
//! invalidates the cascade list — the response envelope stays
//! authoritative even when
//! [`GraphCache::get_graph`][`unblock_core::cache::GraphCache::get_graph`]
//! returns `None` after `execute_write_tool`. The close mutation is
//! durable on GitHub and the Phase-2 cascade field-updates are applied
//! best-effort regardless of the rebuild outcome.
//!
//! What a rebuild failure DOES break is the step 8
//! `update_status_fields` reconciliation (SPEC §8.2 step 8) —
//! cross-checking Status fields for issues NOT already handled by the
//! Phase 2 cascade loop (e.g. issues whose blocker status changed but
//! that were not direct dependents of the closed issue). This step
//! requires the rebuilt graph and cannot run against an empty cache.
//! When the rebuild fails, the handler surfaces a 503-class
//! [`GitHubApi`](unblock_github::errors::Error::GitHubApi) error
//! with a message instructing the caller to re-run `show` so the
//! Status fan-out is reconciled on the next read. The cascade list
//! in the response remains authoritative; the error signals only
//! that the Status-field reconciliation could not complete.
//! Preserves spec §14 invariants 8 (no write leaves cache or Status
//! fields inconsistent) and 13 (Status field values match graph
//! computation — we refuse to pretend reconciliation succeeded when
//! we cannot consult the graph).
//!
//! A separate 503-class error is surfaced when Phase 0 cold-cache
//! prime fails (the `fetch_graph_data` call inside `rebuild_cache`
//! errored and the cache stayed empty). That branch is distinct from
//! the R3 path — it fires *before* the close mutation is attempted,
//! and the message instructs the caller to retry or run `prime`
//! first. The close is NOT attempted on an empty graph.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unblock_core::types::CrossRepoRefs;

/// Input parameters for the `close` MCP tool.
///
/// Only `id` is required. An optional `reason` can be provided, which is added
/// as a comment on the issue before closing.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CloseParams {
    /// Issue number to close (required).
    pub id: u64,
    /// Optional reason for closing. If provided, a comment with this text is
    /// added to the issue before it is closed.
    pub reason: Option<String>,
}

/// Result returned by the `close` MCP tool.
///
/// Contains the closed issue number and the list of issue numbers that were
/// fully unblocked by this close (the cascade).
///
/// Cross-repo cascade members (per SPEC §11.4) are surfaced separately in
/// [`Self::cross_repo_refs`] rather than being flattened into
/// [`Self::unblocked`] — bare `u64` cannot disambiguate across repositories.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CloseResult {
    /// The closed issue number.
    pub issue: u64,
    /// Issue numbers that were fully unblocked by closing this issue.
    ///
    /// Scoped to the configured repo: only dependents whose
    /// `(owner, repo) == (config.owner, config.repo)` appear here. Cross-repo
    /// dependents that were cascade-updated are surfaced in
    /// [`Self::cross_repo_refs`] per SPEC §11.4.
    ///
    /// Only includes issues where ALL blockers are now closed.
    pub unblocked: Vec<u64>,
    /// Cross-repo dependents that were cascade-updated but dropped from
    /// the bare-`u64` projection of [`Self::unblocked`], per SPEC §11.4.
    ///
    /// `Some` iff at least one cascade member lived in a different
    /// repository (i.e. its `(owner, repo)` differed from the configured
    /// repo). `None` otherwise. Elided from the JSON envelope when
    /// `None` via `#[serde(skip_serializing_if = "Option::is_none")]`.
    ///
    /// See SPEC §2.16 (shared type), §11.4 (cross-cutting contract), and
    /// §14 Invariant 14(b) (response-shape determinism — `omitted` is
    /// lexicographically sorted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_repo_refs: Option<CrossRepoRefs>,
}
