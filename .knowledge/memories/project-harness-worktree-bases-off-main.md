---
name: project-harness-worktree-bases-off-main
description: Agent worktree isolation bases the worktree off main, NOT the current feature branch — spawned implementers miss uncommitted/feature-branch work unless they merge-ff it first
type: gotcha
---

When you spawn an agent with `isolation: "worktree"` (Agent tool or Workflow `agent({isolation:'worktree'})`), the harness creates the worktree based on the tip of **`main`**, NOT the current/feature branch you (the orchestrator) are on. Confirmed by probe: orchestrator on `t1.1-policy` (HEAD `fb25ae0`), but the spawned worktree HEAD was `e59d217` (= `main`), on an auto-named branch `worktree-agent-<id>`, and the feature-branch commits were absent.

**Consequence:** an implementer in a worktree will build against STALE docs/code (e.g. an un-reconciled spec) and silently produce wrong work. Miguel caught this in the T1.1 session.

**How to apply:** when the feature branch carries commits the agent needs (e.g. a spec-first reconciliation), make Step 0 of the agent's task: `git merge --ff-only <feature-branch>` inside its worktree, then a guard (`grep` for a known marker) that STOPS if the merge didn't bring the expected state. Integrate the agent's result back via temp-commit-in-worktree + `git cherry-pick --no-commit <sha>` onto the feature branch (the object store is shared). A changed worktree is NOT auto-cleaned, so its commits persist for cherry-pick; clean up afterward with `git worktree remove --force`. See [[project-subagents-cargo-sandbox-denied]] for why the orchestrator still has to run the verify/fix loop itself.
