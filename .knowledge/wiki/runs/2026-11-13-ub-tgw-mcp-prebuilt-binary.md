---
name: 2026-11-13-ub-tgw-mcp-prebuilt-binary
description: Starting the repo's own tracker MCP server from the prebuilt debug binary instead of cargo run (tracker ub-tgw, with ub-00w folded in) — a cold compile that overran the client's startup timeout and a second cargo process on a machine capped at one became a git-common-dir shell wrapper binding the main checkout's tracker from any subdirectory or worktree, after a design gate found the first wiring relied on an expansion Claude Code performs from its own environment, a second round found the PRD still claiming the repo passes no --dir, and Verify read a CI run whose only red jobs were pre-existing advisories and the pending run-report.
type: run
date: 2026-11-13
branch: ub-tgw-mcp-prebuilt-binary
pr: '473'
issues: [ub-tgw, ub-00w]
---

# Run — the tracker MCP server execs the prebuilt binary (no cargo run)

## Context

Issue `ub-tgw`: a chore to stop the repo's own `.mcp.json` from starting the tracker MCP server through
`cargo run`. It was raised by Miguel during the `ub-lp9.3` session (that session's main task) because the
server's builds were loading the machine, and it shipped as its own pull request. `ub-00w` (a chore about
the D39 PRD text claiming `${CLAUDE_PROJECT_DIR}` is not expanded in `.mcp.json`) was folded into it at
design Review round 2. Branch `ub-tgw-mcp-prebuilt-binary`, off main `121c8b7`, in the worktree
`../unblock-tgw`; draft PR #473 carried one WIP commit (`67ad2d0`) for the Verify CI run, re-cut into
atomic commits at Track. Design Review passed in round 3 (rounds 1 and 2 PASS WITH CHANGES; round 3 was a
delta re-verification by the edge-cases/CI lens alone, the other two lenses having passed in round 2).
Verify passed in round 1 (PASS WITH NOTES under all three lenses, zero must-fixes). Team: the
orchestrator, one implementer, three design Review lenses, three Verify lenses and one Track agent.

## What & why

`.mcp.json` ran `cargo run --bin unblock mcp`. Earlier run-reports record three failures caused by it: the
client's 30-second startup timeout expiring on a cold build (the 2026-08-07 code-graph run and the
2026-09-29 init-agents run), the server start waiting on the shared `target/` build lock held by gate
builds (the 2026-10-23 ub-q2v run), and a second concurrent cargo process breaking the one-cargo-at-a-time
load rule of `docs/PROCESS.md` §4. Miguel decided on 2026-11-13 to build once and exec the compiled debug
binary, and to state the build-first duty in PROCESS.md. Authoritative text read: PROCESS.md §4 and §6,
PRD §4 D39 (workspace discovery and `CLAUDE_PROJECT_DIR`) and its bullet at `docs/PRD.md:330-333`, D13 (no
automatic git operations in the product), the Claude Code MCP documentation on environment-variable
expansion in `.mcp.json`, and the CLI's global `--dir` flag (`crates/unblock-cli/src/cli.rs:49`).

## Outcome

Landed over `121c8b7`, re-cut at Track into atomic commits, spec first:
- `docs(prd)` — two inline D39 addenda (`docs/PRD.md:129` and `:333`): Claude Code expands `${VAR}` /
  `${VAR:-default}` in `.mcp.json` from its own environment, and the repo's `.mcp.json` now computes
  `--dir` at runtime in a dev-harness wrapper (git used only by that wrapper, D13 untouched). No new D-id.
  This resolves `ub-00w`; the original sentence at `:330` stays, amended by the note.
- `chore(mcp)` — `.mcp.json` execs `sh -c` with a wrapper that resolves the main checkout from
  `git rev-parse --path-format=absolute --git-common-dir`, then execs that checkout's
  `target/debug/unblock --dir <main>/.unblock mcp`; PROCESS.md §6 gains the build-first bullet
  (`cargo build -j 4 --bin unblock` in the main checkout, alone, before a session and after a pull or branch
  switch; `/mcp` → reconnect applies a rebuild) and §4 a clause keeping worktree builds in their own
  `target/`. The same commit corrects README.md:148-151 on how Claude Code expands `${…}` and updates the
  dogfood-tracker and D39 memories to the wrapper, its failure symptoms, the stale-binary check and
  PROCESS.md's inbound pointer.
- `docs(wiki)` — this report and its index entry.

Design Review: round 1 PASS WITH CHANGES under all three lenses (one shared must-fix: the mechanism),
then Miguel's ruling on the wiring; round 2 two PASS and one PASS WITH CHANGES (one must-fix: the PRD
lines); round 3 PASS. Verify (CI run 43412148480, head `67ad2d0`): three PASS WITH NOTES, zero must-fixes;
16 CI jobs green; red were `audit` and `deny` (pre-existing, see Gotchas) and `run-report-gate` (this
report, pending until Track). Two Verify notes were applied at Track: README.md:149 "is not set" became
"is normally absent", and the dogfood memory names the diagnostics `version` kind (`{"kind":"version"}`
returns `mcp_contract_version`) instead of an action. The tracker re-export rides as its own commit on
the same branch.

## Gotchas

- The first wiring, `sh -c 'exec "${CLAUDE_PROJECT_DIR:-.}/target/debug/unblock" mcp'`, assumed the shell
  expands the token from the child's environment. Claude Code expands `${VAR}` and `${VAR:-default}` in
  `.mcp.json` itself, from its own environment, where `CLAUDE_PROJECT_DIR` is normally absent; the shell
  therefore always received `./target/debug/unblock`, resolved against the session's launch directory.
  It worked only at the main-checkout root: a subdirectory or worktree session failed with "Failed to
  connect — CONNECTION_CLOSED" (design Review round 1, all three lenses, probed with Claude Code 2.1.282).
- Round 2 found `docs/PRD.md:129` (the D39 row) and `:332-333` still stating that the repo's own MCP config
  passes no `--dir`, which the new wrapper made false. `ub-00w`, already open for the expansion sentence
  at `:330`, was folded in and the PRD fixed in the same pull request as an inline D39 addendum.
  `ub-00w`'s acceptance criterion asks for zero hits of "does NOT expand"; under the addendum convention
  `:330` keeps that sentence on purpose.
- CI `audit` and `deny` were red on new RustSec advisories against the lockfile inherited from `121c8b7`:
  RUSTSEC-2026-0462 (rmcp 1.7.0) and RUSTSEC-2026-0486 (rustix 1.1.4), published after main's last green
  CI on 2026-11-02. `Cargo.lock` was byte-identical between base and head; tracked as `ub-9ov`. On GitHub
  only `run-report-gate` is a required status check, so red `audit`/`deny` would not mechanically block a
  merge.
- A round-1 review probe ran the main checkout's binary with its working directory inside `unblock-tgw`,
  which walked up to the worktree's `.unblock/` and created `unblock.db` and `.write.lock` there (both
  gitignored). They were deleted; that worktree's `.unblock/` again holds only `config.toml` and
  `issues.jsonl`. Under the old `cargo run` wiring a worktree session bound such an empty database silently.
- Outside a git repo the wrapper's empty substitution resolves the root to `/`, so the error names
  `//target/debug/unblock`; git's `fatal:` line above it is the real cause. On macOS `/bin/sh` a missing
  binary exits 126 "cannot execute: No such file or directory" (dash on Linux: 127 "not found").

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| round 1 / 2 / 3 | One pass of the design Review gate; round 3 was a delta re-verification by one lens | ub-tgw comments 364, 366, 367 |
| revision 2 / iteration 1 of 2 | The implementer's second revision, sent back from Review round 2; the iteration count against the two-iteration budget before escalation to Miguel | ub-tgw comments 364, 366; docs/PROCESS.md §5 |
| wiring correctness / process-docs / edge-cases-CI | The three design Review lenses (code-reviewer, project-manager, qa-expert) | ub-tgw comment 364 |
| code-review / qa / rust-engineer | The three Verify lenses | ub-tgw comment 369 |
| PASS WITH CHANGES / PASS WITH NOTES | Gate verdicts: pass with required edits before the next step / pass with non-blocking notes | ub-tgw comments 364, 369 |
| AC (1)–(4) | ub-tgw's numbered acceptance criteria: no cargo in `.mcp.json` plus a v1.12 handshake; the build-first duty; no live `cargo run` claim; lints, gate, report and re-export | ub-tgw acceptance criteria |
| dev wrapper | The `sh -c` script in `.mcp.json` that resolves the main checkout from the git common dir and execs its prebuilt binary with `--dir` | .mcp.json:5 |
| D39 addendum | The inline italic note `[D39 addendum, ub-tgw, 2026-11-13]` amending D39 without a new D-id | docs/PRD.md:129, docs/PRD.md:333 |
| CI run 43412148480 | The pull_request CI run on head `67ad2d0` the Verify gate read | PR #473 checks |
| known-pending | An item the Verify gate reports as owned by the later Track step instead of failing the change for it | docs/PROCESS.md §6 |

## Links

- `ub-tgw` — exec the prebuilt tracker binary from `.mcp.json` and state the build-first duty (this run);
  `ub-00w` — the D39 PRD expansion sentence, folded here; `ub-9ov` — CI `audit`/`deny` red on
  RUSTSEC-2026-0462 and RUSTSEC-2026-0486; `ub-lp9.3` — the session's main task, during which this chore
  was raised.
- PR: https://github.com/websublime/unblock/pull/473
- Key files: `.mcp.json`, `docs/PROCESS.md` (§4, §6), `docs/PRD.md` (D39), `README.md`,
  `.knowledge/memories/project-dogfood-unblock-is-the-tracker.md`,
  `.knowledge/memories/project-workspace-discovery-claude-project-dir-d39.md`.
- Prior run-reports that recorded the `cargo run` failures:
  [2026-08-07-code-graph-first-move](2026-08-07-code-graph-first-move.md),
  [2026-09-29-init-agents-and-csv-lifecycle-reports](2026-09-29-init-agents-and-csv-lifecycle-reports.md),
  [2026-10-23-ub-q2v-agents-discriminant](2026-10-23-ub-q2v-agents-discriminant.md).
