# Spec 01 — Graph Engine & Cache Layer

> Companion: [SPEC §3–4](../SPEC.md#3-graph-engine) · Plans: [01-mcp-foundation](../plans/01-plan-mcp-foundation.md) · [02-mcp-complete](../plans/02-plan-mcp-complete.md)  
> Crate: `unblock-core`  
> Status: draft  
> Last updated: 2026-04-10

---

## Table of Contents

1. [Scope](#1-scope)
2. [Types](#2-types)
3. [Graph Construction Algorithm](#3-graph-construction-algorithm)
4. [Ready Set Calculation Algorithm](#4-ready-set-calculation-algorithm)
5. [Cascade Algorithm](#5-cascade-algorithm)
6. [Cycle Detection Algorithms](#6-cycle-detection-algorithms)
7. [Dependency Tree Traversal](#7-dependency-tree-traversal)
8. [Reconciliation Engine Algorithm](#8-reconciliation-engine-algorithm)
9. [Cache Lifecycle](#9-cache-lifecycle)
10. [Fast Path Algorithm (Phase 03)](#10-fast-path-algorithm-phase-03)
11. [Error Catalogue](#11-error-catalogue)
12. [Invariants](#12-invariants)
13. [Open Questions](#13-open-questions)

---

## 1. Scope

This spec defines the **algorithms, edge cases, and invariants** for the graph engine (`unblock-core/src/graph.rs`), cache layer (`unblock-core/src/cache.rs`), and reconciliation engine (`unblock-core/src/reconcile.rs`).

**In scope:** graph construction from GitHub data, ready set calculation, cascade on close, cycle detection (pre-mutation and full), dependency tree traversal, reconciliation drift analysis, cache lifecycle, fast path serving.

**Out of scope:** GitHub API queries (→ [02-spec-github-client.md](./02-spec-github-client.md)), MCP tool handler logic (→ [03-spec-mcp-tools.md](./03-spec-mcp-tools.md)), network I/O.

---

## 2. Types

### 2.1 `QualifiedId`

```rust
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct QualifiedId {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}
```

All graph operations use `QualifiedId`. Never plain `u64`. Prevents collision between issue #5 in repo A and #5 in repo B. Display format: `owner/repo#number`. Short format for local repo: `#number`.

### 2.2 `IssueRef`

```rust
pub enum IssueRef {
    Local(u64),
    CrossRepo { owner: String, repo: String, number: u64 },
}
```

Parsed user input. `IssueRef::Local(42)` resolves to `QualifiedId { owner, repo, number: 42 }` using configured repo context. Parsing: `#42` or `42` → `Local`. `owner/repo#42` → `CrossRepo`.

### 2.3 `DependencyGraph`

```rust
pub struct DependencyGraph {
    graph: DiGraph<QualifiedId, ()>,
    node_map: HashMap<QualifiedId, NodeIndex>,
    issue_status: HashMap<QualifiedId, Status>,
    issue_state: HashMap<QualifiedId, IssueState>,
}
```

`petgraph::graph::DiGraph` with `QualifiedId` nodes. Edge direction: `blocked_issue → blocking_issue` (source depends on target). The graph may span multiple repositories.

### 2.4 `BlockingEdge`

```rust
#[derive(Debug, Clone)]
pub struct BlockingEdge {
    pub source: QualifiedId,   // the blocked issue
    pub target: QualifiedId,   // the blocking issue
}
```

### 2.5 `Status` and `IssueState`

```rust
pub enum Status { Open, InProgress, Blocked, Deferred, Closed }
pub enum IssueState { Open, Closed }
pub enum ReadyState { Ready, Blocked, NotReady, Closed }
```

`Status` is the Projects V2 custom field (workflow state). `IssueState` is GitHub's native binary state. Both are needed: an issue can have `Status::Open` and `IssueState::Open`, or `Status::InProgress` and `IssueState::Open`.

### 2.6 `TreeNode`

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
    pub upstream: Vec<TreeNode>,
    pub downstream: Vec<TreeNode>,
}

pub enum TraversalDirection { Upstream, Downstream, Both }
```

---

## 3. Graph Construction Algorithm

### 3.1 Steps

```
build(issues: &[Issue], edges: &[BlockingEdge]) → DependencyGraph:

  1. graph = DiGraph::new()
     node_map = HashMap::new()
     issue_status = HashMap::new()
     issue_state = HashMap::new()

  2. FOR each issue in issues:
     a. qid = issue.qualified_id.clone()
     b. idx = graph.add_node(qid.clone())
     c. node_map.insert(qid.clone(), idx)
     d. issue_status.insert(qid.clone(), issue.status)
     e. issue_state.insert(qid, issue.state)

  3. FOR each edge in edges:
     a. source_idx = node_map.get(&edge.source)
     b. target_idx = node_map.get(&edge.target)
     c. IF both exist:
          graph.add_edge(source_idx, target_idx, ())
        ELSE:
          tracing::warn!("Skipping edge with unknown node: {edge:?}")
          // Orphaned edge — target issue may be deleted or inaccessible.
          // Reconcile detects this as OrphanedBlockingEdge.

  4. RETURN DependencyGraph { graph, node_map, issue_status, issue_state }
```

### 3.2 Edge cases

- **Missing target node:** A blocking edge references an issue not in the `issues` list (deleted, or cross-repo token lacks access). The edge is skipped with a warning. `reconcile` detects this as `OrphanedBlockingEdge`.
- **Duplicate edges:** `petgraph::DiGraph` allows parallel edges. However, GitHub's blocking API prevents duplicates at the source. If duplicates appear (data race), they are harmless — the graph algorithms handle them correctly.
- **Self-edges:** A→A. Should never appear (GitHub rejects self-blocking). If present, cycle detection catches it.
- **Empty graph:** Zero issues → empty graph. `compute_ready_set()` returns empty. Valid state.

---

## 4. Ready Set Calculation Algorithm

### 4.1 Steps

```
compute_ready_set(graph, issues) → Vec<IssueSummary>:

  ready = []

  FOR each (qid, issue) in issues:
    IF issue.state == Closed:
      CONTINUE

    IF issue.status != Open:
      CONTINUE
      // InProgress, Blocked, Deferred, Closed — not eligible for ready

    // Check all blockers
    IF qid IN node_map:
      idx = node_map[qid]
      blockers = graph.neighbors_directed(idx, Outgoing)
      // Edge direction: blocked → blocker, so Outgoing = "what blocks me"

      all_blockers_closed = TRUE
      FOR each blocker_idx in blockers:
        blocker_qid = graph[blocker_idx]
        IF issue_state[blocker_qid] != Closed:
          all_blockers_closed = FALSE
          BREAK

      IF NOT all_blockers_closed:
        CONTINUE

    // Issue is ready
    ready.push(IssueSummary::from(issue))

  RETURN ready
```

### 4.2 Post-filters (applied in tool layer, NOT in graph engine)

- **Defer Until:** `issue.defer_until > today` → exclude. The graph does not know about dates.
- **Agent filter:** `ready --agent coder` → only issues with matching agent field. Applied after ready set computation.
- **Type filter:** `ready --type bug` → only bugs. Applied after.
- **Priority filter:** `ready --priority P0` → only P0. Applied after.
- **Milestone filter:** `ready --milestone "Sprint 1"` → only that milestone. Applied after.
- **Include claimed:** By default, `Status::InProgress` issues are excluded. `--include_claimed` overrides.

### 4.3 Sorting

Default sort: `priority ASC` (P0 first) → `created_at ASC` (oldest first). Deterministic ordering for agent consistency.

### 4.4 Edge cases

- **Issue not in graph:** An issue exists in GitHub but has no blocking edges (not in `node_map`). It has zero blockers → ready if `Status == Open` and `IssueState == Open`.
- **All blockers closed:** Every outgoing edge leads to a closed issue → ready.
- **Mixed blockers:** Some closed, some open → blocked.
- **Circular dependency:** Issues in a cycle are never ready (they always have an open blocker). `dep_cycles` detects and reports.

---

## 5. Cascade Algorithm

### 5.1 Steps

```
compute_unblock_cascade(graph, closed_qid, issues) → Vec<QualifiedId>:

  IF closed_qid NOT IN node_map:
    RETURN []

  idx = node_map[closed_qid]
  unblocked = []

  // Find all issues that depend on the closed issue
  dependents = graph.neighbors_directed(idx, Incoming)
  // Incoming = "what depends on me"

  FOR each dependent_idx in dependents:
    dependent_qid = graph[dependent_idx]
    dependent_issue = issues[dependent_qid]

    IF dependent_issue.state == Closed:
      CONTINUE

    // Check if ALL blockers of this dependent are now closed
    blockers = graph.neighbors_directed(dependent_idx, Outgoing)
    all_closed = TRUE
    FOR each blocker_idx in blockers:
      blocker_qid = graph[blocker_idx]
      // The just-closed issue counts as closed
      IF blocker_qid == closed_qid:
        CONTINUE
      IF issue_state[blocker_qid] != Closed:
        all_closed = FALSE
        BREAK

    IF all_closed:
      unblocked.push(dependent_qid)

  RETURN unblocked
```

### 5.2 Cascade actions (in tool handler, not engine)

For each unblocked issue:
1. Update `Status` → `open` (if currently `blocked`)
2. Update `Ready State` → `ready`
3. Add comment: "Unblocked — blocker #N was closed"

### 5.3 Edge cases

- **Multi-level cascade:** Closing A unblocks B. B is already `Status::Open` but was blocked. Cascade does NOT recursively process B's dependents in the same pass. B becomes ready; when B is later closed, its own cascade fires.
- **Partial unblock:** A depends on B and C. B is closed. A is not yet unblocked because C is still open. When C closes, cascade fires and A is unblocked.
- **Already open:** A was marked `Status::Open` manually but was still blocked. Cascade detects it's now unblocked and updates `Ready State` → `ready`.
- **Closed dependent:** A depends on B. A is already closed. When B closes, cascade skips A (already closed).

---

## 6. Cycle Detection Algorithms

### 6.1 Pre-mutation check: `would_create_cycle`

```
would_create_cycle(graph, source, target) → bool:

  // Adding edge source → target means "source depends on target"
  // A cycle exists if target already depends on source (path target → source)

  IF source == target:
    RETURN TRUE  // self-loop

  // Check if there's a path from target to source in the current graph
  RETURN has_path_connecting(graph, node_map[target], node_map[source])
```

Uses `petgraph::algo::has_path_connecting`. O(V+E) worst case. Called before `addBlockedBy` in GitHub — prevents cycles from forming.

### 6.2 Full detection: `detect_all_cycles`

```
detect_all_cycles(graph) → Vec<Vec<QualifiedId>>:

  // Tarjan's SCC algorithm
  sccs = tarjan_scc(&graph)

  cycles = []
  FOR each scc in sccs:
    IF scc.len() > 1:
      // Multi-node SCC = cycle
      cycle_qids = scc.iter().map(|idx| graph[idx].clone()).collect()
      cycles.push(cycle_qids)
    ELSE IF scc.len() == 1:
      idx = scc[0]
      IF graph.contains_edge(idx, idx):
        // Self-loop
        cycles.push(vec![graph[idx].clone()])

  RETURN cycles
```

Uses `petgraph::algo::tarjan_scc`. O(V+E). Returns all strongly connected components with size > 1, plus self-loops.

### 6.3 Edge cases

- **No cycles:** Returns empty vec. DAG is valid.
- **Self-loop:** A→A. Detected as SCC of size 1 with self-edge.
- **Two-node cycle:** A→B, B→A. Detected as SCC {A, B}.
- **Complex cycle:** A→B→C→A with D→B (D is not in the cycle). SCC returns {A, B, C}. D is a separate SCC of size 1.
- **Disconnected graph:** Each connected component is analysed independently. Cycles in one component don't affect another.

---

## 7. Dependency Tree Traversal

### 7.1 Algorithm: BFS

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


bfs_tree(graph, start, direction, max_depth) → Vec<TreeNode>:

  visited = HashSet::new()
  queue = [(start_idx, 0)]  // (node, depth)
  result = []

  WHILE queue is not empty:
    (current_idx, depth) = queue.pop_front()

    IF depth > max_depth:
      CONTINUE
    IF current_idx IN visited:
      CONTINUE
    visited.insert(current_idx)

    neighbors = graph.neighbors_directed(current_idx, direction)
    children = []
    FOR each neighbor_idx in neighbors:
      IF neighbor_idx NOT IN visited:
        queue.push_back((neighbor_idx, depth + 1))
        children.push(neighbor_idx)

    IF current_idx != start_idx:
      result.push(TreeNode {
        id: graph[current_idx],
        status: issue_status[graph[current_idx]],
        state: issue_state[graph[current_idx]],
        depth,
        children: [],  // filled by caller
      })

  RETURN result
```

### 7.2 Edge cases

- **Max depth reached:** Traversal stops. Prevents runaway on deep graphs.
- **Cycle in graph:** `visited` set prevents infinite loops. The tree representation truncates at the revisited node.
- **Root not in graph:** Returns empty tree (no upstream, no downstream).
- **Default max_depth:** 10. Configurable per-call.

---

## 8. Reconciliation Engine Algorithm

### 8.1 Overview

The `ReconcileEngine` is pure — no I/O, no async. It receives pre-fetched data and returns a `DriftReport`. 7 drift types detected in a single pass.

### 8.2 Analysis algorithm

```
analyse(graph, issues, computed_ready_set, now) → DriftReport:

  drift = []

  // Pass 1: Stale Ready State fields
  FOR each (qid, issue) in issues:
    IF issue.state == Closed:
      IF issue.ready_state != Closed:
        drift.push(StaleReadyState { qid, field_says: issue.ready_state, graph_says: Closed })
      CONTINUE

    expected = IF qid IN computed_ready_set THEN Ready ELSE Blocked
    IF issue.ready_state != expected:
      drift.push(StaleReadyState { qid, field_says: issue.ready_state, graph_says: expected })

  // Pass 2: Uncascaded closures
  FOR each (qid, issue) in issues:
    IF issue.state == Closed:
      should_unblock = compute_unblock_cascade(graph, qid, issues)
      should_unblock = should_unblock.filter(|id|
        issues[id].ready_state != Ready AND issues[id].state == Open
      )
      IF NOT should_unblock.is_empty():
        drift.push(UncascadedClosure { closed_issue: qid, should_have_unblocked: should_unblock })

  // Pass 3: Orphaned blocking edges
  FOR each edge in graph.all_edges():
    IF edge.target NOT IN issues:
      drift.push(OrphanedBlockingEdge { source: edge.source, missing_target: edge.target })

  // Pass 4: Cycles
  FOR each cycle in graph.detect_all_cycles():
    drift.push(CycleDetected { cycle })

  // Pass 5: Stale claims
  FOR each (qid, issue) in issues:
    IF issue.status == InProgress AND issue.claimed_at IS SOME:
      hours = (now - issue.claimed_at).num_hours()
      IF hours > stale_claim_threshold_hours:
        drift.push(StaleClaim { qid, claimed_at: issue.claimed_at, hours_stale: hours })

  // Pass 6: Malformed agent fields
  FOR each (qid, issue) in issues:
    IF issue.agent IS SOME AND NOT issue.agent.contains(':') AND NOT issue.agent.is_empty():
      drift.push(MalformedAgentField { qid, raw_value: issue.agent })

  // MissingProjectField is detected in the tool handler, not the engine
  // (requires ProjectFieldIds which is a GitHub concern, not domain)

  RETURN DriftReport {
    repo: "",  // filled by handler
    reconciled_at: now,
    issues_scanned: issues.len(),
    edges_scanned: graph.edge_count(),
    drift_found: drift,
    repaired: [],
    errors: [],
    clean: drift.is_empty(),
  }
```

### 8.3 Repair rules (in tool handler)

| Drift type | Auto-repairable | Repair action |
|---|---|---|
| `StaleReadyState` | ✅ | Update Ready State field via `update_field()` |
| `UncascadedClosure` | ✅ | Update Ready State to `Ready` for downstream + audit comment |
| `OrphanedBlockingEdge` | ❌ | Log warning — edge source issue needs manual review |
| `MalformedAgentField` | ❌ | Log warning — agent or human corrects format |
| `MissingProjectField` | ❌ | Log error — run `setup` to recreate |
| `CycleDetected` | ❌ | Log error — manual edge removal required |
| `StaleClaim` | ❌ | Log warning — agent or human decides (unclaim or continue) |

---

## 9. Cache Lifecycle

### 9.1 `GraphCache` state machine

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

### 9.2 Operations

| Method | Precondition | Effect |
|---|---|---|
| `get()` | — | Returns `Fresh`, `Stale`, or `Empty` |
| `update(graph)` | — | Replaces entry, resets `built_at` to now |
| `invalidate()` | — | Clears entry → `Empty` |
| `is_fresh()` | — | `built_at + ttl > now` AND entry exists |

### 9.3 TTL semantics

Default: 30 seconds. Configurable via `UNBLOCK_CACHE_TTL`.

- `Fresh`: entry exists, `built_at + ttl > now`. Zero API calls — serve directly.
- `Stale`: entry exists, `built_at + ttl <= now`. Caller MUST rebuild unconditionally. The stale data is NOT served.
- `Empty`: no entry. Cold start, post-invalidation, or first use.

### 9.4 Invariant

> Every field in `CacheEntry` is reconstructable from GitHub with a single `fetch_graph_data()` call. The cache is a performance optimisation, not a source of truth.

### 9.5 Concurrency

`RwLock<Option<CacheEntry>>`. Multiple readers concurrent. Single writer exclusive. Acceptable for single-process architecture. Last writer wins — no optimistic locking.

---

## 10. Fast Path Algorithm (Phase 03)

### 10.1 Motivation

Cold start requires `fetch_graph_data()` → build graph → compute ready set. For repos with 500+ issues, this takes 2-4 seconds. The fast path serves the ready queue immediately from Projects V2 field values.

### 10.2 Algorithm

```
handle_ready_with_fast_path(state, params) → ReadyOutput:

  MATCH state.cache.get():
    Fresh(entry):
      // Normal path — serve from graph
      RETURN filter_and_sort(entry.ready_set, params)

    Stale(entry):
      // Rebuild required
      rebuild_and_serve(state, params)

    Empty:
      // Fast path — cold start
      // 1. Serve from field values (approximate)
      field_ready = state.github.fetch_ready_from_field()
      // 2. Background rebuild
      tokio::spawn(rebuild_graph_async(state.clone()))
      // 3. Return fast path result
      RETURN ReadyOutput {
        issues: filter_and_sort(field_ready, params),
        source: FastPathSource::Field,
        stale: false,
      }
```

### 10.3 Accuracy

The fast path result is **approximate**. It reflects the last time the MCP server (or `reconcile`) wrote the Ready State field. It may be stale if:
- A human closed a blocker via GitHub UI (cascade didn't fire)
- A human removed a blocking edge via GitHub UI
- External mutations changed the graph without the MCP server running

The `source: Field` marker tells the agent the result is approximate. The background graph rebuild replaces it with authoritative data within seconds.

### 10.4 Invariant

> The fast path NEVER writes to GitHub. It is read-only. Graph computation in the background may trigger Ready State field updates.

---

## 11. Error Catalogue

| Error | Trigger | Crate |
|---|---|---|
| `IssueNotFound { number }` | Issue number not in graph or GitHub | core |
| `CircularDependency { source, target }` | `would_create_cycle` returns true | core |
| `DuplicateDependency { source, target }` | Edge already exists | core |
| `IssueBlocked { number, blockers }` | Claim attempt on blocked issue | core |
| `IssueClosed { number }` | Operation on closed issue | core |
| `IssueNotClosed { number }` | Reopen on non-closed issue | core |
| `IssueDeferred { number, until }` | Claim on deferred issue | core |

---

## 12. Invariants

1. **Ready set never contains blocked issues.** For any graph, no issue in the ready set has an open blocker. Validated by property test.
2. **Cascade is sound.** After closing an issue, every dependent whose blockers are all now closed appears in the cascade result. Validated by property test.
3. **Cycle detection is sound.** If `detect_all_cycles()` returns empty, `would_create_cycle(a, b)` is false for any existing edge direction. Validated by property test.
4. **Cycle detection is complete.** If a cycle exists in the graph, `detect_all_cycles()` finds it. Validated by property test.
5. **Ready set is deterministic.** Computing the ready set twice on the same graph yields the same result. Validated by property test.
6. **Cache is reconstructable.** Deleting the cache and rebuilding from GitHub produces the same graph. Validated by integration test.
7. **Graph construction is idempotent.** Building a graph from the same input data twice produces the same graph. Validated by property test.
8. **Reconcile drift report is deterministic.** Same inputs produce the same drift report (order may vary, but content is identical). Validated by unit test.

---

## 13. Open Questions

1. **Multi-level cascade.** Currently cascade is single-level — closing A unblocks B, but doesn't immediately check B's dependents. Should cascade be recursive? Current answer: no — it's simpler, more predictable, and the next operation on B will trigger its own cascade.

2. **Graph partitioning for cross-repo.** Large org-level projects may have thousands of issues across many repos. Should the graph be partitioned by repo for performance? Current answer: premature — measure first in Phase 03.

3. **Concurrent write ordering.** Two agents closing different issues simultaneously. Both trigger cache invalidation + rebuild. Last writer wins. Is this sufficient? Current answer: yes — GitHub is the source of truth, and the final rebuild reflects all mutations.

---

*This spec defines graph engine algorithms, cache lifecycle, reconciliation analysis, and fast path serving. Tool handler logic is in [03-spec-mcp-tools.md](./03-spec-mcp-tools.md). GitHub API details are in [02-spec-github-client.md](./02-spec-github-client.md).*
