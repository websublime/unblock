---
name: 2026-10-27-ub-kwh-triage-prompt
description: Making the `triage` prompt's suggested calls valid and true (tracker ub-kwh) — stale gains its required older_than, unassigned work is sized by count and filtered client-side, the promised deferred step covers both deferral forms, and a unit test runs every suggested prompt call through the live argument boundary; no contract-hash move.
type: run
date: 2026-10-27
branch: ub-kwh-triage-prompt
pr: ''
issues: [ub-kwh]
---

# Run — the `triage` prompt suggests only valid, truthful calls

## Context

Issue `ub-kwh`, which was blocked by `ub-m07` until that issue merged (PR #465). The `triage` prompt
told agents to call `query {kind:"stale"}`, which the schema rejects because `older_than` is required.
It also said `{kind:"list", assignee:null}` finds unassigned work, but that applies no filter. The run
went through the full lifecycle on branch `ub-kwh-triage-prompt` in the worktree `../unblock-ub-kwh`.
Design Review passed in round 2 (round 1 was 2 PASS / 1 FAIL). Verify passed in round 2 (round 1 was
2 PASS / 1 FAIL).

## What & why

Understand (graph unavailable; grep over `crates/`) confirmed both defects. No query kind can select
unassigned issues. The prompt body is NOT in the `CONTRACT_HASH` tuple: `capabilities()` carries only
each prompt's name and description, and the bodies are pinned only by golden snapshots
(implementation-plan A8). Rewording the body therefore cannot cause a D35 event.

Decide (Miguel, 2026-10-27):
- Reword with no contract move. Unassigned work is sized with `count` `group_by: assignee` (the key
  `""`), then a `list` is filtered client-side.
- The stale cutoff is now minus 30 days.
- After design Review round 1: "unassigned" covers an absent or blank assignee, matching `claim`'s
  `assignee IS NULL OR TRIM(assignee) = ''`.
- After design Review round 1: add the deferred step that the prompt descriptor already promised.
  Only the body changes; the descriptor is untouched, so the hash does not move.

## Outcome

- **Design Review round 1:** 2 PASS (Rust, QA) and 1 FAIL (architecture/spec). The must-fix was that a
  blank assignee is storable, but the "no `assignee` field" rule disagreed with the `count` bucket.
  Nits: the descriptor promised deferred triage but no step delivered it; a misspelled tool name was
  charged to the previous tool; the walker never proved it rejects a bad call; the negative test
  accepted any error.
- **Verify round 1:** 2 PASS and 1 FAIL. An executed duplex proved that `defer` sets only
  `defer_until` and leaves the status unchanged, so `status:["deferred"]` misses those issues. Step 4
  now names both forms: a future `defer_until` in the step-2 list, and `status:["deferred"]` with
  `include_deferred:true`.
- **Verify round 2:** 2/2 PASS, again with an executed duplex covering both deferral forms and the
  assignee buckets.
- **Executed proof:**
  - A mutant that reverts step 3 to `{"kind":"stale"}` fails the unit test.
  - Final state: prompts unit tests 3/3, `contract_suite` 21/21 with the capabilities and schema
    goldens unchanged, clippy `-D warnings` clean, `xtask doc-lint` OK, and the CI `scripts/checks`
    steps exit 0.
- **D35:** no schema or capabilities byte moves, so `CONTRACT_HASH` and `unblock.mcp.v1.10` are
  unchanged.

## Gotchas

- The run again started with edits on `main`'s shared tree (PROCESS §4). The work moved, uncommitted,
  to the worktree, and an issue comment records the correction.
- `defer` is not a status change. A deferred issue keeps its status (usually `open`), so it appears in
  a plain `list`. Only issues whose status was set to `deferred` need `include_deferred`.
- `status:["deferred"]` without `include_deferred` is always empty, because the default visibility
  branch excludes `deferred`.
- An empty assignee is normalised to absent on create and update. Only an assignee made of spaces
  survives, and `count` gives it its own key.
- The argument-boundary test only checks that each call is valid. Dropping `include_deferred` is
  still a valid call, so only the `prompt_triage` golden snapshot catches that.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| argument-boundary test | the unit test that walks every backticked `{...}` call in the 3 prompts and runs it through the live `parse_args` of the tool named before it, with placeholders filled in | `crates/unblock-mcp/src/prompts/mod.rs` (`every_suggested_call_passes_the_argument_boundary`) |
| D42 prologue | the single in-band argument deserialization (`parse_args`) that every tool runs | `crates/unblock-mcp/src/tools/args.rs:493` |
| D35 | the GA contract-freeze decision that decides whether a change moves `CONTRACT_HASH` / `CONTRACT_VERSION` (here it does not) | `docs/PRD.md:125` |
| A8 | the implementation-plan item that pins prompt messages as golden-only, NOT version-coupled | `docs/plans/implementation-plan.md:80` |
| visibility branch | the default filter that hides closed, tombstoned and deferred rows unless the `include_*` flags are set | `crates/unblock-storage/src/libsql/query.rs:465` |
| round 1/2 | successive iterations of the design Review or Verify gate | `ub-kwh` issue comments |

## Links

- `ub-kwh`: the `triage` prompt suggested a call the schema rejects and misdescribed `assignee:null`.
- Prior: `ub-m07` (it settled the query filter semantics first), [2026-10-27-ub-m07-ready-filters](2026-10-27-ub-m07-ready-filters.md).
- Files: `crates/unblock-mcp/src/prompts/triage.rs`, `crates/unblock-mcp/src/prompts/mod.rs`,
  `crates/unblock-mcp/tests/snapshots/contract_suite__prompt_triage.snap`, `docs/plans/00-roadmap.md`,
  `docs/roadmap.html`, `docs/plans/crates/unblock-mcp.md`.
