---
name: 2026-07-23-knowledge-layer-landing
description: The knowledge layer lands self-hosting — the spec-gate saga (two rounds, an authorized third iteration) and the one-PR landing of scaffold, lint, gate, hooks and this first report.
type: run
date: 2026-07-23
branch: knowledge-layer-landing
pr: -
issues: [ub-knowledge-layer-e4s, ub-knowledge-layer-e4s.1, ub-knowledge-layer-e4s.2, ub-knowledge-layer-e4s.4]
---

# Run — the knowledge layer lands (spec saga + self-hosting landing)

## Context

Epic ub-knowledge-layer-e4s (the repo-public `.knowledge/` knowledge layer: memories + wiki + hard
day-1 enforcement). This run is the Implement phase of tasks ub-knowledge-layer-e4s.2 (scaffold +
templates + doc changes) and ub-knowledge-layer-e4s.4 (enforcement: knowledge-lint, the run-report
gate, hooks), executed as ONE self-hosting landing set after the design gate on
ub-knowledge-layer-e4s.1 (the spec package) closed. Branch `knowledge-layer-landing` off `main`; PR
pending at report time. Team shape: a single rust-engineer implementer in an isolated git worktree,
working from the LOCKED spec package; the spec itself was produced by a Spec/Plan team and survived a
three-iteration adversarial design Review (four lenses round 1, three lenses round 2, two delta
reviewers in iteration 3, coordinator throughout).

## What & why

The trigger (epic decide record, 2026-07-21): Miguel flagged a v1.0.1 Verify-gate report as cryptic —
it leaned on session-local codes (a mutant id M10, a must-fix id MF-2, briefing rules R7 and R8, lens
indices, facet letters) that resolve nowhere outside their originating session; a grep over the
tracker export confirmed zero durable hits. The fix is a knowledge layer: durable, descriptive,
machine-enforced from day 1 — plus a clarity rule so prose to Miguel is self-contained. This run
implemented the locked spec: the `.knowledge/` scaffold and seed indexes, the two page templates, the
process/contract doc patches (PROCESS section 7 clarity rule + new section 8; the CLAUDE.md doc-map
row and same-commit clause; the ci-cd section 2.3 normative spec; the drift-gap template fixes; the
STATUS stub clause; the roadmap docs-in-DB row), `cargo xtask knowledge-lint` (checks k1 through k6)
with its fixtures and corpus tests, the shared gate predicate script + its 38-case selftest harness +
the PR-only CI job, the three PreToolUse hooks + the sanctioned memory-retire script, the executable
landing-verify script, the tracker-export handoff, and this report. Authoritative sources read in
full: the locked spec package (all 10 sections + landing map), the round-1/round-2/iteration-3
verdicts, the revision log, and the live repo files each patch targets.

## Outcome

The spec saga this landing closes out: (1) three parallel drafts (process-patch, scaffold, enforcement)
were merged into one spec package with every contradiction resolved inline; (2) design Review round 1
returned PASS_WITH_MUST_FIXES — 18 must-fixes (MF-1..MF-18) + 7 advisories (A-1..A-7), with two
softening fix-arms explicitly rejected; (3) the revision landed all 25 of 25; (4) round 2 returned
FAIL with 3 blockers — B1 (the gate-selftest matrix covered 7 of 14 always-substantive globs and
pinned no arm order), B2 (the glossary comment-scan had no temporal scope, so a later comment would
retroactively redden a frozen report), B3 (the presence-only arm's true cost was under-named) — and
escalated to Miguel per the two-iteration rule; (5) Miguel resolved the forks — glossary depth = the
hard token-coverage arm WITH temporal scoping (comments dated at or before the report's date), the
roadmap docs-in-DB row = ADD in this landing PR — and authorized one iteration-3 pass covering B1, B2,
B3 and the 16 nit edits (N1..N16); (6) iteration 3 returned PASS, fixing three residual defects
in-pass (R3-V1..V3: a stale residual-row range, a retired-branch comment, a stale advisory label), and
the package was LOCKED. This landing then implemented it end to end. Verification on this branch (all
run locally, honestly reported): cargo fmt --check clean; clippy pedantic clean (deny warnings);
workspace test suite green including doc-lint corpus (now with the knowledge-separation pin) and the
new knowledge-lint unit + corpus tests; `cargo xtask doc-lint` green over the grown ci-cd corpus file;
`cargo xtask knowledge-lint` green over this very tree (self-hosting: the first inhabitant of the wiki
is this report, and the lint's comment-scan runs against the committed export); the gate selftest
passed all 38 cases + the single-sourcing pin; landing-verify reported `landing-verify OK: all checks
clean`. The branch-protection flip (required check + admin enforcement) is the sequenced POST-MERGE
manual step owned by Miguel — pending at report time, with the stated minutes-long interim window.

## Gotchas

- Git cannot represent an empty directory, but the spec requires `wiki/topics/` to exist while
  shipping empty — a fresh CI clone would lose the dir and trip the structure guard. Landed a
  `.knowledge/wiki/topics/.gitkeep` plus a narrow, documented k2 exemption for exactly that filename
  (xtask/src/knowledge_lint.rs, walk_knowledge). This is a flagged landing deviation awaiting
  ratification — the spec's k2 table has no placeholder row.
- A git worktree's `.git` is a FILE, not a directory; the bash-guard's repo-root probe checks both
  shapes so trigger B still resolves roots inside worktrees (scripts/hooks/knowledge-memories-bash-guard.py) —
  a one-line hardening beyond the spec's literal body, flagged for review.
- Hook wiring only executes inside a live Claude Code session; this implementer ran SCRIPT-LEVEL
  canaries instead (synthetic PreToolUse payloads on stdin: a Write overwrite of an existing memory,
  a Bash rm naming the knowledge tree, a gh pr create before the report existed, an acp-tool
  overwrite — each denied with exit 2). The four WIRING-LEVEL canaries in a live session are owed
  before the PR by the orchestrator, per the landing checklist.
- The epic's issue comments claim the spec package and verdicts "ride into the repo with the landing
  PR"; the landing map deliberately ships their CONTENT into real homes (ci-cd section 2.3, PROCESS
  section 8, templates, scripts) and no standalone spec file — the comment phrasing, not the map, is
  the loose end. Flagged rather than improvised around.
- The tracker export in this PR is a scratchpad handoff (the shared tree's MCP server holds the DB);
  concurrent sessions reconcile the export by hand until the shared-state release.

## Glossary

| id | what it is (in words) | where it lives (file:line / doc § / issue id) |
|----|-----------------------|-----------------------------------------------|
| M10 | the example mutant id from the cryptic Verify report that triggered this epic (a mutation-testing survivor label, resolvable only in its own session) | epic decide comment on ub-knowledge-layer-e4s; quoted in docs/PROCESS.md section 7 |
| MF-1 | round-1 must-fix: the bash-guard trigger missed parent-dir, pathless and split-context destructive shapes; broadened + sanctioned branch made reachable | round-1 verdict; landed in scripts/hooks/knowledge-memories-bash-guard.py |
| MF-2 | round-1 must-fix: the write-guard matcher missed the allowlisted acp Write/Edit tools; they are now matched and share Write/Edit verdicts | round-1 verdict; landed in .claude/settings.json + scripts/hooks/knowledge-memories-write-guard.py |
| MF-3 | round-1 must-fix: memory-retire accepted the slug "index" and could destroy the curated index mid-flow; now rejected, grammar re-validated, no-partial-destruction ordering | round-1 verdict; landed in scripts/knowledge/memory-retire.sh |
| MF-4 | round-1 must-fix: rename detection let a repo-doc-into-knowledge move ride the neutral strip; classification now runs on a no-renames listing | round-1 verdict; landed in scripts/knowledge/run-report-gate.sh |
| MF-5 | round-1 must-fix: the dependabot exemption keyed on the event actor and could deadlock a re-triggered required check; now keyed on the PR author login | round-1 verdict; landed in the run-report-gate job, .github/workflows/ci.yml |
| MF-6 | round-1 must-fix: "at least 1 glossary table row" was satisfiable by template header/placeholder lines; DATA row is now normatively defined and fixture-pinned | round-1 verdict; landed in ci-cd-and-distribution.md section 2.3.2 + xtask/src/knowledge_lint.rs |
| MF-7 | round-1 must-fix: the glossary duty covered issue comments while the lint read only the report; resolved into the comment-scan (hard arm, later made temporal) | round-1 verdict; landed as the k6 token-coverage rules |
| MF-8 | round-1 must-fix: tracker-export-only PRs (the biggest code coiners) escaped the gate; closed by the comment-coining export trigger, rule 1a | round-1 verdict; landed in scripts/knowledge/run-report-gate.sh |
| MF-9 | round-1 must-fix: the no-import check read only CLAUDE.md while imports nest; now scans the whole import closure | round-1 verdict; landed as the k4 closure check, xtask/src/knowledge_lint.rs |
| MF-10 | round-1 must-fix: the two out-of-tree point-reads were unspecified and a literal build would fail open; parse, path-resolution and fail-closed guard behavior fully specified | round-1 verdict; landed in ci-cd-and-distribution.md section 2.3.2 |
| MF-11 | round-1 must-fix: the issues-resolve check silently assumed exported ids persist forever; the export retention invariant is now a stated contract rider | round-1 verdict; landed in ci-cd-and-distribution.md section 2.3.2 |
| MF-12 | round-1 must-fix: the load-bearing gate script had zero specified tests plus an inverted pathspec claim; a fixture-repo selftest harness was mandated (its completeness became blocker B1) | round-1 verdict; landed as scripts/knowledge/tests/run-report-gate-selftest.sh |
| MF-13 | round-1 must-fix: the branch-protection flip was unsequenced server-side state; now an owned, verified post-merge manual step | round-1 verdict; landed in ci-cd-and-distribution.md section 2.3.3 |
| MF-14 | round-1 must-fix: the package's largest doc artifact had no landing text; the deterministic landing transform produced ci-cd section 2.3 | round-1 verdict; landed as ci-cd-and-distribution.md section 2.3 |
| MF-15 | round-1 must-fix: the same-commit rule is live in several always-in-context places and only one was being extended; every live site now carries the run-report clause, verified executably | round-1 verdict; landed across PROCESS/CLAUDE/STATUS + scripts/knowledge/tests/landing-verify.sh |
| MF-16 | round-1 must-fix: the drift-gap template carried a second stale STATUS.md reference on its Resolution line; both halves fixed | round-1 verdict; landed in docs/plans/templates/drift-gap-report.md |
| MF-17 | round-1 must-fix: the landing verification was self-falsifying prose greps; replaced by the executable allowlist-based script | round-1 verdict; landed as scripts/knowledge/tests/landing-verify.sh |
| MF-18 | round-1 must-fix: the docs-in-DB citation dangled on an open fork; wording made single-branch once the ADD resolution landed | round-1 verdict; landed in docs/plans/00-roadmap.md section 7 + PROCESS section 8 |
| A-1 | round-1 advisory: "a stub cannot pass the pair" was an overclaim; report-content quality is owned in exactly one residual | round-1 verdict; the residual record in ci-cd section 2.3.5 |
| A-2 | round-1 advisory: hook fail-closed holds only once a script runs; the environmental fail-open residual is named and smoke-canaried | round-1 verdict; the residual record in ci-cd section 2.3.5 |
| A-3 | round-1 advisory: the skeleton blocks' placeholder entry lines are illustrative — seed indexes ship with empty entry lists | round-1 verdict; stated in ci-cd section 2.3.1 |
| A-4 | round-1 advisory: the template description-direction was inverted; the index copies the page, not the reverse | round-1 verdict; landed in both templates |
| A-5 | round-1 advisory: "substantive" gained two live senses in PROCESS; the gate sense is disambiguated in section 8 | round-1 verdict; docs/PROCESS.md section 8 |
| A-6 | round-1 advisory: the PROCESS section-8 enforcement bullet was compressed to posture + pointer to avoid an unlinted duplicate | round-1 verdict; docs/PROCESS.md section 8 |
| A-7 | round-1 advisory: the landing map omitted the tracker registration and test files; rows added | round-1 verdict; the spec's landing map |
| R2 | shorthand for design-Review round 2 — the FAIL verdict (3 blockers, 11 nit groups) that triggered the escalation | the epic thread on ub-knowledge-layer-e4s.1 (round-2 comment) |
| R3 | shorthand for design-Review iteration 3 — the PASS verdict; also the prefix of its in-pass fix log entries | the epic thread on ub-knowledge-layer-e4s.1 (iteration-3 comment) |
| R7 | a briefing-rule id quoted from the cryptic report that triggered this epic (a numbered rule of that session's coordinator brief) | epic decide comment on ub-knowledge-layer-e4s |
| R8 | a briefing-rule id quoted from the same cryptic report; reused as the clarity rule's example | epic decide comment; docs/PROCESS.md section 7 |
| B1 | round-2 blocker: the selftest matrix's completeness claim was false (7 of 14 globs, no order pins, no binary arms); matrix completed in iteration 3 | round-2 comment on ub-knowledge-layer-e4s.1; the 38-case matrix in ci-cd section 2.3.3 |
| B2 | round-2 blocker: the comment-scan lacked a temporal scope (retroactive red on frozen reports); resolved as created_at at-or-before the report date, inclusive end-of-day UTC | round-2 comment; the k6 temporal rule in ci-cd section 2.3.2 |
| B3 | round-2 blocker: the presence-only arm's cost was under-named (body-token coverage also unverified); honesty reword landed | round-2 comment; the road-not-taken record in ci-cd section 2.3.5 |
| N1..N16 | the round-2 verdict's sixteen one-line nit edits (landed as seventeen atomic edits, one split disclosed) | round-2 comment on ub-knowledge-layer-e4s.1 |
| Q1 | open question 1 — add the roadmap docs-in-DB row now or defer; Miguel resolved ADD, the row landed in this PR | iteration-3 comment; docs/plans/00-roadmap.md section 7 |
| Q2 | open question 2 — a cross-class micro exemption for tiny diffs; stays open with the unanimous keep-the-baseline recommendation | the locked spec's open-questions record |
| Q3 | open question 3 — glossary enforcement depth; Miguel resolved the hard token-coverage arm with temporal scoping | iteration-3 comment; the k6 rules in ci-cd section 2.3.2 |
| Q4 | open question 4 — a stricter Miguel-only retire flow and freezing merged run files; stays open, baseline kept | the locked spec's open-questions record |
| Q5 | open question 5 — forward-compat visibility/updated schema fields; stays open, deferred by recommendation | the locked spec's open-questions record |
| R3-V1..V3 | the three iteration-3 in-pass fixes: a stale residual-row range, a retired-branch comment in landing-verify, a stale advisory label | iteration-3 comment on ub-knowledge-layer-e4s.1 |
| T1..T8 | the spec's eight deterministic landing-transform rules (marker strip, ref rewrite, heading demotion, qualifier spelling, blockquote disposition, roadmap row, decision-id hygiene, job-table additions) | the locked spec's landing-transform section; output = ci-cd section 2.3 |
| k1..k6 | the six knowledge-lint checks: index resolution, orphans/strays, frontmatter validity, value agreement, slug rules, run-report sections + token coverage | ci-cd-and-distribution.md section 2.3.2; xtask/src/knowledge_lint.rs |

## Links

- ub-knowledge-layer-e4s — the epic (knowledge layer + clarity rule + day-1 enforcement); decide
  record in its comment thread.
- ub-knowledge-layer-e4s.1 — the spec task (closed): the locked spec package + the three gate-round
  comments carrying the saga this report narrates.
- ub-knowledge-layer-e4s.2 — scaffold + templates + doc changes (this landing).
- ub-knowledge-layer-e4s.4 — day-1 enforcement: lint + gate + hooks (this landing).
- Branch `knowledge-layer-landing`; PR pending (backfill optional per the format contract).
- Key files: xtask/src/knowledge_lint.rs; scripts/knowledge/run-report-gate.sh;
  scripts/knowledge/tests/run-report-gate-selftest.sh; scripts/knowledge/tests/landing-verify.sh;
  scripts/knowledge/memory-retire.sh; scripts/hooks/ (three PreToolUse guards);
  docs/plans/ci-cd-and-distribution.md section 2.3; docs/PROCESS.md sections 7–8;
  docs/plans/templates/run-report.md + topic-page.md; .unblock/issues.jsonl.
- The locked spec package and the three review verdicts live in the orchestrator session's scratchpad;
  their normative content landed into the repo homes above per the landing map.
