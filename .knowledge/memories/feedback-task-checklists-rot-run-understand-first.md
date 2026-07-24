---
name: feedback-task-checklists-rot-run-understand-first
description: unblock's impl-plan T-id AC checklists carry stale file:line/site claims — always run a scout sweep before implementing, never trust the checklist
type: gotcha
---

The `docs/plans/implementation-plan.md` per-task AC blocks read as exact (file:line anchors, named goldens,
counts) but **rot silently** — they're written at spec time and the code moves underneath them. CLAUDE.md already
says "a task/plan description is never authoritative"; this is the operational proof of *how badly*.

T3.9 (2026-07-17), a 7-scout Understand sweep vs the ~91-site checklist found: 2 files to edit that **don't exist**
(`session/organization.rs`, `format_issue_long`), 1 test path that doesn't exist (`tests/capabilities.rs:191` →
really `src/resources/capabilities.rs:191`), a wrong version-flip line, a miscount (12 `impl Storage for` blocks,
not 11) with a blanket stub rule wrong for 3 of them, 4 **missed** sites, a missed golden
(`schema_snapshots__epic_status.snap`), 2 golden false-positives, and — worst — claims that were *actively
inverted*: "the D33 snapshot auto-gains the 8th tool table" (it hard-codes `[;7]` + `unwrap_or_default()` → GA
ships an empty table, silently).

**Why:** the dangerous class is the **silent** one — sites whose omission compiles clean, passes clippy, and keeps
CI green while shipping wrong software (catch-all enum arms, `unwrap_or_default()`, derived `PartialEq` quietly
absorbing a new field, re-blessing a golden whose fixture can't diff → a green tick certifying zero coverage).
No test fails. Only a source sweep finds them.

**How to apply:**
- Run **Understand (parallel scouts, one per crate + one cross-cutting for goldens/docs/CI) before any substantive
  T-id**, and have the coordinator ADJUDICATE — scouts over-claim and contradict each other (2 of 3 golden claims
  were wrong in both directions).
- Then land the corrections **spec-first** as the first commits of the task's own PR (not a separate PR — they're
  that task's interface decisions).
- Interrogate every AC for **vacuity**: "re-bless snapshot X" against a default/empty fixture is a no-op. Demand
  the fixture *before* the re-bless is meaningful.
- Expect the sweep to surface genuine **spec forks** the cascade missed → take them to Miguel
  ([[feedback-epic-decision-closure]]).
