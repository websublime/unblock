---
name: 2026-09-11-response-style-guide-landing
description: Landing docs/STYLE.md, the binding prose and code-comment style guide, and importing it from CLAUDE.md so every session loads it — a doc-only commit sent straight to main on Miguel's instruction after the ub-b1a merge, with the run-report the substantive-diff gate requires written in the same commit.
type: run
date: 2026-09-11
branch: main
pr: '-'
issues: [ub-b1a]
---

# Run — the response style guide lands in the always-on contract

## Context

A solo, doc-only commit on `main` right after the ub-b1a merge (PR 443, tip b7940bb). Miguel had
kept `docs/STYLE.md` untracked and the `@docs/STYLE.md` import line in `CLAUDE.md` uncommitted in
the shared checkout through the whole ub-b1a run; both were carried across a stash-and-restore when
`main` was fast-forwarded, and he then asked for them to be committed and pushed to `main` directly.
No team, no gate: the change is two documentation files and touches no crate, script, workflow or
plan.

## What & why

`docs/STYLE.md` binds how prose to Miguel and shipped comments and docs are written (short
declaratives, no colon-hinged sentences, no verbless fragments, expanded ids). It had been applied
retroactively to every artifact of the ub-b1a run through a scratchpad copy handed to each agent;
importing it from `CLAUDE.md` next to `docs/PROCESS.md` makes it load in every session without that
hand-off. Read before committing: `docs/PROCESS.md` sections 6 and 8 (direct commits to `main` are
a human call, and a substantive diff carries its run-report in the same commit) and
`docs/plans/ci-cd-and-distribution.md` section 2.3 (the structural substantive-diff predicate).

## Outcome

One commit, `docs(style): land the response style guide and import it into the always-on contract`,
carrying `CLAUDE.md` (one import line), `docs/STYLE.md` (93 lines) and this report with its index
line. `scripts/knowledge/run-report-gate.sh` first BLOCKED the two-file version (a doc added is a
substantive class) and passes with this report present; `knowledge-layer-invariants.sh` and
`cargo xtask knowledge-lint` are green. The `.unblock/issues.jsonl` export was not refreshed here:
`ub-b1a` closed after PR 443 merged, and that closed state rides the next task's Track re-export.

## Gotchas

- The substantive-diff predicate counts an ADDED doc as substantive even when no code moves, so a
  "just commit the two files" request still owes a run-report; writing one is cheaper than arguing
  the classification.
- A fast-forward of `main` refuses when a locally modified file is also changed by the incoming
  commits (`CLAUDE.md` was); a targeted `git stash push -- CLAUDE.md`, fast-forward, `stash pop`
  keeps the uncommitted edit intact without resetting anything.

## Glossary

No session-local ids were used in this run.

## Links

- `ub-b1a` — the task whose run (PR 443) this landing follows; its report is
  `runs/2026-09-11-startup-failure-render-bound.md`.
- Files: `/Users/ramosmig/Public/WS-Labs/unblock/CLAUDE.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/STYLE.md`.
