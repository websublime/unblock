---
name: feedback-agent-tool-needs-explicit-worktree-isolation
description: An Agent-tool call for a file-mutating task MUST set isolation:'worktree'; omitting it runs in the SHARED tree and a git ff-merge+commit there advances local main (leak) — recovery recipe inside
type: gotcha
---

When spawning a **file-mutating** agent via the **Agent tool** (not Workflow), you MUST pass `isolation: "worktree"` explicitly. The Agent tool does NOT isolate by default — omitting it runs the agent in the **shared working tree**. If that agent then does `git checkout <feature>` or `git merge --ff-only <feature>` and commits (a common "get onto the spec branch first" recipe), it **advances the local `main` ref** because the shared tree is checked out on `main` — the exact leak class in [[feedback-worktree-agents-must-not-reference-shared-repo-path]] / [[project-sendmessage-resume-runs-in-shared-tree]].

Hit this in the T2.7 session: a spec-amend `Agent` call (run_in_background, no isolation) ran in the shared tree, ff-merged the feature branch, and left `main` 2 commits ahead of `origin/main`.

**Why:** the harness worktree bases off main ([[project-harness-worktree-bases-off-main]]); a properly-isolated worktree agent commits on a detached/temp HEAD (or the branch it checks out), never touching the shared `main`. Workflow `agent(..., {isolation:'worktree'})` and Agent-tool `isolation:'worktree'` both do this correctly.

**How to apply:** for ANY spawned agent that writes files, set `isolation:'worktree'` (Agent tool) or `{isolation:'worktree'}` (Workflow). Only omit it for read-only reviewers.

**Recovery if main already moved (commits are safe on the objects; only refs are misplaced):**
1. `git worktree remove` any stale worktree holding the feature branch (frees the ref).
2. `git branch -f <feature> <newHEAD>` — capture the agent's commit on the feature branch (it's a ff descendant of the spec commits).
3. `git checkout main && git reset --hard origin/main` (shared tree is clean → safe).
4. Verify `git rev-parse main == git rev-parse origin/main` (no leak) and `<feature>` has the full lineage.
