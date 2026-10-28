---
name: 2026-10-28-ub-vcp-spawn-lock
description: Serialising every child spawn and io::pipe in the unblock-cli integration-test binaries behind one SPAWN_LOCK (tracker ub-vcp) — macOS std creates pipes with pipe() then FIOCLEX, so a concurrent sibling spawn could inherit another cell's read end and suppress the EPIPE the broken-pipe cells depend on; a source-scan gate keeps new spawns locked and insta no longer forks cargo metadata.
type: run
date: 2026-10-28
branch: ub-vcp-spawn-lock
pr: '468'
issues: [ub-vcp]
---

# Run — one process-wide spawn lock for the unblock-cli test harness

## Context

Issue `ub-vcp`: the `McpClient` broken-pipe witness hung about 1 run in 58. The run went through the full
lifecycle on branch `ub-vcp-spawn-lock` in the worktree `../unblock-ub-vcp`. PR: #468. Design Review passed in round 2 (round 1 FAIL; round 2 three PASS-WITH-MUST-FIX,
folded into plan rev 3). Verify passed in round 2 (round 1 PASS-WITH-MUST-FIX). Team: one coordinator,
one implementer, and three review lenses per gate.

## What & why

Understand read the Rust 1.96.0 std source. On Apple targets `sys/pipe/unix.rs` creates a pipe with
`pipe()` and then sets `FIOCLEX` in a separate call, and `posix_spawnp` runs without
`POSIX_SPAWN_CLOEXEC_DEFAULT`. A sibling spawned between the two calls inherits both pipe ends. An
inherited READ end means the child never gets `EPIPE`, which is what the D38/D50 witness waits for.
`ub-46o` already turned a hang into a 20-second deadline failure, so this change reduces flakes.

Decide (Miguel, 2026-10-28):
- Lock ALL spawns in the test harness, not only `McpClient::spawn_with_args` as the issue body
  proposed. Workspace init, `send_signal`, `json_report` and the fabricated `sh` children also fork.
- Guard new unlocked spawns with a source-scan test, not a clippy lint (`disallowed_methods` would hit
  legitimate spawns in other crates).
- Prove the mechanism at Verify with a throwaway DYLD reproducer that is not committed.

Specs read: PRD D38 and D50 clause 11 (no edit: product behaviour is unchanged), the
`implementation-plan.md` T3.2.1/T3.13 records (no edit), and the `unblock-cli` crate plan test table,
which gained three rows before any code.

## Outcome

- **Landed:** `tests/common/mod.rs` gains `SPAWN_LOCK` and the `spawn_locked`/`output_locked`/
  `status_locked`/`pipe_locked` helpers. The lock is held across `spawn`/`pipe()` only and is
  poison-tolerant. All 74 dispatch sites and 3 `io::pipe` sites are routed through it. The shared lexer
  moved from `no_git_gate.rs` to `tests/source_scan/mod.rs` and now carries string state across lines.
  `tests/spawn_lock_gate.rs` (27 cells) is the new gate. `.cargo/config.toml` sets
  `INSTA_WORKSPACE_ROOT`.
- **Design Review:** round 1 FAIL with 8 must-fixes (incomplete inventory, `output_locked` must not force
  stdio, `io::pipe` under the lock, the error API, the guard choice, the thread-spawn classification,
  a mechanism proof, the roadmap scope). Round 2 had 5 must-fixes (MF-a to MF-e); rev 3 folded them in.
- **Verify round 1:** 1 must-fix, the d44 F9 trip below. Back to Implement.
- **Verify round 2:** PASS. d44 OK, `spawn_lock_gate` 27 passed, fmt and clippy clean, every
  `cargo test -p unblock-cli` binary green.
- **Reproducer, 10 runs per mode on a quiet machine:**
  - Unlocked stdio and unlocked `io::pipe`: the sibling held both ends in 10/10 runs, and the write got
    no `EPIPE`.
  - Locked stdio and locked `io::pipe`: the sibling held neither end in 10/10 runs, and the write got
    `EPIPE`.
- **Insta tripwire:** on `main` insta forks `cargo metadata` inside the test binary; on the branch it
  does not fork (8/8 passed).
- **Smoke only:** `cargo insta test --check` green, and 10 whole-file `mcp_lifecycle` runs green.

## Gotchas

- **Shared-tree incident:** the implementer's first edits landed in `main`'s shared tree, not in the
  worktree (PROCESS §4). They were moved into the worktree, the three files were restored there, and
  the shared tree is clean. An earlier hand edit on `main` during Understand was also reverted.
- **d44 F9 trip:** the phrase "non-atomic" in `docs/plans/crates/unblock-cli.md:95` matched
  `scripts/checks/d44-create-deps-claims.sh` rule F9 (retired create-with-deps framing). The script
  failed on the branch and passed on `main`; the row was reworded.
- **Insta cargo-metadata fork:** insta 1.48 (`src/env.rs:487-494`) runs `cargo metadata` through an
  unlocked `Command::output()` on the first snapshot assert in each binary unless
  `INSTA_WORKSPACE_ROOT` is set at compile time. In `init_agents` that child could inherit a
  broken-pipe cell's read end. The `[env]` entry in `.cargo/config.toml` removes the fork.
- **Reproducer under load:** the unlocked modes showed inheritance in only 18 of 20 runs under load,
  not 20 of 20, because the stall window races the scheduler. Count the quiet-machine runs as the proof.
- `assert_cmd`'s `OutputOkExt` adds `.unwrap()`/`.ok()` to a std `Command`, and both spawn. So the gate
  bans every `assert_cmd` path except `cargo::CommandCargoExt` and `cargo::cargo_bin`.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| SPAWN_LOCK | the one process-wide mutex every test spawn and `io::pipe()` takes, held across `spawn`/`pipe()` only | `crates/unblock-cli/tests/common/mod.rs` |
| sanctioned line | one of the two lines marked `// SPAWN_LOCK: sanctioned` directly under `let _guard = spawn_guard();`, the only dispatch the gate allows | `crates/unblock-cli/tests/common/mod.rs`, `crates/unblock-cli/tests/spawn_lock_gate.rs` |
| reproducer | the throwaway DYLD_INSERT_LIBRARIES program that stalls between `pipe()` and `FIOCLEX`, spawns a sibling and checks with `lsof` for the inherited fd; not committed | `ub-vcp` comments 329, 333 |
| F9 | rule F9 of the d44 doc-lint script, which rejects the retired create-with-deps framing (here, the word "non-atomic") | `scripts/checks/d44-create-deps-claims.sh` |
| MF-a to MF-e | Design Review round-2 must-fixes: insta env, `assert_cmd` hole, roadmap.html, EPIPE attribution, `#[track_caller]` | `ub-vcp` comment 330 |
| round 1/2 | successive iterations of the design Review or Verify gate | `ub-vcp` issue comments |
| plan rev 3 | the Spec/Plan revision after Design Review round 2 | `ub-vcp` comment 331 |

## Links

- `ub-vcp`: `McpClient` child spawns could inherit another cell's pipe ends on macOS.
- Prior: `ub-46o` (the 20-second `McpClient` read deadline), `ub-f1k` (the duplex read deadline).
- Files: `crates/unblock-cli/tests/common/mod.rs`, `crates/unblock-cli/tests/source_scan/mod.rs`,
  `crates/unblock-cli/tests/spawn_lock_gate.rs`, `crates/unblock-cli/tests/no_git_gate.rs`,
  `.cargo/config.toml`, `docs/plans/crates/unblock-cli.md`, `docs/plans/00-roadmap.md`,
  `docs/roadmap.html`.
- Related run-report: [2026-10-27-ub-kwh-triage-prompt](2026-10-27-ub-kwh-triage-prompt.md).
