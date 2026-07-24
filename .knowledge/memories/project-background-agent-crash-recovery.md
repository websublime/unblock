---
name: project-background-agent-crash-recovery
description: A background implementer Agent loses in-process state if the Claude Code process exits, BUT commits it made to its worktree branch persist and are recoverable — don't assume the task is lost
type: recipe
---

When a `run_in_background` Agent (or any spawned implementer) is interrupted because the Claude Code process exited, the harness reports `status: failed` / "in-process state was lost." **Do NOT assume nothing landed** — the notification itself says to check the worktree.

**Recovery recipe (proven on T1.4, PR #380):**
1. `git worktree list` + `git branch | grep <feature>` — the agent's worktree + branch usually survive with whatever it committed.
2. `git -C <worktree> log --oneline` to see committed work; `git -C <worktree> status --short` + `git -C <worktree> diff` for UNCOMMITTED in-progress work in the worktree dir.
3. Validate any uncommitted partial work yourself (`cargo build/test` from the main session — sandbox usually allows it here). If it compiles + passes, **commit it to preserve it** (recovery, not hand-authoring) rather than redoing it.
4. `git worktree remove --force <dir>` to free the branch (the branch persists in `.git` after worktree removal).
5. Relaunch a **continuation** implementer in a fresh isolated worktree; its STEP 0 is `git merge --ff-only <feature-branch>` to pick up the preserved commits (worktrees base off `main`, so this is required — see [[project-harness-worktree-bases-off-main]]), then it finishes the remaining scope and does `git branch -f <feature-branch> HEAD` to re-consolidate.

**Hardening:** tell background/long implementers to **commit incrementally** (per logical chunk) so a crash leaves recoverable commits, not just an in-memory diff. A fresh `Agent` call starts clean (no prior context) — SendMessage won't revive a dead agent.

See [[project-subagents-cargo-sandbox-denied]] for the cargo-in-worktree caveat.
