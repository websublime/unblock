#!/bin/sh
# d52-attribution-discard-claims.sh — the EXECUTABLE zero-live-hits check for the D52 doc cascade
# (PRD §4 D52; spec: ci-cd-and-distribution.md §2.1). Run as a step of the required `doc-lint` job.
#
# WHY THIS EXISTS
# ---------------
# D52 keeps the optional `agent_name`/`harness`/`model` attribution on the MCP wire and publishes that
# unblock discards it until FR-22 [v1.1] records it. The published texts are contract bytes, so the
# contract suite already pins them. This check guards the OTHER direction: no tracked file may go back
# to telling a reader that the self-report is kept. Every hit must be FIXED or on an explicit
# allow-list, and every allow-list entry must still match something. There is no `expect N hits`.
#
# It sweeps ALL TRACKED FILES via `git grep`, the same as its D43 sibling.
#
# Exit: 0 = pass · 1 = BLOCK (an unallowed claim survives, or an allow-list entry rotted)
#       2 = cannot evaluate (fail-closed).
set -u

say() { echo "d52-claims: $*" >&2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { say "not a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || { say "cannot cd to the repo root"; exit 2; }

# ---------------------------------------------------------------------------------------------
# CLM-1 — the RETIRED published field description. Every hit outside the allow-list blocks.
# ---------------------------------------------------------------------------------------------
CLM1_RE='Self-reported (agent name|harness identifier|model identifier) \(capture-only\)'

# ---------------------------------------------------------------------------------------------
# CLM-2 — the RETIRED storage claim that mutations carry attribution. Every hit blocks.
# ---------------------------------------------------------------------------------------------
CLM2_RE='carry (the )?actor \+ optional Tier-1 attribution'

# ---------------------------------------------------------------------------------------------
# CLM-3 — an UNQUALIFIED recording claim. A hit is clean when the same line carries a qualifier
# that ties it to the discard or to FR-22.
# ---------------------------------------------------------------------------------------------
CLM3_RE='attribution.{0,60}(recorded|persisted|written)'
CLM3_QUALIFIER='D52|FR-22|\[v1\.1\]|discard|NULL|not bound|unbound|ub-lp9\.7'

# ---------------------------------------------------------------------------------------------
# THE ALLOW-LIST — `family|path-prefix|line-substring|reason` (an EMPTY substring = PATH-ONLY).
#
# The `unblock-model` `Event` lines describe the storage columns FR-22 will fill; that schema is not
# published over MCP. `.unblock/issues.jsonl` is the GENERATED tracker export, so it is path-only.
# ---------------------------------------------------------------------------------------------
ALLOW="
CLM1|crates/unblock-model/src/relations.rs|self-reported harness identifier (capture-only)|the model Event column doc, not the MCP wire
CLM1|crates/unblock-model/src/relations.rs|self-reported model identifier (capture-only)|the model Event column doc, not the MCP wire
CLM1|crates/unblock-model/tests/snapshots/schema_snapshots__event.snap|self-reported harness identifier (capture-only)|the model Event schema golden, not published over MCP
CLM1|crates/unblock-model/tests/snapshots/schema_snapshots__event.snap|self-reported model identifier (capture-only)|the model Event schema golden, not published over MCP
CLM3|.unblock/issues.jsonl||the GENERATED tracker export, not hand-editable prose
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

scan() { # $1 = family, $2 = match regex, $3 = qualifier regex (empty = every hit counts)
  _fam="$1"; _re="$2"; _qual="$3"
  git grep -n -I -i -E "$_re" -- . 2>/dev/null | while IFS= read -r hit; do
    _path="${hit%%:*}"
    _rest="${hit#*:}"
    _line="${_rest%%:*}"
    _text="${_rest#*:}"
    # This script's own regex literals are not claims about the product.
    case "$_path" in scripts/checks/d52-attribution-discard-claims.sh) continue ;; esac
    if [ -n "$_qual" ] && printf '%s' "$_text" | grep -q -i -E "$_qual"; then continue; fi
    if allowed "$_fam" "$_path" "$_text"; then continue; fi
    echo "$_path:$_line: retired attribution claim ($_fam)"
  done
}

findings="$(scan CLM1 "$CLM1_RE" ''; scan CLM2 "$CLM2_RE" ''; scan CLM3 "$CLM3_RE" "$CLM3_QUALIFIER")"
if [ -n "$findings" ]; then
  echo "$findings" >&2
  say "BLOCKED — the claims above survived the D52 cascade. Fix them, or add an allow-list entry with a reason."
  blocked=1
fi

# ---------------------------------------------------------------------------------------------
# SELF-TEST — every allow-list entry must still match a real line, or the exemption has rotted into
# a silent blind spot.
# ---------------------------------------------------------------------------------------------
echo "$ALLOW" | while IFS='|' read -r fam prefix substr reason; do
  [ -n "$fam" ] || continue
  case "$fam" in
    CLM1) probe="$CLM1_RE" ;;
    CLM2) probe="$CLM2_RE" ;;
    CLM3) probe="$CLM3_RE" ;;
    *) continue ;;
  esac
  matched="$(git grep -n -I -i -E "$probe" -- "$prefix" 2>/dev/null | { if [ -n "$substr" ]; then grep -F "$substr"; else cat; fi; })"
  if [ -z "$matched" ]; then
    echo "$prefix: allow-list entry for $fam matches NOTHING (reason was: $reason)" >&2
    exit 1
  fi
done || blocked=1

[ "$blocked" = "0" ] || exit 1
say "OK — no retired attribution claim survives, and every allow-list entry still matches."
exit 0
