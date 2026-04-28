# ://unblock — Technical Specification

> Version: 0.1-draft  
> Status: Working Draft  
> Companions: [MANIFESTO.md](./MANIFESTO.md) · [PRD.md](./PRD.md)  
> Plans: [01-mcp-foundation](./plans/01-plan-mcp-foundation.md) · [02-mcp-complete](./plans/02-plan-mcp-complete.md) · [03-code-indexer](./plans/03-plan-code-indexer.md) · [04-mcp-production](./plans/04-plan-mcp-production.md) · 05-plugin (planned) · 06-remote-server (planned) · 07-llm-agent (planned) · 08-harness (planned)  
> Specs: [01-mcp-foundation](./specs/01-spec-mcp-foundation.md) · 02-mcp-complete (planned) · 03-code-indexer (planned) · 04-mcp-production (planned) · 05-plugin-pipeline (planned) · 06-remote-server (planned) · 07-llm-agent (planned)

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
| **claim** | Atomic assignment of an issue to an agent. Sets agent name, Status→`in_progress`, and timestamp in a single operation. Two agents cannot claim the same issue. |
| **dependency graph** | The directed acyclic graph of blocking relationships between issues. Edges represent "blocked by" — the source depends on the target. The graph is the product. |
| **qualified ID** | A fully-qualified issue identifier: `owner/repo#number`. Prevents collision between issue #5 in repo A and issue #5 in repo B. Used internally by the graph engine. |
| **graph cache** | The in-memory cache of the computed dependency graph and ready set. Ephemeral — invalidated after every write, reconstructable from GitHub in a single API call. |
| **shared graph cache** | The remote server's multi-tenant graph cache. `DashMap<CacheKey, Arc<RwLock<CacheEntry>>>` keyed by `(owner/repo, token_fingerprint)`. Survives between sessions. |
| **session isolation** | The architectural guarantee that planning, investigation, implementation, review, and QA run in separate sessions. The comment trail is the sole medium of communication between sessions. |
| **structured comment** | A comment posted to a GitHub Issue following a defined template. Types: DISPATCH, INVESTIGATION, COMPLETED, DECISION, DEVIATION, REVIEW, REFACTORING, QA, AUDIT, BLOCKED, PAUSE. |
| **comment trail** | The chronological sequence of structured comments on an issue. Reconstructable by any agent or human. The shared memory that makes session isolation possible. |
| **enforcement layer** | One of three independent mechanisms that prevent pipeline violations: MCP validation (label transition preconditions), Inspector (Gadget — comment trail audit), agent prompt structure (BLOCK conditions). |
| **worktree** | Isolated git worktree at `worktrees/issue-{N}-{slug}` used for implementation and refactoring. The branch is a consequence of the worktree — the worktree is the primary concept. |
| **Projects V2 field** | Custom field on a GitHub Projects V2 board. unblock uses 7: Status, Priority, Pipeline Stage, Agent, Claimed At, Story Points, Defer Until. |
| **body sections** | Three structured sections in the issue body markdown: Description, Design Notes, Acceptance Criteria. Parsed and written by the MCP server. Each data type lives in the correct GitHub primitive — not duplicated. |
| **cross-repo** | Blocking relationships that span repositories. GitHub Issue node IDs are globally unique. unblock supports `owner/repo#number` references for cross-repo dependencies. |
| **MCP** | Model Context Protocol. The communication protocol between AI agents and tool servers. unblock uses stdio (local) and Streamable HTTP (remote). |
| **skill** | A slash-command entry point in the plugin. Lives in `skills/{name}/SKILL.md`. Invoked as `/name`. Skills route intent to agents — they are dispatchers, not executors. |
| **agent** | A named `.md` configuration file that defines a specialised persona with constrained tools, model, and hard boundaries. Not compiled code. |
| **finding** | A deferrable issue created by Fernando when review (SUGGESTION) or QA (MINOR, RISK, DEVIATES, EXTRA) produces observations that do not block merge but should be tracked. Created as sub-issue of the parent epic issue. |
| **drift** | Semantic divergence between the in-memory graph and GitHub reality. Caused by external mutations (human closes an issue via GitHub UI). Detected and repaired by `reconcile`. |

---

## 2. Data Model

unblock stores zero custom data. All state lives in GitHub Issues and Projects V2 fields. The MCP server is a compute layer over existing GitHub primitives.

→ Detailed algorithms and edge cases: [01-spec-mcp-foundation.md](./specs/01-spec-mcp-foundation.md)

### 2.1 GitHub Primitives

| Primitive | Purpose | API |
|---|---|---|
| Issue number | Issue ID (`#42`) — native, universal | REST + GraphQL |
| Issue state | Open/Closed ground truth | REST + GraphQL |
| Issue type | Classification (org-level): `task`, `bug`, `feature`, `epic`, `chore`, `spike`. Epic issues serve as parent containers for sub-issues | REST + GraphQL |
| Labels | Flexible tagging, filterable | REST + GraphQL |
| Assignees | Human assignment | REST + GraphQL |
| Milestones | Sprint/release grouping with due date and progress | REST + GraphQL |
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
| **Status** | Single Select | `ready`, `in_progress`, `blocked`, `deferred`, `closed` | Unified workflow + readiness state. `ready`/`blocked` computed by MCP server from graph; `in_progress`/`deferred`/`closed` set by agent/human |
| **Priority** | Single Select | `P0 - Critical`, `P1 - High`, `P2 - Medium`, `P3 - Low`, `P4 - Backlog` | Sortable priority for the `ready` queue |
| **Pipeline Stage** | Single Select | `investigation`, `implementation`, `review`, `refactoring`, `qa`, `done` | Development pipeline phase. Set by plugin agents on transitions. Source of truth for routing — replaces comment-trail-based inference |
| **Agent** | Text | Free text | Which AI agent is working on this |
| **Claimed At** | Date | ISO datetime | Timestamp of claim |
| **Story Points** | Number | Integer | Estimation |
| **Defer Until** | Date | Date | Hidden from ready queue until this date |

Why custom fields over labels: fields are typed, filterable, sortable, and groupable in Projects V2 views. The Agent field is text — it does not pollute the label namespace.

**Status state machine:** The MCP server manages `ready`↔`blocked` transitions automatically based on the dependency graph. When an agent claims an issue, Status moves to `in_progress`. When deferred, to `deferred`. When closed, to `closed`. On close cascade, newly unblocked dependents transition from `blocked` to `ready`.

**Pipeline Stage state machine:** Plugin agents advance the pipeline stage as work progresses. The `/do` skill reads Pipeline Stage to determine routing — not the comment trail. Comments remain as audit trail but are no longer the source of truth for routing decisions.

```
investigation → implementation → review → refactoring → review → qa → done
                                   ↑          │
                                   └──────────┘ (rework cycle)
```

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
| Epic grouping | **Issue Type `Epic`** + **Sub-Issues** | Epic is an issue with type `Epic`; tasks are sub-issues of the epic |
| Sprint/release grouping | **Milestones** | Native with due dates and progress bar |
| Parent-child hierarchy | **Sub-Issues** | Native API (GA 2025) |
| Blocking relationships | **Blocking API** | `blockedBy`/`blocking` native |

### 2.4 Dependency Model

Single blocking type. GitHub's native `blockedBy`/`blocking` relationship. Binary: an issue either blocks another or it does not. No typed dependencies.

Informational links via issue mentions in comments or body — human/agent readable but not machine-evaluated for blocking.

### 2.5 Pre-configured Views

Created by `setup`. Five views provide opinionated board layouts:

| View | Layout | Purpose |
|---|---|---|
| `𝍄 UNBLOCK://ready` | Board | Agent's ready queue — filtered to `Status:"ready"` |
| `𝍄 UNBLOCK://team` | Board | Tech lead view — grouped by Agent |
| `𝍄 UNBLOCK://pipeline` | Board | Development pipeline — grouped by Pipeline Stage |
| `𝍄 UNBLOCK://roadmap` | Table | Epic-level progress by issue type `Epic` |
| `𝍄 UNBLOCK://timeline` | Roadmap | Date-based timeline for sprint planning |

---

## 3. Graph Engine

Pure Rust, no network, fully testable with in-memory data. Lives in `unblock-core/src/graph.rs`.

→ Detailed algorithms and edge cases: [01-spec-mcp-foundation.md](./specs/01-spec-mcp-foundation.md)

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
1. `IssueState == Open` (not Closed)
2. `Status` is not a preserved state (`InProgress`, `Deferred`, `Closed` are set by agent/human and never overridden)
3. No active blocking dependencies (all blockers have `IssueState::Closed`)

The ready set computation does NOT look at the current `Status` field value to decide readiness — it computes readiness from the graph. Issues with `Status::Blocked` whose blockers are now all closed WILL appear in the ready set. The `update_status_fields` algorithm syncs the Status field to match.

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

→ Detailed algorithms and edge cases: [01-spec-mcp-foundation.md](./specs/01-spec-mcp-foundation.md)

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
    ready_set: Vec<IssueSummary>,
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
  ├─→ Diff against current Status field values
  ├─→ Batch update changed Status fields in GitHub (ready↔blocked transitions)
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

### 4.7 Materialised Fast Path (Phase 04, planned)

On cold start, the server reads Status field values directly from Projects V2 instead of waiting for a full graph rebuild. These field values are written by the MCP server on every mutation and by `reconcile`, so they reflect the ready set. The server serves this approximate result immediately and rebuilds the full graph asynchronously in the background.

Once the graph rebuild completes (typically 1-3 seconds), subsequent `ready` calls use the authoritative graph. The fast path result is marked with `source: "field"` so the agent knows it is approximate.

The fast path is read-only — it never writes to GitHub. No new dependencies. No persistent storage. The existing Status field is both the human-visible board column and the cold start cache.

→ Detailed algorithm: [01-spec-graph-engine.md §10](./specs/01-spec-graph-engine.md#10-fast-path-algorithm-phase-04)

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

The `unblock-mcp` binary serves **two tool sets** from the same process:

1. **Issue-graph tool set** — 20 tools (Phases 01–02) backed by `unblock-tools` (extracted from `unblock-mcp` in Phase 06). Operates on the configured repo. All issue-graph tools follow the same execution pattern: validate input → execute business logic → if write: invalidate cache + rebuild + update Status fields → return result. Catalogued in §6.2.
2. **Code-indexer tool set** — 9 tools (Phase 03) backed by `unblock-indexer` / `unblock-indexer-core`. Operates on the local filesystem (the working repo). Backed by SQLite + FTS5 with tree-sitter WASM grammars fetched at runtime. Catalogued in §6.5 (added in Phase 03).

→ Detailed tool specifications (issue-graph): [03-spec-mcp-tools.md](./specs/03-spec-mcp-tools.md)
→ Detailed tool specifications (code-indexer): `docs/specs/03-spec-code-indexer.md` (planned, after research validation)

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
        update_status_fields(state, &issues, &ready_set).await?;
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
    pub config: Arc<Config>,
    pub github: Arc<dyn GitHubApi>,
    pub cache: Arc<GraphCache>,
}
```

Shared across all tool invocations. `Arc<dyn GitHubApi>` enables dependency injection — tests use `MockGitHubClient`, production uses `GitHubClient`. Phase 02 adds `agent_kind: OnceLock<AgentKind>` and `agent_client: OnceLock<AgentClient>`. In Phase 06, `ServerState` moves to `unblock-tools` crate and is reused by both stdio and HTTP binaries. Phase 03 adds `indexer: Arc<IndexerHandle>` for the code-indexer tool set; the handle is independent of `GitHubApi` and operates on the local filesystem.

### 6.5 Code-Indexer Tool Set (Phase 03)

Authored by Phase 03. Lives in two crates:

- `unblock-indexer-core` — pure Rust. Domain types (symbol kinds, span, query input/output shapes), AST traversal logic over `tree_sitter::Tree`, schema constants. Mirrors the `unblock-core` boundary — zero IO, zero async.
- `unblock-indexer` — impure shell. SQLite (sqlx + FTS5, WAL), tree-sitter WASM runtime, grammar fetcher (reuses the Phase 02 retry / circuit-breaker / OpenTelemetry layer), file walker (`ignore` crate), file watcher (`notify-debouncer-full`), bootstrap parallelism (`rayon`).

| Tool | Type | Purpose |
|---|---|---|
| `find_symbol` | Read | Locate symbols by name (optional kind / language / fuzzy / limit) |
| `list_symbols` | Read | All symbols in a file or path |
| `outline` | Read | Hierarchical tree of file/module structure |
| `get_symbol` | Read | Full details for an opaque `symbol_id` (body read from filesystem on demand) |
| `search_text` | Read | FTS5 matches across names, signatures, comments |
| `find_references` | Read | Best-effort syntactic references — **explicitly marked HEURISTIC** in the tool description |
| `list_languages` | Read | Loaded grammars for the current repo |
| `index_status` | Read | Freshness, last update, totals |
| `reindex` | Write (local) | Force re-parse for whole repo or a path |

Storage layout: `~/.cache/unblock/repos/<repo-hash>/index.db` (SQLite + FTS5 + WAL) and `~/.cache/unblock/grammars/*.wasm` (integrity-verified WASM grammars fetched from versioned GitHub Releases). No body text stored — span-only.

Initial language coverage (Top-10): Rust, TypeScript, JavaScript, Python, Go, Java, C, C++, Ruby, PHP. PR-driven expansion via the CI grammar matrix. `LanguageNotSupported` errors include a `pr_pointer` to the contribution template.

→ Detailed plan: [03-plan-code-indexer.md](./plans/03-plan-code-indexer.md)
→ Detailed spec (planned, post-research): `docs/specs/03-spec-code-indexer.md`

---

## 7. Plugin Pipeline

Specialised agents and skills that turn the MCP server into a structured development pipeline. The plugin is Layer 2 — it adds agent intelligence and process enforcement on top of the MCP server (Layer 1).

→ Detailed specifications: [04-spec-plugin-pipeline.md](./specs/04-spec-plugin-pipeline.md)

### 7.1 Architecture

The plugin is a typed Rust crate `unblock-plugin` whose responsibility is to translate a single authoritative catalog (8 personas, 20 skills, 3 hooks, 14 dynamic supervisors) into the markdown / JSON files that Claude Code and GitHub Copilot read. The data model is the source of truth; renderers emit per-target artefacts. `/setup` is the single entry point.

```
crates/unblock-plugin/
└── src/
    ├── lib.rs
    ├── model/          # Persona, Skill, Hook, Pipeline, Label, CommentKind, TransitionRule, Supervisor, DispatchConvention, SkillHandler
    ├── catalog/        # 8 personas, 20 skills, 3 hooks, 14 supervisors (data + templates)
    ├── detect/         # Stack detection (manifests + docs/PRD|MANIFESTO|SPEC)
    ├── render/
    │   ├── claude_code.rs
    │   ├── copilot_cloud.rs
    │   └── copilot_local.rs
    └── cli.rs          # `unblock-plugin render --target=<t> --supervisors=<list> --out=<dir>`
```

The description-contract lint runs in `build.rs`. Render output is byte-deterministic (no timestamps, no random IDs).

### 7.2 Targets and Files Produced

| Target | Manifest | Agents path | Skills path | Hooks | MCP config |
|---|---|---|---|---|---|
| Claude Code | `.claude-plugin/plugin.json` + `.claude-plugin/marketplace.json` + `CLAUDE.md` | `.claude/agents/*.md` | `.claude/skills/<n>/SKILL.md` | `.claude/hooks/` (3 scripts + `hooks.json`) | `.claude/settings.json` |
| Copilot cloud | `.github/copilot-instructions.md` | `.github/agents/*.md` | `.claude/skills/<n>/SKILL.md` (unified) | `.github/hooks/*.json` (3) | GitHub UI — guide printed in chat |
| Copilot local | `.github/copilot-instructions.md` | — | — | — | VS Code UI — guide printed in chat |

Skills directory is unified at `.claude/skills/` because Copilot cloud officially reads both `.claude/skills` and `.github/skills`.

Universal files (every target): `AGENTS.md`, `UNBLOCK-WORKFLOW.md`.

### 7.3 Skills

20 user-invocable skills + 1 shared-only knowledge pack.

| # | Skill | Stage | Persona / actor |
|---|---|---|---|
| 1 | `workflow` | Meta | Meta-orchestrator (no args ⇒ asks user) |
| 2 | `setup` | Ops | Daphne |
| 3 | `add-supervisor` | Ops | Daphne |
| 4 | `product` | 1 | Orchestrator (Grace + Ada) |
| 5 | `manifesto` | 1 | Grace |
| 6 | `requirements` | 1 | Grace |
| 7 | `architecture` | 1 | Ada |
| 8 | `specification` | 2 | Orchestrator (Ada + Smith + Fernando) |
| 9 | `plan` | 2 | Ada |
| 10 | `research` | 2 | Smith |
| 11 | `spec` | 2 | Ada |
| 12 | `tasks` | 2 | Fernando |
| 13 | `implementation` | 3 | Orchestrator (Supervisor + Sherlock + Linus + Quinn + Fernando) |
| 14 | `investigate` | 3 | Sherlock |
| 15 | `do` | 3 | Supervisor (dynamic) |
| 16 | `review` | 3 | Linus + Fernando — code-level (implementation correctness, gaps, code review) |
| 17 | `quality` | 3 | Quinn + Fernando — output-level (tests, spec conformance, acceptance) |
| 18 | `update` | Ops | Fernando |
| 19 | `reconcile` | Ops | (MCP) |
| 20 | `doctor` | Ops | (MCP) |

**Shared-only:** `subagents-discipline`.

**Description contract (lint enforced in `build.rs`).** Each user-invocable skill's `description` MUST start with an imperative verb, name the input object, include a trigger phrase, and end with a stage tag `[product] | [spec] | [impl] | [ops]`. The contract drives Copilot cloud's natural-language invocation (no slash command).

**`/workflow` invocation modes:**

| Form | Behaviour |
|---|---|
| `/workflow` | Show global state and ask the user which stage / skill to run |
| `/workflow product` | Delegate to `/product` |
| `/workflow specification` | Delegate to `/specification` (asks phase NN) |
| `/workflow implementation` | Delegate to `/implementation` (asks phase NN) |
| `/workflow next` | Auto-determine the next pending step and dispatch |
| `/workflow <skill>` | Verify prerequisites, warn, dispatch |

### 7.4 Agents

8 fixed agents + 14 dynamic supervisors.

**Fixed (8):**

| Persona | Role | Model | Hooks |
|---|---|---|---|
| Grace | Product Manager | opus | — |
| Ada | Architect + Coherence Reviewer | opus | — |
| Smith | Research / API validator | opus | — |
| Sherlock | Investigator | opus | — |
| Fernando | Issue Owner | sonnet | — |
| Linus | Code Reviewer (implementation, gaps vs spec) | opus | PreToolUse(Task), Stop |
| Quinn | QA Gate (tests, spec conformance, acceptance) | opus | PreToolUse(Task), Stop |
| Daphne | Discovery / supervisor installer | sonnet | — |

**Dynamic supervisors (14)** — installed by Daphne during `/setup` based on stack detection. All inherit shared skills `implementation`, `do`, `workflow`, `subagents-discipline` and hooks `PreToolUse(Task)`, `Stop`. Model: `sonnet`.

| Persona | Stack | Detection signal |
|---|---|---|
| Neo | Rust | `Cargo.toml` |
| Nina | Node backend | `package.json` + express/fastify/nest/koa |
| Luna | React | `package.json` + `react` |
| Violet | Vue | `package.json` + `vue` |
| Tessa | Python backend | `pyproject.toml` / `requirements.txt` + fastapi/django/flask |
| Greta | Go | `go.mod` |
| Juno | Java backend | `pom.xml` / `build.gradle` + Spring/Quarkus/Micronaut |
| Kali | Kotlin backend | `build.gradle.kts` + Ktor/Spring (no Android SDK) |
| Maya | Flutter | `pubspec.yaml` |
| Isla | iOS | `*.xcodeproj` / `Package.swift` |
| Ava | Android | `build.gradle` + Android SDK |
| Nova | Blockchain | `hardhat.config` / `Anchor.toml` |
| Iris | ML | `pyproject.toml` + pytorch/tensorflow/sklearn |
| Olive | Infra / CI-CD | `*.tf` / `.github/workflows/*.yml` / `k8s/` |

Daphne consults manifests **and** `docs/PRD.md` / `docs/MANIFESTO.md` / `docs/SPEC.md`. On detection failure, Daphne prompts the user for type / technology / infra.

### 7.5 Comment Templates

| Template | Written by | Consumed by |
|---|---|---|
| `DISPATCH` | Skill (before dispatch) | Supervisor |
| `INVESTIGATION` | Sherlock | Supervisor, `/do`, `reconcile`, `/trail` |
| `DECISION` | Any agent during implementation | Linus, Quinn, human, `/trail` |
| `DEVIATION` | Any agent during implementation | Linus, Quinn, human, `/trail` |
| `COMPLETED` | Supervisor | Linus, Quinn, `/trail` |
| `REVIEW` | Linus | Skill flow, Supervisor (rework), human, `/trail` |
| `QA` | Quinn | Skill flow, human, `/trail` |
| `OVERRIDE` | Quinn (after explicit user confirmation) | `reconcile`, audit, `/trail` |
| `PR` | Supervisor / Fernando | Developer, `/trail` |
| `DEFERRED` | Fernando or developer | `/trail` |
| `NEEDS-HUMAN` | Any agent on enforcement failure or escape valve | Developer, `/trail` |
| `FINDING` | Fernando (issue body of finding bead) | Developer, parent epic tracking |

The `CommentKind` enum mirrors this table. `reconcile` flags out-of-order or missing kinds.

### 7.6 State Model

State is the source of truth; the label is derived. Three Projects V2 single-select fields:

| Field | Values |
|---|---|
| `Unblock Impl State` | `pending`, `done` |
| `Unblock Review State` | `pending`, `approved`, `needs-rework` |
| `Unblock QA State` | `pending`, `passed`, `failed` |

**`derive_label(impl, review, qa)`:**

| impl | review | qa | Label |
|---|---|---|---|
| pending | — | — | (none) |
| done | pending | — | `unblock:review:pending` |
| done | needs-rework | — | `unblock:review:rework` |
| done | approved | pending | `unblock:review:ok` |
| done | approved | passed | `unblock:qa:ok` |
| done | approved | failed | `unblock:qa:rework` |

**Invariants:**

- `set_state` reconciles labels atomically: removes the five state-bound labels and applies `derive_label(...)`.
- F2.b: writing `review=needs-rework` forces `qa=pending` in the same transaction.
- `set_state(qa=failed)` requires the current `review=approved`; otherwise rejected.
- After `qa=failed`, on the next supervisor `claim` the server performs an atomic reset `review=pending` + `qa=pending`; the resulting label is `unblock:review:pending`.
- Exception labels (`unblock:needs-human`, `unblock:paused`, `unblock:no-investigation`) and finding labels (`unblock:finding:*`) are orthogonal — applied directly, not derived.
- Escape valve: 3 iterations of rework per gate (review OR qa) → automatic `unblock:needs-human`.
- On bead close, **all** labels are removed (state-bound, exception, and finding alike). Historical record lives in comments and PR references.

### 7.7 Labels

13 labels, all prefixed `unblock:`.

| Label | Meaning | Source |
|---|---|---|
| `unblock:review:pending` | Implementation complete, awaiting review | Derived |
| `unblock:review:ok` | Review approved | Derived |
| `unblock:review:rework` | Review requires rework | Derived |
| `unblock:qa:ok` | QA passed | Derived |
| `unblock:qa:rework` | QA failed | Derived |
| `unblock:needs-human` | Agent blocked — requires human decision | Direct |
| `unblock:paused` | Work intentionally paused | Direct (user) |
| `unblock:no-investigation` | Issue skips the investigation phase | Direct |
| `unblock:finding:suggestion` | Review SUGGESTION finding | Direct (Fernando) |
| `unblock:finding:minor` | QA MINOR finding | Direct (Fernando) |
| `unblock:finding:risk` | QA RISK finding | Direct (Fernando) |
| `unblock:finding:deviation` | QA DEVIATES finding | Direct (Fernando) |
| `unblock:finding:extra` | QA EXTRA finding | Direct (Fernando) |

### 7.8 Hooks

3 hooks. m-a's `stamp-pending.sh` is dropped (KV store dropped per PRD §7.4.7 D3).

| Hook script | Purpose | Claude Code | Copilot cloud |
|---|---|---|---|
| `session-start.sh` | Dashboard + invokes `prime` | `SessionStart` | `sessionStart` |
| `inject-discipline-reminder.sh` | Supervisor dispatch reminder | `PreToolUse` matcher=`Task` | `preToolUse` filter=sub-agent dispatch |
| `verify-state.sh` | Enforce state dimension via `verify_agent_state` | `Stop` | `agentStop` |

Copilot local: zero hooks (not supported by VS Code Copilot agent mode).

### 7.9 Review and QA Flow

**Review verdict mapping (Linus):**

| Verdict | Condition | Label outcome |
|---|---|---|
| APPROVE | No CRITICAL or WARNING (SUGGESTION allowed) | `unblock:review:ok` |
| NEEDS-REWORK | Any CRITICAL or WARNING | `unblock:review:rework` (auto-dispatch supervisor) |

**Review finding action:** CRITICAL and WARNING → rework, never finding issues. SUGGESTION → Fernando creates finding bead as sub-issue of the originating bead's parent epic, applies `unblock:finding:suggestion`, posts a comment on the reviewed bead referencing each created finding (e.g., "SUGGESTION tracked as #123").

**QA verdict mapping (Quinn):**

| Verdict | Condition | Label outcome |
|---|---|---|
| PASS | No BLOCKER or MAJOR | `unblock:qa:ok` |
| PASS+FINDINGS | No BLOCKER or MAJOR, but MINOR/RISK/DEVIATES/EXTRA | `unblock:qa:ok` + finding beads |
| FAIL | Any BLOCKER or MAJOR | `unblock:qa:rework` (sub-options below) |

**QA finding action:** BLOCKER and MAJOR → rework, never finding issues. MINOR, RISK, DEVIATES, EXTRA → Fernando creates finding beads as sub-issues of the originating bead's parent epic.

**QA FAIL sub-options:**

| Sub-option | Trigger | Effect |
|---|---|---|
| rework | Default | Returns to supervisor — full cycle re-implementation + re-review + re-QA |
| follow-up | User selection | Fernando creates finding beads under parent epic; original bead proceeds to close (degraded) |
| override | User explicitly says "do override" via prompt | Quinn requests confirmation + reason (≥ 20 chars), writes `OVERRIDE:` comment, calls `set_state(qa=passed, override=true)`, Fernando creates `unblock:finding:risk` bead to track |

After supervisor rework on a NEEDS-REWORK review, Linus always re-reviews. Never auto-approve.

### 7.10 New MCP Tools

| Tool | Purpose |
|---|---|
| `set_state(qualified_id, dim, value)` | Write state dim and reconcile label atomically per §7.6 invariants |
| `get_state(qualified_id, dim)` | Read a single state dim |
| `verify_agent_state(agent_id)` | Stop hook helper. Lists open issues claimed by `agent_id` and checks the relevant state dim is not `pending`. Exit 0 = OK; exit 2 = enforcement failure (orchestrator decides re-dispatch or `unblock:needs-human`) |

### 7.11 Plugin File Structure

#### 7.11.1 SKILL.md frontmatter (YAML)

Mandatory fields:

```yaml
---
name: <kebab-case>          # must match directory name
description: <string>       # description contract (§7.3)
user_invocable: <bool>      # true for slash command; false for shared-only knowledge pack
---
```

Optional, target-specific (renderer controls):

- `license: <SPDX>` — Copilot cloud only
- `allowed-tools: [<list>]` — Copilot cloud only
- `tools: [<list>]` — Claude Code only

#### 7.11.2 SKILL.md handler vocabulary (pseudo-XML)

The body of every SKILL.md is markdown structured by handler tags. The renderer emits handlers in canonical order.

| Tag | Cardinality | Purpose |
|---|---|---|
| `<on-init>` | 0–1 | Parse and validate `$ARGUMENTS` |
| `<on-state>` | 0–1 | Display current state (scan files, status fields) |
| `<on-check>` | 0–1 | Numbered list of prerequisites |
| `<on-check-fail if="<cond>">` | 0–N | Recovery / guidance for a failed prerequisite |
| `<on-step>` / `<on-step-skip>` | 0–N | Step orchestration with skip logic |
| `<on-execute>` | 1 | Main action (dispatches, writes) — required for every user-invocable skill |
| `<on-complete if="<cond>">` | 0–N | Conditional completion handler |
| `<on-next>` | 0–1 | Suggest the next step |

Build-time validation: every `user_invocable: true` skill must contain `<on-execute>`. Atomic skills typically use `<on-init>` + `<on-check>` + `<on-execute>`. Orchestrators add `<on-state>` + N × `<on-step>`.

#### 7.11.3 Agent file frontmatter

**Claude Code (`.claude/agents/<n>.md`):**

```yaml
---
name: <kebab-case>
description: <string>
model: opus | sonnet | haiku
tools: [<list>]            # MCP tools + sub-agent dispatch
---
```

**Copilot cloud (`.github/agents/<n>.md`):**

```yaml
---
name: <kebab-case>
description: <string>
prompt: <inline or file ref>
tools: <list>
---
```

The renderer projects the same `Persona` struct onto each frontmatter schema.

#### 7.11.4 Agent handler vocabulary

| Tag | Cardinality | Purpose |
|---|---|---|
| `<on-task-start>` | 0–1 | Actions at the start of every received task |
| `<on-review-findings>` | 0–1 | Structured findings processing (Linus, Quinn) |
| `<naming-rule>` | 0–N | Conventions (bead IDs, branch names, comment kinds) |

The agent body contains the persona prompt, handlers, and references to the shared knowledge packs loaded as context.

### 7.12 Token Substitution

Catalog templates use handlebars-style tokens. The renderer resolves them per target.

**Commands (semantic port from m-a):**

| Token | m-a equivalent | unblock equivalent |
|---|---|---|
| `{{cmd:show <id>}}` | `bd show {id}` | MCP `show(qualified_id)` |
| `{{cmd:ready}}` | `bd ready` | MCP `ready()` |
| `{{cmd:claim <id> <agent>}}` | `bd claim {id}` | MCP `claim(qualified_id, agent)` |
| `{{cmd:create ...}}` | `bd create ...` | MCP `create(...)` |
| `{{cmd:close <id>}}` | `bd close {id}` | MCP `close(qualified_id)` |
| `{{cmd:comment <id> <kind>}}` | `bd comment {id}` | MCP `comment(qualified_id, kind, body)` |
| `{{cmd:set-state <id> <dim> <val>}}` | `bd set-state {id} {dim}={val}` | MCP `set_state(qualified_id, dim, value)` |
| `{{cmd:get-state <id> <dim>}}` | `bd state {id} {dim}` | MCP `get_state(qualified_id, dim)` |
| `{{cmd:dep-add <a> <b>}}` | `bd dep add {a} {b}` | MCP `depends(a, b)` |
| `{{cmd:list ...}}` | `bd list ...` | MCP `list(...)` |
| `{{cmd:reconcile}}` | (n/a) | MCP `reconcile()` |
| `{{cmd:verify-agent-state <id>}}` | `verify-state.sh` reads kv | MCP `verify_agent_state(agent_id)` |
| `{{cmd:kv:*}}` | `bd kv *` | **Dropped — token raises a compile-time error** |

**Labels:**

| Token | Resolves to |
|---|---|
| `{{label:review_pending}}` | `unblock:review:pending` |
| `{{label:review_ok}}` | `unblock:review:ok` |
| `{{label:review_rework}}` | `unblock:review:rework` |
| `{{label:qa_ok}}` | `unblock:qa:ok` |
| `{{label:qa_rework}}` | `unblock:qa:rework` |
| `{{label:needs_human}}` | `unblock:needs-human` |
| `{{label:paused}}` | `unblock:paused` |
| `{{label:no_investigation}}` | `unblock:no-investigation` |
| `{{label:finding:<kind>}}` | `unblock:finding:<kind>` |

**Dispatch (target-specific):**

| Token | Claude Code render | Copilot cloud render |
|---|---|---|
| `{{dispatch <persona> <prompt>}}` | `Task(subagent_type="<persona>", prompt="<prompt>")` | `@<persona>: <prompt>` |

**Paths (universal):**

| Token | Resolves to |
|---|---|
| `{{path:manifesto}}` | `docs/MANIFESTO.md` |
| `{{path:prd}}` | `docs/PRD.md` |
| `{{path:spec_global}}` | `docs/SPEC.md` |
| `{{path:plan NN}}` | `docs/plans/NN-plan-*.md` |
| `{{path:spec NN}}` | `docs/specs/NN-spec-*.md` |
| `{{path:worktree N slug}}` | `worktrees/issue-{N}-{slug}` |

### 7.13 Render Contract

For each `Skill | Persona | Hook` in the catalog, the renderer:

1. Resolves the target (Claude Code / Copilot cloud / Copilot local).
2. Selects the frontmatter schema (§7.11.1 or §7.11.3).
3. Substitutes tokens (§7.12) using target-aware resolution.
4. Emits the file at the path defined by §7.2.

**Idempotence.** Rendering the same catalog + target produces byte-identical output (deterministic ordering, no timestamps, no random IDs).

**Validation.** `unblock-plugin verify --out=<dir>` reads the rendered output and compares it against the expected render. CI uses this to detect drift between catalog source and emitted files.

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
#[non_exhaustive]
pub enum Error {
    Domain { source: DomainError },
    GitHubApi { message: String },
    GitHubGraphQL { errors: Vec<String> },
    GitHubUnavailable { source: reqwest::Error },
    GitHubServerError { status: u16, message: String },
    RateLimited { reset_at: Option<DateTime<Utc>> },
    CircuitBreakerOpen { since: Instant },
    ProjectNotConfigured,
    GitRemote { message: String },
    ViewCreationFailed { message: String },
    OwnerDetectionFailed { owner: String, message: String },
}
```

`#[non_exhaustive]` is workspace policy for public enums that are expected to grow over time — adding a variant must remain a non-breaking change for downstream consumers. The same policy applies to `DomainError` (already non-exhaustive) and to `unblock_core::reconcile::DriftKind` (becomes non-exhaustive in Phase 02 alongside the `StaleStatus` addition). See [CLAUDE.md "Coding Standards"](../CLAUDE.md#coding-standards) for the full project-wide rule.

**Error classification by HTTP status:**

| Status | Error variant | Retryable | Circuit breaker |
|---|---|---|---|
| Network error | `GitHubUnavailable` | ✅ | `record_failure()` |
| 429 | `RateLimited` | ✅ | `record_failure()` |
| 500 | `GitHubServerError` | ❌ | `record_failure()` |
| 502 | `GitHubServerError` | ❌ | `record_failure()` |
| 503 | `GitHubServerError` | ✅ | `record_failure()` |
| 4xx (except 429) | `GitHubApi` | ❌ | neither |

`is_retryable()` matches `RateLimited`, `GitHubUnavailable`, and `GitHubServerError` where `status == 503`.

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

### 13.3 Metrics

**Phase 02 — in-memory `ServerMetrics`.** Single struct with atomic counters + `hdrhistogram` latency histograms. Exposed via the `doctor` MCP tool's `metrics_snapshot` field. No external dependencies beyond `hdrhistogram`. Provisional location: `unblock-mcp::metrics` (or `unblock-core::metrics` if the struct stays free of MCP types — finalised during Phase 02 spec authoring).

| Metric | Type | Labels |
|---|---|---|
| `tool_calls` | Counter | `tool` |
| `tool_durations` | Histogram | `tool` |
| `api_calls` | Counter | `api` (graphql/rest) |
| `api_durations` | Histogram | `api` |
| `cache_hits` / `cache_misses` / `cache_evictions` / `cache_size` | Counter / Gauge | — |
| `graph_issues` / `graph_edges` | Gauge | — |

**Phase 06 — OpenTelemetry adapter.** Wraps the same `ServerMetrics` struct — no schema change, no breaking redesign. Exports to OTLP HTTP. Adapter publishes the OTel metric names previously listed in earlier drafts of this spec (`unblock.tool.duration`, `unblock.github.request.duration`, `unblock.cache.*`, `unblock.graph.*`). Phase 02 ships a forward-compat contract test that locks the snapshot serialisation shape so Phase 06 cannot regress it.

---

## 14. Resilience

The resilience layer is a stand-alone crate `unblock-resilience` (Phase 02+). It has zero dependencies on other unblock crates and is consumed by both `unblock-github` (issue domain HTTP) and `unblock-indexer` (Phase 03+, code-domain grammar fetcher). See [02-plan-mcp-complete §6](./plans/02-plan-mcp-complete.md#6-public-api-surface-for-phase-03) for the public API contract.

### 14.1 Circuit Breaker

Built on the `failsafe` crate. Per-process singleton scope (Phase 06 multi-tenant scoping is a separate decision). Default config:

| Knob | Value |
|---|---|
| Failure threshold | 5 consecutive |
| Cooldown | 10s |
| State machine | Closed → Open → HalfOpen → Closed |

After 5 consecutive failures, the circuit opens — subsequent requests fail immediately with `CircuitBreakerOpen` for 10 seconds. After cooldown, transitions to HalfOpen — one probe request allowed. Success → Closed. Failure → Open again.

Composition: **breaker outside, retry inside**. The breaker counts only the **final** outcome of `ResiliencePolicy::execute` after retries exhaust — a successful retry records as a success.

### 14.2 Retry Policy

Built on the `backoff` crate. Exponential backoff with ±25% jitter. Default config:

| Knob | Default | Env var |
|---|---|---|
| Max attempts | 5 | `UNBLOCK_RETRY_MAX_ATTEMPTS` |
| Total deadline | 30s | `UNBLOCK_RETRY_DEADLINE_SECS` |
| Base delay | 500ms | (hard-coded) |
| Max delay | 5s | (hard-coded) |
| Retry-After cap | 30s | (hard-coded — exceeded value triggers fail-fast) |

Hybrid limit: whichever of max-attempts / deadline hits first wins.

Only retries on errors whose `IsRetryable::is_retryable()` returns `true`. For `unblock-github::Error` this is `RateLimited` (429), `GitHubUnavailable` (network), and `GitHubServerError { status: 503 }`. All other errors propagate immediately.

### 14.3 Reuse by other crates

`ResiliencePolicy::execute` is generic over any error type that implements the `IsRetryable` trait exposed by `unblock-resilience`. Phase 03's grammar fetcher (`unblock-indexer`) reuses it for the GitHub Releases asset HTTP calls. `unblock-indexer` depends directly on `unblock-resilience` — it does **not** transit through `unblock-github` (the two crates are architecturally orthogonal: code domain vs issue domain).

See [02-plan-mcp-complete §6.2](./plans/02-plan-mcp-complete.md#62-reuse-mechanism--locked-extracted-unblock-resilience-crate) for the rationale.

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
| Priority | Must be P0–P4 (display: `P0 - Critical` through `P4 - Backlog`) |
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
