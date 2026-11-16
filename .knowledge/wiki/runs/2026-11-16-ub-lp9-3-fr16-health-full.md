---
name: 2026-11-16-ub-lp9-3-fr16-health-full
description: FR-16 full health (tracker ub-lp9.3, D58) — a D58 taxonomy that classifies database and file-state anomalies by whether an in-product repair exists, a phased doctor --repair split into an exclusive pre-open window and an online session repair, and .unblock/.recovery/ evidence runs; it took three design-Review rounds plus two delta confirmations, SQLite measurements that overturned two written mechanisms (journal unlink at close, the read-only open rebuilding -wal/-shm), an Unsafe gate with a hot-journal exception, and a stale test that only the whole-workspace CI caught.
type: run
date: 2026-11-16
branch: ub-lp9.3-health-full
pr: '474'
issues: [ub-lp9.3, ub-2vp, ub-9ov, ub-tgw]
---

# Run — FR-16 full health: D58 taxonomy, doctor --repair and recovery evidence

## Context

Issue `ub-lp9.3`: implementation task T4.3, the v1.1 remainder of FR-16 (workspace health and
recovery), specified as the new decision D58. The run went through the full lifecycle from
2026-11-13 to 2026-11-16 on branch `ub-lp9.3-health-full`, cut from main `121c8b7`, in the isolated
worktree `../unblock-lp9.3`. PR: #474 (draft for the Verify CI runs). Phases: Understand (three
read-only lenses), three Decide rounds with Miguel, Spec/Plan in three iterations (the third
authorised by Miguel as an extra), design Review in three rounds followed by two delta
confirmations, Implement (one implementer driving five slice sub-agents, then two iterations after
Verify), Verify in two rounds plus a delta re-verification, then this Track step. Team: one
coordinator (orchestrator), three Understand lenses, one spec writer, three design-Review lenses per
round, one implementer with slice sub-agents, four Verify lenses per round, and one Track agent.

## What & why

Before this run FR-16 full was named but barely defined: the four health levels existed, but
`--repair`, the `.unblock/.recovery/` layout, a libsql meaning for Unsafe and any producer of
Drifted did not, `Session::recover()` returned a not-wired error, and the CLI `doctor` exited non-zero
only on integrity rows. Miguel's Decide rulings fixed the shape: a new D58 with an FR-16 v1.1
criterion and an implementation-plan row; Drifted redefined as derived-state drift inside the
database (child counters, duplicate metadata keys, schema stamp behind); Unsafe meaning "no in-product
repair exists" and Recoverable meaning "a repair exists"; the engine gathers database facts and hands
them as data to a classifier that stays storage-free; repair split between an exclusive pre-open
window (sidecars, checkpoint) and an online session repair (REINDEX, recount, dedupe); evidence runs
under `.unblock/.recovery/` with a consistent snapshot and a sha256 manifest; CLI only, MCP contract
unchanged at `unblock.mcp.v1.12`. The suspected v1 descriptor-close lock-loss bug was split out as
`ub-2vp`. Authoritative text: PRD §4 D58 (clauses and lettered criteria, plus its inline
amendments), the reciprocal notes on D14, D27, D29, D31 and D46, PRD FR-14 and FR-16, spine §3,
§4 and §5b, the unblock-health, -storage, -config, -engine and -cli crate plans, and
implementation-plan §5a T4.3.

## Outcome

Landed on the branch over `121c8b7` as atomic commits, spec first:
- `docs(d58)` `8d3ac10` — inline amendment: SQLite unlinks the rolled-back `-journal` at close.
- `docs(d58)` `2d75438` — criterion (g) `-wal` wording and the coupled `help_top_level` re-bless note.
- `docs(d58)` `3b6412b` — one inline amendment for the Unsafe gate, capture before touch, the
  hot-journal exception, the 64 MiB capture cap and the round-1 evidence hardening.
- `feat(storage)` `7df17ca`, `feat(config)` `dcb4a0a`, `feat(health)` `4063cd4`,
  `feat(engine,cli)` `f893061`, `feat(cli)` `7e20e2c`.

The locked spec itself was committed earlier on the branch with its tracker re-export, and the
`scripts/checks/d58-health-full-claims.sh` gate grew to cover the amendments.

Design Review: round 1 PASS WITH CHANGES from all three lenses, 21 must-fixes, treated as a loop
back; round 2 PASS WITH CHANGES with new must-fixes introduced by iteration 2; round 3 PASS WITH
CHANGES with six must-fixes (one a data-loss fork Miguel ruled on); a delta re-verification with one
lens FAIL on two text edits; a final delta confirmation PASS. Verify: round 1 FAIL from all four
lenses (four must-fixes: the stale test plus three security and blocking-I/O items); round 2 PASS
WITH NOTES from all four, no code must-fix; the delta re-verification of iteration 2 PASS from all
four. CI at the final heads: every required job green, including `test (workspace)`, snapshots,
`exclusivity-macos` (27/27) and the 250k scale job; red only on `audit`, `deny` (pre-existing,
`ub-9ov`) and `run-report-gate` (this report). Two optional hardenings were folded in before Track.

## Gotchas

- The design gate needed three rounds and two delta confirmations; the third round and the delta
  closure each required Miguel's explicit authorisation (comments 376 and 379).
- SQLite deletes a rolled-back hot `-journal` when the exclusive connection closes, although the
  written mechanism said a zeroed survivor would be renamed; measured on macOS with the bundled 3.45.1
  and system 3.50.4, then amended spec-first (ruling 385).
- SQLite's read-only open of a WAL database is not write-free: it creates an empty `-wal` and a
  32 KiB `-shm`, and rebuilds a lone `-shm`. The original sidecar bytes must therefore be captured
  before anything opens the database (ruling 390).
- The Unsafe gate (ruling 389) stops every mutating repair, but a hot `-journal` is rolled back by
  SQLite inside the window before the gate can see database classes; this became a documented
  exception (ruling 393).
- A stale assertion in `crates/unblock-cli/tests/shutdown_failure_injection.rs` (the removed
  `integrity: ok` row) passed every targeted per-commit check and failed only the whole-workspace CI
  job; afterwards every test binary of each changed crate and its dependents ran per commit.
- Agents editing through relative paths landed changes in the main checkout instead of the worktree;
  they were moved over and reverted, and one cli-plan edit was lost and re-landed.
- Provider rate limits (HTTP 429) interrupted the Spec iteration 3 writer before its summary and the
  implementer after 1h13m; both were resumed from the worktree state.
- macOS exclusivity (`locking_mode=EXCLUSIVE` refusing another live connection) was inferred from
  SQLite source, so a required `exclusivity-macos` CI job on macos-15 measures it; Windows is
  disclosed as unmeasured.
- `audit` and `deny` were red throughout on advisories already in main's lockfile (`ub-9ov`), and
  the Verify gate read them as known reds.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| T4.3 | The implementation-plan task for FR-16 full health this run delivered | docs/plans/implementation-plan.md §5a |
| F2 | D29 clause on health exposure: no MCP surface; cited as precedent for keeping the contract at v1.12 | docs/PRD.md §4 D29; ub-lp9.3 comment 362 |
| F3 | D29 clause keeping the health crate storage-free (pure classifier over data) | docs/PRD.md §4 D29; ub-lp9.3 comments 361–363 |
| F5 | D29 clause mapping anomalies to severities (Drifted as JSONL drift, conflict markers Unsafe), partly superseded by D58 | docs/PRD.md §4 D29; ub-lp9.3 comments 361–363 |
| R1 | Prefix of R1-x: Miguel's Decide round 1 rulings; R1-5 is ruling 5, CLI-only exposure with the contract unchanged | ub-lp9.3 comments 362, 386 |
| R2 | Prefix of R2-x: Miguel's Decide round 2 rulings; R2-1 is ruling 1, Unsafe vs Recoverable by repair path (unrecognised integrity rows classify Unsafe) | ub-lp9.3 comments 363, 386 |
| Decide round 1 / 2 / 3 | Miguel's three sets of numbered fork resolutions (spec shape; taxonomy and repair; review forks) | ub-lp9.3 comments 362, 363, 373 |
| fork (1)–(13) | The numbered drift/gap items and open forks found by Understand | ub-lp9.3 comment 361 |
| clause (1)–(12) | The numbered clauses of the D58 row; (6)(e) the journal step, (7) init, (8) read-only rollback, (12) the criteria | docs/PRD.md §4 D58 |
| criterion (a)–(r) | The lettered FR-16 v1.1 acceptance criteria inside D58 clause (12); e.g. (d) one integrity scan, (f) exclusivity, (g) sidecar evidence, (h) database-only init, (k) no-default-features build, (o) scale timing, (r) every commit green | docs/PRD.md §4 D58 |
| Q137–Q152, Q142, Q178–Q179, Q243–Q245, Q246–Q257, Q268–Q277 | Numbered claim rows of the d58 gate pinning, respectively, iteration-2 text, the not-SQLite quarantine record, the OrphanSidecars stop-every-process wording, ruling 385, rulings 389/390, and the iteration-2 amendment | scripts/checks/d58-health-full-claims.sh |
| Q26 / Q27 | Rows of the d56 claims gate, precedent cited in the previous config run | scripts/checks/d56-organization-claims.sh |
| architecture / rust-engineer / edge-cases-CI | The three design-Review lenses (edge-cases-CI also called qa) | ub-lp9.3 comments 372, 375, 378 |
| code-review / qa / rust-engineer / security | The four Verify lenses; security added because the change touches storage and repair | ub-lp9.3 comment 387 |
| architect / rust-engineer / research-analyst | The three read-only Understand lenses | ub-lp9.3 comment 361 |
| round 1 / 2 / 3, delta | Passes of a gate; a delta re-verifies only the fixed items | ub-lp9.3 comments 372–381, 388, 392, 395 |
| iteration 1 / 2 | Loop-backs counted against the two-iteration budget before escalation to Miguel | docs/PROCESS.md §5; ub-lp9.3 comments 372, 388 |
| PASS WITH CHANGES / PASS WITH NOTES | Verdicts: pass with required edits / pass with non-blocking notes | ub-lp9.3 comments 372, 392 |
| must-fix (1)–(n) / should-fix | Numbered blocking findings and non-blocking findings of a gate round | ub-lp9.3 comments 372, 375, 378, 380, 388, 392 |
| ruling 385 | Miguel: accept SQLite unlinking the rolled-back journal at close; raw/ copy is the evidence | ub-lp9.3 comment 385 |
| ruling 389 | Miguel: any Unsafe class means doctor --repair performs no mutating repair and writes nothing | ub-lp9.3 comment 389 |
| ruling 390 | Miguel: capture before touch — read anomalous sidecar bytes before any connection opens | ub-lp9.3 comment 390 |
| ruling 393 | Miguel: hot-journal exception to 389, 64 MiB capture cap, no journal capture, no clone | ub-lp9.3 comment 393 |
| rulings 382 / 383 | Orchestrator rulings: coupled help_top_level re-bless; sibling-guard outcome for init | ub-lp9.3 comments 382, 383 |
| StorageWindow / HealthRewrite / EngineHealth / CliInit / CliDoctor | The implementer's five slice sub-agents, one owner per file | ub-lp9.3 comment 384 |
| f1 / f5 | Exclusivity cells: f1 shows EXCLUSIVE refusing a live reader, f5 is the BEGIN EXCLUSIVE-only control | ub-lp9.3 comment 385 |
| CF-E | The reserved Storage::diagnostic_probe seam, withdrawn by D58 | docs/plans/01-design-spine.md §3; ub-lp9.3 comment 361 |
| AF-1 / AF-3 | D27 clauses: doctor exit only on integrity corruption (superseded), and the init clobber guard | docs/PRD.md §4 D27 |
| PC8 | The ub-lp9.25 claims row that pins health's storage-free layering | scripts/checks/ub-lp9.25-dangling-blocker-claims.sh |
| CI run 43840345239 / 43865511692 | Verify round 1 CI run at 2b24baa; the round-2 run at 752f869 | PR #474 checks |
| exclusivity-macos | The required macos-15 CI job running only the exclusivity cells | .github/workflows/ci.yml |
| raw/ / original/ | Evidence subdirectories: raw/ the forensic copies taken in the window, original/ the sidecar bytes captured before any open | docs/PRD.md §4 D58 clause (6) |

## Links

- `ub-lp9.3` — T4.3 / D58, FR-16 full health (this run); `ub-2vp` — the suspected v1 lock-loss when
  health closes a descriptor on the open database, split out to be measured; `ub-9ov` — CI `audit`
  and `deny` red on pre-existing advisories; `ub-tgw` — the tracker MCP server execs the prebuilt
  binary, a chore raised during this session.
- PR: https://github.com/websublime/unblock/pull/474
- Key files: `docs/PRD.md` (D58), `docs/plans/01-design-spine.md`, `docs/plans/crates/unblock-health.md`,
  `scripts/checks/d58-health-full-claims.sh`, `crates/unblock-health/src/`,
  `crates/unblock-storage/src/libsql/health.rs`, `crates/unblock-engine/src/preopen.rs`,
  `crates/unblock-cli/src/commands/doctor.rs`, `.github/workflows/ci.yml`.
- Prior related run-reports: [2026-11-02-fr13-config-layers](2026-11-02-fr13-config-layers.md),
  [2026-08-03-comments-forward-migration](2026-08-03-comments-forward-migration.md).
