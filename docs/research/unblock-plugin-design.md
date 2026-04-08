# Unblock Plugin — Design Document

**Structured development pipeline for AI agents, powered by ://unblock MCP.**

| | |
|---|---|
| **Version** | 0.1.0-design |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Status** | Design — pre-implementation |
| **Date** | April 2026 |

---

## Table of Contents

1. [Overview](#1-overview)
2. [Modes of Use](#2-modes-of-use)
3. [Core Principles](#3-core-principles)
4. [Skills (Commands)](#4-skills-commands)
5. [Comment Templates](#5-comment-templates)
6. [Labels](#6-labels)
7. [Hooks](#7-hooks)
8. [Agents](#8-agents)
9. [Internal Skills](#9-internal-skills)
10. [MCP Backlog](#10-mcp-backlog)
11. [File Structure](#11-file-structure)
12. [Plugin Distribution](#12-plugin-distribution)

---

## 1. Overview

The Unblock Plugin turns Claude Code and Copilot CLI into a structured development pipeline where the developer stays in control. Every task follows the same flow:

```
Think → Plan → Investigate → Implement (self-check) → Review → QA → Ship
```

The backend is the Unblock MCP server — GitHub Issues and Projects V2 with dependency-aware graph computation. Every command that touches task state goes through Unblock MCP tools. The issue is the sole medium of communication between agents and sessions.

**What makes this different from general-purpose plugins:**

- GitHub is the only source of truth — zero local state
- Discipline is enforced architecturally, not by agent instruction alone
- Sessions are sacred — planning never contaminates execution, review never contaminates implementation
- The issue is the contract — every agent reads it fresh from GitHub, every agent writes structured comments back to it

---

## 2. Modes of Use

### 2.1 Solo Developer (Local MCP)

Developer runs MCP server locally. All commands run on the developer's machine. The plugin orchestrates locally against GitHub.

Typical usage: `/do`, `/plan`, `/think`, `/use`, `/info`, `/trail`, `/ship`.

### 2.2 Teams / Enterprise (Remote MCP)

MCP server runs remotely (centralised). Multiple developers and agents access the same MCP. Two autonomous analysis flows run as services:

**Investigation flow** — triggered via `/make`, typically by CI. An agent analyses the codebase and issue context, writes a structured `INVESTIGATION` comment to the issue.

**Review flow** — triggered via `/make`, typically by CI. An agent analyses the implemented work, writes a structured `REVIEW` comment, applies label `unblock:review:ok` or `unblock:review:rework`.

In both cases the issue is the medium — the comment is the deliverable, readable by any subsequent agent or developer regardless of editor or session.

---

## 3. Core Principles

**GitHub is the source of truth.** All state lives in GitHub Issues, Projects V2, labels, and comments. Zero local database.

**Sessions are sacred.** Planning sessions must not contaminate execution sessions. Investigation must run in an isolated session from implementation. Review must never be done in the same session as implementation. Violation triggers a `⚠️ WARNING` at minimum, a hard stop where architecturally enforceable.

**Discipline is architectural, not instructional.** The pipeline refuses to advance without preconditions. Hooks enforce rules mechanically. The issue contract is validated before work begins — not after.

**The issue is the contract.** Every agent reads the issue fresh. Every agent writes structured comments back. The comment trail is the shared memory between all sessions.

**The issue referencing rule.** Any command or agent that references an issue for modification must use structured comment templates. This is enforced by the `PreToolUse` MCP write hook — not by agent memory.

**Three enforcement layers.** Pipeline compliance is enforced at three levels: (1) MCP validation rules — label transitions rejected if preconditions not met; (2) Inspector (Gadget) — runs after every agent dispatch, verifies structured comments and sequence; (3) Agent prompt structure — numbered steps with explicit BLOCK conditions. Each layer is independent — all three must be bypassed simultaneously to violate the pipeline, which is structurally impossible.

**Skills are the unified entry point.** Both Claude Code and Copilot CLI use `skills/*/SKILL.md`. Routing skills use `context: fork` without a fixed `agent:` — the isolated context resolves dispatch from the intent provided. Atomic dispatch skills additionally specify `agent:` for deterministic single-agent execution. Copilot CLI ignores CC-specific frontmatter and uses directive body language for autonomous dispatch. One file, both platforms, same pipeline outcome.

**Worktrees are the isolation mechanism.** All implementation and refactoring work happens in an isolated worktree at `worktrees/issue-{N}-{slug}`. The branch with the same name exists as the delivery artefact for GitHub. The worktree is the primary concept — the branch is a consequence.

---

## 4. Skills (Commands)

Skills are the unified entry point for both Claude Code and Copilot CLI. Each skill lives in `skills/{name}/SKILL.md`. In Claude Code, skills are invoked as `/name` slash commands. In Copilot CLI, skills are invoked the same way or triggered autonomously by the model.

Every routing skill (like `/do`, `/make`) uses `context: fork` without a fixed `agent:` — routing depends on the intent received. Internal atomic dispatch skills use `context: fork` + `agent:` for deterministic single-agent dispatch. Gadget verifies compliance on both platforms.

### `/setup`

Configures the Unblock plugin for the current project. Single command for GitHub setup and local environment setup — one action, complete result.

**Preconditions:** `GITHUB_TOKEN` configured, git repository initialised.

**Phase 1 — Diagnosis**
Calls `doctor` MCP internally. Detects what already exists vs what is missing. Never assumes zero state — safe to re-run.

**Phase 2 — GitHub (if needed)**
Creates labels (all defined in §6), milestone, Projects V2 configuration. Idempotent — skips what already exists.

**Phase 3 — Local environment**
Detects editors present (`.vscode/`, `.cursor/`, etc.) and generates appropriate config files:
- Claude Code → `CLAUDE.md`, `hooks/hooks.json`, agent stubs
- Copilot CLI → `.github/copilot-instructions.md`, `.copilot/mcp-config.json`

Generates config with: MCP tools reference, available commands, workflow rules, template reference.

**Phase 4 — Confirmation**
Output to developer: what was created, what already existed, what still needs action.

**Produces:** Functional environment. Does not create issues or agents.

---

### `/need <context>`

Intent-based agent discovery and installation. The developer describes what they need — the plugin finds and installs the right agent.

**Preconditions:** `/setup` completed.

**Phase 1 — Intent parsing**
Interprets natural language request. Extracts desired capabilities.

Example: `"I need something that creates tasks"` → Fernando (PO agent)
Example: `"I need Rust expertise"` → Rust supervisor

**Phase 2 — Discovery and fetch**
Fetches external agent directory. Semantic matching between request and available agents. Presents options to developer with description of each.

**Phase 3 — Selection**
Developer chooses. Multiple selections allowed.

**Phase 4 — Download and injection**
Downloads agent. Automatically injects:
- Available MCP tools and syntax
- Mandatory comment templates
- Workflow rules specific to Unblock
- Tech-specific conventions (for supervisors)

**Produces:** Agent file in `.claude/agents/` (Claude Code) or `.github/agents/` (Copilot CLI).

---

### `/doctor`

Diagnoses current system state. Read-only. No state changes.

**Phase 1 — MCP health**
Calls `doctor` MCP tool. Verifies connection, auth, configuration.

**Phase 2 — GitHub state**
Verifies labels, milestone, Projects V2 against expected state.

**Phase 3 — Local environment**
Verifies hooks, agent files, CLAUDE.md / copilot-instructions.

**Phase 4 — Structured output**

```
✅ MCP connected
✅ Labels configured (13/13)
⚠️  rust-supervisor not found — run /need "rust expertise"
❌ GITHUB_TOKEN expired
```

**Produces:** Diagnosis. No state changes.

---

### `/think <context>`

Free exploration space. No discipline enforcement, no required issue, no pipeline templates. Full toolset and full agent access — the developer controls what gets created and who helps.

**Preconditions:** None.

**Tools available:** Read, Write, Edit, Glob, Grep, Bash (read-only), MCP (`show`, `list`, `search`)

**Agents available (all optional, developer decides):**

| Agent | Purpose in /think |
|---|---|
| Sherlock | Research — analyse codebase, map patterns, find dependencies |
| Ada | Spec / architecture — design systems, validate technical decisions |
| Fernando | Requirements — structure product requirements, draft issues |
| Any supervisor | Technical exploration — "how would this look in Rust?" |

No Gadget, no templates enforced, no labels, no `claim`. Agents in `/think` produce documents and insights — not pipeline artefacts.

**Phase 1 — Free mode**
No structured output required. Space for exploration, brainstorming, research, iteration. Developer drives entirely.

**Phase 2 — Iteration**
Conversation and agent dispatch at developer's discretion. Can produce any combination of:
- Research document (`docs/research/YYYY-MM-DD-{topic}.md`)
- Product Requirements Document (`PRD.md`)
- Architecture / spec document (`docs/spec-{topic}.md`)
- Any other local file the developer wants

The recommended exploration sequence — each step optional:

```
research doc  →  PRD.md  →  spec doc  →  /plan
```

Developer can stop at any point. Each step is a conscious decision.

**Phase 3 — Contextual handoff**
At the end, offers next step based on what was produced — no obligation:

| What was produced | Offered next step |
|---|---|
| Research document | "Want to advance to PRD? I can dispatch Fernando." |
| PRD | "Want to advance to spec/arch? I can dispatch Ada." / "Ready for `/plan`?" |
| Spec / arch doc | "Ready for `/plan`?" |
| Nothing | "Want to save anything? Or end here." |

In all cases:
- "Want to save as a file? Where?" (developer specifies path)
- "Want to archive this session as a research issue in GitHub?"
- "End without artefacts."

Developer chooses any, all, or none. `/think` creates or overwrites files at the chosen path. Session ends cleanly regardless of what was chosen.

**Produces:** Whatever the developer decides. Nothing mandatory.

---

## 4.1 Document Templates

Templates used by agents during `/think` and `/plan`. Available in `templates/TEMPLATES.md`. All are optional for `/think` — the developer may use their own format. For `/plan`, Ada follows these formats when generating planning documents.

#### Research Document

```markdown
# Research — {topic}

**Date:** {YYYY-MM-DD}
**Author:** {name}
**Status:** draft | reviewed | archived

## Question
{what was being explored}

## Findings
{what was discovered}

## Sources
- {file, issue, external link}

## Conclusions
{key takeaways}

## Open questions
- {what remains unanswered}

## Next steps
- {what this research feeds into}
```

#### PRD — Product Requirements Document

```markdown
# PRD — {project or feature name}

**Date:** {YYYY-MM-DD}
**Author:** {name}
**Status:** draft | approved

## Problem
{what problem this solves and for whom}

## Goals
- {measurable goal}

## Non-goals
- {explicitly out of scope}

## Users
{who uses this and in what context}

## Requirements
### Functional
- {requirement}

### Non-functional
- {performance, security, reliability constraints}

## Constraints
{technical, legal, time, budget constraints}

## Definition of Done
{what does "finished" look like}

## Open questions
- {decisions still pending}
```

#### Spec / Architecture Document

```markdown
# Spec — {component or feature name}

**Date:** {YYYY-MM-DD}
**Author:** {name}
**Status:** draft | approved
**PRD:** [PRD.md](../PRD.md)

## Overview
{what this spec covers}

## Context
{why this design was chosen}

## Design

### Components
{key components and their responsibilities}

### Data flow
{how data moves through the system}

### Interfaces
{APIs, contracts, schemas}

## Decisions
- DECISION: {what was chosen} instead of {alternative} because {reason}

## Risks
- {risk and mitigation}

## Open questions
- {decisions still pending}
```

---

### `/plan <context>`

Transforms exploration artefacts into planning documents and GitHub issues. Has two distinct modes — the developer chooses when to commit to creating issues. Issues are never created speculatively — only when the developer is ready to start a phase.

**Preconditions:** None. Issues `⚠️ WARNING` if same session as `/do` or `/make`.

---

#### Mode A — Global vision (`/plan "auth system for unblock"`)

Produces the project map. No issues created — only structure and documents.

**Step 1 — PRD verification**

Checks for `PRD.md`:

| State | Behaviour |
|---|---|
| Exists and complete | Ada uses it directly |
| Exists with gaps | Ada lists missing fields, asks developer to complete before proceeding |
| Does not exist | Warns: "No PRD found. Consider `/think` first. Proceed without PRD? (y/n)" |

**Step 2 — Phase structuring**
Ada reads PRD + any spec documents. Identifies phases, their goals, dependencies, and rough scope. Does NOT define individual issues yet — only phase-level structure.

**Step 3 — Phase map presentation**
Ada prepares the phase structure and presents it to the developer for review. Developer confirms or adjusts.

BLOCK: Do not write any files until developer confirms the phase map.

**Step 4 — Document generation**
Ada writes `PLAN.md` with the confirmed phase structure. No issue numbers — issues do not exist yet.

```
docs/
├── PRD.md      # Created if missing (minimal), or left unchanged
└── PLAN.md     # Created — phase overview, no issues yet
```

**Produces:** `PRD.md` + `PLAN.md` with phase structure. Zero GitHub issues.

---

#### Mode B — Phase planning (`/plan "phase 1"`)

Plans a specific phase in detail and creates its GitHub issues. Developer calls this when ready to start a phase — after learnings from the previous phase.

**Step 1 — Phase identification**

BLOCK: Must have `PLAN.md` — stops if not found: "No PLAN.md found. Run `/plan` first to define the project phases."

Ada reads `PLAN.md`, identifies the requested phase. Confirms with developer: "Planning Phase 1 — {name}. Correct?"

**Step 2 — Phase detail document**

Ada produces `plans/NN-plan-{slug}.md` with full detail for this phase: objective, scope, acceptance criteria, technical notes, dependencies.

BLOCK: Ada presents the plan document to developer. Developer confirms before any issues are created.

**Step 3 — Issue definition**

Ada defines each issue for this phase with complete fields:

| Field | Required |
|---|---|
| Title | ✅ |
| Description | ✅ |
| Acceptance criteria | ✅ |
| Assignee (supervisor) | ✅ |
| Milestone / Epic | ✅ |
| Design / Notes | ⚠️ recommended |
| Dependencies | ⚠️ if applicable |

BLOCK: Ada does not hand off to Fernando until every issue definition is complete.

**Step 4 — Issue creation (sequential)**

BLOCK: Fernando creates issues one at a time. Each issue is created, GitHub number confirmed, before the next is created. Fernando never batches.

```
Fernando creates issue → GitHub returns #15 → confirmed
Fernando creates issue → GitHub returns #16 → confirmed
Fernando creates issue → GitHub returns #17 → confirmed
BLOCK: All numbers confirmed before proceeding to Step 5.
```

**Step 5 — PLAN.md update**

BLOCK: Only after all issue numbers are confirmed — Ada updates the relevant phase entry in `PLAN.md` with the real issue numbers and the link to the plan file.

```
Phase 1 — Auth        (was: no issues)
→ Issues: #15, #16, #17
→ Detail: plans/01-plan-auth.md
```

**Step 6 — Summary**
Lists created issues with numbers, titles, dependency graph, and links to generated plan files.

**Produces:** `plans/NN-plan-{slug}.md` + GitHub issues with real numbers + `PLAN.md` updated.

---

### Planning Document Templates

#### PLAN.md

Written by `/plan` Mode A. Updated by `/plan` Mode B as each phase is planned.

```markdown
# Plan — {project name}

## Phases

### Phase {N} — {name}
Goal: {1-2 sentences — what this phase delivers}
Issues: {none yet | #15, #16, #17 — populated by /plan Mode B}
Detail: {none yet | [plans/{NN}-plan-{slug}.md](plans/{NN}-plan-{slug}.md)}
Depends on: {Phase X | none}
```

**Critical rule:** Issue numbers are written to `PLAN.md` only after Fernando confirms each GitHub issue was created successfully. Never written speculatively. Status is never stored — always derived from GitHub at read time.

#### plans/NN-plan-{slug}.md

Written by Ada in `/plan` Mode C Step 3, before issues are created. The Issues table starts empty. Fernando populates it with real GitHub numbers in Step 5 after sequential creation.

```markdown
# Phase {N} — {name}

**Created:** {YYYY-MM-DD}
**PRD:** [PRD.md](../PRD.md)

## Objective
{what this phase delivers}

## Scope

### In scope
- {included}

### Out of scope
- {explicitly excluded}

## Issues
| Issue | Title |
|---|---|
| — | — |  ← empty when file is first written; populated after GitHub creation

## Acceptance criteria
- {criterion}

## Technical notes
{architecture decisions, constraints, known gotchas}

## Dependencies
{what must exist or be completed before this phase starts}
```

**After Fernando confirms all issue numbers**, Ada updates the Issues table with real values:

```markdown
## Issues
| Issue | Title |
|---|---|
| #15 | Implement OAuth handler |
| #16 | Add JWT session middleware |
| #17 | Write auth integration tests |
```

---

### `/do <context>`

Intent router for interactive execution. The developer is present — the agent can pause and ask. Routes to the correct agent based on context.

**Preconditions:** Validated per branch — see each branch below. Investigation, spike, and review branches have minimal preconditions. Implementation (Branch A) requires complete issue contract.

**Phase 1 — Intent routing**

The router classifies the request:

| Intent detected | Route |
|---|---|
| Implementation | Branch A |
| Specification / Architecture | Branch B |
| Investigation | Branch C |
| Spike / Research | Branch D |
| PR / branch review | Branch E |
| QA validation | Branch F |
| Planning intent detected | **Stop** → "That is `/plan`." |

**Branch A — Implementation**

Precondition tree:
1. Same session as `/plan`? → `⚠️ WARNING` (large, explicit)
2. Has `INVESTIGATION` comment? No → exceptions:
   - Label `unblock:no-investigation` on issue → proceed
   - Issue type is trivially simple (detected by assignee/title) → proceed
   - Is rework (`unblock:review:rework` or `unblock:qa:rework` label) → use existing investigation
   - Otherwise → **stop**: "Need a clean session for investigation. Run `/do investigate #N` in a new session first."
3. Has supervisor assigned? No → **stop**: "Run `/need` first."
4. Is rework? Yes → asks developer: "A worktree for this issue already exists (`worktrees/issue-{N}-{slug}`). Use it or create a new one?" Developer chooses. No → creates new worktree `worktrees/issue-{N}-{slug}`.

Dispatches correct supervisor via skill dispatch (`context: fork` in CC, directive body in Copilot CLI). Writes `DISPATCH` comment before dispatch.
After completion: reads `COMPLETED` comment, presents summary to developer.

**Branch B — Specification / Architecture**
Dispatches Architect (Ada). No worktree, no branch. Output is a document or structured spec.

**Branch C — Investigation**
Warns developer: "Starting clean session — this session is now invalidated for implementation."
Dispatches Investigator (Sherlock). Writes `INVESTIGATION` comment to issue.

**Branch D — Spike**
Dispatches Architect (Ada) in spike mode. Isolated worktree, disposable branch. Output is a document or POC — not production code. No COMPLETED comment required, no review gate.

**Branch E — PR / Branch Review**
Gadget verifies COMPLETED comment exists and is well-formed before dispatching Linus.
Verifies PR / branch in checkout. Dispatches Linus.
Output: APPROVE / NEEDS-REFACTORING / NEEDS-REWORK.
Gadget verifies REVIEW comment format and label consistency after Linus completes.
If NEEDS-REWORK (CRITICAL or WARNING found) → asks developer if wants to trigger `/do "fix #N"`.
If NEEDS-REFACTORING (SUGGESTION only) → asks developer if wants to trigger Martin.
If APPROVE → SUGGESTION findings (if any) create issues in the issue's parent epic via Fernando. CRITICAL and WARNING are handled via `unblock:review:rework` — they never become standalone findings.

**Branch F — QA**
Precondition: `unblock:review:ok` label present. If not → stops: "Issue has not passed review yet. Run `/do "review #N"` first."
Gadget verifies REVIEW comment with APPROVE verdict exists before dispatching Quinn.
Dispatches Quinn. After completion: Gadget verifies QA comment format and label consistency.
If PASS+FINDINGS → Fernando dispatched to create finding issues in parent epic.

**Produces:** Depends on branch. Always with structured comment on issue if issue referenced.

---

### `/make <context>`

Autonomous execution. No human-in-the-loop. Stricter preconditions than `/do`.

**Preconditions — implementation tasks (hard — no recovery path):**
- Issue has complete contract ✅
- Supervisor assigned ✅
- `INVESTIGATION` exists or `unblock:no-investigation` label ✅
- No open blocking dependencies ✅

**Preconditions — investigation and review tasks:**
- Issue exists ✅
- Issue number provided ✅

If implementation preconditions fail → comments on issue what is missing, applies `unblock:needs-human` label, stops cleanly.

**Phase 1 — Claim and setup**
Calls `claim` MCP. For implementation tasks: creates isolated worktree `worktrees/issue-{N}-{slug}` — branch created implicitly. For investigation and review tasks: no worktree needed — these are read-only operations.

**Phase 2 — Autonomous execution**
Dispatches correct agent. No interaction. Agent decides path.

**Phase 3 — Result**
- COMPLETED → push, label `unblock:review:pending`, output to developer/CI
- BLOCKED → `BLOCKED` comment on issue, `unclaim` with `reason: expired`, label `unblock:needs-human`, worktree preserved for inspection

**CI usage (Teams / Enterprise):**
- `/make investigate #N` → remote MCP → `INVESTIGATION` comment on issue (no label — comment is the signal)
- `/make review #N` → remote MCP → `REVIEW` comment + label (`unblock:review:ok` or `unblock:review:rework`)

**Produces:** Completed work or documented blocked state. Always clean.

---

### `/use <context>`

Direct access to a specific agent or supervisor. The developer knows what they want and with whom.

**Preconditions:** Named agent/supervisor exists.

**Phase 1 — Resolution**
Identifies agent by name or described capability. If ambiguous → presents options.

**Phase 2 — Discipline check**
Issue referenced? Yes → comment templates mandatory (enforced by hook).
No issue → free mode, no enforcement.

**Phase 3 — Dispatch**
Dispatches agent via skill dispatch (`context: fork` in CC, directive body in Copilot CLI). Context passed directly.

**Produces:** Agent output. Structured comment on issue if issue referenced.

---

### `/info <query>`

Natural language query interface over project state. Read-only.

**Maps to MCP tools and local documents based on intent:**

| Query | Sources |
|---|---|
| "how many open issues" | `stats` MCP |
| "show only epics" | `list` MCP with filters |
| "issues blocking me" | `list` MCP + dependency graph |
| "where are we" / "project status" | `PLAN.md` + MCP issue status per phase |
| "is everything in sync" | `doctor` + `prime` MCP |

For "where are we" queries: reads `PLAN.md` to get phase structure and issue numbers, then calls MCP to get real-time status of each issue. Presents a computed overview — no files are modified.

**Produces:** Formatted, readable output. No JSON dumps. No state changes.

---

### `/trail <issue>`

Structured narrative history of an issue. Not a raw dump — an intelligent condensation.

**Phase 1 — Fetch**
Calls `show #N --include_comments`.

**Phase 2 — Structured parsing**
Identifies and orders chronologically:
`DISPATCH` → `INVESTIGATION` → `DECISION/DEVIATION` → `COMPLETED` → `AUDIT` → `REVIEW` → `REFACTORING` → `QA` → `BLOCKED` → `PAUSE`

Note: `AUDIT` and `BLOCKED` appear in the trail at their chronological position — AUDIT after any phase violation, BLOCKED when an agent could not proceed.

**Phase 3 — Condensed narrative**
Not raw comments — structured story: what was investigated, what was decided, what review found, what QA validated.

**Produces:** Chronological narrative. No state changes.

---

### `/ship <issue>`

Pre-merge readiness checkpoint. The developer invokes deliberately before merging.

**Validates sequentially:**

| Check | ✅ / ❌ |
|---|---|
| `unblock:review:ok` label present | |
| `unblock:qa:ok` label present | |
| No open blocking dependencies | |
| No critical findings pending in parent epic | |
| Worktree branch exists with commits after main | |

**Output example — ready:**
```
✅ Issue #42 ready for merge
   Branch: issue-42-implement-rate-limiter
   Review: unblock:review:ok (2026-04-01, Linus)
   QA: unblock:qa:ok (2026-04-02, Quinn)
   Dependencies: all closed
```

**Output example — not ready:**
```
❌ Issue #42 not ready
   ❌ QA not passed — run /make qa #42
   ⚠️  2 pending findings in epic #12
```

**Produces:** Verdict. No state changes. Merge is always the developer's decision.

---

### `/pause <issue>`

⏸ **Pending `unclaim` MCP tool.** Design reserved.

When `unclaim` is available:
- Writes `PAUSE` structured comment to issue
- Calls `unclaim` with `reason: paused`
- Preserves worktree `worktrees/issue-{N}-{slug}` for resumption
- Applies label `unblock:paused`

---

## 5. Comment Templates

All templates are the shared vocabulary between agents. Every agent that writes to an issue must use these formats exactly — they are parsed by downstream agents and by the pipeline itself.

### DISPATCH
*Written by:* skill (before dispatching subagent)
*Consumed by:* supervisor (implementation context)

```
DISPATCH:
Supervisor: {githubusername}-{agent-name}
Task: {what was requested}
Context: {relevant investigation findings, files, gotchas}
```

Note: `{githubusername}` is read from `git config user.name` or the claiming developer's GitHub username from the MCP claim context. `{agent-name}` is the supervisor type (e.g., `rust-supervisor`, `react-supervisor`).

### INVESTIGATION
*Written by:* Investigator (Sherlock)
*Consumed by:* Supervisor, `/do` router (detect if already exists), `/trail`

```
INVESTIGATION:
Summary: {1-2 sentences of what was found}

Root cause:
- File: {path}:{line}
- Function: {name}
- Reason: {explanation}

Affected files:
- {path} — {relevance}
- {path} — {relevance}

Approach:
1. {step}
2. {step}

Risks:
- {risk and mitigation}

Notes:
- {any additional relevant context}
```

### COMPLETED
*Written by:* Supervisor
*Consumed by:* Linus (review), Quinn (QA), `/trail`

```
COMPLETED:
Summary: {1-2 sentences of what was implemented}

Files:
- {path} — {what was done}
- {path} — {what was done}

Decisions: {count or "none"}
Deviations: {count or "none — implemented as spec"}

Tests: {what was tested and how verified}
Build: PASS | FAIL
```

### DECISION
*Written by:* any agent during implementation
*Consumed by:* Linus, Quinn, human, `/trail`

```
DECISION: {what was chosen} instead of {alternative} because {reason}
```

Example:
```
DECISION: Used pagination cursor instead of offset because dataset exceeds 10k rows and offset degrades at scale
```

### DEVIATION
*Written by:* any agent during implementation
*Consumed by:* Linus, Quinn, human, `/trail`

```
DEVIATION: Spec said {X}, implemented {Y} because {reason}
```

Example:
```
DEVIATION: Spec said REST endpoint, implemented WebSocket because spec requires real-time push updates
```

### REVIEW
*Written by:* Linus
*Consumed by:* skill flow, Martin, human, `/trail`

```
REVIEW:
Acceptance: PASS | PARTIAL | FAIL
- {criterion} — PASS | FAIL — {evidence}
- {criterion} — PASS | FAIL — {evidence}

Findings:
- [CRITICAL] {path}:{line} — {description and impact}
- [WARNING] {path}:{line} — {description and suggestion}
- [SUGGESTION] {path}:{line} — {improvement opportunity}
- [GOOD] {path}:{line} — {acknowledgement}

Security: PASS | {issues found}
Performance: PASS | {concerns}
Tests: PASS | {gaps}

Verdict: APPROVE | NEEDS-REFACTORING | NEEDS-REWORK
Reason: {required if not APPROVE}
```

**Verdict → label mapping:**

| Verdict | Trigger | Label |
|---|---|---|
| APPROVE | No remaining CRITICAL, WARNING, or SUGGESTION findings in current code state | `unblock:review:ok` |
| NEEDS-REFACTORING | SUGGESTION findings only (no CRITICAL or WARNING) | `unblock:review:pending` (unchanged) |
| NEEDS-REWORK | Any CRITICAL or WARNING finding | `unblock:review:rework` |

**Finding → action mapping:**

| Severity | Action |
|---|---|
| CRITICAL | Rework — never becomes a finding issue |
| WARNING | Rework — never becomes a finding issue |
| SUGGESTION | Finding issue created in parent epic |
| GOOD | No action |

### REFACTORING
*Written by:* Martin
*Consumed by:* skill flow, Linus (re-review), human

```
REFACTORING:
Findings processed: {total}

FIXED:
- {path}:{line} — {what was changed and why}

DEFERRED:
- {path}:{line} — TODO(#{issue}) — {description}

FALSE-POSITIVE:
- {path}:{line} — {reason it is not a real problem}

SKIPPED:
- {path}:{line} — {reason — too risky, needs dedicated task}

Tests: PASS | FAIL — {what was verified}
Behavior preserved: yes
```

### QA
*Written by:* Quinn
*Consumed by:* skill flow, human, `/trail`

```
QA:
Conformity:
- [CONFORMS] {requirement} — {evidence}
- [DEVIATES] {requirement} — spec: {X}, implemented: {Y} — DEVIATION logged: yes | no
- [MISSING] {requirement} — {not implemented}
- [EXTRA] {description} — {not in spec}

Acceptance criteria:
- [PASS] {criterion} — {evidence}
- [FAIL] {criterion} — {reason}

Boundaries:
- [OK] {area} — {what was checked}
- [RISK] {area} — {unhandled boundary}

Decision trail:
- DECISION comments: {count}
- DEVIATION comments: {count}
- Unlogged deviations found: {count} — {details if any}

Tests: PASS | FAIL — {pass_count} passed, {fail_count} failed
Build: PASS | FAIL
Lint: PASS | FAIL — {error_count} errors, {warning_count} warnings
Functional: VERIFIED | SKIPPED — {what was exercised or why skipped}

Verdict: PASS | PASS+FINDINGS | FAIL
Failures: {BLOCKER | MAJOR — list if FAIL}
Findings: {MINOR | RISK | DEVIATES | EXTRA — list if present, tracked as finding issues}
```

**Verdict → label mapping:**

| Verdict | Trigger | Label |
|---|---|---|
| PASS | No BLOCKER or MAJOR findings | `unblock:qa:ok` |
| PASS+FINDINGS | No BLOCKER or MAJOR, but MINOR/RISK/DEVIATES/EXTRA present | `unblock:qa:ok` + Fernando creates finding issues |
| FAIL | Any BLOCKER or MAJOR finding | `unblock:qa:rework` |

**QA finding → action mapping:**

| Severity | Action |
|---|---|
| BLOCKER | Rework — never becomes a finding issue |
| MAJOR | Rework — never becomes a finding issue |
| MINOR | Finding issue created in parent epic |
| RISK | Finding issue created in parent epic |
| DEVIATES | Finding issue created in parent epic |
| EXTRA | Finding issue created in parent epic |

### AUDIT
*Written by:* Inspector (Gadget)
*Consumed by:* skill flow (remediation dispatch), developer, `/trail`
*Note:* Only written when violations exist. Clean pipelines produce no AUDIT comment.

```
AUDIT:
Phase: {current pipeline phase}
Status: VIOLATIONS FOUND

Violations:
- [MISSING] {what is missing} — {what the agent must do to fix it}
- [MALFORMED] {comment type} — {field missing or wrong format}
- [MISMATCH] {what does not match} — expected {X}, found {Y}
- [SEQUENCE] {what is out of order} — {what this means for the pipeline}

Required action: {explicit instruction for the responsible agent or developer}
```

### BLOCKED
*Written by:* any agent when unable to proceed autonomously
*Consumed by:* developer, `/trail`

```
BLOCKED:
Reason: {description of what cannot be resolved autonomously}
State: {what was completed before blocking}
Needs: {what requires human decision}
Resume with: /do "resume #N" or /make "resume #N"
```

### PAUSE
*Written by:* developer or any agent
*Consumed by:* any agent resuming, `/trail`
*Depends on:* `unclaim` MCP tool (pending)

```
PAUSE:
Paused by: {agent or developer}
State: {what was completed so far}

Remaining:
- {what is left}

Pending decisions:
- {decision left open}

Context:
- {critical information to resume without losing context}

Resume with: /do "resume #N"
```

### FINDING (issue body — created in parent epic)
*Not a comment on the reviewed issue — it is the body of a new issue created in the parent epic*
*Created by:* Fernando, when REVIEW has SUGGESTION or QA has MINOR/RISK/DEVIATES/EXTRA.
CRITICAL and WARNING from REVIEW, and BLOCKER and MAJOR from QA trigger rework — they never become finding issues.

```
Title: [FINDING:{severity}] {short description}

Source: #{origin-issue} — {title of origin issue}
Discovered by: REVIEW | QA
Phase: review | qa

Finding:
{path}:{line} — {full description of the finding}

Context:
{relevant excerpt from REVIEW or QA comment}

Acceptance criteria:
- {what needs to be done to close this finding}
```

**Finding placement rule:** Findings are created as child issues of the parent epic of the reviewed task. If the task has no parent epic, finding is created as a standalone issue with `unblock:finding:{severity}` label.

---

## 6. Labels

All labels created by `/setup`. Grouped by purpose.

### Workflow state

| Label | Meaning |
|---|---|
| `unblock:review:pending` | Implementation complete, awaiting review |
| `unblock:review:ok` | Review approved |
| `unblock:review:rework` | Review requires rework — back to supervisor |
| `unblock:qa:ok` | QA passed — ready for merge |
| `unblock:qa:rework` | QA failed — requires attention |
| `unblock:needs-human` | Autonomous agent blocked — requires human decision |
| `unblock:paused` | Work intentionally paused, worktree preserved |

### Implementation modifiers

| Label | Meaning | Applied by |
|---|---|---|
| `unblock:no-investigation` | Issue does not require investigation phase | Developer or `/plan` |

### Finding severity

| Label | Source | Action |
|---|---|---|
| `unblock:finding:suggestion` | REVIEW [SUGGESTION] | Finding issue |
| `unblock:finding:minor` | QA MINOR | Finding issue |
| `unblock:finding:risk` | QA [RISK] | Finding issue |
| `unblock:finding:deviation` | QA [DEVIATES] | Finding issue |
| `unblock:finding:extra` | QA [EXTRA] | Finding issue |

Note: CRITICAL and WARNING from REVIEW, and BLOCKER and MAJOR from QA trigger `unblock:review:rework` or `unblock:qa:rework` respectively — they never become finding issues.

---

## 7. Hooks

Four hooks. No more needed.

### SessionStart

**Trigger:** Beginning of any session.

**Responsibility:** Inject project context and current state.

**Logic:**
1. Detect mode: local (solo) vs CI (teams/enterprise) via env var presence
2. If local:
   a. Call `prime` MCP — inject current task state, unblocked work, recent activity
   b. If `PLAN.md` exists — inject it as project overview context (it is short by design)
   c. Do NOT inject individual `plans/NN-plan-xxx.md` files — those are read on demand by agents
3. If CI: inject only the specific task context for the current job
4. Check plugin version against installed version — if outdated, notify developer to run `/update`

**File:** `hooks/session-start.sh`

---

### PreToolUse → Task dispatch

**Trigger:** Any subagent dispatch in Claude Code (CC internally uses `Task()` when a skill specifies `context: fork`). CC-specific hook — Copilot CLI uses directive body language instead.

**Responsibility:** Inject discipline reminder when dispatching a supervisor.

**Logic:**
1. Read `tool_name` from input — only proceed if `Task`
2. Read `subagent_type` from input
3. If dispatching an implementation agent → inject system reminder:
   - Read issue before coding
   - Use comment templates
   - Invoke `unblock-verify` before writing `COMPLETED`

**File:** `hooks/inject-discipline.sh`

---

### PreToolUse → MCP write tools

**Trigger:** Any call to MCP write tools (`comment`, `update`, `close`, `claim`).

**Responsibility:** Enforce template discipline when issue is referenced.

**Logic:**
1. Read `tool_name` from input — only proceed if one of the write tools
2. Detect if an issue number (`#N` or integer) is present in `tool_input`
3. If issue referenced → inject template reminder: available templates, required format
4. If no issue referenced → pass through without injection

**File:** `hooks/mcp-write-guard.sh`

---

### SessionEnd

**Trigger:** End of any session.

**Responsibility:** Compliance check and auto-unclaim if session ends without COMPLETED comment on a claimed issue.

**Logic:**
1. Check if current agent has any issue in `claimed` state (via `list --assignee current --status in_progress`)
2. If claimed issue found:
   a. Invoke Gadget to run compliance check on the issue
   b. If Gadget finds violations → writes `AUDIT` comment (Gadget handles this)
   c. Check if `COMPLETED` comment exists on the issue
   d. If no `COMPLETED` comment → call `unclaim` with `reason: expired`
   e. Apply label `unblock:needs-human` to the issue
3. Log what happened to session output

**File:** `hooks/session-end.sh`

**Note:** Requires `unclaim` MCP tool (see §10).

---

## 8. Agents

### Agent design principles

- Every agent reads the full issue via `show #N --include_comments` before any action
- Every agent has a restricted toolset appropriate to its role
- No agent closes issues — developer decides
- No agent merges — developer decides
- Sessions are isolated — no shared context with dispatching agent

---

### Investigator — "Sherlock"

**Role:** Codebase analyst. Reads everything, writes nothing in code.

**Model:** Opus — complex reasoning, root cause analysis

**Tools:** Read, Glob, Grep, Bash (read-only), MCP (`show`, `comment`)

**Bash restriction:** Only read-only commands (`git log`, `git diff`, `git blame`). Never writes files.

**Process:**
1. Reads full issue with comments
2. Locates relevant files via Glob/Grep
3. Analyses code in full context — not just diff, the complete file
4. Four phases: localisation → root cause → affected files → approach and risks
5. Writes `INVESTIGATION` comment to issue

**Output:** `INVESTIGATION` comment on issue

**Hard boundaries:**
- Never writes code
- Never creates worktrees
- Never claims issue
- Never closes issue

---

### Architect — "Ada"

**Role:** Systems thinker, solution designer.

**Model:** Opus — high-level design decisions

**Tools:** Read, Write, Edit, Glob, Grep, MCP (`show`, `comment`)

**Note:** Write and Edit are required for planning mode — Ada produces `PRD.md`, `PLAN.md`, and `plans/NN-plan-{slug}.md` files.

**Mode A — Specification (via `/do spec`):**
1. Reads context and referenced files
2. Produces specification / architecture document
3. Writes `DECISION` comments for relevant architectural choices

**Mode B — Global vision (via `/plan "vision"`):**
1. Reads PRD + any spec documents
2. Identifies phases, goals, dependencies at phase level only — no individual issues
3. Presents phase map to developer for confirmation
4. BLOCK: Does not write files until developer confirms
5. Writes `PRD.md` (if missing) and `PLAN.md` with confirmed phase structure
6. No issues created — zero GitHub writes

**Mode C — Phase planning (via `/plan "phase N"`):**
1. Reads `PLAN.md` to identify the requested phase
2. Confirms phase with developer before proceeding
3. Produces `plans/NN-plan-{slug}.md` with full phase detail
4. BLOCK: Presents plan document to developer — no issues created until confirmed
5. Defines each issue with complete fields, validates completeness
6. BLOCK: Does not hand off to Fernando until all issue definitions are complete
7. Passes validated issue definitions to Fernando for sequential creation
8. After Fernando confirms all numbers — updates `PLAN.md` with real issue numbers and plan file link

**Output:** Specification doc (Mode A) | `PRD.md` + `PLAN.md` (Mode B) | `plans/NN-plan-{slug}.md` + `PLAN.md` updated (Mode C)

**Hard boundaries:**
- Never writes implementation code
- Never claims issues
- Never creates issues directly — always delegates to Fernando
- Never writes files before developer confirmation
- Never hands off incomplete issue definitions to Fernando

---

### Supervisor (generated via `/need`)

**Role:** Implementation specialist. Tech-stack expert. Dynamic persona name.

**Model:** Sonnet — execution

**Tools:** Read, Write, Edit, Glob, Grep, Bash (full), MCP (`show`, `comment`, `update`)

**Process:**
1. Reads full issue with comments
2. Reads `INVESTIGATION` comment for context
3. Creates isolated worktree `worktrees/issue-{N}-{slug}` — branch `issue-{N}-{slug}` is created implicitly
4. Implements following acceptance criteria within the worktree
5. For each non-trivial decision: writes `DECISION` comment
6. For each spec deviation: writes `DEVIATION` comment
7. Invokes internal skill `unblock-verify` before finalising
8. If verify passes: writes `COMPLETED` comment, pushes, applies label `unblock:review:pending`
9. If verify fails after 3 attempts: writes `BLOCKED` comment, applies `unblock:needs-human`

**Injected at install via `/need`:**
- Tech-specific conventions (e.g., Rust idioms, React patterns)
- Specific commands (e.g., `cargo test`, `npm run build`)
- Mandatory comment templates
- MCP tools and syntax

**Hard boundaries:**
- Never closes issues
- Never merges
- Never touches files outside the project
- Never implements without `INVESTIGATION` (except `unblock:no-investigation` label or trivial issue type)

---

### Linus — Code Reviewer

**Role:** Rigorous, fair, constructive quality gate.

**Model:** Opus — code quality judgement

**Tools:** Read, Glob, Grep, Bash (read-only: `git diff`, `git log`, `git branch`), MCP (`show`, `comment`, `update`)

**Process:**
1. Reads full issue with comments — description, acceptance criteria, `INVESTIGATION`, `COMPLETED`
2. Analyses complete diff — full files, not just diff chunks
3. Validates acceptance criteria one by one with evidence
4. Analyses: code quality, security, performance, tests
5. Classifies findings: CRITICAL / WARNING / SUGGESTION / GOOD
6. Writes `REVIEW` comment to issue
7. Applies label based on verdict:
   - APPROVE (no CRITICAL, WARNING) → `unblock:review:ok`
   - NEEDS-REFACTORING (SUGGESTION only) → `unblock:review:pending` (unchanged)
   - NEEDS-REWORK (any CRITICAL or WARNING) → `unblock:review:rework`

**Verdict logic:** Any CRITICAL or WARNING finding forces NEEDS-REWORK regardless of other findings. NEEDS-REFACTORING only applies when the sole non-positive findings are SUGGESTION. Martin is only dispatched for SUGGESTION findings.

**Output:** `REVIEW` comment + correct label

**Hard boundaries:**
- Never writes code
- Never creates worktrees or commits
- Never approves with CRITICAL or WARNING findings
- Never auto-approves after Martin — always re-reviews

---

### Martin — Refactorer

**Role:** Cautious, methodical. Never applies changes blindly.

**Model:** Sonnet — execution with validation

**Tools:** Read, Write, Edit, Glob, Grep, Bash (full), MCP (`show`, `comment`, `update`)

**Input:** Issue number, implementation branch name

**Process:**
1. Creates isolated worktree `worktrees/issue-{N}-review-fix` from the implementation branch — branch `refactor/issue-{N}-review-fix` is created implicitly
2. Reads `REVIEW` comment — extracts all findings
3. For each finding, decision tree within the worktree:
   - Is it a real problem? No → `FALSE-POSITIVE`, logs reason
   - Already tracked in another issue? Yes → `TODO(#N)` in code, commit, `DEFERRED`
   - Safe to fix without changing behaviour? No → `SKIPPED`, logs reason
   - Apply fix → commit → run tests → `FIXED`
4. Writes `REFACTORING` comment to issue
5. Applies `unblock:review:pending` for mandatory re-review by Linus

**Critical rule:** After Martin, Linus ALWAYS re-reviews. Never auto-approve after refactoring.

**Hard boundaries:**
- Never creates issues (that is Fernando)
- Never closes issues
- Never applies fixes without validating the finding first
- Never creates worktrees from main — always from the implementation branch under review

---

### Quinn — QA Gate

**Role:** Meticulous, product-minded. Last gate before merge.

**Model:** Opus — product conformity analysis

**Tools:** Read, Glob, Grep, Bash (test runners, build, lint), MCP (`show`, `comment`, `update`)

**Process:**
1. Reads full issue — description, acceptance criteria, all comments
2. Reads spec / design doc referenced in issue
3. Conformity check: CONFORMS / DEVIATES / MISSING / EXTRA per requirement
4. Validates each acceptance criterion with evidence
5. Analyses boundaries and edge cases
6. Audits decision trail — `DECISION` + `DEVIATION` comments vs actual code
7. Runs tests, build, lint
8. Verifies functionally where possible
9. Writes `QA` comment to issue
10. Applies label: `unblock:qa:ok` (PASS) or `unblock:qa:rework` (FAIL)

**FAIL severity classification:**

| Severity | Action |
|---|---|
| BLOCKER | Rework — cannot merge, broken tests, data loss risk, unmet acceptance criteria |
| MAJOR | Rework — conformity gap, unhandled boundary that affects correctness |
| MINOR | Finding issue — cosmetic gap, can defer |
| RISK | Finding issue — boundary concern, can defer with tracking |
| DEVIATES | Finding issue — unlogged deviation, can defer with tracking |
| EXTRA | Finding issue — out of spec addition, can defer with tracking |

**Output:** `QA` comment + correct label

**Hard boundaries:**
- Never writes code
- Never merges
- Never approves with BLOCKER or MAJOR findings

---

### Fernando — Product Owner

**Role:** Creates and organises issues. Structured executor.

**Model:** Sonnet — structured execution

**Tools:** Read, MCP (`show`, `create`, `update`, `depends`, `list`, `search`)

**Context A — Via `/plan` Mode B (dispatched by Ada):**
1. Receives issue definitions from Ada — complete and validated
2. Creates issues **one at a time** — never in batch:
   - Creates issue → waits for GitHub to return the real issue number → confirms number
   - Only then creates the next issue
   - BLOCK: Never proceed to the next issue before the current one has a confirmed GitHub number
3. Reports each confirmed number back to Ada for PLAN.md update
4. Establishes dependencies via `depends` after all issues are created
5. Stops if any required field is missing — does not create, reports back to Ada

**Context B — Findings tracking (after Linus or Quinn):**
1. Receives list of deferrable findings only:
   - From REVIEW: `[SUGGESTION]` only. CRITICAL and WARNING trigger rework — they never become findings.
   - From QA: `MINOR`, `[RISK]`, `[DEVIATES]`, `[EXTRA]` only. BLOCKER and MAJOR trigger rework — they never become findings.
2. Looks up parent epic of the reviewed issue via `show`
3. Creates finding issues as children of that parent epic
4. If no parent epic: creates standalone issues with `unblock:finding:{severity}` label
5. Each finding issue gets: `unblock:finding:{severity}` label, link back to origin issue

**Output:** Complete issues in GitHub

**Hard boundaries:**
- Never writes code
- Never claims issues
- Never creates issues without all required fields

---

### Inspector — "Gadget"

**Role:** Pipeline compliance checker. Verifies process was followed, not whether code is correct. Read-only, structural auditor.

**Model:** Sonnet — pattern matching and structured verification, no deep reasoning needed

**Tools:** Read, Glob, Grep, MCP (`show`, `comment`, `list`)

**Invoked by:**
- Skills — automatically between phases (before dispatching next agent)
- Developer — via `/do "audit #N"` on demand
- `SessionEnd` hook — as final check before closing any session with a claimed issue

**Process:**

1. Calls `show #N --include_comments` — reads full issue state
2. Determines current pipeline phase from labels and comment trail
3. Runs compliance checks for the current phase and all completed phases
4. Produces `AUDIT` comment if violations found, returns OK silently if clean

**Compliance checks by phase:**

| Phase | Required | Verified |
|---|---|---|
| Investigation | `INVESTIGATION` comment exists | Format: Summary, Root cause, Affected files, Approach, Risks |
| Implementation | `DISPATCH` comment exists | Format: Supervisor, Task, Context |
| Implementation | `COMPLETED` comment exists | Format: Summary, Files, Decisions count, Deviations count, Tests, Build |
| Implementation | DECISION count matches | `Decisions: N` in COMPLETED matches actual DECISION comment count |
| Implementation | DEVIATION count matches | `Deviations: N` in COMPLETED matches actual DEVIATION comment count |
| Review | `REVIEW` comment exists | Format: Acceptance, Findings, Verdict |
| Review | Verdict is explicit | One of APPROVE / NEEDS-REFACTORING / NEEDS-REWORK |
| Review | Label matches verdict — APPROVE | `unblock:review:ok` present |
| Review | Label matches verdict — NEEDS-REWORK | `unblock:review:rework` present |
| Review | Label matches verdict — NEEDS-REFACTORING | `unblock:review:pending` unchanged |
| Refactoring | `REFACTORING` comment exists | Format: Findings processed, FIXED/DEFERRED/FALSE-POSITIVE/SKIPPED |
| Refactoring | `unblock:review:pending` label applied | Ready for Linus re-review |
| QA | `QA` comment exists | Format: Conformity, Acceptance criteria, Boundaries, Decision trail, Tests, Build, Lint, Verdict |
| QA | Verdict is explicit | One of PASS / PASS+FINDINGS / FAIL |
| QA | Label matches verdict — PASS or PASS+FINDINGS | `unblock:qa:ok` present |
| QA | Label matches verdict — FAIL | `unblock:qa:rework` present |

**Sequence validation:**

Gadget also verifies the pipeline sequence has not been violated:

- `COMPLETED` must not exist before `INVESTIGATION` (unless `unblock:no-investigation` label)
- `REVIEW` must not exist before `COMPLETED`
- `QA` must not exist before `REVIEW` with `Verdict: APPROVE`
- `unblock:review:ok` must not exist before `REVIEW` comment
- `unblock:qa:ok` must not exist before `QA` comment with PASS verdict

**Output:**

If all checks pass → returns `OK` to the invoking skill. No comment written (clean pipelines produce no noise).

If violations found → writes `AUDIT` comment to issue:

```
AUDIT:
Phase: {current phase}
Status: VIOLATIONS FOUND

Violations:
- [MISSING] {what is missing} — {what the agent must do to fix it}
- [MALFORMED] {comment type} — {field missing or wrong format}
- [MISMATCH] {what does not match} — expected {X}, found {Y}
- [SEQUENCE] {what is out of order} — {what this means}

Required action: {explicit instruction for the agent or developer}
```

**Integration with skills:**

Every skill that dispatches a subagent calls Gadget after the subagent completes:

```
Dispatch Supervisor → subagent completes (context: fork in CC / directive in Copilot CLI)
→ Gadget checks compliance
  → OK: skill proceeds to apply label and present summary
  → VIOLATIONS: skill re-dispatches agent with explicit remediation prompt (max 2 retries)
  → Still failing after 2 retries: BLOCKED comment, unblock:needs-human label, stop
```

**Hard boundaries:**
- Never writes code
- Never modifies issue state (labels, status, assignee)
- Never closes issues
- Never creates worktrees
- Writes `AUDIT` comment only when violations exist — clean pipelines produce zero noise

---

## 9. Internal Skills

### `unblock-verify`

Not an agent — a skill invoked by the Supervisor before writing `COMPLETED`. Lives in `skills/unblock-verify/SKILL.md` and is available on both platforms.

**Protocol:**
1. Detect test/build/lint commands from the supervisor's injected conventions (provided by `/need` at install time) — fallback to auto-detection from project files (`Cargo.toml` → `cargo test`, `package.json` → `npm test`, etc.)
2. Run tests → fail: fix attempt (max 3) → if persists: `BLOCKED`
3. Run build → fail: fix attempt (max 3) → if persists: `BLOCKED`
4. Run lint → fail: fix attempt (max 3) → if persists: `BLOCKED`
5. Read acceptance criteria from issue → verify each against the code
6. If any criterion unsatisfied → `BLOCKED` with explanation
7. If all pass → returns OK, Supervisor writes `COMPLETED`

**Why a skill and not instructions:** If the protocol changes, the skill updates — not every supervisor. Consistent behaviour across all generated supervisors.

---

## 10. MCP Backlog

Changes needed in the Unblock MCP server before certain plugin features can be fully implemented.

### `unclaim` tool

**Required for:** `/pause` command, `SessionEnd` hook auto-unclaim, CI failure recovery

**Parameters:**
```
unclaim(
  issue: u32,
  reason: "paused" | "expired" | "abandoned"
)
```

**Behaviour by reason:**

| Reason | Label applied | Worktree |
|---|---|---|
| `paused` | `unblock:paused` | Preserved at `worktrees/issue-{N}-{slug}` |
| `expired` | `unblock:needs-human` | Preserved at `worktrees/issue-{N}-{slug}` |
| `abandoned` | `unblock:needs-human` | Cleaned up |

**Atomicity requirement:** Must atomically clear assignee, update status, apply label, and timestamp the operation.

---

### Label transition validation rules

**Required for:** Architectural enforcement of pipeline discipline — agents cannot skip steps even if they try.

The MCP server validates preconditions before applying any label transition. If the precondition is not met, the operation is rejected with an explicit error message that the agent or skill must handle.

| Label being applied | Precondition required | Error if violated |
|---|---|---|
| `unblock:review:pending` | `COMPLETED` comment exists OR `REFACTORING` comment exists | `"BLOCKED: Write COMPLETED or REFACTORING comment before marking review:pending"` |
| `unblock:review:ok` | `REVIEW` comment exists with `Verdict: APPROVE` | `"BLOCKED: REVIEW comment with Verdict: APPROVE required"` |
| `unblock:review:rework` | `REVIEW` comment exists with `Verdict: NEEDS-REWORK` | `"BLOCKED: REVIEW comment with Verdict: NEEDS-REWORK required"` |
| `unblock:qa:ok` | `QA` comment exists with `Verdict: PASS` or `Verdict: PASS+FINDINGS` | `"BLOCKED: QA comment with PASS verdict required"` |
| `unblock:qa:rework` | `QA` comment exists with `Verdict: FAIL` | `"BLOCKED: QA comment with Verdict: FAIL required"` |
| `close` (issue) | `COMPLETED` or `BLOCKED` or `PAUSE` comment exists | `"BLOCKED: Terminal comment required before closing"` |

**Design rationale:** These rules make pipeline compliance structurally impossible to bypass. An agent that skips writing `COMPLETED` cannot apply `unblock:review:pending` — the MCP will reject it. This is enforcement at the infrastructure level, not at the instruction level.

---

## 11. File Structure

### Plugin root

```
unblock-plugin/
├── plugin.json                     # Plugin manifest (CC + Copilot CLI)
├── README.md
├── CHANGELOG.md
│
├── skills/                         # Unified entry point — CC and Copilot CLI
│   ├── setup/SKILL.md
│   ├── need/SKILL.md
│   ├── doctor/SKILL.md
│   ├── think/SKILL.md
│   ├── plan/SKILL.md
│   ├── do/SKILL.md
│   ├── make/SKILL.md
│   ├── use/SKILL.md
│   ├── info/SKILL.md
│   ├── trail/SKILL.md
│   ├── ship/SKILL.md
│   ├── pause/SKILL.md              # ⏸ pending unclaim
│   └── unblock-verify/SKILL.md    # Internal skill — invoked by supervisors
│
├── agents/                         # Core agents (shared CC + Copilot CLI)
│   ├── investigator.md             # Sherlock (CC format)
│   ├── architect.md                # Ada (CC format)
│   ├── linus.md                    # Code Reviewer (CC format)
│   ├── martin.md                   # Refactorer (CC format)
│   ├── quinn.md                    # QA Gate (CC format)
│   ├── fernando.md                 # Product Owner (CC format)
│   ├── inspector.md                # Gadget (CC format)
│   ├── investigator.agent.md       # Sherlock (Copilot CLI format)
│   ├── architect.agent.md          # Ada (Copilot CLI format)
│   ├── linus.agent.md              # Code Reviewer (Copilot CLI format)
│   ├── martin.agent.md             # Refactorer (Copilot CLI format)
│   ├── quinn.agent.md              # QA Gate (Copilot CLI format)
│   ├── fernando.agent.md           # Product Owner (Copilot CLI format)
│   └── inspector.agent.md          # Gadget (Copilot CLI format)
│
├── hooks/
│   ├── hooks.json                  # Hook configuration (Claude Code format)
│   ├── copilot-hooks.json          # Hook configuration (Copilot CLI format)
│   ├── session-start.sh
│   ├── inject-discipline.sh
│   ├── mcp-write-guard.sh
│   └── session-end.sh              # ⏸ full implementation pending unclaim
│
├── templates/
│   └── TEMPLATES.md                # Reference — all comment templates
│
└── setup/                          # Generated by /setup, not distributed
    ├── CLAUDE.md.template
    ├── copilot-instructions.md.template
    └── labels.json                 # All labels to create in GitHub
```

### Generated project files (by `/setup`)

**Claude Code:**
```
{project}/
├── .gitignore                      # worktrees/ added if not already present
├── CLAUDE.md                       # Project context + workflow rules
└── .claude/
    ├── hooks.json                  # Symlink or copy from plugin
    └── agents/                     # Supervisors created via /need
        └── {name}-supervisor.md
```

**Copilot CLI:**
```
{project}/
├── .gitignore                      # worktrees/ added if not already present
├── .github/
│   ├── copilot-instructions.md     # Workflow rules + MCP reference
│   └── agents/                     # Core agents + supervisors
│       └── {name}-supervisor.agent.md
└── .copilot/
    ├── mcp-config.json             # Unblock MCP server config
    └── hooks.json                  # Copilot CLI hook format
```

**Planning documents (by `/think` and `/plan`, committed to version control):**
```
{project}/
└── docs/
    ├── PRD.md                      # Product requirements — created in /think
    ├── PLAN.md                     # Phase overview index — created/updated in /plan
    ├── plans/
    │   ├── 01-plan-{slug}.md       # Phase detail — created in /plan
    │   └── 02-plan-{slug}.md
    └── research/
        └── YYYY-MM-DD-{topic}.md   # Research docs — created in /think
```

Note: Skills live in the plugin and are available to both platforms via the plugin manifest. No project-level skill files are generated — the plugin provides them.

**GitHub Actions (Teams / Enterprise — both platforms):**
```
{project}/
└── .github/
    └── workflows/
        ├── unblock-review.yml      # Triggers on unblock:review:pending label
        └── unblock-qa.yml          # Triggers on unblock:review:ok label
```

`unblock-review.yml` — detects `unblock:review:pending` label event, runs `/make review #N` via remote MCP.
`unblock-qa.yml` — detects `unblock:review:ok` label event, runs `/make qa #N` via remote MCP.

### Agent file discovery precedence

Supervisors generated via `/need` are project-specific and live in the project. Core agents (Sherlock, Ada, Linus, Martin, Quinn, Fernando, Gadget) live in the plugin and are never modified per-project.

**Claude Code precedence:** `.claude/agents/` (project) > plugin `agents/`
**Copilot CLI precedence:** `.github/agents/` (project) > plugin `agents/`

---

## 12. Plugin Distribution

### Claude Code

Distributed via Claude Code plugin marketplace.

```bash
# Install from official marketplace
/plugin install unblock@websublime

# Or via marketplace registry
/plugin marketplace add websublime/unblock-marketplace
/plugin install unblock@unblock-marketplace
```

Plugin manifest (`plugin.json`) declares:
- `skills/` → all skills (slash commands in CC via `SKILL.md`)
- `agents/` → core agents
- `hooks/hooks.json` → hook configuration
- MCP server: `unblock-mcp`

### Copilot CLI

Distributed via Copilot CLI plugin marketplace.

```bash
copilot plugin marketplace add websublime/unblock-marketplace
copilot plugin install unblock@unblock-marketplace
```

Plugin manifest (`plugin.json`) declares:
- `skills/` → all skills (same directory as CC)
- `agents/` → core agents (`.agent.md` format)
- `copilot-hooks.json` → hook configuration
- `.mcp.json` → MCP server config

### Skill frontmatter strategy

Skills use a single `SKILL.md` file that works on both platforms. CC-specific frontmatter fields are silently ignored by Copilot CLI — no errors, no breakage.

**Routing skills** (like `/do`, `/make`) spawn an isolated context but do not fix a single agent — routing depends on the intent received:

```yaml
---
name: do
description: Intent router for task execution. Routes to investigator, supervisor, linus, or quinn based on context. Stops and asks if intent is ambiguous.
disable-model-invocation: true   # CC + Copilot CLI: only developer invokes
context: fork                    # CC only: spawns isolated subagent context
---
```

**Atomic dispatch skills** (internal, invoked by `/do` routing) fix a specific agent:

```yaml
---
name: do-review
description: Internal — dispatches Linus for code review
context: fork
agent: linus
user-invocable: false            # Only /do router invokes this
---
```

**Claude Code** reads all frontmatter fields. `context: fork` on routing skills spawns an isolated context that handles routing and re-dispatches to atomic skills. `context: fork` + `agent:` on atomic skills gives deterministic single-agent dispatch.

**Copilot CLI** ignores `context: fork` and `agent:` as unknown fields. The skill body contains directive language and an explicit stop-and-ask instruction for ambiguous contexts:

```markdown
## Routing

Detect intent from the context provided:
- "review" / "revê" / PR in checkout → dispatch linus agent
- "investigate" / "investiga" → dispatch investigator agent  
- "implement" / "implementa" / issue number only → dispatch supervisor agent
- "qa" / "quality" → dispatch quinn agent
- Ambiguous → **STOP. Ask developer**: "Do you want to investigate, implement, review, or run QA on this issue?"

**BLOCK: Do not guess intent. If unclear, always ask first.**
```

**Result:** One file per skill. CC gets deterministic isolation via frontmatter. Copilot CLI gets strongly-directed routing with explicit stop-and-ask for ambiguity. Gadget verifies compliance regardless of platform.

### What is shared between platforms

- All skills (`skills/*/SKILL.md`) — same files, frontmatter interpreted by CC, ignored by Copilot CLI
- All agent logic and content (adapted to platform format)
- All hook scripts (`*.sh` files)
- All templates (referenced in agent instructions)
- MCP server (identical — Unblock MCP serves both)
- GitHub Actions (review / QA CI — platform-agnostic)

### What differs per platform

| Component | Claude Code | Copilot CLI |
|---|---|---|
| Skill dispatch | `context: fork` + `agent:` frontmatter | Directive body language |
| Agent format | `agents/*.md` | `agents/*.agent.md` |
| Hook config | `hooks/hooks.json` | `copilot-hooks.json` |
| MCP config | plugin.json declaration | `.mcp.json` |
| Context file | `CLAUDE.md` | `copilot-instructions.md` |

---

## Appendix A — Agent Dispatch Map

```
/think          → any agent at developer's discretion (no enforcement)
                  Sherlock (research) | Ada (spec/arch) | Fernando (PRD/requirements)
                  → optional: research doc, PRD.md, spec doc, or nothing
/plan "vision"  → Ada → PRD.md + PLAN.md (no issues yet)
/plan "phase N" → Ada → plans/NN-plan-xxx.md → [developer confirms] → Fernando (sequential) → PLAN.md updated
/do implement   → [Investigator → Gadget] → Supervisor → unblock-verify → Gadget
/do investigate → Investigator → Gadget
/do review      → Gadget → Linus → Gadget → [Martin → Gadget → Linus → Gadget]
                  → [Fernando if SUGGESTION findings on APPROVE]
/do qa          → Gadget → Quinn → Gadget
                  → [Fernando if MINOR/RISK/DEVIATES/EXTRA findings on PASS]
/do audit       → Gadget (on demand)
/do spec        → Architect (Ada)
/do spike       → Architect (Ada) in spike mode
/make           → any agent above, autonomous (Gadget included)
/use            → any named agent directly
```

**Gadget runs after every agent dispatch.** It is never skipped. Clean pipelines produce no noise — Gadget only writes `AUDIT` comments when violations are found.

### Post-COMPLETED pipeline orchestration

The pipeline after `COMPLETED` is label-driven, not session-driven. The Supervisor's session terminates after applying `unblock:review:pending`. Orchestration then diverges by mode:

**Solo developer:**
```
Supervisor writes COMPLETED → unblock:review:pending → session ends

Developer invokes /do "review #N"  → Linus → [Martin → Linus] → unblock:review:ok
                                      [Fernando if SUGGESTION findings on APPROVE]
Developer invokes /do "qa #N"      → Quinn → unblock:qa:ok
                                      [Fernando if MINOR/RISK/DEVIATES/EXTRA findings on PASS]
```

**Teams / Enterprise (CI):**
```
Supervisor writes COMPLETED → unblock:review:pending → session ends

CI detects review:pending   → /make review #N  → Linus → [Martin → Linus] → unblock:review:ok
                                                   [Fernando if SUGGESTION findings on APPROVE]
CI detects review:ok        → /make qa #N      → Quinn → unblock:qa:ok
                                                   [Fernando if MINOR/RISK/DEVIATES/EXTRA findings on PASS]
```

In both cases `unblock:review:pending` is the sole handoff signal between sessions. The label is the medium — no session stays alive to orchestrate.

---

## Appendix B — Full Label List

```
unblock:review:pending
unblock:review:ok
unblock:review:rework
unblock:qa:ok
unblock:qa:rework
unblock:needs-human
unblock:paused
unblock:no-investigation
unblock:finding:suggestion
unblock:finding:minor
unblock:finding:risk
unblock:finding:deviation
unblock:finding:extra
```

Total: 13 labels. All created by `/setup`. All prefixed with `unblock:` to avoid clash with existing project or organisation labels.

Rework-triggering severities (CRITICAL, WARNING from REVIEW; BLOCKER, MAJOR from QA) never become finding issues — they are resolved via the rework cycle and documented in the issue's comment trail.

---

## Appendix C — Session Isolation Rules

| Transition | Allowed in same session | Behaviour if violated |
|---|---|---|
| `/think` → `/plan` | ✅ | — |
| `/plan` → `/do` | ⚠️ | Large warning, developer must confirm |
| `/plan` → `/make` | ⚠️ | Large warning, developer must confirm |
| Investigation → Implementation | ❌ | Hard stop — new session required |
| Implementation → Review | ❌ | Developer invokes `/do "review #N"` in a new session — Linus runs in isolated subagent (via `context: fork` in CC) |
| Review → QA | ❌ | Developer invokes `/do "qa #N"` in a new session — Quinn runs in isolated subagent (via `context: fork` in CC) |
| Any → `/think` | ✅ | — |
| Any → `/info` | ✅ | — |
| Any → `/trail` | ✅ | — |
| Any → `/ship` | ✅ | — |

The label `unblock:review:pending` is the only handoff mechanism between sessions. No session stays alive to orchestrate the next step.
