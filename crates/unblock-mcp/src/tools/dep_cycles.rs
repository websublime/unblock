//! `dep_cycles` tool — detect dependency cycles in the dependency graph.
//!
//! Per SPEC §7.7 this is a **read-only** tool that projects the output of
//! [`DependencyGraph::detect_all_cycles`] (Tarjan's SCC algorithm, see
//! SPEC §3.6) down to a JSON-friendly `Vec<Vec<u64>>` of issue numbers
//! scoped to the configured repository, plus the `cross_repo_refs`
//! envelope mandated by SPEC §11.4.
//!
//! ## Read-only — no write-tool scaffolding
//!
//! The tool does not mutate GitHub and does not invalidate the cache (see
//! SPEC §4.4 invalidation matrix: `dep_cycles = No`). The handler therefore
//! does **not** wrap the operation in the crate-internal
//! `execute_write_tool` helper and does not fire the Projects V2 Status
//! update ladder. It piggy-backs on [`crate::tools::rebuild_cache`] only
//! when the cache is stale, exactly like [`crate::tools::stats`].
//!
//! ## Cache-aware read path (SPEC §7.7 "API calls: 0 (cache hit) | 1+
//! (rebuild)")
//!
//! 1. If the cache is stale/empty, call [`crate::tools::rebuild_cache`]
//!    to warm every cached artefact with a single `fetch_graph_data()`
//!    round-trip.
//! 2. Read [`GraphCache::get_graph`] — an O(1) `Arc` clone — and compute
//!    cycles directly from the cached [`DependencyGraph`].
//! 3. If the cache is still empty after the rebuild attempt (the network
//!    call inside `rebuild_cache` failed), propagate the underlying
//!    GitHub error via `github_error_to_mcp` after one local
//!    [`fetch_graph_data`](unblock_github::GitHubApi::fetch_graph_data)
//!    retry. The spec's [`DepCyclesResult`] has no `stale` field, so
//!    surfacing the error is the only honest signal to the caller. This
//!    mirrors [`stats`](crate::tools::stats) (R6) and the
//!    [`dep_remove`](crate::tools::dep_remove) R3 posture.
//!
//! ## Cross-Repo Response Contract (SPEC §11.4)
//!
//! The graph engine's nodes are [`QualifiedId`] values (§2.1). Cycle
//! detection therefore returns `Vec<Vec<QualifiedId>>` — each inner
//! vector may contain a mix of local and cross-repo members. SPEC §7.7's
//! [`DepCyclesResult::cycles`] is `Vec<Vec<u64>>`, which cannot express
//! the `(owner, repo)` tuple of a cross-repo node. The handler therefore
//! performs an *asymmetric* projection:
//!
//! - **Local members** (those whose `(owner, repo)` matches the
//!   configured `(client.owner(), client.repo())`) are emitted as bare
//!   `u64` issue numbers inside the `cycles` vector, preserving the
//!   relative iteration order that Tarjan returned.
//! - **Cross-repo members** are stripped from the inner vector and
//!   instead accumulated into
//!   [`CrossRepoRefs::omitted`](unblock_core::types::CrossRepoRefs)
//!   using [`QualifiedId::Display`] (`"owner/repo#number"`), sorted
//!   lexicographically to preserve determinism (Invariant 14, SPEC §14).
//!
//! A cycle whose local-projection length drops below 2 after stripping
//! cross-repo members is STILL emitted as a (possibly-shorter)
//! `Vec<u64>` — the agent must know the cycle exists even if the local
//! projection is degraded. This is spelled out in SPEC §7.7 flow step 4b
//! ("the bare-`u64` vector may therefore be shorter than the true cycle
//! length") and echoed in plan Task 06.06 acceptance.
//!
//! ## Population rules (SPEC §11.4)
//!
//! [`DepCyclesResult::cross_repo_refs`] is `Some` iff at least one cycle
//! in the detected set visited a cross-repo `QualifiedId` that was not
//! emitted in the bare-`u64` projection. Otherwise it is `None` and the
//! `#[serde(skip_serializing_if = "Option::is_none")]` attribute elides
//! it from the JSON envelope entirely. The field is NEVER `Some` with an
//! empty `omitted` vector.
//!
//! De-duplication: the same cross-repo `QualifiedId` may theoretically
//! appear in two disjoint cycles. Its `QualifiedId::Display` form is
//! emitted in `omitted` exactly once after de-duplication, which matches
//! the strict reading of SPEC §11.4 ("nodes dropped from the
//! bare-`u64` projection") and preserves the lexicographic-sort
//! invariant trivially.
//!
//! ## Targeted `id` parameter (SPEC §7.7 params)
//!
//! The tool accepts `id: Option<u64>`. Per SPEC §7.7 the parameter is
//! typed as `u64`, not [`IssueRef`], so cross-repo lookups are
//! deliberately NOT supported here (consistent with [`claim`],
//! [`close`], [`reopen`] scope in §5.6). The lookup resolves the bare
//! number against `(client.owner(), client.repo())` and keeps only the
//! SCCs whose member set contains the target node. This reuses
//! [`DependencyGraph::detect_all_cycles`] directly — no new graph-engine
//! API is introduced (see bead `unblock-29p.11` scope notes).
//!
//! The cross-repo projection (above) still applies on the filtered
//! subset: a targeted query that lands on a mixed cycle returns the
//! local projection of that cycle and the cross-repo members in
//! `cross_repo_refs`.
//!
//! ## Server registration deferred to sibling bead
//!
//! The `#[tool]` registration on `UnblockServer` is **deliberately out
//! of scope** for this module — it is tracked by sibling bead
//! `unblock-29p.12`. This file exposes [`handle_dep_cycles`] and the
//! data types [`DepCyclesParams`] / [`DepCyclesResult`] so the sibling
//! bead can wire the router without touching cycle-detection logic.
//!
//! [`DependencyGraph::detect_all_cycles`]: unblock_core::graph::DependencyGraph::detect_all_cycles
//! [`GraphCache::get_graph`]: unblock_core::cache::GraphCache::get_graph
//! [`QualifiedId`]: unblock_core::types::QualifiedId
//! [`QualifiedId::Display`]: unblock_core::types::QualifiedId#impl-Display-for-QualifiedId
//! [`IssueRef`]: unblock_core::types::IssueRef
//! [`claim`]: crate::tools::claim
//! [`close`]: crate::tools::close
//! [`reopen`]: crate::tools::reopen

use rmcp::model::ErrorData;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use unblock_core::graph::DependencyGraph;
use unblock_core::types::{CrossRepoRefs, QualifiedId};

use crate::errors::github_error_to_mcp;
use crate::server::ServerState;
use crate::tools::cross_repo;

/// Input parameters for the `dep_cycles` MCP tool.
///
/// Per SPEC §7.7 the only parameter is `id: Option<u64>`. The bare-`u64`
/// form is intentional: cross-repo targeted lookups are out of scope for
/// this tool (§5.6 cross-repo scope table — read tools that accept an
/// issue number accept local numbers only). When `id` is `None`, the
/// handler returns cycles across the full configured-repo graph.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DepCyclesParams {
    /// Optional local issue number for a targeted cycle check. When
    /// present, only cycles that contain the node
    /// `(client.owner(), client.repo(), id)` are returned.
    pub id: Option<u64>,
}

/// Result returned by the `dep_cycles` MCP tool.
///
/// Per SPEC §7.7. The `cycles` vector carries the local-only projection
/// of each detected SCC (issue numbers in the configured repo); cycles
/// that traversed at least one cross-repo `QualifiedId` surface the
/// omitted members in [`cross_repo_refs`](Self::cross_repo_refs) per
/// SPEC §11.4. A cycle whose local-projection length dropped below 2
/// after stripping cross-repo members is STILL emitted so the agent
/// knows the cycle exists — the missing members are in
/// `cross_repo_refs`.
///
/// `count` mirrors `cycles.len()` at all times.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DepCyclesResult {
    /// Detected cycles, each projected to the bare-`u64` local
    /// representation per SPEC §7.7. May be shorter than the true cycle
    /// length when cross-repo members were stripped — see
    /// [`cross_repo_refs`](Self::cross_repo_refs) and SPEC §11.4.
    pub cycles: Vec<Vec<u64>>,
    /// Number of cycles in [`cycles`](Self::cycles). Always equal to
    /// `cycles.len()`.
    pub count: usize,
    /// Cross-repo `QualifiedId` cycle members dropped from the bare-`u64`
    /// projection of [`cycles`](Self::cycles), per SPEC §11.4.
    ///
    /// `Some` iff at least one cycle visited a cross-repo node; `None`
    /// otherwise. Elided from the JSON envelope when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_repo_refs: Option<CrossRepoRefs>,
}

/// Core projection helper: given the raw cycle set returned by
/// [`DependencyGraph::detect_all_cycles`] (or its `id`-scoped
/// restriction), perform the SPEC §7.7 / §11.4 projection and return the
/// `(cycles, cross_repo_refs)` pair.
///
/// This is a thin facade over
/// [`crate::tools::cross_repo::project_all_cycles`] + the `cycles`-flavoured
/// [`crate::tools::cross_repo::cycles_summary`] closure. Kept at this
/// layer so the handler can stay agnostic of the shared primitives and
/// so the unit tests below continue to exercise the `(cycles,
/// cross_repo_refs)` pairing end-to-end.
///
/// Pulled out of [`handle_dep_cycles`] so it can be covered by
/// hermetic unit tests without constructing a [`ServerState`].
///
/// The `raw` parameter is generic over `AsRef<[QualifiedId]>` so the
/// caller can pass either owned `Vec<QualifiedId>` SCCs (the shape of
/// `detect_all_cycles`'s return) or borrowed `&[QualifiedId]` slices
/// (the zero-copy shape produced by [`filter_cycles_containing`] and
/// the `id`-less passthrough in [`handle_dep_cycles`]). Removes the
/// owned-clone that previously bridged the two shapes — see bead
/// `unblock-29p.45`.
fn project_all<S>(
    raw: &[S],
    configured_owner: &str,
    configured_repo: &str,
) -> (Vec<Vec<u64>>, Option<CrossRepoRefs>)
where
    S: AsRef<[QualifiedId]>,
{
    let (cycles, accum) = cross_repo::project_all_cycles(raw, configured_owner, configured_repo);
    let cross_repo_refs =
        cross_repo::build_cross_repo_refs_with_summary(accum, cross_repo::cycles_summary);
    (cycles, cross_repo_refs)
}

/// Keep only the SCCs that contain the supplied target node.
///
/// Matches the "targeted cycle check involving that node" wording in
/// SPEC §7.7 flow step 2. Comparison is on full [`QualifiedId`] equality,
/// so the caller is responsible for resolving the bare `id` against
/// `(client.owner(), client.repo())` before calling.
///
/// Returns borrowed `&[QualifiedId]` slices rather than borrowed
/// `&Vec<QualifiedId>` so callers can hand the result straight to
/// [`project_all`] (or [`crate::tools::cross_repo::project_all_cycles`])
/// without an intermediate clone of the SCC contents — see bead
/// `unblock-29p.45`.
fn filter_cycles_containing<'a>(
    raw: &'a [Vec<QualifiedId>],
    target: &QualifiedId,
) -> Vec<&'a [QualifiedId]> {
    raw.iter()
        .filter(|scc| scc.contains(target))
        .map(Vec::as_slice)
        .collect()
}

/// Execute the `dep_cycles` tool handler.
///
/// See the module-level docs for the full contract. Flow (mirrors SPEC
/// §7.7):
///
/// 1. Warm the cache if needed ([`crate::tools::rebuild_cache`]).
/// 2. Read the cached [`DependencyGraph`]; fall back to a direct
///    [`fetch_graph_data`](unblock_github::GitHubApi::fetch_graph_data)
///    retry when the cache is still empty after the rebuild attempt.
/// 3. Call [`DependencyGraph::detect_all_cycles`].
/// 4. When `params.id` is `Some`, restrict to SCCs that contain the
///    resolved [`QualifiedId`].
/// 5. Project each cycle onto its local / cross-repo partitions per SPEC
///    §11.4.
/// 6. Return [`DepCyclesResult`] with `count = cycles.len()`.
///
/// # Errors
///
/// Returns [`ErrorData`] only when the cache cannot be warmed (e.g.
/// GitHub network failure). The error surface matches the
/// [`crate::tools::stats`] R6 posture — `DepCyclesResult` has no
/// `stale` field, so failure must be signalled via the error channel.
#[instrument(
    skip(state, params),
    name = "handle_dep_cycles",
    fields(
        agent.kind = state.agent_kind_str(),
        id = params.id,
    ),
)]
pub async fn handle_dep_cycles(
    state: &ServerState,
    params: DepCyclesParams,
) -> Result<DepCyclesResult, ErrorData> {
    info!("DepCycles tool invoked");

    // Step 1: warm the cache if needed. Zero work on the cache-hit path.
    if !state.cache.is_fresh().await {
        tracing::debug!("DepCycles cache is stale — triggering lazy rebuild");
        crate::tools::rebuild_cache(state).await;
    }

    // Step 2: pull the dependency graph from the cache. Fall back to a
    // direct fetch when the rebuild did not populate the cache (network
    // failure inside `rebuild_cache` swallows the error there, so we
    // must re-issue here to surface the real cause — R3 posture,
    // mirrors stats.rs).
    let configured_owner = state.github.owner().to_owned();
    let configured_repo = state.github.repo().to_owned();

    let raw_cycles: Vec<Vec<QualifiedId>> = if let Some(graph) = state.cache.get_graph().await {
        graph.detect_all_cycles()
    } else {
        tracing::warn!("DepCycles cache empty after rebuild — retrying fetch to surface the error");
        let (issues_vec, edges_vec) = state
            .github
            .fetch_graph_data()
            .await
            .map_err(github_error_to_mcp)?;
        // The retry unexpectedly succeeded — populate the cache so a
        // follow-up call is warm, then compute cycles against the
        // freshly built graph without re-reading the cache.
        let graph_built = DependencyGraph::build(&issues_vec, &edges_vec);
        // SPEC §3.3 Filter 3 / §14 Invariant 14(a): scope the cached ready
        // set to the configured (owner, repo) so downstream consumers
        // inherit the local-only guarantee by construction.
        let ready_set =
            graph_built.compute_ready_set(&issues_vec, &configured_owner, &configured_repo);
        let cycles = graph_built.detect_all_cycles();
        state.cache.update(issues_vec, ready_set, graph_built).await;
        cycles
    };

    // Step 3: apply the optional `id` filter. The parameter is a local
    // issue number (SPEC §7.7 types it as `u64`, not `IssueRef`), so
    // resolve it against `(configured_owner, configured_repo)` before
    // comparing. Both branches yield `Vec<&[QualifiedId]>` — a
    // zero-copy slice-of-slice view over the SCCs owned by
    // `raw_cycles`. `project_all` accepts this shape directly via its
    // `AsRef<[QualifiedId]>` bound, so no clone of the SCC contents
    // is required (bead `unblock-29p.45`).
    let filtered_refs: Vec<&[QualifiedId]> = if let Some(id) = params.id {
        let target = QualifiedId::new(configured_owner.clone(), configured_repo.clone(), id);
        filter_cycles_containing(&raw_cycles, &target)
    } else {
        raw_cycles.iter().map(Vec::as_slice).collect()
    };

    // Step 4: project each cycle into its (local, cross-repo) partitions
    // per SPEC §11.4. `project_all` borrows each SCC as a `&[QualifiedId]`
    // slice — the previous owned-Vec clone that bridged this boundary is
    // eliminated (bead `unblock-29p.45`).
    let (cycles, cross_repo_refs) =
        project_all(&filtered_refs, &configured_owner, &configured_repo);

    let count = cycles.len();
    Ok(DepCyclesResult {
        cycles,
        count,
        cross_repo_refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qid(owner: &str, repo: &str, number: u64) -> QualifiedId {
        QualifiedId::new(owner, repo, number)
    }

    // Note: the `project_cycle` and `build_cross_repo_refs` unit-level
    // tests formerly in this block migrated to
    // `crate::tools::cross_repo` alongside the helpers themselves when
    // the shared module was extracted (bead `unblock-eos.2` /
    // `unblock-eos.7`). The `project_all` tests below still exercise the
    // facade here so the `(cycles, cross_repo_refs)` pairing specific to
    // `dep_cycles` keeps its hermetic unit coverage.

    // ── project_all ──────────────────────────────────────────────────

    #[test]
    fn project_all_acyclic_returns_empty_cycles_and_no_cross_repo() {
        let raw: Vec<Vec<QualifiedId>> = vec![];
        let (cycles, refs) = project_all(&raw, "acme", "widgets");
        assert!(cycles.is_empty());
        assert!(refs.is_none());
    }

    #[test]
    fn project_all_local_only_cycle_emits_cycles_and_none_refs() {
        let raw = vec![vec![qid("acme", "widgets", 6), qid("acme", "widgets", 7)]];
        let (cycles, refs) = project_all(&raw, "acme", "widgets");
        assert_eq!(cycles.len(), 1);
        // Relative order preserved as Tarjan returned it.
        assert_eq!(cycles[0], vec![6, 7]);
        assert!(refs.is_none(), "no cross-repo node → refs None");
    }

    #[test]
    fn project_all_mixed_cycle_populates_refs_and_shortened_projection() {
        // Single cycle of length 3: two local + one cross-repo. The
        // cross-repo member is stripped; `cycles` emits only the local
        // two.
        let raw = vec![vec![
            qid("acme", "widgets", 1),
            qid("other", "repo", 99),
            qid("acme", "widgets", 2),
        ]];
        let (cycles, refs) = project_all(&raw, "acme", "widgets");
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0],
            vec![1, 2],
            "cross-repo stripped, locals retained in order"
        );
        let refs = refs.expect("cross-repo node → refs Some");
        assert_eq!(refs.omitted, vec!["other/repo#99".to_owned()]);
    }

    #[test]
    fn project_all_cross_repo_majority_still_emits_short_cycle() {
        // SPEC §7.7 flow step 4b: a cycle whose local-projection length
        // drops below 2 is STILL emitted so the agent knows the cycle
        // exists.
        let raw = vec![vec![
            qid("acme", "widgets", 1),
            qid("other", "repo", 99),
            qid("third", "party", 100),
        ]];
        let (cycles, refs) = project_all(&raw, "acme", "widgets");
        assert_eq!(cycles.len(), 1, "cycle preserved even with 1 local member");
        assert_eq!(cycles[0], vec![1]);
        let refs = refs.expect("cross-repo members → refs Some");
        assert_eq!(
            refs.omitted,
            vec!["other/repo#99".to_owned(), "third/party#100".to_owned()],
        );
    }

    #[test]
    fn project_all_all_cross_repo_cycle_emits_empty_vec_but_preserves_refs() {
        // A cycle with zero local members still appears in `cycles`
        // (as an empty Vec<u64>) per flow step 4b — the cycle exists.
        let raw = vec![vec![qid("other", "repo", 1), qid("third", "party", 2)]];
        let (cycles, refs) = project_all(&raw, "acme", "widgets");
        assert_eq!(cycles.len(), 1);
        assert!(cycles[0].is_empty());
        let refs = refs.expect("cross-repo-only cycle → refs Some");
        assert_eq!(refs.omitted.len(), 2);
    }

    // ── filter_cycles_containing ─────────────────────────────────────

    #[test]
    fn filter_cycles_containing_matches_target_in_scc() {
        let c1 = vec![qid("acme", "widgets", 1), qid("acme", "widgets", 2)];
        let c2 = vec![qid("acme", "widgets", 3), qid("acme", "widgets", 4)];
        let raw = vec![c1.clone(), c2.clone()];
        let target = qid("acme", "widgets", 3);
        let hits = filter_cycles_containing(&raw, &target);
        assert_eq!(hits.len(), 1);
        // `hits[0]` is a `&[QualifiedId]` borrowed from `raw`; compare
        // against the owned `c2` via slice equality.
        assert_eq!(hits[0], c2.as_slice());
    }

    #[test]
    fn filter_cycles_containing_returns_empty_when_absent() {
        let raw = vec![vec![qid("acme", "widgets", 1), qid("acme", "widgets", 2)]];
        let target = qid("acme", "widgets", 999);
        let hits = filter_cycles_containing(&raw, &target);
        assert!(hits.is_empty());
    }

    #[test]
    fn filter_cycles_containing_scopes_by_owner_repo_not_just_number() {
        // A cycle containing `other/repo#42` must NOT match a target
        // resolved to `acme/widgets#42`. The bare `id` parameter is
        // scoped to the configured repo (SPEC §7.7 R8).
        let raw = vec![vec![qid("other", "repo", 42), qid("other", "repo", 43)]];
        let target = qid("acme", "widgets", 42);
        let hits = filter_cycles_containing(&raw, &target);
        assert!(
            hits.is_empty(),
            "`id` target scoped to configured repo — must not leak cross-repo SCC match",
        );
    }

    // ── DepCyclesResult serialization shape ─────────────────────────

    #[test]
    fn dep_cycles_result_serializes_expected_local_shape() {
        let res = DepCyclesResult {
            cycles: vec![vec![6, 7]],
            count: 1,
            cross_repo_refs: None,
        };
        let json = serde_json::to_value(&res).expect("serialize");
        assert_eq!(json["count"], 1);
        assert_eq!(json["cycles"][0][0], 6);
        assert_eq!(json["cycles"][0][1], 7);
        assert!(
            json.get("cross_repo_refs").is_none(),
            "None `cross_repo_refs` MUST be elided via skip_serializing_if: {json}"
        );
    }

    #[test]
    fn dep_cycles_result_serializes_cross_repo_refs_when_some() {
        let res = DepCyclesResult {
            cycles: vec![vec![6]],
            count: 1,
            cross_repo_refs: Some(CrossRepoRefs {
                omitted: vec!["other/repo#99".to_owned()],
                summary: Some("1 cross-repo cycle member omitted from `cycles`".to_owned()),
            }),
        };
        let json = serde_json::to_value(&res).expect("serialize");
        assert_eq!(json["cross_repo_refs"]["omitted"][0], "other/repo#99");
        assert_eq!(
            json["cross_repo_refs"]["summary"],
            "1 cross-repo cycle member omitted from `cycles`"
        );
    }

    #[test]
    fn dep_cycles_result_count_mirrors_cycles_len_invariant() {
        // Trivial structural guard — the handler MUST set
        // `count = cycles.len()`. We construct a direct instance here;
        // the integration tests cover the handler pathway.
        let cycles = vec![vec![1, 2], vec![3, 4, 5]];
        let res = DepCyclesResult {
            count: cycles.len(),
            cycles,
            cross_repo_refs: None,
        };
        assert_eq!(res.count, res.cycles.len());
    }
}
