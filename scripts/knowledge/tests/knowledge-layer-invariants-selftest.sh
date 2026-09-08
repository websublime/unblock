#!/bin/sh
# knowledge-layer-invariants-selftest.sh — executable proof of
# scripts/knowledge/knowledge-layer-invariants.sh (docs/plans/ci-cd-and-distribution.md §2.3.6).
# It is a fixture-repo harness in pure POSIX sh + git, offline and deterministic, shaped like
# run-report-gate-selftest.sh.
#
# PER CASE. Build a throwaway git repo under mktemp -d carrying the whole landed set as stubs, apply
# ONE mutation, run the real check with the fixture as its working directory, and assert the exit
# code, the EXACT number of FAIL rows, a distinguishing stderr substring, and stdout (the OK line on
# a pass, nothing on a failure).
#
# WHAT THE BASE TREE CARRIES. It carries every landed path at its own relative path, the documents
# that state the same-commit rule, both wiki dirs, a settings.json reproducing the live nested-array
# hooks shape, a workflow stub, and a specification stub naming both scripts.
#
# WHY THE FIXTURES LOOK LIKE THE LIVE TREE. The documents that state the same-commit rule also carry
# the DECOY sentences the live files carry, so each negative case proves its row keys on this rule
# rather than on a neighbour. The specification stub also cites §2.3.6 in prose, so the heading row
# must anchor at the start of a line to stay green. The baseline plants the retired names in every
# excluded path, so each exclusion is proven to exclude.
#
# TEST DATA. The fixtures COIN audit-marker-style tokens and spell the retired names. The check's
# marker scans do not reach this path and its whole-tree sweeps exclude it by name.
#
# GUARD. Everything this file does to .knowledge-shaped paths happens inside mktemp trees and inside
# this file, so it is guard-compatible under the PreToolUse bash guard by construction. Never add a
# --repo flag; never document an invocation that pairs a mutating verb with .knowledge.
#
# SINGLE SOURCE. The landed-set lists are spelled ONCE here and drive both the base tree and the
# negative cases, so a list that drifts from the check's is red in both directions.
#
# Exit 0 when every case passes, 1 on any failure.
set -u

SELF_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH='' cd -- "$SELF_DIR/../../.." && pwd)
CHECK_REL="scripts/knowledge/knowledge-layer-invariants.sh"
SELFTEST_REL="scripts/knowledge/tests/knowledge-layer-invariants-selftest.sh"
CHECK="$REPO_ROOT/$CHECK_REL"
WORKFLOW_REL=".github/workflows/ci.yml"
SPEC_REL="docs/plans/ci-cd-and-distribution.md"
OK_LINE="knowledge-layer-invariants OK: every row clean"
FAIL_PREFIX="knowledge-layer-invariants: FAIL — "

fail=0
bad() { printf 'selftest: FAIL — %s\n' "$*" >&2; fail=1; }
note() { printf 'selftest: %s\n' "$*"; }

GITC="git -c user.name=selftest -c user.email=selftest@localhost -c commit.gpgsign=false"

# The retired literals are spelled once, as TEST DATA. This file is carved out of the check's
# retired-name and retitle sweeps by name, and the marker scans never reach scripts/knowledge/tests/.
RETIRED_NAME='run_report_gate'
RETIRED_OLD='landing-verify'
RETIRED_TITLE='Language & artifacts'

# The landed sets are spelled ONCE here. They build the base tree AND drive the negative cases, so a
# path dropped from the check's own loop leaves its case with no failure to find, and a path added to
# the check but not here makes the baseline red.
SCRIPTS="scripts/knowledge/run-report-gate.sh
scripts/knowledge/memory-retire.sh
scripts/knowledge/tests/run-report-gate-selftest.sh
$CHECK_REL
$SELFTEST_REL
scripts/hooks/knowledge-memories-write-guard.py
scripts/hooks/knowledge-memories-bash-guard.py
scripts/hooks/pr-create-run-report-gate.py"

FILES=".knowledge/memories/index.md
.knowledge/wiki/index.md
xtask/src/knowledge_lint.rs
xtask/tests/knowledge_lint_corpus.rs
docs/plans/templates/run-report.md
docs/plans/templates/topic-page.md"

DIRS=".knowledge/wiki/runs
.knowledge/wiki/topics"

# ---------------------------------------------------------------------------------------------------
# Live-tree pins, which are not fixture cases. They are the mutual half of the check's own
# presence-and-mode row, and the workflow naming the check — which the check cannot observe about
# itself from its own step.
# ---------------------------------------------------------------------------------------------------
[ -x "$CHECK" ] || bad "the check is missing or not executable in this checkout: $CHECK"
grep -F -e "$CHECK_REL" "$REPO_ROOT/$WORKFLOW_REL" >/dev/null 2>&1 \
  || bad "the live $WORKFLOW_REL does not name $CHECK_REL"

# ---------------------------------------------------------------------------------------------------
# Fixture-repo helpers.
# ---------------------------------------------------------------------------------------------------

# mk_base <dir> — build the positive baseline. It plants every landed path as a stub at its own
# relative path, the documents that state the same-commit rule (each with the DECOY sentences its
# live original carries), a workflow stub and a specification stub naming both scripts, and the
# retired literals in every excluded path so each exclusion is proven to exclude. The specification
# stub also cites §2.3.6 in prose, so un-anchoring the heading row's pattern leaves that row green on
# the case that strips the heading. In that stub the §2.3.6 heading and the line naming the check's
# path sit on separate lines, because one line carrying both would make the case that strips the
# check's path fail the heading row too and break that case's row-count assertion.
mk_base() {
  d="$1"
  git init -q -b main "$d" >/dev/null 2>&1 || { bad "git init failed in $d"; return; }
  (
    cd "$d" || exit 9
    for p in $SCRIPTS; do
      mkdir -p "$(dirname "$p")"
      printf '#!/bin/sh\nexit 0\n' > "$p"
      chmod 0755 "$p"
    done
    printf '#!/bin/sh\n# fixture stub naming %s and the retired "%s" title\nexit 0\n' \
      "$RETIRED_NAME" "$RETIRED_TITLE" > "$CHECK_REL"
    chmod 0755 "$CHECK_REL"
    printf '#!/bin/sh\n# fixture stub: %s %s "%s" [MF-2] (MF-3 fixture)\nexit 0\n' \
      "$RETIRED_NAME" "$RETIRED_OLD" "$RETIRED_TITLE" > "$SELFTEST_REL"
    chmod 0755 "$SELFTEST_REL"

    for p in $FILES; do
      mkdir -p "$(dirname "$p")"
      printf 'fixture stub at %s\n' "$p" > "$p"
    done
    for p in $DIRS; do mkdir -p "$p"; done
    printf 'an old descriptive run-report: %s %s "%s" [MF-1]\n' \
      "$RETIRED_NAME" "$RETIRED_OLD" "$RETIRED_TITLE" > .knowledge/wiki/runs/2026-01-01-old.md
    printf 'a fixture topic page\n' > .knowledge/wiki/topics/t.md

    mkdir -p docs .github/workflows .claude .unblock
    printf '%s\n' \
      '# PROCESS (fixture stub)' \
      '- The work, the tracker re-export and the wiki run-report land in the same commit/PR.' \
      '- Decoy — the D-range cascade moves every file the list names, all in the same commit.' \
      '- Decoy — re-export the tracker record in the same commit as the work.' \
      "- The enforcement layers are a list that carries no count, and $CHECK_REL is a required doc-lint step." \
      > docs/PROCESS.md
    printf '%s\n' \
      '# CLAUDE (fixture stub)' \
      'Re-export the git record and land the wiki run-report in the same commit as the work.' \
      'Pointers over prose.' \
      > CLAUDE.md
    mkdir -p "$(dirname "$SPEC_REL")"
    printf '%s\n' \
      '# CI/CD (fixture stub)' \
      'A substantive PR carries its wiki run-report in the same commit/PR as the work.' \
      'Decoy — the D-range cascade sites all move in the same commit.' \
      'Decoy — the contract knob rides the same commit as the implementation.' \
      'Layer (iv) of this section is specified at §2.3.6 and runs in the doc-lint job.' \
      '#### 2.3.6 Layer (iv): the knowledge-layer invariants check' \
      "The check is $CHECK_REL, a step of the required doc-lint job." \
      "Its executable proof is $SELFTEST_REL." \
      > "$SPEC_REL"
    printf '%s\n' \
      'name: ci' \
      'jobs:' \
      '  doc-lint:' \
      '    steps:' \
      "      - run: $CHECK_REL" \
      "      - run: $SELFTEST_REL" \
      > "$WORKFLOW_REL"
    printf '%s\n' \
      'a plain tracked document' \
      'with nothing retired in it' \
      > docs/notes.md
    printf '%s\n' \
      '{' \
      '  "permissions": {' \
      '    "allow": []' \
      '  },' \
      '  "hooks": {' \
      '    "PreToolUse": [' \
      '      {' \
      '        "matcher": "Bash",' \
      '        "hooks": [' \
      '          {' \
      '            "type": "command",' \
      '            "command": "scripts/hooks/knowledge-memories-bash-guard.py"' \
      '          }' \
      '        ]' \
      '      }' \
      '    ]' \
      '  }' \
      '}' \
      > .claude/settings.json
    printf '{"id":"ub-fixture.1","text":"the generated export quotes %s and the retired \\"%s\\" title"}\n' \
      "$RETIRED_NAME" "$RETIRED_TITLE" > .unblock/issues.jsonl

    $GITC add -A
    $GITC commit -qm base
  ) || bad "base build failed in $d"
}

# new_case — build a fresh fixture repo for one case and set $repo. Cases never share a tree.
new_case() {
  repo=$(mktemp -d) || { bad "mktemp failed"; exit 1; }
  mk_base "$repo"
}

# no_repo_case — make a fresh directory outside any git repository and set $repo.
no_repo_case() {
  repo=$(mktemp -d) || { bad "mktemp failed"; exit 1; }
}

done_case() { rm -rf "$repo"; }

# stage — make the index match the working tree, in both directions, so a content edit, a new file
# and a deletion are all visible to git grep.
stage() { ( cd "$repo" && $GITC add -A ); }

# strip_lines <file> <fixed pattern> — remove every line containing the pattern, case-insensitively.
# grep's exit status is deliberately IGNORED. A stub whose every line matched would make `&&` skip the
# move and leave the fixture unmutated, and the case would then report a pass where a failure is
# expected. Every stub also carries a line no pattern matches, so both guards are in place.
strip_lines() {
  sf="$repo/$1"; sp="$2"
  grep -v -i -F -e "$sp" "$sf" > "$sf.tmp"
  mv "$sf.tmp" "$sf"
}

# check <case-id> <want-exit> <want-fail-lines> <needle|-> — run the REAL check with the fixture as
# its working directory and assert, IN ORDER: the exit code, the EXACT count of FAIL rows, the
# distinguishing stderr substring, and stdout (the OK line on a pass, empty on a failure).
check() {
  cid="$1"; wexit="$2"; wfails="$3"; needle="$4"
  outf=$(mktemp) || { bad "mktemp failed"; return; }
  errf=$(mktemp) || { bad "mktemp failed"; return; }
  ( cd "$repo" && "$CHECK" ) > "$outf" 2> "$errf"
  code=$?
  nfail=$(grep -c -F -e "$FAIL_PREFIX" "$errf")
  got=$(cat "$outf")
  if [ "$code" -ne "$wexit" ]; then
    bad "case $cid: exit $code != expected $wexit (stderr: $(tr '\n' ' ' < "$errf"))"
  elif [ "$nfail" -ne "$wfails" ]; then
    bad "case $cid: $nfail FAIL row(s) != expected $wfails (stderr: $(tr '\n' ' ' < "$errf"))"
  elif [ "$needle" != "-" ] && ! grep -F -e "$needle" "$errf" >/dev/null 2>&1; then
    bad "case $cid: stderr lacks '$needle' (stderr: $(tr '\n' ' ' < "$errf"))"
  elif [ "$wexit" -eq 0 ] && [ "$got" != "$OK_LINE" ]; then
    bad "case $cid: stdout is not the OK line (stdout: $got)"
  elif [ "$wexit" -ne 0 ] && [ -n "$got" ]; then
    bad "case $cid: stdout is not empty on a failing run (stdout: $got)"
  else
    note "case $cid ok (exit $code, $nfail FAIL row(s))"
  fi
  rm -f "$outf" "$errf"
}

# ---------------------------------------------------------------------------------------------------
# The case matrix — the positive baseline, the non-repository case, and the negative cases, at least
# one per row.
# ---------------------------------------------------------------------------------------------------

new_case
check "baseline" 0 0 "-"
done_case

no_repo_case
check "no-git" 2 0 "not a git repository"
done_case

new_case
strip_lines "docs/PROCESS.md" "same commit/PR"
stage
check "same-commit-process" 1 1 "docs/PROCESS.md no longer states the run-report same-commit rule"
done_case

new_case
strip_lines "CLAUDE.md" "same commit"
stage
check "same-commit-claude" 1 1 "CLAUDE.md no longer states the same-commit rule"
done_case

new_case
strip_lines "$SPEC_REL" "same commit/PR"
stage
check "same-commit-cicd" 1 1 "no longer states the run-report same-commit rule (the §2.3 intro)"
done_case

new_case
printf 'a live document mentioning %s\n' "$RETIRED_NAME" >> "$repo/docs/notes.md"
stage
check "retired-name" 1 1 "live hits of retired draft names"
done_case

new_case
printf '%s in a new staged file\n' "$RETIRED_NAME" > "$repo/docs/new.md"
stage
check "retired-name-new-file" 1 1 "docs/new.md:1:"
done_case

new_case
printf 'a live document mentioning %s\n' "$RETIRED_OLD" >> "$repo/docs/notes.md"
stage
check "retired-name-old-filename" 1 1 "live hits of retired draft names"
done_case

new_case
printf '# one more %s in the excluded fixture path\n' "$RETIRED_NAME" >> "$repo/$SELFTEST_REL"
stage
check "retired-name-selftest-excluded" 0 0 "-"
done_case

new_case
printf 'a live document mentioning the retired "%s" title\n' "$RETIRED_TITLE" >> "$repo/docs/notes.md"
stage
check "retired-title" 1 1 "the retired PROCESS section-7 title"
done_case

new_case
printf 'see STATUS.md for the registry\n' >> "$repo/docs/plans/templates/run-report.md"
stage
check "stale-status-ref" 1 1 "stale STATUS.md refs in templates"
done_case

new_case
printf 'a stray scratchpad marker [MF-9] left in a landed document\n' >> "$repo/docs/PROCESS.md"
stage
check "marker-in-doc" 1 1 "scratchpad audit markers in landed normative docs"
done_case

new_case
printf '# a stray scratchpad marker (MF-9 left in a landed script)\n' \
  >> "$repo/scripts/hooks/knowledge-memories-bash-guard.py"
stage
check "marker-in-script" 1 1 "audit markers in non-fixture scripts"
done_case

for p in $SCRIPTS; do
  new_case
  rm -f "$repo/$p"
  stage
  check "missing-script-$p" 1 1 "missing landed script: $p"
  done_case
done

for p in $SCRIPTS; do
  new_case
  chmod 0644 "$repo/$p"
  stage
  check "mode-script-$p" 1 1 "not executable (0755 expected): $p"
  done_case
done

for p in $FILES; do
  new_case
  rm -f "$repo/$p"
  stage
  check "missing-file-$p" 1 1 "missing landed file: $p"
  done_case
done

for p in $DIRS; do
  new_case
  rm -rf "${repo:?}/$p"
  stage
  check "missing-dir-$p" 1 1 "missing dir: $p"
  done_case
done

new_case
sed 's/^  "hooks": {/  "hooks_off": {/' "$repo/.claude/settings.json" > "$repo/.claude/settings.tmp"
mv "$repo/.claude/settings.tmp" "$repo/.claude/settings.json"
grep -F -e '"hooks"' "$repo/.claude/settings.json" >/dev/null 2>&1 \
  || bad "case hooks-top-level-renamed: the bare token no longer survives, so the case would not prove the hardening"
stage
check "hooks-top-level-renamed" 1 1 'has no top-level "hooks" object'
done_case

new_case
strip_lines "$WORKFLOW_REL" "$CHECK_REL"
stage
check "unwired-check" 1 1 "does not name $CHECK_REL"
done_case

new_case
strip_lines "$WORKFLOW_REL" "$SELFTEST_REL"
stage
check "unwired-selftest" 1 1 "does not name $SELFTEST_REL"
done_case

new_case
strip_lines "$SPEC_REL" "$CHECK_REL"
stage
check "unspecified-check" 1 1 "not specified: $SPEC_REL does not name $CHECK_REL"
done_case

new_case
strip_lines "$SPEC_REL" "$SELFTEST_REL"
stage
check "unspecified-selftest" 1 1 "not specified: $SPEC_REL does not name $SELFTEST_REL"
done_case

new_case
strip_lines "$SPEC_REL" "#### 2.3.6 "
stage
check "unspecified-no-subsection" 1 1 "carries no §2.3.6 heading"
done_case

new_case
strip_lines "docs/PROCESS.md" "$CHECK_REL"
stage
check "unlisted-in-process" 1 1 "the section 8 layer list must name"
done_case

[ "$fail" -eq 0 ] && { printf 'knowledge-layer-invariants-selftest OK: every case clean\n'; exit 0; }
printf 'knowledge-layer-invariants-selftest: FAILURES (see above)\n' >&2
exit 1
