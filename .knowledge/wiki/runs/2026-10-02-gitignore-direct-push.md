---
name: 2026-10-02-gitignore-direct-push
description: Recording the direct push of 87f713e (.gitignore) to main before the v1.0.1 cut (tracker ub-kpu) — the pull-request bypass was intended, but the run-report gate had already blocked locally and the command chain pushed anyway, so main took a substantive commit without a run-report; this report closes that gap.
type: run
date: 2026-10-02
branch: gitignore-direct-push-record
pr: -
issues: [ub-kpu]
---

# Run — the `.gitignore` direct push to main

## Context

Task `ub-kpu` records one commit, 87f713e, pushed straight to `main` on 2026-10-02 at 15:14 +0100
on Miguel's instruction. It came just before he cut v1.0.1 with `cargo xtask release`. Solo
session. This report lands through a normal pull request from `gitignore-direct-push-record` off
`main` 786a7ae.

## What & why

These were read rather than restated here: `RELEASING.md` section 1 (the release pre-flight),
`xtask/src/release.rs` (`working_tree_clean` runs `git status --porcelain`), and PROCESS.md
section 8 with ci-cd §2.3.3 (the same-commit rule and the run-report gate).

The pre-flight of `cargo xtask release` refuses a non-empty `git status --porcelain`, and that
output includes untracked files. The maintainer's per-machine `.claude/settings.local.json` was
untracked, so the cut could not start. Miguel ruled that it be ignored and pushed directly to
`main`. The commit adds `/.claude/settings.local.json` to `.gitignore` next to the existing
`/.claude/worktrees/` rule; the shared `.claude/settings.json` stays versioned.

## Outcome

- The push succeeded under the maintainer's bypass permission. GitHub logged two bypassed rules
  for `refs/heads/main`. "Changes must be made through a pull request" was intended. The required
  check `run-report-gate` was not.
- The gate had been run locally first, and it blocked: `.gitignore` is not in any trivial class, so
  the diff is substantive and needs a run-report. The command chain joined the gate and the push
  with `;` instead of `&&`, so the push ran anyway.
- No later pull request is affected, because the gate compares a branch against its base. The gap
  was the missing record on `main`, and this report closes it.
- The v1.0.1 pre-flight then passed, and the release was cut and published the same afternoon.
- The tracker re-export in this pull request also carries the closes of `ub-lp9.26` and
  `ub-lp9.18`, and the v1.0.2 triage. That triage added `ub-e47` (the `RATE_LIMITED` ruling) and
  moved `ub-lp9.28`, `ub-f1k` and `ub-q1u` into the slot.

## Gotchas

- Any gate that runs right before an irreversible step must sit in an `&&` chain or have its
  status checked; a `;` turns a red gate into a log line.
- `git status --porcelain` counts untracked files, so any per-machine file left in the checkout
  blocks `cargo xtask release`. Ignoring the file is the durable fix; a local
  `.git/info/exclude` entry is the per-machine alternative.

## Glossary

No session-local ids were used in this run.

## Links

- `ub-kpu` — the direct push of 87f713e that bypassed the pull-request rule and the run-report gate.
- Pull request — none yet, opened from this branch; merging is the human gate.
- Key files touched — `/Users/ramosmig/Public/WS-Labs/unblock/.gitignore` (by 87f713e, already
  on `main`).
- Prior related run-report — `runs/2026-10-02-update-acceptance-run.md`, the release this push
  unblocked.
