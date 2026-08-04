---
name: 2026-08-04-label-diff-base-hydration
description: Hydrating the label diff base inside the update transaction (tracker ub-lp9.27) — a before-set that was always empty, so a label removal succeeded while removing nothing and an already-present label add died on a primary key; the production fix then passed every later gate untouched while four adversarial rounds kept finding prose, tests and docs claiming more than was true.
type: run
date: 2026-08-04
branch: ub-lp9.27-label-diff-base (on top of ub-lp9.27-label-hydration-r4)
pr: -
issues: [ub-lp9.27]
---

# Run — the label diff base (ub-lp9.27)

## Context

Tracker issue `ub-lp9.27` — "label removal is a silent no-op and label add/set hard-error on any existing
label; the update path never hydrates labels" — a P1 dogfood finding taken in the v1.0.1 maintenance slot
(`docs/plans/00-roadmap.md`, the v1.0.1 section). One data-integrity defect in `unblock-storage` (layer 2,
the only crate that gained code). The lifecycle ran across 2026-08-03/04 as per-phase Workflows
(`docs/PROCESS.md` §4): three implement rounds and four adversarial gate rounds, with Miguel ruling three
forks along the way.

Branches: the eleven accepted commits sit on `ub-lp9.27-label-hydration-r4`, based on `main` at `4ab02ab`
(`release: v1.0.1-rc.3`); this report and the final gate's sweep-up edits sit on
`ub-lp9.27-label-diff-base`, branched from that head. No pull request existed when this report was
written — the tracking step (a single writer in an isolated worktree) produced the sweep-up commits and
this report, and the pull request follows.

## What & why

`update_issue` built the `Issue` it patches from `issue_from_row`, which projects the `issues` row ALONE.
Nothing hydrated the label relation before `apply_labels` diffed, so the before-set was permanently EMPTY
on every patch. Four observable consequences, all reachable over the shipped tool surface:

- `labels_remove` of a present label removed nothing. Against an empty base the diff came out net-zero, so
  in a label-only patch the empty-diff full skip swallowed the WHOLE patch and returned success; in a mixed
  patch the row update still landed and only the label op vanished. This is the SILENT half.
- `labels_add` / `labels_set` naming an already-present label diffed it as new, so the reconcile re-inserted
  a row that already existed, died on the `labels` `(issue_id, label)` primary key and rolled its whole
  transaction back. This is the LOUD half — it landed nothing at all, not even its row columns, and no
  post-transaction re-read ever ran on it.
- The removal loop iterates before-minus-current, which over an empty before-set is itself always empty, so
  it was UNREACHABLE: `labels_set` was purely ADDITIVE and clearing the set was a no-op.
- A real label change never stamped `updated_at`. The skip guard already carried the label term; the
  stamping condition beside it did not.

The repair is one in-transaction `SELECT label FROM labels WHERE issue_id = ?1 ORDER BY label ASC` (the
read-path `hydrate` shape) placed AFTER the tombstone guard so the reject path pays for no wasted query,
plus the label term added to the stamping condition. Labels only, and for a narrower reason than "the only
relation in this transaction": `apply_reparent` also diffs in this transaction but re-reads its own base
from it, so labels are the only relation whose diff base came from the in-memory `Issue`.

Authoritative material read before writing anything: `docs/plans/01-design-spine.md` §3.2.1
(`update_issue` — the empty-diff full skip, the `updated_at` rule and now the normative label diff base),
§1.8 (the content hash excludes relations AND all timestamps, and every backend RECOMPUTES it on load
rather than trusting the stored column), §3.2 (the `Storage` trait declarations);
`docs/plans/crates/unblock-storage.md` (the `crud.rs` row); `docs/PROCESS.md` §3 (new decision id versus
inline amendment), §6 (tracking, commits, who owns the re-export) and §8 (the knowledge layer).

Miguel ruled three forks:

- **A real label add/remove/set advances `updated_at`**, recorded as an INLINE amendment on the spine's
  existing `update_issue` clause — a consequence of the SAME decision, so no new decision id and no
  decision-range cascade. It makes the reparent (FR-1b) the FIRST of exactly two normative relation
  exceptions and the label change the second.
- **The reconcile INSERT stays STRICT** — no insert-or-ignore. With a correct diff base a duplicate insert
  is unreachable, so the uniqueness constraint is kept deliberately as a loud tripwire that fires if the
  diff base ever regresses.
- **Split the event-ordering question** (see the first gate below): loosen the backend-independent
  conformance suite to a SET comparison, and pin libsql's own order as a backend fact in the behaviour
  suite.

## Outcome

Eleven commits landed on `ub-lp9.27-label-hydration-r4`: the fix plus its four originally-specified
contract cells (`1ee7648`), the spine and crate-plan amendment (`63e9cb9`), and then, round by round, the
event-order split (`61b3376`), the per-issue diff-base cell (`da41928`), the removal of a self-comparing
assertion (`3588a17`), a prose pass over the shipped comments (`01971f8`), the reconcile-DELETE scope step
(`1df8fc1`), the empty-set clear cell (`b9fbda9`), the release-note repair (`1d27a5e`), and two narrowing
passes over the claims (`6686bfa`, `7b0bd85`).

Gate history: the first gate returned two blockers; the second returned one false sentence in the release
note; the third narrowed several claims to what is actually graded; the fourth (final) returned three
must-fixes plus five advisories, all prose, which land in this tracking branch as five commits grouped by
document:

- the roadmap's masking sentence, which said the post-transaction re-read masked BOTH shapes — false and
  self-contradicting, since the same bullet correctly calls the already-present add an opaque error. The
  masking is now scoped to the removal shapes only.
- the same defect, condensed, in the crate plan's `crud.rs` row. (The code comment it was condensed from
  keeps the disambiguator "The remove is the SILENT one" and was already correct.)
- a rotted derived count in `crates/unblock-storage/src/testkit.rs` — "the four `contract_label_*` cases"
  when six label cells are registered, and the literal glob also matches an unrelated query case. Dropped
  in favour of naming the block, which is what the spine text on this very branch demands.
- four spine repairs: a qualifier on the pre-fix skip claim (it is true of a patch whose COMPUTED diff came
  out non-equal, not of one that moved the set in truth); a rewrap of a dangling reflow artifact; half a
  clause saying WHY the relation exceptions are counted while the consequence list is not (a CLOSED
  normative set whose count forbids a third, versus an OPEN list that grows); and the write-scope parity
  clause, since the contract suite hard-gates the reconcile DELETE's per-issue scope while the paragraph
  stated only the read base.
- the implementor-facing `Storage::update_issue` doc, which described `labels_set` as replacing an
  overlapping set but omitted the DROP of unlisted labels and the empty-set clear — both normative in the
  spine and both hard-gated.

Verification actually run on the tracking branch, all green: `cargo xtask doc-lint` (19 docs, 6 classes),
`cargo xtask knowledge-lint` (59 pages, 6 checks), all four `scripts/checks/*.sh`,
`scripts/knowledge/tests/run-report-gate-selftest.sh` (38 cases), `cargo fmt --all --check`,
`cargo clippy -p unblock-storage --all-targets --features testkit -- -D warnings`, and
`cargo test -p unblock-storage --features testkit` (13 targets, 0 failures).

Known pending at the time of writing: the `.unblock/issues.jsonl` re-export. It could not be produced
here — see the Gotchas.

## Gotchas

- **The production code was right after round one and never moved again.** Every defect the three later
  gate rounds found was PROSE — in test comments, in doc comments, in the crate plan, in the spine, in the
  release note — claiming more than was true. A gate that only re-reads the diff's code would have passed
  this change three rounds early.
- **A conformance suite must not grade what its own contract declines to promise.** Two contract cells
  asserted the exact ordered sequence removal-then-addition within one patch, while the spine amendment
  shipping beside them says relative order AMONG the label events of one patch is not guaranteed (the diff
  is a set). A backend reconciling additions first would have failed conformance while fully honouring the
  contract. The suite now compares label events as a set; libsql's own order is pinned as a backend fact in
  `crates/unblock-storage/tests/behaviour.rs`.
- **A positive-only corpus cannot grade a query predicate.** The seed's `WHERE issue_id = ?1` was pinned by
  nothing: every existing label case held exactly ONE issue in the database when its patch ran, so widening
  the predicate to match all rows left the entire suite green on both backends. Grading it needs a fixture
  with a SECOND labelled issue present before the label ops run.
- **And the read fixture still could not grade the WRITE.** Both read directions of that new cell diff to a
  net-zero or to an addition, so neither ever executes a `DELETE`. Only removing a label the two issues
  SHARE makes a real `DELETE` run while another issue holds the same label — the one shape in which an
  unscoped `DELETE` is observable.
- **An assertion can compare a value to itself.** A cell asserted that a loaded issue's `content_hash`
  equals `compute_content_hash()` of that same loaded issue. The loader recomputes the hash from the loaded
  fields and never reads the stored column (spine §1.8), so the assertion is a tautology and a mutant
  writing garbage into the column left the suite green. It was deleted rather than "fixed": no
  backend-independent test can observe that column, because the contract forbids trusting it.
- **A derived count rotted twenty lines from where the same branch removed the identical defect** — and in
  the higher-authority document, where no lint reaches. The branch wrote "the four `contract_label_*`
  cases" in the testkit, then grew the list to six, while the spine text it authored says these
  consequences are "stated as a LIST and never as a count of it, because the list GROWS". Without the
  final gate the pull request would have shipped the rule and its violation in one diff.
- **Hand edits leave dangling short lines that read as truncated sentences.** Three of them survived into
  the gated branch (one in the spine's `updated_at` sentence, one before the tombstone-guard clause, one in
  the roadmap bullet). Rewrapping the affected paragraphs is safe only if the reflow is generated
  mechanically and then verified with `git diff --word-diff`, which is what was done here.
- **The tracker re-export could not run from the isolated worktree.** The workspace database is gitignored,
  so it exists only in the session's shared checkout, and this worktree has no unblock tool access; a
  hand-written `.unblock/issues.jsonl` would not be byte-identical to an export, so none was written. The
  export therefore lands with the orchestrator. Two consequences worth knowing: the same-commit rule pairs
  the export with THIS report, so they land in one pull request rather than one commit; and the glossary
  lint's token check scans the linked issue's COMMENTS, which this worktree cannot see — so the export
  commit must re-run `cargo xtask knowledge-lint` and add a glossary row for any uppercase session-local
  code the comment thread carries.

## Glossary

No session-local ids were coined by this run's report. The rows below are the durable in-file names a cold
reader needs to act on it; the tracker comment thread was not readable from this worktree (see the last
Gotcha), so any code it carries must be reconciled into this table by the commit that re-exports it.

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| the seed | The one in-transaction `SELECT` that fills the label diff base before the ops diff | `crates/unblock-storage/src/libsql/crud.rs:797-811` |
| the skip guard | The three-term empty-diff condition (no staged column AND no label change AND no reparent) that skips the whole update | `crates/unblock-storage/src/libsql/crud.rs:966` |
| the stamping condition | The sibling condition deciding whether `updated_at` is advanced; the label term was missing from it | `docs/plans/01-design-spine.md` §3.2.1, `update_issue` |
| the label block | The `contract_label_*` cases registered together in the storage contract suite (six at the time of writing; named, never counted) | `crates/unblock-storage/src/testkit.rs:155-160` |
| the backend fact | libsql's own reconcile order (removals before additions), pinned outside the backend-independent suite | `crates/unblock-storage/tests/behaviour.rs`, `labels_reconcile_removals_before_additions` |

## Links

- `ub-lp9.27` — the tracker issue: the update path never hydrated labels before diffing label ops. Its
  comment thread is the authoritative per-phase narrative; this report is the depth behind it.
- Interface: `docs/plans/01-design-spine.md` §3.2.1 (`update_issue`, the `updated_at` rule and the
  normative "Label diff base" paragraph) and §1.8 (the content hash).
- Plan: `docs/plans/crates/unblock-storage.md`, the `src/libsql/crud.rs` row.
- Release note: `docs/plans/00-roadmap.md`, the v1.0.1 maintenance slot.
- Key files: `crates/unblock-storage/src/libsql/crud.rs`, `crates/unblock-storage/src/testkit.rs`,
  `crates/unblock-storage/src/trait_def.rs`, `crates/unblock-storage/tests/behaviour.rs`.
- Prior related run-report: [2026-08-03-comments-forward-migration](2026-08-03-comments-forward-migration.md)
  — the sibling defect in the same v1.0.1 slot, and the run whose seventeenth ruling assigned this report
  and the re-export to the tracking step.
