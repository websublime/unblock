#!/bin/sh
# run-report-gate.sh [<base-ref>] — the STRUCTURAL substantive-PR predicate + run-report requirement
# (decision 7(ii); spec: ci-cd-and-distribution.md §2.3). SINGLE SOURCE OF TRUTH: called by the CI job
# `run-report-gate` AND by the pr-create PreToolUse hook. Default base-ref: origin/main.
# Exit: 0 = pass · 1 = BLOCK (substantive diff, no qualifying run-report) · 2 = cannot evaluate (fail-closed).
set -u
DOCS_LINE_BUDGET=20
RUNS_MIN_ADDED=10
# Session-local id pattern (rule 1a) — single-sourced with ci-cd §2.3 and the k6 token-coverage
# const; the §2.3.3 selftest pins this literal against the spec.
SESSION_LOCAL_ID_RE='(^|[^A-Za-z0-9-])(MF|CF|M|R|F|A)-?[0-9]+([^0-9]|$)'
base_ref="${1:-origin/main}"
say() { echo "run-report-gate: $*" >&2; }

base="$(git merge-base "$base_ref" HEAD 2>/dev/null)" \
  || { say "cannot compute merge-base($base_ref, HEAD)"; exit 2; }
# Classification listing is --no-renames: a rename decomposes to D+A, each classified
# fail-closed — with -M, `git mv <repo-doc> .knowledge/...` would list only the neutral NEW path.
changed="$(git diff --name-only --diff-filter=ACDM --no-renames "$base" HEAD 2>/dev/null)" \
  || { say "git diff failed"; exit 2; }
[ -n "$changed" ] || exit 0

substantive=0

# Rule 1a — comment-coining export trigger: an ADDED issues.jsonl line whose "comments" carry a
# session-local-pattern token absent from the paired removed record (same top-level id) is SUBSTANTIVE.
coined="$(git diff -U0 --no-renames "$base" HEAD -- .unblock/issues.jsonl 2>/dev/null \
  | awk -v re="$SESSION_LOCAL_ID_RE" '
      /^[+-]/ && !/^(\+\+\+|---)/ {
        side = substr($0, 1, 1); line = substr($0, 2)
        if (line !~ /"comments"/) next
        id = ""
        if (match(line, /"id":"[^"]*"/)) id = substr(line, RSTART, RLENGTH)  # pairing key only
        rest = line
        while (match(rest, re)) {
          tok = substr(rest, RSTART, RLENGTH)
          gsub(/^[^A-Za-z0-9]+/, "", tok); gsub(/[^0-9]+$/, "", tok)
          if (tok != "") { seen[side, id, tok] = 1; toks[id SUBSEP tok] = 1 }
          rest = substr(rest, RSTART + (RLENGTH ? RLENGTH : 1))
        }
      }
      END { for (k in toks) { split(k, a, SUBSEP)
              if (seen["+", a[1], a[2]] && !seen["-", a[1], a[2]]) { print a[2]; exit } } }')"
if [ -n "$coined" ]; then
  say "issues.jsonl adds a comment coining session-local id '$coined' — substantive (rule 1a)"
  substantive=1
fi

# Rule 1 — neutral strip (a path class never makes a diff substantive; rule 1a above can).
N="$(printf '%s\n' "$changed" | grep -Ev '^(\.knowledge/|\.unblock/issues\.jsonl$)' || true)"
if [ "$substantive" -eq 0 ]; then
  [ -n "$N" ] || { say "only neutral paths changed — pass"; exit 0; }

  # Rule 2 — pure dependency-bump shape: every remaining path is a Cargo manifest / lockfile.
  if ! printf '%s\n' "$N" | grep -Evq '(^|/)Cargo\.(toml|lock)$'; then
    say "pure manifest diff — pass"; exit 0
  fi

  docs=""
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    case "$f" in
      # Rule 3 — always-substantive classes (order matters: crates/** wins over *.md for crate READMEs).
      crates/*|xtask/*|fuzz/*|scripts/*|migrations/*|.github/*|.claude/*|.mcp.json|*.rs|*.sql|*.sh|*.py|rust-toolchain|rust-toolchain.toml)
        say "always-substantive path: $f"; substantive=1; break ;;
      Cargo.toml|*/Cargo.toml|Cargo.lock)
        say "manifest mixed with non-manifest changes: $f"; substantive=1; break ;;
      *.toml)
        say "non-manifest toml: $f"; substantive=1; break ;;
      *.md|LICENSE*)
        docs="${docs}${f}
" ;;
      *)
        say "unclassified path (fail-closed): $f"; substantive=1; break ;;
    esac
  done <<EOF
$N
EOF

  if [ "$substantive" -eq 0 ] && [ -n "$docs" ]; then
    # Rule 4a — a contract-definition line (PRD §4 D-row, FR/NFR def) at any size is substantive.
    if printf '%s' "$docs" | tr '\n' '\0' | xargs -0 git diff -U0 --no-renames "$base" HEAD -- \
         | grep -E '^[+-]' | grep -Ev '^(\+\+\+|---)' \
         | grep -Eq '\|[[:space:]]*\*\*D[0-9]+\*\*[[:space:]]*\||^[+-][[:space:]]*-[[:space:]]*\*\*(FR|NFR)-[0-9]+'; then
      say "PRD-definition pattern touched"; substantive=1
    # Rule 4b — any Added/Deleted doc is substantive (a new/removed artifact; a rename already
    # decomposed to D+A under --no-renames, so a moved doc lands here too).
    elif printf '%s' "$docs" | tr '\n' '\0' \
         | xargs -0 git diff --name-status --diff-filter=ACD --no-renames "$base" HEAD -- | grep -q .; then
      say "doc added/deleted (incl. a rename's decomposed sides)"; substantive=1
    else
      # Rule 4c — line budget (binary '-' counts as over-budget: fail-closed).
      total="$(printf '%s' "$docs" | tr '\n' '\0' | xargs -0 git diff --numstat --no-renames "$base" HEAD -- \
               | awk '{a=($1=="-")?1000:$1; d=($2=="-")?1000:$2; s+=a+d} END {print s+0}')"
      if [ "$total" -ge "$DOCS_LINE_BUDGET" ]; then
        say "docs delta $total >= $DOCS_LINE_BUDGET lines"; substantive=1
      else
        say "docs delta $total < $DOCS_LINE_BUDGET lines, M-only, no definition patterns — trivial"
      fi
    fi
  fi
fi

[ "$substantive" -eq 1 ] || exit 0

# Requirement — >=1 wiki run-report (A|M) with >= RUNS_MIN_ADDED added lines in this diff.
# -M is DELIBERATE here (harder): a content-free `git mv` of an old report surfaces as R — excluded.
# :(glob) is DELIBERATE: it restricts * to one path component, so a nested stray cannot qualify.
ok="$(git diff --numstat -M --diff-filter=AM "$base" HEAD -- ':(glob).knowledge/wiki/runs/*.md' \
      | awk -v m="$RUNS_MIN_ADDED" '$1 != "-" && $1+0 >= m {print $3; exit}')"
if [ -n "$ok" ]; then say "substantive diff carries run-report '$ok' — pass"; exit 0; fi
cat >&2 <<'MSG'
run-report-gate: BLOCKED — this diff is SUBSTANTIVE (rationale above) but adds no wiki run-report.
Write .knowledge/wiki/runs/<YYYY-MM-DD>-<slug>.md (template: docs/plans/templates/run-report.md; the
'## Glossary' is mandatory), index it in .knowledge/wiki/index.md under '## Runs', and commit it on this
same branch (same-commit rule, PROCESS.md §8). Trivial classes never see this gate.
MSG
exit 1
