# Unblock — Project Plan

**Dependency-aware task tracking for AI agents, powered by GitHub.**

| | |
|---|---|
| **Version** | 1.2.0-draft |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Repo** | `websublime/unblock` |
| **Date** | March 2026 |
| **Status** | Draft |

---

## Reference Documents

| ID | Document | Ficheiro | Short ref |
|---|---|---|---|
| **PRD** | Product Requirements Document | `unblock-prd-github.md` | PRD §N |
| **ARCH** | MCP Architecture Specification | `unblock-architecture-github.md` | ARCH §N |
| **DPP** | Desktop Project Plan | `unblock-desktop-project-plan.md` | DPP §N |

> The desktop application has its own project plan. See DPP.

---

## Quality Gate

### Phase 1-2 Quality Gate

No task is done unless ALL of these pass:

| Standard | Requirement |
|---|---|
| **Lint** | `cargo clippy --all-targets -- -D warnings` — zero warnings |
| **Format** | `cargo fmt --check` — zero diffs |
| **Tests** | >80% public API coverage. Unit tests for pure logic, integration tests for I/O. Property tests for graph invariants |
| **Documentation** | `pub fn` and `pub struct` have `///` doc comments. `cargo doc --no-deps` builds clean |
| **Idiomatic Rust** | Edition 2024. `snafu` for errors. No `unwrap()` in production code. `tracing` for logging |
| **Unsafe** | `#![deny(unsafe_code)]` workspace-wide |
| **CI green** | All checks pass before merge |
| **Idempotency** | `init` and `setup` safe to re-run — verified in tests |
| **Cross-repo** | `IssueRef::parse()` and `QualifiedId` property-tested with proptest. Graph invariant: qualified IDs unique across repos |
| **REST API** | View/field endpoints tested with `X-GitHub-Api-Version: 2026-03-10` header |

### Phase 3 Quality Gate (cumulative)

| Standard | Requirement |
|---|---|
| **Tests** | 100% public API coverage. Property tests with proptest. Fuzz testing for parser |
| **Documentation** | Every `pub` item has `///` doc. Module-level `//!` docs. `cargo doc --no-deps` zero warnings |
| **Lint** | `cargo clippy --all-targets -- -D warnings -W clippy::pedantic` — zero warnings |
| **All Phase 1-2 standards** | Still apply |

---

## Phase Overview

| Phase | Version | Goal | Epics | Effort |
|---|---|---|---|---|
| **1 — Foundation** | v0.1.0 | Agent can find, claim, edit, complete work + see cascade. Client detection + data integrity | 6 | ~24.25 days |
| **2 — Complete** | v0.2.0 | Full tool suite + Claude Code plugin | 3 | ~7 days |
| **3 — Production** | v1.0.0 | Hardened, distributed, production-ready + v1.0.0 gap features | 5 | ~11 days |
| **Total** | | | **14 epics** | **~42.25 days focused** |

---

## Phase 1 — Foundation (v0.1.0)

**Goal:** An agent can find work, claim it, edit it, complete it, and see the cascade. The
system detects which AI client is connected, repairs data drift from external mutations, and
provides a session entry point (`prime`) with full context.

**Ref:** PRD §10 Phase 1, ARCH §1-§10

---

### Epic 1.1 — Workspace and Infrastructure

**Goal:** Repo structure, CI pipeline, workspace config. Nothing compiles yet but the skeleton is solid.

| Task | Description | DoD | Ref |
|---|---|---|---|
| **1.1.1** Cargo workspace | Create `websublime/unblock` repo. `Cargo.toml` workspace with `crates/unblock-core`, `crates/unblock-github`, and `crates/unblock-mcp`. Workspace-level `[lints]`, `[dependencies]`, edition 2024 | `cargo check --workspace` passes. `clippy` + `fmt` clean. README.md with project description, license MIT OR Apache-2.0 | ARCH §17.1 |
| **1.1.2** CI pipeline | GitHub Actions: fmt check, clippy, test, tarpaulin coverage on ubuntu + macos. Branch protection on `main` requiring CI pass | Push to `main` triggers CI. Badge in README shows status | ARCH §17.3 |
| **1.1.3** Core crate skeleton | `unblock-core/src/lib.rs` with module declarations: `types`, `graph`, `cache`, `config`, `errors`. Empty modules with `//!` module docs | `cargo doc --no-deps -p unblock-core` builds clean | ARCH §4.1 |
| **1.1.4** GitHub crate skeleton | `unblock-github/src/lib.rs` with module declarations: `client`, `graphql`, `mutations`, `projects`, `errors`. Empty modules with `//!` module docs | `cargo doc --no-deps -p unblock-github` builds clean | ARCH §4.2 |
| **1.1.5** MCP crate skeleton | `unblock-mcp/src/main.rs` with module declarations: `server`, `errors`, `tools/`. Depends on `unblock-core` and `unblock-github`. Empty modules with `//!` module docs | `cargo check -p unblock-mcp` passes | ARCH §4.2 |
| **1.1.6** CLAUDE.md + .claude/ | `CLAUDE.md` at repo root with workflow instructions, tool descriptions, coding standards. `.claude/settings.json` with lint/test commands | Agent can read CLAUDE.md and understand the project workflow | PRD §8 |

---

### Epic 1.2 — Core Library (unblock-core)

**Goal:** Domain types, graph engine, cache. Fully testable without network. This is the brain of the system.

| Task | Description | DoD | Ref |
|---|---|---|---|
| **1.2.1** Domain types | Implement `Issue`, `IssueState`, `Status`, `Priority`, `ReadyState`, `IssueType`, `BlockingEdge`, `IssueSummary`, `BodySections`, `IssueRef (Local/CrossRepo)`, `QualifiedId (owner/repo#number — graph node key)` with all derives. `Priority::as_sort_key()`. `BodySections::from_markdown()` + `to_markdown()` roundtrip | Types compile. Roundtrip test: `parse(render(x)) == x`. Property test with proptest. 100% coverage on `BodySections`. `QualifiedId` roundtrip: `parse(display(x)) == x`. Property test with proptest | ARCH §5 |
| **1.2.2** Domain errors | Implement `DomainError` enum with `snafu`: `IssueNotFound`, `AlreadyClaimed`, `IssueBlocked`, `IssueDeferred`, `IssueClosed`, `CircularDependency`, `DuplicateDependency`, `Validation`. `status_code()` method | Each variant maps to correct HTTP status code. Unit test for every variant | ARCH §13.1 |
| **1.2.3** Configuration | Implement `Config::load()` reading env vars: `GITHUB_TOKEN` (required), `UNBLOCK_REPO`, `UNBLOCK_PROJECT`, `UNBLOCK_AGENT`, `UNBLOCK_CACHE_TTL`, `UNBLOCK_LOG_LEVEL`, `UNBLOCK_OTEL_ENDPOINT` | Missing `GITHUB_TOKEN` returns validation error. Unit tests for defaults, overrides, parsing | ARCH §12.1, PRD §7.7 |
| **1.2.4** Graph — build + ready set | `DependencyGraph::build(issues, edges)` with petgraph DiGraph using `QualifiedId` as node key. `compute_ready_set()` returns `Vec<QualifiedId>` — open, not closed, no active blockers. Handle missing nodes gracefully (warn + skip) | Unit test: blocked not in ready set. Open with no deps in ready set. Closed excluded. Property test: ready set never contains blocked issues (proptest, 1..100 issues, 0..200 edges). Cross-repo: two issues with same number but different repos are distinct nodes | ARCH §6.1, §6.2 |
| **1.2.5** Graph — cascade | `compute_unblock_cascade(closed: &QualifiedId)` — issues unblocked when one closes. Only issues whose ALL blockers are now closed | Unit test: A blocks B+C. Close A → B+C unblocked. A+D block E. Close A → E NOT unblocked. Close D → E unblocked | ARCH §6.3 |
| **1.2.6** Graph — cycles | `would_create_cycle(source: &QualifiedId, target: &QualifiedId)` via `has_path_connecting`. `detect_all_cycles()` via `tarjan_scc`. `dependency_tree(root, direction, max_depth)` via BFS | `would_create_cycle` catches A→B + B→A. `detect_all_cycles` finds SCCs > 1. `dependency_tree` respects max_depth. Property test: cycle detection consistent | ARCH §6.4 |
| **1.2.7** Cache layer | `GraphCache` with `RwLock<Option<CacheEntry>>`. `CacheResult` enum: `Fresh` (within TTL), `Stale` (TTL expired, entry preserved), `Empty` (cold start or post-write invalidation). `invalidate()` clears entry to `None` — post-write data is objectively wrong, error beats stale. Public API: `get()`, `get_ready_set()`, `get_graph()`, `update(graph)`, `invalidate()`, `is_fresh()`. Configurable TTL | Fresh cache returns data. TTL-expired returns `Stale`. After `invalidate()` returns `Empty`. `update()` stores graph and resets TTL. Concurrent access doesn't panic | ARCH §7.1 |

---

### Epic 1.3 — GitHub API Layer (unblock-github)

**Goal:** Connect to GitHub, fetch data, write mutations. The bridge between GitHub and the graph engine. Lives in `crates/unblock-github` as a shared crate reusable by both MCP server and future desktop app.

| Task | Description | DoD | Ref |
|---|---|---|---|
| **1.3.1** GitHub client bootstrap | `GitHubClient::new(config)`: reqwest + auth, `resolve_repo()` from env or git remote, `resolve_project()` from linked Projects V2 | Auto-detect from git directory. `UNBLOCK_REPO` override. Unit test for `parse_github_url()` (HTTPS + SSH). Integration test: connects to real repo | ARCH §8.1 |
| **1.3.2** fetch_graph_data | `fetch_graph_data` — Paginated GraphQL query: open issues + blockedBy/blocking + Project fieldValues → `(Vec<Issue>, Vec<BlockingEdge>)`. Returns all open issues with resolved project field values. Handles pagination for repos with >100 issues | Integration test: fetches real issues. Pagination works. All Projects V2 fields mapped. Missing fields handled | ARCH §8.2 |
| **1.3.3** fetch_issue | Single issue GraphQL query with comments, blockedBy, blocking, parent, subIssues, all fields | Integration test: existing issue with full details. `IssueNotFound` for non-existent. Comments parsed | ARCH §8.2 |
| **1.3.4** Mutations — create + close + comment | `create_issue()` (REST POST), `close_issue()` (REST PATCH), `add_comment()` (REST POST) | Integration test: create → verify exists → close → verify closed. Comment appears | ARCH §8.3 |
| **1.3.5** Mutations — blocking | `add_blocked_by()`, `remove_blocked_by()` (GraphQL). `add_sub_issue()` (GraphQL with sub_issues feature header). Cross-repo: `IssueRef` type supports `owner/repo#number`. `resolve_issue_ref()` resolves from any repo | Integration test: add blocking, verify in fetch. Remove, verify removed. Sub-issue link works. Cross-repo blocking works between different repos | ARCH §8.3 |
| **1.3.6** Projects V2 fields | `resolve_project()`, `setup_fields()` (7 fields, idempotent), `update_field()` with `FieldValue` enum. `ProjectFieldIds` cached | Integration test: setup creates fields. Rerun skips existing. `update_field` changes value, re-fetch confirms | ARCH §8.4 |
| **1.3.7** Infrastructure errors | `Error` enum with snafu: `Domain`, `GitHubApi`, `GitHubGraphQL`, `GitHubUnavailable`, `RateLimited`, `CircuitBreakerOpen`, `ProjectNotConfigured`, `GitRemote`. `From<Error> for McpError` | Every variant tested for Display + status_code. MCP conversion works | ARCH §13.2 |
| **1.3.8** REST API client (views + fields) | REST client with `X-GitHub-Api-Version: 2026-03-10` header. `list_rest_fields()` returns all fields with integer IDs. `create_view()` creates a view with name/layout/filter/visible_fields. `list_views()` via GraphQL for idempotency check. Owner type detection (org vs user) for endpoint routing | Integration: list fields returns integer IDs for built-in + custom fields. Create view succeeds with board/table/roadmap layouts. List views returns created views. API version header sent correctly. Org and user paths both work | ARCH §8.5 |

---

### Epic 1.4 — MCP Server + Phase 1 Tools

**Goal:** MCP server boots, registers tools, agent uses core workflow loop.

| Task | Description | DoD | Ref |
|---|---|---|---|
| **1.4.1** Server bootstrap | `main.rs`: config → tracing (JSON stderr) → GitHubClient → GraphCache → `ServerState` → rmcp stdio. `ServerInfo` with name, version, instructions | Binary starts, logs connection, accepts MCP messages | ARCH §9.1, §9.2 |
| **1.4.2** Tool execution pattern | Shared helper: validate → execute → if write: invalidate + rebuild + update Ready State → return result | Helper used by all write tools. Unit test for rebuild flow | ARCH §9.3 |
| **1.4.3** `init` tool | `#[tool]` macro. Input: `InitParams { scope?, title?, description?, public? }`. Auto-detect org vs user. Create project via `createProjectV2` mutation. Idempotent — skip if project with same title exists | Integration: init creates project with custom title/description. Rerun returns existing (idempotent). Scope auto-detection works for org and user. Public flag respected. Output includes project URL + hint to run setup | ARCH §10.18 |
| **1.4.4** `setup` tool | `#[tool]` macro. Input: `SetupParams { project?, dry_run? }`. Creates 7 fields (idempotent). Detects owner type, queries existing views, discovers field IDs via REST, creates 5 pre-configured views via REST POST /views (://ready, ://team, ://pipeline, ://roadmap, ://timeline). Reports fields + views + manual config guidance. Depends on project existing (via `init` or pre-existing). If no project found, error with hint to run `init` | Integration: fields created. Rerun → no duplicates. Views created with correct layout/filter/visible_fields. Dry run reports only | PRD §6.4, ARCH §10.7 |
| **1.4.5** `ready` tool | Input: `ReadyParams { limit?, type?, priority?, milestone?, agent?, label?, include_claimed? }`. Cache → rebuild if stale → filter → exclude deferred → sort priority ASC + created ASC → top N | Integration: 3 issues (1 blocked, 1 ready, 1 deferred) → returns only ready. Cache hit on second call. Filters work | PRD §6.1, ARCH §10.1 |
| **1.4.6** `claim` tool | Input: `ClaimParams { id, agent? }`. Validate open + not blocked + not deferred. Fields: Status→in_progress, Agent, Claimed At, Ready State→not_ready. Comment. Rebuild | Integration: claim ready → fields updated. Claim blocked → error. Claim closed → error. Comment appears | PRD §6.1, ARCH §10.2 |
| **1.4.7** `close` tool | Input: `CloseParams { id, reason? }`. Close → fields → comment → rebuild → cascade → update unblocked fields + comments | Integration: A blocks B. Close A → B ready. Cascade comment on B. `unblocked` contains B. Already closed → error | PRD §6.1, ARCH §10.3 |
| **1.4.8** `create` tool | Input: `CreateParams { title, type?, priority?, body?, labels?, milestone?, blocked_by?, parent?, story_points?, defer_until? }`. Create → project → fields → deps → rebuild | Integration: create with all fields → verified. `blocked_by` creates relationship. `parent` creates sub-issue. No title → validation error | PRD §6.1, ARCH §10.4 |
| **1.4.9** `show` tool | Input: `ShowParams { id, include_comments?, include_deps? }`. Single query → parse body sections → return | Integration: show with comments and deps. Body sections parsed. Non-existent → `IssueNotFound` | PRD §6.3, ARCH §10.6 |
| **1.4.10** `depends` tool | Input accepts `IssueRef` for cross-repo support. `DependsParams { source, target }`. Cycle check → addBlockedBy → update source fields → rebuild | Integration: depends A B → A blocked by B. Cycle A→B→A → error. Duplicate → error. Cross-repo depends works with owner/repo#number format | PRD §6.2, ARCH §10.5 |
| **1.4.11** `comment` tool | Input: `CommentParams { id, body }`. POST comment | Integration: comment on issue → appears. Non-existent → error | PRD §6.2, ARCH §10.8 |
| **1.4.12** `update` tool | Input: `UpdateParams { id, priority?, status?, labels_add?, labels_remove?, body_section?, milestone?, story_points?, defer_until? }`. Validate exists. Update specified fields via REST + Project V2. Rebuild | Integration: update priority → re-fetch confirms. Update body section → section changed, rest preserved. Non-existent → error | PRD §6.1 |
| **1.4.13** E2E workflow test | Full loop: `init` → `setup` → `create` (3 issues + deps) → `ready` → `claim` → `update` → `comment` → `close` → cascade → `ready` (newly unblocked) | All 10 tools in sequence. Graph consistent throughout. Cleanup after | PRD §8 |
| **1.4.14** `prime` tool | Session context with smart prioritization: in_progress → blocked → ready → completed → hotspots → stale claims. Configurable max_tokens. Includes `SessionMeta` (populated by Epic 1.5) and `drift_warnings` (populated by Epic 1.6). **Promoted from Phase 2** — `prime` is the session entry point, required by both detection and reconciliation | Integration: coherent summary. Hotspots identified. Stale claims flagged. `session` field populated when detection enabled. `drift_warnings` populated when reconciliation enabled | PRD §6.3 |

---

### Epic 1.5 — Agent Client Detection

**Goal:** Identify which AI client is connected and surface this in logs, `prime` output, and
tracing spans — without affecting tool behaviour.

**Depends on:** Epic 1.4 (MCP tools foundation), task 1.4.14 (`prime` tool)

**Detail document:** `unblock-epic-agent-client-detection.md`

| Task | Description | DoD | Ref |
|---|---|---|---|
| **1.5.1** `AgentKind` + `AgentClient` types | In `unblock-core/src/client.rs` (standalone module). `AgentKind` enum with `from_client_name()`, `as_str()`, `Display`. `AgentClient { name, version }` | Unit tests: known names → correct variant. `Unknown(name)` for unrecognised. `cargo doc` clean | ARCH §5 |
| **1.5.2** `ClientDetector` | In `unblock-core/src/detection.rs`. `from_env()`: reads `CLAUDE_CODE_ENTRYPOINT`, `GITHUB_COPILOT_TOKEN`, `CURSOR_TRACE_ID`. `resolve()`: MCP → env → Unknown. `VSCODE_PID` intentionally excluded (D6) | Unit tests: each env var → correct kind. MCP overrides env. No panics | — |
| **1.5.3** MCP `initialize` capture | Override `initialize()` in `UnblockServer`. Resolve `AgentKind` once, store in `ServerState.agent_kind: OnceLock<AgentKind>`. Emit `tracing::info!` at connection. Delegate to rmcp default for peer storage | Integration: mock initialize → kind stored. Env fallback exercised. Tracing event present | ARCH §9 |
| **1.5.4** `SessionMeta` in `prime` | Add `SessionMeta { agent_client, agent_kind, agent_field, connected_at }` to `PrimeResult`. Read from `OnceLock` + `Peer<RoleServer>` | Integration: `prime` JSON includes `session` object with correct fields | PRD §6.3 |
| **1.5.5** `agent.kind` span fields | Add `agent.kind` field to tracing spans in all tool handlers. Read from `ServerState.agent_kind` (lock-free `OnceLock`) | JSON log includes `agent.kind` on all tool spans. Log-capture test | ARCH §14.1 |
| **1.5.6** Tests | Table-driven unit tests, env detection, fuzz `from_client_name` with proptest, integration `initialize → prime` flow | >80% coverage on `client` and `detection` modules | Quality Gate |

---

### Epic 1.6 — Reconciliation

**Goal:** Detect and repair semantic drift between the computed dependency graph and the state
stored in GitHub Projects V2 fields. Handles external mutations (human closes issue via UI,
removes blocking relationship, etc.).

**Depends on:** Epic 1.4 (graph engine + tools), task 1.4.14 (`prime` tool for background integration)

**Detail document:** `unblock-reconciliation-plan.md`

| Task | Description | DoD | Ref |
|---|---|---|---|
| **1.6.1** `DriftKind` + `DriftReport` types | In `unblock-core/src/reconcile.rs`. 7 drift types: `StaleReadyState`, `UncascadedClosure`, `OrphanedBlockingEdge`, `MalformedAgentField`, `MissingProjectField`, `CycleDetected`, `StaleClaim`. `DriftReport` with scan/repair/error counts | Types compile. `cargo doc` clean. All 7 variants constructible in tests | — |
| **1.6.2** `ReconcileEngine::analyse()` | Pure engine: receives graph + issues + computed ready set + now → returns `DriftReport`. No I/O | Unit tests: stale ready state detected, uncascaded closure detected, clean report when consistent, cycle detected. All with in-memory fixtures | — |
| **1.6.3** `reconcile` tool (read-only) | MCP tool handler with `ReconcileParams { fix: bool, stale_claim_hours: u64 }`. Fresh fetch (bypasses cache), rebuild graph, analyse, return report. `fix: false` by default | Integration: drift present → report shows it. No mutations when `fix: false` | — |
| **1.6.4** `reconcile --fix` repair logic | When `fix: true`: repair `StaleReadyState` and `UncascadedClosure` via field updates. Add audit comments. Cycles and stale claims: report only (human decision) | Integration: stale ready state repaired. Cascade repair adds comment. Cycle reported as error | — |
| **1.6.5** Integration into `prime` | Background `tokio::spawn` of read-only reconcile during `prime`. Include `drift_warnings` in output if drift found. Non-blocking — does not penalise `prime` latency | Integration: `prime` with drift → warnings in output. `prime` without drift → no warnings | — |
| **1.6.6** Graph helper methods | Add `all_edges() -> Vec<BlockingEdge>` and `edge_count() -> usize` to `DependencyGraph`. Trivial wrappers over petgraph | Unit tests for both methods | — |
| **1.6.7** Tests | Unit tests for all 7 drift types. Integration: reconcile on clean repo. Reconcile with injected drift + fix. Property test: reconcile on consistent graph always returns clean | >80% coverage on `reconcile` module | Quality Gate |

---

## Phase 2 — Complete (v0.2.0)

**Goal:** Full tool suite + Claude Code plugin. Feature-complete.

**Ref:** PRD §10 Phase 2

---

### Epic 2.1 — Remaining Tools

**Core (required for v0.2.0):**

| Task | Description | DoD | Ref |
|---|---|---|---|
| **2.1.1** `dep_remove` tool | Remove blocking → update Ready State → rebuild | Integration: create dep, remove it. Source unblocked | PRD §6.2 |
| **2.1.2** `reopen` tool | Reopen closed issue. Rebuild graph, evaluate blocking state. Add comment | Integration: close then reopen → status correct. Reopen with blockers → blocked | PRD §6.1 |

**Stretch (ship if time allows, otherwise v0.3.0):**

| Task | Description | DoD | Ref |
|---|---|---|---|
| **2.1.3** `list` tool | Flexible query with filters and sorting | Integration: filter by status, sort by created | PRD §6.3 |
| **2.1.4** `search` tool | GitHub ISSUE_ADVANCED search | Integration: keyword finds match | PRD §6.3 |
| **2.1.5** `stats` tool | Aggregates + agent metrics + bottlenecks + stale claims | Integration: known dataset, counts match | PRD §6.3 |
| **2.1.6** `dep_cycles` tool | Detect cycles, filter by id | Integration: cycle detected | PRD §6.2 |
| **2.1.7** `doctor` tool | System health check. Fields valid, issues in project, no orphans. Incorporates reconciliation findings from Epic 1.6 | Integration: missing field detected. Fix mode works. Drift detected | PRD §6.4 |

---

### Epic 2.2 — Claude Code Plugin

| Task | Description | DoD | Ref |
|---|---|---|---|
| **2.2.1** Plugin structure | `plugin/`: `.claude-plugin/plugin.json`, `.mcp.json`, `marketplace.json` | Files validate. Metadata correct | PRD §7.8, ARCH §11.1-§11.3 |
| **2.2.2** Slash commands | 14 command `.md` files with frontmatter + prompt instructions | Each triggers correct MCP tool | ARCH §11.4 |
| **2.2.3** Workflow skill | `plugin/skills/unblock-workflow.md`: session flow, rules, CLAUDE.md integration | Agent follows workflow when skill loaded | ARCH §11.5, PRD §8 |

---

### Epic 2.3 — Documentation

| Task | Description | DoD | Ref |
|---|---|---|---|
| **2.3.1** Getting started guide | `docs/getting-started.md`: install, GITHUB_TOKEN, setup, first loop | Zero to working in < 5 minutes | PRD §7.2 |
| **2.3.2** Agent workflow guide | `docs/agent-workflow.md`: plugin config, session flow, best practices | Operator can configure agents | PRD §8 |
| **2.3.3** API reference | `cargo doc` published. All public items with examples | Zero doc warnings. Published and linkable | Quality Gate |

---

## Phase 3 — Production (v1.0.0)

**Goal:** Hardened for real-world use.

**Ref:** PRD §10 Phase 3

---

### Epic 3.1 — Resilience

| Task | Description | DoD | Ref |
|---|---|---|---|
| **3.1.1** Circuit breaker | `CircuitBreaker`: Closed/Open/HalfOpen. 5 failures → Open. 10s cooldown → HalfOpen. Success → Closed | Unit: state transitions correct. Open returns error immediately | ARCH §15.1 |
| **3.1.2** Retry with backoff | `RetryPolicy`: exponential + jitter. 3 retries. 500ms base. 5s max. Only `RateLimited` + `GitHubUnavailable` | Unit: retryable retried 3x. Non-retryable fail immediately. Delays increase | ARCH §15.2 |
| **3.1.3** Stale cache serving | GitHub down → serve stale cache with `stale: true`. Writes still fail | Test: cache populated, GitHub fails, `ready` returns stale. `claim` errors | PRD §7.5 |

---

### Epic 3.2 — Persistent Cache

**Goal:** Survive MCP server restarts without a full GitHub rebuild. Computed subgraphs persist in the repository's own git object database (`refs/unblock/*`) — strictly local, never pushed, discardable at any time.

**Depends on:** Epic 3.1 (Resilience — circuit breaker and retry interact with cache rebuild). Task 3.2.0 spike must complete before any other 3.2 task begins.

**Scope boundary:** Stores computed graph data only. Never stores issue body content, comments, or mutations. `show` remains always-fresh regardless of this cache. Pending mutations are explicitly out of scope — they would violate P3 (fail safe for writes).

| Task | Description | DoD | Ref |
|---|---|---|---|
| **3.2.0** Build spike | Evaluate git2 (vendored libgit2), gix (pure Rust gitoxide), and subprocess git CLI against all 5 build targets. Focus on musl static linking (x86_64 + ARM64) and Windows. Produce: chosen library, confirmed CI matrix changes, benchmark of read/write latency per blob | All 5 targets compile and pass a blob round-trip test. Decision documented with rationale. Spike is a prerequisite blocker for all other 3.2 tasks and for 3.3 Distribution | Risk Register |
| **3.2.1** `VersionedBlob` + `GitObjectStore` | `VersionedBlob<T>` wrapper with `version: u32` field — written to every blob from day one. `GitObjectStore::write_versioned()` and `read_versioned()`. Unknown version or deserialization failure → cache miss (never panic, never error). `GitObjectStore::delete()` and `list(prefix)` | Round-trip test: write v1 blob, read back. Unknown version returns None. Corrupt blob returns None. All 5 targets pass | ARCH §7.3 |
| **3.2.2** `SegmentedGraphCache` | Per-repo subgraph blobs at `refs/unblock/graph/{owner}/{repo}`. Per-repo meta at `refs/unblock/meta/{owner}/{repo}` (TTL + cached_at). `cross-edges` blob at `refs/unblock/cross-edges`. `invalidate_repo(owner, repo)` deletes only that repo's blobs. `invalidate_all()` deletes all `refs/unblock/graph/*` blobs | Cold start reads from git objects if TTL valid (0 API calls). Segment invalidation only affects the written repo. Full invalidation removes all segments. Concurrent reads don't panic | ARCH §7.3 |
| **3.2.3** `UnifiedGraph` composition | `UnifiedGraph` composes `Arc`-wrapped segments + cross-edges in memory. `update_segment(repo, new_graph)` replaces only the changed segment (O(1) for unchanged segments). `get_or_compose()` lazy-composes on first access after invalidation | Replacing one segment does not copy other segments. Cross-edges applied correctly. Composed graph ready set matches full rebuild | ARCH §7.3 |
| **3.2.4** Tool integration | All write tools call `cache.invalidate_repo(owner, repo)` instead of full invalidation. `depends` / `dep_remove` with cross-repo refs call `cache.invalidate_repo()` for both repos + `cache.invalidate_cross_edges()`. Cold start path: check git objects → rebuild only stale segments in parallel | Integration: write to repo A → repo B cache unaffected. Cross-repo dep → both segments invalidated. Cold start with valid cache → 0 API calls | ARCH §7.2 |
| **3.2.5** Tests | Unit: `VersionedBlob` round-trip and version mismatch. Unit: `SegmentedGraphCache` invalidation scope. Integration: cold start with pre-populated git objects. Integration: segment invalidation after write. Property: any sequence of writes + reads produces consistent graph | >80% coverage on `cache/` module. All integration tests pass against real repo | Quality Gate |

---

### Epic 3.3 — Observability

| Task | Description | DoD | Ref |
|---|---|---|---|
| **3.3.1** Structured logging | `tracing` JSON stderr. Tool name, duration, result count, cache hit/miss. Token redacted | JSON parseable. Token never in output | ARCH §14.1, §14.2 |
| **3.3.2** OpenTelemetry metrics | Optional when `UNBLOCK_OTEL_ENDPOINT` set. Tool duration, GitHub duration, cache stats (hits/misses), graph stats | Metrics in collector when configured. No impact when absent | ARCH §14.3 |

---

### Epic 3.4 — Distribution

**Depends on:** Epic 3.2.0 spike complete (build matrix validated before distribution workflow is locked in).

| Task | Description | DoD | Ref |
|---|---|---|---|
| **3.4.1** Cross-platform binaries | Release workflow with cargo-dist: 5 targets (linux x86_64/ARM64 musl, macOS x86_64/ARM64, Windows x86_64). Shell + PowerShell installers | Tag triggers build. All 5 binaries as release assets. `curl \| sh` installs correctly | ARCH §17.2 |
| **3.4.2** npm wrapper | `@unblock/cli` on npm. Downloads platform binary on postinstall. `npx @unblock/cli` | `npx @unblock/cli ready` works. Platform detection correct | PRD §10 Phase 3 |
| **3.4.3** v1.0.0 release | Tag `unblock-mcp-v1.0.0`, release notes, README update with badges + install + quick start | Release published. README comprehensive. Plugin listed | — |

> **Deferred to v1.1.0:** Homebrew tap (`websublime/homebrew-tap`) — curl installer + npm + cargo install provide sufficient coverage for v1.0.0.

---

### Epic 3.5 — v1.0.0 Gap Features

**Goal:** Feature gaps identified in competitive analysis that strengthen the v1.0.0 offering.

**Ref:** `research/beads-vs-unblock-comparison.md`

| Task | Description | DoD | Ref |
|---|---|---|---|
| **3.5.1** Batch operations | Accept `ids: Vec<u64>` on `update`, `close`, `reopen`, `show`. Process sequentially, return results array. Rebuild graph once at end | Integration: batch close 3 issues → all closed, cascade computed once. Batch show → all returned. Partial failure → results + errors | Gap G1 |
| **3.5.2** `dep_tree` tool | Expose `dependency_tree(root, direction, max_depth)` from graph engine as MCP tool. Returns tree structure with status annotations | Integration: dep_tree on issue with 3 levels of deps → correct tree. Direction upstream/downstream works. max_depth respected | Gap G3.5 |
| **3.5.3** Date range filters | Add `created_after`, `created_before`, `updated_after`, `updated_before` params to `list` tool | Integration: filter by date range returns correct subset. Combinable with existing filters | Gap G6 |
| **3.5.4** Label OR filter | Add `label_mode: "and" \| "or"` param to `list` tool (default: `and`) | Integration: `list --label bug,enhancement --label_mode or` returns issues with either label | Gap G8 |

---

## Epic Dependency Graph

```
Phase 1
  1.1 Workspace ──► 1.2 Core ──► 1.3 GitHub API ──► 1.4 MCP Tools (includes prime as 1.4.14)
                                                      ├──► 1.5 Detection  ◄── 1.4.14
                                                      └──► 1.6 Reconciliation ◄── 1.4.14

Phase 2
  2.1 Tools (remaining) ◄── 1.4
  2.2 Plugin ◄── 2.1
  2.3 Docs ◄── 2.1

Phase 3
  3.1 Resilience ◄── 1.3
  3.2 Persistent Cache ◄── 3.1 (spike 3.2.0 is a prerequisite gate)
  3.3 Observability ◄── 1.4 + 1.5
  3.4 Distribution ◄── 2.1 + 3.1 + 3.2 + 3.3 (3.2.0 spike must complete first)
  3.5 Gap Features ◄── 2.1
```

> **Note:** `prime` was promoted from Phase 2 (2.1.1) to Phase 1 (1.4.14) because it is the
> session entry point required by both Detection (1.5) and Reconciliation (1.6). The GitHub
> Actions sentinel for reconciliation remains in Phase 3 (additional infrastructure).
>
> **Note:** Epic 3.2 (Persistent Cache) was inserted between Resilience and Observability.
> Its Task 3.2.0 (build spike) is a hard prerequisite for 3.4 Distribution — the musl
> build matrix must be validated before the distribution workflow is locked in. Epics
> previously numbered 3.2–3.4 are now 3.3–3.5.

---

## Risk Register

| Risk | Impact | Mitigation |
|---|---|---|
| GitHub blocking API changes | Medium — beta API | Monitor changelog. Fallback: labels as blocking |
| Rate limits | Low — 120 q/hr of 5000 | Configurable TTL. Backoff on 429 |
| High API call count per operation | Medium — latency | Batch GraphQL mutations, monitor latency |
| Projects V2 field deletion by human | Medium — broken state | Field validation at boot, clear error messages pointing to `setup` |
| Cross-repo token permissions | Low — requires repo scope | Token must have read access to referenced repos. Clear error message on 404 |
| REST API version mismatch | Low | Two API versions in use: `2022-11-28` for issue endpoints, `2026-03-10` for views/fields. Version set per-request via builder method. Both version paths tested |
| **git2 / gix build targets (Phase 3)** | **Medium — blocks Epic 3.2 and 3.3 Distribution** | **Spike required before Epic 3.2: validate git2 (vendored libgit2) vs gix (pure Rust) vs subprocess against all 5 build targets (Linux musl x86_64/ARM64, macOS x86_64/ARM64, Windows x86_64). musl + libgit2 static linking has known edge cases. Spike output: chosen library + confirmed CI build matrix. Blocker for 3.2 implementation and 3.3 Distribution.** |

---

## Milestones

| Milestone | Version | Criteria | Target |
|---|---|---|---|
| **MCP Foundation** | v0.1.0 | 12 tools E2E (incl. prime + reconcile). Client detection. Full agent workflow with drift repair | Week 12 |
| **MCP Complete** | v0.2.0 | Remaining tools + plugin. Feature-complete | Week 16 |
| **MCP Production** | v1.0.0 | Resilient, observable, distributed, gap features | Week 22 |

---

## Task Summary

| Phase | Epics | Tasks | Focused days |
|---|---|---|---|
| Phase 1 | 6 | 48 | ~24.25 |
| Phase 2 | 3 | 13 | ~7 |
| Phase 3 | 5 | 18 | ~11 |
| **Total** | **14** | **79** | **~42.25** |

> Phase 3 increased from 4 to 5 epics (added 3.2 Persistent Cache: 5 tasks, ~3 focused days including spike).

---

For the desktop application (dependency graph visualization, GPUI-based), see `unblock-desktop-project-plan.md`.
