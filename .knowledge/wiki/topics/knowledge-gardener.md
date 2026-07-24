---
name: knowledge-gardener
description: The recurring consolidation sweep over .knowledge/ — the semantic layer above the structural knowledge-lint, run periodically to keep memories and wiki pages timeless and non-contradictory.
type: topic
category: orchestration
---

# Knowledge gardener

## When you need this

- The weekly reminder fires (a periodic prompt to sweep `.knowledge/` for drift).
- A large batch of new `.knowledge/` content just landed (a migration, or several substantive tasks in a
  row each adding their own memories/pages).
- Before locking a version's docs (PROCESS.md §2's just-in-time version lock) — a good checkpoint to make
  sure the descriptive layer isn't carrying dead status framing into the next phase.

## Procedure / facts

`.knowledge/` content decays the same way any narrated status does — see
[[feedback-derived-counts-die-in-prose-plans]] for the general pattern. The gardener is the recurring fix:

1. Run a **multi-lens sweep as a Workflow** (mates in parallel + a coordinator, per PROCESS.md §4), covering:
   - **Staleness** — in-flight framing (`ACTIVE`, `OPEN`, `awaiting merge`, `in progress`) left behind after
     the work it described actually landed or changed state.
   - **Contradiction, page-to-page** — two pages describing the same mechanism or hazard in terms that no
     longer agree (e.g. one page's "still open" contradicted by another page's "merged").
   - **Contradiction, page-to-normative** — a page asserting something the current PRD/spine/crate plans no
     longer say (versions, contract ids, D-ids, milestones that moved on).
   - **Duplicate + index** — near-duplicate pages that should stay distinct-but-sharpened (not merged) vs.
     genuine duplicates, plus index one-liner drift from each page's frontmatter `description`.
2. `cargo xtask knowledge-lint` (checks k1..k6) is the automatic **structural** layer — frontmatter shape,
   slug rules, index-as-data agreement, run-report sections/glossary. The gardener sweep is the **semantic**
   layer above it: knowledge-lint cannot tell you a page is stale, contradictory, or has drifted from the
   normative docs — only a reader (or an adversarial multi-lens sweep) can.
3. **Consolidate** the lenses' findings → build a **fix plan** per page → **surface judgment calls to the
   maintainer** (do not resolve ambiguous merges/retirements unilaterally — see Gotchas) → apply as one of:
   - **UPDATE-to-timeless** (the default) — keep the durable lesson, drop the dead status framing.
   - **Merge** — only when explicitly resolved as a genuine duplicate, never as a unilateral simplification.
   - **Retire** — via the sanctioned `scripts/knowledge/memory-retire.sh` (never a manual `rm`), and only
     when nothing durable would be lost.
4. Land the sweep as a **wiki run-report** (`docs/plans/templates/run-report.md` shape, in
   `.knowledge/wiki/runs/`) + a **PR**, same discipline as any other substantive change (PROCESS.md §6/§8).
5. **On any page ↔ normative conflict, the normative doc (PRD > spine > crate plans) wins** — `.knowledge/`
   is descriptive, never normative (CLAUDE.md doc map; PROCESS.md §8). The fix is always to the memory/wiki
   page, never to the normative doc, and never a silent edit — see Gotchas on judgment calls.

## Gotchas

- **Don't RETIRE a page that still holds a durable lesson.** A stale status wrapper around a real lesson is
  an UPDATE-to-timeless, not a retirement — retiring loses the lesson along with the dead framing.
- **Never merge or simplify unilaterally.** Two pages that read as near-duplicates may in fact describe
  distinct hazards/axes (worked example: [[feedback-verify-reviewers-mutate-shared-tree]] — a single
  reviewer mutating the SHARED checkout — vs. [[feedback-parallel-verify-agents-share-one-worktree-clobber]]
  — a multi-agent RACE inside ONE isolated worktree). Sharpen each page's one-liner to name its own axis
  instead of collapsing them; surface any genuine merge/retire call to the maintainer rather than deciding
  it in the sweep.
- **The index one-liner must track the frontmatter `description` in lockstep.** Changing a page's
  `description:` without updating its `memories/index.md` or `wiki/index.md` entry is an immediate k1
  finding — knowledge-lint catches this mechanically, but it is easy to forget mid-sweep across many files.
- **Run-reports are immutable — never rewrite a past one.** A gardener sweep that finds something wrong in
  an earlier run-report fixes it in the CURRENT sweep's own report and/or the affected normative doc /
  memory, not by editing history.

## Pointers

- `docs/PROCESS.md` §8 — the knowledge layer's rules and hard enforcement.
- `xtask/src/knowledge_lint.rs` — the k1..k6 structural checks (the automatic layer this runbook sits above).
- `scripts/knowledge/memory-retire.sh` — the sanctioned removal path for a memory that is genuinely retired.
- `.knowledge/wiki/runs/2026-07-24-knowledge-gardener-sweep.md` — the inaugural run-report for this
  procedure.
- `ub-knowledge-layer-e4s.5` — the unblock task (this repo's MCP issue tracker) that specified and landed
  this runbook, the last child of the `.knowledge` layer epic `ub-knowledge-layer-e4s`.
