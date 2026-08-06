---
name: 2026-08-06-envelope-id-reject
description: Answering an un-decodable envelope id instead of losing the frame (decision D47, tracker ub-cnv) — a request that vanished into the notification variant and hung its client, of which duplication was only the minority route; four binding rulings, two impossibilities escalated rather than absorbed, a delta-verify that failed inside the repair of its own findings, an implementation split across a session limit, and a mutation matrix measured by a lens the implementer never saw.
type: run
date: 2026-08-06
branch: ubcnv-envelope-id-reject
pr: -
issues: [ub-cnv]
---

# Run — the un-decodable envelope id (D47)

## Context

Tracker issue `ub-cnv` — originally filed as "a duplicate envelope `id` reinterprets a request as a
notification, so the client gets nothing" — taken as the active item in the v1.0.1 maintenance slot
(`docs/plans/00-roadmap.md`, the v1.0.1 section), off `main` at `bd39fa2`. The whole lifecycle ran across
2026-08-05 and 2026-08-06 as per-phase Workflows (`docs/PROCESS.md` §4): Understand (three adversarial
lenses plus a coordinator), Decide (Miguel resolved four forks), Spec/Plan (an empirical probe, then three
planners, then a coordinator), the design Review gate, Implement (two runs, one worktree), the Verify
quality gate (four lenses plus a coordinator) and this Track step.

All work sits on `ubcnv-envelope-id-reject`, one branch off `main`, in a single isolated worktree. No pull
request existed when this report was written: the Track step (a single writer in that same worktree)
produced the disclosure cascade, the tracker re-export and this report, and the pull request follows.

## What & why

rmcp decodes a JSON-RPC frame into a serde UNTAGGED union whose variant order is Request, Response,
Notification, Error. A request requires an `id` that deserializes as a number-or-string; the notification
variant has no `id` field at all, and no rmcp struct denies unknown fields. So ANY frame whose `id` fails
to decode falls through to the notification variant with the surplus `id` DROPPED — no reply, no store
effect, nothing on stdout — while the client that sent an id waits on an untimed await forever. As the
FIRST frame on a connection it was worse than silent: rmcp's `expect_next_message` returns
`ExpectedInitializeRequest` for any non-request message in the initialize slot, so the server died with
exit 1.

The issue's own title named the minority route. Ten shapes were probed live on the shipped binary and all
ten were silent: a duplicated id (equal or differing), `null`, `{}`, `true`, `[1]`, `1e2` (while the same
number written `100` is answered), `100.0`, `2^63` (while `i64::MAX` is answered), and the `id` key spelled
with a Unicode escape for its first character. Only three of the ten carry a duplicate anywhere. A fix keyed on duplication would have
closed three of ten and gone green.

The change makes the owned transport, inside the one arm where a delivered message exists and the raw line
is still in scope, run a `DeserializeSeed` over the ROOT object that keeps every top-level `id` member's
value verbatim, compares keys and values DECODED rather than as raw spans, and yields absent / recovered /
unusable. On recovered it answers `-32600 Invalid Request` ON that id; on unusable it answers with the id
omitted; then it DROPS the frame. A frame with NO `id` member is a genuine notification, is out of the
class by decision, and behaves exactly as before.

Authoritative material read before writing anything: `docs/PRD.md` §4 (D43, whose sixth clause deferred
exactly this frame and is now superseded; D42; D35, the GA semver clause; and the new D47 row), NFR-18
(the unbounded transport read and its already-disclosed double pass),
`docs/plans/01-design-spine.md` §5 (the MCP contract surface), `docs/plans/crates/unblock-mcp.md`
(the `src/wire.rs` and `src/envelope_id.rs` rows), `docs/plans/ci-cd-and-distribution.md` §2.1 and §2.3,
and `docs/PROCESS.md` §3 (new decision id versus inline amendment), §6 and §8.

**The four binding rulings.** Miguel took the recommendation on each, and each one is load-bearing rather
than a preference:

- **Scope is the whole un-decodable-id class**, bound by decode OUTCOME, not by duplication. Closing only
  the duplicate would have let the issue close claiming no frame leaves a client waiting while five
  reproduced shapes still did.
- **The channel is out-of-band `-32600`, answered ON the recovered id.** Not aesthetics: an rmcp client
  DROPS an error carrying no id and its request future has no timeout, so an id-less reply leaves the
  caller stuck exactly as the silence did — it only adds a byte a human can read. Declined: always-null,
  always-omitted, and ratifying the silence.
- **Acceptance criterion 5 was re-scoped** from "no frame can leave a conforming client waiting" to "no
  frame that rmcp itself would answer may go unanswered". Forced, not cosmetic: our forked compatibility
  filter deliberately drops a `notifications/*` frame whose params rmcp cannot type, and satisfying the old
  wording literally would have meant diverging from rmcp on purpose — the exact thing the byte-identity
  harness exists to prevent.
- **The pre-handshake fatality splits.** Its removal rides this fix as a consequence of answer-and-drop
  (confirmed by measurement, not assumed); the separate defect of writing a non-JSON-RPC blob onto stdout
  is `ub-og3`.

**Two impossibilities were escalated rather than absorbed.** Both are the kind a team quietly "handles"
and then documents wrongly:

- **The explicit-null reply the dependency type cannot serialize.** The Decide ruling said the fallback id
  is JSON `null`, which is what JSON-RPC 2.0 literally demands. `rmcp::model::JsonRpcError.id` is
  `Option<RequestId>` under `skip_serializing_if = "Option::is_none"`, so no value of that field can ever
  serialize to `"id":null`: `None` OMITS the member. The literal ruling is therefore untakeable without
  hand-rolling a reply outside rmcp's own encoder. It was escalated and the departure is written into the
  decision row instead of being silently absorbed — the same trade the shipped `-32700` arm already made,
  and it costs the peer nothing measurable, since both spellings decode to `id: None` and are both dropped
  by the client loop.
- **The eight visitor arms that are an equivalent mutant.** The seed's fallback visitor has eight arms that
  no byte corpus can grade: delete all eight and `deserialize_any` fails, `scan` maps a failed seed to
  absent, and the verdict is IDENTICAL either way — the whole suite stays green. That is an equivalent
  mutant by construction, not a coverage hole, and it is stated in A14's own doc so the opposite reading is
  not available to the next reader.

## Outcome

Nine commits on `ubcnv-envelope-id-reject`: the spec commit minting D47 and its whole cascade; the
transport change; the byte-identity pin rebuilt as three tiers; the live-duplex, real-stdio and
pre-handshake coverage; the required-landing gate with its CI step; the tracker re-export the gate forced
early; the executable-bit and row-anchoring repair of that gate; the property test and root-shape cells;
and the concurrency-residual disclosure. This Track step adds the disclosure cascade for the residual's
issue id, the second re-export and this report.

**Gate history, in full.** Design Review round 1 = PASS WITH MUST-FIXES, ten items, all corrections to the
package with no redesign asked for — and that gate had to be run twice, because the first attempt lost all
four agents to a session limit before producing anything. A single-writer revision applied the ten. The
delta-verify then FAILED on three defects, and the shape is the instructive part: each of the three sat
INSIDE the repair of its own item — the revision introduced them while landing the fix it was asked for.
Under the two-iteration rule this escalated to Miguel, who authorised the orchestrator to close the three
directly; they were closed and verified at their own sites. Implement then ran across TWO runs: the first
implementer was killed by a session limit after four commits, and a continuation was launched into the
SAME worktree, verifying what was already committed before adding anything. The Verify gate returned PASS
WITH FOUR must-fixes; a fix pass discharged them; the re-verify returned PASS.

**The mutation result.** The matrix was measured by a lens deliberately withheld from the implementer,
because self-measured coverage is what this repo has learned not to trust: **26 of 26 catalogue mutants
killed, zero survivors**, plus roughly 30 further mutants raised across two independent lenses, all killed
or explicitly declared equivalent. Proven on the wire as well, before and after, against the two binaries
rather than by reading: every in-class frame that was silent on `main` is now answered — the same id twice
comes back carrying THAT id, two different ids and `null` and the exponent spelling come back with the id
omitted, a repeated string id answers on the string — the frame with NO id gets NO response on BOTH
binaries, a clean request afterwards is still answered, and in the pre-handshake position the same frame
went from killing the process to being answered, after which a full handshake and a further request
succeed on that connection.

**Residuals left OPEN and disclosed rather than claimed shut**, each with its own issue: `ub-788` (the
`-32700` arm omits a readable id unconditionally, so a duplicated `method`/`jsonrpc` still leaves an rmcp
client pending); `ub-og3` (the CLI writes a non-JSON-RPC structured-error blob onto the framing channel on
the pre-handshake death, embedding a `Debug` rendering of attacker-controlled bytes); `ub-nbz` (the
`-32600` is LOST whenever rmcp cancels the `receive()` future — measured from one unreplicated harness at
0 of 40 idle, 25 of 40 with four requests in flight, 39 of 40 with eight; pre-existing, shared by the
shipped `-32700` arm, and unrelated to the pre-handshake case, which runs with nothing in flight); and
`ub-q1u` (an intermittent libsql parallel-first-write stress failure, unrelated to this branch, filed with
the caution that "it is just the test" is the diagnosis that must be PROVED rather than assumed).

**What this Track step landed.** The placeholder `ub-TBD` in the decision row and in the transport's module
documentation became `ub-nbz`, the real issue. The v1.0.1 roadmap card's "Three residuals" opener lost its
count word entirely — Miguel's ruling, and the repo's own rule that the LIST is the rule and carries no
count — and both the markdown card and `docs/roadmap.html`, the PUBLISHED artefact that sits outside every
lint corpus, gained the concurrency residual. Two sentences in the module documentation were aligned with
the decision row's measured figures and its one-harness caveat. The claims gate gained P15, mirroring P10
for the second residual id the row now names, with its P-table floor moved 12 → 13. And commit `4cbe9e2`
was REWORDED (safe while unmerged, impossible after): its body claimed the property cell "ALONE fails and
shrinks to `Number(-1)`" under the type-table mutant, which cannot be true, because A12 splices the same
boundary values through the real decoder as its oracle and must fail on the same counterexample. The new
body states the defensible thing: A15 is non-vacuous because under a collector mutation a fixed value table
cannot express — truncating a string id in the collector — it is the ONLY failing cell, while under the
type-table mutant A15 and A12 fail together, as they should. The rebase was verified content-neutral: the
tree hash before and after is `798779b`, byte-identical.

## Gotchas

- **A required check whose rows pin landings from a LATER step than the commit that mints it is red by
  construction.** The gate minted in the implementation commit requires `ub-788` to be present in the
  committed tracker record, while the process assigns that re-export to Track, after both gates. The
  implementer correctly refused to weaken the assertion and could not discharge it either (no tracker
  tools, no CLI verb that creates issues, the live database in a checkout it must not touch, and
  hand-editing a generated export is what the export model forbids); the orchestrator discharged it out of
  band. This run hit the same shape a second time with P15 and `ub-nbz`, and defused it by ORDERING: the
  tracker re-export commit lands BEFORE the commit that adds the row.
- **A claims-check script committed without its executable bit passes every local `sh script` invocation
  and dies in CI.** The workflow step runs it as a bare relative path; `sh scripts/checks/…` never
  exercises the permission bit, so the defect is invisible to exactly the command a developer reaches for.
  Repaired in `6fac170`. Every verification in this run was therefore run as a bare relative path.
- **The published HTML roadmap is outside every lint corpus.** `cargo xtask doc-lint` sees 19 markdown
  documents and none of them is `docs/roadmap.html`; the only thing in CI that notices a stale rendered
  card is the D47 gate's own P11 row. A disclosure cascade that stops at the markdown ships a public page
  contradicting the decision row.
- **A count word in a card whose list grows is a defect with a five-time history at that exact site.** The
  card said "Three residuals" and then a fourth residual was disclosed by the same decision. The fix is not
  "Four" — it is deleting the count and letting the list speak.
- **Two independent cells splicing the same boundary values cannot grade each other's mutant.** The
  original commit body read as if the property cell alone caught the type-table mutation. It could not:
  its sibling table draws its oracle from the same decoder over the same values, so any counterexample is a
  counterexample to both. A non-vacuity claim is only worth what its DISCRIMINATING mutant proves — here, a
  collector-level truncation the fixed table cannot express.

## Glossary

Session-local ids used above or coined by this run. Durable ids (`D47`, `ub-cnv`, `ub-788`, `ub-og3`,
`ub-nbz`, `ub-q1u`, FR/NFR) resolve in the PRD, the spine and the tracker and carry no row.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| A5 | the cell asserting a wrongly-TYPED envelope id is unusable | `crates/unblock-mcp/src/envelope_id.rs:296` |
| A12 | the cell asserting the scan agrees with rmcp's own id deserializer over a FIXED table of 14 boundary values — a named-boundary regression corpus, not the general property | `crates/unblock-mcp/src/envelope_id.rs:500` |
| A14 | the cell covering the ROOT shapes that are not an object (every scalar root, an array root, a nested id) — and whose doc states that the eight visitor arms it exercises are NOT graded by it | `crates/unblock-mcp/src/envelope_id.rs:380` |
| A15 | the property test itself: the scan agrees with rmcp's id deserializer for ANY value spliced as the envelope id, over a bounded recursive value strategy | `crates/unblock-mcp/src/envelope_id.rs:573` |
| F17 | the byte-corpus entry for the deliberate parity drop — an id-carrying `notifications/*` frame whose params rmcp cannot type, which rmcp drops and so do we | `crates/unblock-mcp/src/wire.rs:611` |
| P10 | the gate row requiring the residual id `ub-788`, which the decision row names, to exist in the committed tracker record | `scripts/checks/d47-envelope-id-claims.sh`, the P table |
| P11 | the gate row requiring the RENDERED roadmap to name the decision — the only CI check that can see that file | `scripts/checks/d47-envelope-id-claims.sh`, the P table |
| P15 | the row this run added, mirroring P10 for the second residual id the decision row now names (`ub-nbz`) | `scripts/checks/d47-envelope-id-claims.sh`, the P table |
| Q11 | the row-anchored pin that keys are still compared DECODED, anchored on the collector's own key loop | `scripts/checks/d47-envelope-id-claims.sh`, the Q table |
| Q12 | the row-anchored pin that the recovered arm still answers ON the recovered id, anchored on that argument's own production line | `scripts/checks/d47-envelope-id-claims.sh`, the Q table |

## Links

- `ub-cnv` — the tracker issue this implements; its comment thread is the authoritative per-phase
  narrative (Understand, Decide, Spec/Plan, both gate closures, Implement) and this report is the depth
  behind it.
- `ub-788` — the `-32700` arm omits a readable id unconditionally; blocked on this work landing.
- `ub-og3` — the non-JSON-RPC structured-error blob written onto the framing channel.
- `ub-nbz` — the out-of-band reply lost whenever rmcp cancels the `receive()` future.
- `ub-q1u` — the intermittent libsql parallel-first-write stress failure, reported rather than swept.
- Decision: `docs/PRD.md` §4, the D47 row (and D43 clause (6), which it supersedes with reciprocal
  cross-references).
- Release note: `docs/plans/00-roadmap.md`, the v1.0.1 slot, and its rendered twin `docs/roadmap.html`.
- Plan: `docs/plans/crates/unblock-mcp.md`, the `src/wire.rs` and `src/envelope_id.rs` rows.
- Key files: `crates/unblock-mcp/src/envelope_id.rs`, `crates/unblock-mcp/src/wire.rs`,
  `crates/unblock-mcp/tests/envelope_id_duplex.rs`, `scripts/checks/d47-envelope-id-claims.sh`.
- Prior related run-report:
  [2026-07-29-duplicate-key-execution-flip](2026-07-29-duplicate-key-execution-flip.md) — the decision one
  step earlier, which met this frame, described it correctly, and deferred it on reasoning this run
  falsified on raw bytes.
