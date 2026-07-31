---
name: 2026-07-31-create-deps-atomicity
description: Making a create with declared dependencies one indivisible act — a silent third-party graph corruption nobody had recorded, four spec-repair rounds against an unbounded prose sweep, and a mutation pass that caught a structurally unfailable assertion.
type: run
date: 2026-07-31
branch: worktree-wf_10cf3f9d-dfc-1
pr: -
issues: [ub-lp9.20, ub-lp9.25]
---

# Run — atomic create-with-deps (decision D44)

## Context

Task `ub-lp9.20` (an unblock task id; unblock is this repo's own MCP-based issue tracker, dogfooded) — a
priority-1 data-integrity bug filed during the planning of an earlier task and deliberately ruled out of that
task's pull request so it would get a real diagnosis rather than a fix in passing. The full lifecycle ran in
one session: understand, decide, spec/plan, design review gate (two rounds plus three condition-closing
rounds), implement, verify gate (one round plus one must-fix round), track. Every phase ran as a spawned
Workflow team; the main session orchestrated and never hand-wrote the change. Branch
`worktree-wf_10cf3f9d-dfc-1`, eleven commits on `main`.

`ub-lp9.25` was split out of this task mid-run by Miguel's ruling and ships in the same 1.0.1 patch cut.

## What & why

The MCP call `issue {action:"create", deps:[…]}` committed the issue row in one transaction and then wrote
each declared edge in its **own** independent transaction, anchored on a client-supplied source id the server
never reconciled with the id it had just minted. Authoritative sections read: `docs/PRD.md` §4 (the D22 and
D42 decision rows, whose scoping this change reverses and closes respectively), `docs/plans/01-design-spine.md`
§3.2.1 (the storage create contract), §4.1 (the engine session API) and §5.2 (the wire schemas), plus the
three crate plans for engine, storage and mcp.

The filed observation was that the storage trait's create takes no dependency parameter, so the edges must be
a separate call. The diagnosis found that **premise inverted, in the change's favour**: the shared per-record
insert body already writes the issue's own dependency list inside the one transaction and already binds the
issue's own id as the edge source. The engine simply never populated that list — it built the issue falling
through to `..Issue::default()` — and then looped separate writes. So no storage trait change was needed at
all, and the atomic primitive the fix wanted already existed and was already exercised by the bulk and import
paths.

Three outcomes were reproduced live over the JSON-RPC stdio wire, from a clean workspace, before any change:

1. a non-existent source id returned a database error naming a foreign-key failure **with the issue row
   already committed**, edgeless, and immediately offered by the ready query — and the error was marked
   retryable, so an obedient client mints a fresh orphan on every retry;
2. an existing but **unrelated** source id returned `isError:false` while the edge landed on that unrelated
   issue, which silently dropped out of the ready set with its `updated_at` unmoved — a create call silently
   rewriting a third party's dependency graph, recorded nowhere before this run;
3. a non-existent **blocker** id also returned `isError:false`, planting a permanently unresolvable blocker.

Outcome 3 is a schema property reachable from three entry points, so it was split out as `ub-lp9.25`.

## Outcome

Decision D44 landed, spec-first: the engine gained its own declared-edge input type carrying **no source
field at all**, so a misattached edge is unrepresentable below the wire boundary rather than merely rejected
at it; the engine stamps the minted id and seeds the edges onto the built issue, and the separate-write loop
is deleted; the restored duplicate and gating-cycle guards live on a create-specific path so bulk and import
semantics are untouched; and at the wire the source field became optional with any present value rejected
before the engine is reached. Contract `unblock.mcp.v1.6` → `v1.7`, additive.

Miguel ruled six things: the syntactic form of the rejection (any present value, not only a mismatched one),
the scope split to `ub-lp9.25`, the create-specific guard placement, the 1.0.1 slot, co-shipping `ub-lp9.25`
in the same cut, and — after the spec phase stalled — deleting the coverage meta-claims rather than patching
them again.

Gates. The **design review gate** returned FAIL on round 1 with thirteen must-fixes, then
PASS-WITH-MUST-FIX on round 2 with three; closing those took three further rounds, the third of which changed
approach rather than repeating (see Gotchas). The **verify quality gate** returned PASS-WITH-MUST-FIX with
five blocking items, four of which landed with mutation proof. Neither gate ever disturbed the design shape.

Final state, verified by the orchestrator in a **private** target directory: format clean, clippy across the
workspace with all targets and features and warnings denied clean with no suppression added, 1540 tests
passing and none failing, the feature-gated storage contract suite passing, no snapshots to review, the
documentation lint at nineteen documents and six classes clean, the layering check clean, the knowledge lint
clean, and both executable done-gates exiting zero. And proved live at the wire against the three original
cases: case 1 now rejects and persists nothing, case 2 rejects with the third party's graph untouched, and
with the field omitted the create succeeds with the edge anchored on the minted id and its metadata
round-tripping.

## Gotchas

- **A negative regex sweep over prose is unbounded by construction.** The done-gate hunted the retired claim
  with one single-line pattern per spelling; three consecutive repair rounds each found the *next* spelling,
  and round 3's was unfindable in principle — `crates/unblock-engine/tests/create_bulk.rs:12-13` wraps the
  claim across a line break, subject on one line and predicate on the next. Fixed by pairing the sweep with a
  **positive required landing** keyed to the file rather than to a spelling.
- **Prose that describes the extent of a mechanical check is itself unverifiable prose.** Each repair round
  fixed two false claims and introduced two more, and the new ones were always *meta-claims about the gate's
  own coverage*. Eighteen were eventually deleted with nothing written in their place; the executable
  self-tests were kept, because they fail loudly instead of rotting in silence.
- **A structurally unfailable assertion.** Four mutants against the engine's edge stamp survived the entire
  1539-test suite while three test doc-comments claimed they died. `Session::create_issue` returns a re-read,
  and hydration at `crates/unblock-storage/src/libsql/crud.rs:408` selects `… WHERE issue_id = ?1` bound to
  the issue's own id and reads that column back off the row — so a hydrated edge is anchored on its own issue
  *by construction* and no re-read-based assertion at any layer can observe a bad anchor. Fixed by capturing
  the value handed to storage in the counting decorator, at the boundary the clause actually speaks about.
- **A shared `CARGO_TARGET_DIR` poisons any test that execs the built binary.** `target/debug/unblock` is one
  unhashed path that eleven concurrent worktrees overwrite, and both the CLI wire suite and the `init`
  snapshot suite exec it; a mutation run got five spurious failures from a binary whose `strings` output
  still carried the previous contract version. It fails both ways — a stale binary still containing the guard
  under test would let a deletion mutant pass. The authoritative probe was re-run in a private directory.
- **Restoring a mutated file with a copy that preserves mtime makes cargo skip the rebuild**, so the next run
  executes the stale mutant binary against provably clean source — a false red that reads as a flaky test.
  Restore with `git checkout --`.
- Two stale contract-version literals (in `README.md` and the owning crate plan) survived a green done-gate,
  a green documentation lint and a green knowledge lint, because the gate pinned the version only in code and
  the root README sits outside the documentation-lint corpus.
- A foreground `cd` in a shell call persists for the whole session and silently re-anchors later Workflow and
  worktree creation.

## Glossary

Session-local ids used in this report or in this run's issue comments. The gate rule prefixes below are not
session-local in the usual sense — they ship in a committed script — but they are unresolvable without a key,
which is exactly what a glossary is for.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| F-prefixed rule ids | the done-gate's forbidden-framing families: one single-line search pattern per retired spelling, each with an escape allowing a line that names the decision it scopes | the family table in `scripts/checks/d44-create-deps-claims.sh` |
| R-prefixed rule ids | the done-gate's required-landing rows: each asserts that a named file now contains a named pattern, and each had zero matches before the change so it cannot pass without it | the required-landing table in `scripts/checks/d44-create-deps-claims.sh` |
| RC-prefixed rule ids | the done-gate's contract-version pins: the four files that must carry the current contract identifier, two code and two documentation | the contract-pin table in `scripts/checks/d44-create-deps-claims.sh` |
| R1 | the first of Miguel's numbered rulings from the planning of task `ub-lp9.12`, an earlier and separate task — the ruling on which silent drops in the bulk-create grammar entered that task's scope | coined in that task's workflow; the surviving carrier is `ub-lp9.12`'s comment thread in `.unblock/issues.jsonl` |
| R1-i | the first sub-clause of that ruling — the drop class it originally covered | same as `R1` |
| R1-ii | the second sub-clause — the one that put the dependency metadata bind at the storage layer, which shipped as a clause of D42 | same as `R1`; also quoted in `ub-lp9.20`'s own description |

## Links

- `ub-lp9.20` — the task: before D44, a create carrying declared dependency edges committed its row and each
  of its edges in separate transactions, and anchored them on a client-supplied id.
- `ub-lp9.25` — split out of it mid-run: a dependency may name a non-existent blocker, because the
  dependencies table has no foreign key on the blocker column. Ships in the same 1.0.1 cut.
- `ub-lp9` — the v1.1 epic both hang under.
- Key files: `crates/unblock-engine/src/session/write.rs`, `crates/unblock-storage/src/libsql/crud.rs`,
  `crates/unblock-storage/src/libsql/deps.rs`, `crates/unblock-storage/src/trait_def.rs`,
  `crates/unblock-mcp/src/tools/dto.rs`, `crates/unblock-mcp/src/tools/issue.rs`,
  `crates/unblock-mcp/src/tools/dep.rs`, `scripts/checks/d44-create-deps-claims.sh`.
- Prior related run: `runs/2026-07-29-duplicate-key-execution-flip.md` — the immediately preceding decision,
  another silent-execution defect, and the source of the executable done-gate pattern this run extended.
- Related topic: `topics/knowledge-gardener.md`.
