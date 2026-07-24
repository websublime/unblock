---
name: 2026-07-24-knowledge-gardener-sweep
description: The inaugural knowledge-gardener sweep — the first semantic consolidation pass over .knowledge/, plus standing up the gardener runbook and its weekly reminder.
type: run
date: 2026-07-24
branch: e4s.5-gardener-sweep
pr: -
issues: [ub-knowledge-layer-e4s.5]
---

# Run — the first knowledge-gardener sweep + the recurring mechanism

## Context

Task ub-knowledge-layer-e4s.5 (an unblock task id) — the knowledge gardener, the last child of the
`.knowledge` layer epic (unblock issue ub-knowledge-layer-e4s). The gardener is the SEMANTIC consolidation
layer above the automatic STRUCTURAL knowledge-lint: it marks stale pages, detects contradictions
(page-to-page and page-to-normative-doc), merges duplicates, and keeps the indexes semantically clean. This
is the INAUGURAL sweep, run right after the 49 private memories were migrated, so it doubles as the first
real validation of that content. Branch `e4s.5-gardener-sweep`, worked in an isolated worktree off the
current main. Both gates were met: the Decide was the maintainer's choice of a supervised-reminder mechanism
plus running the first sweep now; the Verify quality gate failed its first round on two real must-fixes
(below) and passed after they were fixed.

## What & why

The sweep ran as a three-lens Workflow (staleness / contradiction / duplicate-and-index) plus a coordinator
over the 49 memories and 3 run-reports; the coordinator re-verified each claim against source. Verdict was
FIXES_NEEDED. The dominant finding: eight-plus project-scope memories, migrated from a private store
spanning weeks, carried DEAD in-flight / pull-request-open / retired-tracker status snapshots that GA
v1.0.0 and the landed D39/D40/D41 decisions had since resolved. Each kept a durable lesson, so all were
fixed UPDATE-to-timeless (rewrite the status framing to the landed fact, keep the lesson) — none retired.
The sweep also caught two hard factual errors that would actively mislead a future session: a memory
claiming the doc-lint corpus was only two files (it is a fixed 19-file set) and a stale hard-coded CI-job
count (dropped, since derived counts rot in prose). In all, sixteen memories were fixed plus two index
one-liners tightened.

Three judgment calls were resolved: two agent-toolset memories disagreed on whether the research-analyst
agent can write files — the agent definitions are authoritative and it CANNOT (read-only plus web), so the
memory that recommended it as a writer role was the error and was corrected; those two memories were KEPT
separate (distinct recipes) rather than merged; and a long dated bootstrap chronicle got a superseded
banner plus a minimal fix instead of a full rewrite, preserving its origin-record value (the maintainer's
call).

Alongside the fixes, this task stood up the RECURRING mechanism: the gardener runbook as the first wiki
topic page (`.knowledge/wiki/topics/knowledge-gardener.md`, category orchestration), a pointer to it from
PROCESS section 8, and a weekly GitHub Actions cron that keeps a single standing reminder issue open. The
sweep itself stays SUPERVISED — the cron only reminds; a session runs the documented pass with a human in
the loop for the judgment calls (merges, contradiction resolutions).

## Outcome

Sixteen memories corrected, residual dead-status markers down to zero, the runbook and the reminder cron
landed, `knowledge-lint` and `doc-lint` both green. The gardener is now a documented, reminded, recurring
procedure. On this merge the last epic child closes and the `.knowledge` layer epic is complete.

## Gotchas

- UPDATE-to-timeless can accidentally drop a DURABLE lesson along with the stale status it was tangled up
  with. The Verify fidelity lens caught exactly that here — a self-update field gotcha (the release
  App-name is `unblock-cli`, not `unblock`, or `unblock update` refuses in the field) had been deleted with
  a pull-request-open progress block; it was restored as a timeless fact. Rewrites need a fidelity check
  against the pre-edit source.
- Run-reports (wiki/runs/*) are IMMUTABLE historical records — the sweep must never rewrite a past one; the
  staleness lens confirmed and left them untouched.
- The memories index is checked as data: an entry's one-liner must equal the file's frontmatter description
  and its backtick type must equal the file's type, so a description change means an index change in
  lockstep.
- Keep the sweep supervised. Its actions include merges, retirements, and picking a side on a contradiction
  — none of which should run unattended on a public repo; the cron reminds, a human-in-the-loop session
  acts.

## Glossary

No session-local id codes (short mutation-testing or must-fix labels — an uppercase letter immediately
followed by a number) appear in this report. The table records the durable references it leans on.

| id | what it is (in words) | where it lives |
|----|-----------------------|----------------|
| ub-knowledge-layer-e4s | the `.knowledge` layer epic (memories + wiki + day-1 enforcement) | the unblock tracker; git record `.unblock/issues.jsonl` |
| ub-knowledge-layer-e4s.5 | this task — the recurring knowledge gardener | the unblock tracker |
| knowledge-lint | the structural `.knowledge` lint (checks k1..k6) the gardener sits above | `xtask/src/knowledge_lint.rs` |
| the gardener runbook | the operational procedure for a sweep | `.knowledge/wiki/topics/knowledge-gardener.md` |
| D39 / D40 / D41 | landed PRD decisions (workspace discovery / EOF exit code / roadmap resequence) that resolved much of the stale status | `docs/PRD.md` §4 |

## Links

- ub-knowledge-layer-e4s.5 — the gardener task; its comment thread carries the sweep verdict and the
  judgment-call resolutions.
- ub-knowledge-layer-e4s — the parent epic; this run closes its last child.
- `.knowledge/wiki/topics/knowledge-gardener.md` — the runbook this sweep followed and documents.
- `.github/workflows/knowledge-gardener-reminder.yml` — the weekly reminder cron.
- `docs/PROCESS.md` section 8 — the pointer to the runbook.
- Prior run-reports: `.knowledge/wiki/runs/2026-07-23-knowledge-layer-landing.md`,
  `.knowledge/wiki/runs/2026-07-24-acp-hook-coverage-removal.md`,
  `.knowledge/wiki/runs/2026-07-24-memory-migration.md`.
