---
name: 2026-09-08-knowledge-layer-invariants
description: Wiring the knowledge layer's landing-verification script into CI as the invariants check (tracker ub-wvh) — a checker red since the day it landed and run by no job, split on Miguel's ruling into two standing parts kept and two landing-era parts deleted, three design-gate rounds and two Verify rounds that each found prose claiming more than its row grades, and a fixture-repo selftest built because the repo's own guard forbids mutating the live knowledge tree.
type: run
date: 2026-09-08
branch: ub-wvh-knowledge-layer-invariants
pr: '-'
issues: [ub-wvh]
---

# Run — the knowledge-layer invariants check

## Context

Task `ub-wvh` — `scripts/knowledge/tests/landing-verify.sh`, written when the knowledge layer
landed, had exited non-zero on pristine `main` ever since and was invoked by no CI job, so its red
was never observed by anyone. The `ub-gwe` Verify gate measured it on 2026-08-07 in two separate
private checkouts, found it identical with and without that change, and scoped it out as its own
task. Branch `ub-wvh-knowledge-layer-invariants`, taken off `main` at b562570. No pull request is
open at the time of writing.

The lifecycle ran across three sessions, from 2026-08-07 to 2026-09-08. The orchestrator ran
Understand and Decide with Miguel. Spec/Plan ran as three facets — landing sites, executable design,
process and scope — consolidated by a coordinator into one implementer-ready specification. The
design Review gate ran three adversarial rounds, each with the same three lenses (executable
correctness, document drift, process and scope) plus a coordinator, followed by a single confirmation
agent that Miguel authorised after round 3. Implement ran twice, each time as one `rust-engineer` in
an isolated worktree. The Verify gate ran two rounds with `code-reviewer`, `qa-expert` and
`rust-engineer`, each lens in its own worktree, plus a coordinator, followed by a confirmation agent
that Miguel authorised after round 2. Track is this report, the tracker re-export, and the commit
that carries them.

## What & why

### The defect

The script had four parts and two of them had rotted. A token allowlist asserted that the tokens
`knowledge-lint`, `run-report-gate`, `memory-retire` and `.knowledge` occur only inside a hard-coded
per-token file list, and later legitimate work added files that were absent from those lists —
`.github/workflows/knowledge-gardener-reminder.yml`,
`scripts/checks/d44-create-deps-claims.sh` and `scripts/checks/ub-lp9.25-dangling-blocker-claims.sh`
— so the script reported failures about correct files. A two-line proximity window asserted that
every occurrence of the phrase "same commit" in four documents sits within two lines of the word
"run-report", and the sentences that tripped it state the decision-range cascade, the tracker
re-export rule and the schema-sequencing split. Those sentences are correct prose about different
same-commit rules, so no allowlist edit reaches them.

The other two parts were green, and most of their rows have no other cover in CI. Nothing else in CI
asserts the hook scripts and `scripts/knowledge/memory-retire.sh`, which no job executes, the
top-level hooks object of `.claude/settings.json`, both templates, the lint's corpus test, the
same-commit sentences, the zero-hits sweeps, or the rows pinning the check's own wiring in the
specification and in `docs/PROCESS.md`. Some rows do have another cover. Knowledge-lint's structure
guard asserts both index files and both wiki directories, and Miguel kept those rows anyway as
deliberate redundant coverage, which is why subsection 2.3.6 sorts every row into a unique-coverage
bucket and an alternative-coverage one.

One fact sharpened the Understand map beyond the issue text. The script's own header cited a
specification section as its spec, and that section enumerated its enforcement layers without ever
naming it.

### The split

Miguel resolved the fork with neither option the issue framed. The two landing-era parts are deleted
at their root, and subsection 2.3.6 bars re-adding either, because an allowlist is maintained state
that goes red the moment later legitimate work mentions its tokens, and a line window cannot tell the
run-report rule from the other same-commit rules that legitimately use the same phrase in the same
documents. The two standing parts are kept, the file is renamed to
`scripts/knowledge/knowledge-layer-invariants.sh`, specified at
`docs/plans/ci-cd-and-distribution.md` section 2.3.6 as the layer that section was missing, and wired
as a step of the required `doc-lint` job. A fixture-repo selftest ships beside it as its executable
proof.

### Sections read

The change was written against these sections, read rather than restated here.

- `docs/plans/ci-cd-and-distribution.md` section 2.3 (the knowledge-layer intro and its layer list),
  section 2.3.3 (the run-report gate, whose selftest is the precedent for a required step carrying no
  decision-range knob), section 2.3.5 (accepted residuals) and the new section 2.3.6.
- `docs/PROCESS.md` section 3 (the decision-range knob rule and the script list that carries one),
  section 5 (gate iteration and escalation), section 6 (the Track step's ownership of the tracker
  re-export and this report) and section 8 (the knowledge layer).
- The `CLAUDE.md` hard rules, for the graph-before-grep boundary the Understand map had to answer to.
- `docs/plans/templates/run-report.md`, the shape of this page.

### The style ruling

`docs/STYLE.md` was added to the shared tree, uncommitted, after the first implementation commit, and
Miguel ruled it binding on this change rather than on later work alone. The repair round therefore
rewrote every sentence this change writes — the check, the selftest, both `ci.yml` step comments, the
specification texts, the `docs/PROCESS.md` section 8 bullet and the commit message — while every
factual constraint stood unchanged. The specification's verbatim texts were rewritten first, then
the tree was reconciled against them and machine-compared whitespace-folded.

## Outcome

### Design Review

- Round 1 (2026-08-07) failed with eleven must-fixes. Two would have shipped red. The draft mandated
  a sentence whose framing one of the required check scripts forbids, which would have turned that
  job red on the very commit that wires the new step, and it justified the new layer with a claim the
  existing run-report-gate selftest already refutes. The third finding reshaped the work, because the
  two rows asserting the wiki directories exist cannot be proven by mutating the live tree at all.
  Miguel answered that with the fixture-repo selftest sibling, following the house pattern the
  run-report-gate selftest already set.
- Round 2 (2026-09-07) failed with seven surviving must-fixes, every one of them a predicate or a
  sentence. No lens contested the architecture. Under `docs/PROCESS.md` section 5 this was the second
  failed iteration, so it escalated to Miguel, who authorised one targeted repair and one further
  round.
- Round 3 (2026-09-07) failed with four must-fixes, three of them defects in the acceptance-probe
  table that never ships. Six of the seven round-2 items had landed. Miguel then ruled that the
  orchestrator apply the four directly and that a single confirmation agent verify them, rather than
  spending a fourth full round on new instances of the same probe-table classes.
- The confirmation pass (2026-09-08) returned PASS with no new must-fix, and the design Review gate
  closed. It rebuilt the whole change in an isolated worktree, machine-compared every shipped text
  against the specification, and ran the check, the selftest, the mutants, the six `scripts/checks`
  gates, `cargo xtask doc-lint`, `cargo xtask knowledge-lint`, the run-report-gate selftest and the
  xtask tests.

### Implement

The implementer produced one commit, 200bbe5, starting from the confirmation rebuild. Verify round 1
sent it back, and the repair produced 2645237, still one commit. The orchestrator applied the round-2
sentence repairs directly and amended them in, which is the gated commit 2b72f25.

### Verify

- Round 1 (2026-09-08, at 200bbe5) failed with one must-fix of one line. Subsection 2.3.6 said the
  check finds audit markers of any grammar in the run-report gate script, the memory-retire script,
  `scripts/hooks/` and the CI workflow, while the row it describes catches the letter `R` only inside
  square brackets. The sentence was corrected to what the row grades, and the regex was left alone.
- Round 2 (2026-09-08, at 2645237) failed with one must-fix, one row above the first in the same
  table. The cell said the marker row over landed normative documents finds nothing of the
  square-bracket audit-marker grammar, while that row matches three of the six letters the repository
  single-sources at `scripts/knowledge/run-report-gate.sh:11`. The round-1 repair had made the
  silence worse by enumerating letters in the sibling cell. Under `docs/PROCESS.md` section 5 this was
  the second Verify iteration without a pass, so it escalated, and Miguel ruled for the sentence
  repair over widening the regex.
- The confirmation pass (2026-09-08, at 2b72f25) returned PASS with no new must-fix, and the Verify
  gate closed. It re-executed everything, compared the specification against the tree, and proved the
  marker rows by planting markers in `docs/PROCESS.md` and observing which ones turn the check red.

### What landed

The gated commit 2b72f25 adds 767 lines and removes 124 across these files.

- `.github/workflows/ci.yml` gains the check's `doc-lint` step and the selftest's, each with the
  comment the specification fixes verbatim.
- `docs/PROCESS.md` section 8 becomes a count-free layer list that names the check.
- `docs/plans/ci-cd-and-distribution.md` gains the count-free section 2.3 intro, the section 2
  job-table row naming both scripts, the narrowed section 2.3.5 residual clauses plus a new named
  residual for the case where both CI steps are removed together, a repair of a pre-existing drift at
  line 360, and subsection 2.3.6.
- `scripts/knowledge/knowledge-layer-invariants.sh` (147 lines, 0755) is new and carries the parts
  the split kept.
- `scripts/knowledge/tests/knowledge-layer-invariants-selftest.sh` (403 lines, 0755) is new, with at
  least one negative fixture case per row.
- `scripts/knowledge/tests/landing-verify.sh` (107 lines) is deleted.

### Advisories left open by design

The closing pass recorded these for a later sweep rather than repairing them here.

- The `ci.yml` step comment and the check's in-body banner over its zero rows say "audit markers"
  without naming a letter set, because each summarises two rows whose sets differ. The check's header
  paragraph names both sets.
- The check's header banner says "non-fixture scripts" where that row also scans `ci.yml`, which
  under-claims its own scope.
- The templates row's failure message speaks of both template halves over three tracked files, and it
  is byte-identical to `main`.
- The bare marker alternative in the scripts row has no left boundary, which nothing in the four
  scanned scopes trips today.
- The zero-live-hits helper reads a `git grep` error as no hits, inherited unchanged from `main`.
- The decoy lines inside the fixtures are themselves ungraded, so a future tidy-up could disarm the
  two hardest rows.
- The selftest runs about a minute, which makes it the largest step of the `doc-lint` job.

## Gotchas

- The round-1 specification draft and six of its eleven must-fixes were lost when that session's
  scratchpad and transcript were purged, and the three issue comments were the whole surviving record
  a month later. The `ub-wvh` comment of 2026-09-07 rebuilt Spec/Plan from them and recorded the
  lesson that loss taught, which is that a gate comment carrying a must-fix count without the
  must-fixes is not a durable record. Every later round of this task wrote each surviving must-fix
  into its own gate comment.
- The PreToolUse bash guard denies any shell command that pairs a mutating verb or an interpreter
  with the literal knowledge-tree path, so the rows asserting the wiki directories exist could not be
  proven against the live tree, and every shipped text carrying that literal had to land through the
  editor tools (design Review round 1 and the round-3 advisories).
- The style rewrite briefly split the phrase a positive acceptance probe keys on across two lines of
  the section 2.3 intro, and no negative sweep could ever have caught that (Implement repair,
  2026-09-08).
- A one-line fixture stub turned the mandated mutation idiom into a silent no-op, because `grep -v`
  selects nothing on it, exits non-zero, and the `&&` then skips the move, so the case passed where a
  failure was expected (design Review round 2).
- `git grep -w` matches a word inside a pipe-delimited pattern, so the rename zero-live-hits probe hit
  the retired name inside the check's own regex and read red against a correct implementation (design
  Review round 3).
- `git grep`'s default engine does not honour a word-boundary escape, so a probe or a row hardened
  with one would pass vacuously (design Review round 3 advisory).
- An unguarded `R-[0-9]+` matches the `R-12` inside the NFR-12-style tokens of
  `.github/workflows/ci.yml`, so widening the marker row to catch a bare `R` marker would have
  reddened the check on its own workflow (Verify round 1).
- The selftest takes about a minute on three machines, which makes it the largest `doc-lint` step and
  is described without a number in the specification (Verify round 1, restated at the close).
- The rename does not survive git's default rename detection, so the commit shows an add and a
  delete, and no shipped text claims otherwise (Verify round 1, recorded and not repaired).
- One lens measured a line-number citation on its own edited tree rather than on `main`, and the
  coordinator rejected the claim for that reason (design Review round 2).

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| MF-9 | a fabricated audit marker planted in `docs/PROCESS.md` as the positive control of the closing marker proof, and the check turns red on it | `ub-wvh` comment of 2026-09-08 closing the Verify gate; the row it exercises is `scripts/knowledge/knowledge-layer-invariants.sh:85` |
| CF-1 | a fabricated audit marker planted in the same proof that the row leaves green by design, because that row matches bracketed R, MF and A only | same comment, same row |
| M10 | a second such marker in that proof, green for the same reason | same comment, same row |
| F-2 | a third such marker in that proof, green for the same reason | same comment, same row |
| R-7 | a fabricated audit marker appended to a hook script in the Verify round-1 proof; bracketed it turns the check red, while parenthesised or bare it passes | `ub-wvh` comment of 2026-09-08 recording Verify round 1; the row it exercises is `scripts/knowledge/knowledge-layer-invariants.sh:87` |
| R-12 | the fragment inside the NFR-12-style tokens of `.github/workflows/ci.yml` that an unguarded `R-[0-9]+` would match, which is why that row stayed narrow | `ub-wvh` comments of 2026-09-08 recording Verify round 1 and the repair that followed it |

## Links

- `ub-wvh` — the knowledge layer's landing-verification script had been red since it landed and ran
  in no CI job; the task this run closes.
- `ub-gwe` — the code-graph-first task whose Verify gate found this defect and scoped it out.
- Pull request — none open at the time of writing.
- Specification — `docs/plans/ci-cd-and-distribution.md` section 2.3.6, the subsection this change
  adds.
- Key files touched — `scripts/knowledge/knowledge-layer-invariants.sh`,
  `scripts/knowledge/tests/knowledge-layer-invariants-selftest.sh`,
  `docs/plans/ci-cd-and-distribution.md`, `docs/PROCESS.md`, `.github/workflows/ci.yml`, and the
  removed `scripts/knowledge/tests/landing-verify.sh`.
- Prior related run-report — `runs/2026-07-23-knowledge-layer-landing.md`, the run that wrote the
  script this task splits and renames.
- Prior related run-report — `runs/2026-08-07-code-graph-first-move.md`, the run that raised
  `ub-wvh`.
