---
name: feedback-no-design-docs
description: In the unblock project, architect/research deliverables must be patches to docs/plans/* and docs/specs/*, never new design doc files
type: gotcha
---

Rule: Never produce standalone design documents (`*-design.md`, `rfc-*.md`, etc.) in the unblock project. All architect and research deliverables land as **patches to existing `docs/plans/*.md` and `docs/specs/*.md` files**. These two are the only canonical artifacts.

**Why:** The rule was stated explicitly (2026-04-16, during dep_cycles bead prep): "para mim exist plan e spec only. tudo deve ficar refletido nesses ficheiros." Multiple artifact types fragment source-of-truth and create drift between design intent and spec.

**How to apply:**
- When dispatching an external architect/planning agent or similar in this project, the prompt must state the deliverable is patches to the relevant `docs/plans/NN-plan-*.md` and/or `docs/specs/NN-spec-*.md` files — not a new design doc.
- Any agent/skill defaults that suggest emitting a "design doc" must be overridden with spec+plan patches.
- Exception: PRD-style inputs (`docs/unblock-prd-*.md`, `docs/unblock-architecture-*.md`) already exist at repo-level; those are pre-existing and not new artifacts. Do not invent parallel design docs beside them.
