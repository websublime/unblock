#!/bin/sh
# landing-verify.sh — executable landing verification for the knowledge layer (spec:
# ci-cd-and-distribution.md §2.3; lands with the PR and stays re-runnable). Semantics: "allow" lists
# are MAY-appear subsets — never a must-appear count; .knowledge/** is carved out of every scan
# (descriptive pages quote anything).
# Exit: 0 = all checks clean · 1 = failures (each listed on stderr).
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
SELF="scripts/knowledge/tests/landing-verify.sh"
fail=0
bad() { printf 'landing-verify: FAIL — %s\n' "$*" >&2; fail=1; }

# -- A) token containment: files mentioning a token must be a subset of its allowlist ---------------
allow() {
  tok="$1"; shift
  for hit in $(git grep -l -F -e "$tok" -- . ":(exclude).knowledge" ":(exclude).unblock/issues.jsonl" 2>/dev/null); do
    ok=0
    for a in "$@"; do if [ "$hit" = "$a" ]; then ok=1; break; fi; done
    [ "$ok" -eq 1 ] || bad "token '$tok' appears in unexpected file: $hit"
  done
}

allow "knowledge-lint" \
  xtask/src/lib.rs xtask/src/main.rs xtask/src/knowledge_lint.rs \
  xtask/tests/knowledge_lint_corpus.rs .github/workflows/ci.yml \
  docs/plans/ci-cd-and-distribution.md docs/PROCESS.md docs/plans/templates/run-report.md "$SELF"

allow "run-report-gate" \
  .github/workflows/ci.yml scripts/knowledge/run-report-gate.sh \
  scripts/knowledge/tests/run-report-gate-selftest.sh scripts/hooks/pr-create-run-report-gate.py \
  docs/PROCESS.md docs/plans/ci-cd-and-distribution.md .claude/settings.json \
  xtask/src/knowledge_lint.rs "$SELF"
# ^ xtask/src/knowledge_lint.rs: the const-equality pin reads the gate script by path (the Rust
#   SESSION_LOCAL_ID_RE const must equal the script's sh const), so the file legitimately names it.

allow "memory-retire" \
  scripts/knowledge/memory-retire.sh scripts/hooks/knowledge-memories-bash-guard.py \
  scripts/hooks/knowledge-memories-write-guard.py docs/PROCESS.md \
  docs/plans/ci-cd-and-distribution.md "$SELF"

allow ".knowledge" \
  CLAUDE.md docs/PROCESS.md docs/plans/ci-cd-and-distribution.md \
  docs/plans/templates/run-report.md docs/plans/templates/topic-page.md \
  docs/plans/templates/drift-gap-report.md .github/workflows/ci.yml \
  xtask/src/knowledge_lint.rs xtask/tests/knowledge_lint_corpus.rs xtask/tests/doc_lint_corpus.rs \
  scripts/knowledge/run-report-gate.sh scripts/knowledge/memory-retire.sh \
  scripts/knowledge/tests/run-report-gate-selftest.sh \
  scripts/hooks/knowledge-memories-write-guard.py scripts/hooks/knowledge-memories-bash-guard.py \
  scripts/hooks/pr-create-run-report-gate.py \
  docs/plans/00-roadmap.md "$SELF"
# ^ docs/plans/00-roadmap.md: the roadmap §7 docs-in-DB row (landed with this layer) names .knowledge;
#   allow = may-appear, so the entry stays correct if the resequence cascade later restyles the row.

# -- B) same-commit rule sites: every live sentence names the run-report -----------------------------
git grep -q -i -F "same commit" -- docs/PROCESS.md || bad "same-commit rule missing from PROCESS.md"
git grep -q -i -F "same commit" -- CLAUDE.md || bad "same-commit rule missing from CLAUDE.md"
git grep -q -i -F "same commit" -- docs/plans/ci-cd-and-distribution.md || bad "same-commit rule missing from ci-cd (the §2.3 intro sentence)"
if ! git grep -i -n -A2 -F "same commit" -- CLAUDE.md docs/PROCESS.md docs/plans/ci-cd-and-distribution.md docs/plans/STATUS.md \
  | awk 'BEGIN { open = 0; seen = 1; ok = 1 }
      /^--$/ { if (open && !seen) ok = 0; open = 0; next }
      { low = tolower($0)
        if (low ~ /same commit/) { if (open && !seen) ok = 0; open = 1; seen = 0 }
        if (open && low ~ /run-report/) seen = 1 }
      END { if (open && !seen) ok = 0; exit ok ? 0 : 1 }'
then
  bad "a live same-commit sentence lacks the run-report clause in its 2-line window"
fi

# -- C) zero-live-hits (retired names, retitles, stale refs, stripped audit markers) -----------------
zero() {
  pat="$1"; label="$2"; shift 2
  if git grep -n -E -e "$pat" -- "$@"; then bad "live hits of $label (listed above; must be zero)"; fi
}
zero 'memory_retire|run_report_gate|substantive_diff|require_run_report|knowledge-memories-guard|gh-pr-create-gate|lint_common' \
  "retired draft names" . ":(exclude).knowledge" ":(exclude)$SELF" ":(exclude).unblock/issues.jsonl"
zero 'Language & artifacts' "the retired PROCESS section-7 title" . ":(exclude).knowledge" ":(exclude)$SELF" ":(exclude).unblock/issues.jsonl"
zero 'STATUS\.md' "stale STATUS.md refs in templates (both template halves)" docs/plans/templates/
zero '\[(R|MF|A)-?[0-9]+\]' "scratchpad audit markers in landed normative docs (stripped at landing)" \
  docs/plans/ci-cd-and-distribution.md docs/plans/templates/ docs/PROCESS.md CLAUDE.md .github/workflows/ci.yml
zero '\((MF|A)-[0-9]+[^)]*\)|\[(R|MF|A)-?[0-9]+\]|(MF|A)-[0-9]+' "audit markers in non-fixture scripts + ci.yml (landing transform)" \
  scripts/knowledge/run-report-gate.sh scripts/knowledge/memory-retire.sh scripts/hooks/ .github/workflows/ci.yml
# (Deliberately EXCLUDED from the marker scan: run-report-gate-selftest.sh and knowledge_lint.rs — their
#  fixtures COIN MF-style tokens as test data (the selftest case matrix; the knowledge-lint arm-B
#  fixtures). The templates and PROCESS §7 keep their parenthetical EXAMPLE ids like "(MF-2)" by design —
#  only the square-bracket marker grammar is banned from landed docs, which is exactly what the pattern
#  above matches.)

# -- D) landed-set existence + mode ------------------------------------------------------------------
for f in scripts/knowledge/run-report-gate.sh scripts/knowledge/memory-retire.sh \
         scripts/knowledge/tests/run-report-gate-selftest.sh "$SELF" \
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
git grep -q '"hooks"' -- .claude/settings.json || bad ".claude/settings.json has no hooks block (ci-cd §2.3.4)"

[ "$fail" -eq 0 ] && { echo "landing-verify OK: all checks clean"; exit 0; }
echo "landing-verify: FAILURES (see above)" >&2
exit 1
