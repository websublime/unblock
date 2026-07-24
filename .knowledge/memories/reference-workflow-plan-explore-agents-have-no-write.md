---
name: reference-workflow-plan-explore-agents-have-no-write
description: In a Workflow, the Plan and Explore agent types have NO Write/Edit tool — a "write findings to file X" instruction silently no-ops; recover the output from the agent's journal jsonl
type: gotcha
---

Workflow agents spawned with `agentType: 'Plan'` or `agentType: 'Explore'` are **read-only** (their tool set excludes Write/Edit/NotebookEdit). If the workflow prompt tells such an agent to "write your findings to `<scratchpad>/file.md`", the file is **never created** — the findings live ONLY in the agent's returned final message. The coordinator that reads sibling files by path will silently miss it (as happened in the T3.5 Understand run `wf_f96b6119-e8d`: `05-architecture.md` was never written).

**Recover** the Plan/Explore agent's output by extracting the last assistant text block from its transcript: `subagents/workflows/<runId>/agent-<id>.jsonl` (find the id via the `agent-<id>.meta.json` whose `agentType` is `Plan`/`Explore`). A short python loop over the jsonl pulling `message.content[].text` for `role=="assistant"` works.

**Avoid it:** for any reader role that must WRITE a file, use `rust-engineer` / `research-analyst` / `mcp-developer` / `code-reviewer` (these have Write). Reserve `Plan`/`Explore` for return-text-only lanes, and have the coordinator read their returned summaries (passed via the workflow's captured return values), not a file. Relates to [[project-workflow-flat-schema-for-coordinator]].
