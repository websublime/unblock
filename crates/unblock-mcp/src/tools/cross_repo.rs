//! Shared primitives for SPEC §11.4 "Cross-Repo Response Contract"
//! projection.
//!
//! Prior to this module, `ready.rs`, `dep_cycles.rs`, `prime.rs`, and
//! (now) the `close` handler each held local copies of the projection
//! logic:
//!
//! 1. Walk the source collection (cycles for `dep_cycles` / `prime`; open
//!    issues with blockers for `ready`; the unblock cascade for `close`)
//!    and partition references into **local** (keep the bare `u64`
//!    number in the response) vs **cross-repo** (collect the
//!    `owner/repo#number` display string for the §11.4 trailer).
//! 2. Collect cross-repo references into a [`BTreeSet<String>`] keyed by
//!    [`QualifiedId::Display`](unblock_core::types::QualifiedId) — the
//!    `BTreeSet` gives us SPEC §14 Invariant 14 (a) determinism (dedup + lex
//!    sort) for free.
//! 3. Wrap the accumulated set into an `Option<CrossRepoRefs>` (None when
//!    the set is empty) with a singular/plural italic summary matching the
//!    tool's semantic noun (`"cycle member(s)"` for projection-from-cycles,
//!    `"blocker(s)"` for projection-from-ready).
//!
//! The canonical primitives live here. Callers bring their own
//! sum­mary-grammar closure so the tool-specific phrasing in SPEC §11.4 is
//! preserved byte-for-byte — parity across all four tools is a user
//! non-negotiable and the summary strings are part of the public response
//! envelope. See [`cycles_summary`], [`ready_summary`], and
//! [`close_summary`] for the exact phrasing.
//!
//! # Non-goals
//!
//! This module intentionally does NOT:
//! - Know how to classify a blocker as "open" (ready's domain — it walks
//!   the graph and consults `issue_state`).
//! - Know how to filter SCCs to a target node (`dep_cycles`'s domain — see
//!   `filter_cycles_containing`).
//! - Wrap in `Option<CrossRepoRefs>` with a pre-baked noun — callers pass
//!   a grammar closure via
//!   [`build_cross_repo_refs_with_summary`].
//!
//! # Determinism contract
//!
//! All helpers use [`BTreeSet<String>`] for accumulation. The final
//! `omitted: Vec<String>` in [`CrossRepoRefs`] emerges lexicographically
//! sorted without an explicit `sort()` call — this is the mechanism by
//! which SPEC §14 Invariant 14 (a) ("identical graph state produces
//! identical responses") is honoured.
//!
//! # Cross-reference
//!
//! - SPEC §11.4 — Cross-Repo Response Contract (markdown + JSON shape).
//! - SPEC §14 Invariant 14 — response determinism.
//! - ARCH §5.5 — cross-repo node keying via `QualifiedId`.
//! - Beads `unblock-eos.2` (this extraction), `unblock-eos.7` (prime
//!   adoption), `unblock-iov` (close adoption).

use std::collections::BTreeSet;

use unblock_core::types::{CrossRepoRefs, QualifiedId};

/// Project a single cycle (`Vec<QualifiedId>`) onto its local and
/// cross-repo partitions, feeding the cross-repo members into the shared
/// `omitted` set.
///
/// The local projection preserves the relative iteration order that
/// Tarjan returned (local-only subsequence of the original SCC). The
/// cross-repo set is a [`BTreeSet<String>`] keyed by
/// [`QualifiedId::Display`](QualifiedId#impl-Display-for-QualifiedId)
/// (`"owner/repo#number"`) so de-duplication across cycles is free and
/// the final `Vec<String>` emerges lexicographically sorted — the
/// Invariant 14 determinism contract holds without an explicit
/// `sort()` call.
///
/// Returns the local projection (may be empty if every member was
/// cross-repo).
///
/// Used by `dep_cycles.rs` (SPEC §7.7) and `prime.rs` (SPEC §7.3).
pub(crate) fn project_cycle(
    cycle: &[QualifiedId],
    configured_owner: &str,
    configured_repo: &str,
    cross_repo_accum: &mut BTreeSet<String>,
) -> Vec<u64> {
    let mut local = Vec::with_capacity(cycle.len());
    for qid in cycle {
        if qid.owner == configured_owner && qid.repo == configured_repo {
            local.push(qid.number);
        } else {
            cross_repo_accum.insert(qid.to_string());
        }
    }
    local
}

/// Project an entire cycle set onto its local + cross-repo partitions.
///
/// Handy for `dep_cycles` (all detected SCCs) and `prime` (the §7.3
/// "Issues with cycles" section). Returns the local-projection vector of
/// vectors AND the raw accumulator so the caller can feed it into
/// [`build_cross_repo_refs_with_summary`] with its own grammar closure.
///
/// SPEC §7.7 flow step 4b: emit every detected cycle, even the ones whose
/// local-projection length is `< 2` (mixed cycles where all but ≤1 member
/// are cross-repo). The agent must know the cycle exists; the missing
/// members land in `cross_repo_refs`.
pub(crate) fn project_all_cycles(
    raw: &[Vec<QualifiedId>],
    configured_owner: &str,
    configured_repo: &str,
) -> (Vec<Vec<u64>>, BTreeSet<String>) {
    let mut cycles: Vec<Vec<u64>> = Vec::with_capacity(raw.len());
    let mut cross_repo_accum: BTreeSet<String> = BTreeSet::new();
    for cycle in raw {
        let local = project_cycle(
            cycle,
            configured_owner,
            configured_repo,
            &mut cross_repo_accum,
        );
        cycles.push(local);
    }
    (cycles, cross_repo_accum)
}

/// Project the unblock cascade (`Vec<QualifiedId>`) onto its local and
/// cross-repo partitions.
///
/// Used by the `close` MCP tool per SPEC §8.2 flow step 9: local
/// dependents are projected to bare `u64` numbers for
/// `CloseResult.unblocked`; cross-repo dependents populate the
/// [`CrossRepoRefs`] envelope per SPEC §11.4 (row 4 of the affected-tools
/// table).
///
/// The local projection preserves the iteration order of `cascade`
/// (petgraph Incoming-neighbour order, the same order the caller already
/// iterated in Phase 3 of the close handler). The cross-repo set is a
/// [`BTreeSet<String>`] keyed by
/// [`QualifiedId::Display`](QualifiedId#impl-Display-for-QualifiedId)
/// (`"owner/repo#number"`) so de-duplication is free and the final
/// `Vec<String>` emerges lexicographically sorted — SPEC §14 Invariant
/// 14 (b) determinism contract holds without an explicit `sort()` call.
///
/// Returns the local projection (may be empty if every cascade member
/// was cross-repo) plus the cross-repo accumulator.
pub(crate) fn project_cascade(
    cascade: &[QualifiedId],
    configured_owner: &str,
    configured_repo: &str,
) -> (Vec<u64>, BTreeSet<String>) {
    let mut local: Vec<u64> = Vec::with_capacity(cascade.len());
    let mut cross_repo_accum: BTreeSet<String> = BTreeSet::new();
    for qid in cascade {
        if qid.owner == configured_owner && qid.repo == configured_repo {
            local.push(qid.number);
        } else {
            cross_repo_accum.insert(qid.to_string());
        }
    }
    (local, cross_repo_accum)
}

/// Build the optional [`CrossRepoRefs`] envelope from the accumulated
/// cross-repo `QualifiedId` display strings, with a caller-supplied
/// summary grammar.
///
/// Returns `None` when `accum` is empty (preserving SPEC §11.4's "`Some`
/// iff `omitted` is non-empty" invariant). Callers can forward this
/// directly into `<ToolResult>::cross_repo_refs` or, for `prime`, into
/// the markdown renderer.
///
/// The `summary_fn` closure receives the count of omitted entries and
/// returns the italic summary line verbatim (without the enclosing
/// underscores — §11.4 adapter wraps them around the summary at render
/// time). Pass `|_| String::new()` and discard the summary field if you
/// need to opt out; empty strings still produce a `Some(...)` with
/// `summary: Some("")`, so don't — use a real phrasing.
///
/// # Determinism
///
/// The `accum`'s iteration order is already lexicographic (it's a
/// [`BTreeSet`]), so the resulting `Vec<String>` is sorted without an
/// explicit `sort()` call. This is the SPEC §14 Invariant 14 (a)
/// determinism contract.
///
/// # Example summaries (must match existing tool phrasing byte-for-byte)
///
/// ```text
/// dep_cycles / prime: {n} cross-repo cycle {member|members} omitted from `cycles`
/// ready:              {n} cross-repo {blocker|blockers} excluded local issue(s) from ready set
/// ```
pub(crate) fn build_cross_repo_refs_with_summary<F>(
    accum: BTreeSet<String>,
    summary_fn: F,
) -> Option<CrossRepoRefs>
where
    F: FnOnce(usize) -> String,
{
    if accum.is_empty() {
        return None;
    }
    let omitted: Vec<String> = accum.into_iter().collect();
    let summary = Some(summary_fn(omitted.len()));
    Some(CrossRepoRefs { omitted, summary })
}

/// Format the **`cycles`-projection** italic summary per SPEC §11.4.
///
/// Exact string (byte-for-byte — do not paraphrase):
///
/// ```text
/// n == 1 → 1 cross-repo cycle member omitted from `cycles`
/// n >= 2 → {n} cross-repo cycle members omitted from `cycles`
/// ```
///
/// Used by `dep_cycles` (JSON `cross_repo_refs.summary` field) and by
/// `prime` (markdown §11.4 trailer). Same phrasing in both tools — parity
/// is a non-negotiable part of the tool-suite contract.
#[must_use]
pub(crate) fn cycles_summary(n: usize) -> String {
    format!(
        "{n} cross-repo cycle {noun} omitted from `cycles`",
        noun = if n == 1 { "member" } else { "members" },
    )
}

/// Format the **ready-projection** italic summary per SPEC §11.4.
///
/// Exact string (byte-for-byte — do not paraphrase):
///
/// - `n == 1` → `"1 cross-repo blocker excluded local issue(s) from ready set"`
/// - `n >= 2` → `"{n} cross-repo blockers excluded local issue(s) from ready set"`
///
/// Used by `ready` (JSON `cross_repo_refs.summary` field).
#[must_use]
pub(crate) fn ready_summary(n: usize) -> String {
    format!(
        "{n} cross-repo {noun} excluded local issue(s) from ready set",
        noun = if n == 1 { "blocker" } else { "blockers" },
    )
}

/// Format the **close-cascade** italic summary per SPEC §11.4 (row 4
/// of the affected-tools table at §11.4, phrasing per §8.2 line 1262).
///
/// Exact string (byte-for-byte — do not paraphrase):
///
/// - `n == 1` → ``"1 cross-repo dependent cascade-updated but omitted from `unblocked`"``
/// - `n >= 2` → ``"{n} cross-repo dependents cascade-updated but omitted from `unblocked`"``
///
/// Used by `close` (JSON `cross_repo_refs.summary` field). The grammar
/// mirrors [`cycles_summary`] / [`ready_summary`]: singular/plural noun
/// (`dependent` / `dependents`) keyed off the count. Cross-repo
/// dependents ARE still cascade-updated in Phase 3 of the close handler
/// — the summary reports what the response projection omitted, not what
/// the mutation skipped.
#[must_use]
pub(crate) fn close_summary(n: usize) -> String {
    format!(
        "{n} cross-repo {noun} cascade-updated but omitted from `unblocked`",
        noun = if n == 1 { "dependent" } else { "dependents" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qid(owner: &str, repo: &str, number: u64) -> QualifiedId {
        QualifiedId::new(owner, repo, number)
    }

    // ── project_cycle ──────────────────────────────────────────────────

    #[test]
    fn project_cycle_all_local_returns_local_numbers_no_cross_repo() {
        let cycle = vec![qid("acme", "widgets", 6), qid("acme", "widgets", 7)];
        let mut accum = BTreeSet::new();
        let local = project_cycle(&cycle, "acme", "widgets", &mut accum);
        assert_eq!(local, vec![6, 7]);
        assert!(accum.is_empty());
    }

    #[test]
    fn project_cycle_all_cross_repo_returns_empty_local_populates_accum() {
        let cycle = vec![qid("other", "repo", 99), qid("third", "repo", 1)];
        let mut accum = BTreeSet::new();
        let local = project_cycle(&cycle, "acme", "widgets", &mut accum);
        assert!(local.is_empty());
        // BTreeSet gives lex order for free.
        let collected: Vec<String> = accum.into_iter().collect();
        assert_eq!(collected, vec!["other/repo#99", "third/repo#1"]);
    }

    #[test]
    fn project_cycle_mixed_splits_local_from_cross_repo() {
        let cycle = vec![
            qid("acme", "widgets", 6),
            qid("other", "repo", 99),
            qid("acme", "widgets", 7),
        ];
        let mut accum = BTreeSet::new();
        let local = project_cycle(&cycle, "acme", "widgets", &mut accum);
        assert_eq!(local, vec![6, 7]); // Tarjan order preserved for locals.
        let collected: Vec<String> = accum.into_iter().collect();
        assert_eq!(collected, vec!["other/repo#99"]);
    }

    #[test]
    fn project_cycle_dedups_repeated_cross_repo_nodes_via_btreeset() {
        let cycle = vec![
            qid("other", "repo", 99),
            qid("other", "repo", 99),
            qid("other", "repo", 99),
        ];
        let mut accum = BTreeSet::new();
        let _ = project_cycle(&cycle, "acme", "widgets", &mut accum);
        assert_eq!(accum.len(), 1);
        assert!(accum.contains("other/repo#99"));
    }

    // ── project_all_cycles ─────────────────────────────────────────────

    #[test]
    fn project_all_cycles_preserves_input_cycle_order() {
        let raw = vec![
            vec![qid("acme", "widgets", 6), qid("acme", "widgets", 7)],
            vec![qid("acme", "widgets", 10), qid("acme", "widgets", 11)],
        ];
        let (cycles, accum) = project_all_cycles(&raw, "acme", "widgets");
        assert_eq!(cycles, vec![vec![6_u64, 7], vec![10_u64, 11]]);
        assert!(accum.is_empty());
    }

    #[test]
    fn project_all_cycles_mixed_emits_short_local_projection_and_accum() {
        // Mixed cycle: local 6 → cross other/repo#99 → local 6. The local
        // projection is [6] (length 1 — still emitted per §7.7 flow 4b).
        let raw = vec![vec![qid("acme", "widgets", 6), qid("other", "repo", 99)]];
        let (cycles, accum) = project_all_cycles(&raw, "acme", "widgets");
        assert_eq!(cycles, vec![vec![6_u64]]);
        let collected: Vec<String> = accum.into_iter().collect();
        assert_eq!(collected, vec!["other/repo#99"]);
    }

    #[test]
    fn project_all_cycles_dedups_cross_repo_across_cycles() {
        let raw = vec![
            vec![qid("acme", "widgets", 6), qid("other", "repo", 99)],
            vec![qid("acme", "widgets", 7), qid("other", "repo", 99)],
        ];
        let (_, accum) = project_all_cycles(&raw, "acme", "widgets");
        assert_eq!(accum.len(), 1);
    }

    // ── build_cross_repo_refs_with_summary ─────────────────────────────

    #[test]
    fn build_cross_repo_refs_empty_accum_returns_none() {
        let accum: BTreeSet<String> = BTreeSet::new();
        let refs = build_cross_repo_refs_with_summary(accum, cycles_summary);
        assert!(refs.is_none());
    }

    #[test]
    fn build_cross_repo_refs_non_empty_returns_some_with_sorted_omitted() {
        let mut accum = BTreeSet::new();
        accum.insert("zeta/repo#1".to_string());
        accum.insert("alpha/repo#99".to_string());
        accum.insert("mid/repo#5".to_string());
        let refs = build_cross_repo_refs_with_summary(accum, cycles_summary)
            .expect("non-empty accum produces Some");
        // BTreeSet iteration = lex order, preserved through Vec::collect.
        assert_eq!(
            refs.omitted,
            vec!["alpha/repo#99", "mid/repo#5", "zeta/repo#1"]
        );
    }

    #[test]
    fn build_cross_repo_refs_with_custom_summary_closure() {
        let mut accum = BTreeSet::new();
        accum.insert("other/repo#1".to_string());
        let refs =
            build_cross_repo_refs_with_summary(accum, |n| format!("custom-{n}")).expect("Some");
        assert_eq!(refs.summary.as_deref(), Some("custom-1"));
    }

    // ── cycles_summary — SPEC §11.4 phrasing parity ────────────────────

    #[test]
    fn cycles_summary_singular_matches_spec() {
        assert_eq!(
            cycles_summary(1),
            "1 cross-repo cycle member omitted from `cycles`"
        );
    }

    #[test]
    fn cycles_summary_plural_matches_spec() {
        assert_eq!(
            cycles_summary(2),
            "2 cross-repo cycle members omitted from `cycles`"
        );
        assert_eq!(
            cycles_summary(5),
            "5 cross-repo cycle members omitted from `cycles`"
        );
    }

    // ── ready_summary — SPEC §11.4 phrasing parity ─────────────────────

    #[test]
    fn ready_summary_singular_matches_spec() {
        assert_eq!(
            ready_summary(1),
            "1 cross-repo blocker excluded local issue(s) from ready set"
        );
    }

    #[test]
    fn ready_summary_plural_matches_spec() {
        assert_eq!(
            ready_summary(2),
            "2 cross-repo blockers excluded local issue(s) from ready set"
        );
        assert_eq!(
            ready_summary(7),
            "7 cross-repo blockers excluded local issue(s) from ready set"
        );
    }

    // ── project_cascade ────────────────────────────────────────────────

    #[test]
    fn project_cascade_all_local_returns_local_numbers_no_cross_repo() {
        let cascade = vec![qid("acme", "widgets", 10), qid("acme", "widgets", 11)];
        let (local, accum) = project_cascade(&cascade, "acme", "widgets");
        assert_eq!(local, vec![10, 11]);
        assert!(accum.is_empty());
    }

    #[test]
    fn project_cascade_all_cross_repo_returns_empty_local_populates_accum() {
        let cascade = vec![qid("other", "repo", 99), qid("third", "repo", 1)];
        let (local, accum) = project_cascade(&cascade, "acme", "widgets");
        assert!(local.is_empty());
        // BTreeSet gives lex order for free.
        let collected: Vec<String> = accum.into_iter().collect();
        assert_eq!(collected, vec!["other/repo#99", "third/repo#1"]);
    }

    #[test]
    fn project_cascade_mixed_splits_local_from_cross_repo() {
        let cascade = vec![
            qid("acme", "widgets", 10),
            qid("other", "repo", 99),
            qid("acme", "widgets", 11),
            qid("alpha", "upstream", 42),
        ];
        let (local, accum) = project_cascade(&cascade, "acme", "widgets");
        // Cascade iteration order is preserved for locals.
        assert_eq!(local, vec![10, 11]);
        // BTreeSet iteration = lex order.
        let collected: Vec<String> = accum.into_iter().collect();
        assert_eq!(collected, vec!["alpha/upstream#42", "other/repo#99"]);
    }

    #[test]
    fn project_cascade_dedups_repeated_cross_repo_nodes_via_btreeset() {
        let cascade = vec![
            qid("other", "repo", 99),
            qid("other", "repo", 99),
            qid("other", "repo", 99),
        ];
        let (_local, accum) = project_cascade(&cascade, "acme", "widgets");
        assert_eq!(accum.len(), 1);
        assert!(accum.contains("other/repo#99"));
    }

    // ── close_summary — SPEC §11.4 / §8.2 phrasing parity ──────────────

    #[test]
    fn close_summary_singular_matches_spec() {
        assert_eq!(
            close_summary(1),
            "1 cross-repo dependent cascade-updated but omitted from `unblocked`"
        );
    }

    #[test]
    fn close_summary_plural_matches_spec() {
        assert_eq!(
            close_summary(2),
            "2 cross-repo dependents cascade-updated but omitted from `unblocked`"
        );
        assert_eq!(
            close_summary(5),
            "5 cross-repo dependents cascade-updated but omitted from `unblocked`"
        );
    }
}
