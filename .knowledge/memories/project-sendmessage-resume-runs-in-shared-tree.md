---
name: project-sendmessage-resume-runs-in-shared-tree
description: Resuming a worktree-isolated Agent via SendMessage runs the turn in the SHARED tree (not a fresh worktree); a `git merge --ff-only <feature>` recipe then fast-forwards local main — recover via `git branch -f main origin/main`
type: gotcha
---

Resuming a worktree-isolated Agent (spawned with `isolation: "worktree"`) **via SendMessage** runs the resumed
turn in the **shared working tree**, NOT a fresh worktree — the original worktree was cleaned up after the agent's
first completion, and the `isolation` flag is **not re-applied** on a SendMessage resume. So any worktree-style git
recipe handed to the resumed agent operates on the shared tree's *currently checked-out branch*.

In T1.7, a fold-in resume was told to `git merge --ff-only t1.7-restore-undelete` (intended to pull the feature
branch into a worktree). It ran while the shared tree was on `main` → it **fast-forwarded local `main`** onto the
feature commits — a "never commit to main" process violation. It is **local-only** (origin/main was untouched, the
work was never pushed to main), caught by `git worktree list` showing `<sha> [main]` at the feature tip.

**Why:** a SendMessage resume ≠ a fresh `isolation: "worktree"` Agent call. `--ff-only <feature>` moves whatever
branch HEAD currently points at; on the shared tree that is often `main`.

**How to apply:**
- For committing fold-ins after a gate, prefer spawning a **fresh `isolation: "worktree"` Agent** (new task) rather
  than a SendMessage resume; or have the resumed agent use **ref-only ops** (`git update-ref`/`git branch -f <feature>`)
  that never move the current branch; or park the shared tree on a throwaway branch first.
- After ANY such resume, re-verify `git rev-parse main == origin/main`. If main drifted, recover (while NOT on main)
  with `git branch -f main origin/main` — the feature commits stay safe on the feature branch.

Related: [[project-harness-worktree-bases-off-main]], [[project-background-agent-crash-recovery]].
