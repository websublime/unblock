#!/usr/bin/env python3
"""PreToolUse gate: gh pr create requires the wiki run-report in the branch diff (ci-cd §2.3)."""
import json, re, subprocess, sys

def deny(msg):
    print(f"pr-create run-report gate: {msg}", file=sys.stderr)
    sys.exit(2)

try:
    p = json.load(sys.stdin)
except Exception as e:
    deny(f"unparsable hook payload ({e}); failing closed")
if p.get("tool_name") != "Bash":
    sys.exit(0)
cmd = (p.get("tool_input") or {}).get("command", "") or ""
is_create = re.search(r'\bgh\s+pr\s+create\b', cmd) is not None
is_api_create = (re.search(r'\bgh\s+api\b', cmd) and re.search(r'/pulls\b', cmd)
                 and re.search(r'(-X\s*POST|--method[=\s]+POST|\s-[fF]\s|--input\b|--raw-field\b)', cmd))
if not (is_create or is_api_create):
    sys.exit(0)

cwd = p.get("cwd") or "."
r = subprocess.run(["git", "-C", cwd, "rev-parse", "--show-toplevel"], capture_output=True, text=True)
if r.returncode != 0:
    deny(f"cannot resolve the repo root: {r.stderr.strip()}; failing closed")
top = r.stdout.strip()
mb = re.search(r'--base[=\s]+(\S+)', cmd)
base_ref = f"origin/{mb.group(1)}" if mb else "origin/main"
g = subprocess.run([f"{top}/scripts/knowledge/run-report-gate.sh", base_ref],
                   cwd=cwd, capture_output=True, text=True)
sys.stderr.write(g.stderr)
if g.returncode == 0:
    sys.exit(0)
deny("blocked before PR creation — see run-report-gate output above. Commit the .knowledge/wiki/runs/ "
     "run-report (and its wiki/index.md entry) on this branch (same-commit rule, PROCESS.md §8), then "
     "re-run gh pr create.")
