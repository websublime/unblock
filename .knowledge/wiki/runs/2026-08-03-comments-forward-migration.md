---
name: 2026-08-03-comments-forward-migration
description: Repairing the missing forward migration for the comments columns (decision D46) — a database no shipped binary could read, an issue whose own proposed fix provably could not work, five design-gate rounds, and a Verify gate that caught the repair's own error message lying about the state it names.
type: run
date: 2026-08-03
branch: ub-lp9.13-impl-r2 (tracking: ub-lp9.13-track)
pr: -
issues: [ub-lp9.13]
---

# Run — the comments forward migration (D46)

## Context

Issue `ub-lp9.13` — "no forward-migration for the comments schema; pre-comments databases break on the GA
binary" — a dogfood finding from 2026-07-20, taken in the v1.0.1 maintenance slot. The whole lifecycle ran
in one day, 2026-08-03: Understand (three read-only specialists plus a coordinator), Decide (Miguel ruled
six forks), Spec/Plan with the design Review gate run FIVE times, Implement, then the Verify quality gate
run twice. Branches: `ub-lp9.13-spec-r2` … `-spec-r5` for the specification rounds, `ub-lp9.13-impl-r2`
for the implementation (head `c9202ef`), `ub-lp9.13-track` for this tracking commit. Seventeen rulings by
Miguel across the run. No pull request existed when this report was written; it lands with the tracker
re-export in the tracking commit.

## What & why

A database created before the `comments.updated_at` / `comments.redacted_at` columns shipped (commit
`c1cf5bb`, 2026-07-17) could not be read by any current binary. The schema had exactly one versioning
mechanism, `PRAGMA user_version`, the current version constant was `1`, the migration ladder was an empty
slice, and the forward-step machinery had never executed in production or in a single test. So `migrate`
saw stamp 1, compared it with constant 1, and returned success having run zero statements.

Authoritative material read before anything was written: `docs/PRD.md` §4 (D37, the decision that added
the comment columns and is the only v1-scope decision that never states its migration posture; D35, the
semver clause; and the new D46 row), `docs/plans/01-design-spine.md` §3.2 (the `Storage::migrate`
declaration, whose doc comment was EMPTY — the root cause of the defect class, not just of this instance)
and §5.4 (the contract ledger), `docs/plans/crates/unblock-storage.md` (which contradicted itself in
adjacent rows: one prescribed the in-place DDL edit that caused this, the next forbade in-place edits by
name), `docs/plans/implementation-plan.md`, `docs/plans/ci-cd-and-distribution.md` §2.1 and §2.3, and
`docs/PROCESS.md` §3 (the decision-id cascade), §6 (tracking) and §8 (the knowledge layer).

## Outcome

Landed on `ub-lp9.13-impl-r2`: the specification commit that mints D46 (documents only), the
implementation commit, and five fix commits. A stale database is now migrated forward automatically on
open; a database whose stamp lies about its shape gets a message that names the stamp as the false thing
and the one recovery that works; and `unblock migrate` reports the repair it performed instead of a
tautology. Gate history: the design Review gate returned FAIL in rounds one, two, three and four and,
in the scoped fifth round, FAIL on a single must-fix that was then closed and verified without a sixth
round; the Verify gate returned FAIL twice, the second time narrowly, with every code defect discharged.

The reasoning behind the rulings that will constrain future schema work — this is the part that does not
live in the diff:

- **Why the embedded DDL constant is FROZEN.** `SCHEMA_SQL` now describes the BASELINE shape (version 1),
  not the current one; every post-baseline element is a ladder step. If the DDL kept absorbing new
  columns, a fresh database would be created already carrying a column that a later step then re-applies,
  and fresh initialisation would hard-error with a duplicate column — which is precisely the failure this
  decision exists to end, merely relocated from old databases to new ones. The freeze is what makes "a
  stamped version implies a known shape" provable rather than asserted. It is enforced by a
  `const`-evaluated content digest over the DDL plus both version constants plus the ladder, asserted
  against a hand-blessed literal (`crates/unblock-storage/src/libsql/schema.rs:312`), so an unaccompanied
  DDL edit is a red BUILD under every job that builds. Every legitimate schema change must re-bless that
  literal; the recurring cost was accepted deliberately, because it is the mechanism.
- **Why the ladder runs on every fresh database.** Fresh initialisation applies the frozen baseline,
  stamps `BASELINE_SCHEMA_VERSION` (`schema.rs:52`) and then FALLS THROUGH the ladder instead of stamping
  the current version. Two consequences worth keeping: a fresh database and a migrated one converge by
  construction rather than by a parity test's good intentions, and every step is exercised by every test
  that creates a workspace, not only by an old-database fixture that somebody has to remember to write.
- **Why the migration runs implicitly on open.** The step is two nullable column additions with no data
  rewrite, taking milliseconds. The policy is now written down rather than left as a precedent: implicit
  for additive/nullable steps, EXPLICIT for anything destructive, data-rewriting or long-running. It was
  named now, ahead of need, because a shared-state migration in a later release will not be safe to run
  unattended inside an MCP server's startup.
- **Why an additive contract bump was acceptable inside a patch release.** Attaching a self-correction
  hint to the stale-schema failure moves `ErrorCode::SchemaMismatch`'s published hint shape off "none",
  which is a byte inside the capabilities document, so the contract hash is re-pinned and the contract id
  bumps `unblock.mcp.v1.8` → `unblock.mcp.v1.9`. Additive, therefore non-breaking under D35 — and the
  fourth such bump in this same v1.0.1 slot, which is why it is stated rather than defended.
- **Why the honest message is the whole remedy for a lying stamp.** There is no in-product rescue for
  that state: `sync export` is itself a casualty of the same shape, so the product cannot get the user's
  data out. The message therefore names the state truthfully and gives the only recovery that works —
  reset the stamp to the baseline, then run migrate — which is safe exactly because the step inspects the
  table before acting and leaves every existing row untouched.
- **Ruling seventeen, recorded because it cost two review rounds.** The knowledge-layer run report and the
  tracker re-export belong to the TRACKING step, not to the implementer, and the Verify gate reports them
  as known pending rather than failing on them. `docs/PROCESS.md` binds them to the work commit in §6/§8
  while placing tracking after the gates in §2/§4, so the gate and the implementer could each reasonably
  believe the other owned them. The required continuous-integration job remains the safety net.

## Gotchas

Things that were BELIEVED and turned out FALSE:

- **The fix this issue proposed could not work.** The issue text said "schema_version bump + ALTER". A
  stale database and a current one BOTH report `user_version = 1`, because the stamp never witnessed the
  column set. A step keyed on the version alone therefore hard-errors with `duplicate column name:
  updated_at` on every database created since 2026-07-17, GA included. The resolution is a ONE-TIME
  shape-sensing step that inspects the `comments` table before acting; from step three onward the
  invariant holds and is written into the spine's `migrate` declaration.
- **"Nobody outside could hold an old database"** — the premise that would have licensed the cheap answer.
  False: prereleases `v1.0.0-rc.2` and `v1.0.0-rc.3` are public, non-draft, and shipped 21 installable
  assets each, both predating the comments commit.
- **"This decision adds no public surface"** — a sentence in the spine, falsified by the implementation:
  the storage crate had to gain a public crate-root version constant, because the engine could not
  otherwise compose its doctor finding. A crate-plan premise about that constant being unreachable from a
  test died with it. Both were corrected in the fix round rather than argued away.
- **"The compile-time assertion prints the digest you need."** It prints nothing. A failing `const`
  assertion hands you no value, so a document telling a developer to transcribe it from the build output
  is telling them to do something impossible; the means of obtaining the new digest had to be named.
- **A derived count that had rotted FIVE times.** The decision-range bump-site count. The round meant to
  repair it wrote a number matching neither reading of its own enumeration. Measured mechanically in this
  run, twice, independently: SIX files carry the live range — `CLAUDE.md:17`,
  `docs/plans/ci-cd-and-distribution.md:65`, `xtask/src/doc_lint.rs:455`, and the range knob of each of
  `scripts/checks/d44-create-deps-claims.sh:78`, `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh:112`
  and `scripts/checks/d46-schema-migration-claims.sh:65`. Ruling sixteen retires the numeral: the prose
  ENUMERATES the files and carries no number, and the guard script asserts that every enumerated file
  carries the live range, so the list cannot be off-by-one against itself.
- **"Nothing was left unbuilt."** The implementation reported so; the guard script the decision row
  MANDATES did not exist. It now does, with its own workflow step, and was proven to fail by removing a
  landing and observing red.

Defects that only a specific KIND of looking would find:

- **Following the advice, instead of reading it.** The lying-stamp sentinel emitted "at schema version 2,
  but this build expects 2" and advised running `unblock migrate`. The ladder is skipped at that stamp,
  so the advised command re-emits the identical error forever. In a task whose subject is the product
  telling the truth about its own state, the first fix shipped a message that lied.
- **Comparing both sides of the assertion.** The cell meant to pin the two new advisory doctor findings
  compared two values that were both the current version, so it could not tell whether either finding
  read what it claimed to read. The real pin re-stamps a migrated store back to the baseline and asserts
  one finding reports the baseline while the other reports the current version
  (`crates/unblock-engine/tests/lifecycle.rs:230`); both directions were mutation-proven.
- **Sweeping the tree instead of reading it.** Four rounds found moving pins by reading the documents.
  The fifth SWEPT for shipped cells that go red under the change and found six more that no document
  named — including one whose expected value is not a literal at all but the version constant itself (so
  it moves whether or not anyone edits the cell) and a contract-version string inside a command-line
  snapshot that nobody would look for while thinking about schema.
- **Tracing a value end to end.** The hint composed in the storage layer was DISCARDED at the config
  boundary, whose coded-error implementation forwarded only the code — while the published capabilities
  document would have advertised a hint shape paid for with a contract bump. Advertise-and-discard,
  caught by following the value from composition to the command-line payload.
- **Reading the order of operations, not the code's intent.** `unblock migrate` read the schema version
  AFTER the facade had already migrated. Before the fix that yielded "1 to 1, nothing applied"; after the
  fix as first specified it would have yielded "2 to 2, nothing applied" — on a database it had just
  repaired. A second false green living inside the fix for the first one.
- **Checking what a new report row displaces.** The two new advisory doctor rows were specified AFTER the
  dangling-dependency block; a shipped test asserts that block is the trailing SUFFIX and its docstring
  names the mutant that assertion exists to kill (`crates/unblock-engine/tests/dangling.rs:337-341`).
  Ruled: the new rows go BEFORE, so the guard survives byte-identical.

Behaviour worth remembering about the broken state itself:

- **A "failed" write may have COMMITTED.** On a stale database, `issue create`, `create_bulk`, `close`,
  `claim` and `defer` commit and then report failure, because the engine re-reads and hydrates after the
  transaction. An agent retrying a "failed" create manufactures duplicates. Demonstrated live, not
  reasoned.
- **Both health surfaces reported green** on a database that could not serve a single read: `migrate` said
  applied=false exit 0 and `doctor` said healthy exit 0. The integrity check is a page-level SQLite check
  that by construction cannot see an application-schema mismatch.

Process gotchas:

- The two-iteration escalation rule was hit and passed: four failed design-gate rounds. Miguel authorized
  a deliberately SCOPED fifth round (two lenses, plus the tree sweep that had never been done
  systematically) rather than a sixth full one.
- The fifth round's single remaining must-fix was hand-written by the orchestrator after verifying all six
  claims directly in the code, rather than spending another workflow on one clause. Recorded honestly in
  the issue thread: no gate re-ran after that edit, so the design gate's last recorded verdict is FAIL
  with its final must-fix closed and verified — not a PASS.

## Glossary

The run's issue comments coined no session-local identifiers; the codes below are the durable, in-file row
codes this report cites, listed so the report resolves without the session.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| clause (10) | The D46 sub-decision that makes `unblock migrate` report the stamp observed BEFORE the facade's own migration | `docs/PRD.md` §4, the D46 row; consumed at `crates/unblock-cli/src/commands/migrate.rs:39` |
| P-row | A required-landing row of a `scripts/checks/` guard script: a presence predicate over a named file | `scripts/checks/d46-schema-migration-claims.sh:96` (the `REQUIRE` table) |
| Q-row | A row-anchored landing of the same script: at least one line must match the anchor, and every anchored line must match the requirement | `scripts/checks/d46-schema-migration-claims.sh:117` (the `REQUIRE_ROW` table) |
| range knob | The shell variable in each shipped check script holding the LIVE decision-id range, in its two spellings | `scripts/checks/d46-schema-migration-claims.sh:65-66` and the two sibling scripts |
| ruling sixteen | Miguel's standing ruling that the bump-site count stops being a numeral in prose and becomes an enumeration a script checks | `docs/PROCESS.md` §3; issue `ub-lp9.13`, the Verify-gate comment of 2026-08-03 |
| ruling seventeen | Miguel's standing ruling that the run report and the tracker re-export belong to the tracking step | `docs/PROCESS.md` §6; issue `ub-lp9.13`, the same comment |

## Links

- `ub-lp9.13` — the tracker issue; its seven comments are the authoritative per-phase narrative (Understand
  map, Decide rulings, four gate verdicts, Verify outcome) and this report is the depth behind them.
- Decision: `docs/PRD.md` §4, the D46 row (SUPERSEDES the D37 clause that said no migration was needed).
- Interface: `docs/plans/01-design-spine.md` §3.2 (`Storage::migrate`) and §5.4 (the contract ledger).
- Key files: `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/schema.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-storage/src/libsql/migrations.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-config/src/context.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/crates/unblock-cli/src/commands/migrate.rs`,
  `/Users/ramosmig/Public/WS-Labs/unblock/scripts/checks/d46-schema-migration-claims.sh`.
- Prior related run-reports: [2026-08-01-dangling-blocker-spec](2026-08-01-dangling-blocker-spec.md) and
  [2026-08-01-dangling-blocker-impl](2026-08-01-dangling-blocker-impl.md) — the sibling decision in the
  same v1.0.1 slot, whose guard script this one is modelled on.
