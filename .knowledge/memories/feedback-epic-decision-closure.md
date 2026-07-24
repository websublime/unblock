---
name: feedback-epic-decision-closure
description: When an architect/research surfaces design options inside an epic, push for a decisive choice and spec-close the question; do not spawn more beads for every edge case
type: gotcha
---

When research or architect work surfaces design alternatives inside an epic, the default reflex should be: present options, recommend one, close the question in the spec, and move on. Do NOT spawn new discovery beads or split sub-decisions into additional sub-beads.

**Why:** a couple of related epics started with 14 tasks and grew to 70+ because every review/research cycle surfaced new findings that became new beads instead of being folded into the next decision. This was explicitly named as a loop to break. Endless decomposition is the failure mode to avoid, not the safety net.

**How to apply:**
- When dispatching architect after research, include ALL pending design decisions in the dispatch prompt so the architect produces a complete spec patch in one pass — not an iterative negotiation.
- When a research agent recommends "don't extend X", always verify the premise before accepting it; research findings can be stale or wrong (e.g., a platform feature that GA'd later than the research assumed).
- Prefer spec-level closure ("Exhaustiveness Rationale" paragraphs, explicit decision memos in the spec) over new follow-up beads for meta-questions like "should this pattern be universal?".
- When presenting options, give a concrete recommendation with tradeoffs — do not force arbitration of every microdecision without guidance.
