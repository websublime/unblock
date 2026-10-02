---
name: 2026-10-02-update-live-smoke
description: Authoring the live unblock update smoke (tracker ub-lp9.26) — a manual workflow_dispatch workflow with one native-runner leg per shipped triple driving the real dist installers, a passing hand rehearsal v1.0.0-rc.6 to v1.0.0 on aarch64-apple-darwin, and the issue left open until the v1.0.0 to v1.0.1 acceptance run.
type: run
date: 2026-10-02
branch: ub-lp9.26-update-smoke
pr: -
issues: [ub-lp9.26]
---

# Run — the live `unblock update` smoke, authored and rehearsed

## Context

Task `ub-lp9.26` is the one non-defect item of the v1.0.1 slot. Until now the self-update path
(axoupdater, then the dist installer, then the SHA256 check, then the swap) had been asserted only
by hermetic stand-ins and had never run against a real published release. The session was solo,
with two decisions put to Miguel. The branch is `ub-lp9.26-update-smoke` off `main` 1c179e2. The
issue stays `in_progress`, because its acceptance run needs v1.0.1 final, which is not published
(the newest tag is `v1.0.1-rc.3`).

## What & why

These sections were read rather than restated here: `docs/plans/ci-cd-and-distribution.md` §3.1
(the `unblock-cli` App name and receipt) and §4 (self-update, NFR-17), the roadmap v1.0.1 slot, the
boundary note in `crates/unblock-cli/tests/update_verify.rs`, and `crates/unblock-cli/src/commands/update.rs`.

Miguel decided the two forks the issue reserved for him. For where the smoke lives, he chose a
manual `workflow_dispatch` workflow plus a runbook step, so nothing runs on a pull request and the
`no-network` scan is untouched. For scope, he chose every shipped triple on a native runner, with
Windows ARM64 hosts named as uncovered (D36). He also approved a rehearsal on two releases that
were already published, so the scripts could be proven before the cut.

## Outcome

### What landed

- `ci(update-smoke)` 4957bc3 adds `.github/workflows/update-smoke.yml` and
  `scripts/release/update-smoke.sh` / `.ps1`. Each leg installs N with the real dist installer
  into an isolated prefix, using `UNBLOCK_CLI_INSTALL_DIR`, `XDG_CONFIG_HOME` and
  `UNBLOCK_CLI_NO_MODIFY_PATH`. It then checks four things: `update --dry-run` reports N+1 and the
  binary stays byte-identical; `update` swaps; the swapped binary reports N+1 and serves `version`,
  `migrate` and `doctor` on a workspace N created; and a second dry-run reports up to date.
- `docs(release)` 5c4c4b0 covers four documents. `RELEASING.md` gains section 5, the
  after-publish step. ci-cd §4 names the workflow, the covered and uncovered triples and the
  non-goals. The roadmap bullet is reconciled. The `update_verify.rs` note now points at the
  workflow instead of "a future v1.0.1".

### Measured

- The rehearsal ran `scripts/release/update-smoke.sh v1.0.0-rc.6 v1.0.0` on aarch64-apple-darwin
  and passed. The rc.6 installer wrote a receipt (`cargo-home` layout, `websublime/unblock`,
  version `1.0.0-rc.6`). The dry-run printed `update available: 1.0.0`, with sha256 unchanged at
  `2f64cc06…`. The update printed `updated to v1.0.0`, the sha256 became `a0ce085e…` and the
  receipt now records version `1.0.0`. The swapped binary printed `1.0.0`; `migrate` and `doctor`
  were healthy, and the second dry-run printed `already up to date`. The full transcript is on
  `ub-lp9.26`, comment 260.
- These gates exited 0: `cargo xtask doc-lint`, `verify-pins`, `no-network` and `knowledge-lint`,
  every `scripts/checks/*.sh`, and the knowledge-layer invariants.

### Not yet run

- The acceptance run, `v1.0.0 → v1.0.1` on every leg, which happens once v1.0.1 is published.
- The Windows (`update-smoke.ps1`) leg, which has never executed. There is no `pwsh` on the
  authoring machine, and `workflow_dispatch` becomes available only once the workflow is on `main`.

## Gotchas

- The dist installers and axoupdater 0.10.0 both check `XDG_CONFIG_HOME` before
  `~/.config` or `LOCALAPPDATA` (`receipt.rs` `get_config_paths`). So that one variable isolates
  the receipt on every platform without touching `HOME`.
- `unblock update` always targets the latest stable release, so the smoke can verify N+1 only
  while N+1 is latest. Pre-releases are skipped, which is why rc.6 → 1.0.0 worked as a rehearsal
  while `v1.0.1-rc.3` existed.
- In PowerShell, `Write-Output` inside a function goes into its return value, which would have
  turned every exit-code comparison into an array filter. The script's transcript uses
  `Write-Host` for that reason.
- GitHub retired the `macos-13` image, so the x86_64 macOS leg uses `macos-15-intel`.

## Glossary

No session-local ids were used in this run.

## Links

- `ub-lp9.26` — the live `unblock update` path had never run against a real published release.
- Pull request — none yet, opened from this branch; merging is the human gate.
- Key files touched —
  `/Users/ramosmig/Public/WS-Labs/unblock/.github/workflows/update-smoke.yml`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/release/update-smoke.sh`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/release/update-smoke.ps1`,
  `/Users/ramosmig/Public/WS-Labs/unblock/RELEASING.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/ci-cd-and-distribution.md` (§4).
