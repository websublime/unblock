# Unblock Plugin — Product Requirements Document

**Structured development pipeline for AI agents, powered by ://unblock.**

| | |
|---|---|
| **Version** | 0.1.0-draft |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Date** | March 2026 |
| **Status** | Draft |
| **Depends on** | `unblock-prd-github.md`, `unblock-architecture-github.md`, `unblock-cicd-architecture.md` |
| **Derived from** | `mister-anderson` plugin (websublime/mister-anderson) |

---

## Table of Contents

1. [Problem](#1-problem)
2. [Solution](#2-solution)
3. [Non-Goals](#3-non-goals)
4. [Personas](#4-personas)
5. [Pipeline Overview](#5-pipeline-overview)
6. [Session Isolation Model](#6-session-isolation-model)
7. [Skills](#7-skills)
8. [Agents](#8-agents)
9. [Comment Trail](#9-comment-trail)
10. [Findings Tracking](#10-findings-tracking)
11. [Hooks](#11-hooks)
12. [GitHub Actions](#12-github-actions)
13. [Plugin Structure](#13-plugin-structure)
14. [Configuration](#14-configuration)
15. [Platform Support](#15-platform-support)
16. [Design Decisions](#16-design-decisions)

---

## 1. Problem

AI agents (Claude Code, GitHub Copilot, Cursor, Aider) can write code, but they lack a structured development process. Without guardrails:

- Implementation happens without investigation — the agent codes before understanding
- No review gate — code goes straight to merge with no quality check
- No spec conformity validation — the agent "finishes" but acceptance criteria aren't met
- Context is lost between sessions — the next session re-discovers what the last one already knew
- Review in the same session as implementation is contaminated — the reviewer "remembers" the code it wrote

The core insight from mister-anderson: **the pipeline is the product, not the tools**. An agent with 17 MCP tools but no process is a chainsaw without a safety guard.

---

## 2. Solution

The Unblock Plugin turns Claude Code (and GitHub Copilot) into a structured development pipeline where the developer stays in control. Every task follows the same flow:

```
Plan → Implement (with self-check) → Review (clean session) → QA (clean session) → Merge
```

The plugin provides:

- **Skills** (slash commands) — user-invocable workflow steps
- **Agents** (personas) — specialized sub-agents with scoped responsibilities
- **Hooks** — automatic discipline injection and session context
- **GitHub Actions** — automated review and QA in isolated sessions
- **Comment trail** — structured comments on GitHub Issues for full traceability

The backend is Unblock MCP server — GitHub Issues + Projects V2 with dependency-aware graph computation. Every command that touches task state goes through Unblock's MCP tools.

---

## 3. Non-Goals

- Multi-agent orchestration — this is a single-developer pipeline with agent delegation, not an agent swarm
- Replacing human judgment — the developer decides when to merge, what to rework, and what findings to defer
- CI/CD pipeline — the plugin triggers GitHub Actions but does not replace existing build/deploy workflows
- Editor-specific extensions — the Unblock MCP server works with any MCP-compatible editor. Claude Code gets the richest experience (plugins, hooks, sub-agents), other editors get MCP tools + custom instructions. No dedicated Copilot Extension, Cursor Extension, etc.

---

## 4. Personas

### 4.1 Solo Developer (Primary)

Working alone with AI agents. Needs structure to prevent the agent from going off-rails. Wants investigation before coding, review after coding, and QA before merge. Doesn't want to manually orchestrate every step.

### 4.2 Tech Lead (Secondary)

Manages a small team using agents. Needs visibility into what agents are doing, what's been reviewed, and what's ready to merge. Uses the desktop app for the graph view and the GitHub Projects board for team-level tracking.

### 4.3 Enterprise Developer (Future)

Works in a team with Jira, compliance requirements, and multiple repos. Uses the plugin within the enterprise workflow described in `unblock-jira-research.md`.

---

## 5. Pipeline Overview

### 5.1 Planning Phase (Local)

```
/product-requirements          Grace (PM) elicits PRD from raw idea
        │
        ▼
/architect-solution            Ada (Architect) designs spec from PRD
        │
        ▼
/create-tasks                  Fernando (PO) creates issues with deps from spec
```

All planning happens locally in the developer's Claude Code session. Interactive — needs human input and approval at each step.

### 5.2 Implementation Cycle (Local)

```
/start-task #42
  │
  ├── Phase 1: Resolve issue, read context
  ├── Phase 2: Resolve supervisor (from assignee field)
  ├── Phase 3: Investigation (if no INVESTIGATION comment exists)
  ├── Phase 4: Branch + Implementation
  │
  ├── Phase 5: Self-check (internal loop)
  │   │
  │   ├── Run tests, build, lint locally
  │   ├── Diff review: read own diff against acceptance criteria
  │   ├── If fail → fix and re-check (max 3 attempts)
  │   ├── If acceptance criteria partial → DECISION comment + continue
  │   │
  │   └── Self-check pass → COMPLETED comment + push
  │
  └── Phase 6: Push + label needs-review
      └── FIM da sessão local
```

The self-check loop catches obvious issues (tests fail, build broken, criterion missed) before handing off to review. This is NOT a review — it's the equivalent of a dev reading their own diff before opening a PR.

### 5.3 Review Cycle (CI — Clean Session)

```
Issue #42: label needs-review added
  │
  ▼
GitHub Action: unblock-review.yml
  │
  ├── Fresh session: /review-task #42
  │   ├── Reads comments (INVESTIGATION, COMPLETED, DECISION, DEVIATION)
  │   ├── Reads diff of branch
  │   ├── Produces REVIEW comment with verdict
  │   │
  │   ├── If APPROVE:
  │   │   ├── Remove needs-review, add approved
  │   │   ├── Findings tracking → dispatch Fernando (only if non-GOOD items)
  │   │   │   └── Creates issues in milestone "Review Findings"
  │   │   │       with blocked_by:#42, labels finding:{severity}
  │   │   └── FIM da sessão
  │   │
  │   ├── If NEEDS-REFACTORING:
  │   │   ├── Findings tracking (same as above)
  │   │   ├── Dispatch Martin (refactoring-supervisor) IN THE SAME SESSION
  │   │   │   ├── Martin fixes validated issues
  │   │   │   ├── Re-dispatch Linus → new verdict
  │   │   │   ├── If approve after refactoring → label approved
  │   │   │   └── FIM da sessão
  │   │   │
  │   │
  │   └── If NEEDS-REWORK:
  │       ├── Findings tracking
  │       ├── Remove needs-review, add needs-rework
  │       ├── REWORK comment with list of problems
  │       └── FIM da sessão
```

The review session starts from zero. The ONLY context available is what's written in the issue comments and the branch diff. No memory of implementation decisions, no recollection of "why I did it this way" — the reviewer judges the code as-is.

### 5.4 QA Cycle (CI — Clean Session)

```
Issue #42: label approved added
  │
  ▼
GitHub Action: unblock-qa.yml
  │
  ├── Fresh session: /qa-task #42
  │   ├── Reads comments (all: INVESTIGATION through REVIEW)
  │   ├── Reads spec/design doc from issue body or parent milestone
  │   ├── Runs tests, build, lint
  │   ├── Validates spec conformity
  │   ├── Produces QA comment with verdict
  │   │
  │   ├── If PASS:
  │   │   ├── Add qa-passed
  │   │   ├── Findings tracking (only if non-positive items exist)
  │   │   └── FIM da sessão → developer notified: "ready to merge"
  │   │
  │   └── If FAIL:
  │       ├── Findings tracking
  │       ├── Add needs-rework
  │       └── FIM da sessão → developer notified with failure reasons
```

### 5.5 Rework Cycle (Local → CI loop)

```
Developer opens CC session → hook SessionStart detects needs-rework
  │
  └── "Issue #42 needs rework. Review findings:
       - [CRITICAL] Missing error handling in parse_config()
       - [WARNING] No test for edge case X
       Run /rework-task #42 to address."

/rework-task #42
  │
  ├── Read issue comments → extract REVIEW/QA findings
  ├── Resolve supervisor (same as start-task)
  ├── Checkout existing branch (no new branch)
  ├── Dispatch supervisor with findings context
  ├── Self-check loop (same as start-task Phase 5)
  ├── Push + re-add label needs-review
  └── FIM da sessão local
       │
       └── Triggers review cycle again (§5.3)
           └── Loop until APPROVE → QA → PASS or developer overrides
```

### 5.6 Complete Flow — Local vs CI

```
LOCAL (developer)                     CI (GitHub Actions)
─────────────────                     ──────────────────

/start-task #42
  investigate
  implement
  self-check (internal loop)
  push + label needs-review ────────► unblock-review.yml triggers
                                        /review-task #42 (clean session)
                                        findings → milestone "Review Findings"

                                      ┌─ APPROVE → label approved ────► unblock-qa.yml triggers
                                      │                                   /qa-task #42 (clean session)
                                      │                                   findings → milestone
                                      │
                                      │                                 ┌─ PASS →
label qa-passed                       │                                 │   developer notified
developer merges                      │                                 │   merge when satisfied
                                      │                                 │
                                      │                                 └─ FAIL →
label needs-rework                    │                                     developer notified
/rework-task #42                      │                                     /rework-task #42
  push + needs-review ──────► loop    │                                     push + needs-review ──► loop
                                      │
                                      ├─ NEEDS-REFACTORING
                                      │   Martin fix in same session
                                      │   re-review → back to top
                                      │
                                      └─ NEEDS-REWORK → label ────────► developer notified
                                                                         /rework-task #42
                                                                         push + needs-review ──► loop
```

### 5.7 Key Principle: Session Isolation

Implementation, review, and QA run in **separate sessions**. The review agent has zero memory of the implementation session. It reconstructs context exclusively from the comment trail on the GitHub Issue. This ensures objectivity — the reviewer can't be biased by implementation context.

### 5.8 Fast Track (small fixes)

Not every change needs the full pipeline. Bug fixes, typos, and P3/P4 tasks skip PRD and architecture:

```
/start-task #42    (issue already exists, investigation optional)
        │
        ▼
[same flow from review onwards]
```

---

## 6. Session Isolation Model

### 6.1 Where Each Phase Runs

| Phase | Where | Session | Why |
|---|---|---|---|
| `/product-requirements` | Local (CC) | Developer session | Interactive — needs human input |
| `/architect-solution` | Local (CC) | Developer session | Interactive — needs human review |
| `/create-tasks` | Local (CC) | Developer session | Interactive — needs human approval |
| `/start-task` | Local (CC) | Developer session | Writes code in local workspace |
| `/review-task` | GitHub Actions | **Fresh session** | Must not be contaminated by implementation context |
| `/qa-task` | GitHub Actions | **Fresh session** | Must not be contaminated by implementation or review context |
| `/rework-task` | Local (CC) | Developer session | Writes code in local workspace |

### 6.2 How Session Isolation Works

Implementation pushes branch and adds label `needs-review` to the GitHub Issue. This triggers a GitHub Action that:

1. Checks out the implementation branch
2. Installs Claude Code
3. Runs `/review-task #42` in a completely new session
4. The review agent reads context from issue comments (comment trail) — it has no other context

The same pattern applies for QA: label `approved` triggers a new Action that runs `/qa-task` in a fresh session.

### 6.3 Context Reconstruction

Since review and QA sessions start from zero, they reconstruct context from:

- **Issue body** — Description, Design Notes, Acceptance Criteria (the 3 body sections)
- **Issue comments** — The structured comment trail (INVESTIGATION, COMPLETED, DECISION, DEVIATION)
- **Branch diff** — `git diff main..{branch}`
- **Spec/design docs** — Referenced in the issue body or parent milestone
- **GitHub auto-links** — Cross-references to related issues

This is sufficient. If information isn't in the comments or the diff, it doesn't exist for the reviewer — and that's correct.

---

## 7. Skills

User-invocable slash commands. Each skill orchestrates one phase of the pipeline.

### 7.1 Planning Skills

| Skill | Command | Purpose | Agents Dispatched |
|---|---|---|---|
| **product-requirements** | `/product-requirements` | Elicit and structure a PRD from a raw idea | Grace (PM) |
| **architect-solution** | `/architect-solution` | Design spec and implementation plan from PRD | Ada (Architect) |
| **create-tasks** | `/create-tasks` | Create GitHub Issues with deps from approved spec/PRD. Milestones for epics, Sub-Issues for hierarchy. Batch, structural | Fernando (PO) |
| **create-issue** | `/create-issue` | Create individual GitHub Issues ad-hoc from documentation or description. Single issue, interactive | Fernando (PO) |

### 7.2 Execution Skills

| Skill | Command | Purpose | Agents Dispatched |
|---|---|---|---|
| **start-task** | `/start-task [#issue]` | Full implementation cycle: investigate → implement → self-check → push | Research + Supervisor |
| **rework-task** | `/rework-task [#issue]` | Fix issues from review/QA findings. No investigation, no new branch | Supervisor |
| **review-task** | `/review-task [#issue]` | Code review gate (runs in CI, clean session) | Linus (Reviewer) + optionally Martin (Refactorer) |
| **qa-task** | `/qa-task [#issue]` | QA validation gate (runs in CI, clean session) | Quinn (QA) |

### 7.3 Setup Skills

| Skill | Command | Purpose |
|---|---|---|
| **setup-project** | `/setup-project` | Bootstrap: `setup` MCP tool (creates Project V2 fields), create "Review Findings" milestone, detect stack, create supervisors, install hooks and GitHub Actions |
| **add-supervisor** | `/add-supervisor [tech]` | Create a new implementation supervisor for a specific technology |
| **update-plugin** | `/update-plugin` | Check for plugin updates, cleanup legacy local files, optionally refresh supervisors via Discovery |

### 7.4 Skill Detail: `/start-task`

The most complex skill. Six phases with an internal self-check loop.

**Phase 1 — Resolve Issue**

If `#issue` provided, use directly. Otherwise, call `ready` MCP tool to list unblocked work. Present to user. Pick by priority, ask if doubt.

Validate issue exists via `show #issue`.

**Phase 2 — Read Full Context**

Parse issue data: description, design notes, acceptance criteria, status, labels, milestone. If issue has a parent (Sub-Issue), read parent for epic-level context. If design doc path in body, read it.

**Phase 3 — Resolve Supervisor**

Read assignees from the issue. The assignee field maps to a supervisor agent file: `{assignee}-supervisor.md` in `.claude/agents/`. If supervisor doesn't exist, suggest `/add-supervisor`. If assignee empty, list available supervisors and ask user.

**Phase 4 — Investigation**

Read issue comments via `show --include_comments`. Search for a comment starting with `INVESTIGATION:`. If found, skip. If not found, ask user: "Investigate first or skip?" If investigate, dispatch research agent. The research agent reads the codebase, finds relevant files, and logs an `INVESTIGATION:` comment on the issue via `comment` tool.

**Phase 5 — Branch + Implementation**

Check if branch exists for this issue (`git branch -a | grep issue-{number}`). If exists, ask user: continue or fresh? If not, ask base branch (default: `main`). Dispatch implementation supervisor with full context.

The supervisor implements, runs tests, and checks the work:

```
Self-check loop (max 3 iterations):
  1. Run tests → if fail → fix → re-run
  2. Run build → if fail → fix → re-run  
  3. Run lint → if fail → fix → re-run
  4. Diff review: read own diff against acceptance criteria
     → if criterion missing → implement → re-check
  5. All pass → exit loop
```

After self-check passes, supervisor logs `COMPLETED:` comment and marks status as `in_progress` → `open` (implementation done, awaiting review).

**Phase 6 — Push + Handoff**

Push branch. Add label `needs-review` via `update --labels_add needs-review`. Update status to `in_progress` (the issue is now in the review pipeline). The push + label addition triggers the GitHub Action for review (see §12).

Inform developer: "Implementation complete. Branch pushed. Review will start automatically in a new session."

### 7.5 Skill Detail: `/rework-task`

Simplified version of `/start-task` for addressing review/QA findings.

1. Read issue and comments — extract REVIEW or QA comment with findings
2. Resolve supervisor from assignee (same as start-task)
3. Checkout existing branch (no new branch, no investigation)
4. Dispatch supervisor with findings as context: "Fix these specific issues: [list from REVIEW/QA comment]"
5. Self-check loop (same as start-task Phase 5)
6. Push + re-add label `needs-review` (triggers review again)

### 7.6 Skill Detail: `/review-task`

Runs in a clean CI session (GitHub Action). No access to previous session context.

1. Read issue and comments via `show --include_comments`
2. Verify `COMPLETED:` comment exists
3. Identify implementation branch
4. Dispatch Linus (code-reviewer) — analyzes diff against acceptance criteria
5. Linus logs `REVIEW:` comment with structured findings and verdict
6. Process verdict:
   - **APPROVE** — remove `needs-review`, add `approved`. Track findings if any non-GOOD items exist (see §10). End session.
   - **NEEDS-REFACTORING** — dispatch Martin (refactoring-supervisor) in same session. Martin fixes validated issues, logs `REFACTORING:` comment. Re-dispatch Linus for re-review. Loop until APPROVE or NEEDS-REWORK.
   - **NEEDS-REWORK** — remove `needs-review`, add `needs-rework`. Track findings (see §10). Log `REWORK:` comment listing what needs fixing. End session. Developer notified.

### 7.7 Skill Detail: `/qa-task`

Runs in a clean CI session (GitHub Action). No access to review session context.

1. Read issue and comments — verify `approved` label and REVIEW comment with APPROVE verdict
2. Locate spec/design doc from issue body or parent milestone
3. Dispatch Quinn (qa-gate) — validates spec conformity, runs tests/build/lint
4. Quinn logs `QA:` comment with structured findings and verdict
5. Process verdict:
   - **PASS** — add `qa-passed`. Track findings if any non-positive items exist (see §10). End session. Developer notified: "Ready to merge."
   - **FAIL** — add `needs-rework`. Track findings (see §10). Log failure details. End session. Developer notified with failure reasons and options: rework, follow-up, or override.

---

## 8. Agents

Specialized sub-agents with scoped responsibilities. Each has a name, a role, and explicit boundaries.

**All agents are `.md` configuration files** — markdown documents defining role, instructions, tool permissions, and model preference. They are not compiled code. This follows the same pattern as mister-anderson: agent definitions are declarative configuration, portable across Claude Code (via `agents/` directory), and interpretable by any MCP-compatible editor via custom instructions.

### 8.1 Agent Roster

| Agent | Name | Role | Model | Dispatched by |
|---|---|---|---|---|
| Product Manager | Grace | PRD elicitation and structuring | opus | `/product-requirements` |
| Architect | Ada | System design and spec creation | opus | `/architect-solution` |
| Product Owner | Fernando | Issue creation with context and deps | sonnet | `/create-tasks`, `/create-issue`, findings tracking |
| Research | Sherlock | Codebase investigation, file discovery | sonnet | `/start-task` (investigation phase) |
| Discovery | Daphne | Tech stack detection, external agent fetch, supervisor generation | sonnet | `/setup-project`, `/add-supervisor` |
| Code Reviewer | Linus | Read-only code review, structured findings | opus | `/review-task` |
| QA Gate | Quinn | Spec conformity, test/build/lint validation | opus | `/qa-task` |
| Refactoring Supervisor | Martin | Fix validated review findings | sonnet | `/review-task` (refactoring phase) |
| Implementation Supervisors | Dynamic | Technology-specific implementation | sonnet | `/start-task`, `/rework-task` |

### 8.2 Implementation Supervisors

**Not distributed with the plugin.** Generated at runtime by `/setup-project` or `/add-supervisor`.

The Discovery agent (Daphne) analyzes the project's codebase, fetches specialist content from an external agent directory, filters it for size and relevance, and creates supervisor agents tailored to the detected technologies. Persona names for each supervisor are generated dynamically at creation time. A Rust project gets `rust-supervisor.md` in `.claude/agents/` with Cargo-specific test/build/lint commands, Rust naming conventions, and project-specific file paths. A React monorepo might get both `react-supervisor.md` and `node-backend-supervisor.md`.

Each generated supervisor:
- Is named `{tech}-supervisor` (e.g., `rust-supervisor`, `react-supervisor`)
- Includes the project's actual test, build, and lint commands (not generic ones)
- Inherits the discipline rules via PreToolUse hook (see §11)
- Knows branch naming: `issue-{number}-{slug}`
- Implements the self-check loop before declaring completion
- Uses the comment format for DECISION, DEVIATION, COMPLETED

Supervisors are stored locally in `.claude/agents/` and never modified by plugin updates. They are the developer's to customise.

### 8.3 Agent Boundaries

| Agent | Can | Cannot |
|---|---|---|
| Grace | Read files, write PRD, ask questions | Write code, create issues, design architecture |
| Ada | Read files, write spec, research | Write code, create issues |
| Fernando | Create/update issues, add comments, add deps | Write code, design solutions |
| Sherlock | Read files, search codebase, add comments | Write code, create issues, modify files |
| Daphne | Read files, detect tech stack, WebFetch external directory, write supervisor agents | Write application code, create issues, close issues |
| Linus | Read files, read diff, add review comment | Write code, modify files, close issues |
| Quinn | Read files, run tests/build/lint, add QA comment | Write code, modify files, close issues |
| Martin | Read/write files, fix specific findings | Create issues, close issues, change scope |
| Supervisors | Read/write files, run tests, add comments | Close issues, skip self-check, merge |

---

## 9. Comment Trail

Every issue accumulates a structured comment history. Each comment is prefixed with a type tag and attributed to the agent that wrote it.

### 9.1 Comment Types

| Type | Author | When | Content |
|---|---|---|---|
| `INVESTIGATION:` | Sherlock | Before implementation | Files found, patterns identified, gotchas, root cause analysis |
| `DECISION:` | Supervisor | During implementation | Non-trivial choices and reasoning |
| `DEVIATION:` | Supervisor | During implementation | Where implementation differs from spec and why |
| `COMPLETED:` | Supervisor | After self-check passes | Summary, files changed, decisions count, deviations count, test results |
| `REVIEW:` | Linus | After code review | Findings with severities, security/performance/test checks, verdict |
| `REFACTORING:` | Martin | After fixing review findings | What was fixed, skipped, or deferred |
| `QA:` | Quinn | After QA validation | Spec conformity, test/build/lint results, verdict |
| `REWORK:` | Orchestrator | After needs-rework verdict | Specific issues to address, referenced from REVIEW or QA comment |

### 9.2 Comment Format

All comments are written via the Unblock `comment` MCP tool. They render as native GitHub Issue comments with timestamps and attribution.

```
COMPLETED:
Summary: Implemented rate limiter middleware with token bucket algorithm.
Files changed: src/middleware/rate_limiter.rs, src/config.rs, tests/rate_limiter_test.rs
Decisions: 2 (see DECISION comments above)
Deviations: 0 — implemented as spec
Tests: cargo test passes. curl verified 429 response after limit exceeded.
```

### 9.3 Traceability

The comment trail survives session restarts, context resets, and platform changes. Any agent or human can reconstruct the full history of any issue by reading its comments in chronological order. This is the primary mechanism for context reconstruction in isolated sessions (§6.3).

---

## 10. Findings Tracking

Review and QA agents produce structured findings. Findings that are not addressed in the current cycle are tracked as new GitHub Issues under a dedicated milestone.

### 10.1 When Findings Are Created

Findings tracking runs **only when non-positive findings exist** in the REVIEW or QA comment:

| Verdict | Findings exist when |
|---|---|
| APPROVE | REVIEW contains `[WARNING]` or `[SUGGESTION]` items |
| PASS | QA contains `[EXTRA]`, `[DEVIATES]`, `[RISK]`, or `[MINOR]` items |
| NEEDS-REFACTORING | After Martin's fixes: remaining SKIPPED or unaddressed items |
| NEEDS-REWORK | `[SUGGESTION]` items that won't be addressed by rework |

If the verdict is a clean APPROVE (all `[GOOD]`) or clean PASS (all `[CONFORMS]`/`[PASS]`), **zero findings are created**. Fernando is not dispatched. The session ends.

### 10.2 Process

1. Parse the REVIEW or QA comment for non-positive findings
2. If refactoring ran, parse REFACTORING comment — remove FIXED items, keep SKIPPED
3. Filter out findings that will be addressed in the current cycle (e.g., CRITICAL findings when verdict is NEEDS-REWORK — these go back to the supervisor)
4. Read the "Review Findings" milestone number from CLAUDE.md (created by `/setup-project` at bootstrap — see §14.2)
5. Dispatch Fernando (PO) to create issues:
   - Each finding becomes a GitHub Issue under the "Review Findings" milestone (via `create --milestone`)
   - `blocked_by` the reviewed issue (traceability via dependency graph)
   - Label `finding:{severity}` (e.g., `finding:suggestion`, `finding:warning`)
   - Priority mapped from severity: P3 for suggestions, P2 for warnings, P1 for critical

### 10.3 Deduplication

Before creating a new finding issue, Fernando checks existing open issues:
- Search by file path and similar title
- If a matching issue exists: add comment linking back to the reviewed issue, add `blocked_by` dependency if not already linked. Do NOT create duplicate.
- If no match: create new issue

Conservative matching — a rare duplicate is better than a lost finding.

---

## 11. Hooks

### 11.1 SessionStart

Triggered when a new Claude Code session starts in a project with the plugin installed.

```bash
# Calls Unblock MCP prime tool for context
# Shows: in-progress issues, ready queue, blocked count, needs-review, needs-rework

## Task Status

### In Progress (resume these):
#42 implement rate limiter [P1, task, in_progress, agent: claude-alpha]

### Needs Rework (review/QA findings to address):
#38 refactor config parser [P2, task, needs-rework]

### Ready (no blockers):
#45 add error types [P2, task]
#46 write integration tests [P2, task]

### Blocked:
#50 deploy pipeline [P3, task, blocked by #42, #45]
```

The `needs-rework` section is critical — it surfaces issues that came back from review or QA so the developer knows immediately what to address with `/rework-task`.

### 11.2 PreToolUse (Discipline Injection)

Triggered when the orchestrator dispatches a supervisor via `Task()`. Injects discipline reminder:

```
SUPERVISOR DISPATCH: Before implementing, follow these rules:
1. Read the issue and comments first (Rule 0.1)
2. Look before you code — verify actual data (Rule 1)
3. Self-check loop: tests, build, lint, acceptance criteria (Rule 2)
4. Log DECISION and DEVIATION comments (Rule 4)
5. Log COMPLETED comment before marking done (Rule 5)
6. Never close issues — your job ends at pushing + needs-review (Rule 6)
```

Only injected for agents with `-supervisor` suffix. Not injected for Linus, Quinn, Grace, Ada, Fernando, or Daphne.

### 11.3 PreCompact (Progress Preservation)

Triggered before Claude Code compacts the conversation context. Preserves work-in-progress state by logging a progress comment on the current issue.

```
# Detects if an issue is currently being worked on (in_progress status with agent set)
# Logs a PROGRESS comment to the issue via the comment MCP tool:

PROGRESS:
Working on: [current task description from issue title]
Files touched: [list of modified files from git status]
Status: [description of current implementation state]
Next steps: [what remains to be done]
```

This ensures that if the session continues after compaction, the agent can reconstruct its working state from the comment trail. Combined with SessionStart's `prime` call, this provides continuity across context resets.

Only triggers if there is an active `in_progress` issue claimed by the current agent. If no issue is in progress, the hook is a no-op.

---

## 12. GitHub Actions

### 12.1 Review Gate

```yaml
# .github/workflows/unblock-review.yml
name: "://review"
on:
  issues:
    types: [labeled]

jobs:
  review:
    if: github.event.label.name == 'needs-review'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0  # Full history for diff
      
      - name: Find implementation branch
        id: branch
        run: |
          BRANCH=$(git branch -r --list "*issue-${{ github.event.issue.number }}*" | head -1 | xargs)
          echo "name=${BRANCH#origin/}" >> $GITHUB_OUTPUT
      
      - name: Checkout implementation branch
        run: git checkout ${{ steps.branch.outputs.name }}
      
      - name: Install Claude Code
        run: npm install -g @anthropic-ai/claude-code
      
      - name: Run review in fresh session
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          claude --print "/review-task ${{ github.event.issue.number }}"
```

### 12.2 QA Gate

```yaml
# .github/workflows/unblock-qa.yml
name: "://qa"
on:
  issues:
    types: [labeled]

jobs:
  qa:
    if: github.event.label.name == 'approved'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      
      - name: Find implementation branch
        id: branch
        run: |
          BRANCH=$(git branch -r --list "*issue-${{ github.event.issue.number }}*" | head -1 | xargs)
          echo "name=${BRANCH#origin/}" >> $GITHUB_OUTPUT
      
      - name: Checkout implementation branch
        run: git checkout ${{ steps.branch.outputs.name }}
      
      - name: Install project dependencies
        run: |
          # Auto-detect and install (cargo, npm, pip, etc.)
          [ -f Cargo.toml ] && rustup default stable
          [ -f package.json ] && npm ci
          [ -f requirements.txt ] && pip install -r requirements.txt
      
      - name: Install Claude Code
        run: npm install -g @anthropic-ai/claude-code
      
      - name: Run QA in fresh session
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          claude --print "/qa-task ${{ github.event.issue.number }}"
```

### 12.3 Secrets Required

| Secret | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` | Claude Code API access for review/QA sessions |
| `GITHUB_TOKEN` | Auto-provided by Actions. Issue label changes, comments, Unblock MCP |

### 12.4 Cost Considerations

Each review/QA session consumes API tokens. For a typical implementation:
- Review session: ~10-30k input tokens (diff + comments + files) + ~2-5k output tokens
- QA session: similar, plus test/build/lint execution time

The GitHub Actions runner is free for public repos (2,000 minutes/month) or billed for private repos. The main cost is the Anthropic API.

---

## 13. Plugin Structure

### 13.1 File Layout

The plugin lives inside the monorepo at `plugin/`, with independent versioning (like `unblock-mcp` and `unblock-app`). Source repository is `websublime/unblock`.

```
websublime/unblock/
└── plugin/
    ├── .claude-plugin/
    │   └── plugin.json                    # Plugin metadata (version, source: websublime/unblock)
    ├── marketplace.json                   # Marketplace listing metadata
    ├── agents/
    │   ├── architect.md                   # Ada — system design
    │   ├── product-manager.md             # Grace — PRD elicitation
    │   ├── product-owner.md               # Fernando — issue management
    │   ├── research.md                    # Sherlock — codebase investigation
    │   ├── discovery.md                   # Daphne — tech stack detection + supervisor generation
    │   ├── code-reviewer.md              # Linus — code review gate
    │   ├── qa-gate.md                     # Quinn — QA validation gate
    │   └── refactoring-supervisor.md     # Martin — fix review findings
    ├── skills/
    │   ├── product-requirements/SKILL.md
    │   ├── architect-solution/SKILL.md
    │   ├── create-tasks/SKILL.md
    │   ├── create-issue/SKILL.md
    │   ├── start-task/SKILL.md
    │   ├── rework-task/SKILL.md
    │   ├── review-task/SKILL.md
    │   ├── qa-task/SKILL.md
    │   ├── setup-project/SKILL.md
    │   ├── add-supervisor/SKILL.md
    │   ├── update-plugin/SKILL.md
    │   └── subagents-discipline/SKILL.md
    ├── hooks/
    │   ├── hooks.json
    │   ├── session-start.sh
    │   ├── inject-discipline-reminder.sh
    │   └── pre-compact.sh
    └── templates/
        ├── UNBLOCK-WORKFLOW.md            # Injected into each generated supervisor
        ├── CLAUDE.md.template             # Project orchestrator config
        ├── AGENTS.md.template             # Agent workflow guide
        └── github-actions/
            ├── unblock-review.yml
            └── unblock-qa.yml
```

**No supervisor files in the plugin source.** Supervisors are generated at runtime by `/setup-project` (via Daphne, the Discovery agent) based on the project's detected tech stack. Daphne analyzes the codebase, fetches specialist content from an external directory, filters it for size and relevance, generates a dynamic persona name, injects the UNBLOCK-WORKFLOW.md template, and writes the supervisor to `.claude/agents/`. The `/add-supervisor` skill generates additional supervisors on demand.

### 13.2 Versioning

The plugin follows independent versioning within the monorepo, consistent with `unblock-mcp` (v1.x) and `unblock-app` (v2.x):

```
unblock-plugin   → v0.1.0, v0.2.0, v0.3.0 ...  (Phase 4)
```

Version is tracked in `plugin/.claude-plugin/plugin.json` and `plugin/marketplace.json`. The `/update-plugin` skill compares the local version file (`.claude/.unblock-plugin-version`) against the installed plugin version to detect updates.

### 13.3 What the Plugin Provides vs What Stays Local

| Component | Provided by plugin | Local to project |
|---|---|---|
| Core agents (8) | ✅ Grace, Ada, Fernando, Sherlock, Daphne, Linus, Quinn, Martin | — |
| Skills (12) | ✅ | — |
| Hooks (3) | ✅ SessionStart, PreToolUse, PreCompact | — |
| Templates (3) | ✅ UNBLOCK-WORKFLOW.md, CLAUDE.md.template, AGENTS.md.template | — |
| GitHub Action templates | ✅ (copied to `.github/workflows/` on setup) | ✅ (customisable) |
| Implementation supervisors | ❌ **not in plugin** | ✅ generated by `/setup-project` via Daphne |
| CLAUDE.md / AGENTS.md | — | ✅ (generated from templates on setup, project-specific) |
| Unblock MCP server config | ✅ (injected on setup) | ✅ (env vars) |
| `.github/copilot-instructions.md` | ✅ (generated on setup) | ✅ (customisable) |

---

## 14. Configuration

### 14.1 Plugin Installation

```bash
# Step 1: Add marketplace (source: monorepo websublime/unblock, path: plugin/)
/plugin marketplace add websublime/unblock

# Step 2: Install plugin
/plugin install unblock@websublime-unblock

# Step 3: Bootstrap project
/setup-project
```

### 14.2 What `/setup-project` Does

1. Run Unblock `setup` MCP tool — creates Projects V2 fields (Status, Priority, Agent, etc.)
2. Create "Review Findings" milestone via `create` — persistent milestone for tracking review and QA findings across all tasks. Created once, reused forever. This ensures findings tracking (§10) never needs to search-or-create at runtime
3. Detect tech stack from project files (Cargo.toml, package.json, etc.)
4. Generate implementation supervisors for detected technologies via Discovery agent (Daphne) — fetches specialist content from external directory, filters for size/relevance, injects UNBLOCK-WORKFLOW.md template, writes to `.claude/agents/`
5. Copy GitHub Action workflows to `.github/workflows/` (unblock-review.yml, unblock-qa.yml)
6. Generate `.github/copilot-instructions.md` — Copilot custom instructions with workflow rules, discipline, MCP tool guidance, and supervisor conventions (see §15.2)
7. Write CLAUDE.md from template — project context, tech stack, supervisor routing, "Review Findings" milestone reference, and orchestrator identity rules
8. Write AGENTS.md from template — workflow guide, MCP tool reference, available agents (core + generated supervisors), issue types, and session completion checklist
9. Configure Unblock MCP server in `.claude/settings.json` and `.vscode/mcp.json`
10. Write version file (`.claude/.unblock-plugin-version`) for update tracking

> Template content for UNBLOCK-WORKFLOW.md, CLAUDE.md, and AGENTS.md is specified in `unblock-architecture-plugin.md`.

### 14.3 MCP Server Configuration

```json
{
  "mcpServers": {
    "unblock": {
      "command": "unblock-mcp",
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}"
      }
    }
  }
}
```

### 14.4 Environment Variables

| Variable | Required | Purpose |
|---|---|---|
| `GITHUB_TOKEN` | Yes | Unblock MCP server auth |
| `ANTHROPIC_API_KEY` | Yes (for CI) | Review/QA sessions in GitHub Actions |

---

## 15. Platform Support

### 15.1 Claude Code (Primary)

Full support. All skills, agents, hooks, and GitHub Actions integration.

Claude Code provides:
- Plugin system with slash commands
- `Task()` dispatch for sub-agents
- Hooks (SessionStart, PreToolUse)
- MCP server integration
- Worktree support (future: parallel implementation)

### 15.2 GitHub Copilot (via MCP + Custom Instructions)

Copilot agent mode supports MCP servers natively — the Unblock MCP server works without any adapter or extension. GitHub deprecated Copilot Extensions (GitHub App-based) in November 2025 in favour of MCP, making this the official and only integration path.

The integration uses two mechanisms: **MCP tools** for task operations (identical to CC) and **custom instructions** (`.github/copilot-instructions.md`) for workflow guidance.

#### Feature Comparison

| Feature | Claude Code | GitHub Copilot |
|---|---|---|
| MCP tools (Unblock) | ✅ via plugin config | ✅ via `.vscode/mcp.json` |
| Slash commands | ✅ plugin skills | ❌ no equivalent natively |
| Sub-agent dispatch | ✅ `Task()` | ❌ no equivalent |
| Hooks (SessionStart) | ✅ auto-injects context | ❌ no equivalent |
| Hooks (PreToolUse) | ✅ discipline injection | ❌ no equivalent |
| GitHub Actions (review/QA) | ✅ | ✅ identical workflows |
| Agent personas | ✅ via agent `.md` files | ⚠️ via custom instructions |
| Worktree support | ✅ | ✅ (via VS Code) |

#### Custom Instructions (`.github/copilot-instructions.md`)

Copilot reads `.github/copilot-instructions.md` as system-level context for every conversation. This file replaces agent personas, discipline rules, hooks, and workflow instructions — everything that CC handles via plugins, hooks, and agent files.

`/setup-project` generates this file automatically. Contents:

```markdown
# Unblock Development Workflow

You are working in a project that uses ://unblock for dependency-aware task tracking.
All task state lives in GitHub Issues + Projects V2 via the Unblock MCP server.

## MCP Tools Available

You have access to the Unblock MCP server with these tools:
ready, claim, close, create, update, show, comment, depends, dep_remove,
dep_cycles, list, search, stats, prime, setup, reopen, doctor.

Use these tools for ALL task operations. Never modify issue state directly.

## Workflow

### Starting work
1. Call `prime` to see current task status
2. Call `ready` to find unblocked work
3. Call `show #issue --include_comments` to read full context before coding
4. Call `claim #issue` to take ownership

### During implementation
- Create feature branch: `issue-{number}-{slug}` from main
- Read the issue's Description, Design Notes, and Acceptance Criteria
- If comments contain an INVESTIGATION: comment, use that context
- Log non-trivial decisions: `comment #issue "DECISION: [choice] because [reason]"`
- Log spec deviations: `comment #issue "DEVIATION: Spec said [X], did [Y] because [reason]"`

### Self-check before pushing
1. Run tests — fix if failing (max 3 attempts)
2. Run build — fix if failing
3. Run lint — fix if failing
4. Review own diff against acceptance criteria
5. If all pass, log completion:
   `comment #issue "COMPLETED: Summary: [what]. Files: [list]. Tests: [how verified]."`

### After pushing
1. Push branch
2. Add label: `update #issue --labels_add needs-review`
3. STOP. Review and QA happen automatically via GitHub Actions in clean sessions.

### Rework (when issue has needs-rework label)
1. `show #issue --include_comments` — read REVIEW or QA findings
2. Checkout existing branch
3. Fix the specific issues listed in findings
4. Self-check loop (same as above)
5. Push + `update #issue --labels_add needs-review`

## Discipline Rules

1. **Read before coding** — always `show` the issue and read comments first
2. **Look before assuming** — verify actual data, don't guess field names or types
3. **Log decisions** — every non-trivial choice gets a DECISION comment
4. **Log deviations** — every spec difference gets a DEVIATION comment
5. **Self-check** — tests, build, lint, acceptance criteria before pushing
6. **Never close issues** — your job ends at push + needs-review
7. **Never merge** — the developer decides when to merge

## Review Findings Milestone

Finding issues are tracked under the "Review Findings" milestone (#{milestone_number}).
This is managed automatically by the review/QA GitHub Actions.

## Supervisors

This project uses these implementation patterns:
{generated list of tech stacks and their conventions}
```

#### MCP Configuration for Copilot

`/setup-project` generates `.vscode/mcp.json`:

```json
{
  "servers": {
    "unblock": {
      "command": "unblock-mcp",
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}"
      }
    }
  }
}
```

#### What Works Identically

- **MCP tools** — Copilot calls `ready`, `claim`, `show`, `comment`, `close` etc. exactly like CC
- **GitHub Actions** — review and QA run in clean CI sessions, triggered by label changes. Zero difference from CC
- **Comment trail** — all comments (INVESTIGATION, DECISION, DEVIATION, COMPLETED) are on the GitHub Issue. Tool-agnostic
- **Findings tracking** — runs in the CI session, not locally. Works regardless of which editor pushed the branch

#### What Is More Manual

- **No auto-context on session start** — developer manually asks "show me task status" or calls `prime`. CC injects this via SessionStart hook
- **No discipline injection on dispatch** — discipline rules are in custom-instructions, read once. CC injects them per-dispatch via PreToolUse hook
- **No sub-agent personas** — developer asks Copilot directly: "investigate the codebase for issue #42" instead of dispatching Sherlock. The custom instructions guide behavior but don't enforce persona boundaries
- **No slash commands** — developer describes what they want in natural language ("start working on issue #42") instead of `/start-task #42`. Custom instructions ensure the right workflow is followed

#### Why No Dedicated Copilot Extension

GitHub deprecated Copilot Extensions (GitHub App-based) in September 2025 and completed the sunset on November 10, 2025. The replacement is MCP servers — which is exactly what Unblock already is.

The Unblock MCP server IS the Copilot integration. No additional extension, app, or middleware is needed. Copilot Agent Mode calls MCP tools autonomously, guided by the custom instructions. The developer says "start working on issue #42" and Copilot, with access to Unblock MCP tools + the workflow rules in `copilot-instructions.md`, executes the same pipeline that CC executes via `/start-task`.

This is a strategic advantage: one MCP server serves every compatible editor. Build once, works everywhere.

### 15.3 Supported Editors Summary

The Unblock MCP server is the universal integration layer. Custom instructions provide the workflow guidance. GitHub Actions handle review/QA. The combination works across any MCP-compatible editor.

| Editor | MCP Tools | Workflow Config | Sub-agent Dispatch | Hooks | GitHub Actions |
|---|---|---|---|---|---|
| **Claude Code** | ✅ plugin config | CLAUDE.md + agent files | ✅ `Task()` | ✅ SessionStart, PreToolUse | ✅ |
| **GitHub Copilot** | ✅ `.vscode/mcp.json` | `.github/copilot-instructions.md` | ❌ natural language | ❌ | ✅ |
| **Cursor** | ✅ MCP config | `.cursor/rules/unblock.mdc` | ❌ natural language | ❌ | ✅ |
| **Windsurf** | ✅ MCP config | `.windsurfrules` | ❌ natural language | ❌ | ✅ |
| **Aider** | ✅ MCP config | `.aider.conf.yml` | ❌ natural language | ❌ | ✅ |

`/setup-project` generates the config files for all detected editors (based on presence of `.vscode/`, `.cursor/`, etc.). The content is the same — workflow rules, discipline, MCP tool guidance — adapted to each editor's format.

The pipeline is editor-agnostic from review onwards. Any editor that can push a branch and add a label triggers the same GitHub Actions.

---

## 16. Design Decisions

| # | Decision | Rationale |
|---|---|---|
| PL1 | Session isolation via GitHub Actions | Review/QA in the same session as implementation is contaminated. CI runners provide guaranteed clean sessions |
| PL2 | Labels as state machine triggers | `needs-review` → review Action, `approved` → QA Action, `needs-rework` → developer notification. Simple, event-driven, visible on the board |
| PL3 | Self-check loop before push | Reduces needs-rework rate by catching obvious issues (tests fail, build broken, criterion missed) before review |
| PL4 | Comment trail as primary context mechanism | Survives session resets, is readable by any agent or human, renders natively in GitHub, queryable via API |
| PL5 | Findings tracking only when non-positive items exist | Clean APPROVE or PASS creates zero overhead. Fernando only dispatched when there are actual findings to track |
| PL6 | Rework returns to local session, not CI | Rework involves writing code in the developer's workspace. CI for review/QA is read-only (analysis), not write (implementation) |
| PL7 | Supervisors generated at setup, not distributed with plugin | Discovery analyzes the actual project and creates supervisors with correct commands, paths, and conventions. Generic templates would miss project-specific details. One supervisor per tech stack, routing via assignee field |
| PL8 | GitHub Actions over local watcher | Event-driven (no polling), runs in clean environment (no local state leakage), logs visible in GitHub, free for public repos |
| PL9 | Plugin provides core agents + skills, project owns supervisors | Core agents (reviewer, QA, PM, architect) are universal. Supervisors are project-specific, generated by Discovery, and never modified by plugin updates |
| PL10 | Claude Code first, other editors via MCP + custom instructions | CC has the richest plugin model (Task dispatch, hooks, agent files). All other editors get the same MCP tools + workflow via custom instructions. GitHub deprecated Copilot Extensions in favour of MCP — validating this approach |
| PL11 | Unblock MCP as backend, not Beads CLI | GitHub Issues are the source of truth. Zero local database. Comment trail in GitHub comments. Labels as state. Projects V2 fields for typed data. No Dolt/SQLite dependency |
| PL12 | `/rework-task` as separate skill from `/start-task` | Rework is simpler — no investigation, no new branch, no supervisor selection. Reads findings directly and fixes. Distinct skill reduces confusion |
| PL13 | "Review Findings" milestone created at setup, not at first finding | Eliminates search-or-create logic at runtime in CI sessions. `/setup-project` creates it once, CLAUDE.md stores the reference, all agents read from there |
| PL14 | MCP server IS the Copilot/editor integration — no extension needed | GitHub sunset Copilot Extensions (Nov 2025) in favour of MCP. The Unblock MCP server works natively with Copilot Agent Mode, Cursor, Windsurf, Aider. Build once, works everywhere. Custom instructions provide the workflow layer |
| PL15 | `/setup-project` generates editor configs for all detected editors | One command creates `.github/copilot-instructions.md`, CLAUDE.md, `.cursor/rules/`, etc. Developer doesn't manually configure each editor |
