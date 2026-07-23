#!/bin/sh
# run-report-gate-selftest.sh — executable proof of the shared gate predicate
# scripts/knowledge/run-report-gate.sh (ci-cd §2.3.3). Fixture-repo harness, pure POSIX sh + git,
# offline and deterministic: for each case, build a throwaway repo under mktemp -d, apply the case's
# diff on a branch, run the gate, and assert BOTH the exit code and a distinguishing stderr rationale
# substring. Also pins the script's SESSION_LOCAL_ID_RE= literal against the spec literal (and the
# landed ci-cd §2.3.3 text) — the single-sourcing pin. Covers every arm of the script: each of the 14
# rule-3 case-globs (the .md-inside-dir cases double as case-arm ORDER pins), the *.toml /
# manifest-mixed / docs / fallthrough arms, rules 1/1a/2, 4a-4c including 4c's binary fail-closed arm,
# and rule 7 including its binary numstat exclusion — and all three exit codes 0/1/2. The dependabot
# exemption is job-level `if:` metadata, outside the script.
# Exit: 0 = all cases pass · 1 = any failure.
set -u

SELF_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH='' cd -- "$SELF_DIR/../../.." && pwd)
GATE="$REPO_ROOT/scripts/knowledge/run-report-gate.sh"
CICD_DOC="$REPO_ROOT/docs/plans/ci-cd-and-distribution.md"

fail=0
bad() { printf 'selftest: FAIL — %s\n' "$*" >&2; fail=1; }
note() { printf 'selftest: %s\n' "$*"; }

[ -x "$GATE" ] || { bad "gate script missing or not executable: $GATE"; echo "selftest: FAILURES" >&2; exit 1; }

# ---------------------------------------------------------------------------------------------------
# Pin: the SESSION_LOCAL_ID_RE literal is single-sourced (spec == script == landed ci-cd text).
# ---------------------------------------------------------------------------------------------------
LIT='(^|[^A-Za-z0-9-])(MF|CF|M|R|F|A)-?[0-9]+([^0-9]|$)'
got=$(sed -n "s/^SESSION_LOCAL_ID_RE='\(.*\)'\$/\1/p" "$GATE")
[ "$got" = "$LIT" ] || bad "SESSION_LOCAL_ID_RE in the gate script != the spec literal (got: $got)"
grep -F "$LIT" "$CICD_DOC" >/dev/null 2>&1 \
  || bad "the landed ci-cd doc does not carry the SESSION_LOCAL_ID_RE literal (single-sourcing pin)"

# ---------------------------------------------------------------------------------------------------
# Fixture-repo helpers.
# ---------------------------------------------------------------------------------------------------
GITC="git -c user.name=selftest -c user.email=selftest@localhost -c commit.gpgsign=false"

# mk_base <dir> <export-line> — a mini-tree base commit: one crate file, one doc, a Cargo manifest
# pair, the .knowledge skeleton (incl. one old run-report), and a one-record issues.jsonl.
mk_base() {
  d="$1"; export_line="$2"
  git init -q -b main "$d"
  (
    cd "$d" || exit 9
    mkdir -p crates/unblock-x/src docs .knowledge/memories .knowledge/wiki/runs .knowledge/wiki/topics .unblock
    echo 'pub fn x() {}' > crates/unblock-x/src/lib.rs
    i=1; while [ "$i" -le 30 ]; do echo "line $i of the fixture doc" >> docs/notes.md; i=$((i+1)); done
    printf '[package]\nname = "fixture"\n' > Cargo.toml
    printf '# lock\n' > Cargo.lock
    printf '# Memory index\n\nOne line per memory.\n' > .knowledge/memories/index.md
    printf '# Wiki index\n\n## Runs\n\n## Topics\n' > .knowledge/wiki/index.md
    i=1; while [ "$i" -le 12 ]; do echo "old report line $i" >> .knowledge/wiki/runs/2026-01-01-old.md; i=$((i+1)); done
    printf '%s\n' "$export_line" > .unblock/issues.jsonl
    $GITC add -A
    $GITC commit -qm base
  ) || bad "base build failed in $d"
}

EXPORT_PLAIN='{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","created_at":"2026-07-21T12:00:00Z","text":"initial note"}]}'
EXPORT_WITH_TOKEN='{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","created_at":"2026-07-21T12:00:00Z","text":"already carries MF-9 here"}]}'

# add_report <dir> <path> <lines> — a plain-text report file with N lines.
add_report() {
  rd="$1"; rp="$2"; rn="$3"
  mkdir -p "$rd/$(dirname "$rp")"
  i=1; while [ "$i" -le "$rn" ]; do echo "report line $i" >> "$rd/$rp"; i=$((i+1)); done
}

# check <case#> <want-exit> <stderr-needle or -> — run the gate on the prepared repo and assert.
# The repo (with its case branch committed) is $repo; base ref passed to the gate is $2 of the gate.
check() {
  cnum="$1"; want="$2"; needle="$3"; ref="${4:-main}"
  errf="$repo/.selftest-stderr"
  ( cd "$repo" && "$GATE" "$ref" ) 2> "$errf"
  code=$?
  if [ "$code" -ne "$want" ]; then
    bad "case $cnum: exit $code != expected $want (stderr: $(tr '\n' ' ' < "$errf"))"
    return
  fi
  if [ "$needle" != "-" ] && ! grep -F "$needle" "$errf" >/dev/null 2>&1; then
    bad "case $cnum: stderr lacks '$needle' (stderr: $(tr '\n' ' ' < "$errf"))"
    return
  fi
  note "case $cnum ok (exit $code)"
}

# new_case [export-line] — fresh fixture repo + case branch; sets $repo.
new_case() {
  repo=$(mktemp -d) || { bad "mktemp failed"; exit 1; }
  mk_base "$repo" "${1:-$EXPORT_PLAIN}"
  ( cd "$repo" && $GITC switch -q -c case )
}

commit_case() { ( cd "$repo" && $GITC add -A && $GITC commit -qm case ); }

# ---------------------------------------------------------------------------------------------------
# The case matrix.
# ---------------------------------------------------------------------------------------------------

# 1 — empty diff.
new_case
check 1 0 -

# 2 — .knowledge page + export touch, no new comment codes.
new_case
printf '%s\n' 'a topic page' > "$repo/.knowledge/wiki/topics/t.md"
printf '%s\n' '{"id":"ub-fixture.2","status":"open"}' >> "$repo/.unblock/issues.jsonl"
commit_case
check 2 0 "only neutral paths"

# 3 — export-only diff whose added comment coins a new MF-9-style token.
new_case
printf '%s\n' '{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","created_at":"2026-07-21T12:00:00Z","text":"initial note"},{"id":2,"issue_id":"ub-fixture.1","created_at":"2026-07-22T12:00:00Z","text":"gate verdict coined MF-9"}]}' > "$repo/.unblock/issues.jsonl"
commit_case
check 3 1 "coining session-local id 'MF-9'"

# 4 — case 3 + a qualifying run-report.
new_case
printf '%s\n' '{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","created_at":"2026-07-21T12:00:00Z","text":"initial note"},{"id":2,"issue_id":"ub-fixture.1","created_at":"2026-07-22T12:00:00Z","text":"gate verdict coined MF-9"}]}' > "$repo/.unblock/issues.jsonl"
add_report "$repo" ".knowledge/wiki/runs/2026-07-23-case.md" 12
commit_case
check 4 0 "carries run-report"

# 5 — export record rewrite where the token already existed in the removed line (no re-trigger).
new_case "$EXPORT_WITH_TOKEN"
printf '%s\n' '{"id":"ub-fixture.1","status":"open","comments":[{"id":1,"issue_id":"ub-fixture.1","created_at":"2026-07-21T12:00:00Z","text":"already carries MF-9 here"},{"id":2,"issue_id":"ub-fixture.1","created_at":"2026-07-22T12:00:00Z","text":"no new codes"}]}' > "$repo/.unblock/issues.jsonl"
commit_case
check 5 0 "only neutral paths"

# 6 — Cargo.toml + Cargo.lock only (pure dependency-bump shape).
new_case
printf '\n# bump\n' >> "$repo/Cargo.toml"
printf '# bump\n' >> "$repo/Cargo.lock"
commit_case
check 6 0 "pure manifest"

# 7-20 — one case per rule-3 case-glob (all 14); .md-inside-dir cases pin the case-arm ORDER.
rule3_case() {
  r3num="$1"; r3path="$2"
  new_case
  mkdir -p "$repo/$(dirname "$r3path")"
  echo 'x' > "$repo/$r3path"
  commit_case
  check "$r3num" 1 "always-substantive path: $r3path"
}
rule3_case 7 "crates/README.md"
rule3_case 8 "xtask/NOTES.md"
rule3_case 9 "fuzz/README.md"
rule3_case 10 "scripts/notes.txt"
rule3_case 11 "migrations/0001.md"
rule3_case 12 ".github/PULL_REQUEST_TEMPLATE.md"
rule3_case 13 ".claude/settings.json"
rule3_case 14 ".mcp.json"
rule3_case 15 "main.rs"
rule3_case 16 "query.sql"
rule3_case 17 "build.sh"
rule3_case 18 "tool.py"
rule3_case 19 "rust-toolchain"
rule3_case 20 "rust-toolchain.toml"

# 21 — deny.toml (the non-manifest *.toml arm).
new_case
echo 'x' > "$repo/deny.toml"
commit_case
check 21 1 "non-manifest toml"

# 22 — Cargo.toml mixed with a doc.
new_case
printf '\n# bump\n' >> "$repo/Cargo.toml"
echo 'extra' >> "$repo/docs/notes.md"
commit_case
check 22 1 "manifest mixed"

# 23 — unclassified foo.xyz (fail-closed fallthrough).
new_case
echo 'x' > "$repo/foo.xyz"
commit_case
check 23 1 "unclassified path"

# 24 — doc M-only adding a | **D9** | row (4a).
new_case
echo '| **D9** | stable rust | rationale |' >> "$repo/docs/notes.md"
commit_case
check 24 1 "PRD-definition pattern touched"

# 25 — doc M-only adding a - **FR-3** line (4a).
new_case
echo '- **FR-3** [must] — scheduling.' >> "$repo/docs/notes.md"
commit_case
check 25 1 "PRD-definition pattern touched"

# 26 — new doc (A) (4b).
new_case
echo 'new doc' > "$repo/docs/new.md"
commit_case
check 26 1 "doc added/deleted"

# 27 — deleted doc (D) (4b).
new_case
rm "$repo/docs/notes.md"
commit_case
check 27 1 "doc added/deleted"

# 28 — git mv doc -> doc (rename decomposition) (4b).
new_case
( cd "$repo" && $GITC mv docs/notes.md docs/renamed.md )
commit_case
check 28 1 "doc added/deleted"

# 29 — git mv repo-doc -> .knowledge/wiki/topics/x.md (the D side is classified).
new_case
( cd "$repo" && $GITC mv docs/notes.md .knowledge/wiki/topics/x.md )
commit_case
check 29 1 "doc added/deleted"

# 30 — 25-line M-only doc edit (4c).
new_case
i=1; while [ "$i" -le 25 ]; do echo "appended line $i" >> "$repo/docs/notes.md"; i=$((i+1)); done
commit_case
check 30 1 "docs delta"

# 31 — 5-line M-only doc edit (trivial).
new_case
i=1; while [ "$i" -le 5 ]; do echo "appended line $i" >> "$repo/docs/notes.md"; i=$((i+1)); done
commit_case
check 31 0 "trivial"

# 32 — binary-content .md file, M-only (numstat '-' counted as 1000; 4c binary arm, fail-closed).
new_case
printf 'BIN\000\001\002\n' > "$repo/docs/notes.md"
commit_case
check 32 1 "docs delta 2000"

# 33 — substantive + report with only 3 added lines (10-line floor).
new_case
echo 'x' > "$repo/main.rs"
add_report "$repo" ".knowledge/wiki/runs/2026-07-23-short.md" 3
commit_case
check 33 1 "BLOCKED"

# 34 — substantive + git mv old report -> new name, zero added lines (-M keeps R excluded).
new_case
echo 'x' > "$repo/main.rs"
( cd "$repo" && $GITC mv .knowledge/wiki/runs/2026-01-01-old.md .knowledge/wiki/runs/2026-01-02-new.md )
commit_case
check 34 1 "BLOCKED"

# 35 — substantive + only a NESTED .knowledge/wiki/runs/sub/x.md report (:(glob) pin).
new_case
echo 'x' > "$repo/main.rs"
add_report "$repo" ".knowledge/wiki/runs/sub/x.md" 12
commit_case
check 35 1 "BLOCKED"

# 36 — substantive + only a BINARY-content run-report (rule-7 binary numstat exclusion).
new_case
echo 'x' > "$repo/main.rs"
mkdir -p "$repo/.knowledge/wiki/runs"
printf 'BIN\000\001\002\n' > "$repo/.knowledge/wiki/runs/2026-07-23-bin.md"
commit_case
check 36 1 "BLOCKED"

# 37 — nonexistent base-ref (cannot evaluate; fail-closed).
new_case
check 37 2 "cannot compute merge-base" "no-such-ref"

# 38 — substantive + qualifying report (>=10 added lines, top-level).
new_case
echo 'x' > "$repo/main.rs"
add_report "$repo" ".knowledge/wiki/runs/2026-07-23-full.md" 12
commit_case
check 38 0 "carries run-report"

# ---------------------------------------------------------------------------------------------------
if [ "$fail" -eq 0 ]; then
  echo "run-report-gate-selftest OK: 38 cases + the single-sourcing pin clean"
  exit 0
fi
echo "run-report-gate-selftest: FAILURES (see above)" >&2
exit 1
