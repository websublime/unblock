---
name: 2026-10-06-update-refusal-classification
description: Classifying GitHub's refusals of the unblock update release query (tracker ub-e47, decision D55) — a 403 or 429 now gives RATE_LIMITED exit 2 and a 401 gives CONFIG_ERROR exit 7 instead of INTERNAL_ERROR exit 1, through a pure status classifier and two new CLI error variants, with nine hermetic cells, a d55 claims gate, mutation probes and a live 401 check, and the contract unblock.mcp.v1.10 unchanged.
type: run
date: 2026-10-06
branch: ub-e47-update-rate-limited
pr: '459'
issues: [ub-e47, ub-bq8, ub-pe6]
---

# Run — `unblock update` classifies GitHub's refusals of the release query

## Context

`ub-e47` carries Miguel's 2026-10-02 ruling that GitHub's refusal of the release query must name
`AXOUPDATER_GITHUB_TOKEN` and surface as `RATE_LIMITED`, not as an internal error. The run went
through the full lifecycle. Understand used three read-only lenses: the CLI code path, the spec
sites, and upstream axoupdater and GitHub behaviour. Decide used three lenses, and Miguel ruled
the five forks F1-F5. One spec writer wrote D55. The design Review had three reviewers and a
confirmation pass. One implementer built the change. Verify had four reviewers and a delta pass.
Track wrote this report. The work ran in an isolated worktree on branch `ub-e47-update-rate-limited`
off `main` 03489ec.

## What & why

These were read rather than restated here: PRD §4 D34, D35, D40 and D55, spine §2.2 and §2.3,
ci-cd §2.1 and §4, and the `unblock-cli` crate plan.

A 403 from GitHub on the release query came out as `INTERNAL_ERROR`, exit 1, `retryable:false`.
That event is an anonymous rate limit in most cases, and it was never an unexpected internal fault.
axoupdater keeps only the status, the reason phrase and the URL, so the status is the one signal
unblock can read. Miguel ruled that 403 and 429 move to `RATE_LIMITED` and 401 moves to
`CONFIG_ERROR`. Everything else stays where it was. The remedy travels in `message`, because a hint
would move the contract. D55 records why a post-GA reclassification is acceptable here.

## Outcome

### What landed

- The D55 row in the PRD, with a reciprocal note on D34 clause (3), and matching updates to spine
  §2.2 and §2.3 and the `unblock-error` crate plan. The D-range moved to D1..D55 at every
  PROCESS.md §3 site, and the NEWEST pointer cascade retargeted d53 Q53 and d54 Q67 and Q72.
- A `v1.0.2 — maintenance patch [PLANNED]` slot in the roadmap and in `docs/roadmap.html`. The
  stale v1.0.1 tags now say it was released on 2026-10-02.
- Two `self-update`-gated variants in `exit.rs`. `UpdateRateLimited` maps to `RATE_LIMITED` and
  `UpdateUnauthorized` maps to `CONFIG_ERROR`.
- In `update.rs`, a pure `refusal_for_status` and a pure `refusal_message` with unit cells. A
  two-valued `TokenState` reaches the mapper, and the token text never does.
- Nine hermetic `update_refusal_…` cells, plus the re-pinned token-gated cell.
- `scripts/checks/d55-update-refusal-claims.sh` with 1 P row and 83 Q rows, wired into the
  doc-lint job of `ci.yml`.
- A README line naming `AXOUPDATER_GITHUB_TOKEN`.

### Gate verdicts

- Design Review round 1 found 3 must-fix and 12 should-fix items. All fifteen were applied.
- The confirmation pass found 0 must-fix and 7 should-fix items. All seven were applied.
- Verify returned PASS WITH NOTES with 0 must-fix and 9 should-fix items. The delta pass confirmed
  them all.

### Measured

- `cargo test --workspace` passed 1869 tests across 129 binaries, with 0 failed and 8 ignored.
- With `main` 03489ec's code, the new test file failed 8 of 14 cells. Only the 404 boundary cell
  passed among the refusal cells.
- Mutation probes killed 13 mutants. Two survived. Hard-coding the token state at the
  `load_receipt` site is equivalent, because that call reads only a local file. The dry-run
  `is_update_needed` site cannot see a refusal, and ci-cd §2.1 discloses it. Mutant m survived
  every cell until the ninth cell was added.
- A live run with a bogus token gave `CONFIG_ERROR` and exit 7. The token appeared on neither stream
  across 20 trace lines. An anonymous dry-run printed `update available: 1.0.1`.
- The contract suite passed, so `unblock.mcp.v1.10` stands and `CONTRACT_HASH` did not move.

### Commits

The planned sequence is docs(spec) → fix(update) → test(update) → ci(d55) → chore(tracker).

## Gotchas

- The spec writer's first edits used repo-relative paths and landed in the main checkout. They were
  moved into the worktree. Every edit in a worktree run needs an absolute worktree path.
- The release smoke has two querying binaries. `from_tag` runs `update --dry-run` and `update`, and
  `to_tag` runs the post-swap dry-run. Design Review round 1 caught the single-binary claim.
- The fix commit had to carry the re-pin of the existing token-gated cell. Without it, `cargo test`
  goes red on that commit.
- A status error's `source()` is None, so the classifier has to match the `Reqwest` variant itself.
- Four reviewers shared one target dir. One built with `--no-default-features` mid-run, and two
  cells failed once (the `exit_codes` update cell and `help_snapshots` `top_level_help`). This is
  the `ub-lp9.47` pair.
- One test file is split across the fix and test commits. That needed the stored Phase A2 blob.

## Glossary

| Id | Meaning |
|---|---|
| F1 | Decide fork on the 401 code. Ruled `CONFIG_ERROR`, exit 7. |
| F2 | Decide fork on the message text. The remedy goes in `message`, never `hint`. |
| F3 | Decide fork on other moves. Nothing besides 403, 429 and 401 moves. |
| F4 | Decide fork on the published `RateLimited` doc text. It stays byte-identical. |
| F5 | Decide fork on the release slot. A v1.0.2 roadmap slot opens. |
| F-8 | The `unblock-mcp` crate-plan row for the NFR-18 rate-limit chokepoint, which describes `RATE_LIMITED`'s MCP producer (`docs/plans/crates/unblock-mcp.md:45`). Cited in the Understand comment. |
| D55-SEC-N4 | Security note on a bad installer base URL echoing its userinfo. Filed as `ub-pe6`. |
| D55-SEC-N5 | Security note on the bearer token following a `Link: rel="next"` URL. Filed as `ub-bq8`. |
| Q70 | Row of the d55 claims script that pins the README token line. |
| Q83 | Row of the d55 claims script that pins the ninth cell, the tokenless 401. |
| Phase A | The implementation part of Implement, which maps onto the fix(update) commit. |
| A2 | Phase A2, the stored fix-commit version of `update_verify.rs`, carrying only the re-pin. |
| 9cd5f3b | The git blob id of Phase A2. |
| mutant m | The probe that mapped the tokenless 401 back to `INTERNAL_ERROR`. |

## Links

- `ub-e47` — GitHub's refusal of the release query surfaced as `INTERNAL_ERROR`.
- `ub-jh5` — the token read whose message D55 builds on.
- `ub-lp9.26` — the live update smoke.
- `ub-bq8` — D55-SEC-N5, the bearer token sent to a `Link` header URL.
- `ub-pe6` — D55-SEC-N4, the base-URL userinfo echo.
- `ub-lp9.47` — the shared target dir test pair.
- Pull request — 459 (`websublime/unblock`), opened 2026-10-06; merging is the human gate.
- Key files —
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/commands/update.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/exit.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/tests/update_verify.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/checks/d55-update-refusal-claims.sh`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/PRD.md`,
  `/Users/ramosmig/Public/WS-Labs/unblock/docs/plans/ci-cd-and-distribution.md`.
- Prior related run-reports — `runs/2026-10-02-update-github-token.md` and
  `runs/2026-10-02-update-live-smoke.md`.
