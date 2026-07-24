---
name: reference-d41-doc-cascade-trap-classes
description: A version-renumbering doc cascade is NOT a sed job — 5 trap classes that a token rename gets wrong, and the CI blind spot that hides the worst one
type: gotcha
---

Learned executing `ub-lp9.15` (D41 post-GA version resequence, merged 2026-07-21 PR #426). When a decision
renumbers versions and the cascade must reach derived docs, a mechanical token rename is **actively wrong** in
five distinct ways. Reuse this checklist for any future resequence.

1. **Renumbering can assert a falsehood.** D41 *removed* streamable-HTTP from the committed versions. A rename
   produces "v1.5 streamable-HTTP" — a claim that never existed. Items that were **dropped or moved to a
   direction bucket must be REWRITTEN, never renumbered.**
2. **Section cross-refs need SWAPS, not shifts** (§3↔§4, §5↔§6). ~14 sites.
3. **Stance defects hide behind correct numbering.** Four sites asserted positions the roadmap contradicts —
   worst: `unblock-storage.md` OQ-6, where the specced renumber would have **re-committed into v1.3 a decision
   the roadmap records as *dropped*** (`00-roadmap.md:255-256`). A 4th sibling (`crates/unblock-sync/src/error.rs:86`,
   "used at v1.3 reconciliation") was **manufactured by our own renumber** and only caught post-implementation.
4. **Inverted participation claims.** "No v1.2 participation" was doubly wrong (number moved AND roadmap §9 marks
   the crate as participating) — renumbering alone leaves the false claim standing.
5. **Protected lookalikes:** `unblock.mcp.v1.N` / `unblock.scheduler.vN` contract ids, `--tlsv1.2`, Cargo versions,
   generic enumerations, and `v1.2–v1.5` ranges. Verify with a diff check that the id is **byte-identical on both
   sides** and only surrounding release prose moved.

**Why:** classes 1/3/4 are semantic — a reviewer checking the *version* and *§-ref* axes returns the file clean.
In this task each stance defect was found by **one lens out of three**, which is the whole argument for
perspective-diverse (not redundant) reviewers.

**THE CI BLIND SPOT — no mechanical guard exists.** doc-lint class (b) needs an FR/NFR-adjacent `[marker]`, which
no version prose has; class (e) only checks that a `§N` **resolves** — and §3–§6 all exist, so a ref into the
**wrong existing section passes CI silently**. `docs/PROCESS.md` and `docs/roadmap.html` are OUT of the 19-file
corpus entirely despite PROCESS.md being `@`-imported into every session. "The lint will catch it later" is
**not available** for this defect class.

**How to apply:**
- Partition scope by **the doc-lint corpus list itself** (`xtask/src/doc_lint.rs:29`), not by intuition — corpus
  entries 6 and 7 (`implementation-plan.md`, `ci-cd-and-distribution.md`) were left ownerless and both carried live
  stale hits; that was the design gate's root-cause blocker.
- Fix the **authoritative source first** — verify the "atomic" landing PR didn't leave the PRD/roadmap themselves
  stale. Here the PRD had 19 stale sites when the brief estimated ~11.
- Re-read for **new** incompleteness after applying: minting a new version slot made `CLAUDE.md:23` incomplete —
  a defect no pre-change sweep could find.
- Prefer **rot-proof pointer form** over bare numbers in always-loaded docs (`until the shared-state release
  (00-roadmap.md §4)`, not `until the v1.2 shared remote`).

See [[project-roadmap-resequence-2026-07]] for the D41 decision itself and
[[feedback-implementer-probe-must-include-cargo-fmt]] for the CI probe requirements.
