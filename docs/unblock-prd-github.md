# Unblock — Product Requirements Document

**Dependency-aware task tracking for AI agents, powered by GitHub.**

| | |
|---|---|
| **Version** | 1.1.0-draft |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Repo** | `websublime/unblock` |
| **License** | MIT |
| **Date** | March 2026 |
| **Backend** | GitHub Issues + Projects V2 |

---

## 1. Problem

AI coding agents (Claude Code, Copilot, Codex, Aider, Cursor) lack persistent, structured memory for managing multi-step work across sessions. Current approaches fall short:

- **Markdown plans** — unstructured, no dependency awareness, lost between sessions, no enforcement
- **GitHub Issues alone** — no ready-work calculation, no atomic claim, no context injection
- **Linear/Jira/Asana** — enterprise tools, not agent-optimised, heavy API overhead, external infra
- **Local databases (Beads, SQLite)** — heavy local infrastructure, no native UI, no team visibility

The core question an agent needs answered: **"What can I work on right now?"**

This requires knowing what's open, what's blocked, what blocks what, and computing the unblocked set — in under a second, with zero setup friction.

## 2. Solution

Unblock is an MCP server (Rust binary) that turns GitHub Issues into a dependency-aware task tracking system for AI agents.

It reads GitHub Issues, blocking relationships, and Projects V2 custom fields via the GitHub API. It builds a dependency graph in memory, computes the ready set, and exposes agent-optimised tools via MCP protocol.

- **Agents** interact via MCP tools (stdio transport)
- **Orchestrators** use the same tools with `--agent` parameter to assign work across multiple agents
- **Humans** interact via GitHub Issues UI, Projects V2 boards, and the `gh` CLI
- **Both see the same data** — GitHub is the single source of truth
- **Zero custom storage** — the MCP server computes, GitHub stores

## 3. Non-goals

- Custom UI — GitHub Issues and Projects V2 are the UI
- Offline operation — GitHub API required
- Agent orchestration logic — Unblock tracks state, it doesn't decide what agent does what
- Billing, auth, or user management — delegated to GitHub

---

## 4. Personas

### 4.1 AI Agent (Primary)

- Any MCP-compatible agent: Claude Code, Copilot, Codex, Aider, Cursor
- Operates in a coding session (terminal, IDE)
- Lifecycle: find work → claim → execute → discover new work → close
- Constraints: limited context window, no persistent memory between sessions, needs structured JSON

### 4.2 Orchestrator (Secondary)

- A system or human coordinating multiple agents
- Assigns work: `claim #42 --agent reviewer`
- Monitors: `ready --agent coder`, `blocked --agent reviewer`
- Needs: visibility into agent allocation, ability to redirect

### 4.3 Developer (Secondary)

- Human who reviews agent work, creates epics, triages bugs
- Interacts via GitHub UI (Projects boards, issue pages, `gh` CLI)
- Needs: visibility into agent activity, ability to override, audit trail

---

## 5. Data Model

Unblock stores **zero custom data**. All state lives in GitHub Issues and Projects V2 fields. The MCP server is a **compute layer** over existing GitHub primitives.

### 5.1 GitHub Primitives Used

| Primitive | Purpose | API |
|---|---|---|
| Issue number | Issue ID (`#42`) — native, universal | REST + GraphQL |
| Issue state | Open/Closed ground truth | REST + GraphQL |
| Issue type | Classification (org-level): `task`, `bug`, `feature`, `epic`, `chore`, `spike` | REST (`?type=bug`) + GraphQL |
| Labels | Flexible tagging, filterable | REST + GraphQL |
| Assignees | Human assignment | REST + GraphQL |
| Milestones | Epic/grouping with due date and progress | REST + GraphQL |
| Comments | Discussion thread, audit trail | REST |
| Sub-issues | Parent/child hierarchy | GraphQL (header: `GraphQL-Features: sub_issues`) |
| Blocking | Dependency edges: `blockedBy` / `blocking` | GraphQL mutations: `addBlockedBy`, `removeBlockedBy` |
| Issue body | Markdown with structured sections | REST + GraphQL |
| Projects V2 | Custom fields, views, automations, boards | GraphQL |

### 5.2 Projects V2 Custom Fields

Created by the `setup` tool on a GitHub Project linked to the repo. These provide structured metadata that Issues alone can't express.

| Field | Type | Values | Purpose |
|---|---|---|---|
| **Status** | Single Select | `open`, `in_progress`, `blocked`, `deferred`, `closed` | Fine-grained workflow state beyond GitHub's binary open/closed |
| **Priority** | Single Select | `P0`, `P1`, `P2`, `P3`, `P4` | Sortable priority for the `ready` queue |
| **Agent** | Text | Free text | Which AI agent is working on this |
| **Claimed At** | Date | ISO datetime | Timestamp of claim |
| **Ready State** | Single Select | `ready`, `blocked`, `not_ready`, `closed` | Materialised by MCP server for human visibility in board views |
| **Story Points** | Number | Integer | Estimation |
| **Defer Until** | Date | Date | Hidden from ready queue until this date |

**Why custom fields over labels?** Labels are flat strings. Custom fields are typed, filterable, sortable, and groupable in Projects V2 views. A board grouped by Status with swimlanes by Priority is a built-in view. The Agent field is text, not a label, so it doesn't pollute the label namespace. Defer Until is a date with calendar picker, not a parseable string.

### 5.3 Issue Body Structure

Three sections only. Each data type lives in the correct GitHub primitive — not duplicated in markdown.

```markdown
## Description
Full issue description.

## Design Notes
Technical design decisions.

## Acceptance Criteria
- [ ] Criterion 1
- [ ] Criterion 2
```

The MCP server reads and writes specific sections by parsing markdown headers. Native markdown rendering in GitHub.

**Where other data lives:**

| Data | GitHub Primitive | Why not in body |
|---|---|---|
| Work progress, context, discoveries | **Comments** (`comment` tool) | Append-only, timestamped, attributed to agent. Comments are the work log |
| Related issues, PRs, discussions | **Auto-links** (mention `#N` in comments/body) | GitHub creates bidirectional cross-references automatically |
| Status, Priority, Agent, Story Points | **Projects V2 custom fields** | Typed, filterable, sortable, groupable in board views |
| Labels | **GitHub Labels** | Native, queryable, visual on board |
| Epic grouping | **Milestones** | Native with due dates and progress bar |
| Parent-child hierarchy | **Sub-Issues** | Native API (GA 2025) |
| Blocking relationships | **Blocking API** | `blockedBy`/`blocking` native |

### 5.4 Dependency Model

**Single blocking type.** GitHub's native `blockedBy`/`blocking` relationship. Binary: an issue either blocks another or it doesn't. No typed dependencies (`conditional-blocks`, `relates-to`, `discovered-from`).

**Informational links** via issue mentions: "Discovered while working on #42" in a comment or body. Human/agent readable but not machine-evaluated for blocking.

**Cross-repo blocking** is supported natively by GitHub but scoped to current repo in v1.

### 5.5 Relationship to Existing GitHub Features

| GitHub feature | Unblock's relationship |
|---|---|
| Projects V2 built-in Status field | **Not used.** Unblock creates its own Status single-select with 5 values. Users can hide the built-in field |
| Projects V2 Iteration field | **Independent.** Users use Iterations for sprint planning alongside Unblock fields |
| GitHub Actions | **Complementary.** Actions can trigger on issue events for additional automation |
| GitHub CLI (`gh`) | **Complementary.** Different interface to the same data |
| Copilot in GitHub Issues | **Complementary.** Can use Unblock's fields for context |

---

## 6. MCP Tools

17 tools total. Each operates on the current repo.

### 6.1 Core Workflow Tools

#### `ready` — Find unblocked work

**Purpose:** The fundamental question: "What can I work on right now?"

**Input:**
- `limit` (optional, default: 10, max: 50)
- `type` (optional, filter by issue type)
- `priority` (optional, filter by priority)
- `milestone` (optional, filter by milestone/epic)
- `agent` (optional, filter by agent name)
- `label` (optional, filter by label)
- `include_claimed` (optional, default: false)

**Logic:**
1. Fetch open issues with blocking relationships and Project fields (single GraphQL query)
2. Build dependency graph (petgraph) — use cache if fresh
3. Compute ready set: Status = `open` + no active blockers
4. Post-filter: exclude Defer Until > today
5. Apply params, sort by Priority ASC → created ASC
6. Return top N

**Output per issue:**
```json
{
  "number": 42,
  "title": "Implement auth flow",
  "type": "task",
  "priority": "P1",
  "milestone": "Auth System",
  "story_points": 3,
  "labels": ["backend", "api"],
  "created": "2026-03-15T10:00:00Z",
  "url": "https://github.com/websublime/project/issues/42"
}
```

**Performance:** 0 API calls (cache hit) or 1 GraphQL query + in-memory compute. Sub-second for repos with <1000 issues.

---

#### `claim` — Take ownership

**Purpose:** Atomically assign an issue to an agent and mark it as in-progress.

**Input:**
- `id` (required, issue number)
- `agent` (optional, free text, defaults to config — enables orchestration: `claim 42 --agent reviewer`)

**Logic:**
1. Fetch issue, verify Status = `open`, not blocked, not deferred
2. Update Project fields: Status → `in_progress`, Agent → name, Claimed At → now
3. Add comment: "Claimed by {agent} at {timestamp}"
4. Update Ready State → `not_ready`
5. Invalidate cache

**Errors:** `AlreadyClaimed` (Status = in_progress), `IssueBlocked`, `IssueDeferred`, `IssueClosed`

---

#### `create` — Create issue

**Purpose:** Create an issue with structured fields and optional relationships.

**Input:**
- `title` (required)
- `type` (optional, default: `task`)
- `priority` (optional, default: `P2`)
- `body` (optional, markdown — can include structured sections)
- `labels` (optional, comma-separated)
- `milestone` (optional, name or number)
- `blocked_by` (optional, array of issue numbers — creates blocking relationships)
- `parent` (optional, issue number — creates sub-issue relationship)
- `story_points` (optional, number)
- `defer_until` (optional, date)

**Logic:**
1. Create issue (title, body, labels, milestone, type)
2. Add to Project, set fields (Priority, Status=open, Ready State, Story Points, Defer Until)
3. If `blocked_by`: for each issue number, `addBlockedBy` + Status → `blocked`, Ready State → `blocked`
4. If `parent`: `addSubIssue`
5. Invalidate cache

**ID:** GitHub issue number `#N`. No custom generation.

---

#### `update` — Edit issue fields

**Purpose:** Edit fields on an existing issue — priority, status, labels, assignees, body sections, milestone, story_points, defer_until.

**Input:**
- `id` (required, issue number)
- `priority` (optional)
- `status` (optional)
- `labels_add` (optional, labels to add)
- `labels_remove` (optional, labels to remove)
- `assignees_add` (optional, assignees to add)
- `assignees_remove` (optional, assignees to remove)
- `body_section` (optional, object with `name` and `content` — updates a named markdown section in the issue body)
- `milestone` (optional)
- `story_points` (optional)
- `defer_until` (optional)

**Logic:**
1. Fetch issue → validate exists
2. Update specified fields via Project V2 field mutations + REST API as appropriate
3. If `body_section` provided: parse markdown body, locate section by header name, update section content, PATCH body via REST
4. Invalidate cache + rebuild graph

**Errors:** `IssueNotFound`, `ValidationError`

---

#### `close` — Complete work

**Purpose:** Close an issue and cascade unblocking.

**Input:**
- `id` (required, issue number)
- `reason` (optional, default: "completed")

**Logic:**
1. Verify issue exists and not already closed
2. Close issue via GitHub API
3. Update Project fields: Status → `closed`, Ready State → `closed`
4. Add comment: "Closed: {reason}"
5. Compute unblock cascade:
   - Rebuild graph, find dependents blocked ONLY by this issue
   - For each newly unblocked: Status `blocked` → `open`, Ready State → `ready`
   - Add comment: "Unblocked: #{id} was closed"
6. Invalidate cache

---

#### `reopen` — Reopen a closed issue

**Purpose:** Reopen a closed issue (agent closed by mistake, or work was incomplete).

**Input:**
- `id` (required, issue number)
- `reason` (optional, text explaining why the issue is being reopened)

**Logic:**
1. Verify issue exists and is currently closed
2. Reopen issue via REST API
3. Rebuild dependency graph
4. Evaluate blocking state:
   - If no active blockers: Status → `open`, Ready State → `ready`
   - If active blockers exist: Status → `blocked`, Ready State → `blocked`
5. Add comment: "Reopened: {reason}"
6. Invalidate cache + rebuild

**Errors:** `IssueNotFound`, `IssueNotClosed` (if already open)

---

#### `prime` — Session context injection

**Purpose:** Compile current state for agent context at session start.

**Input:**
- `agent` (optional, filter by agent)
- `max_tokens` (optional, default: 2000, configurable)

**Prioritization order:**
1. **In-progress issues first** — agent's current work (highest relevance)
2. **Blocked issues with context** — what the agent is waiting on and why
3. **Top ready by priority** — next work to pick up
4. **Recently completed** — closed in last 24h for continuity

**Output format:**
```
## Session Context

### Currently working on
- [P1] #42: Implement auth flow (task, in_progress since 2h ago)

### Blocked (1)
- [P0] #45: Deploy pipeline — blocked by #43 (DB migration)

### Hotspots (high fan-out blockers)
- #43: DB schema migration — blocks 4 downstream issues (#45, #46, #50, #51)

### Stale claims (in_progress > 24h)
- #38: Refactor config loader — claimed by coder 36h ago, may be abandoned

### Ready to pick up (top 5)
- [P0] #48: Fix login crash (bug)
- [P1] #50: Token refresh (task)

### Recently completed (24h)
- #43: DB schema migration (completed)
```

**Token budget:** Truncate sections in reverse priority order (Recently Completed first, then Ready, then Stale Claims, then Hotspots) if exceeding max_tokens.

---

### 6.2 Dependency Tools

#### `depends` — Create blocking relationship

**Purpose:** Declare that one issue depends on another.

**Input:**
- `source` (required, issue number — the issue that depends)
- `target` (required, issue number — the issue it depends on)

**Logic:**
1. Verify both issues exist in repo
2. Cycle detection via cached graph
3. `addBlockedBy` mutation
4. Update source: Status → `blocked`, Ready State → `blocked`
5. Invalidate cache

**Errors:** `CircularDependency`, `DuplicateDependency`, `IssueNotFound`

---

#### `dep_remove` — Remove blocking relationship

**Purpose:** Remove a dependency between two issues.

**Input:**
- `source` (required, issue number)
- `target` (required, issue number)

**Logic:**
1. `removeBlockedBy` mutation
2. If source has no other active blockers: Status → `open`, Ready State → `ready`
3. Invalidate cache

---

#### `dep_cycles` — Detect circular dependencies

**Purpose:** Scan the repo's dependency graph for cycles.

**Input:**
- `id` (optional — if provided, check only cycles involving this issue)

**Output:**
```json
{
  "cycles": [["#42", "#45", "#48", "#42"]],
  "count": 1
}
```

---

#### `comment` — Add a note to an issue

**Purpose:** Agent leaves a comment during work — context, discoveries, progress notes.

**Input:**
- `id` (required, issue number)
- `body` (required, markdown text)

**Logic:**
1. Verify issue exists
2. Add comment via REST API (`POST /repos/{owner}/{repo}/issues/{number}/comments`)
3. Comment author is the authenticated token user

Used internally by `claim` ("Claimed by...") and `close` ("Closed:...") but also exposed as a standalone tool for agent notes during work.

---

### 6.3 Query Tools

#### `show` — Full issue details

**Purpose:** Return complete issue with all fields, deps, and comments.

**Input:**
- `id` (required, issue number)
- `include_comments` (optional, default: true)
- `include_deps` (optional, default: true)

**Output:** Title, body (parsed sections), type, status, priority, agent, labels, assignees, milestone, blockedBy, blocking, parent, subIssues, comments, story_points, defer_until.

---

#### `list` — List issues with filters

**Purpose:** Query issues with flexible filtering and sorting.

**Input:**
- `status` (optional)
- `type` (optional)
- `priority` (optional)
- `agent` (optional)
- `label` (optional)
- `milestone` (optional)
- `sort` (optional, default: `priority`, values: `priority`, `created`, `updated`)
- `limit` (optional, default: 20, max: 100)

All filters optional. Without filters, returns all open issues.

---

#### `search` — Full-text search

**Purpose:** Search issues by text content.

**Input:**
- `query` (required, search text)
- `status` (optional, filter)
- `type` (optional, filter)
- `limit` (optional, default: 20)

Uses GitHub's advanced search API (`ISSUE_ADVANCED` type in GraphQL).

---

#### `stats` — Repository statistics

**Purpose:** Aggregate statistics about the current repo.

**Input:**
- `milestone` (optional, filter)

**Output:**
```json
{
  "total_issues": 47,
  "by_status": { "open": 12, "in_progress": 5, "blocked": 8, "deferred": 3, "closed": 19 },
  "by_type": { "task": 25, "bug": 10, "feature": 8, "epic": 3, "chore": 1 },
  "by_priority": { "P0": 2, "P1": 8, "P2": 20, "P3": 12, "P4": 5 },
  "dependencies": { "total": 34, "active_blockers": 8, "cycles": 0 },
  "agents": { "in_progress": 5, "by_agent": { "coder": 3, "reviewer": 2 } },
  "completion_rate": { "closed_last_7d": 12 },
  "avg_claim_duration": "4h 32m",
  "agent_throughput": {
    "coder": { "claimed": 8, "completed": 6, "avg_duration": "3h 15m" },
    "reviewer": { "claimed": 5, "completed": 4, "avg_duration": "5h 48m" }
  },
  "bottlenecks": [
    { "number": 43, "title": "DB schema migration", "blocks_count": 4 }
  ],
  "stale_claims": [
    { "number": 38, "title": "Refactor config loader", "agent": "coder", "claimed_at": "2026-03-15T02:00:00Z", "duration": "36h" }
  ]
}
```

---

### 6.4 Setup & Diagnostics Tools

#### `setup` — Configure Projects V2 fields

**Purpose:** One-time setup of custom fields on the linked GitHub Project.

**Input:**
- `project` (optional, project number — auto-detect if omitted)
- `dry_run` (optional, default: false)
- `migrate` (optional, default: false — if true, iterate all open issues, add to Project, set defaults)

**Logic:**
1. Find or create a Project V2 linked to the repo
2. Create 7 custom fields (skip existing — idempotent)
3. Optionally create seed labels
4. If `migrate`: iterate all open issues, add each to Project, set Status=open, Priority=P2, Ready State based on blocking evaluation. Existing labels/milestones preserved. No data loss.
5. Report success

---

#### `doctor` — Diagnose system health

**Purpose:** Diagnose system health.

**Input:**
- `fix` (optional, boolean, default: false)

**Output:** Structured report with checks:

| Check | Description |
|---|---|
| `project_linked` | A Project V2 is linked to the repo |
| `fields_valid` | All 7 custom fields exist with correct names and options |
| `all_issues_in_project` | All open issues are added to the Project |
| `no_cycles` | Dependency graph contains no circular dependencies |
| `no_orphaned_edges` | No blocking edges reference closed or deleted issues |
| `cache_fresh` | In-memory cache is within TTL |

```json
{
  "status": "warning",
  "checks": [
    { "name": "project_linked", "status": "ok" },
    { "name": "fields_valid", "status": "ok", "detail": "7/7 fields present" },
    { "name": "all_issues_in_project", "status": "warning", "detail": "3 open issues not in Project: #51, #52, #53" },
    { "name": "no_cycles", "status": "ok", "detail": "0 cycles" },
    { "name": "no_orphaned_edges", "status": "warning", "detail": "1 blocking edge references closed issue #10" },
    { "name": "cache_fresh", "status": "ok", "age_seconds": 12 }
  ],
  "fixed": []
}
```

If `fix=true`: add missing issues to the Project, set default field values (Status=open, Priority=P2).

---

## 7. Architecture

### 7.1 System Overview

```
┌──────────────────┐   MCP/stdio   ┌──────────────────┐   HTTPS   ┌──────────────────┐
│   Claude Code    │◄─────────────►│   unblock-mcp    │◄─────────►│   GitHub API     │
│   (or any MCP    │               │   (Rust binary)  │           │   (GraphQL+REST) │
│    client)       │               │                  │           │                  │
└──────────────────┘               └──────────────────┘           └──────────────────┘
                                           │
                                    In-memory graph
                                    cache (petgraph)
```

### 7.2 Crate Structure

```
websublime/unblock/
├── Cargo.toml                     # Workspace
├── crates/
│   ├── unblock-core/              # Library
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types.rs           # Issue, Dependency (mapped from GitHub)
│   │       ├── graph.rs           # petgraph: ready set, cascade, cycles, tree traversal
│   │       ├── cache.rs           # TTL cache
│   │       ├── config.rs          # Configuration loading
│   │       └── errors.rs          # Domain errors (snafu)
│   │
│   └── unblock-mcp/               # Binary
│       └── src/
│           ├── main.rs
│           ├── server.rs          # rmcp handler, tool registration
│           ├── errors.rs          # Infrastructure errors, MCP conversion
│           ├── github/
│           │   ├── mod.rs
│           │   ├── client.rs      # reqwest + auth + auto-detect repo
│           │   ├── graphql.rs     # Read queries (issues, deps, project fields)
│           │   ├── mutations.rs   # Write ops (create, update, close, block)
│           │   └── project.rs     # Projects V2 field CRUD
│           └── tools/
│               ├── mod.rs
│               ├── ready.rs
│               ├── claim.rs
│               ├── close.rs
│               ├── create.rs
│               ├── update.rs
│               ├── reopen.rs
│               ├── depends.rs
│               ├── dep_remove.rs
│               ├── dep_cycles.rs
│               ├── show.rs
│               ├── list.rs
│               ├── search.rs
│               ├── stats.rs
│               ├── prime.rs
│               ├── comment.rs
│               ├── setup.rs
│               └── doctor.rs
│
├── plugin/                        # Claude Code plugin
│   ├── .claude-plugin/plugin.json
│   ├── .mcp.json
│   ├── marketplace.json
│   ├── commands/                  # 17 slash commands
│   └── skills/
│       └── unblock-workflow.md
│
└── docs/
    ├── getting-started.md
    └── agent-workflow.md
```

### 7.3 Core Dependencies

| Crate | Purpose |
|---|---|
| `rmcp` v1.0+ | MCP protocol (server, stdio) |
| `reqwest` | HTTP client |
| `petgraph` | Graph algorithms |
| `snafu` | Error handling |
| `tokio` | Async runtime |
| `serde` / `serde_json` | Serialization |
| `tracing` | Structured logging |
| `schemars` | MCP tool schema generation |
| `chrono` | Date/time |

### 7.4 Graph Engine

The core of Unblock. Pure Rust, no network, fully testable.

- `DependencyGraph::build(issues, blocking_edges)` — builds petgraph DiGraph
- `compute_ready_set()` — issues with Status=open and no active blockers
- `compute_unblock_cascade(closed_id)` — issues unblocked when one closes
- `would_create_cycle(source, target)` — pre-check before addBlockedBy
- `detect_all_cycles()` — Tarjan's SCC for full graph scan
- `dependency_tree(root, direction, depth)` — BFS traversal

### 7.5 Cache and Scaling Strategy

**v1: In-memory pure (Strategy A).** The MCP server rebuilds the graph from GitHub on every cold start (session start). Intra-session, the graph is cached in memory with configurable TTL (default 30s). Invalidated on every write operation. Process death = cache gone.

Cold start cost: 1 GraphQL query fetching all open issues + blocking relationships. For repos with <500 issues this is sub-second. For 1000+ issues, pagination adds 2-4 seconds. This is acceptable for v1's target (developer solo + agents, repos with 50-500 issues).

**Ready State field:** After every graph recomputation, the MCP server writes the computed Ready State to each issue's Projects V2 field. In v1, this is write-only — a convenience for the GitHub Projects board so humans can filter by "Ready State = ready". The MCP server never reads this field for its own logic; it always recomputes from the graph.

**Scale path: Materialised fast path (Strategy D).** When repos grow past 1000+ issues and cold start becomes noticeable, the Ready State field — already being written — becomes the persistent cache. The migration from A to D is additive:

1. Cold start: query issues where `Ready State = "ready"` (1 lightweight filtered query) → respond immediately
2. Background: `tokio::spawn` full graph rebuild → recompute ready set → diff against current Ready State values → batch update any stale fields
3. If diff finds changes, subsequent `ready` calls see corrected data

The agent doesn't wait for the full rebuild — it gets results in milliseconds from the materialised field, and the graph catches up asynchronously. If the field was stale (e.g. a human closed an issue via the GitHub UI while no MCP server was running), the agent sees slightly outdated data for 1-2 seconds until the background rebuild completes.

**What D adds to the codebase:** ~50-100 lines. A fast-path read in the `ready` tool, a `tokio::spawn` for async rebuild, and a freshness check (Ready Computed At timestamp vs TTL). Zero changes to the graph engine, cache struct, or any other tool.

**Why not now:** For <500 issues, A and D have identical user-perceived performance. D adds async complexity and a correctness nuance (stale data window) that isn't worth the trade-off until cold start actually becomes a problem. The design is prepared — the Ready State field write already exists — so the migration is a PR, not a redesign.

**Alternatives evaluated and rejected:**

| Alternative | Why rejected |
|---|---|
| Local file cache (JSON/bincode) | Two sources of truth. Freshness check = 1 API call, same cost as just rebuilding for small repos. File management complexity |
| SQLite local cache | Recreates Beads. Schema migration, sync conflicts, two sources of truth |
| Background daemon (long-lived process) | Changes deployment model completely. Not "install binary, use". Incompatible with stdio MCP transport |

### 7.6 GitHub API Strategy

| Operation | API | Notes |
|---|---|---|
| Fetch issues + deps + fields | GraphQL | Single query, nested blockedBy/blocking + project fieldValues |
| Create/close/reopen issues | REST | Simpler for single-entity ops |
| Update issue fields | GraphQL + REST | Project V2 fields via GraphQL, labels/assignees/body via REST |
| Blocking relationships | GraphQL | `addBlockedBy` / `removeBlockedBy` |
| Project field updates | GraphQL | `updateProjectV2ItemFieldValue` |
| Sub-issues | GraphQL | `addSubIssue` (header: `GraphQL-Features: sub_issues`) |
| Comments | REST | `POST /repos/{owner}/{repo}/issues/{number}/comments` |
| Search | GraphQL | `ISSUE_ADVANCED` search type |
| Setup (field creation) | GraphQL | `createProjectV2Field` |

**Rate limits:** 5000 points/hour (GraphQL) + 5000 requests/hour (REST). Separate pools. No custom throttling needed.

### 7.7 Configuration

| Variable | Required | Default | Notes |
|---|---|---|---|
| `GITHUB_TOKEN` | Yes | — | PAT or GitHub App token |
| `GITHUB_API_URL` | No | `https://api.github.com` | API base URL. GHE Server: `https://<host>/api/v3` |
| `UNBLOCK_REPO` | No | Auto-detect from git remote | `owner/repo` format |
| `UNBLOCK_PROJECT` | No | Auto-detect first linked project | Project number |
| `UNBLOCK_AGENT` | No | `agent` | Default agent name |
| `UNBLOCK_CACHE_TTL` | No | `30` | Cache TTL seconds |
| `UNBLOCK_LOG_LEVEL` | No | `info` | Log level |
| `UNBLOCK_OTEL_ENDPOINT` | No | — | OpenTelemetry collector |

**Auto-detection:** Binary reads git remote for `owner/repo`. Queries linked Projects V2 for project number. Zero config for the common case.

### 7.8 Plugin Architecture

The Claude Code plugin bundles the MCP server config and slash commands.

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

Repo auto-detection from cwd means no per-project config. One plugin install works across all repos.

**GitHub Enterprise Server** — set `GITHUB_API_URL`:

```json
{
  "mcpServers": {
    "unblock": {
      "command": "unblock-mcp",
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}",
        "GITHUB_API_URL": "https://ghe.corp.com/api/v3"
      }
    }
  }
}
```

### 7.9 Resilience

- **Circuit breaker:** 5 failures in 60s → fail fast for 10s. `Mutex<CircuitBreakerInner>` pattern.
- **Retry:** Max 3, exponential backoff (500ms base, 5s max, ±25% jitter). Only 429 and 503.
- **Cache degradation:** Stale graph served with `stale: true` flag if refresh fails.

### 7.10 Field Validation at Boot

On MCP server startup, after resolving the Project, the server validates all 7 custom fields exist with correct names and option values.

- **Fields checked:** Status, Priority, Agent, Claimed At, Ready State, Story Points, Defer Until
- **If a field is missing:** Log an error with a clear message pointing to the `setup` tool: `"Missing Project field 'Priority'. Run the 'setup' tool to create required fields."`
- **If option values are wrong** (e.g. someone renamed "open" to "Open" in the GitHub UI): Log a warning with the diff: `"Status field option mismatch: expected 'open', found 'Open'. Run 'setup' to repair."`
- **Behaviour on failure:** The server starts but tools that depend on Project fields return `ProjectNotConfigured` errors with actionable guidance. Read-only tools (`show`, `search`) that don't require Project fields continue to work.

### 7.11 Concurrency Model

- MCP server is single-process, single-repo. Multiple agents on the same repo share one MCP server instance.
- If multiple MCP servers run against the same repo (e.g. two terminals, two IDE instances): last writer wins at the GitHub level. Each side rebuilds from the source of truth (GitHub) after writes.
- No optimistic locking in v1 — acceptable because agent sessions are typically serial (one claim at a time). If contention becomes a problem, consider etag-based validation on `claim`.

### 7.12 Migration Path for Existing Repos

- Repos with existing Issues but no Project: `setup` creates Project + fields, but existing issues are not added automatically.
- The `setup --migrate` flag handles the full migration:
  1. Create Project (if not exists)
  2. Create all 7 custom fields (idempotent)
  3. Iterate all open issues, add each to the Project
  4. Set Status=open, Priority=P2 (default), Ready State based on blocking relationship evaluation
  5. Existing labels and milestones are preserved. No data loss.
- Issues with existing blocking relationships (set via GitHub UI or `gh` CLI) are detected and mapped into the dependency graph automatically.

### 7.13 API Call Optimization

**Problem:** `claim` does ~7 API calls. `close` with cascade does 1+1+2+1+N×3 calls. For large cascades, this can be slow and rate-limit-hungry.

**Strategy:**
- **Batch Project field updates:** GitHub's `updateProjectV2ItemFieldValue` is per-field, but multiple mutations can be fired in a single GraphQL request (multiple named mutations in one POST body).
- **Cascade batching:** When `close` unblocks N issues, collect all unblocked issues and batch their field updates into fewer GraphQL requests rather than issuing 3 calls per issue.
- **Parallel REST calls:** Where independent (e.g. adding a comment and updating labels), fire requests concurrently via `tokio::join!`.

**Target:** No single tool call should exceed 2 seconds for repos with <500 issues.

### 7.14 Observability

- **Logging:** `tracing`, JSON to stderr. Token redacted.
- **OpenTelemetry (optional):** Tool duration, API duration, cache hits/misses, graph size. Via `UNBLOCK_OTEL_ENDPOINT`.

---

## 8. Agent Workflow

### 8.1 Standard Session

```
1. Session starts
   └─→ Plugin hook: SessionStart → prime → context injected

2. Agent reads context
   └─→ ready → picks highest priority unblocked issue

3. Agent claims
   └─→ claim #42 → Status=in_progress, Agent="coder"

4. Agent works
   ├─→ update #42 --priority P0 "Found this is critical"
   ├─→ create "Bug in auth" --blocked_by 42
   ├─→ depends #45 #42
   ├─→ comment #42 "Found edge case in token refresh"
   └─→ show #42 (refresh context)

5. Agent completes
   └─→ close #42 "completed" → #45 unblocked → loop to step 2

6. Session ends
   └─→ Plugin hook: PreCompact
```

### 8.2 CLAUDE.md Template

```markdown
## Task Tracking — Unblock

Backend: GitHub Issues + Projects V2

### Workflow

1. Context auto-injected via `prime`
2. `ready` — find unblocked work. NEVER work on blocked issues
3. `claim #id` — claim before starting. NEVER work unclaimed
4. `update #id --field value` — update fields when context changes
5. `create --blocked_by #current` — track discovered work
6. `depends #source #target` — declare dependencies
7. `comment #id "context"` — leave notes during work
8. `close #id "reason"` — close when done
9. Include in commits: `git commit -m "Implement auth (#42)"`

### Rules

- NEVER work without claiming
- NEVER work on blocked issues
- ALWAYS `ready` before picking work
- ALWAYS track dependencies when discovered
```

---

## 9. Error Handling

| Error | Code | When |
|---|---|---|
| `IssueNotFound` | 404 | Issue number doesn't exist |
| `AlreadyClaimed` | 409 | Status = in_progress |
| `IssueBlocked` | 409 | Active blockers exist |
| `IssueDeferred` | 409 | Defer Until > today |
| `IssueClosed` | 409 | Already closed |
| `IssueNotClosed` | 409 | Trying to reopen an issue that is already open |
| `CircularDependency` | 422 | `depends` would create cycle |
| `DuplicateDependency` | 409 | Blocking relationship exists |
| `ProjectNotConfigured` | 500 | No Project linked or fields missing — run `setup` |
| `GitHubApiError` | 502 | GitHub API error |
| `GitHubUnavailable` | 503 | Cannot reach api.github.com |
| `RateLimited` | 429 | Rate limit hit |
| `CircuitBreakerOpen` | 503 | Too many failures |
| `ValidationError` | 400 | Invalid input |

---

## 10. Delivery Plan

### Phase 1 — Foundation (v0.1.0)

**Goal:** An agent can find work, claim it, edit it, complete it, and see the cascade. The minimum viable loop.

**Scope:** 9 tools (setup + 7 core workflow + depends + comment).

| Tool | Rationale |
|---|---|
| `setup` | Required first — creates Project fields |
| `ready` | The core question. Requires graph engine, cache |
| `claim` | Atomic ownership. Requires Project field writes |
| `close` | Completion + cascade. Requires graph recomputation |
| `create` | Agent creates work during execution |
| `update` | Agents need full CRUD, not just create+close. Edit is the most common operation after read |
| `show` | Agent sees full details before claiming |
| `depends` | Agent declares discovered dependencies |
| `comment` | Agent leaves notes during work. Trivial (1 REST call) but essential — agents can't use the GitHub UI |

**Infrastructure:** Cargo workspace, CI, graph engine, cache, GitHub client, MCP server, integration tests, docs.

**Not in Phase 1:** `list`, `search`, `stats`, `prime`, `dep_remove`, `dep_cycles`, `reopen`, `doctor`, plugin, resilience, observability. Useful but not required for the core loop.

**Quality standards:** Quality standards for Phase 1-2 prioritize working software over exhaustive coverage. Target: >80% public API coverage, property tests for graph invariants, clippy clean. Full 100% coverage and pedantic clippy enforced from Phase 3.

**Effort:** ~14 days focused.

---

### Phase 2 — Complete (v0.2.0)

**Goal:** Full tool suite + Claude Code plugin. Feature-complete product.

**Core scope:** 4 tools + plugin.

| Component | Rationale |
|---|---|
| `prime` | Session context injection — the "memory" at session start |
| `dep_remove` | Dependency management requires both add and remove |
| `reopen` | Agents need to recover from premature closes |
| Plugin | Slash commands, workflow skill, marketplace, hooks (SessionStart, PreCompact) |

**Stretch scope:** 5 tools.

| Component | Rationale |
|---|---|
| `list` | Flexible query beyond `ready` |
| `search` | Full-text search (GitHub advanced search API) |
| `stats` | Aggregate view for orchestrators |
| `dep_cycles` | Diagnostic for graph health |
| `doctor` | Operational health — self-diagnosis for unattended MCP servers |

**Rationale for split:** Agents use `ready` 95% of the time; `list`/`search`/`stats` are for orchestrators and can wait. Core Phase 2 tools (`prime`, `dep_remove`, `reopen`) directly improve the agent workflow loop.

**Effort:** ~8 days focused.

---

### Phase 3 — Production (v1.0.0)

**Goal:** Hardened for real-world use.

| Component | Rationale |
|---|---|
| Circuit breaker | Graceful degradation on GitHub outages |
| Retry with backoff | Transient failure handling |
| OpenTelemetry | Production debugging |
| Cross-platform binaries | Linux x86_64/ARM64, macOS x86_64/ARM64, Windows x86_64 |
| Homebrew formula | `brew install websublime/tap/unblock` |
| npm wrapper | `npx @unblock/cli` |

**Effort:** ~6 days focused.

---

### Total: ~28 days → 10-15 weeks part-time

---

### Future (post v1.0)

See `unblock-prd-desktop.md` and `unblock-desktop-project-plan.md` for the desktop application vision and delivery plan.

| Feature | Description |
|---|---|
| Materialised fast path | Strategy D: use Ready State field as persistent cache for cold start. Serve immediately from field, rebuild graph async. ~50-100 lines change. Trigger: cold start > 2s on target repos |
| Cross-repo blocking | GitHub supports natively, extend MCP to handle multi-repo graphs |
| Webhook cache invalidation | GitHub Actions → HTTP → invalidate cache for instant consistency |
| Agent Session tracking | Structured comments or dedicated tracking issue per session |
| `merge` tool | Duplicate issue consolidation |
| GitHub App auth | Higher rate limits (15k/h), org-wide install, no PAT needed |
| Multi-repo dashboard | Aggregate `stats` across repos |
| Conditional blocking | "Unblock if target fails" — convention-based via labels/comments |

---

## 11. Design Decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | GitHub over Fibery | 1000x better rate limits (5000/h vs 3/s), zero external infra, native blocking API, developer-native |
| D2 | Projects V2 custom fields over labels | Typed, filterable, sortable, groupable. Proper structured data for workflow logic |
| D3 | Issue number as ID | Native, universal, zero collision risk, linkable in commits |
| D4 | Single blocking type | GitHub native. Covers 95% of workflows. Informational deps via mentions |
| D5 | Markdown body sections for rich fields | Three sections only (Description, Design Notes, Acceptance Criteria). Work progress lives in comments, cross-references in auto-links. Each data type in the correct GitHub primitive |
| D6 | Ready State as convenience field | Project field for board filtering. MCP always recomputes from graph |
| D7 | 17 tools total | Focused on what agents uniquely need. `label`, `delete` handled by GitHub UI/CLI. `update`, `comment`, `reopen` included because agents can't use UI and need full CRUD |
| D8 | Auto-detect repo from git remote | Zero config. Override via `UNBLOCK_REPO` |
| D9 | Plugin bundles `.mcp.json` | Only needs `GITHUB_TOKEN`. Auto-detect handles repo scoping |
| D10 | Milestones as Epics | Native GitHub with due dates and progress. No custom entity |
| D11 | Rust + rmcp | Type safety, single binary, async. Graph engine is backend-agnostic |
| D12 | In-memory graph, no materialisation dependency | Graph always recomputed from fresh data. Ready State field is write-only convenience for UI. Scale path (Strategy D) uses the same field as persistent cache when cold start exceeds 2s — additive change, ~50-100 lines, zero redesign |
| D13 | `blocked_by` as array on `create` | Real-world issues often have multiple blockers. Array input creates all relationships in one call, reducing agent round-trips |
| D14 | `update` tool included from Phase 1 | Agents need full CRUD, not just create+close. Edit is the most common operation after read |
| D15 | `doctor` tool for operational health | MCP servers run unattended; agents need self-diagnosis capability |
| D16 | Batch GraphQL mutations to reduce API call count | Multiple `updateProjectV2ItemFieldValue` mutations in a single POST body. Critical for cascade operations and `update` with multiple field changes |
| D17 | Configurable API base URL via `GITHUB_API_URL` | Supports GitHub Enterprise Server and GHE Cloud. Follows `gh` CLI / GitHub Actions naming convention. Default `https://api.github.com` preserves zero-config for github.com users |
