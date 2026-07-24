---
name: reference-workflow-agent-prompt-must-be-joined-string
description: Workflow agent() first arg MUST be a string — passing a bare array of lines (forgot .join('\n')) makes the prompt render as "[object]" and the agent silently no-ops ("no task payload")
type: gotcha
---

In a Workflow script, `agent(prompt, opts)` takes a STRING prompt. If you build the prompt as an array of
lines and forget `.join('\n')`, the array is passed as-is and the subagent receives garbage — it reports the
task "rendered as '[object]'" / "no task payload to implement against" and returns a CLEAN no-op (zero commits,
empty diff). The Verify gate then correctly FAILs on the null diff, but the root cause is the missing join, NOT
the agent type or the spec.

Symptom to recognize instantly: an Implement agent returns `commits: []`, `files_touched: []`, `diff_path: none`,
and an open_question mentioning `[object]` / "no task payload". Do NOT chase it as an agent-type or prompt-content
problem — grep the script for every `agent([` and confirm each closes with `].join('\n'),` before the opts object.
Bit the CD-3/4/5 bulk workflow TWICE (the impl agent() lacked `.join('\n')` while the verify agents had it).

Related workflow-authoring pitfalls: [[reference-workflow-no-inner-backticks-in-prompts]],
[[project-workflow-flat-schema-for-coordinator]]. See also the impl-must-commit gotcha in
[[project-mcp-conformance-confrontation-d1-d5]].
