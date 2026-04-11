# Plan 01 — MCP Foundation (v0.1.0)

> Phase: 01  
> Version: v0.1.0  
> Crates: `unblock-core`, `unblock-github`, `unblock-mcp`  
> Depends on: nothing  
> Required by: Phase 02 (MCP Complete)  
> Status: in progress (11 of 17 tools implemented, 6 remaining)  
> Companion specs: [01-spec-graph-engine.md](../specs/01-spec-graph-engine.md) · [02-spec-github-client.md](../specs/02-spec-github-client.md) · [03-spec-mcp-tools.md](../specs/03-spec-mcp-tools.md)

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [Rust Idioms & Rules](#2-rust-idioms--rules)
3. [Public API Surface](#3-public-api-surface)
4. [Priority & Dependency Legend](#4-priority--dependency-legend)
5. [Epics](#5-epics)
   - [Epic 01 — Workspace and Infrastructure](#epic-01--workspace-and-infrastructure)
   - [Epic 02 — Core Library (unblock-core)](#epic-02--core-library-unblock-core)
   - [Epic 03 — GitHub API Layer (unblock-github)](#epic-03--github-api-layer-unblock-github)
   - [Epic 04 — MCP Server + Core Tools](#epic-04--mcp-server--core-tools)
   - [Epic 05 — GitHubApi Trait Abstraction](#epic-05--githubapi-trait-abstraction)
   - [Epic 06 — Foundation Completion](#epic-06--foundation-completion)
6. [Definition of Done](#6-definition-of-done)

---

## 1. Purpose

Phase 01 delivers the minimum viable agent workflow loop. An agent connects via MCP (stdio), finds unblocked work, claims it, implements, closes, and sees the cascade promote newly unblocked issues to ready. The graph engine, cache, GitHub client, and 17 MCP tools form a complete local product.

**Scope — 17 MCP tools:**

| # | Tool | Type | Purpose |
|---|---|---|---|
| 1 | `init` | Write | Create Projects V2 board for the repo |
| 2 | `setup` | Write | Create fields, views, migrate issues (idempotent) |
| 3 | `ready` | Read | Find issues with no active blockers |
| 4 | `claim` | Write | Atomic assignment of issue to agent |
| 5 | `create` | Write | Create issue with optional deps, parent, fields |
| 6 | `update` | Write | Update issue fields, labels, body sections |
| 7 | `close` | Write | Close issue + cascade-unblock dependents |
| 8 | `reopen` | Write | Reopen closed issue, evaluate blocking status |
| 9 | `show` | Read | Full issue detail with comments and deps (always fresh) |
| 10 | `list` | Read | Filtered, sorted, paginated issue list |
| 11 | `search` | Read | Full-text search via GitHub Search API |
| 12 | `stats` | Read | Aggregate counts by status, priority, agents |
| 13 | `prime` | Read | Context summary for agent session injection |
| 14 | `comment` | Write | Add comment to issue (no cache invalidation) |
| 15 | `depends` | Write | Add blocking edge with cycle detection |
| 16 | `dep_remove` | Write | Remove blocking edge |
| 17 | `dep_cycles` | Read | Detect dependency cycles |

**Supporting infrastructure:**

- Cargo workspace (3 crates): `unblock-core`, `unblock-github`, `unblock-mcp`
- Graph engine: petgraph `DiGraph` with `QualifiedId` nodes
- TTL cache: `GraphCache` with async `RwLock`, invalidation after every write
- GitHub client: `GitHubClient` with paginated GraphQL reads, REST/GraphQL mutations
- `GitHubApi` trait abstraction for testing with `MockGitHubClient`
- Projects V2 field management: 7 custom fields, 5 pre-configured views
- CI pipeline: GitHub Actions (fmt, clippy, test, doc)
- Error model: `snafu` exclusive, three-layer hierarchy (domain → infrastructure → MCP)

**Outcome:** `v0.1.0` — agent can `prime` → `ready` → `claim` → work → `close` → cascade.

---

## 2. Rust Idioms & Rules

### 2.1 Edition 2024, no unsafe

Workspace-wide `edition = "2024"` and `#![deny(unsafe_code)]`. No exceptions.

### 2.2 `snafu` exclusive

Every crate uses `snafu` for errors. No `thiserror`, no `anyhow`, no `Box<dyn Error>`. Every crate defines its error types in `src/errors.rs` and re-exports a crate-scoped `Result<T>` alias. `unwrap()` and `expect()` forbidden outside test modules.

### 2.3 Pure core, impure shell

`unblock-core` has zero network I/O. All computation is testable with in-memory data. `unblock-github` handles all GitHub API communication. `unblock-mcp` is the thin MCP shell that wires tools to state.

### 2.4 Trait abstraction for testing

`GitHubApi` trait in `unblock-github/src/api.rs` abstracts all GitHub operations. `ServerState` holds `Arc<dyn GitHubApi>`. Tests use `MockGitHubClient` with call counters and response stubs, feature-gated behind `test-hooks`.

### 2.5 Environment-based config

`Config::load_from(env_reader)` accepts a closure for testability. No `std::env::set_var` in tests (unsafe in edition 2024). Tests supply `HashMap`-backed closures.

### 2.6 Write-through cache

Every write tool: execute mutation → `cache.invalidate()` → `fetch_graph_data()` → build graph → compute ready set → `cache.update()`. Lock never held across network I/O.

---

## 3. Public API Surface

### 3.1 `unblock-core`

```
src/
  lib.rs           ← pub mod types, graph, cache, config, errors, reconcile, client, detection
  types.rs         ← QualifiedId, Issue, IssueState, Status, Priority, PipelineStage,
                      IssueType, BlockingEdge, IssueSummary, IssueRef, BodySections,
                      TraversalDirection, IssueComment, RelatedIssue
  graph.rs         ← DependencyGraph: build(), compute_ready_set(), compute_unblock_cascade(),
                      would_create_cycle(), detect_all_cycles(), dependency_tree(),
                      all_edges(), edge_count()
  cache.rs         ← GraphCache: new(), get_ready_set(), get_graph(), update(), invalidate(), is_fresh()
  config.rs        ← Config: load(), load_from()
  errors.rs        ← DomainError (11 variants)
  reconcile.rs     ← DriftKind (7 variants), DriftReport, ReconcileEngine (Phase 02 early)
  client.rs        ← AgentKind, AgentClient (Phase 02 early)
  detection.rs     ← ClientDetector (Phase 02 early)
```

### 3.2 `unblock-github`

```
src/
  lib.rs           ← pub mod client, graphql, mutations, projects, errors, api, mock
  client.rs        ← GitHubClient: new(), owner(), repo(), rest_url(), graphql_url(), field_ids()
  api.rs           ← GitHubApi trait (33 methods), blanket impl on GitHubClient
  graphql.rs       ← fetch_graph_data(), fetch_issue(), fetch_issue_ref()
  mutations.rs     ← create_issue(), close_issue(), add_comment(), add_blocked_by(),
                      remove_blocked_by(), add_sub_issue(), update_issue_body(), label ops
  projects.rs      ← resolve_project_info(), setup_fields(), update_field(),
                      ProjectFieldIds, SetupReport, view management
  errors.rs        ← Error (9 variants + CircuitBreakerOpen stub)
  mock.rs          ← MockGitHubClient, CallCounts, Stubs (test-hooks feature)
```

### 3.3 `unblock-mcp`

```
src/
  lib.rs           ← pub mod server, errors, tools
  main.rs          ← binary entrypoint, stdio transport
  server.rs        ← ServerState, UnblockServer, tool registration (12 tools currently)
  errors.rs        ← github_error_to_mcp() conversion
  tools/
    mod.rs         ← execute_read_tool(), execute_write_tool(), rebuild_cache(), normalize_filter()
    init.rs        ← InitParams, InitResult
    setup.rs       ← SetupParams, SetupResult
    ready.rs       ← ReadyParams, ReadyResult, ReadyIssueSummary
    claim.rs       ← ClaimParams, ClaimResult, ClaimCandidate, validate_claimable()
    close.rs       ← CloseParams, CloseResult
    create.rs      ← CreateParams, CreateResult
    show.rs        ← ShowParams, ShowResult, ShowIssue, ShowBodySections
    comment.rs     ← CommentParams, CommentResult
    update.rs      ← UpdateParams, UpdateResult, BodySectionUpdate, SectionName
    depends.rs     ← DependsParams, DependsResult
    prime.rs       ← PrimeParams, PrimeResult, SessionMeta, ProjectMeta
    reconcile.rs   ← ReconcileParams, ReconcileOutput (Phase 02 early)
```

---

## 4. Priority & Dependency Legend

### Priority levels

| Level | Meaning |
|---|---|
| **P0 - Critical** | Absolute blocker — nothing moves forward until this is done |
| **P1 - High** | Critical for the phase to be functional — happy path |
| **P2 - Medium** | Important but does not block the happy path |
| **P3 - Low** | Quality, ergonomics, extra coverage |
| **P4 - Backlog** | Nice to have — included if time permits, does not delay done |

### Dependency fields

Every task carries three metadata fields:

- **Priority** — P0 through P4 as defined above
- **Depends on** — task IDs within this plan that must be complete before this task starts
- **Blocked by** — external blockers (other phases, tools, decisions outside this plan)

---

## 5. Epics

---

### Epic 01 — Workspace and Infrastructure

**Goal:** Cargo workspace scaffold, CI pipeline, crate skeletons, developer tooling.

**Status:** ✅ Complete

---

#### Task 01.01 — Cargo workspace scaffold

> **Priority:** P0  
> **Depends on:** nothing  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**Files:** `Cargo.toml` (workspace root), `crates/*/Cargo.toml`

Requirements:
- Workspace with 3 members: `unblock-core`, `unblock-github`, `unblock-mcp`
- Edition 2024, `#![deny(unsafe_code)]` workspace-wide
- Shared dependencies via `[workspace.dependencies]`
- Core dependencies: `petgraph`, `rmcp`, `reqwest`, `snafu`, `tracing`, `tokio`, `serde`, `schemars`, `chrono`

---

#### Task 01.02 — CI pipeline — GitHub Actions

> **Priority:** P0  
> **Depends on:** Task 01.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `.github/workflows/ci.yml`

Requirements:
- Quality gate: `cargo fmt --check --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --no-deps --workspace`
- Runs on: push to main, pull requests

---

#### Task 01.03 — Crate skeletons

> **Priority:** P0  
> **Depends on:** Task 01.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**Files:** `crates/unblock-core/src/lib.rs`, `crates/unblock-github/src/lib.rs`, `crates/unblock-mcp/src/lib.rs`

Requirements:
- Each crate compiles with empty module declarations
- Dependency graph: `unblock-mcp` → `unblock-github` → `unblock-core`

---

#### Task 01.04 — Developer tooling

> **Priority:** P1  
> **Depends on:** Task 01.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**Files:** `CLAUDE.md`, `.claude/agents/`

Requirements:
- `CLAUDE.md` with project overview, coding standards, workspace commands
- Agent definitions for `rust-supervisor` and `infra-supervisor`

---

### Epic 02 — Core Library (unblock-core)

**Goal:** Pure Rust domain types, graph engine, cache layer, configuration. Zero network I/O.

**Status:** ✅ Complete

---

#### Task 02.01 — Domain types

> **Priority:** P0  
> **Depends on:** Task 01.03  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-core/src/types.rs`

Types implemented:
- `QualifiedId` — `{owner, repo, number}`, implements `Display`, `FromStr`, `Hash`, `Eq`
- `Issue` — full issue data with all Projects V2 fields (status, priority, pipeline_stage, agent, claimed_at, story_points, defer_until, body sections, relationships)
- `IssueState` — `{Open, Closed}` (GitHub native)
- `Status` — `{Ready, InProgress, Blocked, Deferred, Closed}` (Projects V2 unified workflow + readiness)
- `Priority` — `{P0, P1, P2, P3, P4}` with `as_sort_key()`
- `PipelineStage` — `{Investigation, Implementation, Review, Refactoring, Qa, Done}` (Projects V2 development pipeline)
- `IssueType` — `{Task, Bug, Feature, Epic, Chore, Spike}`
- `BlockingEdge` — `{source: QualifiedId, target: QualifiedId}`
- `IssueSummary` — lightweight issue for list/ready responses
- `IssueRef` — `{Local(u64), CrossRepo{owner, repo, number}}` with `resolve()`
- `BodySections` — `{description, design_notes, acceptance_criteria}` with `from_markdown()`, `to_markdown()`
- `TraversalDirection` — `{Upstream, Downstream, Both}`
- `IssueComment`, `RelatedIssue`

---

#### Task 02.02 — Domain errors

> **Priority:** P0  
> **Depends on:** Task 02.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-core/src/errors.rs`

11 variants: `IssueNotFound`, `AlreadyClaimed`, `IssueBlocked`, `IssueDeferred`, `IssueClosed`, `IssueNotClosed`, `IssueAlreadyOpen`, `CircularDependency`, `DuplicateDependency`, `FieldNotFound`, `Validation`. Each with `status_code() -> u16` for HTTP mapping.

---

#### Task 02.03 — Configuration

> **Priority:** P0  
> **Depends on:** Task 02.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-core/src/config.rs`

`Config` struct with fields: `token`, `api_base_url`, `github_url`, `repo`, `project_number`, `agent`, `cache_ttl`, `log_level`, `otel_endpoint`. Loaded from environment variables via `load_from(env_reader)`.

---

#### Task 02.04 — Graph engine — build and ready set

> **Priority:** P0  
> **Depends on:** Task 02.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-core/src/graph.rs`

`DependencyGraph` with petgraph `DiGraph<QualifiedId, ()>`. Edge direction: blocked → blocker.

Methods:
- `build(issues, edges) -> Self` — construct from issue/edge arrays
- `compute_ready_set(issues) -> Vec<IssueSummary>` — issues with no open blockers, sorted by priority ASC → created_at ASC

---

#### Task 02.05 — Graph engine — cascade

> **Priority:** P0  
> **Depends on:** Task 02.04  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-core/src/graph.rs`

- `compute_unblock_cascade(closed_id, all_issues) -> Vec<QualifiedId>` — dependents whose blockers are all now closed

---

#### Task 02.06 — Graph engine — cycles and tree traversal

> **Priority:** P0  
> **Depends on:** Task 02.04  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-core/src/graph.rs`

- `would_create_cycle(source, target) -> bool` — pre-mutation check via `has_path_connecting`
- `detect_all_cycles() -> Vec<Vec<QualifiedId>>` — Tarjan's SCC algorithm
- `dependency_tree(root, direction, max_depth) -> Vec<(QualifiedId, usize)>` — BFS traversal
- `all_edges() -> Vec<BlockingEdge>`, `edge_count() -> usize`

---

#### Task 02.07 — Cache layer

> **Priority:** P0  
> **Depends on:** Task 02.04  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-core/src/cache.rs`

`GraphCache` with `RwLock<Option<CacheEntry>>` and `Duration` TTL. Methods: `new()`, `get_ready_set()`, `get_graph()`, `update()`, `invalidate()`, `is_fresh()`. Returns `Arc` handles for cheap sharing.

---

### Epic 03 — GitHub API Layer (unblock-github)

**Goal:** GitHub client with paginated GraphQL reads, REST/GraphQL mutations, Projects V2 field management.

**Status:** ✅ Complete

---

#### Task 03.01 — GitHub client bootstrap

> **Priority:** P0  
> **Depends on:** Task 01.03  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/client.rs`

`GitHubClient` with `reqwest::Client`, token in default headers, repo resolution from `UNBLOCK_REPO` or git remote parsing. GHE URL resolution: `graphql_url()` strips `/v3` suffix for GHE Server.

---

#### Task 03.02 — fetch_graph_data — paginated GraphQL

> **Priority:** P0  
> **Depends on:** Task 03.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/graphql.rs`

Single paginated query returns all open issues with blocking edges, sub-issues, Projects V2 field values. 100 issues per page. Cross-repo blocking edges supported.

---

#### Task 03.03 — fetch_issue — single issue query

> **Priority:** P0  
> **Depends on:** Task 03.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/graphql.rs`

`fetch_issue(number)` and `fetch_issue_ref(IssueRef)` — always fresh, never cached. Includes comments and full field values.

---

#### Task 03.04 — Mutations — create, close, comment

> **Priority:** P0  
> **Depends on:** Task 03.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/mutations.rs`

REST mutations: `create_issue()`, `close_issue()`, `add_comment()`, `update_issue_body()`, label operations, assignee operations, milestone operations.

---

#### Task 03.05 — Mutations — blocking relationships and sub-issues

> **Priority:** P0  
> **Depends on:** Task 03.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/mutations.rs`

GraphQL mutations: `add_blocked_by()`, `remove_blocked_by()`, `add_sub_issue()`, `add_blocked_by_ref()`. Cross-repo dependencies supported.

---

#### Task 03.06 — Projects V2 fields — setup and update

> **Priority:** P0  
> **Depends on:** Task 03.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/projects.rs`

`resolve_project_info()`, `setup_fields()`, `update_field()`, `query_setup_status()`. 7 custom fields: Status, Priority, Pipeline Stage, Agent, Claimed At, Story Points, Defer Until. 5 pre-configured views. REST view management with integer field ID discovery.

---

#### Task 03.07 — Infrastructure errors

> **Priority:** P0  
> **Depends on:** Task 03.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/errors.rs`

`Error` enum: `Domain`, `GitHubApi`, `GitHubGraphQL`, `GitHubUnavailable`, `RateLimited`, `CircuitBreakerOpen` (stub), `ProjectNotConfigured`, `GitRemote`, `UnknownOwnerType`, `MockNotStubbed`. With `status_code()` mapping.

---

### Epic 04 — MCP Server + Core Tools

**Goal:** MCP server over stdio with tool registration, shared execution pattern, and 11 core tools.

**Status:** ✅ Complete (11 of 17 tools — remaining 6 in Epic 06)

---

#### Task 04.01 — MCP server bootstrap

> **Priority:** P0  
> **Depends on:** Task 03.06  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**Files:** `unblock-mcp/src/main.rs`, `unblock-mcp/src/server.rs`

`ServerState` with `Arc<Config>`, `Arc<dyn GitHubApi>`, `Arc<GraphCache>`, `OnceLock<AgentKind>`, `OnceLock<AgentClient>`. `UnblockServer` implements `rmcp::ServerHandler`. Stdio transport via `rmcp::ServiceExt::serve`.

---

#### Task 04.02 — Tool execution pattern

> **Priority:** P0  
> **Depends on:** Task 04.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/mod.rs`

Shared helpers:
- `execute_read_tool(state, op)` — maps errors to `ErrorData`, no cache invalidation
- `execute_write_tool(state, op)` — maps errors, then `rebuild_cache(state)`
- `rebuild_cache(state)` — invalidate → fetch_graph_data → build graph → compute ready set → update cache

---

#### Task 04.03 — `init` tool

> **Priority:** P0  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/init.rs`

Creates Projects V2 board. Detects owner type (org vs user). Idempotent — returns existing project if found.

---

#### Task 04.04 — `setup` tool

> **Priority:** P0  
> **Depends on:** Task 04.03  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/setup.rs`

Creates 7 fields and 5 views. Dry-run mode. Idempotent. REST field discovery for integer IDs.

---

#### Task 04.05 — `ready` tool

> **Priority:** P0  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/ready.rs`

Filters: `issue_type`, `priority`, `milestone`, `agent`, `label`, `include_claimed`. Post-filter: `defer_until > today` excluded. Sort: priority ASC → created_at ASC. Default limit: 10.

---

#### Task 04.06 — `claim` tool

> **Priority:** P0  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/claim.rs`

Validates: open, no open blockers, not deferred, not already claimed. Updates: Status → InProgress, Agent, Claimed At. Posts claim comment.

---

#### Task 04.07 — `close` tool

> **Priority:** P0  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/close.rs`

Closes issue via REST. Updates Projects V2 fields. Computes cascade: for each newly unblocked dependent, updates Status → Ready and posts unblock comment. Returns list of unblocked issue numbers.

---

#### Task 04.08 — `create` tool

> **Priority:** P0  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/create.rs`

Creates issue with optional: `blocked_by` (cross-repo), `parent` (sub-issue), `labels` (auto-created), `milestone`, `story_points`, `defer_until`. Sets Projects V2 fields after creation.

---

#### Task 04.09 — `show` tool

> **Priority:** P0  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/show.rs`

Always fresh (never cached). Returns full issue detail with parsed body sections, blocking/blocked_by relationships, parent/sub-issues, dependency tree (BFS, max depth 3), and comments.

---

#### Task 04.10 — `comment` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/comment.rs`

Adds comment to issue. No cache invalidation — comments don't affect the graph.

---

#### Task 04.11 — `update` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/update.rs`

Selective updates: `priority`, `status`, `labels_add/remove`, `assignees_add/remove`, `body_section` (read-modify-write via `BodySections`), `milestone`, `story_points`, `defer_until`. Returns list of updated fields.

---

#### Task 04.12 — `depends` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/depends.rs`

Adds blocking relationship: `source` (u64) is blocked by `target` (IssueRef, cross-repo). Cycle detection via `would_create_cycle()`. Updates source: Status → Blocked. Rebuilds cache.

**Known deviations from spec** (tracked in beads):
- `source` param is `u64` instead of `IssueRef` (unblock-b6b.86)
- Output shape differs from spec `DependsResult` (unblock-b6b.87)

---

#### Task 04.13 — `prime` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/tools/prime.rs`

Context summary for agent session injection. Returns structured `PrimeResult` with `SessionMeta`, `ProjectMeta`, ready/blocked/in-progress counts, cycle warnings.

---

#### Task 04.14 — E2E workflow integration test

> **Priority:** P1  
> **Depends on:** Tasks 04.03–04.13  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/tests/e2e_workflow.rs`

Full agent loop: prime → ready → claim → comment → close → verify cascade.

---

### Epic 05 — GitHubApi Trait Abstraction

**Goal:** Extract `GitHubApi` trait for dependency injection. Enable unit testing of tool handlers without live GitHub.

**Status:** ✅ Complete

---

#### Task 05.01 — Define GitHubApi trait

> **Priority:** P0  
> **Depends on:** Epic 03  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/api.rs`

`GitHubApi` trait with 33 methods (sync accessors + async operations). Blanket impl on `GitHubClient`. `async_trait` for object safety.

---

#### Task 05.02 — Implement MockGitHubClient

> **Priority:** P0  
> **Depends on:** Task 05.01  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-github/src/mock.rs` (feature-gated: `test-hooks`)

`MockGitHubClient` with `CallCounts` (33 atomic counters) and `Stubs` (33 response queues). Full `GitHubApi` trait implementation. Macro-generated `push_*` helpers.

---

#### Task 05.03 — Migrate ServerState and tools to trait

> **Priority:** P0  
> **Depends on:** Task 05.02  
> **Blocked by:** nothing  
> **Status:** ✅ Done

**File:** `unblock-mcp/src/server.rs`

`ServerState.github` changed from `Arc<GitHubClient>` to `Arc<dyn GitHubApi>`. All tool handlers operate on the trait, enabling mock-based testing.

---

### Epic 06 — Foundation Completion

**Goal:** Implement the 6 remaining tools required for Phase 01 scope (17 tools total). These tools complete the read and dependency management capabilities that Phase 02 assumes exist.

**Status:** Not started

---

#### Task 06.01 — `list` tool

> **Priority:** P0  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/tools/list.rs`

The `list` tool provides filtered, sorted, paginated access to all issues. Unlike `ready` (which only returns unblocked issues), `list` returns any issue matching the filter criteria regardless of blocking status.

→ Spec: [03-spec-mcp-tools.md §4.5](../specs/03-spec-mcp-tools.md#45-list)

Requirements:
- `ListParams`: `status`, `priority`, `type`, `milestone`, `agent`, `label`, `assignee`, `sort` (priority|created|updated), `limit` (default: 50, max: 200), `offset` (default: 0)
- `ListResult`: `issues: Vec<IssueSummary>`, `total: usize`, `stale: bool`
- Read tool — uses cache, no invalidation
- All filters are optional, combinable (AND logic)
- Sort: `priority` ASC (default), `created` ASC, `updated` DESC
- Pagination: offset/limit for sequential pages
- Filter normalisation: empty/whitespace-only strings treated as absent (use `normalize_filter()`)

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListParams {
    pub status: Option<String>,
    pub priority: Option<String>,
    #[serde(rename = "type")]
    pub issue_type: Option<String>,
    pub milestone: Option<String>,
    pub agent: Option<String>,
    pub label: Option<String>,
    pub assignee: Option<String>,
    pub sort: Option<String>,
    #[serde(default = "default_list_limit")]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

fn default_list_limit() -> Option<usize> { Some(50) }

#[derive(Debug, Serialize)]
pub struct ListResult {
    pub issues: Vec<IssueSummary>,
    pub total: usize,
    pub stale: bool,
}
```

**Tests:**
- `list_returns_all_issues_no_filter`
- `list_filters_by_status`
- `list_filters_by_priority`
- `list_filters_by_type`
- `list_filters_by_milestone`
- `list_filters_by_label`
- `list_filters_by_assignee`
- `list_combined_filters`
- `list_sorts_by_created`
- `list_pagination_offset_limit`
- `list_empty_string_filter_treated_as_absent`

---

#### Task 06.02 — `search` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/tools/search.rs`

Full-text search via GitHub Search API. Bypasses cache entirely — does not use the graph.

→ Spec: [03-spec-mcp-tools.md §4.6](../specs/03-spec-mcp-tools.md#46-search)

Requirements:
- `SearchParams`: `query: String` (required, non-empty), `limit: Option<u32>` (default: 20)
- `SearchResult`: `issues: Vec<IssueSummary>`, `count: usize`
- Constructs search query: `"repo:{owner}/{repo} is:issue {query}"`
- Maps GitHub Search API results to `IssueSummary`
- Uses `execute_read_tool` but bypasses cache — each search hits GitHub Search API
- Validation: `query` must be non-empty

**GitHub API:** `GET /search/issues?q=repo:{owner}/{repo}+is:issue+{query}&per_page={limit}`

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Search query — matched against issue title and body.
    pub query: String,
    /// Maximum results to return. Default: 20.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub issues: Vec<IssueSummary>,
    pub count: usize,
}
```

**New method required on `GitHubApi` trait:**
- `async fn search_issues(&self, query: &str, limit: u32) -> Result<Vec<Issue>, Error>`

**Tests:**
- `search_returns_matching_issues`
- `search_empty_query_returns_validation_error`
- `search_respects_limit`

---

#### Task 06.03 — `stats` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/tools/stats.rs`

Aggregate counts across all issues. Read-only, uses cache.

→ Spec: [03-spec-mcp-tools.md §4.4](../specs/03-spec-mcp-tools.md#44-stats)

Requirements:
- `StatsParams`: `milestone: Option<String>` (optional filter)
- `StatsResult`: `total`, `by_status: HashMap<String, usize>`, `by_priority: HashMap<String, usize>`, `blocked_count`, `ready_count`, `cycle_count`, `agents: Vec<AgentStats>`
- `AgentStats`: `name: String`, `in_progress: usize`, `completed: usize`
- Computed from cached graph data
- Optional milestone filter reduces scope
- Cycle count from `detect_all_cycles().len()`

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatsParams {
    /// Filter stats to a specific milestone.
    pub milestone: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StatsResult {
    pub total: usize,
    pub by_status: HashMap<String, usize>,
    pub by_priority: HashMap<String, usize>,
    pub blocked_count: usize,
    pub ready_count: usize,
    pub cycle_count: usize,
    pub agents: Vec<AgentStats>,
}

#[derive(Debug, Serialize)]
pub struct AgentStats {
    pub name: String,
    pub in_progress: usize,
    pub completed: usize,
}
```

**Tests:**
- `stats_returns_correct_counts`
- `stats_filters_by_milestone`
- `stats_counts_agents`
- `stats_counts_cycles`

---

#### Task 06.04 — `reopen` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/tools/reopen.rs`

Reopens a closed issue and evaluates its blocking status.

→ Spec: [03-spec-mcp-tools.md §5.7](../specs/03-spec-mcp-tools.md#57-reopen)

Requirements:
- `ReopenParams`: `id: u64` (required)
- `ReopenResult`: `issue: u64`, `blocked: bool`, `status: String`
- Validates: issue must be in `IssueState::Closed` → else `IssueNotClosed` or `IssueAlreadyOpen`
- Reopens via REST PATCH `state=open`
- Rebuilds graph to evaluate blocking status
- If issue has open blockers: Status → Blocked
- If no open blockers: Status → Ready
- Write tool — invalidates cache and rebuilds

**New method required on `GitHubApi` trait:**
- `async fn reopen_issue(&self, number: u64) -> Result<(), Error>`

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReopenParams {
    /// Issue number to reopen.
    pub id: u64,
}

#[derive(Debug, Serialize)]
pub struct ReopenResult {
    pub issue: u64,
    pub blocked: bool,
    pub status: String,
    pub pipeline_stage: Option<String>,
}
```

**Tests:**
- `reopen_closed_issue_sets_ready`
- `reopen_closed_issue_with_blockers_sets_blocked`
- `reopen_open_issue_returns_error`

---

#### Task 06.05 — `dep_remove` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/tools/dep_remove.rs`

Removes a blocking relationship between two issues.

→ Spec: [03-spec-mcp-tools.md §5.5](../specs/03-spec-mcp-tools.md#55-dep_remove)

Requirements:
- `DepRemoveParams`: `source: String` (IssueRef), `target: String` (IssueRef)
- `DepRemoveResult`: `removed: bool`, `source: String`, `target: String`, `message: String`
- Resolves both IssueRefs (supports cross-repo)
- Validates edge exists in the graph
- Calls `remove_blocked_by()` mutation (already exists on `GitHubApi`)
- Rebuild graph, recompute ready states
- If source now has zero open blockers: update Status → Ready
- Write tool — invalidates cache and rebuilds

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DepRemoveParams {
    /// Issue that is currently blocked (local or cross-repo IssueRef).
    pub source: String,
    /// Issue that is currently blocking the source (local or cross-repo IssueRef).
    pub target: String,
}

#[derive(Debug, Serialize)]
pub struct DepRemoveResult {
    pub removed: bool,
    pub source: String,
    pub target: String,
    pub message: String,
}
```

**Tests:**
- `dep_remove_removes_existing_edge`
- `dep_remove_updates_status_when_unblocked`
- `dep_remove_nonexistent_edge_returns_error`
- `dep_remove_cross_repo`

---

#### Task 06.06 — `dep_cycles` tool

> **Priority:** P1  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/tools/dep_cycles.rs`

Detects dependency cycles in the graph. Read-only, uses cache.

→ Spec: [03-spec-mcp-tools.md §4.7](../specs/03-spec-mcp-tools.md#47-dep_cycles)

Requirements:
- `DepCyclesParams`: `id: Option<u64>` (optional — targeted check from specific issue)
- `DepCyclesResult`: `cycles: Vec<Vec<u64>>`, `count: usize`
- If `id` provided: check for cycles involving that specific issue
- If `id` absent: `detect_all_cycles()` on full graph
- Read tool — uses cache, no invalidation
- Returns cycles as vectors of issue numbers

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DepCyclesParams {
    /// Optional issue number for targeted cycle check.
    pub id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct DepCyclesResult {
    pub cycles: Vec<Vec<u64>>,
    pub count: usize,
}
```

**Tests:**
- `dep_cycles_no_cycles_returns_empty`
- `dep_cycles_detects_simple_cycle`
- `dep_cycles_detects_complex_cycle`
- `dep_cycles_targeted_check_by_id`

---

#### Task 06.07 — Register 6 new tools in MCP server

> **Priority:** P0  
> **Depends on:** Tasks 06.01–06.06  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/server.rs`

Register `list`, `search`, `stats`, `reopen`, `dep_remove`, `dep_cycles` in the `#[tool_router]` macro alongside existing 11 tools. Total: 17 tools.

Also add module declarations in `tools/mod.rs`:
```rust
pub mod list;
pub mod search;
pub mod stats;
pub mod reopen;
pub mod dep_remove;
pub mod dep_cycles;
```

**Tests:**
- `server_lists_all_17_tools`

---

#### Task 06.08 — `search_issues` and `reopen_issue` on GitHubApi trait

> **Priority:** P0  
> **Depends on:** nothing  
> **Blocked by:** nothing

**Files:** `unblock-github/src/api.rs`, `unblock-github/src/mutations.rs`, `unblock-github/src/mock.rs`

Two new methods required by Epic 06 tools:

1. `search_issues(query, limit) -> Result<Vec<Issue>, Error>` — REST `GET /search/issues`. Parse `items` array.
2. `reopen_issue(number) -> Result<(), Error>` — REST `PATCH /repos/{o}/{r}/issues/{n}` with `{"state": "open"}`.

Add to `GitHubApi` trait, implement on `GitHubClient`, add stubs to `MockGitHubClient`.

**Tests:**
- `search_issues_returns_results` (integration)
- `reopen_issue_changes_state` (integration)
- Mock stubs compile and function

---

## 6. Definition of Done

Phase 01 is complete when:

1. **All 17 tools registered and functional** — `server_lists_all_17_tools` test passes
2. **Quality gate green** — `cargo fmt --check --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --no-deps --workspace`
3. **E2E workflow** — Full agent loop: `prime` → `ready` → `claim` → `close` → cascade verified
4. **Integration tests** — Each tool has at least one integration test with `MockGitHubClient`
5. **Agent can do productive work** — `prime` → `ready` → `claim` in under 2 seconds on warm cache
6. **Zero data loss** — If `unblock-mcp` process dies, all state is in GitHub

---

*This plan defines Phase 01 scope, implementation status, and remaining work. The how is in the companion specs. Phase 02 (MCP Complete) depends on all 17 tools being functional.*
