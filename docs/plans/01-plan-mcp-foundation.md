# Plan 01 — MCP Foundation (v0.1.0)

> Phase: 01
> Version: v0.1.0
> Crates: `unblock-core`, `unblock-github`, `unblock-mcp`
> Depends on: nothing
> Required by: Phase 02 (MCP Complete)
> Source: [MANIFESTO](../MANIFESTO.md) · [PRD](../PRD.md) · [SPEC](../SPEC.md)

---

## Table of Contents

0. [Implementation History](#implementation-history)
1. [Purpose](#1-purpose)
2. [Scope](#2-scope)
3. [Out of Scope](#3-out-of-scope)
4. [Crate Architecture](#4-crate-architecture)
5. [Rust Idioms & Rules](#5-rust-idioms--rules)
6. [Data Model](#6-data-model)
7. [Epic 01 — Workspace & Infrastructure](#epic-01--workspace--infrastructure)
8. [Epic 02 — Core Library (unblock-core)](#epic-02--core-library-unblock-core)
9. [Epic 03 — GitHub API Layer (unblock-github)](#epic-03--github-api-layer-unblock-github)
10. [Epic 04 — MCP Server + Core Tools](#epic-04--mcp-server--core-tools)
11. [Epic 05 — GitHubApi Trait Abstraction](#epic-05--githubapi-trait-abstraction)
12. [Epic 06 — Foundation Completion](#epic-06--foundation-completion)
13. [Gap Analysis — Implementation vs Plan](#gap-analysis--implementation-vs-plan)
14. [Definition of Done](#definition-of-done)

---

## Implementation History

Phase 01 was originally implemented against an earlier plan and architecture (now archived in `docs/archive/`). The current plan and spec (this document + `specs/01-spec-mcp-foundation.md`) supersede those archived documents. The [Gap Analysis](#gap-analysis--implementation-vs-plan) section (GAPs 01–13) documents all known divergences between the existing code and this plan. Implementation work for Phase 01 completion must address these gaps before implementing the 6 remaining tools (Epic 06).

---

## 1. Purpose

Phase 01 delivers the minimum viable agent workflow loop. An agent connects via MCP (stdio), finds unblocked work, claims it, implements, closes, and sees the cascade promote newly unblocked issues to ready. The graph engine, cache, GitHub client, and 17 MCP tools form a complete local product.

**Outcome:** `v0.1.0` — agent can `prime` → `ready` → `claim` → work → `close` → cascade. Under 2 seconds on warm cache.

**Governing laws** (from MANIFESTO):

1. GitHub stores, Rust computes — zero custom storage
2. Every write invalidates and recomputes — consistency after mutations
3. The agent is always one command away from productive work
4. Correct GitHub primitive — comments for work log, fields for typed data, body sections for prose
5. Session isolation is architectural
6. Cascade is structural — not optional
7. One graph, one truth — the dependency graph is authoritative

---

## 2. Scope

### 2.1 — 17 MCP Tools

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

### 2.2 — Infrastructure

- Cargo workspace: 3 crates (`unblock-core`, `unblock-github`, `unblock-mcp`)
- Graph engine: petgraph `DiGraph` with `QualifiedId` nodes
- TTL cache: `GraphCache` with async `RwLock`, invalidation after every write
- GitHub client: `GitHubClient` with paginated GraphQL reads, REST/GraphQL mutations
- `GitHubApi` trait abstraction for testing with `MockGitHubClient`
- Projects V2 field management: 7 custom fields, 5 pre-configured views
- CI pipeline: GitHub Actions (fmt, clippy, test, doc)
- Error model: `snafu` exclusive, three-layer hierarchy (domain → infrastructure → MCP)
- Logging: `tracing`, JSON to stderr (stdout reserved for MCP stdio)
- Configuration: environment variables only, `Config::load_from()` pattern

---

## 3. Out of Scope

These are explicitly **not** Phase 01:

| Feature | Phase | Rationale |
|---|---|---|
| `reconcile` tool | 02 | Drift detection requires recently-closed query |
| `doctor` tool | 02 | Health checks build on reconcile |
| `commit_context` tool | 02 | Structured commit messages |
| Agent client detection (`AgentKind`) | 02 | Detection heuristics, `SessionMeta` |
| Circuit breaker | 02 | Resilience layer |
| Retry with exponential backoff | 02 | Resilience layer |
| OpenTelemetry metrics | 02 | Observability layer |
| Materialised fast path | 03 | Cold start optimisation |
| `cargo-dist` distribution | 03 | Cross-platform binaries |
| GitHub App authentication | 03 | Enterprise auth |
| GHE Server testing | 03 | Enterprise support |
| Plugin pipeline (skills, agents) | 04 | Orchestration layer |
| Remote server (HTTP transport) | 05 | Multi-tenant |
| LLM agent (autonomous) | 06 | Codestral integration |

---

## 4. Crate Architecture

```
unblock-mcp (bin, stdio)
  ├── unblock-github (lib)
  │     └── unblock-core (lib)
  └── unblock-core (lib)
```

| Crate | Role | Network I/O | License |
|---|---|---|---|
| `unblock-core` | Domain types, graph engine, cache, config | None | MIT |
| `unblock-github` | GitHub API client (GraphQL + REST) | Yes | MIT |
| `unblock-mcp` | MCP server binary, tool handlers | Via unblock-github | MIT |

**Principle:** Pure core, impure shell. `unblock-core` has zero network I/O. All computation is testable with in-memory data.

---

## 5. Rust Idioms & Rules

### 5.1 — Edition 2024, no unsafe

Workspace-wide `edition = "2024"` and `#![deny(unsafe_code)]`. No exceptions.

### 5.2 — `snafu` exclusive

Every crate uses `snafu` for errors. No `thiserror`, no `anyhow`, no `Box<dyn Error>`. Every crate defines its error types in `src/errors.rs` and re-exports a crate-scoped `Result<T>`. `unwrap()` and `expect()` forbidden outside test modules.

### 5.3 — Pure core, impure shell

`unblock-core` has zero network I/O. All computation testable with in-memory data. `unblock-github` handles all GitHub API communication. `unblock-mcp` is the thin MCP shell that wires tools to state.

### 5.4 — Trait abstraction for testing

`GitHubApi` trait in `unblock-github/src/api.rs` abstracts all GitHub operations. `ServerState` holds `Arc<dyn GitHubApi>`. Tests use `MockGitHubClient` with call counters and response stubs, feature-gated behind `test-hooks`.

### 5.5 — Environment-based config

`Config::load_from(env_reader)` accepts a closure for testability. No `std::env::set_var` in tests (unsafe in edition 2024). Tests supply `HashMap`-backed closures.

### 5.6 — Write-through cache

Every write tool: execute mutation → `cache.invalidate()` → `fetch_graph_data()` → build graph → compute ready set → `cache.update()`. Lock never held across network I/O.

---

## 6. Data Model

> Source: SPEC §2

### 6.1 — GitHub Primitives Used

| Primitive | Purpose | API |
|---|---|---|
| Issue number | Issue ID (`#42`) | REST + GraphQL |
| Issue state | Open/Closed ground truth | REST + GraphQL |
| Issue type | Classification: `task`, `bug`, `feature`, `epic`, `chore`, `spike` | REST + GraphQL (org-level) |
| Labels | Flexible tagging | REST + GraphQL |
| Assignees | Human assignment | REST + GraphQL |
| Milestones | Sprint/release grouping | REST + GraphQL |
| Comments | Discussion thread, audit trail | REST |
| Sub-issues | Parent/child hierarchy | GraphQL (`sub_issues` feature) |
| Blocking | Dependency edges | GraphQL mutations |
| Issue body | Markdown with structured sections | REST + GraphQL |
| Projects V2 | Custom fields, views, boards | GraphQL + REST (views) |

### 6.2 — Projects V2 Custom Fields (7 total)

> Source: SPEC §2.2

| Field | Type | Values | Purpose |
|---|---|---|---|
| Status | Single Select | `ready`, `in_progress`, `blocked`, `deferred`, `closed` | Unified workflow + readiness state |
| Priority | Single Select | `P0 - Critical`, `P1 - High`, `P2 - Medium`, `P3 - Low`, `P4 - Backlog` | Sortable priority for ready queue |
| Pipeline Stage | Single Select | `investigation`, `implementation`, `review`, `refactoring`, `qa`, `done` | Development pipeline phase |
| Agent | Text | Free text | Which AI agent is working on this |
| Claimed At | Date | ISO datetime | Timestamp of claim |
| Story Points | Number | Integer | Estimation |
| Defer Until | Date | Date | Hidden from ready queue until this date |

**Status transitions managed by MCP server:**
- `ready` ↔ `blocked`: automatic, from dependency graph computation
- → `in_progress`: on `claim`
- → `deferred`: on `update` with `defer_until`
- → `closed`: on `close`
- `blocked`/`ready` → any: on `reopen` (re-evaluated from graph)

**Pipeline Stage:** Created by `setup` in Phase 01 for field existence. Agent advancement is Phase 04 (plugin). The field exists so that early adopters can use it manually.

### 6.3 — Issue Body Structure

> Source: SPEC §2.3

Three structured sections:

```markdown
## Description
Full issue description.

## Design Notes
Technical design decisions.

## Acceptance Criteria
- [ ] Criterion 1
```

Parsed by `BodySections::from_markdown()`. Round-trippable via `to_markdown()`.

### 6.4 — Pre-configured Views (5 total)

> Source: SPEC §2.5

| View | Layout | Purpose |
|---|---|---|
| `UNBLOCK://ready` | Board | Agent's ready queue, filtered to Status: `ready` |
| `UNBLOCK://team` | Board | Tech lead view, grouped by Agent |
| `UNBLOCK://pipeline` | Board | Dev pipeline, grouped by Pipeline Stage |
| `UNBLOCK://roadmap` | Table | Epic-level progress |
| `UNBLOCK://timeline` | Roadmap | Date-based timeline |

### 6.5 — Dependency Model

Single blocking type. GitHub's native `blockedBy`/`blocking`. Binary: blocks or does not. Edge direction in graph: `blocked_issue → blocking_issue` (source depends on target). Cross-repo supported via `QualifiedId`.

---

## Epic 01 — Workspace & Infrastructure

**Goal:** Cargo workspace scaffold, CI pipeline, crate skeletons, developer tooling.

### Task 01.01 — Cargo workspace scaffold

**Files:** `Cargo.toml` (workspace root), `crates/*/Cargo.toml`

- Workspace with 3 members: `unblock-core`, `unblock-github`, `unblock-mcp`
- Edition 2024, `#![deny(unsafe_code)]` workspace-wide
- Shared dependencies via `[workspace.dependencies]`
- Core dependencies: `petgraph`, `rmcp`, `reqwest`, `snafu`, `tracing`, `tokio`, `serde`, `schemars`, `chrono`

### Task 01.02 — CI pipeline

**File:** `.github/workflows/ci.yml`

Quality gate: `cargo fmt --check --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --no-deps --workspace`. Runs on push to main and pull requests.

### Task 01.03 — Crate skeletons

**Files:** `crates/*/src/lib.rs`

Each crate compiles with module declarations. Dependency graph: `unblock-mcp` → `unblock-github` → `unblock-core`.

---

## Epic 02 — Core Library (unblock-core)

**Goal:** Pure Rust domain types, graph engine, cache layer, configuration. Zero network I/O.

### Task 02.01 — Domain types

**File:** `unblock-core/src/types.rs`

Types:
- `QualifiedId { owner, repo, number }` — Display, FromStr, Hash, Eq
- `Issue` — full issue with all Projects V2 fields: status, priority, pipeline_stage, agent, claimed_at, story_points, defer_until, body sections, relationships
- `IssueState { Open, Closed }` — GitHub native
- `Status { Ready, InProgress, Blocked, Deferred, Closed }` — Projects V2 unified field
- `Priority { P0, P1, P2, P3, P4 }` — with `as_sort_key()`
- `PipelineStage { Investigation, Implementation, Review, Refactoring, Qa, Done }`
- `IssueType { Task, Bug, Feature, Epic, Chore, Spike }`
- `BlockingEdge { source, target }` — both `QualifiedId`
- `IssueSummary` — lightweight issue for list/ready responses
- `IssueRef { Local(u64), CrossRepo { owner, repo, number } }` — with `resolve()`
- `BodySections { description, design_notes, acceptance_criteria }` — `from_markdown()`, `to_markdown()`
- `TraversalDirection { Upstream, Downstream, Both }`
- `IssueComment { author, body, created_at }`
- `RelatedIssue { number, title, state, repo_owner: Option<String>, repo_name: Option<String> }` — `#[non_exhaustive]`; construct via `RelatedIssue::local()` (same-repo-as-enclosing) or `RelatedIssue::cross_repo()` (explicit owner/name). See SPEC §2.12; hardened in unblock-29p.66.
- `CrossRepoRefs { omitted: Vec<String>, summary: Option<String> }` — cross-repo response contract (SPEC §2.16, §11.4)

### Task 02.02 — Domain errors

**File:** `unblock-core/src/errors.rs`

`DomainError` enum — 13 variants: `IssueNotFound`, `AlreadyClaimed`, `IssueBlocked`, `IssueDeferred`, `IssueClosed`, `IssueNotClosed`, `IssueAlreadyOpen`, `CircularDependency`, `DuplicateDependency`, `FieldNotFound`, `Validation`, `InvalidIssueRef`, `CrossRepoAccessDenied`. Each with `status_code() -> u16`.

**Cross-repo-aware variant typing (SPEC §11.1 Decision 1, unblock-eos arbitration 2026-04-17).** The following three variants carry `IssueRef` (§2.7), NOT bare `u64`:

- `IssueBlocked { number: u64, blockers: Vec<IssueRef> }`
- `CircularDependency { source: IssueRef, target: IssueRef }`
- `DuplicateDependency { source: IssueRef, target: IssueRef }`

All other variants retain their current shape (bare `u64` or strings) because they refer to an issue in the configured repo only — their call sites in §5.6 never surface a cross-repo number into these error paths.

**BREAKING CHANGE discipline (CLAUDE.md → Pub API Change Tracking).** The variant field-type changes above are an incompatible change to the `unblock-core` pub API (`DomainError` is `pub`, its variants are `pub`, and `Vec<IssueRef>` / `IssueRef` are not interchangeable with `Vec<u64>` / `u64` at the type level). The implementing bead (unblock-29p.25) MUST land the change with a commit message footer of the form:

```
BREAKING CHANGE: DomainError::{IssueBlocked, CircularDependency,
DuplicateDependency} variants now carry `IssueRef` instead of bare
`u64` for cross-repo-aware error reporting. Callers constructing
these variants via snafu context selectors (e.g. IssueBlockedSnafu,
CircularDependencySnafu, DuplicateDependencySnafu) must pass
IssueRef values. Display output for the Local(n) case is preserved
byte-for-byte; CrossRepo renders as "owner/repo#number". See SPEC
§11.1 Decision 1 / §11.4 cross-repo contract closure.
```

An `API:` body line is INSUFFICIENT — Conventional Commits requires `BREAKING CHANGE:` for incompatible pub changes. Scope: library crate (`unblock-core`), so the rule applies per CLAUDE.md.

**Display preservation (SPEC §11.1).** The existing Display tests at `crates/unblock-core/src/errors.rs:215-240` MUST continue to pass byte-for-byte for the `IssueRef::Local` case. Specifically:

- `CircularDependencySnafu { source: IssueRef::Local(1), target: IssueRef::Local(2) }.build().to_string()` MUST render `"Circular dependency: adding #1 → #2 creates cycle"` (matches current test assertions on substrings `'1'`, `'2'`, `"cycle"`).
- `DuplicateDependencySnafu { source: IssueRef::Local(4), target: IssueRef::Local(5) }.build().to_string()` MUST contain `'4'` and `'5'` (matches current test assertions).
- `IssueBlockedSnafu { number: 10, blockers: vec![IssueRef::Local(1), IssueRef::Local(2)] }.build().to_string()` MUST contain `"10"` (matches `errors.rs:170-174`).
- Cross-repo case: `CircularDependencySnafu { source: IssueRef::CrossRepo { owner: "acme".into(), repo: "widgets".into(), number: 1 }, target: IssueRef::Local(2) }.build().to_string()` renders with `"acme/widgets#1"` somewhere in the string. Add a new test asserting this; do NOT modify the existing Local-only tests.

**Implementer trap — Debug vs. Display for `IssueBlocked.blockers`.** The current `#[snafu(display(...))]` attribute at `crates/unblock-core/src/errors.rs:41` is `"Issue #{number} is blocked by: {blockers:?}"` — `{blockers:?}` is the Debug formatter. Under `Vec<u64>` it renders `[1, 2]`; under `Vec<IssueRef>` it renders `[Local(1), Local(2)]` (variant names leak). The existing test at `errors.rs:170-174` only asserts substring `"10"` so this Debug output still passes, but any future tightening to assert `"#1"` would silently break. The implementation MUST replace the `{blockers:?}` Debug attribute with a Display-based renderer — either a format string interpolating `IssueRef::Display` via a joined helper, or a pre-formatted blocker list built by iterating and calling `IssueRef`'s `Display` impl. This is not a contract change; it is an implementer trap flagged so the Display-preservation contract above is not silently violated. See SPEC §11.1 "Implementer trap".

**Error-side wiring (SPEC §11.1 Decision 2, confirmed no shape change).**

- `InvalidIssueRef { input: String }` — emitted whenever `IssueRef::from_str` fails on tool input. The tool-layer call sites (§8.4 `depends`, §8.5 `dep_remove`, §8.3 `create.blocked_by`, §7.2 `show`) MUST wrap the parse error into `InvalidIssueRefSnafu { input: <raw user string> }` and propagate. Maps to HTTP 400 → MCP `-32602` (invalid params) via §11.3.
- `CrossRepoAccessDenied { owner: String, repo: String }` — emitted when a GraphQL fetch against a cross-repo node returns `FORBIDDEN` (or equivalent 403). Maps to HTTP 403. Per SPEC §11.3 `github_error_to_mcp` the 403 branch maps to MCP `-32602` (invalid params / business rule, same bucket as other 4xx domain errors). The `unblock-mcp/src/errors.rs` match arm MUST add the 403 → `-32602` mapping explicitly so regressions are caught by the error-mapping test. Add a unit test in `unblock-mcp/src/errors.rs` that maps the new 403 branch so future variant additions don't silently collapse into the catch-all.

These two variants are already spec'd (§11.1 lines above in the table); the implementation bead unblock-6xj wires the propagation paths listed above. No spec shape change is needed for this task — only the wiring contract is new.

### Task 02.03 — Configuration

**File:** `unblock-core/src/config.rs`

`Config` struct: `token`, `api_base_url`, `github_url`, `repo`, `project_number`, `agent`, `cache_ttl`, `log_level`, `otel_endpoint`. Loaded from environment variables via `load_from(env_reader)`.

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `GITHUB_TOKEN` | Yes | — | Authentication |
| `GITHUB_API_URL` | No | `https://api.github.com` | GHE support |
| `GITHUB_URL` | No | `https://github.com` | GHE support |
| `UNBLOCK_REPO` | No | Auto-detect from git remote | Repository |
| `UNBLOCK_PROJECT` | No | Auto-detect | Project number |
| `UNBLOCK_AGENT` | No | `"agent"` | Default agent name |
| `UNBLOCK_CACHE_TTL` | No | `30` | Cache TTL (seconds) |
| `UNBLOCK_LOG_LEVEL` | No | `"info"` | Log level |

### Task 02.04 — Graph engine — build and ready set

**File:** `unblock-core/src/graph.rs`

`DependencyGraph` with petgraph `DiGraph<QualifiedId, ()>`. Edge direction: blocked → blocker.

Methods:
- `build(issues, edges) -> Self`
- `compute_ready_set(issues, configured_owner, configured_repo) -> Vec<IssueSummary>` — issue is ready if: `IssueState::Open` AND `Status::Ready` (not InProgress/Blocked/Deferred/Closed) AND `qualified_id.(owner, repo) == (configured_owner, configured_repo)` (SPEC §3.3 Filter 3 / §14 Invariant 14(a), introduced by unblock-eos.4 / D6.a / GAP-14.b) AND all blockers have `IssueState::Closed`. Sorted: priority ASC → created_at ASC. BREAKING CHANGE on `unblock-core` pub API — see GAP-14.b.

### Task 02.05 — Graph engine — cascade

**File:** `unblock-core/src/graph.rs`

- `compute_unblock_cascade(closed_id, all_issues) -> Vec<QualifiedId>` — dependents whose blockers are all now closed

### Task 02.06 — Graph engine — cycles and tree traversal

**File:** `unblock-core/src/graph.rs`

- `would_create_cycle(source, target) -> bool` — pre-mutation check via `has_path_connecting`
- `detect_all_cycles() -> Vec<Vec<QualifiedId>>` — Tarjan's SCC
- `dependency_tree(root, direction, max_depth) -> DependencyTree` — BFS traversal returning `DependencyTree { root, upstream: Vec<TreeNode>, downstream: Vec<TreeNode> }`
- `all_edges() -> Vec<BlockingEdge>`, `edge_count() -> usize`

### Task 02.07 — Cache layer

**File:** `unblock-core/src/cache.rs`

`GraphCache` with `RwLock<Option<CacheEntry>>` and `Duration` TTL.

```
CacheEntry { graph, ready_set, built_at }
CacheResult  { Fresh, Stale, Empty }
```

Methods: `new()`, `get_ready_set()`, `get_graph()`, `update()`, `invalidate()`, `is_fresh()`.

**Invariant:** Every field in `CacheEntry` is reconstructable from GitHub with a single `fetch_graph_data()` call. The cache is a performance optimisation, not a source of truth.

---

## Epic 03 — GitHub API Layer (unblock-github)

**Goal:** GitHub client with paginated GraphQL reads, REST/GraphQL mutations, Projects V2 field management.

### Task 03.01 — GitHub client bootstrap

**File:** `unblock-github/src/client.rs`

`GitHubClient` with `reqwest::Client`, token in default headers, repo resolution from `UNBLOCK_REPO` or git remote parsing. GHE URL resolution: `graphql_url()` strips `/v3` suffix for GHE Server.

### Task 03.02 — `fetch_graph_data` — paginated GraphQL

**File:** `unblock-github/src/graphql.rs`

Single paginated query returns all open issues with blocking edges, sub-issues, Projects V2 field values. 100 issues per page. Cross-repo blocking edges supported.

### Task 03.03 — `fetch_issue` — single issue query

**File:** `unblock-github/src/graphql.rs`

`fetch_issue(number)` and `fetch_issue_ref(IssueRef)` — always fresh, never cached. Includes comments and full field values.

### Task 03.04 — Mutations (REST + GraphQL)

**File:** `unblock-github/src/mutations.rs`

REST: `create_issue()`, `close_issue()`, `reopen_issue()`, `add_comment()`, `update_issue_body()`, label ops, assignee ops, milestone ops.
GraphQL: `add_blocked_by()`, `remove_blocked_by()`, `add_sub_issue()`.

Cross-repo scope: dependency mutations accept any two Issue node IDs. Write mutations (close, reopen, update) scoped to configured repo.

### Task 03.05 — Projects V2 field management

**File:** `unblock-github/src/projects.rs`

`ProjectFieldIds` — caches GraphQL node IDs and option IDs for all 7 fields.
`FieldMeta { field_id, options: HashMap<String, String> }` — for single-select fields.

Functions: `resolve_project_info()`, `setup_fields()`, `update_field()`, `batch_update_fields()`, `query_setup_status()`.

7 custom fields: Status, Priority, Pipeline Stage, Agent, Claimed At, Story Points, Defer Until.

### Task 03.06 — View management

**File:** `unblock-github/src/projects.rs`

5 views created via REST API (`X-GitHub-Api-Version: 2026-03-10`). Owner type detection (org vs user) for correct REST endpoint. Integer field ID discovery via REST `/fields`.

### Task 03.07 — Infrastructure errors

**File:** `unblock-github/src/errors.rs`

`Error` enum: `Domain`, `GitHubApi`, `GitHubGraphQL`, `GitHubUnavailable`, `RateLimited`, `CircuitBreakerOpen` (stub — active in Phase 02), `ProjectNotConfigured`, `GitRemote`, `UnknownOwnerType`.

`status_code() -> u16` mapping. `is_retryable()` classification (prep for Phase 02 retry logic).

---

## Epic 04 — MCP Server + Core Tools

**Goal:** MCP server over stdio with tool registration, shared execution pattern, and 11 core tools.

### Task 04.01 — MCP server bootstrap

**Files:** `unblock-mcp/src/main.rs`, `unblock-mcp/src/server.rs`

`ServerState` with `Arc<Config>`, `Arc<dyn GitHubApi>`, `Arc<GraphCache>`.

Bootstrap: load config → init tracing (JSON, stderr) → create GitHub client → resolve repo + project + fields → validate fields → create cache → create server → serve on stdio.

Bootstrap mode: if no project detected, only `init` and `setup` are functional. All other tools return `ProjectNotConfigured`.

### Task 04.02 — Tool execution pattern

**File:** `unblock-mcp/src/tools/mod.rs`

Shared helpers:
- `execute_read_tool(state, op)` — maps errors to `ErrorData`, no cache invalidation
- `execute_write_tool(state, op)` — maps errors, then `rebuild_cache(state)`
- `rebuild_cache(state)` — invalidate → fetch_graph_data → build graph → compute ready set → diff Status fields → batch update changed fields → update cache

### Task 04.03 — `init` tool

Creates Projects V2 board. Detects owner type (org vs user). Idempotent.

> **Spec note:** The spec (§8.9) simplifies `init` to a single optional `title` param. The original `scope`, `description`, and `public` params were removed during spec refinement as unnecessary complexity for Phase 01.

### Task 04.04 — `setup` tool

Creates 7 fields and 5 views. Dry-run mode. Idempotent. REST field discovery for integer IDs.

> **Spec note:** The spec (§8.10) adds a `migrate: Option<bool>` param not in the original plan. When enabled, `setup` adds existing open issues to the project with default field values.

### Task 04.05 — `ready` tool

Filters: `issue_type`, `priority`, `milestone`, `agent`, `label`, `include_claimed`, `limit`.
Post-filter: `defer_until > today` excluded.
Sort: priority ASC → created_at ASC. Default limit: 10.
Cache-aware: Fresh → serve, Stale/Empty → rebuild.

**Acceptance (additional):** `ReadyResult.cross_repo_refs: Option<CrossRepoRefs>` populated per SPEC §11.4 — surfaces any cross-repo `QualifiedId` that was an open blocker of a local issue kept out of the ready set (the agent otherwise cannot see what silently blocked the local issue). `None` when no cross-repo node influenced filtering. Integration test with `MockGitHubClient` must cover both the `Some` and `None` branches.

**Acceptance (additional, unblock-eos.4 / D6.a / GAP-14.b):** `ReadyResult.issues` MUST contain ONLY source issues whose `qualified_id.(owner, repo) == (configured_owner, configured_repo)` — SPEC §14 Invariant 14(a). This is enforced at the graph engine (`compute_ready_set`, SPEC §3.3 Filter 3), not re-checked at the tool layer. The tool handler MUST NOT add a redundant owner/repo filter on `ready_set`; doing so violates the "one chokepoint" design. Integration test (`crates/unblock-mcp/tests/integration.rs`) MUST add a case where `fetch_graph_data` returns a mix of configured-repo and cross-repo OPEN issues and assert that `ReadyResult.issues` contains no cross-repo source. `cross_repo_refs` remains the ONLY channel for cross-repo information in a `ReadyResult`, and it carries BLOCKERS only (never sources) post-eos.4.

### Task 04.06 — `claim` tool

Validates: open, not blocked, not deferred, not already claimed.
Updates: Status → `in_progress`, Agent → name, Claimed At → now.
Posts claim comment.

### Task 04.07 — `close` tool

Validates: open.
**Critical (MUST, per SPEC §8.2 step 2 and §3.4):** Cascade MUST be computed BEFORE closing the issue (pre-close graph). POST-close cascade topology is unsound by construction because `fetch_graph_data` is `states: OPEN`-only (`unblock-github/src/graphql.rs:129`) and excludes the just-closed issue from the rebuilt `node_map`; `compute_unblock_cascade` then short-circuits to `Vec::new()` at `unblock-core/src/graph.rs:289-291` regardless of whether dependents exist. See GAP-15 for the ordering-divergence remediation tracker.
Close via REST. Update Status → `closed`.
For each unblocked dependent: update Status → `ready`, post unblock comment.

**Acceptance (additional):** `CloseResult.cross_repo_refs: Option<CrossRepoRefs>` populated per SPEC §11.4 — cross-repo dependents that were cascade-updated but dropped from the `unblocked: Vec<u64>` projection MUST appear in `cross_repo_refs.omitted`, sorted by `QualifiedId::Display`. Cross-repo dependents ARE still cascade-updated (same Status/comment path) — only the response shape differs. Integration test must cover the cross-repo-dependent case.

### Task 04.08 — `create` tool

Params: title, type, priority, body, labels, milestone, blocked_by (cross-repo), parent (sub-issue), story_points, defer_until.
Creates issue, adds to project, sets fields. If blocked_by: cycle check + add deps + Status → `blocked`.

### Task 04.09 — `show` tool

Always fresh (never cached). Full issue with parsed body sections, blocking/blocked_by, parent/sub-issues, dependency tree (BFS, max depth), comments.

### Task 04.10 — `comment` tool

Posts comment. No cache invalidation — comments don't affect the graph.

### Task 04.11 — `update` tool

Selective updates: priority, status, labels_add/remove, assignees_add/remove, body_section (Description/Design Notes/Acceptance Criteria), milestone, story_points, defer_until, agent.

### Task 04.12 — `depends` tool

Params: `source` (IssueRef — local or cross-repo), `target` (IssueRef).
Validates both exist. Cycle check. Duplicate check.
`add_blocked_by()` mutation. Update source Status → `blocked`.

### Task 04.13 — `prime` tool

Context summary for agent session injection. Returns: repo, project, ready/blocked/in-progress counts, cycle warnings. Markdown blob for agent injection.

**Acceptance (additional):** Markdown output MUST include a trailing `## Cross-repo references` section per SPEC §11.4 (markdown adaptation) when the cycle summary touches cross-repo `QualifiedId` nodes. Section omitted entirely when no cross-repo node participated. Entries rendered as `owner/repo#N` (QualifiedId::Display), sorted lexicographically. Integration test must cover both branches.

### Task 04.14 — Error mapping

**File:** `unblock-mcp/src/errors.rs`

`github_error_to_mcp(err) -> McpError`. Maps domain errors to `-32602` (invalid params / business rule), infrastructure errors to `-32603` (internal / GitHub).

---

## Epic 05 — GitHubApi Trait Abstraction

**Goal:** Extract `GitHubApi` trait for dependency injection. Enable unit testing without live GitHub.

### Task 05.01 — Define GitHubApi trait

**File:** `unblock-github/src/api.rs`

Trait with all GitHub operations. Blanket impl on `GitHubClient`. `async_trait` for object safety.

### Task 05.02 — MockGitHubClient

**File:** `unblock-github/src/mock.rs` (feature: `test-hooks`)

`MockGitHubClient` with `CallCounts` (per-method atomic counters) and `Stubs` (per-method response queues). Full `GitHubApi` trait implementation.

### Task 05.03 — Migrate ServerState to trait

`ServerState.github`: `Arc<GitHubClient>` → `Arc<dyn GitHubApi>`. All tool handlers operate on the trait.

---

## Epic 06 — Foundation Completion

**Goal:** Implement the 6 remaining tools required for Phase 01 scope (17 tools total).

### Task 06.01 — `list` tool

Filtered, sorted, paginated access to all issues. Params: `status`, `priority`, `type`, `milestone`, `agent`, `label`, `assignee`, `sort`, `limit`, `offset`. Read tool, uses cache.

### Task 06.02 — `search` tool

Full-text search via GitHub Search API. Bypasses cache entirely. Params: `query`, `limit`.

**Requires:** `search_issues()` method on `GitHubApi` trait.

### Task 06.03 — `stats` tool

Aggregate counts: by_status, by_priority, blocked_count, ready_count, cycle_count, agents. Optional milestone filter. Read tool, uses cache.

### Task 06.04 — `reopen` tool

Reopens closed issue. Evaluates blocking status from graph. Updates Status → `ready` (no blockers) or `blocked` (has blockers).

**Requires:** `reopen_issue()` method on `GitHubApi` trait.

### Task 06.05 — `dep_remove` tool

Removes blocking relationship. Params: `source` (IssueRef), `target` (IssueRef). Cross-repo supported. If source now has zero open blockers: Status → `ready`.

### Task 06.06 — `dep_cycles` tool

Detects dependency cycles. Optional `id` param for targeted check. Read tool, uses cache.

**Acceptance (additional):** `DepCyclesResult.cross_repo_refs: Option<CrossRepoRefs>` populated per SPEC §11.4 — cross-repo `QualifiedId` cycle members dropped from the `cycles: Vec<Vec<u64>>` projection MUST appear in `cross_repo_refs.omitted`. When a cycle's local-projection length drops below 2 after stripping cross-repo members, the cycle is STILL emitted as a (possibly-shorter) `Vec<u64>` so the agent knows the cycle exists; the missing members are in `cross_repo_refs`. Integration tests must cover: (a) local-only cycle (`cross_repo_refs == None`), (b) mixed cycle (`cross_repo_refs == Some`), (c) the dep_cycles bead (unblock-29p.11) can resume implementation against this acceptance as-is.

### Task 06.07 — Register 6 new tools in MCP server

Register `list`, `search`, `stats`, `reopen`, `dep_remove`, `dep_cycles` in server.rs. Total: 17 tools.

### Task 06.08 — `search_issues` and `reopen_issue` on GitHubApi

Two new methods on trait + GitHubClient implementation + MockGitHubClient stubs.

---

## Gap Analysis — Implementation vs Plan

> This section compares the current codebase against this plan. Each gap is categorised:
> - **DRIFT** — implementation diverges from what docs specify
> - **MISSING** — not yet implemented
> - **EXTRA** — implemented but not in Phase 01 scope

---

### GAP-01 — `Status` enum values

| | Plan (from SPEC §2.2) | Implementation |
|---|---|---|
| Variants | `Ready, InProgress, Blocked, Deferred, Closed` | `Open, InProgress, Blocked, Deferred, Closed` |

**Type:** DRIFT
**Impact:** Critical — the entire system revolves around Status. The SPEC says `Ready` is a Status value. The implementation uses `Open` instead.
**Resolution:** Code changes to `Ready` per SPEC. `Status::Open` → `Status::Ready` across all crates.

---

### GAP-02 — `ReadyState` field exists but is not in SPEC

| | Plan (from SPEC §2.2) | Implementation |
|---|---|---|
| Field | Does not exist | `ReadyState { Ready, Blocked, NotReady, Closed }` |

**Type:** DRIFT / EXTRA
**Impact:** Critical — the implementation has split the concept across two fields: `Status` (workflow) and `ReadyState` (graph-computed readiness). The SPEC has a single unified `Status` field where `ready`/`blocked` are graph-computed values.
**Resolution:** Remove `ReadyState`. Unify into single `Status` field per SPEC. The `ready`/`blocked` transitions are managed by the MCP server via graph computation — no need for a separate field.

---

### GAP-03 — `ProjectFieldIds` field mismatch

| | Plan (7 fields from SPEC §2.2) | Implementation (7 fields) |
|---|---|---|
| Field 1 | Status | Status ✅ |
| Field 2 | Priority | Priority ✅ |
| Field 3 | **Pipeline Stage** | **IssueType** ❌ |
| Field 4 | Agent | Agent ✅ |
| Field 5 | **Claimed At** | **Story Points** ❌ |
| Field 6 | Story Points | **Defer Until** ❌ |
| Field 7 | Defer Until | **ReadyState** ❌ |

**Type:** DRIFT
**Impact:** Critical — 4 of 7 field slots differ.
- `Pipeline Stage` — missing from implementation (replaced by `IssueType`)
- `Claimed At` — missing from implementation entirely
- `IssueType` — in implementation but not in SPEC as a Projects V2 field (IssueType is a native GitHub org-level feature, not a custom field)
- `ReadyState` — in implementation but not in SPEC (see GAP-02)

**Resolution:** Align to SPEC. Add `Pipeline Stage` + `Claimed At`. Remove `IssueType` (it's a GitHub native feature, not a Projects V2 custom field) + `ReadyState` (unified into Status per GAP-02). Final 7 fields: Status, Priority, Pipeline Stage, Agent, Claimed At, Story Points, Defer Until.

---

### GAP-04 — Ready set computation filter

| | Plan (from SPEC §3.3) | Implementation |
|---|---|---|
| Filter | `IssueState::Open` AND `Status::Ready` | Only `IssueState::Open` checked |

**Type:** DRIFT
**Impact:** High — the ready set may include issues with Status `InProgress`, `Blocked`, or `Deferred` because the Status filter is missing.
**Location:** `unblock-core/src/graph.rs` (marked as TODO in code)
**Fix:** Add `Status` filter to `compute_ready_set()`.

---

### GAP-05 — `dependency_tree` return type

| | Plan (from SPEC §3.6) | Implementation |
|---|---|---|
| Return type | `DependencyTree { root, upstream, downstream }` | `Vec<(QualifiedId, usize)>` |

**Type:** DRIFT
**Impact:** Medium — no structured tree with directional separation.
**Location:** `unblock-core/src/graph.rs` (marked as DEVIATION in code)

---

### GAP-06 — `depends` tool source parameter

| | Plan | Implementation |
|---|---|---|
| `source` param | `IssueRef` (String — local or cross-repo) | `u64` (local only) |

**Type:** DRIFT
**Impact:** Medium — `depends` cannot accept cross-repo source. Only `target` supports `IssueRef`.
**Location:** `unblock-mcp/src/tools/depends.rs`

---

### GAP-07 — `close` tool Status values

| | Plan | Implementation |
|---|---|---|
| Closed issue Status | → `closed` | → `Done` |
| Unblocked dependent Status | → `ready` | → `Backlog` |

**Type:** DRIFT
**Impact:** High — Status option names don't match SPEC. Cascade sets dependents to `Backlog` instead of `ready`.

---

### GAP-08 — 6 tools not implemented

| Tool | Status |
|---|---|
| `list` | MISSING |
| `search` | MISSING |
| `stats` | MISSING |
| `reopen` | MISSING |
| `dep_remove` | MISSING |
| `dep_cycles` | MISSING |

**Type:** MISSING
**Impact:** Phase 01 requires 17 tools; only 11 are registered (+ reconcile which is Phase 02).

---

### GAP-09 — Phase 02 features implemented early

| Feature | Phase | Status in code |
|---|---|---|
| `reconcile` tool | 02 | Implemented and registered |
| `ReconcileEngine` | 02 | Implemented in `unblock-core/src/reconcile.rs` |
| `AgentKind` / `ClientDetector` | 02 | Implemented in `unblock-core/src/client.rs`, `detection.rs` |
| `SessionMeta` in prime | 02 | Implemented in `unblock-mcp/src/tools/prime.rs` |
| `OnceLock<AgentKind>` in ServerState | 02 | In `server.rs` |
| `OnceLock<AgentClient>` in ServerState | 02 | In `server.rs` |

**Type:** EXTRA
**Impact:** Low — these don't break Phase 01 but add scope beyond what the plan defines.
**Resolution:** Keep the code in place (harmless, already tested). Exclude from Phase 01 acceptance criteria. These features are Phase 02 scope and will be formally validated then.

---

### GAP-10 — Status field option values in Projects V2

| | Plan (from SPEC §2.2) | Implementation (projects.rs) |
|---|---|---|
| Options | `ready, in_progress, blocked, deferred, closed` | Needs verification — likely different given GAP-01 and GAP-07 |

**Type:** DRIFT (probable)
**Impact:** High — the `setup` tool creates these options in GitHub. If they don't match the SPEC, the entire field management is misaligned.

---

### GAP-11 — `FieldValue::Date` type

| | Plan (from SPEC) | Implementation |
|---|---|---|
| Type | ISO 8601 String | `NaiveDate` (chrono) |

**Type:** DRIFT
**Impact:** Low — `NaiveDate` is typed and safer than raw String.
**Resolution:** Keep `NaiveDate` in code. Update SPEC §5 to reflect `Date(NaiveDate)` instead of `Date(String)`. This is a SPEC improvement, not a code fix.

---

### GAP-12 — Missing integration tests — RESOLVED (unblock-29p.13)

Previously, four tools carried `TODO` comments pointing at missing
integration coverage:

- `close` tool — already-closed short-circuit (`IssueClosedSnafu`) +
  co-blocking dependent exclusion from `unblocked` / `cross_repo_refs`
- `depends` tool — happy-path local edge (Status=Blocked), §8.4
  `source != target` rejection, and cycle rejection via warm cache
- `claim` tool — happy-path open issue (two field updates +
  claim comment) and already-claimed rejection
- `comment` tool — happy path with cache-freshness invariant (§8.8
  skips rebuild) and §8.8 empty-body rejection without network calls

**Resolution:** Ten G5-style integration tests added against
`MockGitHubClient` in `crates/unblock-mcp/tests/integration.rs` covering
all four tools. The four in-code TODOs in
`crates/unblock-mcp/src/tools/{claim,depends,comment,close}.rs` were
deleted as part of the same change.

**Type:** MISSING — RESOLVED
**Impact:** Medium — reduces confidence in tool correctness.

---

### GAP-13 — Duplicated field update logic

`server.rs` has the project field update logic duplicated 4 times (marked as TODO).

**Type:** Technical debt
**Impact:** Low — works but fragile.

---

### GAP-14 — Cross-repo response contract retro-migration (SPEC §11.4)

SPEC §11.4 (added via unblock-9f7) introduces a uniform `cross_repo_refs: Option<CrossRepoRefs>` response field for tools whose `u64` projection drops cross-repo nodes. The contract covers 4 tools: `ready` (§7.1), `prime` (§7.3), `dep_cycles` (§7.7), `close` (§8.2).

| Tool | Status in code | Retro-fit needed |
|---|---|---|
| `ready` | Implemented (GAP-08 satisfied via prior work) | YES — add `cross_repo_refs` to `ReadyResult` |
| `prime` | Implemented | YES — add `## Cross-repo references` markdown section |
| `close` | Implemented (GAP-08 satisfied via prior work) | YES — add `cross_repo_refs` to `CloseResult` |
| `dep_cycles` | NOT yet implemented (unblock-29p.11, blocked by unblock-9f7) | NO retro-fit — spec is authoritative on first implementation |

**Type:** DRIFT (for the 3 retro-fit tools) / READY (for `dep_cycles`)
**Impact:** Medium — API shape of 3 implemented tools diverges from SPEC §11.4 until retro-fitted.

**Retro-migration sequencing:**

1. **This task (unblock-9f7) — spec only.** No crate edits. Both spec and plan land first. Unblocks unblock-29p.11.
2. **unblock-29p.11 (next).** Implements `dep_cycles` against the new §11.4 contract directly — NOT a retro-fit, first implementation. No behavioural regression for downstream agents because the tool is new.
3. **Retro-fit follow-ups (one bead per tool, tracked by orchestrator).** Each follow-up:
   - Adds `cross_repo_refs: Option<CrossRepoRefs>` to the response struct with `#[serde(skip_serializing_if = "Option::is_none")]`.
   - Wires population logic inside the tool handler.
   - Adds the integration test branch (`Some` and `None`) required by the updated Task 04.05 / 04.07 / 04.13 acceptance criteria.
   - Because `Option` + `skip_serializing_if` is additive and absent-from-JSON-on-None, this is a non-breaking change for existing MCP clients that did not inspect the new field. (API: note required per CLAUDE.md.)
4. **Error-side siblings (unblock-29p.25, unblock-6xj) — parallel track.** Cross-referenced in §11.1 and §11.4. Safe to implement before, during, or after this task's retro-fits — the two halves of the cross-repo contract (response-side §11.4, error-side §11.1) are independent. See GAP-14.c for the error-side contract closure (unblock-eos Decisions 1 + 2, arbitrated 2026-04-17).

**Resolution:** Land spec+plan here (unblock-9f7). Create one follow-up bead per retro-fit target (`ready`, `prime`, `close`). Implementation of each retro-fit is sibling-scope to this task, NOT part of it.

---

### GAP-14.b — Ready-set configured-repo source scoping (unblock-eos.4, Direction 1)

SPEC §3.3 Filter 3, §7.1 source-scoping guarantee, §11.4 `ready` row, and §14 Invariant 14(a) formalise a previously-unenforced scoping rule: `compute_ready_set` MUST return only issues whose `qualified_id.(owner, repo)` equals the configured repo. The existing implementation (`graph.rs:144-217`) filters on `IssueState`, `Status`, and blocker closure, but NOT on source repository — so a cross-repo source issue fed to `compute_ready_set` currently appears in `ready_set`. Direction 1 (user-chosen from the four options in the unblock-eos.4 investigation) fixes this at the graph engine itself by taking `(configured_owner, configured_repo)` as new required parameters. See D6 addendum below for the accepted direction.

**Why at the graph engine (Direction 1) and not the tool layer:** The cached `ready_set` in `GraphCache::CacheEntry` is consumed by `ready` (§7.1) AND `prime` (§7.3, code reference: `prime.rs:1496`). A tool-layer scrub would duplicate the filter across consumers and make `update_status_fields` (§10) race-prone on cross-repo sources. One chokepoint, one guarantee — §14 Invariant 14(a).

**Breaking change discipline (CLAUDE.md → Pub API Change Tracking):**

`compute_ready_set` is `pub` on `DependencyGraph` in `unblock-core` (library crate). Signature change is INCOMPATIBLE — callers MUST pass `(configured_owner, configured_repo)`. The implementing commit's message footer MUST include:

```
BREAKING CHANGE: DependencyGraph::compute_ready_set now requires
(configured_owner, configured_repo) parameters. Callers must pass the
configured repository coordinates so the graph engine can enforce
SPEC §14 Invariant 14(a). Cross-repo source issues are now filtered
out of the ready set at the engine level (previously: tool-layer
responsibility that was not honoured). See unblock-eos.4 / SPEC §3.3.
```

An `API:` body line is INSUFFICIENT for this change — Conventional Commits requires `BREAKING CHANGE:` for incompatible pub changes.

**Migration note (consumers):**

1. `crates/unblock-mcp/src/tools/mod.rs::rebuild_cache` — call site must pass `state.github.owner()` and `state.github.repo()`.
2. `crates/unblock-mcp/src/tools/prime.rs:1496` — uses cached `ready_set`; no call-site change, but the test fixture at `prime.rs:1488-1490` DOES call `compute_ready_set` directly and must pass `"test"`-like coordinates matching the test issues.
3. `crates/unblock-core/src/graph.rs` existing tests — `graph.rs:1399-1414` (`cross_repo_ready_set_with_mixed_repos`) and any other call-sites in the test module MUST be updated. The current `cross_repo_ready_set_with_mixed_repos` test explicitly asserts cross-repo sources are admitted (`ready[0].qualified_id == qid_repo("acme", "gadgets", 1)`); the expected behaviour post-eos.4 is to configure the engine with `"acme"/"widgets"` and assert the opposite — the cross-repo source is excluded, and the local source is filtered only by its open blocker (not by repo). This test is the concrete regression target.
4. `crates/unblock-mcp/tests/integration.rs:896-977` — any integration tests asserting on ready set composition in the presence of cross-repo issues must be updated to the new invariant.

**Retro-fit interaction with other eos siblings:**

- **unblock-eos.1** — touches the same file (`crates/unblock-mcp/src/tools/ready.rs`). Must merge first OR this bead rebases on top. Add a hard `depends` edge from this bead to unblock-eos.1 IF the eos.1 work is not yet landed by the time this bead starts; otherwise document the rebase in the bead description.
- **unblock-fah** — prime consumes cached `ready_set` (`prime.rs:1496`). After this bead lands, the cached set is guaranteed local-only, so any "cross-repo source leaked into prime categorisation" defensive paths in unblock-fah become unreachable by construction. unblock-fah does NOT need to rerun; its test surface only needs to assert the invariant is now upstream-guaranteed.
- **unblock-iov** — sibling retro-fit close-out for the same §14 Invariant 14(a) work. Must coordinate closing order: unblock-iov closes AFTER this bead's implementation is merged AND property tests at §13.3 #7 are green.

**Resolution:** Implement via new bead(s) child of `unblock-eos`. Single commit family: (1) graph-engine signature change + Filter 3 + property test, (2) all call-site updates, (3) test fixture updates for `cross_repo_ready_set_with_mixed_repos` and integration tests. Each commit compiles; the BREAKING CHANGE footer rides on the first commit that lands the signature change.

---

### GAP-14.c — Error-side cross-repo contract closure (unblock-eos, Decisions 1 + 2)

SPEC §11.1 (error-side half of the cross-repo contract) and SPEC §11.4 (response-side half) together form the two halves of the cross-repo contract. The response-side retro-fits are tracked by GAP-14; the ready-set source scoping is tracked by GAP-14.b. This entry closes the remaining error-side work under the unblock-eos sub-epic arbitration of 2026-04-17 (Decisions 1–4).

**Decision 1 — Uniform `IssueRef` typing for cross-repo-aware domain errors (SPEC §11.1).** The variants `IssueBlocked`, `CircularDependency`, and `DuplicateDependency` change field types from bare `u64` to `IssueRef` (§2.7). Rationale is embedded in SPEC §11.1 "Cross-repo-aware variant typing — Exhaustiveness Rationale"; do not re-open. Implementation lives in unblock-29p.25 (Task 02.02 above). BREAKING CHANGE footer is MANDATORY per CLAUDE.md Pub API Change Tracking; the exact footer body is spec'd verbatim in Task 02.02. Display byte-for-byte preservation for `IssueRef::Local(n) → "#n"` is required so `crates/unblock-core/src/errors.rs:215-240` existing tests pass without edits.

**Decision 2 — No shape change for `InvalidIssueRef` / `CrossRepoAccessDenied` (SPEC §11.1).** Both variants are already spec'd; the remaining work is wiring only, landed by unblock-6xj:

1. `IssueRef::from_str` failures at tool-boundary call sites (§7.2 `show`, §8.3 `create.blocked_by`, §8.4 `depends`, §8.5 `dep_remove`) MUST propagate as `InvalidIssueRefSnafu { input: <raw string> }` → HTTP 400 → MCP `-32602`.
2. GraphQL `FORBIDDEN` (or 403) on cross-repo fetch MUST propagate as `CrossRepoAccessDeniedSnafu { owner, repo }` → HTTP 403 → MCP `-32602`.
3. `crates/unblock-mcp/src/errors.rs` `github_error_to_mcp` match-arm MUST have an explicit 403 → `-32602` branch per Task 02.02 "Error-side wiring" (single source of truth; the unit-test requirement for the new branch is hoisted into Task 02.02 to avoid drift).

**Decision 3 — Response-shape universal pattern is documentation-only.** §11.4 affected set is frozen at `ready`, `prime`, `dep_cycles`, `close`; exhaustiveness rationale is embedded inline in SPEC §11.4 (see "Exhaustiveness Rationale — response-shape universality"). No new beads. New tools re-derive the (a)+(b) conjunction from §5.6 + their own response typing when added.

**Decision 4 — No further decomposition.** The unblock-eos sub-epic closes with these spec+plan patches. Only two implementation beads consume them: unblock-6xj (error-side wiring) and unblock-29p.25 (Task 02.02 `IssueRef`-typed variants + BREAKING CHANGE commit). No new sub-beads for edge cases; fold anything discovered during implementation into inline decision memos within this GAP or the affected task, not into a new bead.

**Parallel-track guarantees:**

- GAP-14.c is independent of GAP-14 (response-side) and GAP-14.b (ready-set scoping) — any ordering is safe.
- GAP-14.c lands independently; GAP-14.b has already shipped via `unblock-eos.5` (closed). Future cross-repo BREAKING CHANGEs re-enter §11.1 via an explicit spec PR.

**Resolution:** Implementation is split across unblock-6xj (wiring for `InvalidIssueRef` / `CrossRepoAccessDenied` / MCP error mapping) and unblock-29p.25 (Task 02.02 variant shape change + BREAKING CHANGE footer + tests). No additional beads required.

---

### GAP-15 — `close` cascade PRE vs POST ordering divergence (unblock-29p.61)

**Spec source of truth:** SPEC §8.2 step 2 and §3.4 "Critical" both mandate PRE-close cascade computation. SPEC §8.2's "Pre-close cascade MUST be captured before the mutation" paragraph (added by unblock-29p.61) tightens the prescription with a normative MUST that forbids the POST-close topology.

**Implementation state (pre-fix):** `crates/unblock-mcp/src/server.rs` lines ~1040-1266 implement the close handler as a two-phase flow — Phase 1 runs `execute_write_tool` (closes the issue + rebuilds the cache), Phase 2 reads `state.cache.get_graph()` and computes the cascade from the *post-close* rebuilt graph. This is the POST-close topology the spec forbids.

**Why the impl currently "passes tests":** mock integration fixtures (e.g. `crates/unblock-mcp/tests/integration.rs:4941-4946` — explicit inline comment acknowledging the mismatch) retain the just-closed issue in the rebuilt graph, which diverges from production where `FETCH_GRAPH_DATA_QUERY` (`unblock-github/src/graphql.rs:129`) uses `states: OPEN` and excludes it. In real production every call to `close` currently silently returns `unblocked = []` regardless of actual downstream dependents — the cascade feature is effectively dead.

**Relationship to unblock-29p.60:** The silent `Vec::new()` default in `compute_unblock_cascade` (`unblock-core/src/graph.rs:289-291`) is the *mechanism* by which the POST-close topology fails silently. Switching to PRE-close (this GAP) makes that branch unreachable from the close path because the closed issue is still in `node_map` at cascade-computation time. GAP-15 therefore **subsumes** unblock-29p.60 — the silent-default branch remains as defensive code (it is still the correct shape when `closed_id` legitimately isn't in the graph, e.g. a missing `create`-then-immediately-`close` race), but its triggering path in the close handler disappears. unblock-29p.60 should be closed as superseded once GAP-15 implementation lands.

**Remediation plan (implementation bead to be dispatched by the orchestrator — out of scope for this plan patch):**

1. **Refactor `close` handler in `crates/unblock-mcp/src/server.rs`** so the cascade is computed BEFORE the close mutation and BEFORE cache invalidation:
   - Phase 0 (NEW, PRE-close): ensure the cache is primed (if cold, issue one `fetch_graph_data` — reuse `rebuild_cache` helper in `tools/mod.rs:162`). Read the graph, build `issue_qid`, call `graph.compute_unblock_cascade(&issue_qid, &[])`, and capture the cascade `Vec<QualifiedId>` into a handler-local binding.
   - Phase 1 (MUTATION): invoke `execute_write_tool` to run `close_issue` + the existing Projects V2 `Status → closed` ladder on the closed issue + cache rebuild. Unchanged from today except the cascade is NO LONGER read back from the post-rebuild cache.
   - Phase 2 (CASCADE FIELD-UPDATE LOOP): iterate over the cascade list *captured in Phase 0* (not re-read from the rebuilt cache). For each cascaded dependent dispatch via the `_ref` primitives (`add_comment_ref`, `fetch_issue_ref`, `update_field`) exactly as today (SPEC §8.2 step 6 / §5.6 `close` row / §11.4 row 4 — all unchanged).
   - Phase 3 (RESPONSE PROJECTION): partition the Phase 0 cascade into `unblocked: Vec<u64>` + `cross_repo_refs: Option<CrossRepoRefs>` using the existing `crate::tools::cross_repo::project_cascade` + `build_cross_repo_refs_with_summary` helpers. Unchanged from today.
2. **Repurpose the existing R3 503-class error posture** (`tools/close.rs:17-36` module doc + `server.rs:1116-1130` let-Some/else). Under PRE-close ordering the cascade list is captured before the mutation, so a post-close rebuild failure no longer invalidates the cascade *list* — the error now signals only that step 8 (`update_status_fields` reconciliation) could not run. Update the error message to reflect the refocused semantics ("close succeeded, cascade field-updates applied best-effort, but Status reconciliation could not complete — re-run `show` to confirm final Status fan-out"). Integration test `close_surfaces_error_when_rebuilt_cache_missing_closed_issue` must be updated or renamed to `close_surfaces_error_when_rebuild_fails_after_pre_cascade` and its assertions adjusted accordingly.
3. **Update integration tests**:
   - Remove the inline acknowledgement comment at `integration.rs:4941-4946` (the mock fixture no longer needs to "cheat" by keeping #8 in the rebuilt graph — under PRE-close ordering the cascade is captured from the cache BEFORE the close mutation, so the post-close rebuild fixture can faithfully emit `states: OPEN` semantics, excluding #8).
   - Add a dedicated production-realism test `close_cascade_survives_open_only_rebuild` that uses a rebuilt-graph fixture *without* #8 and asserts that `unblocked` still contains the pre-close dependents — locks the PRE-close contract against regression.
   - Existing 4 happy-path tests (`close_no_cross_repo_dependents_cross_repo_refs_is_none`, `close_cross_repo_dependent_populates_cross_repo_refs`, `close_single_cross_repo_dependent_uses_singular_summary`, `close_cross_repo_add_comment_ref_failure_warns_and_continues_cascade`) MUST be migrated to production-realistic fixtures (post-close rebuild fixture excludes the closed issue, matching `states: OPEN` semantics). Keeping the cheat-topology "closed issue still in rebuilt graph" shape in these tests is forbidden — it is precisely the asymmetry GAP-15 exists to eliminate, and allowing it to survive in any test leaves an open regression window for a future refactor to silently re-introduce POST-close lookup.
4. **Close unblock-29p.60 as superseded** once the implementation lands and the quality gate is green. The silent `Vec::new()` default at graph.rs:289-291 is retained as defensive code — it is the correct behaviour for a missing `closed_id`; only the code path that triggered it in production (POST-close from OPEN-only rebuild) is removed.

**Type:** DRIFT (spec-vs-impl)
**Impact:** High — P1 production correctness bug. Every close in real production silently reports `unblocked = []` regardless of actual dependents; the cascade feature is dead.
**Concurrency/race semantics note:** PRE-close introduces no new concurrency races vs POST-close. The only window is between Phase 0 (cascade computation) and Phase 1 (close mutation); a concurrent blocker-close or edge-add in that window would be missed in either ordering (POST also fetches a stale graph, just one invalidation tick later). Soundness is strictly better under PRE — the closed issue is guaranteed present in the graph used to compute the cascade, which is not the case POST.
**Implementation bead:** to be created by the orchestrator after this plan patch merges. Scope: `crates/unblock-mcp/src/server.rs` close handler + `crates/unblock-mcp/tests/integration.rs` cascade tests + close unblock-29p.60 as superseded.

---

### Summary — Decisions (Resolved)

| # | Decision | Resolution |
|---|---|---|
| D1 | `Status` enum values | **Code changes to SPEC.** `Status::Open` → `Status::Ready` |
| D2 | `ReadyState` field | **Remove.** Unify into single `Status` field per SPEC |
| D3 | Projects V2 fields | **Code changes to SPEC.** Add `Pipeline Stage` + `Claimed At`, remove `IssueType` + `ReadyState` |
| D4 | Phase 02 early features | **Keep code, exclude from F1 acceptance criteria** |
| D5 | `FieldValue::Date` | **Keep `NaiveDate`, update SPEC** (improvement, not drift) |
| D6 | Cross-repo response shape | **Uniform `cross_repo_refs: Option<CrossRepoRefs>` per SPEC §11.4.** Retro-fit `ready`, `prime`, `close` via follow-up beads; `dep_cycles` lands with contract from day one. |
| D6.a | Ready-set source scoping (unblock-eos.4) | **Direction 1 — enforce `(configured_owner, configured_repo)` filter inside `compute_ready_set` at the graph engine.** `pub fn DependencyGraph::compute_ready_set` gains two new required parameters and the function drops cross-repo source issues BEFORE the blocker filter. Direction chosen over (2) tool-layer scrub, (3) fail-fast panic, and (4) soft-warn log. Single chokepoint → every downstream consumer (cached `ready_set`, `ready`, `prime`, `update_status_fields`) inherits §14 Invariant 14(a) for free. BREAKING CHANGE on `unblock-core` pub API — Conventional Commits footer MANDATORY. See GAP-14.b for full migration. |
| D6.b | Error-side cross-repo variant typing (unblock-eos Decision 1) | **Uniform `IssueRef` typing for cross-repo-aware variants.** `DomainError::{IssueBlocked, CircularDependency, DuplicateDependency}` field types change from bare `u64` to `IssueRef` (§2.7). Display preserved byte-for-byte for `IssueRef::Local(n) → "#n"` so existing tests at `crates/unblock-core/src/errors.rs:215-240` pass unchanged; CrossRepo renders as `"owner/repo#number"`. BREAKING CHANGE on `unblock-core` pub API — Conventional Commits footer MANDATORY on unblock-29p.25 commit. See GAP-14.c and Task 02.02 for the full footer template. |
| D6.c | Error-side wiring for `InvalidIssueRef` / `CrossRepoAccessDenied` (unblock-eos Decision 2) | **No shape change, wiring only.** `IssueRef::from_str` failures at tool boundary → `InvalidIssueRefSnafu` → 400 → MCP `-32602`. GraphQL `FORBIDDEN` on cross-repo fetch → `CrossRepoAccessDeniedSnafu` → 403 → MCP `-32602` (explicit match arm in `crates/unblock-mcp/src/errors.rs`). Implementation: unblock-6xj. See GAP-14.c and Task 02.02 for the wiring contract. |
| D6.d | Response-shape universality (unblock-eos Decision 3) | **Documentation-only; frozen.** SPEC §11.4 "Exhaustiveness Rationale" derives the affected set (`ready`, `prime`, `dep_cycles`, `close`) mechanically from §5.6 + response typing. New tools re-derive (a)+(b) in their own spec entries. No new beads; no re-opening. |
| D6.e | Meta-process (unblock-eos Decision 4) | **No further decomposition.** unblock-eos sub-epic closes with these three spec+plan patches. Only unblock-6xj and unblock-29p.25 remain as implementation beads under this sub-epic. Edge cases discovered during implementation fold into inline decision memos within GAP-14.c / Task 02.02, NOT into new beads. |
| D7 | `close` cascade PRE vs POST ordering (unblock-29p.61) | **Option (a) — PRE-close cascade.** Refactor impl to match the existing SPEC §8.2 step 2 + §3.4 "Critical" prescription rather than rewriting spec to match the broken impl. PRE-close is sound by construction (closed issue still present in `node_map` at cascade computation); POST-close is unsound by construction (`FETCH_GRAPH_DATA_QUERY` is `states: OPEN`-only, closed issue absent from rebuilt `node_map`, silent `Vec::new()` short-circuit at `graph.rs:289-291` fires on every production close). Subsumes unblock-29p.60 — the silent-default branch remains as defensive code but becomes unreachable from the close path. Rejected option (b) because it would codify a production-broken impl and still require fixing .60 independently (via `fetch_graph_data` closed-issue extension or a second query round-trip). Rejected option (c) because a double-traversal is unnecessary — PRE alone yields correct cascade and the Phase 3 field-update loop already applies to the cascade targets without needing a second POST pass. See GAP-15 for the full remediation plan. |

---

## Definition of Done

Phase 01 is complete when:

1. **All 17 tools registered and functional** — `server_lists_all_17_tools` test passes
2. **Quality gate green** — `cargo fmt`, `cargo clippy`, `cargo test`, `cargo doc` all pass
3. **E2E workflow** — Full agent loop: `prime` → `ready` → `claim` → `close` → cascade verified
4. **Integration tests** — Each tool has at least one test with `MockGitHubClient`
5. **Data model aligned** — All decision points (D1–D6) resolved, implementation matches plan
6. **7 Projects V2 fields** — Created by `setup`, used by all write tools
7. **5 Views** — Created by `setup`
8. **Performance** — `prime` → `ready` → `claim` in under 2 seconds on warm cache
9. **Zero data loss** — If `unblock-mcp` process dies, all state is in GitHub
10. **Coverage** — >80% for all 3 crates
11. **Cross-repo contract (SPEC §11.1, §11.4, §14 Invariant 14)** — All three clauses MUST be green:
    - **11(a) — Invariant 14(a) — ready-set source scoping (unblock-eos.4 / D6.a / GAP-14.b).** `DependencyGraph::compute_ready_set` takes `(configured_owner, configured_repo)` and filters cross-repo source issues at §3.3 Filter 3. A property test (§13.3 #7) asserts mixed-repo input → only configured-repo sources in the output. The BREAKING CHANGE footer per CLAUDE.md Pub API discipline has landed on the commit that changed the signature.
    - **11(b) — Invariant 14(b) — response-shape contract (GAP-14 / D6).** `ready`, `prime`, `dep_cycles`, `close` honor the `cross_repo_refs` contract. GAP-14 retro-fits landed. Integration tests cover `Some`/`None` branches for each.
    - **11(c) — error-side contract (GAP-14.c / unblock-eos Decisions 1 + 2).** `DomainError::{IssueBlocked, CircularDependency, DuplicateDependency}` carry `IssueRef` (unblock-29p.25, Task 02.02) with Display byte-for-byte preservation for `IssueRef::Local`; `InvalidIssueRef` and `CrossRepoAccessDenied` are wired at tool-boundary parse failures and GraphQL `FORBIDDEN` respectively (unblock-6xj); `crates/unblock-mcp/src/errors.rs` explicitly maps 403 → `-32602`. BREAKING CHANGE footer per CLAUDE.md has landed on the unblock-29p.25 commit.
