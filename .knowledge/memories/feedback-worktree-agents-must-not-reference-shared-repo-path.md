---
name: feedback-worktree-agents-must-not-reference-shared-repo-path
description: Worktree-isolated agents told to `git -C <shared-repo>` or edit via shared-tree absolute paths leak staged changes into main's index; anchor them to their own worktree + verify main clean between phases
type: gotcha
---

When spawning a worktree-isolated agent (Agent `isolation: "worktree"` or a Workflow `agent()` with worktree isolation), do NOT instruct it to use `git -C <shared-repo-root> …` or to Edit/Write via absolute paths under the **shared** repo root. Those target the shared `main` working tree, not the agent's isolated worktree — the agent then stages its edits into **main's index** (observed: a T1.8 spec amend left 11 files `git add`-staged on main while HEAD stayed `0547671`).

**Why:** the harness gives the agent its own worktree as CWD, but absolute `${REPO}/…` paths and `git -C ${REPO}` bypass it and hit the shared checkout.

**How to apply:**
- In the agent prompt, make it anchor to ITS worktree: `WT=$(git rev-parse --show-toplevel)` (must be under `.claude/worktrees/`); run all `git` from CWD (no `-C` to a foreign path); Edit/Write only `$WT/…` paths; explicitly forbid touching the shared repo root directly. (The standalone fold/impl agents that followed this did NOT leak; the one using `git -C ${REPO}` did.)
- As orchestrator, after EVERY worktree workflow, verify the shared tree: `git status -s` on main should be 0 dirty. If contaminated, confirm the work is safe on the branch (`git diff --cached --name-only` each MATCHes `git diff <branch> -- <file>`), then `git reset --hard HEAD` on main (the branch ref keeps the commits).
- The committed branch ref always survives worktree removal — `git worktree remove --force` only deletes the working dir, never the branch/commits. Free a worktree (so the next phase can `git switch` the branch) and re-verify main == `origin/main` after.

Relates to [[project-sendmessage-resume-runs-in-shared-tree]], [[feedback-verify-reviewers-mutate-shared-tree]], [[project-harness-worktree-bases-off-main]].
