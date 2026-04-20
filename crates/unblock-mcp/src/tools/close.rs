//! Close tool — closes an issue and triggers cascade unblock.
//!
//! Validates the issue is open, closes it via the GitHub API, updates Projects V2
//! fields (Status=Done), rebuilds the cache, then computes the unblock cascade.
//! For each newly unblocked issue, updates its Projects V2 fields
//! (Status=Backlog if not already `InProgress`) and posts an unblock comment.
//!
//! Cross-repo dependents (SPEC §11.4): when the cascade returned by
//! [`compute_unblock_cascade`][`unblock_core::graph::DependencyGraph::compute_unblock_cascade`]
//! touches a `QualifiedId` whose `(owner, repo)` differs from the configured
//! repo, the dependent is STILL cascade-updated (same Status / comment path) —
//! only the response shape differs. The bare-`u64` [`CloseResult::unblocked`]
//! vector is scoped to the configured repo; cross-repo dependents are surfaced
//! in [`CloseResult::cross_repo_refs`] (`Some` iff at least one cross-repo
//! dependent participated in the cascade, per SPEC §14 Invariant 14(b)).

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

// TODO(unblock-45a.12): Extend close-tool integration coverage for the two
// paths the §11.4 cross-repo suite did not exercise:
//
//   * already-closed — Phase 1 `IssueClosedSnafu` short-circuit when the
//     fetched issue's `state == IssueState::Closed` (server.rs §`close`
//     handler, step 1). `compute_unblock_cascade` is unit-tested at the
//     graph-engine level but the tool boundary is not.
//   * co-blocking — end-to-end assertion that a dependent with at least
//     one remaining open blocker is NOT emitted in `unblocked` /
//     `cross_repo_refs`. The graph engine's
//     `cascade_co_blockers_returns_empty_when_other_open` covers the
//     topology, but the MCP response projection through the tool is not.
//
// Already covered by the §11.4 suite in `tests/integration.rs`
// (`close_no_cross_repo_dependents_cross_repo_refs_is_none`,
// `close_cross_repo_dependent_populates_cross_repo_refs`,
// `close_single_cross_repo_dependent_uses_singular_summary`,
// `close_cross_repo_add_comment_ref_failure_warns_and_continues_cascade`):
// the None branch, plural-dependents branch, singular-summary branch,
// and the best-effort `add_comment_ref` failure path.
