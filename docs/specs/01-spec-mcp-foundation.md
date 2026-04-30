# Spec 01 — MCP Foundation (v0.1.0)

> Phase: 01
> Crates: `unblock-core`, `unblock-github`, `unblock-mcp`
> Source: [SPEC](../SPEC.md) · [PRD](../PRD.md) · [MANIFESTO](../MANIFESTO.md)
> Plan: [01-plan-mcp-foundation](../plans/01-plan-mcp-foundation.md)
> Status: draft
> Last updated: 2026-04-16

---

## Table of Contents

1. [Scope & Conventions](#1-scope--conventions)
2. [Types](#2-types)
3. [Graph Engine](#3-graph-engine)
4. [Cache Layer](#4-cache-layer)
5. [GitHub API Client](#5-github-api-client)
6. [MCP Server](#6-mcp-server)
7. [Tool Catalogue — Read Tools](#7-tool-catalogue--read-tools)
8. [Tool Catalogue — Write Tools](#8-tool-catalogue--write-tools)
9. [Body Section Parsing](#9-body-section-parsing)
10. [Status Update Algorithm](#10-status-update-algorithm)
11. [Error Model](#11-error-model)
12. [Configuration](#12-configuration)
13. [Testing Strategy](#13-testing-strategy)
14. [Invariants](#14-invariants)

---

## 1. Scope & Conventions

### 1.1 What this spec covers

Everything needed to implement Phase 01 (v0.1.0): 17 MCP tools, the graph engine, cache layer, GitHub API client, error model, configuration, and testing. This is the single source of truth for implementation agents working on Phase 01.

### 1.2 What this spec does NOT cover

- `reconcile`, `doctor`, `commit_context` tools (Phase 02)
- Circuit breaker and retry logic (Phase 02 — error types exist as stubs)
- Agent client detection / `AgentKind` / `SessionMeta` (Phase 02)
- OpenTelemetry metrics (Phase 02)
- Materialised fast path (Phase 04)
- Distribution, GHE testing, GitHub App auth (Phase 04)
- Plugin pipeline, skills, agents (Phase 05)
- Remote server, shared cache (Phase 06)

### 1.3 Pseudocode conventions

- Algorithms use numbered steps with plain English, not fake Rust
- Type definitions use Rust syntax — these are the implementation contract
- `→` means "returns"
- Indentation indicates nesting
- `IF`, `FOR`, `MATCH`, `RETURN` are control flow keywords

### 1.4 References

When this spec says "SPEC §N.N" it refers to the top-level [SPEC.md](../SPEC.md). When it says "PLAN GAP-N" it refers to the [Phase 01 Plan](../plans/01-plan-mcp-foundation.md) gap analysis.

---

## 2. Types

> Crate: `unblock-core/src/types.rs`

### 2.1 `QualifiedId`

```rust
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct QualifiedId {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}
```

Canonical node key. Display: `owner/repo#number`. FromStr: parses `owner/repo#42`. All graph operations use `QualifiedId` — never plain `u64`.

### 2.2 `IssueState`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueState {
    Open,
    Closed,
}
```

GitHub's native binary state. Ground truth for whether an issue is open or closed.

### 2.3 `Status`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Ready,
    InProgress,
    Blocked,
    Deferred,
    Closed,
}
```

Projects V2 custom field. Unified workflow + readiness state.

**Transition rules:**
- `Ready` ↔ `Blocked`: computed automatically by MCP server from dependency graph
- → `InProgress`: on `claim` (agent/human set)
- → `Deferred`: on `update` with `defer_until` (agent/human set)
- → `Closed`: on `close` (agent/human set)
- `Blocked`/`Ready` → re-evaluated: on `reopen` (graph-computed)

**Who sets what:**
- MCP server manages `Ready` ↔ `Blocked` transitions (graph-computed)
- Agent/human sets `InProgress`, `Deferred`, `Closed` (preserved by server — never overridden)

**Projects V2 option values:** `ready`, `in_progress`, `blocked`, `deferred`, `closed`

### 2.4 `Priority`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
    P4,
}
```

`as_sort_key() → u8`: P0=0, P1=1, P2=2, P3=3, P4=4. Used for deterministic ready queue sorting.

**Projects V2 option values:** `P0 - Critical`, `P1 - High`, `P2 - Medium`, `P3 - Low`, `P4 - Backlog`

### 2.5 `PipelineStage`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineStage {
    Investigation,
    Implementation,
    Review,
    Refactoring,
    Qa,
    Done,
}
```

Development pipeline phase. Created by `setup` in Phase 01 for field existence. Agent advancement is Phase 05 (plugin). The field exists so early adopters can use it manually and views work.

**Projects V2 option values:** `investigation`, `implementation`, `review`, `refactoring`, `qa`, `done`

### 2.6 `IssueType`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueType {
    Task,
    Bug,
    Feature,
    Epic,
    Chore,
    Spike,
}
```

GitHub's native org-level issue type. **NOT a Projects V2 custom field.** Read from GraphQL `issueType { name }` on each issue. Epic issues serve as parent containers for sub-issues.

### 2.7 `IssueRef`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueRef {
    Local(u64),
    CrossRepo { owner: String, repo: String, number: u64 },
}
```

Parsed user input. `#42` or `42` → `Local(42)`. `owner/repo#42` → `CrossRepo`. `resolve(owner, repo) → QualifiedId` converts Local to fully qualified.

Implements `FromStr` and `Display`.

### 2.8 `Issue`

```rust
pub struct Issue {
    pub qualified_id: QualifiedId,
    pub number: u64,
    pub node_id: String,                        // GitHub GraphQL node ID
    pub title: String,
    pub issue_type: Option<IssueType>,          // GitHub native (NOT Projects V2)
    pub status: Status,                         // Projects V2 field
    pub priority: Priority,                     // Projects V2 field
    pub pipeline_stage: Option<PipelineStage>,  // Projects V2 field
    pub agent: Option<String>,                  // Projects V2 field
    pub claimed_at: Option<DateTime<Utc>>,      // Projects V2 field
    pub story_points: Option<i32>,              // Projects V2 field
    pub defer_until: Option<NaiveDate>,         // Projects V2 field
    pub labels: Vec<String>,
    pub milestone: Option<String>,
    pub assignees: Vec<String>,
    pub state: IssueState,                      // GitHub native Open/Closed
    pub body: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub url: String,
    pub comments: Vec<IssueComment>,
    pub blocked_by: Vec<RelatedIssue>,          // from Issue.blockedBy
    pub blocking: Vec<RelatedIssue>,            // from Issue.blocking
    pub parent: Option<RelatedIssue>,
    pub sub_issues: Vec<RelatedIssue>,
}
```

### 2.9 `IssueSummary`

```rust
pub struct IssueSummary {
    pub qualified_id: QualifiedId,
    pub number: u64,
    pub title: String,
    pub issue_type: Option<IssueType>,
    pub status: Status,
    pub priority: Priority,
    pub agent: Option<String>,
    pub milestone: Option<String>,
    pub story_points: Option<i32>,
    pub defer_until: Option<NaiveDate>,
    pub labels: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub url: String,
}
```

Lightweight issue for list/ready responses. Derived from `Issue`.

**Scoping invariant (§14 Invariant 14).** `IssueSummary` is the shared shape behind both the ready-set projection (§7.1) and the filtered-list projection (§7.5). Callers of `compute_ready_set` (§3.3) receive a slice guaranteed to contain ONLY configured-repo source issues — `IssueSummary::qualified_id.(owner, repo) == (configured_owner, configured_repo)` for every element. The `list` tool (§7.5) enforces the same scope at the tool layer. No consumer may observe an `IssueSummary` whose `qualified_id` is cross-repo in either of these projections. `show` (§7.2) and `search` (§7.6) operate on bare `Issue` data and are exempt — they are explicitly allowed to surface cross-repo issues.

### 2.10 `BlockingEdge`

```rust
#[derive(Debug, Clone)]
pub struct BlockingEdge {
    pub source: QualifiedId,   // the blocked issue
    pub target: QualifiedId,   // the blocking issue
}
```

### 2.11 `IssueComment`

```rust
pub struct IssueComment {
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}
```

### 2.12 `RelatedIssue`

```rust
#[non_exhaustive]
pub struct RelatedIssue {
    pub number: u64,
    pub title: String,
    pub state: IssueState,
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
}

impl RelatedIssue {
    pub fn local(number: u64, title: impl Into<String>, state: IssueState) -> Self;
    pub fn cross_repo(
        number: u64,
        title: impl Into<String>,
        state: IssueState,
        owner: impl Into<String>,
        name: impl Into<String>,
    ) -> Self;
}
```

**Construction.** `RelatedIssue` is `#[non_exhaustive]`; callers build
instances through the `local` / `cross_repo` helpers (or via
`..Default::default()` at extension points). `local` leaves `repo_owner`
/ `repo_name` as `None`, which callers MUST interpret as "same repo as
the containing issue" (the "None = same-repo-as-enclosing" convention).
`cross_repo` takes an explicit `(owner, name)` pair for cross-repository
relations that need to be disambiguated from same-repo relations with the
same number (SPEC §11.4, unblock-29p.43).

### 2.13 `TraversalDirection`

```rust
pub enum TraversalDirection {
    Upstream,
    Downstream,
    Both,
}
```

### 2.14 `TreeNode` and `DependencyTree`

```rust
pub struct TreeNode {
    pub id: QualifiedId,
    pub status: Status,
    pub state: IssueState,
    pub depth: usize,
    pub children: Vec<TreeNode>,
}

pub struct DependencyTree {
    pub root: QualifiedId,
    pub upstream: Vec<TreeNode>,    // what root depends on
    pub downstream: Vec<TreeNode>,  // what depends on root
}
```

### 2.15 `BodySections`

```rust
pub struct BodySections {
    pub description: Option<String>,
    pub design_notes: Option<String>,
    pub acceptance_criteria: Option<String>,
}
```

With `from_markdown(&str) → BodySections` and `to_markdown() → String`. See §9 for algorithms.

### 2.16 `CrossRepoRefs`

```rust
pub struct CrossRepoRefs {
    pub omitted: Vec<String>,       // "owner/repo#number" via QualifiedId::Display
    pub summary: Option<String>,    // human-readable context
}
```

Shared response-side type carrying cross-repo nodes that were dropped from a bare-`u64` projection. Governed by the cross-repo response contract in §11.4. Full rules (population, determinism, markdown adaptation, affected tools) live there.

---

## 3. Graph Engine

> Crate: `unblock-core/src/graph.rs`
> Pure Rust. No network. No async. Fully testable with in-memory data.

### 3.1 `DependencyGraph`

```rust
pub struct DependencyGraph {
    graph: DiGraph<QualifiedId, ()>,
    node_map: HashMap<QualifiedId, NodeIndex>,
    issue_status: HashMap<QualifiedId, Status>,
    issue_state: HashMap<QualifiedId, IssueState>,
}
```

Edge direction: `blocked_issue → blocking_issue` (source depends on target). Outgoing edges from a node = "what blocks me". Incoming edges to a node = "what I block".

### 3.2 `build` — Graph construction

```
build(issues, edges) → DependencyGraph:

  1. Create empty DiGraph, node_map, issue_status, issue_state

  2. FOR each issue in issues:
     a. Create QualifiedId from issue
     b. Add node to graph, store index in node_map
     c. Store issue.status in issue_status
     d. Store issue.state in issue_state

  3. FOR each edge in edges:
     a. Look up source_idx and target_idx in node_map
     b. IF both exist: add edge (source_idx → target_idx)
     c. ELSE: log warning "Skipping edge with unknown node"
        (Orphaned edge — target issue may be deleted or inaccessible)

  4. RETURN DependencyGraph
```

**Edge cases:**
- Missing target node: edge skipped with warning. `reconcile` detects as `OrphanedBlockingEdge`.
- Duplicate edges: `DiGraph` allows parallel edges. GitHub prevents duplicates at source. If present, harmless.
- Self-edges: A→A. Should not appear (GitHub rejects self-blocking). Cycle detection catches it.
- Empty graph: zero issues → empty graph. `compute_ready_set()` returns empty. Valid state.

### 3.3 `compute_ready_set` — Ready set calculation

**Signature (BREAKING CHANGE vs. pre-unblock-eos.4 implementation):**

```rust
pub fn compute_ready_set(
    &self,
    issues: &[Issue],
    configured_owner: &str,
    configured_repo: &str,
) -> Vec<IssueSummary>
```

The engine takes `(configured_owner, configured_repo)` so that it can enforce the scoping invariant (Filter 3 below, §14 Invariant 14) at the source of truth. Prior to unblock-eos.4 the engine accepted only `issues` and allowed cross-repo source issues into the ready set; that projection was unsound — the tool-layer projections (`ready`, `prime`, cached `ready_set` consumed by `prime`) cannot represent non-local source issues in their bare-`u64` / local-only shapes (§11.4). See PLAN GAP-14 + D6 for the migration and commit discipline.

```
compute_ready_set(graph, issues, configured_owner, configured_repo) → Vec<IssueSummary>:

  ready = []

  FOR each issue in issues:
    // Filter 1: must be open in GitHub
    IF issue.state == Closed:
      CONTINUE

    // Filter 2: skip preserved states (set by agent/human)
    IF issue.status == InProgress:
      CONTINUE
    IF issue.status == Deferred:
      CONTINUE
    IF issue.status == Closed:
      CONTINUE

    // Filter 3: source issue MUST live in the configured (owner, repo).
    //          Cross-repo source issues are never members of the local
    //          ready-set projection (§11.4, §14 Invariant 14). Applied
    //          BEFORE Filter 4 so cross-repo blocker traversal is never
    //          performed for a cross-repo source. This is the scrub
    //          introduced by unblock-eos.4 (Direction 1).
    IF issue.qualified_id.owner != configured_owner:
      CONTINUE
    IF issue.qualified_id.repo != configured_repo:
      CONTINUE

    // Filter 4: check all blockers via graph (was Filter 3 pre-eos.4).
    //           Cross-repo blockers ARE honoured here — an open
    //           cross-repo blocker keeps the local source out of the
    //           ready set, and the tool layer surfaces the dropped
    //           blocker via §11.4 cross_repo_refs.
    IF issue.qualified_id IN node_map:
      idx = node_map[issue.qualified_id]
      blockers = graph.neighbors_directed(idx, Outgoing)

      all_blockers_closed = TRUE
      FOR each blocker_idx in blockers:
        blocker_qid = graph[blocker_idx]
        IF issue_state[blocker_qid] != Closed:
          all_blockers_closed = FALSE
          BREAK

      IF NOT all_blockers_closed:
        CONTINUE

    // Issue is ready (local-owned, open, not preserved, all blockers
    // closed or no blockers)
    ready.push(IssueSummary::from(issue))

  // Deterministic sort: priority ASC (P0 first) → created_at ASC (oldest first)
  ready.sort_by(|a, b| a.priority.as_sort_key().cmp(&b.priority.as_sort_key())
                        .then(a.created_at.cmp(&b.created_at)))

  RETURN ready
```

**Key:** The ready set computation does NOT look at the current `Status` field value to decide readiness. It computes readiness from the graph. Issues with `Status::Blocked` that now have all blockers closed WILL be in the ready set. The `update_status_fields` algorithm (§10) syncs the Status field to match.

**Scoping invariant (Filter 3):** `compute_ready_set` is the single chokepoint that enforces `ready_set ⊆ { issue | issue.qualified_id.(owner, repo) == (configured_owner, configured_repo) }`. Every downstream consumer of the ready set (cached `ready_set` in `GraphCache`, `prime` categorisation in §7.3, `ready` tool in §7.1, `update_status_fields` in §10) inherits this guarantee without re-checking. This is §14 Invariant 14's "configured-repo source" clause.

**Post-filters** (applied in tool layer, NOT in graph engine):
- `defer_until > today` → exclude (the graph does not know about dates)
- Agent filter, type filter, priority filter, milestone filter, label filter → applied after

**Edge cases:**
- Issue not in graph: has zero blockers → ready if local-owned and not in a preserved state
- Cross-repo source issue: dropped by Filter 3 regardless of blocker state — never in the ready set
- All blockers closed: every outgoing edge leads to a closed issue → ready (if local-owned)
- Mixed blockers: some closed, some open → not ready (blocked)
- Circular dependency: issues in a cycle always have an open blocker → never ready

### 3.4 `compute_unblock_cascade` — Cascade on close

```
compute_unblock_cascade(graph, closed_qid, issues) → Vec<QualifiedId>:

  IF closed_qid NOT IN node_map:
    RETURN []

  idx = node_map[closed_qid]
  unblocked = []

  // Find all issues that depend on the closed issue (Incoming = "what depends on me")
  dependents = graph.neighbors_directed(idx, Incoming)

  FOR each dependent_idx in dependents:
    dependent_qid = graph[dependent_idx]
    dependent_issue = find issue by dependent_qid

    IF dependent_issue.state == Closed:
      CONTINUE

    // Check if ALL blockers of this dependent are now closed
    blockers = graph.neighbors_directed(dependent_idx, Outgoing)
    all_closed = TRUE
    FOR each blocker_idx in blockers:
      blocker_qid = graph[blocker_idx]
      IF blocker_qid == closed_qid:
        CONTINUE  // the just-closed issue counts as closed
      IF issue_state[blocker_qid] != Closed:
        all_closed = FALSE
        BREAK

    IF all_closed:
      unblocked.push(dependent_qid)

  RETURN unblocked
```

**Critical (MUST):** The cascade MUST be computed from the PRE-CLOSE graph state — before the issue is closed in GitHub and before cache invalidation. Since bead `unblock-a36` widened `fetch_graph_data` to `states: [OPEN, CLOSED]` (§5.5), the just-closed issue would still appear in a POST-close rebuilt `node_map` (as `IssueState::Closed`), and the `blocker_qid == closed_qid` special-case in the loop above would still resolve. But PRE-close ordering remains MANDATORY for two reasons that are NOT addressed by the widening: (a) the rebuilt `Incoming` traversal from a Closed `closed_qid` would include already-closed dependents, and this function filters them only on `dependent_issue.state == Closed` (the explicit CONTINUE above) — relying on that filter holding stable is fragile versus relying on graph shape; (b) any race where a concurrent mutation alters a blocker's state between close-mutation and rebuild would silently shift the cascade set. Capturing PRE-close freezes the snapshot against both risks. The defensive `Vec::new()` short-circuit on `node_map.get(closed_qid) → None` at `unblock-core/src/graph.rs:289-291` remains correct for create-then-immediately-close races where `closed_qid` legitimately is not yet in the graph. See §8.2 (`close` tool flow) for the required ordering and the "Pre-close cascade MUST be captured before the mutation" paragraph for the normative prohibition.

**Edge cases:**
- Multi-level cascade: NOT recursive. Closing A unblocks B. B becomes ready. When B is later closed, its own cascade fires.
- Partial unblock: A depends on B and C. B is closed. A is NOT unblocked because C is still open.
- Already-closed dependent: A depends on B. A is already closed. When B closes, cascade skips A.

### 3.5 `would_create_cycle` — Pre-mutation cycle check

```
would_create_cycle(graph, source, target) → bool:

  // Adding edge source → target means "source depends on target"
  // A cycle exists if target already depends on source (path target → source)

  IF source == target:
    RETURN TRUE  // self-loop

  IF source NOT IN node_map OR target NOT IN node_map:
    RETURN FALSE  // new node, can't form cycle

  RETURN has_path_connecting(graph, node_map[target], node_map[source])
```

Uses `petgraph::algo::has_path_connecting`. O(V+E). Called before `add_blocked_by` in GitHub — prevents cycles from forming.

### 3.6 `detect_all_cycles` — Full cycle detection

```
detect_all_cycles(graph) → Vec<Vec<QualifiedId>>:

  sccs = tarjan_scc(&graph)

  cycles = []
  FOR each scc in sccs:
    IF scc.len() > 1:
      // Multi-node SCC = cycle
      cycles.push(scc mapped to QualifiedIds)
    ELSE IF scc.len() == 1:
      idx = scc[0]
      IF graph.contains_edge(idx, idx):
        // Self-loop
        cycles.push([graph[idx]])

  RETURN cycles
```

Uses `petgraph::algo::tarjan_scc`. O(V+E).

### 3.7 `dependency_tree` — BFS traversal

```
dependency_tree(graph, root, direction, max_depth) → DependencyTree:

  upstream = []
  downstream = []

  IF direction == Upstream OR direction == Both:
    upstream = bfs_tree(graph, root, Outgoing, max_depth)
    // Outgoing from root = "what does root depend on"

  IF direction == Downstream OR direction == Both:
    downstream = bfs_tree(graph, root, Incoming, max_depth)
    // Incoming to root = "what depends on root"

  RETURN DependencyTree { root, upstream, downstream }
```

**Default max_depth:** 10. Configurable per-call. `visited` set prevents infinite loops on cycles.

### 3.8 Accessor methods

- `node_map() → &HashMap<QualifiedId, NodeIndex>`
- `inner_graph() → &DiGraph<QualifiedId, ()>`
- `issue_state() → &HashMap<QualifiedId, IssueState>`
- `issue_status() → &HashMap<QualifiedId, Status>`
- `all_edges() → Vec<BlockingEdge>`
- `edge_count() → usize`

---

## 4. Cache Layer

> Crate: `unblock-core/src/cache.rs`

### 4.1 `GraphCache`

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
```

### 4.2 Methods

| Method | Effect |
|---|---|
| `new(ttl)` | Create empty cache |
| `get_ready_set() → Option<Arc<Vec<IssueSummary>>>` | Returns ready set if entry exists |
| `get_graph() → Option<Arc<DependencyGraph>>` | Returns graph if entry exists |
| `update(ready_set, graph)` | Replaces entry, resets `built_at` to now |
| `invalidate()` | Clears entry → Empty |
| `is_fresh() → bool` | `built_at + ttl > now` AND entry exists |

### 4.3 State machine

```
                 invalidate()
    ┌───────┐ ───────────────► ┌───────┐
    │ Fresh │                  │ Empty │
    └───┬───┘ ◄──────────────  └───┬───┘
        │        update()          │
        │                          │
        │ TTL expires              │ update()
        ▼                          ▼
    ┌───────┐                  ┌───────┐
    │ Stale │ ────update()───► │ Fresh │
    └───────┘                  └───────┘
```

- **Fresh:** entry exists, `built_at + ttl > now`. Serve directly, zero API calls.
- **Stale:** entry exists, `built_at + ttl <= now`. Caller MUST rebuild unconditionally.
- **Empty:** no entry. Cold start, post-invalidation, or first use.

Default TTL: 30 seconds. Configurable via `UNBLOCK_CACHE_TTL`.

### 4.4 Invalidation matrix

| Tool | Invalidates | Reason |
|---|---|---|
| `close` | Yes | Cascade changes topology |
| `claim` | Yes | Status field changes |
| `create` | Yes | New node in graph |
| `depends` | Yes | New edge in graph |
| `dep_remove` | Yes | Edge removed |
| `update` | Yes | Status/defer may change ready set |
| `reopen` | Yes | Node returns to graph |
| `comment` | **No** | Graph topology unchanged |
| `show` | **No** | Read-only, always fresh from GitHub |
| `ready` | **No** | Read-only |
| `prime` | **No** | Read-only |
| `stats` | **No** | Read-only |
| `list` | **No** | Read-only |
| `search` | **No** | Bypasses cache entirely |
| `dep_cycles` | **No** | Read-only |

### 4.5 Concurrency

`RwLock<Option<CacheEntry>>`. Multiple readers concurrent. Single writer exclusive. Last writer wins — no optimistic locking. Acceptable for single-process architecture.

**Invariant:** Every field in `CacheEntry` is reconstructable from GitHub with a single `fetch_graph_data()` call. The cache is a performance optimisation, not a source of truth.

---

## 5. GitHub API Client

> Crate: `unblock-github`

### 5.1 `GitHubClient`

```rust
pub struct GitHubClient {
    http: reqwest::Client,
    token: String,
    api_base_url: String,
    github_url: String,
    owner: String,
    repo: String,
    project_number: Option<u64>,
    project_id: Option<String>,
    field_ids: Option<ProjectFieldIds>,
}
```

### 5.2 `ProjectFieldIds`

```rust
pub struct ProjectFieldIds {
    pub status: FieldMeta,
    pub priority: FieldMeta,
    pub pipeline_stage: FieldMeta,
    pub agent: String,          // text field — field_id only, no options
    pub claimed_at: String,     // date field — field_id only
    pub story_points: String,   // number field — field_id only
    pub defer_until: String,    // date field — field_id only
}

pub struct FieldMeta {
    pub field_id: String,
    pub options: HashMap<String, String>,  // display_name → option_node_id
}
```

**7 fields. No more, no less.** `IssueType` is NOT a Projects V2 custom field — it's GitHub's native org-level feature.

### 5.3 `FieldValue`

```rust
pub enum FieldValue {
    SingleSelectOption(String),  // option node ID
    Text(String),
    Date(NaiveDate),
    Number(f64),
}
```

### 5.4 `GitHubApi` trait

Defined in `unblock-github/src/api.rs`. Abstracts all GitHub operations. `async_trait` for object safety. Blanket impl on `GitHubClient`. Tests use `MockGitHubClient` (feature-gated `test-hooks`).

`ServerState` holds `Arc<dyn GitHubApi>`.

**Sync accessors:** `owner()`, `repo()`, `project_number()`, `api_base_url()`, `rest_url()`, `graphql_url()`, `field_ids()`, `set_field_ids()`

**GraphQL reads:**
- `fetch_graph_data() → (Vec<Issue>, Vec<BlockingEdge>)` — all issues (both `Open` and `Closed`) with edges and field values; `IssueState` on each node is preserved so closed nodes can be consumed by `list(status="Closed")`, cascade walks (§3.4), and the dep_remove endpoint-Closed UX (§8.5)
- `fetch_issue(number) → Issue` — single issue with comments, always fresh
- `fetch_issue_ref(ref) → Issue` — resolve IssueRef then fetch

**Mutations:**
- `create_issue(params) → Issue`
- `close_issue(number, reason)`
- `reopen_issue(number)`
- `add_comment(number, body) → String`
- `update_issue_body(number, body)`
- `add_labels_to_issue(number, labels)`
- `remove_label_from_issue(number, label)`
- `add_assignees_to_issue(number, assignees)`
- `remove_assignees_from_issue(number, assignees)`
- `list_milestones() → Vec<Milestone>`
- `update_issue_milestone(number, milestone_number)`
- `add_blocked_by(issue_number, blocked_by_number)`
- `add_blocked_by_ref(issue_number, blocker: &IssueRef)`
- `remove_blocked_by(issue_number, blocked_by_number)`
- `add_sub_issue(parent_number, child_number)`
- `resolve_issue_ref(ref) → String` (node ID)
- `search_issues(query, limit) → Vec<Issue>`

**Projects V2:**
- `resolve_project_info() → ProjectInfo`
- `setup_fields(project_id) → SetupReport`
- `query_setup_status(project_id) → SetupStatus`
- `update_field(project_id, item_id, field_id, value)`
- `get_project_item_id(issue_node_id, project_id) → String`
- `detect_owner_type() → OwnerType`
- `list_rest_fields(owner_type) → Vec<RestField>`
- `create_view(owner_type, params) → ProjectView`
- `list_views(owner_type) → Vec<ProjectView>`
- `list_owner_projects(owner_type) → Vec<OwnerProject>`
- `create_project(owner_node_id, title) → CreatedProject`
- `ensure_labels(labels)`

### 5.5 GraphQL read queries

**`fetch_graph_data()`** — primary read query. Paginated (100 issues per page). Returns:

```graphql
query($owner: String!, $repo: String!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    issues(first: 100, after: $cursor, states: [OPEN, CLOSED]) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number title state createdAt
        labels(first: 10) { nodes { name } }
        milestone { number title }
        assignees(first: 5) { nodes { login } }
        issueType { name }
        blockedBy(first: 50) {
          nodes { number repository { owner { login } name } state }
        }
        blocking(first: 50) {
          nodes { number repository { owner { login } name } state }
        }
        parent { number }
        projectItems(first: 5) {
          nodes {
            project { number }
            fieldValues(first: 20) {
              nodes {
                ... on ProjectV2ItemFieldSingleSelectValue {
                  field { ... on ProjectV2SingleSelectField { name } } name
                }
                ... on ProjectV2ItemFieldTextValue {
                  field { ... on ProjectV2Field { name } } text
                }
                ... on ProjectV2ItemFieldDateValue {
                  field { ... on ProjectV2Field { name } } date
                }
                ... on ProjectV2ItemFieldNumberValue {
                  field { ... on ProjectV2Field { name } } number
                }
              }
            }
          }
        }
      }
    }
  }
}
```

**Blocking edges:** extracted from `Issue.blockedBy` (what blocks this issue) and `Issue.blocking` (what this issue blocks). Both traversed for complete edge set.

**Schema anchor (matches GitHub public GraphQL schema as of 2026-04-30):** `Issue.blockedBy` and `Issue.blocking` are GA `IssueConnection!` fields (no `GraphQL-Features` preview header required — verified via live introspection against `api.github.com/graphql`). `Issue.blockedBy` enumerates issues that block the current issue; `Issue.blocking` enumerates issues the current issue blocks. The legacy field names `trackedByIssues` / `trackedIssues` previously referenced in this spec do NOT exist on `Issue` (HTTP 422 from the GraphQL endpoint); they were a documentation drift introduced in early phase work. See bead `unblock-741` for the post-mortem.

**Cross-repo:** `blockedBy.nodes[].repository` may differ from queried repo. `QualifiedId` constructed from each node's repository context.

**`fetch_issue(number)`** — single issue with full comments (first 50), blocking/blocked_by relationships, parent/sub-issues, Projects V2 field values. Always fresh, never cached.

### 5.6 Mutations

**REST mutations:** use `X-GitHub-Api-Version: 2022-11-28`.
- `POST /repos/{o}/{r}/issues` — create
- `PATCH /repos/{o}/{r}/issues/{n}` — close (`state: "closed"`), reopen (`state: "open"`), update body/labels/assignees/milestone
- `POST /repos/{o}/{r}/issues/{n}/comments` — add comment

**GraphQL mutations** (schema as of 2026-04-30; see §5.5 schema anchor — all four are GA on the public GraphQL API and require no `GraphQL-Features` preview header):
- `addBlockedBy` — add blocking relationship (cross-repo). Input: `AddBlockedByInput { issueId, blockingIssueId, clientMutationId }`. Replaces the legacy `addIssueDependency` mutation referenced in earlier drafts of this spec.
- `removeBlockedBy` — remove blocking relationship. Input: `RemoveBlockedByInput { issueId, blockingIssueId, clientMutationId }`. Replaces the legacy `removeIssueDependency`.
- `addSubIssue` — add parent-child relationship.
- `updateProjectV2ItemFieldValue` — update Projects V2 field.

**Batch mutations:** Multiple `updateProjectV2ItemFieldValue` in a single GraphQL request using aliases (`update0`, `update1`, `update2`, ...).

**Cross-repo scope:**

| Operation | Cross-repo | Rationale |
|---|---|---|
| `depends` / `dep_remove` | Yes | Dependencies are the core cross-repo use case |
| `show` / `fetch_issue_ref` | Yes | Inspect cross-repo blockers |
| `close` | Cascade side-effects only | The `closeIssue` mutation itself remains scoped to the configured repo for safety; cross-repo **dependents unblocked by the close** receive the Status → `ready` + unblock-comment side effects per §8.2 step 6 / §11.4 row 4. Cross-repo cascade side effects are best-effort: a foreign repo on which the configured token lacks write scope fails with a logged warning and does not abort the close. |
| `reopen`, `update`, `claim`, `comment` | No | Scoped to configured repo for safety |
| `create` (`blocked_by` param) | Yes | Cross-repo deps at creation time |

**Cascade-primitive asymmetry — no `update_field_ref`.** The Phase-3 cascade ladder in §8.2 step 6 dispatches three side effects per cross-repo dependent: `fetch_issue`, Projects V2 `update_field`, and `add_comment`. Only two of these are addressed by `(owner, repo, number)` and therefore require `*_ref` variants to route cross-repo — `fetch_issue_ref` (§5.4) and `add_comment_ref`. `update_field` is intentionally NOT extended with an `update_field_ref` variant, because `updateProjectV2ItemFieldValue` operates on globally-scoped Projects V2 node IDs (`project_id` + `item_id`), not on `(owner, repo, number)`. The project item is resolved once per cascade member from the `fetch_issue_ref` result (`issue_node_id` → `get_project_item_id(issue_node_id, project_id)`), and those node IDs are fed directly to the existing `update_field`. A `*_ref` wrapper would add no routing — the node IDs already identify the correct item across repos. This keeps the `GitHubApi` surface minimal: `*_ref` variants exist only where the underlying API endpoint is addressed by `(owner, repo, number)` and cross-repo retargeting is possible.

### 5.7 Projects V2 field management

**`resolve_project_info()`** — called once at startup:
1. Find project number (from config or auto-detect first linked project)
2. Resolve project node ID
3. Query all fields, map to `ProjectFieldIds`
4. Validate 7 required fields exist with correct types

**`setup_fields(project_id)`** — idempotent field creation:
1. Query existing fields
2. For each of 7 required fields: if missing, create with correct type and options
3. Return `SetupReport { created, skipped }`

**7 required fields with their type and options:**

| Field | Type | Options (for Single Select) |
|---|---|---|
| Status | Single Select | `ready`, `in_progress`, `blocked`, `deferred`, `closed` |
| Priority | Single Select | `P0 - Critical`, `P1 - High`, `P2 - Medium`, `P3 - Low`, `P4 - Backlog` |
| Pipeline Stage | Single Select | `investigation`, `implementation`, `review`, `refactoring`, `qa`, `done` |
| Agent | Text | — |
| Claimed At | Date | — |
| Story Points | Number | — |
| Defer Until | Date | — |

### 5.8 View management

5 views created via REST API (`X-GitHub-Api-Version: 2026-03-10`):

| View | Layout | Filter |
|---|---|---|
| `UNBLOCK://ready` | Board | `Status:"ready"` |
| `UNBLOCK://team` | Board | — (grouped by Agent) |
| `UNBLOCK://pipeline` | Board | — (grouped by Pipeline Stage) |
| `UNBLOCK://roadmap` | Table | — |
| `UNBLOCK://timeline` | Roadmap | — |

View creation requires integer field IDs (not GraphQL node IDs). Discovered via REST `GET /fields`.

Owner type detection (org vs user) determines REST endpoint: `/orgs/{org}/projectsV2/{n}/views` vs `/users/{user}/projectsV2/{n}/views`.

Idempotent: if view already exists (matching name), skip.

### 5.9 URL resolution

| Environment | `GITHUB_API_URL` | GraphQL endpoint |
|---|---|---|
| github.com | `https://api.github.com` | `{base}/graphql` |
| GHE Server | `https://<host>/api/v3` | Strip `/v3` → `{base}/graphql` |
| GHE Cloud | `https://api.<host>` | `{base}/graphql` |

`graphql_url()`: if `api_base_url` ends with `/v3`, strip suffix before appending `/graphql`.

Trailing slashes normalised at load time.

### 5.10 Pagination

Cursor-based. Loop while `hasNextPage == true`, advancing `cursor`. Each page returns up to 100 items.

**Edge cases:**
- Empty repo: zero issues → zero pages → empty result. Valid.
- Exactly 100 issues: one page, `hasNextPage: false`.
- Concurrent mutations mid-pagination: issue created between pages may be missed. Acceptable — next rebuild catches it.

---

## 6. MCP Server

> Crate: `unblock-mcp`

### 6.1 `ServerState`

```rust
pub struct ServerState {
    pub config: Arc<Config>,
    pub github: Arc<dyn GitHubApi>,
    pub cache: Arc<GraphCache>,
}
```

Shared across all tool invocations.

**Note:** Phase 02 adds `agent_kind: OnceLock<AgentKind>` and `agent_client: OnceLock<AgentClient>`. If these already exist in code, they are excluded from Phase 01 acceptance criteria.

### 6.2 Bootstrap sequence

```
1. Config::load()
2. Init tracing (JSON format, stderr — stdout reserved for MCP stdio)
3. GitHubClient::new(config) — resolve repo from git remote, resolve project + fields
4. Validate 7 required fields exist (if project detected)
5. GraphCache::new(config.cache_ttl)
6. ServerState { config, github, cache }
7. UnblockServer::new(state).serve(stdio())
```

**Bootstrap mode:** if no project detected (first-time use), only `init` and `setup` are functional. All other tools return `ProjectNotConfigured`.

### 6.3 Tool execution pattern

```rust
// File: unblock-mcp/src/tools/mod.rs

pub async fn execute_read_tool<F, R>(state, op: F) -> CallToolResult
where F: Future<Output = Result<R, Error>>
{
    match op.await {
        Ok(result) => success_response(result),
        Err(err) => error_response(github_error_to_mcp(err)),
    }
}

pub async fn execute_write_tool<F, R>(state, op: F) -> CallToolResult
where F: Future<Output = Result<R, Error>>
{
    match op.await {
        Ok(result) => {
            rebuild_cache(state).await;
            success_response(result)
        }
        Err(err) => error_response(github_error_to_mcp(err)),
    }
}

pub async fn rebuild_cache(state) {
    state.cache.invalidate();
    let (issues, edges) = state.github.fetch_graph_data().await?;
    let graph = DependencyGraph::build(&issues, &edges);
    // §3.3 Filter 3 / §14 Invariant 14(a): engine is the single chokepoint
    // that scrubs cross-repo source issues from the ready-set projection.
    // Callers pass the configured (owner, repo) so downstream consumers
    // (cached `ready_set`, `ready`, `prime`, `update_status_fields`) inherit
    // the guarantee without re-checking.
    let ready_set = graph.compute_ready_set(
        &issues,
        state.github.owner(),
        state.github.repo(),
    );
    update_status_fields(state, &issues, &ready_set).await?;
    state.cache.update(ready_set, graph);
}
```

### 6.4 `set_project_fields` helper

Extracted shared helper for setting Projects V2 fields on an issue. Used by `claim`, `close`, `create`, `update`, `reopen`, and cascade.

```
set_project_fields(state, issue_node_id, project_id, fields: Vec<(field_id, FieldValue)>):
  1. Get project item ID: get_project_item_id(issue_node_id, project_id)
  2. For each (field_id, value): update_field(project_id, item_id, field_id, value)
```

Prevents the field-update logic from being duplicated across tools (see PLAN GAP-13).

---

## 7. Tool Catalogue — Read Tools

### 7.1 `ready`

```rust
pub struct ReadyParams {
    pub limit: Option<u32>,           // default: 10, max: 100
    pub issue_type: Option<String>,   // "task", "bug", etc.
    pub priority: Option<String>,     // "P0", "P1", etc.
    pub milestone: Option<String>,
    pub agent: Option<String>,
    pub label: Option<String>,
    pub include_claimed: Option<bool>, // default: false
}

pub struct ReadyResult {
    pub issues: Vec<IssueSummary>,
    pub count: usize,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_repo_refs: Option<CrossRepoRefs>,  // §11.4
}
```

**Validation:**
- `limit`: 1..=100 if present
- `priority`: must be P0–P4 if present

**Flow:**
1. Check cache: Fresh → use cached ready set; Stale/Empty → fetch + rebuild
2. Start with ready set from cache/rebuild (guaranteed local-only per §3.3 Filter 3 / §14 Invariant 14 — no defensive owner/repo check required in the tool handler)
3. Post-filter: exclude `defer_until > today`
4. If NOT `include_claimed`: exclude `Status::InProgress` (already excluded from ready set, but defensive)
5. Filter by: `issue_type`, `priority`, `milestone`, `agent`, `label`
6. Sort: priority ASC → created_at ASC (already sorted from `compute_ready_set`)
7. Limit to top N
8. Set `stale = !cache.is_fresh()`
9. Compute `cross_repo_refs` per §11.4: collect every cross-repo `QualifiedId` that appears as an OPEN blocker of any local issue that was filtered OUT of the ready set by step 6 of `compute_ready_set` (§3.3) due to that blocker being non-closed. Filter 3 of §3.3 already removed any cross-repo source issue from the projection, so this step only inspects LOCAL sources and their cross-repo blockers. These refs are not expressible in `IssueSummary.number: u64` because the local projection cannot represent them. Deduplicate, sort, attach.

**Source-scoping guarantee (§14 Invariant 14).** Per §3.3 Filter 3, every `IssueSummary` returned by `ready.issues` has `qualified_id.(owner, repo) == (configured_owner, configured_repo)`. The `ready` handler does NOT re-check — the graph engine is the single chokepoint. `cross_repo_refs` remains the ONLY channel through which cross-repo information surfaces in a `ReadyResult` (always as blockers, never as sources).

**Cross-repo contract (§11.4):** Cross-repo blockers silently influence ready-set filtering — a local issue can be held out of the ready set by a cross-repo dependency the agent cannot see in `issues`. The `cross_repo_refs` field surfaces those nodes. `None` when no cross-repo blocker participated in filtering.

**Cache:** Read-only. No invalidation.
**API calls:** 0 (cache hit) | 1+ (rebuild)

### 7.2 `show`

```rust
pub struct ShowParams {
    pub issue: String,                // IssueRef: "#42", "42", "owner/repo#42"
    pub include_comments: Option<bool>, // default: true
    pub include_deps: Option<bool>,     // default: true
}

pub struct ShowResult {
    pub issue: ShowIssue,             // full issue with parsed body sections
    pub comments: Option<Vec<IssueComment>>,
    pub upstream: Option<Vec<TreeNode>>,
    pub downstream: Option<Vec<TreeNode>>,
}
```

**Validation:**
- `issue`: must parse as valid IssueRef

**Flow:**
1. Parse IssueRef
2. `fetch_issue_ref(ref)` — ALWAYS fresh, never cached
3. Parse body sections via `BodySections::from_markdown()`
4. If `include_deps`: `dependency_tree(root, Both, max_depth=5)` (from cache or rebuild)
5. Return

**Cache:** NOT used for the issue itself. Graph cache used only for dependency tree.
**API calls:** 1 (always)

### 7.3 `prime`

```rust
pub struct PrimeParams {}

pub struct PrimeResult {
    pub context: String,  // markdown blob for agent injection
}
```

**Flow:**
1. Fetch graph data (or use cache)
2. Build context summary:
   - Repo: `owner/repo`
   - Project: number
   - Ready count, blocked count, in-progress count
   - Issues with cycles (if any)
3. Append cross-repo section per §11.4 (markdown adaptation): list each cross-repo `QualifiedId` that participated in the cycle summary but could not be rendered as a local `#number` reference. Omit the entire section when no such refs exist.
4. Return markdown blob

**Cross-repo contract (§11.4):** Because `prime` returns markdown rather than a typed struct, the cross-repo refs are rendered as a trailing `## Cross-repo references` section. Entries use `QualifiedId::Display` format (`owner/repo#N`), sorted lexicographically. The section is omitted entirely when no cross-repo node contributed to the cycle summary.

**Cache:** Read-only.
**API calls:** 0 (cache hit) | 1+ (rebuild)

### 7.4 `stats`

```rust
pub struct StatsParams {
    pub milestone: Option<String>,
}

pub struct StatsResult {
    pub total: usize,
    pub by_status: HashMap<String, usize>,
    pub by_priority: HashMap<String, usize>,
    pub blocked_count: usize,
    pub ready_count: usize,
    pub cycle_count: usize,
    pub agents: Vec<AgentStats>,
}

pub struct AgentStats {
    pub name: String,
    pub in_progress: usize,
    pub completed: usize,
}
```

**Flow:**
1. Fetch graph data (or use cache)
2. Aggregate counts across all issues (filter by milestone if provided)
3. Cycle count from `detect_all_cycles().len()`

**Cache:** Read-only.
**API calls:** 0 (cache hit) | 1+ (rebuild)

### 7.5 `list`

```rust
pub struct ListParams {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub issue_type: Option<String>,
    pub milestone: Option<String>,
    pub agent: Option<String>,
    pub label: Option<String>,
    pub assignee: Option<String>,
    pub sort: Option<String>,         // "priority" (default), "created", "updated"
    pub limit: Option<usize>,         // default: 50, max: 200
    pub offset: Option<usize>,        // default: 0
}

pub struct ListResult {
    pub issues: Vec<IssueSummary>,
    pub total: usize,
    pub stale: bool,
}
```

**Validation:**
- `limit`: 1..=200 if present
- `sort`: must be "priority", "created", or "updated" if present
- Empty/whitespace-only filter strings treated as absent

**Flow:**
1. Fetch graph data (or use cache)
2. Filter by all params (AND logic — all filters must match)
3. Sort by requested field (priority ASC default, created ASC, updated DESC)
4. Record `total` before pagination
5. Paginate: skip `offset`, take `limit`

**`status="Closed"` visibility.** Before bead `unblock-a36`, `fetch_graph_data` filtered to `states: [OPEN]`, so `list(status="Closed")` always returned `{ issues: [], total: 0 }`. After widening `fetch_graph_data` to `states: [OPEN, CLOSED]` (§5.5), the cache is populated with both live and archived issues; `list(status="Closed")` returns the configured-repo subset in the same sorted/paginated projection as any other status filter. `status="Ready"` and the other live buckets are unaffected — the filter is applied after the cache read so closed issues are excluded from any ready-class projection.

**Cache:** Read-only.
**API calls:** 0 (cache hit) | 1+ (rebuild)

### 7.6 `search`

```rust
pub struct SearchParams {
    pub query: String,                // required, non-empty
    pub limit: Option<u32>,           // default: 20
}

pub struct SearchResult {
    pub issues: Vec<IssueSummary>,
    pub count: usize,
}
```

**Validation:**
- `query`: non-empty

**Flow:**
1. Call `search_issues(query, limit)` — GitHub Search API
2. Search query: `"repo:{owner}/{repo} is:issue {query}"`
3. Map results to `IssueSummary`

**Cache:** Bypassed entirely. Each search hits GitHub Search API directly.
**API calls:** 1

### 7.7 `dep_cycles`

```rust
pub struct DepCyclesParams {
    pub id: Option<u64>,  // optional — targeted check from specific issue
}

pub struct DepCyclesResult {
    pub cycles: Vec<Vec<u64>>,  // issue numbers scoped to configured repo — cross-repo cycle members are surfaced in `cross_repo_refs` per §11.4
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_repo_refs: Option<CrossRepoRefs>,  // §11.4
}
```

**Flow:**
1. Fetch graph data (or use cache)
2. If `id` provided: targeted cycle check involving that node
3. If `id` absent: `detect_all_cycles()` on full graph
4. Project `Vec<Vec<QualifiedId>>` → `Vec<Vec<u64>>`:
   a. For each cycle: keep only nodes whose `(owner, repo)` matches the configured repo; emit as a `Vec<u64>` of bare numbers.
   b. A cycle whose local-projection length is `< 2` after filtering (a cycle that becomes trivial once cross-repo members are stripped) is still emitted if the original had ≥2 nodes, so the agent knows the cycle exists — the bare-`u64` vector may therefore be shorter than the true cycle length. Callers MUST consult `cross_repo_refs` for the missing members.
   c. Collect every cross-repo `QualifiedId` that was stripped in step (a) into the `cross_repo_refs` set.
5. Populate `cross_repo_refs` per §11.4. `summary` example: `"3 cross-repo cycle members omitted from `cycles`"`.

**Cross-repo contract (§11.4):** `cycles: Vec<Vec<u64>>` cannot express cross-repo cycle members. When a detected cycle traverses at least one `QualifiedId` outside the configured repo, those nodes are omitted from the local vector and surfaced in `cross_repo_refs`. The field is `None` when no cycle touches a cross-repo node.

**Cache:** Read-only.
**API calls:** 0 (cache hit) | 1+ (rebuild)

---

## 8. Tool Catalogue — Write Tools

### 8.1 `claim`

```rust
pub struct ClaimParams {
    pub id: u64,
    pub agent: Option<String>,  // defaults to config.agent
}

pub struct ClaimResult {
    pub issue: IssueSummary,
}
```

**Validation:**
- `id`: positive integer
- `agent`: non-empty if present

**Flow:**
1. Fetch issue (single query, always fresh)
2. Validate:
   a. `IssueState == Open` → else `IssueClosed`
   b. `Status != InProgress` → else `AlreadyClaimed`
   c. Not blocked: check graph → else `IssueBlocked { blockers }`
   d. Not deferred: `defer_until <= today` → else `IssueDeferred`
3. Update fields: Status → `in_progress`, Agent → name, Claimed At → now
4. Add comment: `"Claimed by {agent} at {timestamp}"`
5. Invalidate cache + rebuild + update Status fields

**API calls:** 1 (fetch) + 3 (field updates) + 1 (comment) + 1+ (rebuild)
**Cache:** Invalidates.

### 8.2 `close`

```rust
pub struct CloseParams {
    pub id: u64,
    pub reason: Option<String>,
}

pub struct CloseResult {
    pub issue: IssueSummary,
    pub unblocked: Vec<u64>,  // scoped to configured repo; cross-repo dependents that were cascade-updated are surfaced in `cross_repo_refs` per §11.4
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_repo_refs: Option<CrossRepoRefs>,  // §11.4
}
```

**Validation:**
- `id`: positive integer

**Cross-repo contract (§11.4):** `compute_unblock_cascade` (§3.4) returns `Vec<QualifiedId>`. Local dependents are projected to `u64` and emitted in `unblocked`; cross-repo dependents are dropped from that projection and surfaced in `cross_repo_refs`. Cross-repo dependents ARE still cascade-updated in step 6 — only the response shape differs. `cross_repo_refs` is `None` when no cross-repo dependent was cascaded. `summary` example: `"1 cross-repo dependent cascade-updated but omitted from `unblocked`"`.

**Flow (ordering is critical):**
1. Fetch issue, validate `IssueState == Open` → else `IssueClosed`
2. **PRE-CLOSE cascade computation (MUST, see §3.4 + `Why step 2 before step 3` below):**
   a. Ensure graph is built (from cache or fresh fetch) — if the cache is
      cold, issue a `fetch_graph_data` round-trip before step 3 so the
      cascade is computed against a graph that still contains the closed
      issue as an OPEN node. This is the only chokepoint where the
      cascade list can be captured soundly.
   b. `compute_unblock_cascade(graph, closed_qid, issues)` — captures
      the full cascade list (local + cross-repo dependents) while
      `closed_qid ∈ graph.node_map`.
   c. Save the unblocked list (`Vec<QualifiedId>`) for Phase 3 field
      updates in step 6 and the response projection in step 9. The
      graph still contains the issue as open at this point.
3. Close issue: REST PATCH `state: "closed"`
4. Update fields: Status → `closed`
5. Add comment: `"Closed: {reason}"` (or `"Closed"` if no reason)
6. For each unblocked (from step 2 — cascade list captured PRE-close):
   a. Update Status → `ready`
   b. Add comment: `"Unblocked — blocker #{id} was closed"`
7. Invalidate cache + rebuild graph (post-close: issue appears in the rebuilt graph as `IssueState::Closed` per `fetch_graph_data`'s widened `states: [OPEN, CLOSED]` filter at `unblock-github/src/graphql.rs:129`; PRE-close cascade capture in step 2 remains MANDATORY for the reasons enumerated in §3.4 Critical — the rebuild is a rebuild, not a cascade source)
8. `update_status_fields` — syncs Status for issues NOT already handled in step 6 (e.g., issues whose blocker status changed but were not direct dependents of the closed issue)
9. Partition the cascade list from step 2 by `(owner, repo) == (config.owner, config.repo)`: local dependents go into `unblocked: Vec<u64>`; cross-repo dependents populate `cross_repo_refs` per §11.4 (deduplicated, sorted by `QualifiedId::Display`).
10. Update cache

**Pre-close cascade MUST be captured before the mutation.** The cascade list is an
authoritative output of the tool — the agent relies on it to drive downstream
work. PRE-close ordering is MANDATORY, not advisory. Two independent reasons,
either of which is sufficient:

1. **Traversal-set fragility.** Since `fetch_graph_data` now returns `states:
   [OPEN, CLOSED]` (bead `unblock-a36`), a POST-close rebuild carries the
   just-closed issue as `IssueState::Closed` and the `Incoming` traversal in
   §3.4 enumerates already-closed dependents alongside live ones. The
   `dependent_issue.state == Closed → CONTINUE` filter handles that on the
   happy path, but relying on a post-filter of a widened traversal is strictly
   fragile versus capturing the authoritative set from the pre-close graph
   where the traversal shape is already correct.
2. **Concurrent-mutation races.** Between the close mutation and the rebuild,
   a concurrent write to any blocker (close, reopen, or edge change) can
   silently shift which dependents satisfy `all_closed`. Capturing PRE-close
   freezes the snapshot; POST-close leaves the output dependent on uncoordinated
   races.

The POST-close → rebuild → cascade topology is a correctness defect and MUST
NOT be reintroduced. An impl that computed the cascade from the post-rebuild
cache would silently degrade the cascade list under either of the two
conditions above — neither is catchable from the cascade's return shape alone.

**Post-rebuild field-sync failure.** Step 2's cascade list is already captured
and durable in memory before the mutation; a later rebuild failure does NOT
invalidate that list. The Phase 3 field-update loop in step 6 (Status → `ready`,
unblock comment) is best-effort per the existing close semantics — individual
failures are logged and the cascade continues. However, if the step 7 rebuild
fails (transient 503 during `fetch_graph_data`, or similar) AND the step 8
`update_status_fields` cross-check cannot be performed, the tool MUST surface a
503-class error with a message instructing the caller to re-run `show` rather
than returning a response that implies the post-close Status-field fan-out is
synced. The cascade list in the response (from step 2) remains authoritative;
the error signals only that the reconciliation in step 8 could not run. The
`close` mutation is durable on GitHub regardless of this failure. Preserves §14
invariants 8 and 13 (no fictional Status-sync claims when the graph cannot be
consulted).

**Why step 2 before step 3:** PRE-close freezes the cascade snapshot against (a) the traversal-set fragility enumerated above — a POST-close rebuild carries the closed issue as `IssueState::Closed` under the widened `states: [OPEN, CLOSED]` query (§5.5), and the `Incoming` walk would include already-closed dependents that the `dependent_issue.state == Closed` CONTINUE then filters; and (b) concurrent blocker mutations between close and rebuild. See §3.4 "Critical" note for the full normative rationale.

**Why step 6 uses only two `*_ref` primitives:** Each cross-repo cascade member triggers three side effects — `fetch_issue` (to obtain `issue_node_id`), Projects V2 `update_field` (Status → `ready`), and `add_comment` (unblock note). Of these, only `fetch_issue` and `add_comment` are addressed by `(owner, repo, number)` and therefore need `*_ref` variants (`fetch_issue_ref`, `add_comment_ref`) to route cross-repo. `update_field` does NOT get an `update_field_ref` variant because `updateProjectV2ItemFieldValue` operates on globally-scoped node IDs (`project_id` + `item_id`), not on `(owner, repo, number)` — once `fetch_issue_ref` yields the cross-repo issue's node ID, `get_project_item_id(issue_node_id, project_id)` resolves the item on the configured project's board, and the existing `update_field(project_id, item_id, field_id, value)` applies the Status update directly. See §5.6 "Cascade-primitive asymmetry" for the routing rationale.

**API calls:** 0-1 (pre-close graph: 0 if cache warm, 1 if cold) + 1 (fetch) + 1 (close) + 1+ (fields) + 1 (comment) + 1+ (rebuild) + N×2 per unblocked (field + comment)
**Cache:** Invalidates.

### 8.3 `create`

```rust
pub struct CreateParams {
    pub title: String,
    pub issue_type: Option<String>,       // default: "task"
    pub priority: Option<String>,         // default: "P2"
    pub body: Option<String>,
    pub labels: Option<Vec<String>>,
    pub milestone: Option<String>,        // milestone title
    pub blocked_by: Option<Vec<String>>,  // Vec<IssueRef> — local or cross-repo
    pub parent: Option<String>,           // IssueRef
    pub story_points: Option<u32>,
    pub defer_until: Option<String>,      // ISO date
}

pub struct CreateResult {
    pub issue: IssueSummary,
}
```

**Validation:**
- `title`: non-empty, max 500 chars
- `priority`: P0–P4 if present
- `issue_type`: valid IssueType name if present
- `defer_until`: valid ISO date if present

**Flow:**
1. Create issue: REST POST
2. Add to project: `addProjectV2Item`
3. Set fields: Priority, Status → `ready` (or `blocked` if has blockers), Story Points, Defer Until
4. If `blocked_by`:
   a. For each blocker: resolve IssueRef, `would_create_cycle` check, `add_blocked_by`
   b. Update Status → `blocked`
5. If `parent`: resolve IssueRef, `add_sub_issue`
6. If `labels`: `ensure_labels` (auto-create missing) + `add_labels_to_issue`
7. If `milestone`: resolve milestone by title, `update_issue_milestone`
8. Invalidate cache + rebuild

**API calls:** 1 (create) + 1 (add to project) + 3-7 (fields) + 0-N (deps) + 0-1 (parent) + 1+ (rebuild)
**Cache:** Invalidates.

### 8.4 `depends`

```rust
pub struct DependsParams {
    pub source: String,  // IssueRef — the issue that will be blocked
    pub target: String,  // IssueRef — the issue that blocks
}

pub struct DependsResult {
    pub created: bool,
    pub source: String,
    pub target: String,
    pub message: String,
}
```

**Validation:**
- `source`: valid IssueRef
- `target`: valid IssueRef
- `source != target`

**Flow:**
1. Resolve both IssueRefs
2. Cycle detection: `would_create_cycle(source, target)` → `CircularDependency` if true
3. Duplicate check: edge already exists → `DuplicateDependency` if true
4. `add_blocked_by` mutation (or `add_blocked_by_ref` for cross-repo)
5. Update source fields: Status → `blocked`
6. Invalidate cache + rebuild

**Both `source` and `target` accept `IssueRef` format** (local `#42` or cross-repo `owner/repo#42`). See PLAN GAP-06.

**API calls:** 0-2 (resolve) + 1 (mutation) + 0-2 (fields) + 1+ (rebuild)
**Cache:** Invalidates.

### 8.5 `dep_remove`

```rust
pub struct DepRemoveParams {
    pub source: String,  // IssueRef — the currently blocked issue
    pub target: String,  // IssueRef — the currently blocking issue
}

pub struct DepRemoveResult {
    pub removed: bool,   // true iff the edge existed and was removed;
                         // false iff the pre-mutation probe proved the
                         // edge did not exist and the mutation was skipped
    pub source: String,
    pub target: String,
    pub message: String,
}
```

**Validation:**
- `source`: valid IssueRef
- `target`: valid IssueRef
- `source != target`

**Flow:**
1. Resolve both IssueRefs
2. Classify the edge via a three-outcome pre-mutation probe
   (`EdgePresence`). Uniform across all paths (warm-local,
   warm-cross-repo, cold-local, cold-cross-repo):
   a. **`Present`** — edge exists in the graph AND both endpoints are
      `IssueState::Open`. Proceed to step 3.
   b. **`EndpointClosed(qid)`** — either endpoint's `IssueState` is
      `Closed`. Source is inspected before target, so when both are
      Closed the source's `QualifiedId` is reported. The mutation is
      SKIPPED and the handler surfaces `DomainError::EndpointClosed
      { qid }` (§11.1, 409 → `INVALID_PARAMS`). The error message
      MUST name the endpoint's `QualifiedId` and tell the agent to
      `reopen` it or accept the dangling edge. Rationale: the prior
      two-outcome posture would have classified this as `Present`
      and run the mutation, but a closed endpoint's Status field is
      frozen and the rebuilt graph would diverge from the agent's
      mental model of "both sides were live when I dropped the
      edge". This is a cross-cutting contract, not just UX: it
      prevents silent drift between the graph engine and the
      Projects V2 Status field for closed nodes.
   c. **`MissingSkipMutation`** — both endpoints are Open but the
      edge does not exist. Return `DepRemoveResult { removed: false,
      ... }` WITHOUT running step 3 (honours §14 Invariant 11).
      Missing-edge is never surfaced as an error — only endpoint-Closed
      is.
3. `remove_blocked_by` mutation
4. Rebuild graph, recompute ready states
5. If source now has zero open blockers: Status → `ready`
6. Update cache

**Probe cache-mode branching (scope of the in-memory edge-existence
guard).** Flow step 2 is `EdgePresence`-uniform on outcomes, but the
mechanism that produces those outcomes is cache-mode branched and this
branching is normative:

- **Warm cache AND both endpoints `Local`** — in-memory fast path. The
  probe consults the cached graph directly (`guard_edge_exists`); no
  GraphQL round-trip is issued for the edge check. `IssueState` on the
  cached nodes disambiguates Closed endpoints from absent nodes.
- **Cold cache OR at least one endpoint cross-repo** — single-issue
  GraphQL probe. The probe issues exactly one `fetch_issue_ref` against
  the source and inspects the returned `state` + `blockedBy` list
  (`probe_edge_via_fetch`; schema as of 2026-04-30). The `blockedBy`
  subselection carries both `repository { owner { login } name }` and
  `state`, so the Closed-endpoint check needs no second round-trip
  regardless of which side is Closed.

The in-memory fast path is therefore scoped to warm-cache + both-Local
inputs; all other combinations (cold cache, cross-repo source, cross-
repo target) bypass the in-memory guard and run the single-issue
fetch-based probe instead. The three-outcome classification
(`Present` / `EndpointClosed` / `MissingSkipMutation`) is identical
across both branches — only the transport (memory vs. one GraphQL RTT)
differs. Implementers MUST NOT conflate "the in-memory guard is scoped"
with "the existence check is skipped": the existence check runs on
every path; only the *zero-RTT* form of that check is warm+both-Local.

**Error-contract row** (consumed by the cross-tool error mapping in §11.1
and the tool-handler dispatch in §8):

| Condition | Outcome | Error variant | HTTP | MCP code |
|---|---|---|---|---|
| Either endpoint is `Closed` | Mutation skipped, error surfaced | `DomainError::EndpointClosed { qid }` | 409 | `INVALID_PARAMS` |
| Edge missing (both endpoints Open) | Mutation skipped, `removed: false` | — (success) | — | — |
| Edge present (both endpoints Open) | Mutation runs | — (success) | — | — |
| Mutation ran + cache rebuild failed (cache empty) | 503-class error surfaced; mutation durable on GitHub | `unblock_github::errors::Error` (infrastructure) | 503 | `INTERNAL_ERROR` |

**Post-rebuild cache-empty failure.** If the `remove_blocked_by`
mutation in step 3 succeeds but the subsequent `execute_write_tool`
cache rebuild fails (e.g. transient GitHub 503, rate-limit, or network
error), leaving the cache empty, the handler cannot compute
`has_open_blockers` locally and therefore cannot evaluate step 5's
Status → `ready` transition. In that case the Local-source path MUST
surface a 503-class error with a message instructing the caller to
re-run `show` rather than returning a response that implies the
post-removal Status fan-out is synced. The `remove_blocked_by`
mutation is durable on GitHub regardless of this failure — the error
signals only the inability to compute the final blocker set and Status
fields locally. Preserves §14 invariants 8 and 13 (no fictional
Status-sync claims when the graph cannot actually be consulted) and
mirrors the `reopen` R3 posture in §8.7.

**API calls:** 0-2 (resolve) + 1 (mutation, only on `Present`) + 0-2 (fields) + 1+ (rebuild). `EndpointClosed` and `MissingSkipMutation` both skip the mutation and the rebuild; the warm-cache probe is purely in-memory, while the cold-cache probe may issue one `fetch_issue_ref` for cross-repo endpoint resolution.
**Cache:** Invalidates on `Present` only. `EndpointClosed` and `MissingSkipMutation` do not invalidate.

### 8.6 `update`

```rust
pub struct UpdateParams {
    pub id: u64,
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub labels_add: Option<Vec<String>>,
    pub labels_remove: Option<Vec<String>>,
    pub assignees_add: Option<Vec<String>>,
    pub assignees_remove: Option<Vec<String>>,
    pub milestone: Option<String>,
    pub story_points: Option<u32>,
    pub defer_until: Option<String>,
    pub agent: Option<String>,
    pub description: Option<String>,          // body section
    pub design_notes: Option<String>,         // body section
    pub acceptance_criteria: Option<String>,   // body section
}

pub struct UpdateResult {
    pub issue: IssueSummary,
    pub updated_fields: Vec<String>,
}
```

**Validation:**
- `id`: positive integer
- At least one field to update
- `title`: non-empty, max 500 chars if present
- `priority`: P0–P4 if present
- `status`: valid Status variant if present
- `defer_until`: valid ISO date if present
- `body` and body section params (`description`, `design_notes`, `acceptance_criteria`) are **mutually exclusive**. If `body` is provided, section-level params MUST be absent — validation rejects the call otherwise. `body` replaces the entire issue body; section params merge into the existing body via §9.3

**Flow:**
1. Fetch issue, validate not closed (unless reopening via status)
2. If body sections changed: parse existing body, merge sections (§9.3), write back
3. If REST fields changed (title, body, labels, assignees, milestone): PATCH issue
4. If Project fields changed (status, priority, agent, story_points, defer_until): `set_project_fields`
5. Invalidate cache + rebuild

**API calls:** 1 (fetch) + 0-1 (PATCH) + 0-N (field updates) + 1+ (rebuild)
**Cache:** Invalidates.

### 8.7 `reopen`

```rust
pub struct ReopenParams {
    pub id: u64,
}

pub struct ReopenResult {
    pub issue: u64,
    pub blocked: bool,
    pub status: String,
}
```

**Validation:**
- `id`: positive integer

**Flow:**
1. Fetch issue, validate `IssueState == Closed` → else `IssueNotClosed` or `IssueAlreadyOpen`
2. Reopen: REST PATCH `state: "open"`
3. Rebuild graph to evaluate blocking status
4. If issue has open blockers: Status → `blocked`
5. If no open blockers: Status → `ready`
6. Update cache

**Error-contract row** (consumed by the cross-tool error mapping in §11.1
and the tool-handler dispatch in §8):

| Condition | Outcome | Error variant | HTTP | MCP code |
|---|---|---|---|---|
| Post-rebuild re-evaluation failure (rebuild succeeded but reopened issue missing) | 503-class error surfaced; mutation durable on GitHub | `unblock_github::errors::Error` (infrastructure) | 503 | `INTERNAL_ERROR` |

**Post-rebuild re-evaluation failure.** If the rebuild succeeds but the
reopened issue cannot be located in the rebuilt graph (transient 503, or
the issue has been re-closed concurrently between steps 2 and 3), the
tool MUST surface a 503-class error with a message instructing the
caller to re-run `show` rather than defaulting `blocked` to `false`. The
`reopen` mutation is durable on GitHub regardless of this failure — the
error signals only the inability to compute the final `blocked` /
`status` fields locally. Preserves §14 invariants 8 and 13 (no fictional
Status/`blocked` claims when the graph cannot actually be consulted).

**API calls:** 1 (fetch) + 1 (reopen) + 1-2 (fields) + 1+ (rebuild)
**Cache:** Invalidates.

### 8.8 `comment`

```rust
pub struct CommentParams {
    pub id: u64,
    pub body: String,
}

pub struct CommentResult {
    pub created: bool,
}
```

**Validation:**
- `id`: positive integer
- `body`: non-empty

**Flow:**
1. `add_comment(id, body)`
2. NO cache invalidation — comments don't affect the graph

**API calls:** 1
**Cache:** NO invalidation.

### 8.9 `init`

```rust
pub struct InitParams {
    pub title: Option<String>,  // default: "UNBLOCK://{owner}/{repo}"
}

pub struct InitResult {
    pub project_number: u64,
    pub created: bool,
}
```

**Flow:**
1. Detect owner type (org vs user)
2. Check if project already exists (by title) → return existing if so
3. Create Projects V2 board via GraphQL mutation
4. Store project_id and project_number in client

**API calls:** 1-2 (detect + check) + 0-1 (create)

### 8.10 `setup`

```rust
pub struct SetupParams {
    pub project: Option<u64>,
    pub dry_run: Option<bool>,    // default: false
    pub migrate: Option<bool>,    // default: false
}

pub struct SetupResult {
    pub fields_created: Vec<String>,
    pub views_created: Vec<String>,
    pub migrated_count: Option<usize>,
}
```

**Flow:**
1. Resolve project (param or auto-detect)
2. Query existing fields
3. Create 7 missing fields (skip existing) — idempotent
4. Detect owner type (org vs user)
5. Query existing views (GraphQL)
6. Discover field IDs (REST GET /fields — integer IDs)
7. Create 5 missing views (REST POST /views) — idempotent
8. If `migrate`: add existing open issues to project, set default field values
9. Return report

**Idempotent:** safe to run multiple times. Skips existing fields and views.
**API calls:** 1 (field query) + 0-7 (create fields) + 1 (views query) + 1 (REST fields) + 0-5 (create views) + 0-N (migrate)

---

## 9. Body Section Parsing

### 9.1 `from_markdown` — parse body into sections

```
from_markdown(body) → BodySections:

  sections = { description: "", design_notes: "", acceptance_criteria: "" }
  current_section = "description"  // default before first heading

  FOR each line in body.lines():
    IF line starts with "## Description":
      current_section = "description"
      CONTINUE
    IF line starts with "## Design Notes":
      current_section = "design_notes"
      CONTINUE
    IF line starts with "## Acceptance Criteria":
      current_section = "acceptance_criteria"
      CONTINUE
    IF line starts with "## " (other heading):
      current_section = None  // unknown section — preserved as-is
      CONTINUE

    IF current_section IS SOME:
      sections[current_section].push(line)

  RETURN BodySections {
    description: trim(sections.description) or None if empty,
    design_notes: trim(sections.design_notes) or None if empty,
    acceptance_criteria: trim(sections.acceptance_criteria) or None if empty,
  }
```

### 9.2 `to_markdown` — render sections to body

```
to_markdown(sections) → String:

  parts = []
  IF sections.description is non-empty:
    parts.push("## Description\n\n{description}")
  IF sections.design_notes is non-empty:
    parts.push("## Design Notes\n\n{design_notes}")
  IF sections.acceptance_criteria is non-empty:
    parts.push("## Acceptance Criteria\n\n{acceptance_criteria}")

  RETURN parts.join("\n\n")
```

### 9.3 Merge algorithm (for `update` tool)

```
merge_sections(existing_body, updates) → String:

  current = from_markdown(existing_body)

  IF updates.description IS SOME:
    current.description = updates.description
  IF updates.design_notes IS SOME:
    current.design_notes = updates.design_notes
  IF updates.acceptance_criteria IS SOME:
    current.acceptance_criteria = updates.acceptance_criteria

  RETURN to_markdown(current)
```

### 9.4 Edge cases

- **No headings in body:** entire body is treated as description.
- **Empty sections:** a heading with no content below it → None for that section.
- **Nested headings:** `### Sub-heading` within a section → treated as section content, not a new section.
- **Unknown headings:** `## Foo` → skipped during parsing. Content under unknown headings is lost during round-trip. This is acceptable for Phase 01.

---

## 10. Status Update Algorithm

### 10.1 `update_status_fields` — after every write that invalidates cache

```
update_status_fields(state, issues, ready_set) → Result<()>:

  updates = []

  FOR each issue in issues:
    expected = compute_expected_status(issue, ready_set)
    IF issue.status != expected:
      updates.push((issue.project_item_id, expected))

  FOR each (item_id, new_status) in updates:
    state.github.update_field(project_id, item_id, status_field_id,
      SingleSelectOption(status.option_id()))

  log: "{updates.len()} Status fields synchronised"
```

### 10.2 `compute_expected_status`

```
compute_expected_status(issue, ready_set) → Status:

  IF issue.state == Closed:
    RETURN Closed

  // Preserved states — set by agent/human, never overridden by server
  IF issue.status == InProgress:
    RETURN InProgress
  IF issue.status == Deferred:
    RETURN Deferred

  // Graph-computed states
  IF issue.qualified_id IN ready_set:
    RETURN Ready
  RETURN Blocked
```

### 10.3 Edge cases

- **No changes:** if all fields match, zero API calls. Common on read-heavy workloads.
- **Issue not in project:** cannot update field. Skip with warning.
- **Batch size:** large cascades may generate many updates. Use GraphQL aliases for batching.

---

## 11. Error Model

### 11.1 Domain errors (`unblock-core/src/errors.rs`)

```rust
#[derive(Debug, Snafu)]
pub enum DomainError {
    IssueNotFound { number: u64 },
    AlreadyClaimed { number: u64, agent: String },
    IssueBlocked { number: u64, blockers: Vec<IssueRef> },
    IssueDeferred { number: u64, until: String },
    IssueClosed { number: u64 },
    IssueNotClosed { number: u64 },
    IssueAlreadyOpen { number: u64 },
    CircularDependency { source: IssueRef, target: IssueRef },
    DuplicateDependency { source: IssueRef, target: IssueRef },
    EndpointClosed { qid: QualifiedId },
    FieldNotFound { name: String },
    Validation { message: String },
    InvalidIssueRef { input: String },
    CrossRepoAccessDenied { owner: String, repo: String },
}
```

`EndpointClosed` carries a `QualifiedId` (not `IssueRef`) because it is always surfaced by `dep_remove` after both endpoints have been resolved — at that point the fully-qualified `(owner, repo, number)` is known and disambiguation is required. Rendered as `"acme/widgets#42"` (configured-repo endpoint) or `"otherowner/otherrepo#42"` (cross-repo endpoint) — the `QualifiedId::Display` impl always emits the `owner/repo#number` qualified form.

Each variant has `status_code() → u16`:

| Error | HTTP Code |
|---|---|
| `IssueNotFound` | 404 |
| `AlreadyClaimed` | 409 |
| `IssueBlocked` | 409 |
| `IssueDeferred` | 409 |
| `IssueClosed` | 409 |
| `IssueNotClosed` | 409 |
| `IssueAlreadyOpen` | 409 |
| `CircularDependency` | 422 |
| `DuplicateDependency` | 409 |
| `EndpointClosed` | 409 |
| `FieldNotFound` | 404 |
| `Validation` | 400 |
| `InvalidIssueRef` | 400 |
| `CrossRepoAccessDenied` | 403 |

`InvalidIssueRef`, `CrossRepoAccessDenied`, and the cross-repo-aware forms of `CircularDependency`/`DuplicateDependency`/`IssueBlocked` are the **error-side** half of the cross-repo contract. The successful-response half — how responses disclose cross-repo nodes that were flattened to local numbers — is specified in §11.4.

**Cross-repo-aware variant typing — Exhaustiveness Rationale (Decision 1, 2026-04-17).**

`IssueBlocked.blockers`, `CircularDependency.{source, target}`, and `DuplicateDependency.{source, target}` use `IssueRef` (§2.7) rather than bare `u64`. GitHub's native sub-issue / `blockedBy` / `addIssueDependency` graph has been cross-repo-aware since GA in 2024 (schema anchor — see §5.5): a cross-repo blocker, a cross-repo cycle participant, and a cross-repo duplicate-edge endpoint are all observable via the API and reachable from any configured repository. Bare `u64` cannot disambiguate `#42` in `configured/repo` from `#42` in `other/repo`; an error referring to the latter would silently alias to the former and mislead the agent. `IssueRef` is the unique fully-qualified-or-local carrier already used by §8.4 `depends`, §8.5 `dep_remove`, §8.3 `create.blocked_by`, and §11.4 `cross_repo_refs::omitted` — keeping §11.1 consistent with §11.4 is the closure property of the cross-repo contract. This is a BREAKING CHANGE in the `unblock-core` pub API (`DomainError` variant field types change); the implementing commit MUST carry a `BREAKING CHANGE:` footer per CLAUDE.md "Pub API Change Tracking" discipline. This rationale closes the question: no further sub-beads are needed for per-variant re-evaluation; new `DomainError` variants that carry issue references MUST default to `IssueRef` typing by the same argument.

**Display byte-for-byte preservation (local-only case).**

`IssueRef::Display` MUST render `IssueRef::Local(n)` as exactly `"#n"` (e.g. `Local(42)` → `"#42"`) so every existing `Display` snapshot at `crates/unblock-core/src/errors.rs:215-240` (and equivalent assertions elsewhere) continues to pass byte-for-byte without edits. `IssueRef::CrossRepo { owner, repo, number }` renders as `"owner/repo#number"` (e.g. `"acme/widgets#42"`), matching `QualifiedId::Display` so agents can copy-paste error text into follow-up tool calls (e.g. `show acme/widgets#42`). Concretely:

- `CircularDependency { source: IssueRef::Local(1), target: IssueRef::Local(2) }` → `"Circular dependency: adding #1 → #2 creates cycle"` (unchanged from today).
- `DuplicateDependency { source: IssueRef::Local(4), target: IssueRef::Local(5) }` → `"Blocking relationship already exists: #4 → #5"` (unchanged from today).
- `IssueBlocked { number: 10, blockers: vec![IssueRef::Local(1), IssueRef::Local(2)] }` → MUST still include the substrings `"10"`, `"1"`, and `"2"` (the existing test at `errors.rs:170-174` asserts only substring containment, so `"Issue #10 is blocked by: [#1, #2]"` and `"Issue #10 is blocked by: #1, #2"` are both acceptable formats; the implementation chooses one and commits to it with a test).
- Cross-repo example: `IssueBlocked { number: 10, blockers: vec![IssueRef::CrossRepo { owner: "acme".into(), repo: "widgets".into(), number: 1 }] }` renders with `"acme/widgets#1"` in the blocker list.

The implementation MAY route `IssueRef::Display` through `#[snafu(display(...))]` directly (via `{source}` / `{target}` interpolation that calls `Display`) or pre-format the blocker list with a helper; both satisfy the preservation contract.

**Implementer trap (Debug vs. Display in the existing `IssueBlocked` attribute).** The current `#[snafu(display(...))]` attribute at `crates/unblock-core/src/errors.rs:41` is `"Issue #{number} is blocked by: {blockers:?}"` — the `{blockers:?}` specifier is the Debug formatter, which under `Vec<u64>` renders `[1, 2]` and under `Vec<IssueRef>` renders `[Local(1), Local(2)]` (the `IssueRef` variant names leak into the output). This variant-leaking Debug output satisfies the current substring test at `crates/unblock-core/src/errors.rs:170-174` only because that test asserts `"10"` (the issue number); a future tightening of the test to assert `"#1"` or `"#2"` would silently break. The implementation of this variant MUST replace the `{blockers:?}` Debug attribute with a Display-based renderer — either a format string that interpolates `IssueRef::Display` (e.g. a joined helper) or a pre-formatted blocker list via a helper function that iterates and calls `IssueRef`'s `Display` impl. This is not a contract change; it is an implementer trap flagged so the Display-preservation contract above is not silently violated by leaving the Debug formatter in place.

### 11.2 Infrastructure errors (`unblock-github/src/errors.rs`)

```rust
#[derive(Debug, Snafu)]
pub enum Error {
    Domain { source: DomainError },
    GitHubApi { message: String },
    GitHubGraphQL { errors: Vec<String> },
    GitHubUnavailable { source: reqwest::Error },
    GitHubServerError { status: u16, message: String },
    RateLimited,
    CircuitBreakerOpen,           // stub — active in Phase 02
    ProjectNotConfigured,
    GitRemote { message: String },
    ViewCreationFailed { message: String },
    OwnerDetectionFailed { owner: String, message: String },
}
```

**Error classification:**

| HTTP Status | Error variant | Retryable (Phase 02) |
|---|---|---|
| Network error | `GitHubUnavailable` | Yes |
| 429 | `RateLimited` | Yes |
| 500 | `GitHubServerError` | No |
| 502 | `GitHubServerError` | No |
| 503 | `GitHubServerError` | Yes |
| 4xx (except 429) | `GitHubApi` | No |

### 11.3 MCP error mapping (`unblock-mcp/src/errors.rs`)

```
github_error_to_mcp(err) → ErrorData:

  Domain errors     → code: -32602 (invalid params / business rule)
  Infrastructure    → code: -32603 (internal error / GitHub)
```

Propagation chain: `DomainError` (core) → `Error` (github) → `McpError` (mcp).

### 11.4 Cross-Repo Response Contract

The graph engine nodes are `QualifiedId { owner, repo, number }` (§2.1). Many response types project cross-repo nodes down to bare `u64` issue numbers scoped to the configured repository. When a computation touches one or more `QualifiedId` nodes whose `(owner, repo)` differs from the configured repo AND those nodes are dropped from the bare-`u64` projection of the response, the response MUST surface them in an explicit `cross_repo_refs` field. This is the dual of the error-side contract in §11.1: §11.1 governs how cross-repo failures are reported (`InvalidIssueRef`, `CrossRepoAccessDenied`, and the `IssueRef`-typed forms of `CircularDependency` / `DuplicateDependency` / `IssueBlocked`); §11.4 governs how successful responses disclose cross-repo nodes that were flattened to local numbers.

**Shared type** (`unblock-core/src/types.rs`):

```rust
/// Cross-repo references that participated in a response computation but were
/// dropped from the local `u64` projection of that response.
///
/// Populated when a tool returns issue numbers scoped to the configured repo
/// but the underlying graph traversal touched nodes in other repositories.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CrossRepoRefs {
    /// Qualified refs omitted from the bare-`u64` projection, one per line.
    /// Each entry uses `QualifiedId::Display` → `"owner/repo#number"`.
    pub omitted: Vec<String>,
    /// Human-readable summary for agent consumption.
    /// Example: `"2 cross-repo cycle members omitted from `cycles`"`.
    pub summary: Option<String>,
}
```

**Response integration.** Every tool response affected by the contract adds:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub cross_repo_refs: Option<CrossRepoRefs>,
```

**Population rules.** The field is populated (i.e. `Some`) iff BOTH of the following hold:

1. The computation backing the response visited ≥1 `QualifiedId` whose `(owner, repo)` differs from the configured `(config.owner, config.repo)`.
2. That same node was NOT emitted in the bare-`u64` projection of the response (because bare `u64` cannot disambiguate across repos).

When either condition fails, the field is omitted from the JSON response (`#[serde(skip_serializing_if = "Option::is_none")]`). The field is NEVER `Some` with an empty `omitted` vector.

**Rendering.** `omitted` entries use `QualifiedId::Display` (§2.1): `"owner/repo#number"`. This format is stable, human-readable, and parseable back into `IssueRef::CrossRepo` (§2.7) for follow-up tool calls (e.g. `show owner/repo#42`).

**Markdown adaptation (`prime`).** Tools that return markdown instead of a typed struct (§7.3 `prime`) render the same information as a trailing section:

```
## Cross-repo references
- `owner/repo#42`
- `owner/repo#99`

_{summary}_
```

The section is omitted entirely when `cross_repo_refs` would be `None` under the typed-response rules.

**Affected tools.** The following §7/§8 tools MUST implement this contract:

| Tool | Section | Projection that drops cross-repo info |
|---|---|---|
| `ready` | §7.1 | Cross-repo blockers silently exclude local issues from the ready set. Source issues are guaranteed LOCAL-ONLY by §3.3 Filter 3 (unblock-eos.4 scrub); `cross_repo_refs` carries blockers only, never cross-repo sources. |
| `prime` | §7.3 | Cycle summary lists issue numbers |
| `dep_cycles` | §7.7 | `cycles: Vec<Vec<u64>>` drops cross-repo cycle members |
| `close` | §8.2 | `unblocked: Vec<u64>` drops cross-repo dependents |

Tools explicitly NOT affected (documented here to pre-empt retro-adoption questions):

| Tool | Rationale |
|---|---|
| `show` (§7.2) | `TreeNode.id: QualifiedId` already fully qualified (§2.14) |
| `stats` (§7.4) | Aggregate counts only, no issue IDs in response |
| `list` (§7.5) | Scoped to configured repo; cross-repo issues never enumerated |
| `search` (§7.6) | GitHub Search query pinned to `repo:{owner}/{repo}` |
| `claim` / `create` / `update` / `reopen` (§8.1, §8.3, §8.6, §8.7) | Mutations scoped to configured repo (§5.6 cross-repo scope table) |
| `depends` / `dep_remove` (§8.4, §8.5) | Request and response use `IssueRef` strings; no `u64` projection |
| `comment` (§8.8) | Boolean response only |
| `init` / `setup` (§8.9, §8.10) | Project-level, no issue references |

**Exhaustiveness Rationale — response-shape universality (Decision 3, 2026-04-17).**

The `cross_repo_refs: Option<CrossRepoRefs>` field is NOT a universal response contract; it applies to exactly the four tools listed in the affected-tools table above — `ready` (§7.1), `prime` (§7.3), `dep_cycles` (§7.7), `close` (§8.2) — and no others. The axiom that derives the affected set is §5.6 "Cross-repo scope": a tool qualifies iff (a) its response projects node identity down to a bare `u64` AND (b) §5.6 permits cross-repo traversal to touch nodes that would be flattened by that projection. The exempt tools listed above each fail at least one leg of the conjunction for a structural reason documented in their row, not by accident of implementation:

- `show` has bare-`u64` fields in the response projection — `ShowIssue.number` (`crates/unblock-mcp/src/tools/show.rs:73`) and `ShowRelatedIssue.number` (`crates/unblock-mcp/src/tools/show.rs:131`) are `u64`, so leg (a) holds. The exemption is on leg (b): per §5.6 "Cross-repo scope", `show`'s traversal (sub-issues + `Issue.blockedBy`) is scoped to the configured repo, so no cross-repo node ever reaches the bare-`u64` projection.
- `stats` emits no issue IDs at all, so (a) fails.
- `list` / `search` / `claim` / `create` / `update` / `reopen` / `comment` / `init` / `setup` are scoped by §5.6 to the configured repo on the traversal side, so (b) fails.
- `depends` / `dep_remove` round-trip `IssueRef` strings (never `u64`) on both request and response, so (a) fails.

Because the derivation is mechanical from §5.6 + the §7/§8 response typing, future tools inherit the exemption rule automatically: a new tool requires `cross_repo_refs` iff it independently satisfies both (a) and (b). No standalone bead is needed to audit tool-by-tool; the test is applied as tools are specified. This rationale closes the question raised during unblock-eos arbitration (2026-04-17) — the four-tool set is complete and frozen for Phase 01. Tools added in later phases re-evaluate (a)+(b) on their own spec entries and do NOT re-open this decision.

**Determinism.** `omitted` MUST be sorted lexicographically by `QualifiedId::Display` so identical graph state produces identical responses (per Invariant 5, §14).

---

## 12. Configuration

### 12.1 Environment variables

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `GITHUB_TOKEN` | Yes | — | Authentication (PAT) |
| `GITHUB_API_URL` | No | `https://api.github.com` | GHE support |
| `GITHUB_URL` | No | `https://github.com` | GHE support |
| `UNBLOCK_REPO` | No | Auto-detect from git remote | Repository `owner/repo` |
| `UNBLOCK_PROJECT` | No | Auto-detect from linked projects | Project number |
| `UNBLOCK_AGENT` | No | `"agent"` | Default agent name |
| `UNBLOCK_CACHE_TTL` | No | `30` | Cache TTL in seconds |
| `UNBLOCK_LOG_LEVEL` | No | `"info"` | Log level |

### 12.2 `Config` struct

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
}
```

`Config::load_from(env: impl Fn(&str) → Result<String, VarError>) → Result<Self, DomainError>`

No config file. Environment variables only. The `load_from` pattern accepts a custom env reader — tests supply `HashMap`-backed closures (no `std::env::set_var` — unsafe in edition 2024).

### 12.3 Token handling

- `GITHUB_TOKEN` loaded from environment only
- Never logged (redacted in debug output)
- Never included in MCP tool responses
- Never embedded in binary

### 12.4 Input validation

| Field | Validation |
|---|---|
| Issue numbers | Positive integers |
| Titles | Non-empty, max 500 chars |
| Agent names | Non-empty, max 100 chars |
| Priority | Must be P0–P4 |
| Dates | Valid ISO format |

---

## 13. Testing Strategy

### 13.1 Test layers

| Crate | Type | What | GitHub Required |
|---|---|---|---|
| `unblock-core` | Unit | Graph engine, cache, types, config | No |
| `unblock-core` | Property | Graph invariants (proptest) | No |
| `unblock-github` | Unit | Error conversion, URL construction | No |
| `unblock-github` | Integration | Wiremock-based API tests | No |
| `unblock-mcp` | Unit | Body section parsing, error conversion | No |
| `unblock-mcp` | Integration | Full tool flows with `MockGitHubClient` | No |
| `unblock-mcp` | E2E | Full agent loop | Yes (optional) |

### 13.2 Quality gate

```bash
cargo fmt --check --all                                    # zero diffs
cargo clippy --workspace --all-targets -- -D warnings      # zero warnings
cargo test --workspace                                     # all pass
cargo doc --no-deps --workspace                            # zero warnings
```

Coverage target: >80% for Phase 01.

### 13.3 Property tests

```rust
proptest! {
    #[test]
    fn ready_set_never_contains_blocked_issues(
        issues in vec(arb_issue(), 1..100),
        edges in vec(arb_edge(), 0..200),
    ) {
        let graph = DependencyGraph::build(&issues, &edges);
        // Post-eos.4 signature (§3.3 Filter 3 / §14 Invariant 14(a)):
        // `compute_ready_set` takes the configured (owner, repo) so that
        // cross-repo source issues are scrubbed before blocker traversal.
        // Generator `arb_issue()` produces issues in the configured repo so
        // Filter 3 is a no-op here; Invariant 14(a) is exercised by #7 below.
        let ready = graph.compute_ready_set(&issues, "owner", "repo");
        for issue in &ready {
            // No issue in ready set has an open blocker
            let blockers = graph.get_blockers(&issue.qualified_id);
            for blocker in blockers {
                assert_eq!(graph.issue_state()[&blocker], IssueState::Closed);
            }
        }
    }
}
```

Graph invariants:
1. Ready set never contains blocked issues
2. Cascade is sound (all newly unblocked dependents appear)
3. Cycle detection is sound and complete
4. Ready set is deterministic (same input → same output)
5. Graph construction is idempotent
6. Cross-repo response contract is complete (§14 Invariant 14(b)): for every §11.4-affected tool, every cross-repo node that was dropped during bare-`u64` projection appears in `cross_repo_refs.omitted`, sorted.
7. Ready set is configured-repo-source-scoped (§14 Invariant 14(a)): for any input mixing issues from the configured repo with cross-repo source issues, `compute_ready_set(issues, configured_owner, configured_repo)` returns zero elements whose `qualified_id.(owner, repo)` differs from `(configured_owner, configured_repo)`. Drives the unblock-eos.4 graph-engine scrub.

### 13.4 `test-hooks` feature

`#[cfg(feature = "test-hooks")]` gates test-only code paths:
- `MockGitHubClient` in `unblock-github/src/mock.rs`
- `set_project_fields()` helpers
- Any test-only mutation methods

Never enabled in production builds.

### 13.5 Required tests per tool

Every tool MUST have at least one integration test with `MockGitHubClient` covering:
- Happy path
- Primary error case

---

## 14. Invariants

These invariants MUST hold at all times. Property tests validate where applicable.

1. **Ready set never contains blocked issues.** No issue in the ready set has an open blocker in the graph.
2. **Cascade is sound.** After closing an issue, every dependent whose blockers are all now closed appears in the cascade result.
3. **Cycle detection is sound.** If `detect_all_cycles()` returns empty, no cycle exists.
4. **Cycle detection is complete.** If a cycle exists, `detect_all_cycles()` finds it.
5. **Ready set is deterministic.** Same input → same output. Sorting by priority ASC → created_at ASC.
6. **Cache is reconstructable.** Deleting the cache and rebuilding produces the same graph.
7. **Graph construction is idempotent.** Same input data → same graph.
8. **Every write invalidates + rebuilds + updates fields.** No write tool leaves cache or Status fields inconsistent. Exception: `comment` (no graph impact).
9. **`show` is always fresh.** Never served from cache.
10. **`search` bypasses cache.** Uses GitHub Search API directly.
11. **Validation before mutation.** All tools validate input before calling GitHub. No partial mutations from validation failures.
12. **Token never logged.** Redacted in all debug output. Never in MCP responses.
13. **Status field values match graph computation.** After every write, `update_status_fields` syncs the Projects V2 Status field with the graph-computed expected status.
14. **Cross-repo response contract is complete (§11.4).** Two clauses, both MUST hold:
    - **14(a) — Configured-repo source scoping (graph engine).** `compute_ready_set` (§3.3) returns a `Vec<IssueSummary>` in which every element satisfies `qualified_id.(owner, repo) == (configured_owner, configured_repo)`. The ready set contains only configured-repo source issues. This is enforced at the graph engine (§3.3 Filter 3) as the single chokepoint — every downstream consumer (cached `ready_set`, `ready` tool, `prime`, `update_status_fields`) inherits the guarantee. Cross-repo source issues are NEVER members of the local ready-set projection regardless of their blocker state. Property tests MUST cover: mixed-repo input → only configured-repo issues in the output.
    - **14(b) — Affected-tools response shape.** For every tool listed in the §11.4 affected-tools table, if the computation visited a cross-repo `QualifiedId` that was NOT emitted in the bare-`u64` projection of the response, the response MUST carry that node's `QualifiedId::Display` form in `cross_repo_refs.omitted`. The field is `Some` iff `omitted` is non-empty. `omitted` is sorted lexicographically (preserves Invariant 5). For `ready` specifically, combining 14(a) with 14(b) means `cross_repo_refs` may carry cross-repo BLOCKERS only — cross-repo sources are already excluded by the graph engine.

---

*This spec defines everything needed to implement Phase 01 (v0.1.0). The governing principles are in the [MANIFESTO](../MANIFESTO.md). The product scope is in the [PRD](../PRD.md). The full technical architecture is in the [SPEC](../SPEC.md). The implementation plan and gap analysis are in the [Phase 01 Plan](../plans/01-plan-mcp-foundation.md).*
