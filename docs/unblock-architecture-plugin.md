# Unblock Plugin — Architecture Specification

**Detailed design for the structured development pipeline plugin.**

| | |
|---|---|
| **Version** | 0.1.0-draft |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Date** | March 2026 |
| **Status** | Draft |
| **Depends on** | `unblock-prd-plugin.md` v0.1.0-draft, `unblock-architecture-github.md` v1.1.0-draft |
| **Derived from** | `mister-anderson` plugin (websublime/mister-anderson) |

---

## Table of Contents

1. [Overview](#1-overview)
2. [beads → MCP Translation](#2-beads--mcp-translation)
3. [Agent Specifications](#3-agent-specifications)
4. [Skill Specifications](#4-skill-specifications)
5. [Templates](#5-templates)
6. [Hooks](#6-hooks)
7. [Discovery System](#7-discovery-system)
8. [GitHub Actions](#8-github-actions)
9. [Plugin Lifecycle](#9-plugin-lifecycle)
10. [Platform Adaptation](#10-platform-adaptation)

---

## 1. Overview

This document specifies the implementation details for the Unblock Plugin — the structured development pipeline described in `unblock-prd-plugin.md`. Where the PRD defines **what** the plugin does, this document defines **how** each component is built.

The plugin is a collection of `.md` configuration files (agents, skills), shell scripts (hooks), and templates. It contains zero compiled code. All task state operations go through the Unblock MCP server.

### 1.1 Key Architectural Difference from mister-anderson

The mister-anderson plugin uses `bd` CLI commands (beads) backed by a Dolt SQL database. The Unblock plugin uses MCP tool calls backed by GitHub Issues + Projects V2. This changes:

- **Command syntax** — `bd show {ID}` becomes `show {id}` MCP tool call
- **Identifiers** — beads use string IDs (e.g., `TASK-42`), unblock uses GitHub issue numbers (e.g., `42`)
- **Branch naming** — `bd-{BEAD_ID}` becomes `issue-{number}-{slug}`
- **Status transitions** — beads use `bd update --status`, unblock uses `update --status` via Projects V2 field
- **Dependencies** — beads use `bd dep add`, unblock uses `depends` MCP tool with GitHub's native blocking relationships
- **Comments** — beads use `bd comments add`, unblock uses `comment` MCP tool writing to GitHub Issue comments
- **No local database** — beads require Dolt sql-server. Unblock requires only `GITHUB_TOKEN`

### 1.2 Design Principles

| Principle | Meaning |
|---|---|
| GitHub is the source of truth | All state lives in GitHub Issues, Projects V2, and comments. Zero local storage |
| MCP tools are the only interface | Agents never call GitHub API directly. All operations go through Unblock MCP tools |
| Agents are configuration | Every agent is a `.md` file with role, instructions, tools, and model. No compiled code |
| Templates are injectable | Workflow templates are injected into generated supervisors, not inherited |
| Hooks are shell scripts | Simple, debuggable, no runtime dependencies beyond bash and the MCP server |

---

## 2. beads → MCP Translation

Canonical translation table. All agent and skill files MUST use the Unblock MCP tool syntax, never `bd` CLI commands.

### 2.1 Read Operations

| beads (bd CLI) | Unblock (MCP tool) | Notes |
|---|---|---|
| `bd show {ID}` | `show {id}` | Returns issue with body sections, fields, deps |
| `bd show {ID}` (with comments) | `show {id} --include_comments` | Includes full comment trail |
| `bd comments {ID}` | `show {id} --include_comments` | No separate comments-only command |
| `bd list` | `list` | Supports filters: `--status`, `--priority`, `--label`, `--milestone` |
| `bd list --label needs-review` | `list --label needs-review` | Filter by label |
| `bd ready` | `ready` | Returns issues with no active blockers |
| `bd search {query}` | `search {query}` | Full-text via GitHub search API |
| `bd stats` | `stats` | Aggregates by status, priority, agents |

### 2.2 Write Operations

| beads (bd CLI) | Unblock (MCP tool) | Notes |
|---|---|---|
| `bd create --title ... --deps ...` | `create --title ... --blocked_by ...` | `--deps` becomes `--blocked_by` |
| `bd create --parent {ID}` | `create --parent {id}` | Sub-Issue relationship |
| `bd update {ID} --status in_progress` | `update {id} --status in_progress` | Via Projects V2 field |
| `bd update {ID} --assignee rust-supervisor` | `update {id} --assignees rust-supervisor` | Assignee field |
| `bd label add {ID} needs-review` | `update {id} --labels_add needs-review` | Labels via update |
| `bd label remove {ID} needs-review` | `update {id} --labels_remove needs-review` | Labels via update |
| `bd comments add {ID} "..."` | `comment {id} "..."` | Writes GitHub Issue comment |
| `bd close {ID}` | `close {id}` | Agents should NEVER use this (Rule 6) |
| `bd dep add {ID} --blocks {TARGET}` | `depends --source {target} --target {id}` | Note: direction is reversed — `depends` means "source is blocked by target" |
| `bd claim {ID}` | `claim {id}` | Sets agent, status, timestamp atomically |

### 2.3 Branch Naming

| beads | Unblock |
|---|---|
| `bd-{BEAD_ID}` (e.g., `bd-TASK-42`) | `issue-{number}-{slug}` (e.g., `issue-42-implement-rate-limiter`) |

Branch type prefixes from beads (`feat/`, `fix/`, `chore/`) are embedded in the slug, not as path prefix:
- Feature: `issue-42-add-rate-limiter`
- Bug fix: `issue-43-fix-config-parser`
- Chore: `issue-44-update-deps`

### 2.4 Identifiers

| beads | Unblock |
|---|---|
| String ID: `TASK-42`, `BUG-7` | Integer: `42`, `7` |
| Referenced as `{BEAD_ID}` in templates | Referenced as `{ISSUE_NUMBER}` or `#42` in templates |

---

## 3. Agent Specifications

Each agent is a `.md` file in `plugin/agents/`. The file defines the agent's persona, model, tools, instructions, and output format.

### 3.1 Agent File Format

```markdown
---
name: {agent-name}
model: {opus|sonnet|haiku}
tools:
  - {tool1}
  - {tool2}
---

# {Persona Name} — {Role Title}

## Identity
{Who this agent is and what it does}

## Instructions
{Step-by-step process}

## Output Format
{What this agent produces}

## Boundaries
{What this agent can and cannot do}
```

### 3.2 Model Selection Rationale

| Model | Agents | Rationale |
|---|---|---|
| **opus** | Grace, Ada, Linus, Quinn | Complex reasoning: PRD elicitation, system design, code review judgement, spec conformity analysis |
| **sonnet** | Fernando, Sherlock, Daphne, Martin, Supervisors | Execution tasks: issue creation, codebase search, tech detection, targeted fixes, implementation |

### 3.3 Tool Permissions Matrix

| Agent | Read | Write | Edit | Glob | Grep | Bash | MCP (unblock) | WebFetch |
|---|---|---|---|---|---|---|---|---|
| Grace (PM) | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| Ada (Architect) | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| Fernando (PO) | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ❌ |
| Sherlock (Research) | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ (read-only) | ✅ (comment only) | ❌ |
| Daphne (Discovery) | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ | ✅ |
| Linus (Reviewer) | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ (read-only) | ✅ (comment only) | ❌ |
| Quinn (QA) | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ (test/build/lint) | ✅ (comment only) | ❌ |
| Martin (Refactorer) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ (comment only) | ❌ |
| Supervisors | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ (comment, update) | ❌ |

### 3.4 Comment Formats

Each agent type produces structured comments via the `comment` MCP tool. These formats MUST be followed exactly — review and QA agents parse them for context reconstruction.

#### INVESTIGATION (Sherlock)

```
INVESTIGATION:

Root cause: {what was found}

Files:
- {file_path}:{line_number} — {what's relevant}
- {file_path}:{line_number} — {what's relevant}

Approach:
1. {step}
2. {step}

Risks:
- {risk description}

Related tests:
- {test_file}:{test_name}
```

#### DECISION (Supervisors)

```
DECISION: {choice made}

Context: {what was the question}
Options considered: {alternatives}
Chosen: {what was picked}
Reason: {why}
```

#### DEVIATION (Supervisors)

```
DEVIATION: {what differs from spec}

Spec said: {original requirement}
Implementation does: {what was actually done}
Reason: {why the deviation was necessary}
```

#### COMPLETED (Supervisors)

```
COMPLETED:

Summary: {what was implemented}
Files changed: {file1}, {file2}, {file3}
Decisions: {N} (see DECISION comments above)
Deviations: {N} — {brief or "implemented as spec"}
Tests: {test command} passes. {additional verification}
```

#### REVIEW (Linus)

```
REVIEW:

## Acceptance Criteria
- [PASS] {criterion} — {evidence}
- [FAIL] {criterion} — {what's missing}

## Findings
- [CRITICAL] {file}:{line} — {description}
- [WARNING] {file}:{line} — {description}
- [SUGGESTION] {file}:{line} — {description}
- [GOOD] {description}

## Security
{assessment or "No issues found"}

## Performance
{assessment or "No issues found"}

## Tests
{coverage assessment}

## Verdict: {APPROVE|NEEDS-REFACTORING|NEEDS-REWORK}
{rationale}
```

#### REFACTORING (Martin)

```
REFACTORING:

## Fixed
- {finding} — {what was done}

## Deferred
- {finding} — TODO({ISSUE_NUMBER}): {reason}

## False Positive
- {finding} — {why it's not actually a problem}

## Skipped
- {finding} — {reason for skipping}

Tests: {test command} — {PASS|FAIL}
Behavior preserved: {yes|no — explanation}
```

#### QA (Quinn)

```
QA:

## Spec Conformity
- [CONFORMS] {requirement} — {evidence}
- [DEVIATES] {requirement} — {how it deviates}
- [MISSING] {requirement} — {not implemented}
- [EXTRA] {what was added beyond spec}

## User Stories
- [PASS] {criterion} — {evidence}
- [FAIL] {criterion} — {what fails}

## Boundaries & Edge Cases
- {scenario} — {result}

## Decision Trail
Decisions logged: {N}
Deviations logged: {N}
Unlogged deviations found: {N or "none"}

## Tests: {PASS|FAIL}
{output summary}

## Build: {PASS|FAIL}
{output summary}

## Lint: {PASS|FAIL}
{output summary}

## Verdict: {PASS|FAIL}
{rationale}
{if FAIL: severity per item — BLOCKER|MAJOR|MINOR}
```

#### PROGRESS (PreCompact hook)

```
PROGRESS:

Working on: {issue title}
Files touched: {list from git status}
Status: {current implementation state}
Next steps: {what remains}
```

#### REWORK (Orchestrator)

```
REWORK:

Source: {REVIEW|QA} comment
Issues to address:
1. {finding with severity}
2. {finding with severity}
```

---

## 4. Skill Specifications

Each skill is a `SKILL.md` file in `plugin/skills/{skill-name}/`. Skills orchestrate one phase of the pipeline by dispatching agents and coordinating MCP tool calls.

### 4.1 Skill File Format

Skills follow Claude Code's skill format with frontmatter and markdown instructions.

### 4.2 Skill Dispatch Pattern

All skills that dispatch agents use Claude Code's `Task()` tool:

```
Agent(subagent_type: "{agent-name}", prompt: "...")
```

The prompt MUST include:
- Issue number (when applicable)
- Full context (what to read, what to do)
- Output expectations (what comment to log, what to return)

### 4.3 `/setup-project` — Phases

1. **Check state:** Is Unblock MCP server accessible? Are Projects V2 fields configured?
   - If no MCP server: error with install instructions
   - If no fields: run `setup` MCP tool
2. **Create "Review Findings" milestone** via `create --type epic --title "Review Findings"`. Store milestone number in CLAUDE.md
3. **Detect tech stack** — scan for `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`, `Dockerfile`, etc.
4. **Dispatch Daphne (Discovery)** with detected technologies → generates supervisors in `.claude/agents/`
5. **Copy GitHub Actions** — `unblock-review.yml` and `unblock-qa.yml` to `.github/workflows/`
6. **Generate editor configs:**
   - `.github/copilot-instructions.md` from template
   - `.vscode/mcp.json` with Unblock server config
   - `.cursor/rules/unblock.mdc` if `.cursor/` exists
   - `.windsurfrules` if Windsurf detected
7. **Generate CLAUDE.md** from `templates/CLAUDE.md.template` with project-specific values
8. **Generate AGENTS.md** from `templates/AGENTS.md.template` with core + generated supervisors
9. **Write version file** `.claude/.unblock-plugin-version`

### 4.4 `/start-task` — Phases

1. **Resolve issue** — from argument `#42` or present `ready` list for selection
2. **Read context** — `show {id} --include_comments`. Parse body sections, status, labels, milestone. If Sub-Issue, read parent for epic context
3. **Resolve supervisor** — read assignees from issue. Map to `{assignee}-supervisor.md` in `.claude/agents/`. If missing, suggest `/add-supervisor`
4. **Investigation** — search comments for `INVESTIGATION:`. If missing, ask user: "Investigate first or skip?" If investigate, dispatch Sherlock
5. **Branch** — check `git branch -a | grep issue-{number}`. If exists, ask continue or fresh. Create branch `issue-{number}-{slug}` from base
6. **Dispatch supervisor** — full context: issue number, epic context, base branch, investigation findings

### 4.5 `/rework-task` — Phases

1. **Resolve issue** — from argument or `list --label needs-rework`
2. **Read context** — extract REVIEW or QA comment with findings
3. **Resolve supervisor** — same as start-task
4. **Checkout existing branch** — no new branch, no investigation
5. **Dispatch supervisor** — with findings as context: "Fix these specific issues: [list]"
6. **After dispatch** — push + `update {id} --labels_add needs-review`

### 4.6 `/review-task` — Phases

1. **Resolve issue** — from argument or `list --label needs-review`
2. **Read context** — verify COMPLETED comment exists. Identify implementation branch
3. **Dispatch Linus** — analyzes diff, logs REVIEW comment
4. **Process verdict:**
   - APPROVE → `update {id} --labels_remove needs-review --labels_add approved`. Track findings via Fernando if non-GOOD items
   - NEEDS-REFACTORING → track findings. Ask to dispatch Martin. If yes, Martin fixes → re-dispatch Linus (loop)
   - NEEDS-REWORK → `update {id} --labels_remove needs-review --labels_add needs-rework`. Track findings. Log REWORK comment

### 4.7 `/qa-task` — Phases

1. **Resolve issue** — from argument or `list --label approved`
2. **Read context** — verify REVIEW with APPROVE verdict. Locate spec/design doc
3. **Dispatch Quinn** — with spec path, PRD path, full context
4. **Process verdict:**
   - PASS → `update {id} --labels_add qa-passed`. Track findings if non-positive items. Notify developer: "Ready to merge"
   - FAIL → `update {id} --labels_add needs-rework`. Track findings. Notify with failure reasons

### 4.8 `/create-tasks` — Phases

1. **Locate PRD** — ask for path, verify APPROVED status
2. **Locate spec/plan** — ask for architecture spec or product plan (if available)
3. **Dispatch Fernando** — "Create epics and issues from this PRD + spec":
   - Epics → milestones or parent issues
   - Tasks → issues with `--blocked_by` deps, `--priority`, `--labels`, `--assignee` (supervisor)
   - Acceptance criteria from spec → issue body
4. **Dry run** — Fernando presents plan before creating. User approves or adjusts
5. **Execute** — Fernando creates all issues

### 4.9 `/create-issue` — Phases

1. **Gather context** — from argument or ask user for description, docs
2. **Dispatch Fernando** — "Create a single issue with full context":
   - Interactive: Fernando asks for title, type, priority, acceptance criteria
   - Sets `--blocked_by` if dependencies mentioned
   - Sets `--assignee` for supervisor routing
3. **Confirm** — show created issue number and details

### 4.10 `/product-requirements` — Phases

1. **Gather context** — ask for existing docs, raw idea
2. **Define output path** — suggest `docs/{name}-prd.md`
3. **Dispatch Grace** — with idea + doc paths
4. **After dispatch** — verify PRD created, show DRAFT/APPROVED status

### 4.11 `/architect-solution` — Phases

1. **Locate PRD** — ask for path, verify APPROVED status
2. **Define output path** — suggest `docs/{name}-architecture.md`
3. **Dispatch Ada** — with PRD path + spec output path
4. **After dispatch** — verify spec created, show DRAFT/APPROVED status

### 4.12 `/add-supervisor` — Phases

1. **Resolve technology** — from argument or ask user
2. **Verify** — check `.claude/agents/` for existing `{tech}-supervisor.md`
3. **Dispatch Daphne** — in on-demand mode for single supervisor
4. **Confirm** — show created supervisor with persona name

### 4.13 `/update-plugin` — Phases

1. **Version check** — read `.claude/.unblock-plugin-version`, compare with installed plugin version from `plugin.json`
2. **Detect legacy files** — scan `.claude/agents/` for core agent copies, scan local skills/hooks directories
3. **Cleanup** — remove legacy copies, preserve dynamic supervisors (files ending in `-supervisor.md` that are NOT `refactoring-supervisor.md`)
4. **Post-update verification** — verify supervisors intact, no legacy duplicates
5. **Optional refresh** — ask to re-run Daphne to refresh supervisors with latest external content
6. **Notify** — remind user to refresh plugin cache in Claude Code

---

## 5. Templates

### 5.1 UNBLOCK-WORKFLOW.md

Injected at the beginning of each generated supervisor by Daphne. This is the equivalent of BEADS-WORKFLOW.md from mister-anderson, translated to Unblock MCP tools.

```markdown
<unblock-workflow>
<on-task-start>
  1. Parse ISSUE_NUMBER from dispatch prompt
  2. Check git status — warn if uncommitted changes
  3. Checkout BASE_BRANCH (from dispatch prompt, default: main)
  4. Create branch: issue-{ISSUE_NUMBER}-{slug-from-title}
  5. Set status: call `update {ISSUE_NUMBER} --status in_progress` MCP tool
  6. Read issue: call `show {ISSUE_NUMBER} --include_comments` MCP tool
  7. If issue has parent (Sub-Issue): read parent for epic/design context
  8. Invoke discipline: Skill(skill: "subagents-discipline")

<execute-with-confidence>
  Default: Execute based on issue comments (INVESTIGATION, DECISION)
  Only deviate if clear evidence the approach is wrong
  Log deviations via `comment` MCP tool

<during-implementation>
  - Work only in your branch
  - Commit frequently with descriptive messages (conventional commits)
  - Log non-trivial decisions: `comment {ISSUE_NUMBER} "DECISION: ..."`
  - Log spec deviations: `comment {ISSUE_NUMBER} "DEVIATION: ..."`

<self-check-loop>
  Max 3 iterations:
  1. Run tests → if fail → fix → re-run
  2. Run build → if fail → fix → re-run
  3. Run lint → if fail → fix → re-run
  4. Diff review: read own diff against acceptance criteria
     → if criterion missing → implement → re-check
  5. All pass → exit loop

<on-completion>
  1. Commit all changes
  2. Log completion: `comment {ISSUE_NUMBER} "COMPLETED: Summary: [what]. Files: [list]. Decisions: [N]. Deviations: [N]. Tests: [how verified]."`
  3. Push to remote: `git push origin issue-{ISSUE_NUMBER}-{slug}`
  4. Add review label: `update {ISSUE_NUMBER} --labels_add needs-review`
  5. Return completion report to orchestrator

<banned>
  - Working directly on main branch
  - Implementing without ISSUE_NUMBER
  - Merging your own branch
  - Editing files outside project
  - Closing issues (use `update --labels_add needs-review` instead)
  - Skipping self-check loop
  - Using `close` MCP tool
</unblock-workflow>
```

### 5.2 CLAUDE.md Template

Generated by `/setup-project`. Placeholders in `{CAPS}`.

```markdown
# {PROJECT_NAME}

## Project Overview
{PROJECT_DESCRIPTION}

## Tech Stack
- Languages: {LANGUAGES}
- Libraries: {DEPENDENCIES}
- Infrastructure: {INFRASTRUCTURE}

## Task Tracking
This project uses ://unblock for dependency-aware task tracking. All task state lives in GitHub Issues + Projects V2 via the Unblock MCP server.

### MCP Tools Available
ready, claim, close, create, update, show, comment, depends, dep_remove,
dep_cycles, list, search, stats, prime, setup, reopen, doctor.

Use these tools for ALL task operations. Never modify issue state directly via GitHub API.

### Review Findings Milestone
Finding issues are tracked under the "Review Findings" milestone (#{MILESTONE_NUMBER}).

## Supervisors
{SUPERVISOR_LIST}

## Your Identity
- Orchestrate work through skills and agents
- Constructive skeptic — discuss before acting
- Follow skill instructions exactly
- NEVER use `isolation: "worktree"` when dispatching agents
- Ask user if unclear

## Commit Strategy
- Atomic commits as you go
- Conventional commit messages
- Tests must pass before every commit
- Fix code, not tests

## Quality Gate
- Lint: `{LINT_COMMAND}`
- Format: `{FORMAT_COMMAND}`
- Test: `{TEST_COMMAND}`
- Build: `{BUILD_COMMAND}`
```

### 5.3 AGENTS.md Template

Generated by `/setup-project`. Provides workflow guidance for agents and developers.

```markdown
# Agents — {PROJECT_NAME}

## Workflow

### Starting work
1. Call `prime` to see current task status
2. Call `ready` to find unblocked work
3. Call `show #issue --include_comments` to read full context
4. Call `claim #issue` to take ownership

### During implementation
- Branch naming: `issue-{number}-{slug}`
- Log decisions: `comment #issue "DECISION: ..."`
- Log deviations: `comment #issue "DEVIATION: ..."`
- Self-check before pushing: tests, build, lint, acceptance criteria

### After implementation
1. Log completion: `comment #issue "COMPLETED: ..."`
2. Push branch
3. Add label: `update #issue --labels_add needs-review`
4. STOP — review and QA happen automatically

### Rework
1. `show #issue --include_comments` — read REVIEW or QA findings
2. Checkout existing branch
3. Fix specific issues from findings
4. Self-check loop
5. Push + `update #issue --labels_add needs-review`

## Issue Types
- `bug` — defect report
- `feature` — new functionality
- `task` — implementation work item
- `epic` — collection of related issues (mapped to milestone or parent issue)
- `chore` — maintenance, deps, config

## Priorities
- `P0` — critical, drop everything
- `P1` — high, current sprint
- `P2` — medium, next sprint
- `P3` — low, backlog
- `P4` — nice to have

## Available Agents

### Core (provided by plugin)
{CORE_AGENTS_LIST}

### Implementation Supervisors (project-specific)
{SUPERVISOR_AGENTS_LIST}

## Session Completion Checklist
- [ ] All changes committed
- [ ] Tests pass
- [ ] Build passes
- [ ] Lint passes
- [ ] Branch pushed to remote
- [ ] Issue status updated
- [ ] Completion comment logged
```

---

## 6. Hooks

### 6.1 hooks.json

```json
{
  "PreToolUse": [
    {
      "matcher": "Task",
      "hooks": [{
        "type": "command",
        "command": "${CLAUDE_PLUGIN_ROOT}/hooks/inject-discipline-reminder.sh"
      }]
    }
  ],
  "SessionStart": [
    {
      "hooks": [{
        "type": "command",
        "command": "${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh"
      }]
    }
  ],
  "PreCompact": [
    {
      "hooks": [{
        "type": "command",
        "command": "${CLAUDE_PLUGIN_ROOT}/hooks/pre-compact.sh"
      }]
    }
  ]
}
```

### 6.2 session-start.sh

```bash
#!/bin/bash
# SessionStart hook — show task dashboard via prime MCP tool

# Check if Unblock MCP server is available
if ! command -v unblock-mcp &> /dev/null; then
  echo "⚠️  unblock-mcp not found. Install: curl ... | sh or npx @unblock/cli"
  exit 0
fi

# Check for uncommitted changes on main
CURRENT_BRANCH=$(git branch --show-current 2>/dev/null)
if [ "$CURRENT_BRANCH" = "main" ] || [ "$CURRENT_BRANCH" = "master" ]; then
  DIRTY=$(git status --porcelain 2>/dev/null | head -1)
  if [ -n "$DIRTY" ]; then
    echo "⚠️  Uncommitted changes on $CURRENT_BRANCH. Commit or stash before starting work."
  fi
fi

# Check for plugin updates
VERSION_FILE=".claude/.unblock-plugin-version"
if [ -f "$VERSION_FILE" ]; then
  LOCAL_VERSION=$(cat "$VERSION_FILE")
  PLUGIN_VERSION=$(cat "${CLAUDE_PLUGIN_ROOT}/.claude-plugin/plugin.json" 2>/dev/null | grep '"version"' | head -1 | sed 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
  if [ -n "$PLUGIN_VERSION" ] && [ "$LOCAL_VERSION" != "$PLUGIN_VERSION" ]; then
    echo "📦 Plugin update available: $LOCAL_VERSION → $PLUGIN_VERSION. Run /update-plugin"
  fi
fi

# Inject prime context reminder
echo "Call \`prime\` to see current task status (in-progress, ready, blocked, needs-rework)."
```

### 6.3 inject-discipline-reminder.sh

```bash
#!/bin/bash
# PreToolUse hook — inject discipline for supervisor dispatches
# Reads JSON input from stdin, checks if dispatching a supervisor

INPUT=$(cat)
TOOL_NAME=$(echo "$INPUT" | grep -o '"tool_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"tool_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')

if [ "$TOOL_NAME" != "Task" ]; then
  exit 0
fi

SUBAGENT_TYPE=$(echo "$INPUT" | grep -o '"subagent_type"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"subagent_type"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')

if echo "$SUBAGENT_TYPE" | grep -q '\-supervisor$'; then
  echo "<system-reminder>SUPERVISOR DISPATCH: Before implementing, invoke Skill(skill: \"subagents-discipline\") to load the engineering discipline rules. Follow them exactly.</system-reminder>"
fi
```

### 6.4 pre-compact.sh

```bash
#!/bin/bash
# PreCompact hook — preserve work-in-progress state before context compaction

# Only act if there's an active issue (check for issue branch)
CURRENT_BRANCH=$(git branch --show-current 2>/dev/null)

if echo "$CURRENT_BRANCH" | grep -q '^issue-'; then
  ISSUE_NUMBER=$(echo "$CURRENT_BRANCH" | sed 's/^issue-\([0-9]*\).*/\1/')
  FILES_TOUCHED=$(git diff --name-only HEAD 2>/dev/null | head -20 | tr '\n' ', ' | sed 's/,$//')

  if [ -n "$ISSUE_NUMBER" ] && [ -n "$FILES_TOUCHED" ]; then
    echo "<system-reminder>CONTEXT COMPACTION: You are working on issue #$ISSUE_NUMBER (branch: $CURRENT_BRANCH). Files touched: $FILES_TOUCHED. Log a PROGRESS comment to preserve state: comment $ISSUE_NUMBER \"PROGRESS: Working on: [current task]. Files touched: $FILES_TOUCHED. Status: [describe current state]. Next steps: [what remains].\"</system-reminder>"
  fi
fi
```

---

## 7. Discovery System

### 7.1 Daphne (Discovery Agent) — Operation Modes

**Full Scan Mode** — dispatched by `/setup-project`:
1. Scan codebase for tech indicators (see §7.2)
2. For each detected technology, fetch specialist content from external directory
3. Filter fetched content (see §7.3)
4. Generate dynamic persona name for each supervisor
5. Inject UNBLOCK-WORKFLOW.md template (see §5.1) at the beginning of each supervisor
6. Write supervisor files to `.claude/agents/`
7. Update CLAUDE.md and AGENTS.md with supervisor list

**On-Demand Mode** — dispatched by `/add-supervisor`:
1. Accept technology name from argument
2. Verify supervisor doesn't already exist in `.claude/agents/`
3. Follow steps 2-6 from Full Scan for single technology

### 7.2 Tech Detection Table

| Indicator File | Technology | Supervisor Name |
|---|---|---|
| `Cargo.toml` | Rust | `rust-supervisor` |
| `package.json` | Node.js | `node-supervisor` |
| `package.json` + `react` dep | React | `react-supervisor` |
| `package.json` + `vue` dep | Vue | `vue-supervisor` |
| `package.json` + `next` dep | Next.js | `nextjs-supervisor` |
| `go.mod` | Go | `go-supervisor` |
| `pyproject.toml` / `setup.py` | Python | `python-supervisor` |
| `Gemfile` | Ruby | `ruby-supervisor` |
| `Dockerfile` / `docker-compose.yml` | Docker/DevOps | `devops-supervisor` |
| `pubspec.yaml` | Flutter/Dart | `flutter-supervisor` |
| `*.xcodeproj` / `Package.swift` | iOS/Swift | `ios-supervisor` |
| `build.gradle` / `build.gradle.kts` | Android/Kotlin | `android-supervisor` |
| `terraform/*.tf` | Terraform | `terraform-supervisor` |

Multiple technologies can be detected in a single project (e.g., a full-stack app gets both `react-supervisor` and `node-supervisor`).

### 7.3 Content Filtering Rules

External specialist content is fetched via WebFetch and filtered before injection:

| Rule | Action |
|---|---|
| Code blocks > 3 lines | Remove — supervisors should reference project files, not contain generic code |
| Tutorial / example sections | Remove — keep only standards, conventions, and scope definitions |
| Installation / setup guides | Remove — not relevant for implementation |
| Technology version specifics | Keep — important for correct API usage |
| Naming conventions | Keep — critical for code consistency |
| File structure patterns | Keep — guides where to place new code |
| Testing patterns | Keep — guides how to write tests |

**Target size:** 80-120 lines of specialist content per supervisor. Total supervisor file (with UNBLOCK-WORKFLOW.md injected): 150-220 lines.

### 7.4 Dynamic Persona Names

Instead of a fixed mapping table, Daphne generates a persona name at supervisor creation time. The name:
- Is a human first name
- Is unique within the project (no duplicates across supervisors)
- Is written into the supervisor `.md` file header
- Is listed in AGENTS.md under "Implementation Supervisors"

### 7.5 External Agent Directory

The plugin reuses the same external agent directory as mister-anderson. This directory contains technology-specific specialist content (conventions, patterns, standards) that is language/framework-specific but workflow-agnostic.

The UNBLOCK-WORKFLOW.md template (§5.1) provides the workflow-specific content. The external directory provides the technology-specific content. Together they form a complete supervisor.

If the external directory becomes unavailable, Daphne falls back to generating supervisors from local project analysis only (Cargo.toml structure, test commands, lint config, etc.) without specialist content.

---

## 8. GitHub Actions

The GitHub Action workflows are defined in the PRD §12. This section adds implementation notes.

### 8.1 Review Action — Implementation Notes

- The action triggers on `issues.labeled` event with `needs-review` label
- Branch discovery uses `git branch -r --list "*issue-{number}*"` — must handle multiple matches (pick most recent)
- Claude Code runs with `--print` flag for non-interactive execution
- The `GITHUB_TOKEN` provided by Actions has sufficient permissions for issue comments and label changes
- Timeout of 30 minutes prevents runaway sessions

### 8.2 QA Action — Implementation Notes

- Same trigger pattern as review but on `approved` label
- Installs project dependencies before running QA (auto-detect: Cargo.toml → rustup, package.json → npm ci)
- Quinn runs tests, build, and lint as part of validation — these need the correct runtime installed

### 8.3 Findings Tracking in CI

When review or QA produces non-positive findings, Fernando is dispatched within the same CI session to create tracking issues:

1. Parse REVIEW or QA comment for non-positive items
2. Read "Review Findings" milestone number from CLAUDE.md
3. For each finding:
   - Search existing open issues for duplicates (by file path + similar title)
   - If match: add comment linking back, add `blocked_by` if not linked
   - If no match: `create --title "..." --milestone {milestone} --blocked_by {reviewed_issue} --labels finding:{severity} --priority {mapped}`
4. Priority mapping: `[SUGGESTION]` → P3, `[WARNING]` → P2, `[CRITICAL]` → P1

---

## 9. Plugin Lifecycle

### 9.1 Version Tracking

```
plugin/.claude-plugin/plugin.json    → canonical version (e.g., "0.2.0")
.claude/.unblock-plugin-version      → installed version per project (written by setup/update)
```

### 9.2 Update Detection

The SessionStart hook compares the two version files. If they differ, it notifies the user to run `/update-plugin`.

### 9.3 Update Flow

1. User runs `/update-plugin`
2. Skill compares versions
3. If update available:
   - Detect legacy local copies (core agents in `.claude/agents/` that match plugin agent names)
   - Remove legacy copies (preserve any file ending in `-supervisor.md` except `refactoring-supervisor.md`)
   - Update `.claude/.unblock-plugin-version`
4. Optionally re-run Daphne to refresh supervisors with latest external content
5. Remind user to restart Claude Code to pick up plugin changes

### 9.4 What Is Preserved on Update

| Component | Preserved | Reason |
|---|---|---|
| Implementation supervisors (`.claude/agents/*-supervisor.md`) | ✅ | Project-specific, customised by developer |
| CLAUDE.md | ✅ | Project-specific content |
| AGENTS.md | ✅ | May have manual edits |
| `.github/workflows/unblock-*.yml` | ✅ | May have manual customisations |
| `.github/copilot-instructions.md` | ✅ | May have manual edits |
| Core agents (if copied locally) | ❌ removed | Now provided by plugin |
| Legacy skills (if copied locally) | ❌ removed | Now provided by plugin |
| Legacy hooks (if copied locally) | ❌ removed | Now provided by plugin |

---

## 10. Platform Adaptation

### 10.1 Claude Code (Full Experience)

All components active:
- Plugin system with agents, skills, hooks
- `Task()` dispatch for sub-agents with persona enforcement
- SessionStart, PreToolUse, PreCompact hooks
- MCP tools via plugin config

### 10.2 GitHub Copilot (MCP + Custom Instructions)

Generated by `/setup-project` step 6:
- `.vscode/mcp.json` — Unblock MCP server config
- `.github/copilot-instructions.md` — contains: workflow rules, discipline, MCP tool reference, supervisor conventions

**What's more manual:**
- No auto-context on session start (developer calls `prime` manually)
- No discipline injection per dispatch (rules in custom instructions, read once)
- No sub-agent personas (developer instructs Copilot directly)
- No slash commands (developer describes intent in natural language)

### 10.3 Cursor (MCP + Rules)

Generated by `/setup-project` step 6:
- MCP config in Cursor's format
- `.cursor/rules/unblock.mdc` — same content as `copilot-instructions.md` adapted to Cursor rules format

### 10.4 Windsurf (MCP + Rules)

Generated by `/setup-project` step 6:
- MCP config in Windsurf's format
- `.windsurfrules` — same content adapted to Windsurf format

### 10.5 Platform Feature Matrix

| Feature | Claude Code | Copilot | Cursor | Windsurf |
|---|---|---|---|---|
| MCP tools | ✅ plugin config | ✅ `.vscode/mcp.json` | ✅ MCP config | ✅ MCP config |
| Workflow guidance | CLAUDE.md + agents | `copilot-instructions.md` | `.cursor/rules/` | `.windsurfrules` |
| Agent dispatch | ✅ `Task()` | ❌ natural language | ❌ natural language | ❌ natural language |
| Hooks | ✅ all 3 | ❌ | ❌ | ❌ |
| GitHub Actions | ✅ | ✅ | ✅ | ✅ |
| Comment trail | ✅ | ✅ | ✅ | ✅ |

The pipeline is editor-agnostic from review onwards. Any editor that can push a branch and trigger a label change activates the same GitHub Actions.
