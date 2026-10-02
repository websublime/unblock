---
name: 2026-10-02-update-acceptance-run
description: The v1.0.0 to v1.0.1 live unblock update acceptance run (tracker ub-lp9.26) — v1.0.1 published, the update smoke passed on all five triples (four in CI, aarch64-apple-darwin by hand after two anonymous GitHub rate-limit 403s that exposed ub-jh5), the D46 migration ran on a GA-created database, and the docs and runbook stop describing the run as future.
type: run
date: 2026-10-02
branch: ub-lp9.26-record-acceptance-run
pr: -
issues: [ub-lp9.26, ub-jh5]
---

# Run — the v1.0.0 → v1.0.1 live update acceptance run

## Context

This is the second half of `ub-lp9.26`. The first half,
`runs/2026-10-02-update-live-smoke.md` (pull request 453), authored the smoke and rehearsed it. The
issue could close only on a run against two real releases, so this session waited for the
maintainer to cut v1.0.1. Solo session; the branch is `ub-lp9.26-record-acceptance-run` off `main`
6b03e88.

## What & why

These were read rather than restated here: ci-cd §4, `RELEASING.md` section 5, and the
`ub-lp9.26` acceptance criteria. After the run, ci-cd §4, the roadmap bullet and the published
`docs/roadmap.html` still described the acceptance run as future and v1.0.1 as planned. The issue
names exactly that kind of drift as forbidden, so recording the run is part of closing it.

## Outcome

### The run

- v1.0.1 was published at 14:28:15Z as `Latest`. `release.yml` run 37018763544 passed every job,
  and `gh attestation verify` on the aarch64-apple-darwin archive exited 0.
- `gh workflow run update-smoke.yml -f from_tag=v1.0.0 -f to_tag=v1.0.1` became run 37020158075. It
  passed on x86_64-apple-darwin, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu and
  x86_64-pc-windows-msvc; the last was the first execution of the powershell script.
- aarch64-apple-darwin failed twice (the original run and one re-run) at `update --dry-run` with
  `403 rate limit exceeded`. It then passed by hand with the same script on an aarch64-apple-darwin
  machine. Per that hand run: the dry-run reported 1.0.1 with the binary byte-identical, the update
  swapped (sha256 a0ce085e… → 50e413a6…), and the swapped binary printed 1.0.1. `migrate` on the
  v1.0.0 workspace applied schema 1 → 2, and `doctor` reported healthy at schema 2. The full
  transcript is `ub-lp9.26` comment 265.

### What landed

- ci-cd §4 records the run; the roadmap bullet says the acceptance run passed.
- `RELEASING.md` step 3 names the anonymous rate-limit 403 as the one red-leg exception and says to
  re-run that leg by hand.
- `docs/roadmap.html` marks the post-GA smoke done, rewords the "never run end-to-end" note and the
  v1.0.1 list item, and changes v1.0.1 from "Patch · planned" to "Patch · released".

### Measured

`cargo xtask doc-lint` and `knowledge-lint`, every `scripts/checks/*.sh` and the knowledge-layer
invariants passed.

## Gotchas

- The 403 was not flakiness. v1.0.0 and v1.0.1 never send a GitHub token, so every query is
  anonymous, and shared macOS runners exhaust the per-IP limit. Filed and fixed as `ub-jh5`
  (pull request 455, `runs/2026-10-02-update-github-token.md`). The fix helps the CI smoke only once
  `from_tag` carries it.
- `docs/roadmap.html` is outside the run-report gate's trivial classes, so any edit to it needs a
  run-report even when the change is a status label.

## Glossary

No session-local ids were used in this run.

## Links

- `ub-lp9.26` — the live `unblock update` path had never run against a real published release.
- `ub-jh5` — `unblock update` never sent a GitHub token; found by this run, fixed in pull request 455.
- Pull request — none yet, opened from this branch; merging is the human gate.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/RELEASING.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/ci-cd-and-distribution.md` (§4),
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/00-roadmap.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/roadmap.html`.
- Prior related run-reports — `runs/2026-10-02-update-live-smoke.md`,
  `runs/2026-10-02-update-github-token.md`.
