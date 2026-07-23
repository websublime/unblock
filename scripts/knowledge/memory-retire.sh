#!/bin/sh
# memory-retire.sh <slug> — the ONLY sanctioned shell removal of a memory (ci-cd §2.3).
# Ordered so NO partial destructive state survives a mid-script failure.
# Exit: 3 usage · 4 no-such-memory / broken tree · 5 invalid slug (grammar, or the reserved 'index').
set -eu
[ $# -eq 1 ] || { echo "usage: memory-retire.sh <slug>" >&2; exit 3; }
slug="$1"
# The §2.3.1 slug grammar re-validated HERE; 'index' is the curated index, not a memory — explicit exclusion.
case "$slug" in
  index) echo "memory-retire: 'index' is the curated index, not a memory — refusing" >&2; exit 5 ;;
  "" | -* | *[!a-z0-9-]*) echo "memory-retire: invalid slug '$slug' (grammar: [a-z0-9][a-z0-9-]*)" >&2; exit 5 ;;
esac
top="$(git rev-parse --show-toplevel)"
f="$top/.knowledge/memories/$slug.md"
idx="$top/.knowledge/memories/index.md"
[ -f "$f" ] || { echo "memory-retire: no such memory '$slug'" >&2; exit 4; }
[ -f "$idx" ] || { echo "memory-retire: memories/index.md missing — tree broken; fix it first" >&2; exit 4; }
tmp="$idx.retire-tmp"
# Pure computation first: build the de-indexed content OFF to the side; a failure here leaves the
# tree untouched.
python3 - "$idx" "$slug.md" "$tmp" <<'PY'
import sys
idx, target, tmp = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(idx, encoding="utf-8").read().splitlines(keepends=True)
kept = [l for l in lines if f"({target})" not in l]
if len(kept) == len(lines):
    sys.stderr.write(f"memory-retire: WARNING no index entry referenced {target}\n")
open(tmp, "w", encoding="utf-8").writelines(kept)
PY
# Mutations last; the destructive step is FINAL.
mv "$tmp" "$idx"
git -C "$top" rm --quiet -- ".knowledge/memories/$slug.md"
git -C "$top" add -- ".knowledge/memories/index.md"
echo "memory-retire: '$slug' removed and de-indexed (staged, not committed — commit via the Track flow)."
