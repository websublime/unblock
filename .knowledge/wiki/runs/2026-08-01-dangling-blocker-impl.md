---
name: 2026-08-01-dangling-blocker-impl
description: Implementing the dangling dependency-target guard (decision D45) — three parallel implementers behind a design gate that had already failed twice, seventeen mutation kills an independent lens reproduced one by one, three claimed kills honestly declared equivalent instead, the two costs the specification refused to accept an opinion about, and a second round in which the 250k scale gate rejected one of those costs and the listing view was amended into a single SQL query.
type: run
date: 2026-08-01
branch: ub-lp9.25-dangling-blocker
pr: -
issues: [ub-lp9.25]
---

# Run — dangling blocker target, the implementation commit (decision D45)

## Context

Task `ub-lp9.25` (an unblock task id; unblock is this repo's own MCP-based issue tracker, dogfooded) —
a priority-0 data-integrity defect: a dependency edge could name a blocker issue id that does not
exist, the call returned `isError:false`, and the issue acquired a blocker that could never be created
and therefore never closed. Lifecycle phase: Implement, then the Verify quality gate, then the
must-fix pass this report closes. Branch `ub-lp9.25-dangling-blocker`; no pull request yet; the
`.unblock/issues.jsonl` re-export rides the same commit.

**The design gate ahead of this work had already FAILED TWICE** — round 1 on eleven blocking findings,
round 2 on ten more, which under `docs/PROCESS.md` §5 escalated to Miguel rather than looping a third
time; he ruled and directed a closed-scope third pass. That story is the sibling report
`.knowledge/wiki/runs/2026-08-01-dangling-blocker-spec.md`, and it matters here for one reason: this
implementation was written against text that had been adversarially rebuilt twice, so almost every
trap that would normally surface during implementation had already been paid for in prose.

Team shape: the code landed as three implementers in isolated worktrees (core storage/model, the
middle engine/sync layer, the L7 surface) plus a separate author for the executable claim gate, then
a Verify gate of three adversarial lenses (code, QA, specification) and a coordinator. This closing
pass was one implementer, solo.

## What & why

Built, against `docs/PRD.md` §4 (the D45 row) and `docs/plans/01-design-spine.md` §1.9, §1.10, §3.1,
§3.2.1, §4.1, §5.2 and §5.4, with the acceptance criteria at `docs/plans/implementation-plan.md`
T1.6:

- **An in-transaction target-existence guard on every edge-writing path.** Three production SQL
  bodies (`crates/unblock-storage/src/libsql/crud.rs` shared per-record insert body and reparent,
  `crates/unblock-storage/src/libsql/deps.rs` `add_dependency`) cover the five wire entry points —
  create with declared deps, `dep add`, the JSONL and `bd` import legs, reparent, and `create_bulk`.
  The guard runs INSIDE the caller's `BEGIN IMMEDIATE`, so a rejection leaves zero rows and zero
  events.
- **One ASCII-case-insensitive `external:` predicate**, `unblock_model::is_external_target`
  (`crates/unblock-model/src/id.rs`), at layer 0 — the only layer both storage and engine may depend
  on — retiring the case-SENSITIVE `starts_with` that disagreed with the SQL `LIKE` the read side
  already used. A write guard stricter than the read side is the invariant this closes.
- **The export corpus closed under its blockers in BOTH edge directions**
  (`crates/unblock-sync/src/export.rs`), because an edge is stored on one row while blockedness flows
  along it both ways — a kept epic with an ephemeral non-terminal child is blocked today and would
  arrive ready after an out-edges-only round trip.
- **A `dangling` diagnostics kind and the same findings folded into the CLI `doctor` report**, one
  composition (`crates/unblock-engine/src/diagnostics.rs`) awaited from both places rather than two
  implementations, and the contract bumped to `unblock.mcp.v1.8`.
- **An executable claim gate**, `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh`, plus the
  required CI step that makes the new engine cell actually execute
  (`cargo test -p unblock-engine --features testkit --locked --test dangling`) — without it the cell
  would have been green by non-execution in every job.

## Outcome

The Verify gate returned **PASS WITH MUST-FIXES** — five of them, none touching the correctness of the
guard, the carve-out, the closure or the contract. One (a verbatim-duplicated D-id-range knob in
`scripts/checks/d44-create-deps-claims.sh`) was fixed immediately. The other four landed in this pass:

1. **The doctor cost measurement the spine makes an obligation OF THIS COMMIT** (MF-1). The
   `Session::doctor()` row in `docs/plans/01-design-spine.md` defers the cheaper single-query
   alternative to v1.1 "with an obligation, not an opinion": the implementation commit measures the
   composed path on the existing large-workspace fixture and records the number. No number existed.
   It does now, measured on the existing NFR-2 250k fixture `crates/unblock-engine/tests/scale.rs`
   (dev profile, exactly as the CI `scale` job runs it) and recorded in the spine clause itself
   rather than only here: **over three runs the fold costs 4.51s, 4.55s and 4.65s, the composed
   `doctor()` 7.00s, 7.05s and 7.12s, and the pre-D45 half 2.47s, 2.48s and 2.53s — the fold roughly
   TRIPLES `doctor()` at 250k and is the dominant term.** The measurement is a reporting pair of timings inside the existing
   `run_scale`, so it is re-derived on every `scale` run instead of decaying in prose. Two honesty
   bounds are recorded with it: the seeded corpus has an EMPTY dependencies table, so the number is
   the row half alone and a real edge graph pays more; and the fold is a read pair, so the cost is
   caller latency, not lock hold time.
2. **Acceptance criterion (3) met to the letter** (MF-3). The criterion requires BOTH letter-case
   spellings of an external target on EVERY one of the five paths; three of the storage contract
   cell's four legs carried one spelling each. Four spellings were added in
   `crates/unblock-storage/src/testkit.rs`. They are not decorative, and the proof needed a ladder
   rather than a single mutant run, because all four legs live in ONE sequential cell and a mutant
   kills the first leg it reaches, hiding the rest. Under the case-SENSITIVE predicate mutant the
   ladder lowered each earlier leg's upper spelling in turn and read the offending id out of the
   panic: create → bulk (`EXTERNAL:jira-3b`, added) → `dep add` (`EXTERNAL:jira-4b`, added) →
   reparent, four distinct reds at four distinct assertions. The two added LOWER spellings are not
   reachable by that mutant by construction. **This report first claimed they were killed by the
   no-carve-out mutant, and that claim was FALSE — the exact false-coverage class this task's gates
   exist to catch, caught here by the independent checker rather than by its author.** Under
   no-carve-out the cell dies at the FIRST leg's `external:jira-1`, and deleting both added lower
   spellings leaves it equally red, so that mutant cannot distinguish them: it proves nothing about
   the cells it was cited for. They ARE load-bearing, and the mutant that shows it is a predicate
   accepting only the UPPER spelling, run through the same ladder — which yields reds at
   `external:jira-3` (`crates/unblock-storage/src/testkit.rs:3688`) and `external:jira-5-lower`
   (`:3725`). Both source files were restored from `cp` backups and verified by md5.
3. **The export closure's cost stated in the specification** (MF-5). Spine §1.10 property 1 states
   that the closure TERMINATES and says nothing about what it costs, while the sibling doctor clause
   states its cost — D45 stating the cost of its cheap composition and omitting the cost of its
   quadratic one. Measured through the real `export_jsonl` on an adversarial corpus, TWICE, by two
   people on two profiles — and the two agree on the SHAPE while differing on the absolutes, which is
   why both are recorded rather than one. Release profile: **327ms at 4k, 1.21s at 8k, 5.06s at 16k
   and 24.6s at 32k** excluded rows, against a linear control of **59, 75, 123 and 238ms**. Dev
   profile, measured independently by the checker: **3.42s, 13.67s, 55.97s and 244.1s**, against a
   control of **101, 193, 378 and 753ms**. Both give roughly 4× per doubling — **quadratic**, as the
   shape predicts, since the specified worklist is re-scanned on growth and a pass may add one row.
   **The release absolutes are single-sourced:** the checker could not build release (the machine ran
   out of disk mid-run), so those four numbers were not independently reproduced and are labelled as
   such here and in the specification. The adversarial shape is named in the spine so it can be
   rebuilt: one kept row plus N ephemeral rows in a single chain whose ids ascend ALONG the chain, so
   the `id ASC` read order is the exact reverse of eligibility order. **The algorithm was deliberately
   NOT changed**: the worklist shape is normatively prescribed by that same property, so a faster
   shape amends the specification first and is Miguel's call under the no-simplify rule. Given that
   `sync export` is a routine command and this repository gates performance at 250k issues, that
   amendment is recorded as the first follow-up rather than an optional one.
4. **This report** (MF-4), with the `.unblock/issues.jsonl` re-export, per `docs/PROCESS.md` §6/§8.

### Round 2 (2026-08-02) — the scale gate rejected the cost that measurement had just recorded

Everything above stood except its conclusion. **The must-fix-1 measurement above was taken on a
laptop; CI took its own, and CI's is the one that counts.** On the runner the same 250k fixture
reported `integrity_check` **5.51s**, the dangling fold **10.72s** and `doctor()` **16.31s** — over
the 15s boundedness guard the cell has always asserted, so the required `scale` job went RED while
every other gate was green. The laptop had reported roughly half of that (fold 4.51s, `doctor()`
7.00s) and had been read as evidence the composition was affordable for v1.0.1. It was not.

**The cause was structural, not a threshold to nudge**, and the diagnosis is the reason no threshold
was touched: the composition read the WHOLE dependency graph and then EVERY issue row through
fully-inclusive filters — hydrating each row's labels, dependencies and comments merely to derive an
id SET — and differenced them in memory. Two full scans and `O(rows)` peak memory. The `doctor` fold
was simply where a test happened to be watching: the agent-facing `diagnostics {kind:"dangling"}`
action paid the identical 10.72s with nothing watching it at all.

**Miguel ruled on 2026-08-02: move the composition into ONE SQL query.** He was offered raising the
guard and dropping the fold, and rejected both — the guard did its job, and dropping the fold would
have left the action just as slow and unwatched.

Spec-first, because the specification pinned the OPPOSITE and that pin had survived three adversarial
rounds. The amendment rides D45 and mints no new decision id (`docs/PROCESS.md` §3: same view, same
finding shape, same order, same trap — the MECHANISM changes, the intent does not), and every
superseded clause is left visibly superseded rather than overwritten: `docs/PRD.md` §4 D45 clause (6)
(plus its FR-15/FR-16 rows and its scope statement, which had said no implementor's method set moves),
`docs/plans/01-design-spine.md` (the §4.1 `dangling` clause, the trait declaration block, the
`Session::doctor()` COST paragraph, and the §3.2.1 bullet whose "no trait signature changes" claim is
now explicitly scoped to the WRITE guard), `docs/plans/crates/unblock-engine.md`,
`docs/plans/crates/unblock-storage.md` and the T2.7 acceptance criteria in
`docs/plans/implementation-plan.md`.

Then the code: **one additive `Storage` method, `dangling_dependencies() -> Vec<GraphEdge>`**, whose
libsql body is a single `LEFT JOIN issues i ON i.id = d.depends_on_id WHERE i.id IS NULL AND
d.depends_on_id NOT LIKE 'external:%' ORDER BY d.issue_id, d.type, d.depends_on_id`. Twelve `impl
Storage` blocks gained it. The engine's `dangling_findings` stayed the ONE HOME and became a thin map
— second read gone, id set gone, in-memory difference gone, **and the re-sort deleted deliberately**,
because a redundant engine-side sort would mask a broken `ORDER BY`.

**The trap survived the rewrite by changing shape, which was the whole risk of this round.** The
retired clause warned that the id set had to be FULLY INCLUSIVE, since default filters exclude closed
and tombstoned rows and would report every CLOSED blocker as dangling. In SQL that becomes: the join
matches on EXISTENCE ALONE and never on status. A single `AND i.status NOT IN ('closed','tombstone')`
in the `ON` clause reintroduces the identical defect, and it is the most natural thing in the world to
write. It is now stated in the trait doc, the query doc, both crate plans, the PRD, the spine, the
acceptance criteria and the gate — and, more usefully, it is **mutation-proved**: with that term added,
`a_closed_deferred_or_tombstoned_blocker_is_not_dangling`
(`crates/unblock-engine/tests/dangling.rs`) reports `blocks -> ub-closed` and `blocks -> ub-tombstoned`
as findings, and the new NFR-16 contract case
`contract_dangling_dependencies_lists_only_absent_targets`
(`crates/unblock-storage/src/testkit.rs`) fails the same way at layer 2. Both were run red, then the
file was restored from a `cp` backup and verified by md5.

**The new numbers, on the same laptop class as the superseded pair so the two are comparable: the fold
costs 88.7µs / 88.4µs / 88.7µs and `doctor()` costs 2.529s / 2.567s / 2.520s, of which
`integrity_check` alone is 2.516s / 2.542s / 2.533s.** The fold went from 4.51s to 88.7µs and is now
~0.004% of `doctor()`: it has stopped being a term. **The 15s guard was not relaxed** — which is the
only reason those numbers mean anything.

Two fakes were given HONEST implementations rather than convenient ones. The `unblock-sync`
`FakeStorage` doubles (`src/testutil.rs`, `tests/fake.rs`) answer over the rows they already hold; the
file idiom for an undriven method there is `unimplemented!()`, and `Ok(Vec::new())` would have been
worse than either — a double that always answers "this workspace is clean" makes every dangling
assertion in that crate vacuously green. The other nine sites are decorators and delegate, as they do
for every non-gated read.

The claim gate moved with the code, since two of its code-side rows would otherwise have certified the
retired design: `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh` gained **PC13** (the join clause)
and **PC14** (the `ORDER BY`), its code-side floor went 12 → 14, and its specification paragraph in
`docs/plans/ci-cd-and-distribution.md` §2.1 gained the matching rules. The existing engine row (PC8)
keeps its anchor — the ONE HOME did not move, only the work behind it — but its rationale no longer
claims a two-read composition.

**On the mutation discipline, including where it did not pay off.** Every coverage claim in this
change names the mutant it kills in its own body, and an independent Verify lens re-ran all seventeen
of them with md5-verified restores: 17 of 17 reproduced, zero false kills, zero cells passing in both
worlds. That is worth recording because this repository has twice shipped false coverage claims.
Three claimed kills did NOT survive and were rewritten as honest declarations instead of quiet
deletions:

- Flipping the case predicate at ONE of its three engine call sites is not a mutant — the whole-ref
  arm, the parse fallthrough and the id-half arm each catch the input independently, so removing any
  one leaves the batch accepted and the cell green. The claim was moved to the single layer-0 home
  (`crates/unblock-engine/tests/create_bulk.rs`).
- Removing the external-target filter from inside the closure walk is an EQUIVALENT mutant: an
  `external:`-prefixed string can never BE a row id, so with the filter gone neither lookup can hit
  and no executable distinguishes the two builds. The cell says so in as many words
  (`crates/unblock-sync/src/export.rs`) and pins what it CAN pin instead — that both external edges
  survive verbatim and the corpus gains no line.
- One property test's docstring claims more than its generator reaches (`crates/unblock-model/src/id.rs`):
  under the case-sensitive mutant the failure came from the pinned-corpus cell beside it, never from
  the property, because the generator effectively never emits an upper-case prefix. Redundant rather
  than false — the load is carried by its neighbours — and the gate recorded it as a should-fix.

**Follow-up work the gate listed** (each its own task, none blocking): linearize the export blocker
closure after amending spine §1.10 property 1; a crate-wide clip sweep of attacker-controlled ids in
`unblock-storage` error constructors; the `issue create {parent: <typo>}` path that still returns
`DATABASE_ERROR` and exit 2 where every D45 path returns `ISSUE_NOT_FOUND` and exit 3; forwarding the
storage error context through the sync boundary so the import refusal is machine-readable; the
unreachable `issue update {parent: null}` detach; batch-scoping the per-record existence memo;
publishing the precedence chain as per-RECORD with a cross-record cell; and deciding whether the
`external:` suffix gets a shape or length bound, stating the answer either way.

## Gotchas

- **A four-leg cell cannot be mutation-proved with one run.** The legs are sequential in one function;
  the first failure hides every later leg. The ladder (lower one leg, re-run, read the offending id
  out of the panic message) is what makes each leg's claim independent — and when the mutant kills an
  EARLIER cell in the suite, the target cell needs its own throwaway single-cell harness or the whole
  ladder is shadowed. Both were needed here.
- **The 250k scale fixture seeds rows with no dependencies, labels or comments**
  (`crates/unblock-storage/src/testkit.rs`, `seed_corpus`), so the measured doctor cost is the
  row-hydration half with the edge half measured at exactly zero. Any future reader comparing against
  a real workspace will see a larger number, never a smaller one. **Round 2 note:** after the SQL
  amendment the same bound points the other way — the empty `dependencies` table is now the query's
  DRIVING table, so a real edge graph costs more in proportion to its EDGE count rather than its ROW
  count, which is exactly what the amendment bought.
- **A laptop measurement is not a gate, and this task learned it the expensive way.** The cost number
  the specification obliged this commit to record was taken locally and understated CI by roughly 2x
  on the same fixture (fold 4.51s vs 10.72s), which was the difference between "affordable, defer the
  alternative to v1.1" and a RED required job. When a number is being used to justify a design choice,
  take it where the gate runs, or state the environment loudly enough that nobody reads it as a budget.
- **The adversarial export corpus only reaches the quadratic branch if the ids ascend ALONG the
  chain.** With the natural ordering the first pass drains the whole set and the same row and edge
  count runs ~100× faster — which is precisely why two Verify lenses measured the same code and
  disagreed about whether the cost was a finding. Both numbers were real.
- **The new claim gate `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh` sweeps with `git grep`,
  which sees only TRACKED files**, so on an unstaged working tree it exits 1 claiming a cascade never
  landed in a file whose very first line carries it. Green after `git add -N`. CI is safe; the message
  names the wrong cause (recorded by the gate as a should-fix).
- **This pass could not run the full workspace gate suite**: the machine had ~1.1 GiB free and a
  private target directory for the whole workspace needs several gigabytes. Everything reported above
  was built and run in a private target directory crate by crate; the un-runnable gates are named
  explicitly in the hand-off rather than reported as green.
- **Every `file:line` citation D45 wrote into a file D45 itself edits is now stale** — spec-first
  writes the pointer before the code moves. Systematic, not careless, and recorded by the gate as a
  should-fix.

## Glossary

Session-local ids used in this report and in this run's issue comments. Durable ids (D45, T1.6,
FR/NFR, `ub-lp9.25`, layer numbers) resolve in `docs/PRD.md`, `docs/plans/` and the tracker and need no
row.

| id | what it is (in words) | where it lives |
|----|-----------------------|----------------|
| MF-1 | Verify must-fix one: the doctor cost measurement the spine obliges this commit to make and record | the `Session::doctor()` row, `docs/plans/01-design-spine.md` §4.1; discharged in that same clause |
| MF-2 | Verify must-fix two: a verbatim-duplicated D-id-range knob in a sibling gate script | `scripts/checks/d44-create-deps-claims.sh` (fixed before this pass) |
| MF-3 | Verify must-fix three: acceptance criterion (3) unmet to the letter — one letter-case spelling on three of the five paths | the cell `contract_external_target_is_accepted_on_every_write_path`, `crates/unblock-storage/src/testkit.rs` |
| MF-4 | Verify must-fix four: the missing implementation run-report, issue comments and JSONL re-export | this file, plus `.unblock/issues.jsonl` |
| MF-5 | Verify must-fix five: the export closure's cost unstated while the sibling doctor clause states its own | `docs/plans/01-design-spine.md` §1.10, normative property 1 |
| SF-1 | Verify should-fix one: the new claim gate false-blocks on an untracked file because it sweeps with `git grep` | `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh` |
| SF-4 | Verify should-fix four: D45 `file:line` citations into files D45 edits are stale | across the D45 spec cascade in `docs/` |
| the case-sensitive mutant | the shared external-target predicate reverted to a case-SENSITIVE prefix comparison; kills every upper-spelled carve-out cell | `unblock_model::is_external_target`, `crates/unblock-model/src/id.rs` |
| the no-carve-out mutant | the same predicate forced to answer "not external" for every input; kills every external cell, lower spellings included | same function |
| the status-aware-join mutant | round 2's mutant: `AND i.status NOT IN ('closed','tombstone')` appended to the new read's `ON` clause, so a CLOSED or TOMBSTONED blocker's edge is reported as dangling — the retired fully-inclusive-filters trap returning in SQL form | `dangling_dependencies`, `crates/unblock-storage/src/libsql/diagnostics.rs`; killed by `crates/unblock-engine/tests/dangling.rs` and by `contract_dangling_dependencies_lists_only_absent_targets` in `crates/unblock-storage/src/testkit.rs` |
| PC8 / PC13 / PC14 | rows of the claim gate's code-side table: the engine ONE HOME, the new read's join clause, and its `ORDER BY` | `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh`, specified in `docs/plans/ci-cd-and-distribution.md` §2.1 |

## Links

- `ub-lp9.25` — the dangling dependency-target guard (decision D45), the priority-0 defect this run
  closes; the comment thread carries the per-phase narrative and links here for depth.
- `.knowledge/wiki/runs/2026-08-01-dangling-blocker-spec.md` — the specification commit for the same
  task, including the two failed design-gate rounds and the two Miguel rulings this code implements.
- `.knowledge/wiki/runs/2026-07-31-create-deps-atomicity.md` — the immediately preceding task, whose
  repair removed the failure that used to mask this defect.
- Key files: `crates/unblock-model/src/id.rs`, `crates/unblock-storage/src/libsql/crud.rs`,
  `crates/unblock-storage/src/libsql/deps.rs`, `crates/unblock-storage/src/testkit.rs`,
  `crates/unblock-sync/src/export.rs`, `crates/unblock-engine/src/diagnostics.rs`,
  `crates/unblock-engine/src/session/lifecycle.rs`, `crates/unblock-engine/tests/dangling.rs`,
  `crates/unblock-engine/tests/scale.rs`, `crates/unblock-cli/tests/dangling_blocker_wire.rs`,
  `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh`, `.github/workflows/ci.yml`.
