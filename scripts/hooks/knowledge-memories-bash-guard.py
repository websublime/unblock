#!/usr/bin/env python3
"""PreToolUse guard: deny destructive Bash on .knowledge/** + pathless tree-destroyers (ci-cd §2.3)."""
import json, os, re, shlex, sys

def deny(msg):
    print(f"knowledge memories bash-guard: {msg}", file=sys.stderr)
    sys.exit(2)

try:
    p = json.load(sys.stdin)
except Exception as e:
    deny(f"unparsable hook payload ({e}); failing closed")
if p.get("tool_name") != "Bash":
    sys.exit(0)
cmd = (p.get("tool_input") or {}).get("command", "") or ""
cwd = p.get("cwd") or os.getcwd()

# 1. Sanctioned flow FIRST — reachable by construction: one un-chained retire call; arg
#    constrained to the §2.3.1 slug grammar (the script re-validates and rejects 'index').
SANCTIONED = re.compile(
    r'^\s*(?:"?\$CLAUDE_PROJECT_DIR"?/|\./)?scripts/knowledge/memory-retire\.sh\s+[a-z0-9][a-z0-9-]*\s*$')
if SANCTIONED.match(cmd):
    sys.exit(0)

def repo_root(start):
    d = os.path.abspath(start)
    while True:
        if os.path.isdir(os.path.join(d, ".git")) or os.path.isfile(os.path.join(d, ".git")):
            return d
        parent = os.path.dirname(d)
        if parent == d:
            return None
        d = parent

# 2. Trigger B — pathless destructive shapes: deny even with no .knowledge mention.
if re.search(r'\bgit\s+clean\b', cmd):
    deny("git clean removes untracked files — uncommitted memories have no other protective layer. "
         "Inspect with 'git status'; delete specific non-knowledge paths explicitly instead.")

def recursive_rm_targets(command):
    try:
        toks = shlex.split(command, posix=True)
    except ValueError:
        deny("unparsable shell quoting; failing closed")
    seps = {"&&", "||", ";", "|", "&"}
    out, i = [], 0
    while i < len(toks):
        if toks[i].rsplit("/", 1)[-1] == "rm":
            recursive, args, j = False, [], i + 1
            while j < len(toks) and toks[j] not in seps:
                t = toks[j]
                if t.startswith("-"):
                    if "r" in t.lower():
                        recursive = True
                else:
                    args.append(t)
                j += 1
            if recursive:
                out.extend(args or ["."])
            i = j
        else:
            i += 1
    return out

for t in recursive_rm_targets(cmd):
    if t.strip() in ("/", "~"):
        deny(f"recursive rm of '{t}' — wholesale destruction shape")
    base = t.rstrip("*") or "."          # 'rm -rf *' → the containing dir decides
    tgt = os.path.normpath(base if os.path.isabs(base) else os.path.join(cwd, base))
    root = repo_root(cwd)
    if root:
        kn = os.path.join(root, ".knowledge")
        if (tgt == root or root.startswith(tgt.rstrip("/") + "/")
                or tgt == kn or tgt.startswith(kn + "/")):
            deny(f"recursive rm target '{t}' resolves to '{tgt}' — at/above the repo root or inside "
                 ".knowledge; uncommitted memories would be unrecoverable (no other layer exists)")

# 3. Trigger A — the .knowledge prefix scan (broadened from '.knowledge/memories').
if ".knowledge" not in cmd:
    sys.exit(0)

HARD = r'\b(rm|unlink|rmdir|mv|cp|dd|tee|shred|truncate|install|ln|touch|chmod|chown|rsync|xargs|eval)\b'
COND = [
    (r'\bsed\b[^|;&]*\s-\S*i', "sed -i (in-place edit)"),
    (r'\bperl\b[^|;&]*\s-\S*i', "perl -i (in-place edit)"),
    (r'\bfind\b.*(-delete\b|-exec\b)', "find -delete/-exec"),
    (r'\bgit\s+(rm|clean|checkout|restore|reset|filter-branch|filter-repo|stash)\b', "destructive git verb"),
    (r'\b(python[0-9.]*|node|ruby)\b', "interpreter with a .knowledge path in scope"),
    (r'\b(sh|bash|zsh)\s+-c\b', "shell -c with a .knowledge path in scope"),
    (r'>{1,2}\s*"?\S*\.knowledge', "redirection into .knowledge/"),
]
m = re.search(HARD, cmd)
reason = f"mutating verb '{m.group(1)}'" if m else next((why for rx, why in COND if re.search(rx, cmd)), None)
if reason:
    deny(f"{reason} with '.knowledge' in the command. The knowledge tree is append/curate-only from the "
         "shell: create pages via Write (new file), curate via Edit, retire a memory ONLY via "
         "scripts/knowledge/memory-retire.sh <slug> (keeps the index consistent; git is the archive). "
         "For pure reads, split the command so no mutating verb shares it with the .knowledge path.")
sys.exit(0)
