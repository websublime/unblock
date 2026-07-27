#!/bin/sh
# d43-argument-boundary-claims.sh — the EXECUTABLE zero-live-hits check for the D43 doc cascade
# (PRD §4 D43; spec: ci-cd-and-distribution.md §2.1). Run as a step of the required `doc-lint` job.
#
# WHY THIS EXISTS
# ---------------
# A D-id cascade is "done" only when a grep for the old framing returns zero live hits, and this repo
# has learned twice over that a derived COUNT written into prose rots silently. So the done-condition
# is a predicate, not a number: every hit must be either FIXED or on an explicit allow-list, and every
# allow-list entry must still match something (a rotted entry that matches nothing is itself a
# failure). There is no `expect N hits` anywhere below.
#
# It sweeps ALL TRACKED FILES via `git grep`, deliberately — dotfiles, workflows and config are
# exactly where a rename/cascade sweep over `*.rs` misses the last live hit.
#
# Exit: 0 = pass · 1 = BLOCK (an unallowed claim survives, or an allow-list entry rotted)
#       2 = cannot evaluate (fail-closed).
set -u

say() { echo "d43-claims: $*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# ---------------------------------------------------------------------------------------------
# CLM-1 — an UNQUALIFIED argument-boundary claim.
#
# The bare phrase "strictly deserialized" WITHOUT a duplicate-key qualifier on the same line. Before
# D43 that phrase was the load-bearing overclaim: the boundary published a guarantee it did not have,
# because `deny_unknown_fields` operates on the ALREADY-PARSED object and a duplicated key is
# collapsed while that object is built.
#
# Case-INSENSITIVE on purpose: the case-sensitive exact-phrase families this replaced missed
# `docs/PRD.md`'s actual live wording ("quota-checked and then strictly deserialized ... at the L7
# boundary") entirely, i.e. they passed vacuously on the very line they existed to catch.
# ---------------------------------------------------------------------------------------------
CLM1_RE='strictly deserialized'
CLM1_QUALIFIER='duplicate-key|duplicate key|duplicate JSON key|D43'

# ---------------------------------------------------------------------------------------------
# CLM-2 — STALE RESIDUAL FRAMING: a line that still says the duplicate-key class is open.
# ---------------------------------------------------------------------------------------------
CLM2_RE='NOT rejected|not closed|ub-lp9\.21'
CLM2_SUBJECT='DUPLICATE JSON KEYS|duplicate JSON key'

# ---------------------------------------------------------------------------------------------
# THE ALLOW-LIST — `family|path-prefix|line-substring` (an EMPTY substring = PATH-ONLY).
#
# Path-only is deliberate for `.unblock/issues.jsonl`: it is the GENERATED tracker export, re-written
# wholesale on every `sync export`, so keying it on line text would break on a routine re-export.
# Everything else is keyed on the line's own text, so a real reword is noticed.
# ---------------------------------------------------------------------------------------------
ALLOW="
CLM1|.unblock/issues.jsonl||the GENERATED tracker export, not hand-editable prose
CLM1|docs/plans/implementation-plan.md|T2.3|a HISTORICAL record of what T2.3 shipped; deliberately not reworded
CLM2|.unblock/issues.jsonl||the GENERATED tracker export, not hand-editable prose
"

allowed() { # $1 = family, $2 = path, $3 = line text -> 0 if allowed
  _f="$1"; _p="$2"; _t="$3"
  echo "$ALLOW" | while IFS='|' read -r fam prefix substr _reason; do
    [ -n "$fam" ] || continue
    [ "$fam" = "$_f" ] || continue
    case "$_p" in "$prefix"*) ;; *) continue ;; esac
    if [ -z "$substr" ]; then exit 9; fi
    case "$_t" in *"$substr"*) exit 9 ;; esac
  done
  [ "$?" = "9" ]
}

blocked=0

scan() { # $1 = family, $2 = match regex, $3 = second regex, $4 = mode (qualifier|subject)
  _fam="$1"; _re="$2"; _second="$3"; _mode="$4"
  git grep -n -I -i -E "$_re" -- . 2>/dev/null | while IFS= read -r hit; do
    _path="${hit%%:*}"
    _rest="${hit#*:}"
    _line="${_rest%%:*}"
    _text="${_rest#*:}"
    # This script's own regex literals are not claims about the product.
    case "$_path" in scripts/checks/d43-argument-boundary-claims.sh) continue ;; esac
    if [ "$_mode" = "qualifier" ]; then
      # A hit is CLEAN when the same line also carries the duplicate-key qualifier.
      if printf '%s' "$_text" | grep -q -i -E "$_second"; then continue; fi
    else
      # A hit only counts when the same line is ABOUT duplicate keys.
      if ! printf '%s' "$_text" | grep -q -i -E "$_second"; then continue; fi
    fi
    if allowed "$_fam" "$_path" "$_text"; then continue; fi
    echo "$_path:$_line: unqualified argument-boundary claim ($_fam)"
  done
}

findings="$(scan CLM1 "$CLM1_RE" "$CLM1_QUALIFIER" qualifier; scan CLM2 "$CLM2_RE" "$CLM2_SUBJECT" subject)"
if [ -n "$findings" ]; then
  echo "$findings" >&2
  say "BLOCKED — the claims above survived the D43 cascade. Qualify them, or add an allow-list entry with a reason."
  blocked=1
fi

# ---------------------------------------------------------------------------------------------
# SELF-TEST — the thing that stops this check decaying into a vacuous pass.
#
# Every allow-list entry must still match a real line. An entry that matches nothing means the line it
# excused was reworded or deleted, and the exemption is now silently widening the check's blind spot.
# ---------------------------------------------------------------------------------------------
echo "$ALLOW" | while IFS='|' read -r fam prefix substr reason; do
  [ -n "$fam" ] || continue
  case "$fam" in
    CLM1) probe="$CLM1_RE" ;;
    CLM2) probe="$CLM2_RE" ;;
    *) continue ;;
  esac
  matched="$(git grep -n -I -i -E "$probe" -- "$prefix" 2>/dev/null | { if [ -n "$substr" ]; then grep -F "$substr"; else cat; fi; })"
  if [ -z "$matched" ]; then
    echo "$prefix: allow-list entry for $fam matches NOTHING (reason was:$reason)" >&2
    exit 1
  fi
done || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — no unqualified argument-boundary claim survives, and every allow-list entry still matches."
exit 0
