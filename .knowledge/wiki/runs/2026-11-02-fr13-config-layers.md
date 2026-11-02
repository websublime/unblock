---
name: 2026-11-02-fr13-config-layers
description: FR-13 full config layers (tracker ub-lp9.2, D57) — a user config file and a DB config table join the CLI, env and project layers in one six-layer fold, startup and runtime keys are partitioned, runtime keys reload live behind the shared session with a fail-closed gate on a bad DB row, and an issue config action ships in contract unblock.mcp.v1.12 with a new d57 claims gate.
type: run
date: 2026-11-02
branch: ub-lp9.2-config-layers
pr: '472'
issues: [ub-lp9.2, ub-p1h, ub-v2m]
---

# Run — FR-13 full config layers: user file, DB table, runtime partition, live reload

## Context

Issue `ub-lp9.2`: implementation task T4.2, the v1.1 remainder of FR-13 (full configuration layers).
The run went through the full lifecycle on branch `ub-lp9.2-config-layers`, off main `533453a`, in the
isolated worktree `../unblock-lp9.2`. PR: #472 (opened as a draft for the Verify CI runs, marked ready
at Track). Design Review passed in round 3 (rounds 1 and 2 FAIL; round 3 run after Miguel authorised
an extra iteration). Verify passed in round 3 (round 1 FAIL under all four lenses, round 2 FAIL under
QA only). Team: one coordinator, three read-only Understand lenses, one spec writer, one implementer,
three design-review lenses per round and four Verify lenses per round, plus one Track agent.

## What & why

FR-13 (PRD §3) had acceptance criteria only for the v1 layers (CLI, env, project file, defaults); the
crate plan (`docs/plans/crates/unblock-config.md`) already promised a user file, a DB layer and
`reload_runtime`, but no D-id, no implementation-plan row and no storage table existed, and the
startup/runtime key lists had no production caller. Decide (Miguel, 2026-10-30) fixed the order
CLI > env > project > user > DB > defaults, a `config` table in schema step 3, the user file at
`$XDG_CONFIG_HOME/unblock/config.toml` (else `~/.config/unblock/config.toml`) on every OS, enforced
partitioning with live reload, and an `issue` tool `config` action (get/set/unset/list) under the
8-tool cap; config rows stay out of the JSONL export. Authoritative text: PRD §4 D57 with its acceptance
criteria, PRD §8.1 (fuzz row amended for the `unblock-fuzz -> unblock-config` edge), spine §3.2 and §6,
the unblock-config, unblock-storage, unblock-mcp, unblock-cli and unblock-fuzz crate plans, and
implementation-plan §5a T4.2.

## Outcome

Landed on the branch over `533453a`, re-cut into atomic commits at Track, spec first:
- `docs(d57)` — D57 row, FR-13 v1.1 criterion, spine amendments, crate plans, T4.2 row, plus the
  Verify round-1 doc drift and Miguel's round-3 ruling that the user file is validated whole.
- `feat(model)`, `feat(storage)` (schema step 3, `config` table, Storage config methods),
  `feat(config)` (user file, DB layer, startup/runtime partition, `ConfigReloader`), `test(config)`
  (six-layer fold totality proptest), `feat(engine)` (config_get/list/set/unset, live reload,
  fail-closed gate), `feat(mcp)` (contract `unblock.mcp.v1.12`), `feat(cli)`.
- `docs(d57)` fuzz layering edge, `test(fuzz)` `config_toml` target, `ci(d57)`
  `scripts/checks/d57-config-layers-claims.sh` wired into the doc-lint job.

Design Review: round 1 FAIL (all three lenses), round 2 FAIL (two lenses; the first attempt never ran
because of a provider quota error), round 3 PASS with no must-fix. Verify: round 1 FAIL (four lenses;
CI red on a stale schema-version assertion), round 2 FAIL (QA: blank-actor misruling and a commit that
was not green after autosquash), round 3 PASS. Every commit was checked green individually at Track.
Follow-ups filed: `ub-p1h`, `ub-v2m`.

## Gotchas

- The default subagent model (github-copilot) failed every design reviewer on wake with an HTTP 402
  quota error; round 2 was re-run on another provider.
- The design gate needed three rounds; the third required Miguel's explicit authorisation.
- The `config_toml` fuzz target needed an `unblock-fuzz -> unblock-config` edge that PRD §8.1 and
  `xtask/src/layering.rs` forbade; fixed spec-first by amending the PRD fuzz row before the code.
- Implementers twice edited the main checkout through relative paths instead of the worktree; every
  worktree agent must use absolute paths.
- The round-1 ruling that a blank user-file actor is refused was wrong — blank means unset at every
  actor tier (`crates/unblock-config/src/actor.rs:15`); reverted in Verify round 3.
- `crates/unblock-storage/tests/migrate_under_held_lock.rs` still asserted schema version 2 and masked
  several storage tests in CI; bump every version literal with a new schema step.
- Re-cutting with `--autosquash` reordered a config fixup ahead of the proptest commit and conflicted in
  `crates/unblock-config/tests/proptest.rs`; the proptest hunk was moved to the test commit.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| T4.2 | The implementation-plan task for FR-13 full config layers this run delivered | docs/plans/implementation-plan.md §5a |
| AC (a)–(q) | The lettered acceptance criteria of the D57 row; (d) fail-closed gate, (q) every commit green | docs/PRD.md §4 D57 |
| clause (1)–(10) | The numbered clauses of the D57 row (e.g. (4)/(6)/(10) user-file validation) | docs/PRD.md §4 D57 |
| forks 1–5 | The Understand questions Miguel ruled on in Decide | ub-lp9.2 Decide comment |
| sub-forks (1)–(4) | The Spec/Plan questions Miguel ruled on (action placement, export, no-workspace paths, entry shape) | ub-lp9.2 Spec/Plan comment |
| Q26 / Q27 | d56 claims rows on the issue tool description, re-pointed for the config action | scripts/checks/d56-organization-claims.sh |
| FaultyDbConfig | The config error raised while an undecodable or startup-key DB config row exists; gates commands with CONFIG_ERROR exit 7 until the row is unset | crates/unblock-config/src/error.rs:170 |
| architecture / rust-engineer / edge-cases-CI | The three design Review lenses | ub-lp9.2 design Review comments |
| code-review / QA / rust-engineer / security | The four Verify lenses | ub-lp9.2 Verify comments |
| round | One pass of a gate (Review rounds 1–3, Verify rounds 1–3) | docs/PROCESS.md §6 |

## Links

- `ub-lp9.2` — FR-13 full config layers (this run); `ub-p1h` — a consumer for the `jsonl_export`
  key; `ub-v2m` — repair of out-of-band oversized config keys.
- PR: https://github.com/websublime/unblock/pull/472
- Key files: `crates/unblock-config/src/user_config.rs`, `crates/unblock-config/src/db_layer.rs`,
  `crates/unblock-config/src/reload.rs`, `crates/unblock-engine/src/session/config.rs`,
  `crates/unblock-mcp/src/tools/issue.rs`, `crates/unblock-fuzz/src/config_target.rs`,
  `scripts/checks/d57-config-layers-claims.sh`, `docs/PRD.md`, `docs/plans/01-design-spine.md`.
- Prior run-reports: [2026-10-29-ub-lp9-1-fr6-organization](2026-10-29-ub-lp9-1-fr6-organization.md).
