# ://unblock — Technical Specification

> Version: 0.1-draft  
> Status: Working Draft  
> Companions: [MANIFESTO.md](./MANIFESTO.md) · [PRD.md](./PRD.md)  
> Plans: [01-mcp-foundation](./plans/01-plan-mcp-foundation.md) · [02-mcp-complete](./plans/02-plan-mcp-complete.md) · [03-mcp-production](./plans/03-plan-mcp-production.md) · [04-plugin](./plans/04-plan-plugin.md) · [05-remote-server](./plans/05-plan-remote-server.md) · [06-llm-agent](./plans/06-plan-llm-agent.md) · [07-harness](./plans/07-plan-harness.md)  
> Specs: [01-graph-engine](./specs/01-spec-graph-engine.md) · [02-github-client](./specs/02-spec-github-client.md) · [03-mcp-tools](./specs/03-spec-mcp-tools.md) · [04-plugin-pipeline](./specs/04-spec-plugin-pipeline.md) · [05-remote-server](./specs/05-spec-remote-server.md) · [06-llm-agent](./specs/06-spec-llm-agent.md)

---

## Table of Contents

1. [Terminology](#1-terminology)
2. [Data Model](#2-data-model)
3. [Graph Engine](#3-graph-engine)
4. [Cache Layer](#4-cache-layer)
5. [GitHub API Layer](#5-github-api-layer)
6. [MCP Tools](#6-mcp-tools)
7. [Plugin Pipeline](#7-plugin-pipeline)
8. [Remote Server](#8-remote-server)
9. [LLM Agent](#9-llm-agent)
10. [Harness](#10-harness)
11. [Configuration](#11-configuration)
12. [Error Model](#12-error-model)
13. [Observability](#13-observability)
14. [Resilience](#14-resilience)
15. [Security](#15-security)
16. [Build & Distribution](#16-build--distribution)
17. [Testing](#17-testing)

---

## 1. Terminology

| Term | Definition |
|---|---|
| **ready set** | The set of issues with no active blockers — all blocking dependencies are closed. The fundamental output of the graph engine. Agents work from the ready set, never from a flat list. |
| **cascade** | The automatic recomputation that occurs when an issue is closed. Dependents whose blockers are now all resolved are promoted to ready. Cascade is structural — it is not optional. |
| **claim** | Atomic assignment of an issue to an agent. Sets agent name, status, timestamp, and Ready State in a single operation. Two agents cannot claim the same issue. |
| **dependency graph** | The directed acyclic graph of blocking relationships between issues. Edges represent "blocked by" — the source depends on the target. The graph is the product. |
| **qualified ID** | A fully-qualified issue identifier: `owner/repo#number`. Prevents collision between issue #5 in repo A and issue #5 in repo B. Used internally by the graph engine. |
| **graph cache** | The in-memory cache of the computed dependency graph and ready set. Ephemeral — invalidated after every write, reconstructable from GitHub in a single API call. |
| **shared graph cache** | The remote server's multi-tenant graph cache. `DashMap<CacheKey, Arc<RwLock<CacheEntry>>>` keyed by `(owner/repo, token_fingerprint)`. Survives between sessions. |
| **session isolation** | The architectural guarantee that planning, investigation, implementation, review, and QA run in separate sessions. The comment trail is the sole medium of communication between sessions. |
| **structured comment** | A comment posted to a GitHub Issue following a defined template. Types: DISPATCH, INVESTIGATION, COMPLETED, DECISION, DEVIATION, REVIEW, REFACTORING, QA, AUDIT, BLOCKED, PAUSE. |
| **comment trail** | The chronological sequence of structured comments on an issue. Reconstructable by any agent or human. The shared memory that makes session isolation possible. |
| **enforcement layer** | One of three independent mechanisms that prevent pipeline violations: MCP validation (label transition preconditions), Inspector (Gadget — comment trail audit), agent prompt structure (BLOCK conditions). |
| **worktree** | Isolated git worktree at `worktrees/issue-{N}-{slug}` used for implementation and refactoring. The branch is a consequence of the worktree — the worktree is the primary concept. |
| **Projects V2 field** | Custom field on a GitHub Projects V2 board. unblock uses 7: Status, Priority, Agent, Claimed At, Ready State, Story Points, Defer Until. |
| **body sections** | Three structured sections in the issue body markdown: Description, Design Notes, Acceptance Criteria. Parsed and written by the MCP server. Each data type lives in the correct GitHub primitive — not duplicated. |
| **cross-repo** | Blocking relationships that span repositories. GitHub Issue node IDs are globally unique. unblock supports `owner/repo#number` references for cross-repo dependencies. |
| **MCP** | Model Context Protocol. The communication protocol between AI agents and tool servers. unblock uses stdio (local) and Streamable HTTP (remote). |
| **skill** | A slash-command entry point in the plugin. Lives in `skills/{name}/SKILL.md`. Invoked as `/name`. Skills route intent to agents — they are dispatchers, not executors. |
| **agent** | A named `.md` configuration file that defines a specialised persona with constrained tools, model, and hard boundaries. Not compiled code. |
| **finding** | A deferrable issue created by Fernando when review (SUGGESTION) or QA (MINOR, RISK, DEVIATES, EXTRA) produces observations that do not block merge but should be tracked. Created as child of the parent epic. |
| **drift** | Semantic divergence between the in-memory graph and GitHub reality. Caused by external mutations (human closes an issue via GitHub UI). Detected and repaired by `reconcile`. |

---

## 2. Data Model

unblock stores zero custom data. All state lives in GitHub Issues and Projects V2 fields. The MCP server is a compute layer over existing GitHub primitives.

→ Detailed algorithms and edge cases: [01-spec-graph-engine.md](./specs/01-spec-graph-engine.md)

### 2.1 GitHub Primitives

| Primitive | Purpose | API |
|---|---|---|
| Issue number | Issue ID (`#42`) — native, universal | REST + GraphQL |
| Issue state | Open/Closed ground truth | REST + GraphQL |
| Issue type | Classification (org-level): `task`, `bug`, `feature`, `epic`, `chore`, `spike` | REST + GraphQL |
| Labels | Flexible tagging, filterable | REST + GraphQL |
| Assignees | Human assignment | REST + GraphQL |
| Milestones | Epic/grouping with due date and progress | REST + GraphQL |
| Comments | Discussion thread, audit trail | REST |
| Sub-issues | Parent/child hierarchy | GraphQL (`GraphQL-Features: sub_issues`) |
| Blocking | Dependency edges: `blockedBy` / `blocking` | GraphQL mutations |
| Issue body | Markdown with structured sections | REST + GraphQL |
| Projects V2 | Custom fields, views, automations, boards | GraphQL |

Cross-repo blocking works natively — GitHub's `addIssueDependency` accepts any two Issue node IDs regardless of repository.

### 2.2 Projects V2 Custom Fields

Created by the `setup` tool. These provide structured metadata that Issues alone cannot express.

| Field | Type | Values | Purpose |
|---|---|---|---|
| **Status** | Single Select | `open`, `in_progress`, `blocked`, `deferred`, `closed` | Fine-grained workflow state beyond GitHub's binary open/closed |
| **Priority** | Single Select | `P0`, `P1`, `P2`, `P3`, `P4` | Sortable priority for the `ready` queue |
| **Agent** | Text | Free text | Which AI agent is working on this |
| **Claimed At** | Date | ISO datetime | Timestamp of claim |
| **Ready State** | Single Select | `ready`, `blocked`, `not_ready`, `closed` | Materialised by MCP server for human visibility in board views |
| **Story Points** | Number | Integer | Estimation |
| **Defer Until** | Date | Date | Hidden from ready queue until this date |

Why custom fields over labels: fields are typed, filterable, sortable, and groupable in Projects V2 views. The Agent field is text — it does not pollute the label namespace.

### 2.3 Issue Body Structure

Three sections only. Each data type lives in the correct GitHub primitive.

```markdown
## Description
Full issue description.

## Design Notes
Technical design decisions.

## Acceptance Criteria
- [ ] Criterion 1
- [ ] Criterion 2
```

Data placement rule:

| Data | GitHub Primitive | Why not in body |
|---|---|---|
| Work progress, context, discoveries | **Comments** | Append-only, timestamped, attributed. Comments are the work log |
| Related issues, PRs, discussions | **Auto-links** (mention `#N`) | GitHub creates bidirectional cross-references automatically |
| Status, Priority, Agent, Story Points | **Projects V2 custom fields** | Typed, filterable, sortable, groupable in board views |
| Labels | **GitHub Labels** | Native, queryable, visual on board |
| Epic grouping | **Milestones** | Native with due dates and progress bar |
| Parent-child hierarchy | **Sub-Issues** | Native API (GA 2025) |
| Blocking relationships | **Blocking API** | `blockedBy`/`blocking` native |

### 2.4 Dependency Model

Single blocking type. GitHub's native `blockedBy`/`blocking` relationship. Binary: an issue either blocks another or it does not. No typed dependencies.

Informational links via issue mentions in comments or body — human/agent readable but not machine-evaluated for blocking.

### 2.5 Pre-configured Views

Created by `setup`. Five views provide opinionated board layouts:

| View | Layout | Purpose |
|---|---|---|
| `𝍄 UNBLOCK://ready` | Board | Agent's ready queue — filtered to ready issues |
| `𝍄 UNBLOCK://team` | Board | Tech lead view — who is working on what |
| `𝍄 UNBLOCK://pipeline` | Board | Classic kanban — full workflow |
| `𝍄 UNBLOCK://roadmap` | Table | Epic-level progress by milestone |
| `𝍄 UNBLOCK://timeline` | Roadmap | Date-based timeline for sprint planning |

---

## 3. Graph Engine

Pure Rust, no network, fully testable with in-memory data. Lives in `unblock-core/src/graph.rs`.

→ Detailed algorithms and edge cases: [01-spec-graph-engine.md](./specs/01-spec-graph-engine.md)

### 3.1 Data Structure

```rust
pub struct DependencyGraph {
    graph: DiGraph<QualifiedId, ()>,
    node_map: HashMap<QualifiedId, NodeIndex>,
    issue_status: HashMap<QualifiedId, Status>,
    issue_state: HashMap<QualifiedId, IssueState>,
}
```

`petgraph::graph::DiGraph` with `QualifiedId` nodes. Edge direction: `blocked_issue → blocking_issue` (source depends on target). The graph may span multiple repositories when cross-repo dependencies exist.

### 3.2 Qualified IDs

```rust
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct QualifiedId {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}
```

All graph operations use `QualifiedId` — never plain `u64`. This prevents collision between issues in different repos. `IssueRef` is the parsed user input (may be local or cross-repo); `QualifiedId` is the resolved internal representation.

### 3.3 Ready Set Calculation

An issue is ready if:
1. `Status == Open` (not InProgress, Blocked, Deferred, Closed)
2. `IssueState == Open` (not Closed)
3. No active blocking dependencies (all blockers have `IssueState::Closed`)

`Defer Until` filtering is a post-filter in the tool layer, not in the graph engine — the graph does not know about dates.

### 3.4 Cascade

When an issue is closed, the graph recomputes which dependents become unblocked. A dependent is promoted to ready if all of its blockers are now closed. The cascade is structural — it happens on every close, unconditionally.

### 3.5 Cycle Detection

Two operations:

- **Pre-mutation check:** `would_create_cycle(source, target)` — called before `addBlockedBy` in GitHub. Uses `has_path_connecting` from petgraph. Prevents cycles from forming.
- **Full detection:** `detect_all_cycles()` — Tarjan's SCC algorithm. Returns all cycles. Used by `dep_cycles` tool and `reconcile`.

### 3.6 Dependency Tree

BFS traversal from a root issue in three directions: `Upstream` (what does this depend on?), `Downstream` (what depends on this?), `Both`. Used by `show` to display dependency context and by `dep_cycles` for targeted checks.

```rust
pub enum TraversalDirection { Upstream, Downstream, Both }

pub struct DependencyTree {
    pub root: QualifiedId,
    pub upstream: Vec<TreeNode>,
    pub downstream: Vec<TreeNode>,
}
```

---

## 4. Cache Layer

→ Detailed algorithms and edge cases: [01-spec-graph-engine.md](./specs/01-spec-graph-engine.md)

### 4.1 Design Decisions

Two decisions are fixed by correctness requirements, not performance:

**`show` is always fresh, never cached.** The `show --include_comments` output is used by review, QA, and rework agents to reconstruct context from structured markers. Serving stale comments produces incorrect review findings. The rate limit cost is negligible (<0.1% of the 5000 requests/hour budget).

**`comment` has no cache invalidation.** Comments do not affect the dependency graph. `GraphCache` stores graph topology and ready set — neither is changed by a comment.

### 4.2 GraphCache (Local)

```rust
pub struct GraphCache {
    ttl: Duration,
    inner: RwLock<Option<CacheEntry>>,
}

struct CacheEntry {
    graph: DependencyGraph,
    ready_set: HashSet<QualifiedId>,
    built_at: Instant,
}

pub enum CacheResult<'a> {
    Fresh(&'a CacheEntry),
    Stale(&'a CacheEntry),
    Empty,
}
```

**Invariant:** Every field in `CacheEntry` is reconstructable from GitHub with a single `fetch_graph_data()` call.

### 4.3 Cache Lifecycle

```
Write operation (close, claim, create, depends, dep_remove, update, reopen)
  ├─→ Execute write in GitHub (mutation)
  ├─→ cache.invalidate()              ← clears entry
  ├─→ fetch_graph_data()              ← unconditional fetch
  ├─→ Build new DependencyGraph
  ├─→ Compute ready set
  ├─→ Diff against current Ready State field values
  ├─→ Batch update changed Ready State fields in GitHub
  └─→ cache.update(graph)

Read operation (ready, prime, stats, list, dep_cycles)
  ├─→ cache.get() == Fresh    → return cached data (0 API calls)
  ├─→ cache.get() == Stale    → rebuild → cache.update(graph)
  └─→ cache.get() == Empty    → rebuild → cache.update(graph)

show / fetch_issue_ref (single issue with comments)
  └─→ ALWAYS fetches fresh from GitHub (1 API call)
```

### 4.4 Invalidation Matrix

| Tool | Invalidates GraphCache | Reason |
|---|---|---|
| `close` | ✅ | Cascade changes topology |
| `claim` | ✅ | Status field changes |
| `create` | ✅ | New node in graph |
| `depends` | ✅ | New edge in graph |
| `dep_remove` | ✅ | Edge removed |
| `update` | ✅ | Status/defer may change ready set |
| `reopen` | ✅ | Node returns to graph |
| `comment` | ❌ | Graph topology unchanged |
| `show` | ❌ | Read-only, always fresh |
| `ready` | ❌ | Read-only |
| `prime` | ❌ | Read-only |
| `stats` | ❌ | Read-only |
| `search` | ❌ | Bypasses cache entirely (GitHub Search API) |
| `dep_cycles` | ❌ | Read-only |
| `commit_context` | ❌ | Read-only |
| `reconcile` | ❌ | Fresh fetch bypasses cache; populates after analysis |

Cross-repo invalidation: any write invalidates the entire cache regardless of target repo.

### 4.5 Field Validation at Boot

After `resolve_project()`, the server validates all 7 required Projects V2 fields exist with correct types and option values. Missing field → hard error, server refuses to start. Wrong option values → warning logged, server continues.

### 4.6 Concurrency

Single-process architecture. Multiple agents on separate stdio connections share the in-memory cache. Last writer wins — no optimistic locking. Acceptable because GitHub is the source of truth and write operations always invalidate + rebuild.

### 4.7 Persistent Cache (Phase 03, planned)

Git object cache using `refs/unblock/*` in the repository's `.git/objects/`. Persists computed subgraphs across process restarts. Never stores issue content or mutations. Discardable — deleting all `refs/unblock/*` refs returns to pure in-memory behaviour. Library: `git2` with vendored `libgit2-sys`.

---

## 5. GitHub API Layer

Lives in `unblock-github`. Depends on `unblock-core`. No transport, no MCP.

→ Detailed algorithms and edge cases: [02-spec-github-client.md](./specs/02-spec-github-client.md)

### 5.1 Client Architecture

```rust
pub struct GitHubClient {
    http: reqwest::Client,
    token: String,
    api_base_url: String,       // GITHUB_API_URL
    owner: String,
    repo: String,
    project_number: Option<u64>,
    project_id: Option<String>,
    field_ids: Option<ProjectFieldIds>,
}
```

`ProjectFieldIds` caches the GraphQL node IDs and option IDs for all 7 Projects V2 fields. Resolved once at startup.

### 5.2 Read Queries (GraphQL)

| Function | Purpose | Caching |
|---|---|---|
| `fetch_graph_data()` | All open issues + blocking edges + Project fields — the primary read query | Populates GraphCache |
| `fetch_issue(number)` | Single issue by number (local repo) | Never cached |
| `fetch_issue_ref(ref)` | Single issue by `IssueRef` (local or cross-repo) | Never cached |

`fetch_graph_data` is a single GraphQL query with pagination. Returns everything needed for graph construction.

### 5.3 Mutations

| Function | API | Cross-repo |
|---|---|---|
| `create_issue` | REST POST | No |
| `close_issue` | REST PATCH | No |
| `reopen_issue` | REST PATCH | No |
| `update_issue` | REST PATCH | No |
| `add_comment` | REST POST | No |
| `add_blocked_by` | GraphQL mutation | Yes — any two Issue node IDs |
| `remove_blocked_by` | GraphQL mutation | Yes |
| `add_sub_issue` | GraphQL mutation | No |

Write operations scoped to configured repo for safety. Dependency mutations (`depends`, `dep_remove`) accept cross-repo `IssueRef`.

### 5.4 Projects V2 Field Management

| Function | Purpose |
|---|---|
| `resolve_project()` | Resolve Project V2 node ID + field IDs. Called once at startup |
| `update_field()` | Update a single field value on a project item |
| `batch_update_fields()` | Multiple field updates in one GraphQL mutation |
| `setup_fields()` | Create missing custom fields (idempotent) |
| `migrate_issues()` | Add existing open issues to the project (idempotent) |

### 5.5 REST Views and Fields

View and field management uses REST API (`2026-03-10`), not GraphQL. The `X-GitHub-Api-Version: 2026-03-10` header is sent selectively for these endpoints. All other REST calls use `2022-11-28`.

### 5.6 GHE URL Resolution

| Environment | `GITHUB_API_URL` | GraphQL endpoint |
|---|---|---|
| github.com (default) | `https://api.github.com` | `{base}/graphql` |
| GHE Server | `https://<host>/api/v3` | Strip `/v3` → `{base}/../graphql` |
| GHE Cloud (dedicated) | `https://api.<host>` | `{base}/graphql` |

`GitHubClient::graphql_url()` handles the GHE Server case: if `api_base_url` ends with `/v3`, it strips the suffix before appending `/graphql`.

---

## 6. MCP Tools

20 tools total. Each operates on the configured repo. All tools follow the same execution pattern: validate input → execute business logic → if write: invalidate cache + rebuild + update Ready State fields → return result.

→ Detailed tool specifications: [03-spec-mcp-tools.md](./specs/03-spec-mcp-tools.md)

### 6.1 Tool Execution Pattern

```rust
pub async fn execute(state: &ServerState, params: P) -> Result<R, McpError> {
    validate(&params)?;
    let result = do_work(state, &params).await?;

    if is_write {
        state.cache.invalidate();
        let (issues, edges) = state.github.fetch_graph_data().await?;
        let graph = DependencyGraph::build(&issues, &edges);
        let ready_set = graph.compute_ready_set();
        update_ready_state_fields(state, &issues, &ready_set).await?;
        state.cache.update(graph);
    }

    Ok(result)
}
```

### 6.2 Tool Catalogue

| Tool | Type | Purpose |
|---|---|---|
| `ready` | Read | Find unblocked work — the fundamental question |
| `claim` | Write | Atomic assignment of issue to agent |
| `close` | Write | Close issue + cascade-unblock dependents |
| `create` | Write | Create issue with optional deps, parent, fields |
| `depends` | Write | Add blocking edge (with cycle detection) |
| `dep_remove` | Write | Remove blocking edge |
| `dep_cycles` | Read | Detect dependency cycles |
| `show` | Read | Full issue detail with comments and deps (always fresh) |
| `list` | Read | Filtered, sorted, paginated issue list |
| `search` | Read | GitHub Search API — bypasses cache |
| `stats` | Read | Aggregate counts by status, priority, agents |
| `prime` | Read | Context summary for agent session injection |
| `comment` | Write | Add comment to issue (no cache invalidation) |
| `update` | Write | Update issue fields, labels, body sections |
| `reopen` | Write | Reopen closed issue, evaluate blocking status |
| `init` | Write | Create Projects V2 board for the repo |
| `setup` | Write | Create fields, views, migrate issues (idempotent) |
| `doctor` | Read | Operational health checks with optional self-repair |
| `commit_context` | Read | Structured commit message with git trailers |
| `reconcile` | Write | Detect and repair semantic drift (7 drift types) |

### 6.3 Server Bootstrap

Load config → init tracing → create GitHub client → resolve repo + project + fields → validate fields → create cache → create server → serve on transport. If no project detected, bootstrap mode: only `init` and `setup` are functional.

### 6.4 ServerState

```rust
pub struct ServerState {
    pub config: Config,
    pub github: GitHubClient,
    pub cache: GraphCache,
}
```

Shared across all tool invocations. In Phase 05, `ServerState` moves to `unblock-tools` crate and is reused by both stdio and HTTP binaries.

---

## 7. Plugin Pipeline

Specialised agents and skills that turn the MCP server into a structured development pipeline. The plugin is Layer 2 — it adds agent intelligence and process enforcement on top of the MCP server (Layer 1).

→ Detailed specifications: [04-spec-plugin-pipeline.md](./specs/04-spec-plugin-pipeline.md)

### 7.1 Architecture

The plugin consists of skills (entry points), agents (personas), hooks (mechanical enforcement), and comment templates (shared vocabulary). All are `.md` configuration files — not compiled code.

Platforms: Claude Code (richest — agents, skills, hooks, `context: fork`), GitHub Copilot CLI (same skills, directive body language for routing), Cursor, Windsurf.

### 7.2 Skills

| Skill | Purpose |
|---|---|
| `/setup` | Bootstrap: GitHub labels, milestone, Projects V2, editor configs, hooks |
| `/need` | Intent-based agent discovery and installation |
| `/doctor` | Diagnostic: MCP health, GitHub state, local environment |
| `/think` | Free exploration — no pipeline enforcement, no required issue |
| `/plan` | Mode A: global vision (PRD → PLAN.md). Mode B: phase planning (plans/ + specs/ + GitHub Issues) |
| `/do` | Intent router: implementation (A), spec (B), investigation (C), spike (D), review (E), QA (F) |
| `/make` | Autonomous execution — same routing, no human-in-the-loop, stricter preconditions |
| `/use` | Direct agent dispatch by name |
| `/info` | Natural language query over project state |
| `/trail` | Structured narrative history of an issue |
| `/ship` | Pre-merge readiness check |

### 7.3 Agents

| Agent | Name | Role | Model | Tools |
|---|---|---|---|---|
| Investigator | Sherlock | Codebase analysis, root cause, approach | opus | Read, Glob, Grep, Bash (read-only), MCP (show, comment) |
| Architect | Ada | System design, specs, phase planning | opus | Read, Write, Edit, Glob, Grep, MCP (show, comment) |
| Product Owner | Fernando | Issue creation (sequential), findings tracking | sonnet | Read, MCP (show, create, update, depends, list, search) |
| Code Reviewer | Linus | Read-only review, structured findings, verdicts | opus | Read, Glob, Grep, Bash (read-only), MCP (show, comment, update) |
| QA Gate | Quinn | Spec conformity, tests, build, lint | opus | Read, Glob, Grep, Bash (test runners), MCP (show, comment, update) |
| Refactorer | Martin | Fix validated review findings | sonnet | Read, Write, Edit, Glob, Grep, Bash (full), MCP (show, comment, update) |
| Inspector | Gadget | Pipeline compliance — comment trail audit | sonnet | Read, Glob, Grep, MCP (show, comment, list) |
| Implementation | Dynamic | Tech-specific supervisors via `/need` | sonnet | Read, Write, Edit, Glob, Grep, Bash (full), MCP (show, comment, update) |

### 7.4 Comment Templates

All templates are the shared vocabulary between agents. Parsed by downstream agents and by the pipeline itself.

| Template | Written by | Consumed by |
|---|---|---|
| `DISPATCH` | Skill (before dispatch) | Supervisor |
| `INVESTIGATION` | Sherlock | Supervisor, `/do` router, `/trail` |
| `COMPLETED` | Supervisor | Linus, Quinn, `/trail` |
| `DECISION` | Any agent during implementation | Linus, Quinn, human, `/trail` |
| `DEVIATION` | Any agent during implementation | Linus, Quinn, human, `/trail` |
| `REVIEW` | Linus | Skill flow, Martin, human, `/trail` |
| `REFACTORING` | Martin | Skill flow, Linus (re-review), human |
| `QA` | Quinn | Skill flow, human, `/trail` |
| `AUDIT` | Gadget (violations only) | Skill flow, developer, `/trail` |
| `BLOCKED` | Any agent when stuck | Developer, `/trail` |
| `PAUSE` | Developer or agent | Any resuming agent, `/trail` |
| `FINDING` | Fernando (issue body) | Developer, parent epic tracking |

### 7.5 Three Enforcement Layers

Pipeline compliance is structurally impossible to bypass — all three layers must be circumvented simultaneously:

**Layer 1 — MCP validation.** The server rejects label transitions when preconditions are not met:

| Label | Precondition |
|---|---|
| `unblock:review:pending` | `COMPLETED` or `REFACTORING` comment exists |
| `unblock:review:ok` | `REVIEW` comment with `Verdict: APPROVE` |
| `unblock:review:rework` | `REVIEW` comment with `Verdict: NEEDS-REWORK` |
| `unblock:qa:ok` | `QA` comment with `Verdict: PASS` or `PASS+FINDINGS` |
| `unblock:qa:rework` | `QA` comment with `Verdict: FAIL` |

**Layer 2 — Inspector (Gadget).** Runs after every agent dispatch. Verifies structured comments exist, are well-formed, and follow the correct sequence. Writes `AUDIT` comment only when violations are found. Clean pipelines produce zero noise.

Gadget checks comment trail only. It does not check code, files, or spec/plan existence.

Sequence validation:
- `COMPLETED` must not exist before `INVESTIGATION` (unless `unblock:no-investigation`)
- `REVIEW` must not exist before `COMPLETED`
- `QA` must not exist before `REVIEW` with `Verdict: APPROVE`
- `unblock:review:ok` must not exist before `REVIEW` comment
- `unblock:qa:ok` must not exist before `QA` comment with PASS verdict

**Layer 3 — Agent prompt structure.** Numbered steps with explicit BLOCK conditions. The agent cannot proceed past a gate without the required artefact. Spec/plan existence is enforced here — BLOCK conditions in `/do` and `/make` prevent implementation without the required planning artefacts.

### 7.6 Labels

| Label | Meaning |
|---|---|
| `unblock:review:pending` | Implementation complete, awaiting review |
| `unblock:review:ok` | Review approved |
| `unblock:review:rework` | Review requires rework |
| `unblock:qa:ok` | QA passed — ready for merge |
| `unblock:qa:rework` | QA failed — requires attention |
| `unblock:needs-human` | Autonomous agent blocked — requires human decision |
| `unblock:paused` | Work intentionally paused |
| `unblock:no-investigation` | Issue does not require investigation phase |
| `unblock:finding:suggestion` | Review SUGGESTION finding |
| `unblock:finding:minor` | QA MINOR finding |
| `unblock:finding:risk` | QA RISK finding |
| `unblock:finding:deviation` | QA DEVIATES finding |
| `unblock:finding:extra` | QA EXTRA finding |

### 7.7 Hooks

| Hook | Trigger | Responsibility |
|---|---|---|
| `SessionStart` | Session beginning | Inject context via `prime`, detect mode, check plugin version |
| `PreToolUse → Task dispatch` | Subagent dispatch | Inject discipline reminder for supervisors |
| `PreToolUse → MCP write tools` | MCP write tool call | Enforce template discipline when issue referenced |
| `SessionEnd` | Session end | Compliance check, auto-unclaim if no COMPLETED |

### 7.8 Review and QA Flow

**Review verdict mapping:**

| Verdict | Condition | Label |
|---|---|---|
| APPROVE | No CRITICAL or WARNING (SUGGESTION allowed) | `unblock:review:ok` |
| NEEDS-REFACTORING | SUGGESTION only (no CRITICAL or WARNING) | `unblock:review:pending` (unchanged) |
| NEEDS-REWORK | Any CRITICAL or WARNING | `unblock:review:rework` |

**Finding action mapping:** CRITICAL and WARNING → rework (never finding issues). SUGGESTION → Fernando creates finding issue in parent epic, then applies `unblock:review:ok` label and posts a comment on the reviewed issue referencing each created finding (e.g., "SUGGESTION tracked as #123").

**QA verdict mapping:**

| Verdict | Condition | Label |
|---|---|---|
| PASS | No BLOCKER or MAJOR | `unblock:qa:ok` |
| PASS+FINDINGS | No BLOCKER or MAJOR, but MINOR/RISK/DEVIATES/EXTRA | `unblock:qa:ok` + findings |
| FAIL | Any BLOCKER or MAJOR | `unblock:qa:rework` |

**QA finding action mapping:** BLOCKER and MAJOR → rework (never finding issues). MINOR, RISK, DEVIATES, EXTRA → Fernando creates finding issues in parent epic, then applies `unblock:qa:ok` label and posts a comment on the reviewed issue referencing each created finding (e.g., "MINOR tracked as #124, RISK tracked as #125").

After Martin (refactoring), Linus always re-reviews. Never auto-approve.

### 7.9 Internal Skills

**`unblock-verify`** — invoked by Supervisor before writing `COMPLETED`. Detects test/build/lint commands from injected conventions (via `/need`), runs them, verifies acceptance criteria against code. Max 3 fix attempts per gate. If persists → BLOCKED.

---

## 8. Remote Server

The same MCP tools, served over Streamable HTTP from a persistent server. Foundation for teams and for the autonomous agent (§9).

→ Detailed specifications: [05-spec-remote-server.md](./specs/05-spec-remote-server.md)

### 8.1 Architecture

Phase 05 introduces two new crates:

**`unblock-tools`** (library) — all tool implementations extracted from `unblock-mcp`. Pure tool logic: validate input, call GitHub, rebuild graph, return result. No transport, no MCP bootstrap. Shared by both binaries.

**`unblock-mcp-remote`** (binary) — thin HTTP bootstrap:

```
POST /mcp              ← Streamable HTTP (rmcp + axum)
POST /webhooks/github  ← Webhook handler (HMAC-SHA256)
GET  /health           ← Health check
```

### 8.2 Shared Graph Cache

```rust
pub struct SharedGraphCache {
    inner: DashMap<CacheKey, Arc<RwLock<CacheEntry>>>,
}

#[derive(Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    pub repo: RepoKey,              // "owner/repo"
    pub token_fingerprint: TokenFingerprint,  // SHA-256(token)
}
```

Keyed by `(owner/repo, token_fingerprint)` — two users connecting to the same repo get independent caches (different token permissions may see different issues). `DashMap` provides concurrent access without a global lock. Each entry has its own `RwLock`.

The graph survives between sessions. No cold start after the first connection.

### 8.3 Auth Model

The client's GitHub token is simultaneously identity, credential, and scope. No user database, no API keys, no sessions to store.

```
Authorization: Bearer ghp_xxxxxxxxxxxxxxxxxxxx
```

Token validated once per session via GitHub `GET /user`. Cached by `TokenFingerprint` (SHA-256) for 5 minutes in `IdentityCache`. Token is wrapped in `secrecy::SecretString` — never logged, never stored beyond request lifetime.

### 8.4 Session Context via `initialize`

The MCP `initialize` handshake carries repo context:

```json
{
  "method": "initialize",
  "params": {
    "meta": {
      "unblock:repo": "websublime/unblock",
      "unblock:project": "42"
    }
  }
}
```

Background cache warm-up dispatched immediately after initialize — agent gets response without waiting.

### 8.5 Webhook Handler

Receives GitHub `issues` events. Invalidates cache on: closed, reopened, opened, labeled, unlabeled. Does not trigger rebuild — next tool call rebuilds lazily. Webhook handler is fast (<1ms) and avoids redundant rebuilds during cascades.

HMAC-SHA256 verification via `WEBHOOK_SECRET` env var.

### 8.6 GHE Support

`GITHUB_API_URL` set at deploy time. No per-request override — accepting arbitrary GitHub API URLs per request introduces SSRF risk. The server routes to one GitHub instance.

### 8.7 Transport

Streamable HTTP only (current MCP spec standard, 2025-03). SSE excluded — deprecated, two-endpoint complexity. Single endpoint: `POST /mcp`.

---

## 9. LLM Agent

Autonomous investigation and code review. Co-deployed with the remote server. Client of `unblock-mcp-remote` via HTTP — not a Rust dependency.

→ Detailed specifications: [06-spec-llm-agent.md](./specs/06-spec-llm-agent.md)

### 9.1 Design Principles

- Read-only except for comments. Never creates branches, claims issues, or modifies code.
- Idempotent. If a structured comment already exists, skip and exit.
- Ephemeral per event. Each webhook triggers one agent run. No persistent state between runs.
- GitHub is still the source of truth. Zero custom storage.

### 9.2 Flow 1 — Investigation (Sherlock)

**Trigger:** `issues.labeled` where `label.name == "needs-investigation"`

1. MCP `show` → check for existing `INVESTIGATION:` comment → exit if found (idempotent)
2. Parse issue body (Description, Design Notes, Acceptance Criteria)
3. If sub-issue → MCP `show` parent for epic context
4. GitHub Contents API → discover relevant files (directory tree + code search)
5. Fetch file content (parallel, max 12 files, max 400 lines each)
6. Codestral → produce structured investigation
7. MCP `comment` → post `INVESTIGATION:` comment
8. GitHub REST → remove `needs-investigation` label

### 9.3 Flow 2 — PR Review (Linus)

**Trigger:** `pull_request.opened` or `pull_request.ready_for_review`

1. Extract linked issue number from PR body (`Closes #N`, `Fixes #N`, `Resolves #N`). If none → post comment requesting link, exit
2. MCP `show` → read issue + comment trail. Check for existing `REVIEW:` → exit if found
3. Verify `COMPLETED:` comment exists. If not → post note, exit
4. GitHub REST → fetch PR diff
5. Codestral → cross-reference diff against acceptance criteria + comment trail
6. MCP `comment` → post `REVIEW:` comment on the Issue
7. GitHub REST → submit PR Review with `event: "COMMENT"` (never APPROVE or REQUEST_CHANGES)

### 9.4 LLM Strategy

| Model | Role |
|---|---|
| Codestral (Mistral, 22B) | Primary — code specialist, cheapest, OpenAI-compatible API |
| Mistral Small 3.1 (24B) | Fallback — stronger instruction following if format compliance drops |

Framework: `rig` (Rust-native, tool calling, OpenAI-compatible). Cost: ~€0.001/investigation, ~€0.0005/review.

### 9.5 Safety Limits

| Parameter | Default |
|---|---|
| Max LLM turns | 15 |
| Max files fetched | 12 |
| Max lines per file | 400 |
| Max input tokens | 80,000 |
| Run timeout | 120s |

If limits exceeded → post `[INCOMPLETE]` comment with partial findings.

### 9.6 Auth

Dedicated GitHub service account (fine-grained PAT initially, GitHub App for production — `unblock-agent[bot]` identity). Token scoped to: Issues (R/W), Pull Requests (R/W), Contents (R), Metadata (R). No code write access — enforced by token scope.

Authenticates to `unblock-mcp-remote` via `Authorization: Bearer` like any other client. Separate webhook subscription from remote MCP (Option A — independent endpoints).

### 9.7 Relationship with Plugin Agents

The autonomous agent is a fast first pass — not a replacement for the plugin's interactive Sherlock and Linus (Claude/opus in-session). The plugin's review is the gating review. The autonomous review uses `event: "COMMENT"` — it never blocks merges.

---

## 10. Harness

Orchestration layer above the plugin. Pre-defined workflows that compose the plugin's agents and skills into coordinated multi-step sequences. Phase 07 — design is outline-level.

### 10.1 Concept

A harness takes a high-level intent and executes the full pipeline automatically:

```
"build feature X from scratch"  →  /think → /plan → /do → review → QA → /ship
"triage and fix this bug"       →  /do investigate → /do implement → review → QA
"plan the next sprint"          →  /plan (global) → /plan (per phase)
```

### 10.2 Agent Team Patterns

| Pattern | Description |
|---|---|
| Pipeline | Sequential phases — output of one feeds input of next |
| Fan-out | Multiple agents work in parallel on independent sub-tasks |
| Producer-reviewer | One agent implements, another reviews — structurally enforced |
| Supervisor | Coordinating agent dispatches and monitors worker agents |

### 10.3 Boundaries

The harness does not add new capabilities. It composes what the plugin already provides. Each step uses the same skills, agents, enforcement layers, and comment templates.

The harness never merges — it produces a branch that a human can review before merge.

---

## 11. Configuration

### 11.1 Environment Variables (Local + Shared)

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `GITHUB_TOKEN` | Yes | — | GitHub authentication |
| `GITHUB_API_URL` | No | `https://api.github.com` | GHE Server: `https://<host>/api/v3` |
| `GITHUB_URL` | No | `https://github.com` | GHE Server: `https://<host>` |
| `UNBLOCK_REPO` | No | Auto-detect from git remote | Repository `owner/repo` |
| `UNBLOCK_PROJECT` | No | Auto-detect from linked projects | Project number |
| `UNBLOCK_AGENT` | No | `"agent"` | Default agent name |
| `UNBLOCK_CACHE_TTL` | No | `30` | Cache TTL in seconds |
| `UNBLOCK_LOG_LEVEL` | No | `"info"` | Log level |
| `UNBLOCK_OTEL_ENDPOINT` | No | — | OpenTelemetry collector |

### 11.2 Environment Variables (Remote-only)

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `BIND_ADDR` | No | `0.0.0.0:3000` | TCP bind address |
| `WEBHOOK_SECRET` | No | — | GitHub webhook HMAC-SHA256 secret |
| `IDENTITY_CACHE_TTL` | No | `300` | Token identity cache TTL (seconds) |

### 11.3 Environment Variables (LLM Agent)

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `MISTRAL_API_KEY` | Yes | — | Mistral API authentication |
| `UNBLOCK_REMOTE_URL` | Yes | — | Remote MCP endpoint URL |
| `AGENT_GITHUB_TOKEN` | Yes | — | Service account token |
| `AGENT_WEBHOOK_SECRET` | No | — | Agent webhook HMAC secret |
| `AGENT_MAX_TURNS` | No | `15` | Max LLM turns per run |
| `AGENT_MAX_FILES` | No | `12` | Max files per investigation |
| `AGENT_RUN_TIMEOUT` | No | `120` | Seconds per agent run |

### 11.4 Config Loading

```rust
pub struct Config {
    pub token: String,
    pub api_base_url: String,
    pub github_url: String,
    pub repo: Option<String>,
    pub project_number: Option<u64>,
    pub agent: String,
    pub cache_ttl: u64,
    pub log_level: String,
    pub otel_endpoint: Option<String>,
}

impl Config {
    pub fn load_from(
        env: impl Fn(&str) -> Result<String, VarError>,
    ) -> Result<Self, DomainError> { /* ... */ }
}
```

No config file. Environment variables only. The `load_from` pattern accepts a custom env reader — tests supply a `HashMap`-backed closure instead of mutating process-global state (`std::env::set_var` is `unsafe` in edition 2024).

---

## 12. Error Model

Every crate uses `snafu` exclusively. No `thiserror`, no `anyhow`, no `Box<dyn Error>`. `unwrap()` and `expect()` forbidden outside test modules.

### 12.1 Domain Errors (unblock-core)

```rust
#[derive(Debug, Snafu)]
pub enum DomainError {
    IssueNotFound { number: u64 },
    AlreadyClaimed { number: u64, agent: String },
    IssueBlocked { number: u64, blockers: Vec<u64> },
    IssueDeferred { number: u64, until: String },
    IssueClosed { number: u64 },
    IssueNotClosed { number: u64 },
    IssueAlreadyOpen { number: u64 },
    CircularDependency { source: u64, target: u64 },
    DuplicateDependency { source: u64, target: u64 },
    FieldNotFound { name: String },
    Validation { message: String },
    InvalidIssueRef { input: String },
    CrossRepoAccessDenied { owner: String, repo: String },
}
```

### 12.2 Infrastructure Errors (unblock-github)

```rust
#[derive(Debug, Snafu)]
pub enum Error {
    Domain { source: DomainError },
    GitHubApi { message: String },
    GitHubGraphQL { errors: Vec<String> },
    GitHubUnavailable { source: reqwest::Error },
    RateLimited,
    CircuitBreakerOpen,
    ProjectNotConfigured,
    GitRemote { message: String },
    ViewCreationFailed { message: String },
    OwnerDetectionFailed { owner: String, message: String },
}
```

### 12.3 Error Code Mapping

| Error | HTTP Code | Trigger |
|---|---|---|
| `IssueNotFound` | 404 | Any tool with invalid issue number |
| `AlreadyClaimed` | 409 | `claim` on in-progress issue |
| `IssueBlocked` | 409 | `claim` on blocked issue |
| `IssueClosed` | 409 | `close`/`claim`/`update` on closed issue |
| `CircularDependency` | 422 | `depends` that would create cycle |
| `DuplicateDependency` | 409 | `depends` on existing edge |
| `Validation` | 400 | Any input validation failure |
| `InvalidIssueRef` | 400 | Malformed issue reference string |
| `CrossRepoAccessDenied` | 403 | Token lacks access to cross-repo issue |

### 12.4 Error Propagation

`DomainError` (core) → `Error` (github, via `#[snafu(context(false))]`) → `McpError` (mcp, via `From<Error>`). Each layer adds context without losing the original.

---

## 13. Observability

### 13.1 Logging

All via `tracing` crate. JSON to stderr (stdout is reserved for MCP protocol on stdio transport). Remote server logs to stdout (no stdio conflict).

```rust
tracing::info!(
    tool = "ready",
    repo = %state.github.repo,
    cache_hit = true,
    duration_ms = 12,
    result_count = 5,
    "Ready query completed"
);
```

### 13.2 Log Levels

| Level | Content |
|---|---|
| `error` | Failed operations, GitHub errors, circuit breaker trips |
| `warn` | Stale cache served, skipped blocking edges with unknown issues |
| `info` | Tool invocations, result summaries, setup progress, repo/project detection |
| `debug` | GitHub request/response details (token redacted), graph computation |
| `trace` | MCP protocol messages, cache operations |

### 13.3 Metrics (OpenTelemetry, optional)

| Metric | Type | Labels |
|---|---|---|
| `unblock.tool.duration` | Histogram | `tool`, `status` |
| `unblock.github.request.duration` | Histogram | `api` (graphql/rest), `status` |
| `unblock.cache.hits` | Counter | `tool` |
| `unblock.cache.misses` | Counter | `tool` |
| `unblock.graph.nodes` | Gauge | — |
| `unblock.graph.edges` | Gauge | — |
| `unblock.graph.cycles` | Gauge | — |
| `unblock.graph.recalculations` | Counter | `trigger` (write/stale) |

---

## 14. Resilience

### 14.1 Circuit Breaker

```rust
pub struct CircuitBreaker {
    state: CircuitState,          // Closed, Open, HalfOpen
    failure_count: usize,
    failure_threshold: usize,     // 5
    cooldown: Duration,           // 10s
}
```

After 5 consecutive GitHub API failures, the circuit opens — all subsequent requests fail immediately with `CircuitBreakerOpen` for 10 seconds. After cooldown, transitions to HalfOpen — one request allowed. Success → Closed. Failure → Open again.

### 14.2 Retry Policy

```rust
pub struct RetryPolicy {
    pub max_retries: usize,       // 3
    pub base_delay: Duration,     // 500ms
    pub max_delay: Duration,      // 5s
}
```

Exponential backoff with ±25% jitter. Only retries on `RateLimited` (429) and `GitHubUnavailable` (503). All other errors propagate immediately.

---

## 15. Security

### 15.1 Token Handling

- `GITHUB_TOKEN` loaded from environment variable only
- Never logged (redacted in debug output)
- Never included in MCP tool responses
- Never embedded in binary
- Remote: wrapped in `secrecy::SecretString`, never stored beyond request lifetime
- Plugin `.mcp.json` uses `${GITHUB_TOKEN}` expansion

### 15.2 Input Validation

| Field | Validation |
|---|---|
| Issue numbers | Positive integers |
| Titles | Non-empty, max 500 chars |
| Agent names | Non-empty, max 100 chars |
| Priority | Must be P0–P4 |
| Dates | Valid ISO format |

### 15.3 Transport Security

**Local (stdio):** Process-local. No network exposure. Only the spawning process can communicate.

**Remote (Streamable HTTP):** TLS termination at load balancer/proxy. Auth via `Authorization: Bearer`. Webhook verification via HMAC-SHA256.

### 15.4 SSRF Prevention

No per-request GitHub API URL override on the remote server. `GITHUB_API_URL` is set at deploy time. Accepting arbitrary URLs from untrusted clients introduces SSRF risk.

### 15.5 LLM Agent Token Scope

Agent token scoped to: Issues (R/W), Pull Requests (R/W), Contents (R), Metadata (R). No code write access — enforced by token scope, not just by convention.

---

## 16. Build & Distribution

### 16.1 Workspace

```toml
[workspace]
members = [
    "crates/unblock-core",
    "crates/unblock-github",
    "crates/unblock-tools",         # Phase 05
    "crates/unblock-mcp",
    "crates/unblock-mcp-remote",    # Phase 05
    "crates/unblock-agent",         # Phase 06
]
resolver = "2"

[workspace.package]
edition = "2024"
repository = "https://github.com/websublime/unblock"

[workspace.lints.rust]
unsafe_code = "deny"
missing_docs = "warn"
```

### 16.2 Release Targets

| Platform | Target |
|---|---|
| Linux x86_64 | `x86_64-unknown-linux-musl` |
| Linux ARM64 | `aarch64-unknown-linux-musl` |
| macOS x86_64 | `x86_64-apple-darwin` |
| macOS ARM64 | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

### 16.3 Distribution Channels

| Channel | Binary | Mechanism |
|---|---|---|
| GitHub Releases | `unblock-mcp` | cargo-dist (auto-generated `release.yml`, tag `unblock-mcp-v*`) |
| Homebrew | `unblock-mcp` | `brew install websublime/tap/unblock` |
| npm | `unblock-mcp` | `npx @unblock/cli` |
| Docker | `unblock-mcp-remote` + `unblock-agent` | `ghcr.io/websublime/unblock-mcp-remote` (tag `unblock-mcp-remote-v*`) |
| Shell/PowerShell | `unblock-mcp` | Installer scripts via cargo-dist |

### 16.4 Licensing

| Crate | License |
|---|---|
| `unblock-core` | MIT |
| `unblock-github` | MIT |
| `unblock-tools` | MIT |
| `unblock-mcp` | MIT |
| `unblock-mcp-remote` | BSL 1.1 → MIT (4 years) |
| `unblock-agent` | BSL 1.1 → MIT (4 years) |

---

## 17. Testing

### 17.1 Strategy

| Layer | Type | What | GitHub Required? |
|---|---|---|---|
| `unblock-core` | Unit | Graph engine, cache, types, config | No |
| `unblock-core` | Property | Graph invariants (proptest) | No |
| `unblock-github` | Unit | Error conversion, URL construction | No |
| `unblock-mcp` | Unit | Body section parsing, error conversion | No |
| `unblock-mcp` | Integration | Full tool flows against real repo | Yes |
| `unblock-tools` | Unit | Tool validation logic | No |
| `unblock-mcp-remote` | Integration | Auth, webhook, shared cache | Yes |
| `unblock-agent` | Unit | Prompt formatting, idempotency detection | No |
| `unblock-agent` | Integration | End-to-end agent run against test repo | Yes |

### 17.2 Quality Gate

Every change must pass:

```bash
cargo fmt --check --all                                    # zero diffs
cargo clippy --workspace --all-targets -- -D warnings      # zero warnings
cargo test --workspace                                     # all pass
cargo doc --no-deps --workspace                            # zero warnings
```

Coverage target: >80% for Phase 01–02, 100% from Phase 03 onwards.

### 17.3 Property Tests

```rust
proptest! {
    #[test]
    fn ready_set_never_contains_blocked_issues(
        issues in vec(arb_issue(), 1..100),
        edges in vec(arb_edge(), 0..200),
    ) { /* ... */ }
}
```

Graph invariants validated by proptest: ready set excludes blocked issues, cascade produces correct promotions, cycle detection is sound and complete.

### 17.4 Test-hooks Feature

The `test-hooks` cargo feature gates test-only code paths (e.g., `set_project_fields`) behind a compile-time flag. Never enabled in production builds.

---

*This document defines how unblock works technically. The why is in the MANIFESTO. The what and when is in the PRD. Detailed algorithms and edge cases are in the per-component specs.*
