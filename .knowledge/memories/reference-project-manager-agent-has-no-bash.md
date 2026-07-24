---
name: reference-project-manager-agent-has-no-bash
description: project-manager agent type has NO Bash — can't ff-merge/git-grep in a worktree-based Verify lens; pick a Bash-capable agent for those roles
type: gotcha
---

The `project-manager` agent type's toolset is Read/Write/Edit/Glob/Grep/WebFetch/WebSearch — **no Bash**.
Same for `Plan`, `Explore`, `research-analyst` has Bash but no Write, `qa-expert` has Bash but no Write/Edit.

**Bite (D39 T3.10 Verify, 2026-07-17):** the doc-cascade Verify lens was given to `project-manager` with an
`isolation:'worktree'` + a `git merge --ff-only <SHA>` instruction issued through a command-rewriting shell hook.
It could not run git at all, so it fell back to reviewing the scratchpad DRAFTS instead of the committed artifact,
and raised BLOCKER-class findings (PRD SEED/two-tier precedence, README v1.4 regression) that were all
FALSE-in-commit (already fixed). The coordinator recovered by reading the commit directly — but the lens was wasted.

**Rule:** any Verify/review lens that must ff-merge/cherry-pick, `git grep`, or run a probe in an isolated
worktree needs a **Bash-capable writer** — use `rust-engineer`, `cli-developer`, `mcp-developer`, or
`code-reviewer` (all have Bash+Write/Edit). Reserve `project-manager` for read-from-scratchpad doc/PM synthesis,
or feed it the artifact via files rather than a worktree it can't materialize. Match the agent's toolset to what
the prompt actually requires BEFORE spawning. Relates to [[reference-workflow-plan-explore-agents-have-no-write]].
