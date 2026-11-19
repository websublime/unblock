---
name: 2026-11-19-ub-ode-macos-arm-release-runner
description: Pinning the aarch64-apple-darwin release runner to macos-15 on release/v1.0.x (tracker ub-ode) — dist 0.32.0 defaults that target to the retired macos-14 image, so the v1.0.3-rc.1 release run was cancelled; a dist-workspace.toml custom-runner key fixes it with no release.yml regeneration.
type: run
date: 2026-11-19
branch: ub-ode-macos-arm-release-runner
pr: pending
issues: [ub-ode, ub-kpe]
---

# Run — v1.0.3: pin the Apple Silicon release runner (ub-ode)

## Context

`ub-ode` (P1, the release job for `aarch64-apple-darwin` cannot start), found while publishing v1.0.3-rc.1 for
`ub-kpe`. Implement phase, single pass, on branch `ub-ode-macos-arm-release-runner` cut from
`release/v1.0.x` at the v1.0.3-rc.1 release commit. Pull request into `release/v1.0.x` pending. Solo
implementer under a coordinator.

## What & why

The v1.0.3-rc.1 `Release` run 44522869682 was cancelled. dist 0.32.0 plans `aarch64-apple-darwin` on
`macos-14`, which GitHub has retired ("The configuration 'macos-14-arm64-us-default' is not supported"). The
other four targets built with `--package=unblock-cli`. No release could ship from the 1.0 line until the
runner moved. Spec read: ci-cd §3.1 (the dist config and the D17 amendment note), `RELEASING.md` §3 (it
lists only the `update-smoke.yml` legs, which already name `macos-15`, so it needed no change), PRD D17
(it names no runner). The key shape `[dist.github-custom-runners] aarch64-apple-darwin = "macos-15"` comes
from the dist book, Customizing GitHub Actions, Custom runners.

## Outcome

Landed: a dated ci-cd §3.1 note, then the `[dist.github-custom-runners]` table in `dist-workspace.toml`,
then the tracker re-export, then this report. `release.yml` takes `runs-on: ${{ matrix.runner }}` from the
`plan` job's `artifacts_matrix`, so the key needs no workflow regeneration. Verify pending: the pull
request's `Release` `plan` job must show `macos-15` for `aarch64-apple-darwin`. Track pending: v1.0.3-rc.2
must build all five targets, and `main` must carry the same pin before v1.1.0.

## Gotchas

- The local `dist` is 0.30.2, not the pinned 0.32.0, so `dist plan` cannot be run locally to confirm the
  matrix; the pull request's `plan` job is the only check.
- The dist book's current runner table already lists `macos-15` for Apple Silicon; 0.32.0's default was
  still `macos-14`, so the book alone would suggest no fix was needed.
- The runner image does not set the binary's minimum macOS: rustc 1.96 prints
  `MACOSX_DEPLOYMENT_TARGET=11.0` for `aarch64-apple-darwin`.

## Glossary

This run coined no ids. The rows below are session-local ids from the linked `ub-kpe` tracker comments,
defined as in the ub-kpe run-report.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| A1 | Design Review 3 architect must-fix 1: "opens no descriptor" wording becomes "never closes a descriptor on it" | ub-kpe comment 451 |
| A2 | Design Review 3 architect must-fix 2: the holder's health and sync arms are cfg-gated by engine feature | ub-kpe comment 451 |
| A3 | Design Review 3 architect must-fix 3: one `SessionFiles` type | ub-kpe comment 451 |
| M1 | Design Review 3 rust must-fix 1, merged with A2 | ub-kpe comment 451 |
| M2 | Design Review 3 rust must-fix 2: spell out the non-unix arms and add a Windows compile check | ub-kpe comment 451 |
| M3 | Design Review 3 rust must-fix 3: one `pub(crate)` call returning the header and the session set | ub-kpe comment 451 |
| F3 | Existing D29 clause: `run_doctor` is pure, non-async and storage-free | PRD §4 D29 |
| F5 | Existing D29 clause: the file-state severity map and the `DatabaseNotSqlite` header check | PRD §4 D29 |
| F6 | D29 clause: the header is read before any connection opens, and doctor never closes a descriptor on a session file | PRD §4 D29 |
| R1 | The ruled way the ub-kpe fix PR passes `run-report-gate`: a branch copy of its report | ub-kpe comment 446 |

## Links

- `ub-ode` — dist 0.32.0 builds `aarch64-apple-darwin` on the retired `macos-14` runner.
- `ub-kpe` — the v1.0.3 maintenance patch whose rc.1 release exposed it.
- Files: `/dist-workspace.toml`, `/docs/plans/ci-cd-and-distribution.md` §3.1, `/.unblock/issues.jsonl`.
- Prior run: [2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix](2026-11-19-ub-kpe-v1-0-x-doctor-probe-fix.md).
