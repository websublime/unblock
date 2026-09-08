#!/bin/sh
# knowledge-layer-invariants.sh — the standing invariants of the knowledge layer's MACHINERY. It is
# the layer of docs/plans/ci-cd-and-distribution.md §2.3 specified at §2.3.6, and a step of the
# required doc-lint job. Its executable proof is
# scripts/knowledge/tests/knowledge-layer-invariants-selftest.sh, which runs this file against
# throwaway fixture repos and never mutates the live tree.
#
# ROWS. Each failure line names its own row.
#
# SAME-COMMIT rows. The rule that the work, the tracker re-export and the wiki run-report land
# together is still stated in docs/PROCESS.md, CLAUDE.md and the specification. Those documents are
# the ones this check rows; another file may state the rule and carry no row here. Each row is a
# presence grep keyed on the phrasing ITS document uses for THIS rule, because docs/PROCESS.md and
# the specification also carry unrelated same-commit sentences (the D-range cascade; the sibling
# gates' knob sequencing) that a plain-phrase check would accept in the rule's place. A line window
# cannot tell this rule from those, so no proximity heuristic belongs here.
#
# ZERO rows. Each sweep must find nothing: retired draft names (this check's own pre-rename filename
# among them), the retired PROCESS section-7 title, stale STATUS.md refs inside the templates,
# bracketed R, MF and A markers inside landed normative docs, and MF and A markers in any grammar,
# plus bracketed R, inside non-fixture scripts.
#
# LANDED rows. Every layer script is present and executable. The lint module and its corpus test,
# both templates, both index files and both wiki dirs are present. The last landed row reads the
# TOP-LEVEL "hooks" object of .claude/settings.json and anchors on the key line, because the bare
# token also occurs in that file as nested per-matcher arrays and a bare-token grep survives the very
# mutation it exists to catch.
#
# WIRING rows. The doc-lint job and the specification each NAME this file and its selftest, the
# specification carries the §2.3.6 heading, and docs/PROCESS.md names this file — so the check can
# ship neither wired-but-unspecified nor specified-but-unwired.
#
# SCOPE. .knowledge/** (descriptive pages quote anything), .unblock/issues.jsonl (the generated
# tracker export), this file and its selftest (their literals are the rules and the fixture data) are
# carved out of the whole-tree sweeps. Everything else tracked is in scope.
#
# WHAT THIS IS NOT. It pins no decision range, so it carries no RANGE_RE knob and is absent from the
# docs/PROCESS.md section 3 list of knob-bearing scripts — the same footing as
# scripts/knowledge/tests/run-report-gate-selftest.sh. It also carries no allowlist, and none may be
# added, because an allowlist is maintained state that goes red the moment later, legitimate work
# mentions its tokens.
#
# PORTABILITY. Every variable expansion goes through printf rather than echo, no piped while
# surrounds state that must survive the loop, and no case arm sits inside a $( ) substitution
# (macOS /bin/sh). Each rule is deliberate.
#
# Exit: 0 = every row clean · 1 = at least one row failed (each listed on stderr) · 2 = cannot
#       evaluate (not inside a git repository; fail-closed).
set -u
top=$(git rev-parse --show-toplevel 2>/dev/null) \
  || { printf 'knowledge-layer-invariants: not a git repository (cannot evaluate)\n' >&2; exit 2; }
cd "$top" || { printf 'knowledge-layer-invariants: cannot cd to the repo root (cannot evaluate)\n' >&2; exit 2; }
SELF="scripts/knowledge/knowledge-layer-invariants.sh"
SELFTEST="scripts/knowledge/tests/knowledge-layer-invariants-selftest.sh"
WORKFLOW=".github/workflows/ci.yml"
SPEC="docs/plans/ci-cd-and-distribution.md"
fail=0
bad() { printf 'knowledge-layer-invariants: FAIL — %s\n' "$*" >&2; fail=1; }

# -- SAME-COMMIT rows. docs/PROCESS.md, CLAUDE.md and the specification each state the rule ----------
# Those documents are the ones this check rows; another file may state the rule and carry no row
# here. Each row keys on the phrasing ITS document uses for this rule. If an unrelated same-commit
# sentence is ever added to CLAUDE.md, that row moves to a discriminating spelling the same way the
# rows over the other documents already did — a presence grep proves the phrase is in the file,
# nothing finer.
git grep -q -i -F "same commit/PR" -- docs/PROCESS.md \
  || bad "docs/PROCESS.md no longer states the run-report same-commit rule (section 8)"
git grep -q -i -F "same commit" -- CLAUDE.md \
  || bad "CLAUDE.md no longer states the same-commit rule"
git grep -q -i -F "same commit/PR" -- "$SPEC" \
  || bad "$SPEC no longer states the run-report same-commit rule (the §2.3 intro)"

# -- ZERO rows. Retired names (this check's pre-rename filename among them), the retired PROCESS
#    section-7 title, stale refs in the templates and stripped audit markers each have zero hits ----
zero() {
  pat="$1"; label="$2"; shift 2
  if git grep -n -E -e "$pat" -- "$@" >&2; then bad "live hits of $label (listed above; must be zero)"; fi
}
# The pattern below also carries this check's own pre-rename filename. The 2026-07-23 run-report
# keeps that name as history, and the row excludes .knowledge.
zero 'memory_retire|run_report_gate|substantive_diff|require_run_report|knowledge-memories-guard|gh-pr-create-gate|lint_common|landing-verify|landing_verify' \
  "retired draft names" . ":(exclude).knowledge" ":(exclude)$SELF" ":(exclude)$SELFTEST" ":(exclude).unblock/issues.jsonl"
zero 'Language & artifacts' "the retired PROCESS section-7 title" . ":(exclude).knowledge" ":(exclude)$SELF" ":(exclude)$SELFTEST" ":(exclude).unblock/issues.jsonl"
zero 'STATUS\.md' "stale STATUS.md refs in templates (both template halves)" docs/plans/templates/
zero '\[(R|MF|A)-?[0-9]+\]' "scratchpad audit markers in landed normative docs (stripped at landing)" \
  docs/plans/ci-cd-and-distribution.md docs/plans/templates/ docs/PROCESS.md CLAUDE.md .github/workflows/ci.yml
zero '\((MF|A)-[0-9]+[^)]*\)|\[(R|MF|A)-?[0-9]+\]|(MF|A)-[0-9]+' "audit markers in non-fixture scripts + ci.yml (landing transform)" \
  scripts/knowledge/run-report-gate.sh scripts/knowledge/memory-retire.sh scripts/hooks/ .github/workflows/ci.yml

# This file and its selftest are excluded from the retired-name and retitle sweeps, because their
# literals are the RULES and the selftest's negative fixtures plant them as test data.
# run-report-gate-selftest.sh and knowledge_lint.rs are excluded from the marker scans, because their
# own fixtures COIN such tokens as test data. The selftest sits under scripts/knowledge/tests/, which
# neither marker scan reaches. The templates and PROCESS section 7 keep their parenthesised EXAMPLE
# ids by design, and the doc scan above matches only the square-bracket grammar. The selftest file
# may therefore carry any retired name, which is a stated blind spot.

# -- LANDED rows. Every landed script is present and executable, every landed file and wiki dir
#    exists, and .claude/settings.json keeps its top-level hooks object ----------------------------
for f in scripts/knowledge/run-report-gate.sh scripts/knowledge/memory-retire.sh \
         scripts/knowledge/tests/run-report-gate-selftest.sh "$SELF" "$SELFTEST" \
         scripts/hooks/knowledge-memories-write-guard.py \
         scripts/hooks/knowledge-memories-bash-guard.py scripts/hooks/pr-create-run-report-gate.py; do
  if [ ! -f "$f" ]; then bad "missing landed script: $f"; continue; fi
  [ -x "$f" ] || bad "not executable (0755 expected): $f"
done
for f in .knowledge/memories/index.md .knowledge/wiki/index.md xtask/src/knowledge_lint.rs \
         xtask/tests/knowledge_lint_corpus.rs docs/plans/templates/run-report.md \
         docs/plans/templates/topic-page.md; do
  [ -f "$f" ] || bad "missing landed file: $f"
done
[ -d .knowledge/wiki/runs ] || bad "missing dir: .knowledge/wiki/runs"
[ -d .knowledge/wiki/topics ] || bad "missing dir: .knowledge/wiki/topics"

# This row anchors on the top-level key LINE, its indentation plus the object opener. The bare token
# "hooks" also occurs in that file as nested per-matcher ARRAYS, so a bare-token grep survives the
# very mutation it exists to catch. The regex carries the indentation Claude Code writes, so no prose
# has to. A reformat of that file is itself substantive under the run-report gate, and this row's
# message names the file.
git grep -q -E '^  "hooks": *\{' -- .claude/settings.json \
  || bad ".claude/settings.json has no top-level \"hooks\" object (ci-cd §2.3.4)"

# -- WIRING rows. The doc-lint job and the specification each NAME this file and its selftest, the
#    specification carries the §2.3.6 heading, and docs/PROCESS.md names this file — so the check can
#    ship neither wired-but-unspecified nor specified-but-unwired ------------------------------------
git grep -q -F "$SELF" -- "$WORKFLOW" \
  || bad "not wired: $WORKFLOW does not name $SELF"
git grep -q -F "$SELFTEST" -- "$WORKFLOW" \
  || bad "not wired: $WORKFLOW does not name $SELFTEST"
git grep -q -F "$SELF" -- "$SPEC" \
  || bad "not specified: $SPEC does not name $SELF"
git grep -q -F "$SELFTEST" -- "$SPEC" \
  || bad "not specified: $SPEC does not name $SELFTEST"
git grep -q -E '^#### 2\.3\.6 ' -- "$SPEC" \
  || bad "not specified: $SPEC carries no §2.3.6 heading (the subsection that specifies this check)"

# This row proves that docs/PROCESS.md names this file somewhere. The section 8 layer list is where
# it belongs and the section 3 knob list is where it must not go, and a file-level row cannot tell
# those apart. docs/PROCESS.md is out of the doc-lint corpus, and the sibling gates that read it pin
# only its section 3 script list, so without this row nothing in the tree notices the section 8 layer
# list going stale.
git grep -q -F "$SELF" -- docs/PROCESS.md \
  || bad "not named in docs/PROCESS.md: the section 8 layer list must name $SELF"

[ "$fail" -eq 0 ] && { printf 'knowledge-layer-invariants OK: every row clean\n'; exit 0; }
printf 'knowledge-layer-invariants: FAILURES (see above)\n' >&2
exit 1
