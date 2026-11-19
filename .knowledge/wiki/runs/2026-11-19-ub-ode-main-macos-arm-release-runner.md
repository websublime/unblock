---
name: 2026-11-19-ub-ode-main-macos-arm-release-runner
description: Porting the aarch64-apple-darwin release-runner pin to main (tracker ub-ode AC 3) — the macos-15 custom-runner key and ci-cd §3.1 note that shipped v1.0.3 on release/v1.0.x now land on main, so v1.1.0 does not plan the retired macos-14 image.
type: run
date: 2026-11-19
branch: ub-ode-main-macos-arm-release-runner
pr: pending
issues: [ub-ode]
---

# Run — port the Apple Silicon release-runner pin to main (ub-ode)

## Context

`ub-ode` (P1, dist 0.32.0 plans `aarch64-apple-darwin` on the retired `macos-14` runner), acceptance
criterion 3: `main` must carry the pin that pull request #479 landed on `release/v1.0.x`. Implement phase,
single pass, on branch `ub-ode-main-macos-arm-release-runner` cut from `origin/main` at `b1c653c`. Pull
request into `main` pending. Solo implementer under a coordinator.

## What & why

On `release/v1.0.x` the pin unblocked v1.0.3: rc.1's `Release` run 44522869682 was cancelled, while
v1.0.3-rc.2 (run 44531801963) and v1.0.3 (run 44535938580) built all five targets with `aarch64-apple-darwin`
on `macos-15`. `main` still had dist's default, so v1.1.0 would hit the same cancellation. Spec read:
ci-cd §3.1 (the dist config and the D17 amendment note), `RELEASING.md` (it already names `macos-15` for the
`aarch64-apple-darwin` smoke leg, so it needed no change), roadmap v1.0.3 subsection (it names no release
runner, so it needed no change).

## Outcome

Landed: the ci-cd §3.1 note (ported from the 1.0 line, plus a sentence that v1.0.3 shipped with the pin),
then the `dist-workspace.toml` hunk verbatim from `7b2e556`, then this report, then the tracker re-export.
Verify pending: the pull request's `Release` `plan` job must show `macos-15` for `aarch64-apple-darwin`.

## Gotchas

- The `release.yml` workflow on `main` needs no regeneration: runners come from the `plan` job's matrix at
  run time.

## Glossary

This run coined no ids. The row below is a session-local id that appears in `ub-ode`'s tracker comments.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| F6 | D29 clause: the header is read before any connection opens, and doctor never closes a descriptor on a session file; named in the `exclusivity (macOS, D29 clause F6)` required check | PRD §4 D29; ub-ode comment 467 |

## Links

- `ub-ode` — dist 0.32.0 builds `aarch64-apple-darwin` on the retired `macos-14` runner.
- `ub-kpe` — the v1.0.3 maintenance patch whose rc.1 release exposed it.
- Files: `/dist-workspace.toml`, `/docs/plans/ci-cd-and-distribution.md` §3.1, `/.unblock/issues.jsonl`.
- Prior run: [2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix](2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix.md).
