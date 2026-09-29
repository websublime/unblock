---
name: 2026-09-29-attribution-discard-disclosure
description: Publishing that the MCP wire discards Tier-1 attribution instead of silently dropping it (decision D52, tracker ub-lp9.22) — the fields stay accepted, five tool descriptions and three field descriptions now say so under an additive bump to unblock.mcp.v1.10, a D43-shaped claim gate guards the retired wording, and the run finished solo at reduced build parallelism after the machine crashed and stalled under parallel builds.
type: run
date: 2026-09-29
branch: ub-lp9.22-attribution-discard
pr: '-'
issues: [ub-lp9.22]
---

# Run — the wire discloses that Tier-1 attribution is discarded

## Context

`ub-lp9.22` is a finding from the ub-lp9.12 Verify gate, where it was must-fix item 4. Every mutating
MCP arm accepts the optional `agent_name`/`harness`/`model` fields and drops them, and the `events`
attribution columns read back `NULL`. The published schema still called the fields "capture-only",
and no tool description mentioned them.

The run went off `main` at 586ff0f on branch `ub-lp9.22-attribution-discard`, in the worktree
`../unblock-lp9.22`. The branch carries three gated commits, c67428c (spec), eb95233 (code) and
c79bb7c (claim gate), and this Track commit sits on top of them. No pull request exists at the time of
writing, so the `pr` field holds `-`.

Understand ran as three read-only scouts, and the code graph answered at 586ff0f. Decide ran with
Miguel. A first Spec/Plan agent was lost when the computer crashed. After the restart, Miguel reported
that the machine became unresponsive under parallel agents and concurrent builds. The orchestrator
therefore wrote Spec/Plan and Implement itself and ran one narrowed build at a time
(`CARGO_BUILD_JOBS=4`, niced), which departs from PROCESS §4. Both gates still ran with three read-only
reviewers each, consolidated by the orchestrator.

## What & why

Read before deciding: PRD §4 D35 (semver), FR-20 and its carve-out list, FR-22, §7 `Event`; spine §5.2
and the §5.4 contract ledger; ci-cd §2.1; PROCESS §3.

Recording attribution is FR-22 [v1.1], owned by `ub-lp9.7`, so the storage gap is a deliberate
deferral. The defect was the silent contract. Miguel ruled on three forks.

- Route: document the discard. Removing the fields would make `deny_unknown_fields` reject calls that
  succeed today, a 2.0.0 event. Binding them now would thread attribution through every `Session`
  mutator and `Storage` method, and the JSONL round-trip the issue asked for has no carrier.
- Decision record: mint D52, with the full D-range cascade.
- Release slot: v1.0.1, beside D42–D50.

## Outcome

### What landed

- D52 in PRD §4, with reciprocal notes at FR-1a, FR-20 carve-out (i) and the §7 `Event` row.
- Spine §5.2 holds the normative texts, and the §5.4 ledger gains the v1.10 entry.
- The three field descriptions and five tool descriptions state the discard. The tool sentence is
  conditional because read actions and `issue create_bulk` do not flatten `Attribution`.
- `unblock.mcp.v1.10` with a `CONTRACT_HASH` re-pin. The goldens moved by exactly 42 field
  descriptions, 5 tool descriptions and 2 `contract_version` lines. `AGENTS.md` moved by five rows and
  its contract line.
- D-range D1..D52 at every site PROCESS §3 lists, and `CONTRACT_RE` at v1.10 in six scripts.
- `scripts/checks/d52-attribution-discard-claims.sh`, wired as a required `doc-lint` step.
- Drift fixed in the plans README (CF-F) and the mcp, policy and storage crate plans.

### The gate record

- Design Review: PASS-WITH-FIXES, with one must-fix. CLM-1 blocked every hit but also matched the
  `unblock-model` `Event` doc-comments that the spec allow-listed. The should-fixes were the
  conditional tool sentence, removing a derived arm count, the "all four bumps" summaries, the
  "always the NEWEST script" wording, and T3.14 gaps. All were applied.
- Verify: PASS. Security and QA had no findings. The code review's two nits were applied.

### Evidence

- `cargo fmt --check` passed, and `cargo clippy -p unblock-mcp -p unblock-storage --all-targets -D
  warnings` was clean.
- `cargo test -p unblock-mcp` passed in full, as did `cargo test -p unblock-cli --test init_agents`.
- All nine `scripts/checks/*.sh`, `cargo xtask doc-lint`, `knowledge-lint` and the invariants check
  passed.
- A smoke run with the built binary in a temporary workspace passed. `tools/list` and `capabilities`
  carry the new texts, create and claim still accept attribution, `issue show` rejects it with
  `VALIDATION_FAILED`, and the event rows hold `NULL`.
- The full workspace suite and the rest of `unblock-cli` were not run locally, so CI covers them.

## Gotchas

- The shell exported `CI=true`, so insta refused to write snapshots. Re-blessing needed `env -u CI
  INSTA_UPDATE=always cargo test …`.
- An empty `UNBLOCK_DIR` in the environment reaches clap as `--dir` with no value, and `unblock init`
  exits 2. Remove the variable instead of blanking it.
- One full compile of `unblock-mcp` took 34 minutes, and `cargo test -p unblock-mcp -p unblock-cli`
  hit a one-hour limit with no test run. Narrowed targets (`--test contract_suite`, `--test
  init_agents`) finished in 3 to 13 minutes.
- A tracker MCP call timed out during a build, and its comment was not written. Posting after the
  build finished worked.
- The new claim gate first blocked its own CI comment, which used the very recording wording it forbids. The
  comment was reworded rather than allow-listed.
- The first Track export blocked the gate with a CLM-2 hit in `.unblock/issues.jsonl`, because this
  task's own Understand comment quoted the retired storage claim word for word. The comment was
  reworded through the `comment` tool's `update` action and the record re-exported.
- `[v1.1]` inside a Rust doc comment is intra-doc-link syntax, so the two doc comments say `(v1.1)`.
- The CLM-3 allow-list entry for `.unblock/issues.jsonl` matches only `ub-lp9.22`'s own title.
  Retitling that issue would turn the self-test red.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| MF-4 | must-fix item 4 of the ub-lp9.12 Verify gate, the finding that became this issue | `ub-lp9.22` description; `ub-lp9.12` comments |
| R1 | the ub-lp9.12 briefing rule that an acceptance criterion must not be true only by omission | `ub-lp9.22` description |
| wf_4efbe011-3ae | the workflow run of the ub-lp9.12 Verify gate that found MF-4 | `ub-lp9.22` description |
| CLM-1 | the claim-gate family for the retired field description "Self-reported … (capture-only)" | `scripts/checks/d52-attribution-discard-claims.sh`; ci-cd §2.1 D52 sub-check |
| CLM-2 | the claim-gate family for the retired storage claim that mutations carry attribution | same script; ci-cd §2.1 D52 sub-check |
| CLM-3 | the claim-gate family for an unqualified line saying attribution is recorded | same script; ci-cd §2.1 D52 sub-check |

## Links

- `ub-lp9.22` — this task: the wire accepted and silently discarded Tier-1 attribution.
- `ub-lp9.7` — FR-22, the v1.1 audit recorder that will bind and record attribution.
- `ub-lp9.12` — the D42 task whose Verify gate found this issue.
- Key files: `docs/PRD.md` (D52 row), `docs/plans/01-design-spine.md` (§5.2, §5.4),
  `crates/unblock-mcp/src/tools/dto.rs`, `crates/unblock-mcp/src/options.rs`,
  `crates/unblock-mcp/src/resources/capabilities.rs`,
  `scripts/checks/d52-attribution-discard-claims.sh`.
- Prior related run-report: `.knowledge/wiki/runs/2026-09-21-pre-handshake-frame-gate.md` (D50, the
  newest range-carrying claim gate).
