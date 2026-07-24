---
name: project-workflow-resume-after-session-limit
description: Recover a mid-run Workflow killed by "session limit" — resume with resumeFromRunId (cached prefix replays free); read mid-run agent verdicts from the run's journal.jsonl
type: recipe
---

When a Workflow run dies because subagents hit "You've hit your session limit" (it may take 2+ resumes if the limit re-hits), do NOT re-author or re-run from scratch:

1. `Workflow({scriptPath: <persisted script>, resumeFromRunId: "wf_..."})` — every *completed* `agent()` call in the unchanged prefix returns its cached result instantly (zero tokens); execution continues at the first failed/new call. Same runId can be resumed repeatedly.
2. Results produced mid-run (e.g. a judge verdict from a gate that completed before the crash) are recoverable without waiting: read `subagents/workflows/<runId>/journal.jsonl` — events are `{type: 'started'|'result', key, agentId, result}` in call order; match `result` events to `agent()` call order (labels are not stored, order is).
3. Files written by completed agents (spec packages, reports in the scratchpad) persist across the failure — verify with `ls` before resuming, don't regenerate.

Bit us twice on T2.6 (2026-07-03): the Spec/Review workflow failed at round-1 reviews, then after resume at revise/round-2 — both times the resume replayed the finished phases from cache and only re-ran the dead tail. Related: [[project-workflow-flat-schema-for-coordinator]].
