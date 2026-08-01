---
name: 2026-08-01-dangling-blocker-spec
description: Specifying the dangling dependency-target guard (decision D45) — a design gate that failed TWICE (eleven blocking findings, then ten more), two Miguel rulings that first reversed the exporter repair into a corpus widening and then forced that widening to follow incoming edges as well, and two open questions closed rather than shipped half-open.
type: run
date: 2026-08-01
branch: ub-lp9.25-dangling-blocker
pr: -
issues: [ub-lp9.25]
---

# Run — dangling blocker target, the specification commit (decision D45)

## Context

Task `ub-lp9.25` (an unblock task id; unblock is this repo's own MCP-based issue tracker, dogfooded) — a
priority-0 data-integrity defect split out of `ub-lp9.20` mid-run by Miguel's ruling and shipping in the same
1.0.1 patch cut, because that task removed the foreign-key failure that used to mask this one. Lifecycle phase:
spec/plan, then the design Review gate — **which FAILED TWICE. Round 1 failed on eleven blocking findings;
round 2, run against the repaired text, failed on ten more.** This report covers all three passes, and it says
so plainly because a run-report that recorded only the successful pass would be worthless: the second failure
is the more instructive one, since round 1's repair introduced defects of its own and re-broke a claim round 1
had got right. `docs/PROCESS.md` §5 escalates a second failure to Miguel rather than looping further, which is
what happened — he ruled on the one round-2 finding that was his, and directed a closed-scope repair of the
other nine. This run is that third pass.
The change is normative text only; the code lands in a second commit. Branch `ub-lp9.25-dangling-blocker`, each
pass written by a single spawned implementer in an isolated worktree, orchestrated from the main session. No
pull request yet.

## What & why

Reproduced live over the MCP wire: a dependency edge may name a blocker issue id that does not exist, the call
returns `isError:false`, and the issue acquires a blocker that can never be created and therefore never closed.
The blocker column carries no foreign key deliberately — a target prefixed `external:` is a legitimate blocker
no key could satisfy — so the repair has to be application-level. The specification commit mints decision D45
and cascades it through the product truth, the interface contract, the task DAG, both roadmaps, six crate plans
and the executable claim gate.

Round 1 of the design gate returned eleven blocking findings and round 2 returned ten; this run repairs all of
them. Authoritative sections
read: `docs/PRD.md` §4 (the D23, D44 and new D45 rows), §5 (the FR-5/FR-7/FR-15/FR-20 requirements), §6
(NFR-13); `docs/plans/01-design-spine.md` §1.9 (the external-target predicate), §1.10 (the export corpus),
§3.1 (the storage error taxonomy), §3.2.1 (the guard, its precedence rank, the diagnostics composition), §5.2
and §5.4 (the wire rejection set and the contract ledger); `docs/plans/crates/` for model, storage, sync,
engine, mcp and cli; and `docs/plans/ci-cd-and-distribution.md` §2.1 for the claim gate. Source was read
directly wherever the text made a claim about behaviour — three of round 1's eleven findings, and four of round
2's ten, were false claims about shipped code that only a source read could settle.

Three findings deserve naming here because they changed the design rather than the prose. Two came from round
1; the third, and the most consequential, came from round 2.

The first is the exporter, from round 1. The landed text had the exporter DROP an edge whose target row the
export corpus excluded. Miguel ruled that out on measured evidence: the blocked-set query has no ephemeral
exclusion, so an issue blocked by an ephemeral row is blocked today, and dropping the edge would have converted
blocked work into ready work in the destination workspace. The corpus WIDENS instead — to the transitive
closure of its blockers, so an excluded row travels with the export whenever it stands in a non-external
dependency relation with a kept row, in EITHER direction. Because that reverses a published clause of decision
D23, and because the reversal falls out of D45's own decision, it rides D45 and mints no further id, but both
rows carry reciprocal pointers.

The second is the precedence chain, also from round 1. The text published an order the specified guard
placement cannot produce: the duplicate-edge rejection fires in the create wrapper, which runs after the shared
per-record body where the new guard goes, so the real order inverts the published pair. The published chain
moved rather than the code — the new link is inserted between the self-dependency and duplicate links, which
preserves every pair the prior decision published, and the sibling path's query order moves to match so one
chain still describes every path.

**The third is round 2's, and it is a hole in the DESIGN rather than in the prose: the corpus widening that
round 1 introduced followed only the edges LEAVING an exported issue.** An issue can be blocked through an edge
stored on the OTHER row. The parent-child edge lives on the CHILD, and the blocked-set query's second pass
marks the EPIC PARENT blocked because it has a non-terminal child — with no ephemeral exclusion, exactly like
the first pass the original ruling was measured on. Measured consequence: a durable epic blocked by an
ephemeral child is BLOCKED today and arrives READY after an export and a re-import, because the child row is
excluded and the blocking edge leaves with it; the third pass then propagates that ready-ness down to every
kept child. So the widening the first ruling introduced silently preserved one of the three sources of
blockedness and broke the other two — and the property the text promised, that an issue blocked before the
export is blocked after it, was false as written. **Miguel's ruling: the closure follows INCOMING edges too**,
pulling any excluded row that BLOCKS an exported issue, whichever side of the edge stores it. It is now stated
as a rule about ROWS rather than about edge lists, because it reads better and generalises correctly: an
ephemeral or wisp row that blocks something exported stops counting as excluded for export purposes. Every
sentence about the closure was re-derived from that rule rather than patched, the walk's specified shape
changed with it (an out-edge queue drain no longer suffices — a row can become eligible because some OTHER row
was just pulled in), and the acceptance criteria gained a second cell per direction, since every cell that
existed passed without the incoming half.

Two further round-2 findings were decided by the orchestrator rather than re-litigated. The dependency-add
source guard had been published as ranking FIRST while also being specified in-transaction, which is
unproducible: the self-dependency rejection returns before the transaction opens. The rank published is now the
rank the code will execute — the source guard sits immediately after the self-dependency check, both being
questions of whether the edge is well-formed at all — and the self-dependency check is deliberately NOT
relocated, since it needs no transaction and moving it would invert a shipped pair to rescue a published
sentence. The text says that explicitly so the next reader does not re-derive the same wrong fix. Separately,
the claim gate's rows over the generated tracker export could never have gone green: the issue's own comment
history legitimately quotes the retired framings, and one issue record is one physical line, so a negative
sweep there would demand deleting the history the process exists to keep. It is resolved the way the sibling
gate already resolves the same shape — allow-list the negative families on that path, and put the teeth in
positive, spelling-independent landings on the same file.

## Outcome

Round 1's eleven must-fixes repaired, plus the exporter ruling and both open questions, and all sixteen of that
round's should-fixes. (That count is recorded because it changed twice and each change is a lesson: this report
first claimed nine, the independent checker counted thirteen against the tree and was right, and the
orchestrator then landed the last three. A self-reported scale is exactly the kind of claim that needs an
outside count.) **Round 2's ten then landed in this pass:** the incoming-edge ruling above; two surviving live
copies of the REPLACED exporter design, one in the product truth's round-trip requirement and one in the sync
crate plan, each contradicting the same file it sat in; the source-guard rank; the tracker gate rows; a
retryability claim about the database error code that was false at all three authority tiers; an entry-point
count that said the shared body reaches four of five paths where the product truth says three (four plus two
named siblings is six on a five-path list); a false claim that the doctor entry point sits behind the health
feature gate, which had also left the interface contract and the engine crate plan disagreeing about gating the
shared composition — a gated composition could not have compiled the new arm at all; six stale claims that the
diagnostic set still has seven kinds, two of them asserting the set is UNCHANGED while another line in the same
crate plan already said eight; and one pre-landing of the new action in the published surface document, which
violated the commit's own sequencing rule and was reverted. Notable landings beyond those:

- A false behavioural claim about the bulk-create path was struck from all three authority tiers. That path
  already rejects an unknown blocker today, whole-batch, with a validation error — it is the template for the
  batch-aware predicate, not a hole. Its only real defect is that it rejects a correctly-spelled external
  target, which the carve-out relaxes.
- Both open questions were closed rather than left for the implementer: the dependency-add path guards its
  edge source too (today the same typo yields a database-level failure or a clean not-found depending on which
  field carries it), and a foreign one-shot import file carrying a dangling edge is rejected, never repaired,
  on the stated principle that the exporter may widen but the importer may never invent.
- Three executable obligations that were satisfiable vacuously now have teeth: the range-pin gate row became
  row-anchored on the normative line (proven by mutation, below), the round-trip property moved off a suite
  that cannot host it onto the integration suite that can, and the anticipating wire cell's conditional branch
  is specified for deletion rather than re-balancing.
- The new listing view had no continuous-integration job at all — the findings are composed in the engine while
  every shipped testkit test step is storage-only — so the job step is now specified, with the wiring riding
  the implementation commit.

Gates run in the worktree after each repair pass: the documentation lint, the claim-sweep script, the knowledge
lint, the formatter check and the layering check. Verdicts are recorded in the task's issue comments.

**The sequencing rule this commit obeys, restated because round 2 caught it being broken.** The cascade lands
in TWO commits — this specification commit carries normative text only, the implementation commit carries the
code. So no existing contract-version literal moves here, and no file that makes PRESENT-TENSE claims about the
shipped surface may yet be updated to describe the new behaviour. The published surface document is the
specific trap: it publishes the live tool and action surface and a sibling gate pins it exclusively on the
contract version, so adding the new action there before the code exists publishes a falsehood AND turns that
required job red. Naming the next contract version in NEW normative prose inside the planning documents is
correct and is what this commit does.

## Gotchas

- `git checkout -- <file>` restores to HEAD, not to the working state. Used to undo a deliberate mutation
  during a non-vacuity proof, it silently discarded every edit made to that file in this run — including the
  applied patch's own hunks. Recovery: re-apply the single-file hunk extracted from the patch, then redo the
  edits. Prefer a scratch copy over `git checkout` when the file is uncommitted.
- The claim gate's required-landing table only supported presence predicates, so a row meant to pin a normative
  literal in `docs/plans/ci-cd-and-distribution.md` passed while the normative statement carried the retired
  value — the file's own explanatory prose satisfied the check. Two changes were needed together: a new
  row-anchored table in `scripts/checks/d44-create-deps-claims.sh`, and removing the live literal from prose so
  the anchored line carries exactly one occurrence. Either alone still passes vacuously.
- Three of the eleven findings were claims about shipped behaviour that read plausibly and were false. Each was
  settled only by opening the source: `crates/unblock-engine/src/session/bulk.rs:378-388`,
  `crates/unblock-storage/src/libsql/crud.rs:57-62`, and `crates/unblock-storage/src/libsql/query.rs:288-294`.
  A fidelity review that checks whether the brief landed cannot catch a claim that is false in the brief.
- A citation can be correct about what lines ARE and wrong about what they are cited FOR. The product truth
  cited the sibling guard site at the seven-column insert rather than at the guards, and the two sibling sites
  were cited at different granularities. Both are now function-level and consistent.
- **A repair pass introduces its own defects, and the second gate is not a formality.** Round 1's corpus
  widening — itself a ruling that fixed a real hole — shipped with a hole of the same shape one level down,
  and round 1's repair of the retryability wording INVERTED a claim round 1 had originally got right. Budget
  for a second adversarial pass over a repair, not just over the original.
- **A closure over a graph is only as correct as the direction its evidence was measured in.** Two of round
  2's three reviewers affirmatively CLEARED the export closure, both having verified the blocked-set query's
  FIRST pass and neither having looked at the second. The defect was off their axis, not absent. When a rule
  is justified by one measured query, check whether the same file contains a sibling query that decides the
  same predicate the other way round.
- **The residue of a replaced design hides where nothing lints.** All three survivors of round 3 sat in the
  two files outside every lint corpus and outside the claim gate's issue-anchored rows — the plans index and
  one roadmap paragraph. A design that changes mid-cascade needs a repo-wide re-grep for the SUPERSEDED rule,
  not only a check that the new rule landed.
- **A worktree based off the default branch will not accept a patch cut against a feature branch's tip.** Here
  exactly one generated file — the committed tracker export — differed by one commit, and `git apply` refused
  the whole patch for it. Materialising that one file at the patch's base revision (`git show <rev>:<path>`,
  a plain write, no branch operation) and then applying is enough; the resulting diff must then be taken
  against that same base so it still applies where it is destined.

## Glossary

The design Review gate ran twice and FAILED both times, and **each round numbered its own blocking findings
from one independently** — so the spelling `MF-3` means one thing in round 1's verdict and a different thing in
round 2's. Every row below therefore states BOTH meanings for the spelling it defines; a reader who meets a
bare code in this task's issue comments must first check which round the comment belongs to. Round 1 raised
eleven (must-fix one through eleven), round 2 raised ten.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| MF-1 | **Round 1:** the published precedence chain no order of the specified guard placement can produce. **Round 2:** the product truth's testability requirement still published the REPLACED exporter design ("the exporter now drops it"), contradicting the same file three times over and outranking the interface contract, so an implementer resolving it by the authority hierarchy would have shipped an edge-dropper. | Both gate verdicts; round 1 repaired at `docs/plans/01-design-spine.md` §3.2.1, the rank bullet; round 2 at `docs/PRD.md` §6, NFR-16 obligation (1) |
| MF-2 | **Round 1:** a behavioural claim about the bulk-create path that was false before and after the change, in all three authority tiers. **Round 2:** a SECOND live copy of that replaced exporter design, in the crate plan that owns the exporter, contradicting its own export row. | Both verdicts; round 1 at `docs/PRD.md` §4 clause (10)(i), spine §5.4, `docs/plans/00-roadmap.md` §1; round 2 at `docs/plans/crates/unblock-sync.md`, the bd-import row |
| MF-3 | **Round 1:** the required run-report continuous-integration job would have blocked this diff — script and rendered-roadmap paths but no run-report. **Round 2:** the dependency-add SOURCE guard was published as ranking first while also specified in-transaction, which no implementation can produce, because the self-dependency rejection returns before the transaction opens. | Both verdicts; round 1 repaired by this report; round 2 at `docs/PRD.md` §4 D45 clause (11), spine §3.2.1 and `docs/plans/crates/unblock-storage.md` |
| MF-4 | **Round 1:** the tracker-export rows of the claim gate were escapable — one issue record is one physical line, so a single new comment cleared every forbidden family at once. **Round 2:** the export closure walked outgoing edges only, so the published property that an issue blocked before the export is blocked after it was FALSE — the epic-parent shape blocks through an edge stored on the child's row. Miguel's ruling: follow incoming edges too. | Both verdicts; round 1 at `docs/plans/ci-cd-and-distribution.md` §2.1; round 2 re-derived across `docs/PRD.md` §4 clause (5) and FR-7, spine §1.10, `docs/plans/crates/unblock-sync.md`, `docs/plans/implementation-plan.md`, both roadmaps and the plans index |
| MF-5 | **Round 1:** the decision-range pin passed vacuously — a file-level token check on a document that also quotes the range in prose. **Round 2:** the database error code was published as RETRYABLE at all three authority tiers and is not; round 1 had this right and its repair inverted it. | Both verdicts; round 2 verified against `crates/unblock-error/src/code.rs:335-348` and struck at the three sites |
| MF-6 | **Round 1:** the dangling-id corpus was pinned to a fully-inclusive filter and then glossed as the export corpus, which is narrower. **Round 2:** the task DAG said the shared insert body reaches four of the five entry points where the product truth says three — and four plus two named siblings is six on a five-path list. | Both verdicts; round 2 at `docs/plans/implementation-plan.md`, the T1.6 row |
| MF-7 | **Round 1:** the round-trip obligation was hung on a suite that never touches storage, export or import. **Round 2:** the claim that the doctor entry point sits behind the health feature gate is false — the method is unconditional and only its body blocks are conditional — and the interface contract and the engine crate plan then disagreed about gating the shared composition, which a `--no-default-features` build could not have compiled. | Both verdicts; round 2 verified against `crates/unblock-engine/src/session/lifecycle.rs:167-184` and repaired in spine §4.1 and `docs/plans/crates/unblock-engine.md` |
| MF-8 | **Round 1:** two crate plans contradicted themselves — the widened diagnostics taxonomy in the type column, the old taxonomy in the test column. **Round 2:** the claim gate's rows over the generated tracker export could never go green, because the issue's comment history legitimately quotes the retired framings and one record is one physical line. | Both verdicts; round 2 at `docs/plans/ci-cd-and-distribution.md` §2.1, the D45 sub-check paragraph |
| MF-9 | **Round 1:** the wire cell written to anticipate this change branches on whether the call errored, so it passes in both worlds. **Round 2:** six stale claims that the diagnostic set still has seven kinds survived across two crate plans, two of them asserting the set is UNCHANGED, while another line in one of them already said eight. | Both verdicts; round 2 at `docs/plans/crates/unblock-engine.md` and `docs/plans/crates/unblock-mcp.md` |
| MF-10 | **Round 1:** the exporter repair silently converted blocked work into ready work — superseded by Miguel's corpus-widening ruling. **Round 2:** the published surface document pre-landed the new action under a heading that pins the contract at the CURRENT version, publishing a false present-tense claim and breaking a sibling gate; reverted. | Both verdicts; round 2 reverted in `README.md` |
| MF-11 | **Round 1 only** (round 2 raised ten): the new listing view's acceptance cell executed in no continuous-integration job. | Round 1 verdict; repaired in the ci-cd D45 sub-check and the engine crate plan |
| SF-1 … SF-16 | The sixteen non-blocking should-fixes from ROUND 1's verdict, numbered in that document only. ALL sixteen landed: thirteen in the writer's pass because they touched the same sentences the must-fixes rewrote, and the last three in the orchestrator's follow-up — the published action list that still showed seven actions, the unpinned contract bytes of the new variant's doc comment, and the unacknowledged cost profile of the doctor fold. Round 2's should-fixes carry no letter code; they are plain numbered items in that verdict's own should-fix section. | Round 1 verdict; all sixteen are visible in this commit's diff |
| Lens A / Lens B / Lens C | The three independent reviewer perspectives used in BOTH gate rounds — implementability and code-truth, cross-document fidelity, and adversarial/citation accuracy respectively. Named here only because the verdicts attribute findings to them, and because round 2's central finding came from one lens alone while the other two had affirmatively cleared the same text. | Each gate verdict's disagreement section |
| RW1 | The row-anchored landing added to the claim-sweep script in this run: the decision-range literal must appear on the class-(a) statement line of the continuous-integration document, not merely somewhere in the file. | `scripts/checks/d44-create-deps-claims.sh`, the `REQUIRE_ROW` table |
| R16 | The claim-gate row pinning the decision-range bump site in the always-on contract document. Left file-level: that file carries exactly one occurrence. | `scripts/checks/d44-create-deps-claims.sh`, the `REQUIRE` table |
| R17 | The claim-gate row that pinned the same range in the continuous-integration document and passed vacuously. Removed as a file-level row and re-landed row-anchored. | Formerly `scripts/checks/d44-create-deps-claims.sh`, the `REQUIRE` table |
| R18 | The claim-gate row pinning the decision-range bump site in the documentation-lint source comment. Left file-level for the same reason as the first. | `scripts/checks/d44-create-deps-claims.sh`, the `REQUIRE` table |

## Links

- `ub-lp9.25` — dangling dependency target on every edge-writing path; the task this run specifies. Ships in
  the same 1.0.1 cut as `ub-lp9.20`.
- `ub-lp9.20` — the one-transaction create with implicit edge ownership; this task was split out of it, and its
  shipped clause is the one D45 partially reverses.
- Files touched, all nineteen (repo-relative — an absolute path from one machine is noise to every other
  reader, and this repository is public): `docs/PRD.md`, `docs/plans/01-design-spine.md`,
  `docs/plans/implementation-plan.md`, `docs/plans/00-roadmap.md`, `docs/plans/README.md`,
  `docs/roadmap.html`, `docs/plans/ci-cd-and-distribution.md`, the six crate plans under
  `docs/plans/crates/` (`unblock-model.md`, `unblock-storage.md`, `unblock-sync.md`, `unblock-engine.md`,
  `unblock-mcp.md`, `unblock-cli.md`), `scripts/checks/d44-create-deps-claims.sh`, `xtask/src/doc_lint.rs`,
  `CLAUDE.md`, the committed tracker export `.unblock/issues.jsonl`, this report, and the wiki index
  `.knowledge/wiki/index.md` that lists it. **`README.md` is deliberately NOT in that list:** round 1 had
  pre-landed the new action there and round 2 caught it as a sequencing violation, so the change was reverted
  and lands with the implementation commit instead.
- Prior related run-report: [2026-07-31-create-deps-atomicity](2026-07-31-create-deps-atomicity.md) — the run
  that split this task out and shipped the decision D45 partially reverses.
