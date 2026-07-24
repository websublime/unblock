#!/usr/bin/env python3
"""PreToolUse guard: .knowledge/memories/** — deny destructive Write overwrites (ci-cd §2.3)."""
import json, os, sys

WRITE_TOOLS = {"Write"}                               # wholesale-overwrite capability
EDIT_TOOLS = {"Edit", "MultiEdit"}

def deny(msg):
    print(f"knowledge memories write-guard: {msg}", file=sys.stderr)
    sys.exit(2)

try:
    p = json.load(sys.stdin)
except Exception as e:  # fail closed
    deny(f"unparsable hook payload ({e}); failing closed")

tool = p.get("tool_name", "")
if tool not in WRITE_TOOLS | EDIT_TOOLS:
    sys.exit(0)  # not a write-capable tool (defensive; the matcher already scopes)
ti = p.get("tool_input") or {}
path = ""
for key in ("file_path", "path", "abs_path"):  # tolerate frontends that vary the path key; the
    if ti.get(key):                            # fallbacks fall through to the fail-closed deny below
        path = str(ti[key])                    # when a matched tool presents no recognizable path.
        break
if not path:
    deny(f"tool '{tool}' presented no recognizable path field; failing closed")
path = path.replace("\\", "/")
if not os.path.isabs(path):                    # normalize cwd-aware BEFORE the marker check
    cwd = (p.get("cwd") or "").replace("\\", "/")
    if not cwd:
        deny("relative path with no cwd in the payload — cannot anchor it; failing closed")
    path = os.path.join(cwd, path)
path = os.path.normpath(path).replace("\\", "/")   # collapses '..' hops too
marker = ".knowledge/memories/"
if marker not in path:
    sys.exit(0)
rel = path.split(marker, 1)[1]
if "/" in rel:
    deny(f"nested path '{rel}' — memories/ is flat (one file per atomic memory)")
if rel == "index.md":
    sys.exit(0)  # curated index: Write/Edit allowed
if tool in WRITE_TOOLS and os.path.exists(path):
    deny(f"Write would OVERWRITE existing memory '{rel}'. Use Edit for surgical curation; "
         "retirement goes through scripts/knowledge/memory-retire.sh (sanctioned flow).")
sys.exit(0)
