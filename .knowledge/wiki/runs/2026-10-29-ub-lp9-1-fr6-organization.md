---
name: 2026-10-29-ub-lp9-1-fr6-organization
description: The FR-6 organization remainder (tracker ub-lp9.1, D56) — a label catalog and label rename, an epic rollup read and an explicit close_eligible with a nested-epic cascade, shipped as four new arms on the existing query and issue tools under the 8-tool cap, moving the contract to unblock.mcp.v1.11 with a new d56 claims gate.
type: run
date: 2026-10-29
branch: ub-lp9.1-labels-epic-rollups
pr: '471'
issues: [ub-lp9.1, ub-bnb]
---

# Run — FR-6 organization remainder: labels, epic rollups, close_eligible

## Context

Issue `ub-lp9.1`: implementation task T4.1, the v1.1 remainder of FR-6 (label rename and list-all, epic
rollups with auto-close-eligibility). The run went through the full lifecycle on branch
`ub-lp9.1-labels-epic-rollups`, off main `b73749a`, with spec and implementation done in isolated
worktrees. PR: #471 (opened as a draft for the Verify CI run). Design Review passed in round 2 (round 1
FAIL under all four lenses; round 2 PASS with no must-fix, six text fixes applied after). Verify passed
in iteration 2 (iteration 1: four lenses PASS with no must-fix, five should-fixes applied; the first CI
run failed on one root cause, fixed by coverage). Team: one coordinator, three read-only Understand
lenses, one implementer, four review lenses per gate.

## What & why

FR-6 (PRD §3) had acceptance criteria only for comments; the remainder had none, and the MCP tool budget
(spine §6.6) was already full. Understand mapped bd's `rename_label`, `list-all`, `epic status` and
`epic close-eligible` against unblock's storage, engine and MCP layers and raised eight forks. Decide
(Miguel, 2026-10-29) took every recommended default: an explicit `close_eligible` mutation with dry_run
through the normal close path (diverging from bd's raw UPDATE, so `content_hash` is recomputed), one epic
read with optional id and eligible_only, the LabelRemoved/LabelAdded event pair for rename, a new label
catalog query with bd's visibility, four arms on the existing `query` and `issue` tools, one PR and one
contract bump, and D56 to ratify it. bd's open-children close guard was split out as `ub-bnb`.
Authoritative text: PRD §4 D56 with acceptance criteria (a)–(q), spine §3.2.1 and §6.1, the unblock-mcp
crate plan (OQ-3 resolved), implementation-plan §5a T4.1, ci-cd §2.1.

## Outcome

Landed on the branch over `b73749a`:
- `docs(spec)` — D56 row, spine amendment, crate plans, roadmaps, ci-cd §2.1, D-range bump to D1..D56.
- `feat(d56)` — model results, three new Storage methods in all 12 implementations, the shared epic
  eligibility predicate and cascade in the engine, four Session methods, four MCP arms, contract
  `unblock.mcp.v1.11`, regenerated AGENTS.md/README/snapshots, the D53 duplicate-key corpus cells for the
  new arms, and the spine's rename SQL amended to the correlated EXISTS form (spec-first on drift).
- `ci(d56)` — `scripts/checks/d56-organization-claims.sh` (110 rows; on `b73749a` 108 fail and only P1
  and Q28 pass) wired into CI.
- `docs(process)` — PROCESS.md §4 machine-load rule and the §6 draft-PR exception for Verify.

Design Review: round 1 FAIL (ten defects, all fixed), round 2 PASS, zero must-fix. Verify: PASS,
zero must-fix; CI on PR #471 green on every required job after the corpus fix, with `run-report-gate`
pending on this report.

## Gotchas

- PROCESS.md was not loaded at session start, so the lifecycle rules were read late; read it before
  Understand.
- Relative paths in one agent's tool calls resolved against the main checkout instead of the worktree
  and leaked edits there once; every worktree agent now uses absolute paths.
- `cargo test --workspace` froze the machine. Response: local cargo is capped at 4 cores, tests are
  always targeted, one cargo process at a time, and the full suite runs only in CI through a draft PR
  (PROCESS.md §4, §6).
- CI triggers only on `pull_request` and pushes to `main`; a pushed branch alone runs nothing, hence the
  draft PR.
- The D53 cell g3 (`crates/unblock-mcp/tests/duplicate_key_duplex.rs:496`) fails whenever a new
  published MCP arm lacks a duplicate-key corpus cell or exemption; it turned the first CI run red in
  `test (workspace)` and `snapshots`. Add corpus cells in
  `crates/unblock-mcp/src/duplicate_key_corpus.rs` with every new arm.
- The code-graph index still has the misaligned name column (`ub-7m4`); every graph claim was confirmed
  by grep/read.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| T4.1 | The implementation-plan task for the FR-6 remainder this run delivered | docs/plans/implementation-plan.md §5a |
| OQ-3 | The unblock-mcp crate plan's open question on where label operations go, deferred to v1.1 and resolved by D56 | docs/plans/crates/unblock-mcp.md (OQ-3) |
| AC (a)–(q) | The lettered acceptance criteria of the D56 row; (n) names live-contract-id files, (o) the nested-epic cascade fixture | docs/PRD.md §4 D56 |
| clause (1)–(5) | The numbered clauses of the D56 row (e.g. (4) epic read, (5) rename merge rule) | docs/PRD.md §4 D56 |
| F3 | Understand-map fork on label rename semantics (event pair, merge, tombstone skip); its default set the merge-event rule | ub-lp9.1 Understand comment (fork 3) |
| forks 1–8 | The eight Understand questions Miguel ruled on in Decide | ub-lp9.1 Understand and Decide comments |
| g3 | The D53 cell `g3_every_published_union_arm_is_covered_or_explicitly_exempt`, requiring a duplicate-key corpus cell or exemption per published MCP arm | crates/unblock-mcp/tests/duplicate_key_duplex.rs:496 |
| P1 | d56 gate row asserting the tracker export names `ub-lp9.1`; passes on main by design | scripts/checks/d56-organization-claims.sh:118 |
| Q28 | The single negative d56 gate row: no non-comment libsql line spells OR IGNORE | scripts/checks/d56-organization-claims.sh:179 |
| N1 | Rust-lens design-review note: remote mode takes no write lock, so close_eligible's race there is silent; routed to roadmap §4 open question 7 | ub-lp9.1 design Review comment |
| N2 | Rust-lens design-review note: the close_issues skip guard and the shared update_issue body each load the row (double read) | ub-lp9.1 design Review comment |
| architect / rust-engineer / research-analyst | The three read-only Understand lenses | ub-lp9.1 Understand comment |
| architecture / Rust feasibility / research fidelity / project management | The four design Review lenses | ub-lp9.1 design Review comment |
| code / QA / Rust / security | The four Verify review lenses | ub-lp9.1 Implement and Verify comments |
| round / iteration | One pass of a gate (Review round 1–2, Verify iteration 1–2) | docs/PROCESS.md §6 |

## Links

- `ub-lp9.1` — FR-6 organization remainder (this run); `ub-bnb` — bd's open-children epic close guard,
  split out; `ub-7m4` — graph index name-column misalignment.
- PR: https://github.com/websublime/unblock/pull/471
- Key files: `crates/unblock-engine/src/epic.rs`, `crates/unblock-engine/src/session/organization.rs`,
  `crates/unblock-storage/src/libsql/crud.rs`, `crates/unblock-storage/src/trait_def.rs`,
  `crates/unblock-mcp/src/tools/issue.rs`, `crates/unblock-mcp/src/tools/query.rs`,
  `crates/unblock-mcp/src/duplicate_key_corpus.rs`, `scripts/checks/d56-organization-claims.sh`,
  `docs/PROCESS.md`, `docs/PRD.md`, `docs/plans/01-design-spine.md`.
- Prior run-reports: [2026-10-28-ub-vcp-spawn-lock](2026-10-28-ub-vcp-spawn-lock.md).
