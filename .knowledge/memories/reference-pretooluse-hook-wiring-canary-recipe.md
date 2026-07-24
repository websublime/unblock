---
name: reference-pretooluse-hook-wiring-canary-recipe
description: How to live-fire the .knowledge PreToolUse hook-wiring canaries safely (deny-proves-wiring; a pass alone is indistinguishable from no-hook)
type: recipe
---

To confirm a Claude Code PreToolUse hook is actually WIRED (fires + its exit-2 blocks the tool) from a live session, exercise a **deny** — a pass alone is indistinguishable from an unwired hook (the tool would run either way). Safe zero-footprint canaries used for the unblock `.knowledge` hooks (task .6 of the .knowledge layer epic, unblock issue ub-knowledge-layer-e4s; 2026-07-24):

- **write-guard** (`knowledge-memories-write-guard.py`): `Write` to `.knowledge/memories/<sub>/x.md` → denied "nested path — memories/ is flat". No file created; needs no pre-existing file (unlike the overwrite branch, which needs an existing non-index memory).
- **bash-guard** (`knowledge-memories-bash-guard.py`): `rm -rf .knowledge/__nonexistent__` → denied (recursive-rm target inside .knowledge). Target nonexistent = safe even if the hook were broken.
- **pr-create gate** (`pr-create-run-report-gate.py`): `gh pr create --base <nonexistent-branch> …` → gate fails closed (merge-base fails) → denied before gh runs (unambiguous wiring proof); then `gh pr create --help` on main (empty diff) → gate passes → help prints, no block = the deny-then-pass pair. `--help` never creates a PR.

Gotcha: `mcp__acp__Write`/`Edit` (ACP = Agent Client Protocol, used by the desktop app / IDE bridge frontends, NOT the terminal CLI) are NOT surfaced in the terminal CLI, so an acp-targeted canary can't be live-fired there — only `Write`/`Edit` are. Branch protection on `main` is a GitHub **ruleset** here (classic `branches/main/protection` 404s); verify required checks via `gh api repos/<o>/<r>/rules/branches/main`.
