---
name: project-workflow-flat-schema-for-coordinator
description: Workflow coordinator agents with deeply-nested object output schemas hit the StructuredOutput retry cap and fail the whole run; use FLAT schemas and have agents write files first
type: gotcha
---

When orchestrating unblock lifecycle phases as `Workflow` runs, a coordinator/synthesis `agent({schema})`
whose schema contains **deeply-nested objects** (e.g. `decision_forks: [{id, question, options[], recommendation, ...}]`)
can exceed the **StructuredOutput retry cap (5)** and fail the ENTIRE workflow with
`TelemetrySafeError: StructuredOutput retry cap (5) exceeded`.

**Why:** Prefer FLAT return schemas — top-level fields that are strings, enums, booleans, and **arrays of
strings** (not arrays of objects). The T2.5 Understand workflow died on a nested `decision_forks` object
array; the T2.5 design-Review workflow used flat `must_fix: string[]` / `should_fix: string[]` / `verdict: enum`
and succeeded cleanly.

**How to apply:**
- Always have workflow reader/coordinator agents **WRITE their full output to a scratchpad file** and RETURN only
  a bounded flat summary (this is also PROCESS.md §4's "avoid the schema-output loop"). Then the real artifact
  survives even if the structured return fails.
- If a workflow fails on the retry cap, the agents likely **already wrote their files** — recover by reading the
  scratchpad `.md` files directly (the DOSSIER/review files were all on disk despite the failure). Do NOT re-run.
- Keep coordinator schemas flat: encode structured findings as `string[]` with a `file:line — one sentence` convention
  rather than object arrays.

Related: [[project-unblock-rust-rewrite]], [[feedback-implementer-probe-must-include-cargo-fmt]].
