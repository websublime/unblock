---
name: feedback-concurrent-session-shares-the-working-tree
description: A second concurrent coding session can own the shared working tree — a foreign branch or dirty files there may belong to that other session, not a leak from your own agent, and must never be reset
type: gotcha
---

Multiple concurrent coding sessions can run against the same repository checkout, sharing ONE working tree. When that happens, another session's `git checkout` and uncommitted edits can appear in the current session's `git status` mid-task.

Observed case: a session left the shared tree on `main`; on return it was on a different feature branch with several dirty docs. The first instinct — "a review agent mutated the shared tree" (the failure mode [[feedback-verify-reviewers-mutate-shared-tree]] and [[feedback-agent-tool-needs-explicit-worktree-isolation]] warn about) — was **wrong**. It was a second, concurrently-running session doing unrelated work a few minutes earlier. A reflex `reset --hard` / `checkout main` would have destroyed real in-flight work.

**Why:** the shared tree has no single owner. Both the "my agent leaked" and the "another session owns this" hypotheses produce identical symptoms, and only one of them makes destruction safe.

**How to apply:**
1. **Diagnose before touching.** `git reflog -12` distinguishes them: local checkouts appear in order; a foreign one appears *after* the last local op and names a branch nothing in the local session's history mentions. Cross-check `ls -lT` mtimes against `date` — minutes-old = live, not stale.
2. **Worktree ownership is checkable.** Workflow worktrees live under `.claude/worktrees/wf_<runId>-N`, where `<runId>` matches a Workflow actually launched from the current session. A `wf_*` directory that doesn't match anything launched locally is **not this session's** — leave it.
3. **Never reset/clean/stash the shared tree** to "recover". Verify `main == origin/main` (that's what actually matters) and do mutating work entirely in worktrees.
4. **Escalate rather than resolve unilaterally** — a concurrent GA-blocking task needs human sequencing, and doc-corpus edits across sessions will textually conflict (the usual overlap is the tracker export / PRD / spine / impl-plan files).
