# Spec 03 — MCP Tools

> Companion: [SPEC §6](../SPEC.md#6-mcp-tools) · Plans: [01-mcp-foundation](../plans/01-plan-mcp-foundation.md) · [02-mcp-complete](../plans/02-plan-mcp-complete.md)  
> Crate: `unblock-mcp`  
> Status: draft  
> Last updated: 2026-04-10

---

## Table of Contents

1. [Scope](#1-scope)
2. [Tool Execution Pattern](#2-tool-execution-pattern)
3. [Server Bootstrap Algorithm](#3-server-bootstrap-algorithm)
4. [Tool Specifications — Read Tools](#4-tool-specifications--read-tools)
5. [Tool Specifications — Write Tools](#5-tool-specifications--write-tools)
6. [Tool Specifications — Phase 02 Tools](#6-tool-specifications--phase-02-tools)
7. [Body Section Parsing](#7-body-section-parsing)
8. [Status Update Algorithm](#8-status-update-algorithm)
9. [Error Mapping](#9-error-mapping)
10. [Invariants](#10-invariants)
11. [Open Questions](#11-open-questions)

---

## 1. Scope

This spec defines the **input/output contracts, validation rules, execution flows, and edge cases** for all 20 MCP tools.

**In scope:** tool parameters and validation, execution algorithms, cache interaction, error mapping to MCP protocol, body section parsing, Status update after writes.

**Out of scope:** graph algorithms (→ [01-spec-graph-engine.md](./01-spec-graph-engine.md)), GitHub API details (→ [02-spec-github-client.md](./02-spec-github-client.md)), plugin pipeline (→ Phase 04).

---

## 2. Tool Execution Pattern

Every tool follows the same pattern:

```
execute(state, params) → Result<R, McpError>:

  // 1. Validate input
  validate(&params)?

  // 2. Execute business logic
  result = do_work(state, &params).await?

  // 3. If write: invalidate + rebuild + update fields
  IF is_write_tool:
    state.cache.invalidate()
    (issues, edges) = state.github.fetch_graph_data().await?
    graph = DependencyGraph::build(&issues, &edges)
    ready_set = graph.compute_ready_set(&issues)
    update_status_fields(state, &issues, &ready_set).await?
    state.cache.update(graph)

  // 4. Return result
  RETURN Ok(result)
```

**Exception tools:**
- `comment` — write but NO cache invalidation (graph unchanged)
- `reconcile` — bypasses cache; fresh fetch + analyse + populate
- `show` — always fresh single-issue fetch (never cached)
- `search` — bypasses cache entirely (GitHub Search API)

---

## 3. Server Bootstrap Algorithm

```
bootstrap():

  // 1. Load config
  config = Config::load()?

  // 2. Init tracing
  init_tracing(config.log_level)

  // 3. Select auth strategy
  auth = select_auth(&config)?
  tracing::info!(auth_type = auth.type_name(), "Auth configured")

  // 4. Create GitHub client
  github = GitHubClient::new(config, auth).await?
  // → auto-detects repo from git remote
  // → resolves project number + ID
  // → resolves field IDs

  // 5. Validate fields
  IF github.project_id IS NONE:
    // Bootstrap mode — only init + setup functional
    tracing::warn!("No project detected — bootstrap mode")
  ELSE:
    validate_fields(github.field_ids)?

  // 6. Create cache
  cache = GraphCache::new(config.cache_ttl)

  // 7. Init metrics (optional)
  metrics = init_metrics(&config)

  // 8. Create server state
  state = ServerState {
    config, github, cache, metrics,
    agent_kind: OnceLock::new(),
  }

  // 9. Serve on stdio
  UnblockServer::new(state).serve(stdio()).await
```

### 3.1 Bootstrap mode

When no project is detected (first-time use), only `init` and `setup` are functional. All other tools return `McpError` with `ProjectNotConfigured` and guidance to run `init` first.

### 3.2 CLI tool mode (Phase 02)

`unblock-mcp --run-tool <name> <json-params>` executes a single tool outside of MCP protocol:

```
unblock-mcp --run-tool reconcile '{"fix": true}'
```

1. Same bootstrap as MCP mode (config, auth, project resolution)
2. Deserialise JSON params into the tool's input struct
3. Execute the tool handler
4. Print result JSON to stdout
5. Exit 0 (success) or 1 (error with JSON error body)

No MCP handshake, no stdio transport, no session. Used by CI workflows (GitHub Actions sentinel) and scripts. Reuses the same handler code — zero duplication.

---

## 4. Tool Specifications — Read Tools

### 4.1 `ready`

```
Input:  ReadyParams {
  limit: Option<u32>,           // default: 20
  type: Option<String>,         // issue type filter
  priority: Option<String>,     // P0-P4
  milestone: Option<String>,    // milestone title
  agent: Option<String>,        // agent name filter
  label: Option<String>,        // label filter
  include_claimed: Option<bool>, // default: false
}
Output: ReadyResult {
  issues: Vec<IssueSummary>,
  count: usize,
  stale: bool,
  source: Option<FastPathSource>,  // Phase 03
}

Validation:
  - limit: 1..=100 if present
  - priority: must be P0-P4 if present

Flow:
  1. Check cache
     - Fresh → filter + defer post-filter + return
     - Empty (Phase 03) → fast path from fields + background rebuild
     - Stale/Empty → fetch_graph_data → build graph → update cache
  2. Filter by params (type, priority, milestone, agent, label)
  3. Post-filter: exclude defer_until > today
  4. If NOT include_claimed: exclude Status::InProgress
  5. Sort: priority ASC → created_at ASC
  6. Limit to top N
  7. Return

API calls: 0 (cache hit) | 1+ (rebuild)
Cache: read-only, no invalidation
```

### 4.2 `show`

```
Input:  ShowParams {
  issue: String,                // IssueRef: "#42", "42", "owner/repo#42"
  include_comments: Option<bool>, // default: true
  include_deps: Option<bool>,     // default: true
}
Output: ShowResult {
  issue: IssueDetail,           // full issue with parsed body sections
  comments: Option<Vec<Comment>>,
  upstream: Option<Vec<TreeNode>>,
  downstream: Option<Vec<TreeNode>>,
}

Validation:
  - issue: must parse as valid IssueRef

Flow:
  1. Parse IssueRef
  2. fetch_issue_ref(ref) — ALWAYS fresh, never cached
  3. Parse body sections (BodySections::from_markdown)
  4. If include_deps: dependency_tree(root, Both, max_depth=5)
  5. Return

API calls: 1 (always)
Cache: not used — correctness requires fresh comments
```

### 4.3 `prime`

```
Input:  PrimeParams {}
Output: PrimeResult {
  context: String,              // markdown context for agent injection
  session: SessionMeta,         // Phase 02
  drift_warnings: Option<Vec<String>>,  // Phase 02
}

Flow:
  1. Fetch graph data (or use cache)
  2. Build context summary:
     - Repo: owner/repo
     - Project: number
     - Ready count, blocked count, in-progress count
     - Issues with cycles
     - Current agent assignment
  3. Build SessionMeta from OnceLock (Phase 02)
  4. Background drift check (Phase 02): tokio::spawn reconcile(fix=false)
     - Await with 2s timeout
     - If drift: add warnings
  5. Return markdown blob

API calls: 0 (cache hit) | 1+ (rebuild)
Cache: read-only
```

### 4.4 `stats`

```
Input:  StatsParams { milestone: Option<String> }
Output: StatsResult {
  total: usize,
  by_status: HashMap<String, usize>,
  by_priority: HashMap<String, usize>,
  blocked_count: usize,
  ready_count: usize,
  cycle_count: usize,
  agents: Vec<AgentStats>,
}

Flow:
  1. Fetch graph data (or use cache)
  2. Aggregate counts
  3. Optional milestone filter
  4. Return

API calls: 0 (cache hit) | 1+ (rebuild)
Cache: read-only
```

### 4.5 `list`

```
Input:  ListParams {
  status: Option<String>,
  priority: Option<String>,
  type: Option<String>,
  milestone: Option<String>,
  agent: Option<String>,
  label: Option<String>,
  assignee: Option<String>,
  sort: Option<String>,         // "priority", "created", "updated"
  limit: Option<u32>,           // default: 50
  offset: Option<u32>,          // default: 0
}
Output: ListResult {
  issues: Vec<IssueSummary>,
  total: usize,
  stale: bool,
}

Validation:
  - limit: 1..=200 if present
  - sort: must be valid field name if present

Flow:
  1. Fetch graph data (or use cache)
  2. Filter by all params
  3. Sort by requested field
  4. Paginate with offset/limit
  5. Return

API calls: 0 (cache hit) | 1+ (rebuild)
Cache: read-only
```

### 4.6 `search`

```
Input:  SearchParams { query: String, limit: Option<u32> }
Output: SearchResult { issues: Vec<IssueSummary>, count: usize }

Validation:
  - query: non-empty

Flow:
  1. GitHub Search API: "repo:{owner}/{repo} is:issue {query}"
  2. Map results to IssueSummary
  3. Return

API calls: 1
Cache: bypassed entirely (uses GitHub Search API)
```

### 4.7 `dep_cycles`

```
Input:  DepCyclesParams { id: Option<u64> }
Output: DepCyclesResult { cycles: Vec<Vec<u64>>, count: usize }

Flow:
  1. Fetch graph data (or use cache)
  2. If id: targeted cycle check from that node
  3. Else: detect_all_cycles on full graph
  4. Return

API calls: 0 (cache hit) | 1+ (rebuild)
Cache: read-only
```

---

## 5. Tool Specifications — Write Tools

### 5.1 `claim`

```
Input:  ClaimParams { id: u64, agent: Option<String> }
Output: ClaimResult { issue: IssueSummary }

Validation:
  - id: positive integer
  - agent: non-empty if present (defaults to config.agent)

Flow:
  1. Fetch issue (single query)
  2. Validate:
     - IssueState == Open → else IssueClosed
     - Status == Ready → else AlreadyClaimed (if InProgress) or IssueBlocked (if Blocked) or IssueDeferred (if Deferred)
     - Not blocked: check graph (cache or rebuild) → else IssueBlocked { blockers }
     - Not deferred: defer_until <= today → else IssueDeferred
  3. Update fields: Status→in_progress, Agent→name, Claimed At→now
  4. Add comment: "Claimed by {agent} at {timestamp}"
  5. Invalidate cache + rebuild + update Status fields

API calls: 1 (fetch) + 4 (field updates) + 1 (comment) + 1+ (rebuild)
```

### 5.2 `close`

```
Input:  CloseParams { id: u64, reason: Option<String> }
Output: CloseResult { issue: IssueSummary, unblocked: Vec<u64> }

Validation:
  - id: positive integer

Flow:
  1. Fetch issue, validate IssueState == Open → else IssueClosed
  2. Compute cascade from PRE-CLOSE graph:
     a. Ensure graph is built (from cache or fresh fetch)
     b. compute_unblock_cascade(graph, closed_qid, issues)
     c. Save unblocked list — the graph still contains the issue as open
  3. Close issue (REST PATCH state=closed)
  4. Update fields: Status→closed
  5. Add comment: "Closed: {reason}" (or "Closed" if no reason)
  6. Invalidate cache + rebuild graph (post-close: issue excluded from OPEN query)
  7. For each unblocked (from step 2):
     a. Update Status→ready
     b. Add comment: "Unblocked — blocker #{id} was closed"
  8. Update Status fields from new graph
  9. Update cache

  NOTE: Cascade MUST be computed BEFORE closing the issue (step 2 before step 3).
  After close, fetch_graph_data() returns only OPEN issues — the closed issue is
  excluded from the rebuilt graph. compute_unblock_cascade requires the closed issue
  to be present as a node to find its dependents via Incoming edges.

API calls: 0-1 (pre-close graph) + 1 (fetch) + 1 (close) + 2 (fields) + 1 (comment)
           + 1+ (rebuild) + N×3 per unblocked (2 fields + 1 comment)
```

### 5.3 `create`

```
Input:  CreateParams {
  title: String,
  type: Option<String>,
  priority: Option<String>,       // default: P2
  body: Option<String>,
  labels: Option<Vec<String>>,
  milestone: Option<String>,
  blocked_by: Option<Vec<String>>, // IssueRef array
  parent: Option<String>,          // IssueRef
  story_points: Option<u32>,
  defer_until: Option<String>,     // ISO date
}
Output: CreateResult { issue: IssueSummary }

Validation:
  - title: non-empty, max 500 chars
  - priority: P0-P4 if present
  - type: valid issue type if present
  - defer_until: valid ISO date if present

Flow:
  1. Create issue (REST POST)
  2. Add to project (addProjectV2Item)
  3. Set fields: Priority, Status=ready (or blocked if has blockers), Story Points, Defer Until
  4. If blocked_by:
     a. For each blocker: resolve IssueRef, cycle check, addBlockedBy
     b. Update Status→blocked
  5. If parent: resolve IssueRef, addSubIssue
  6. Invalidate cache + rebuild

API calls: 1 (create) + 1 (add to project) + 3-7 (fields) + 0-N (deps) + 0-1 (parent) + 1+ (rebuild)
```

### 5.4 `depends`

```
Input:  DependsParams { source: String, target: String }
Output: DependsResult { created: bool }

Validation:
  - source: valid IssueRef
  - target: valid IssueRef
  - source != target

Flow:
  1. Resolve both IssueRefs
  2. Cycle detection: would_create_cycle(source, target)
     → CircularDependency if true
  3. Check duplicate: edge already exists
     → DuplicateDependency if true
  4. addBlockedBy mutation
  5. Update source fields: Status→blocked (if was ready)
  6. Invalidate cache + rebuild

API calls: 0-2 (resolve) + 1 (mutation) + 0-2 (fields) + 1+ (rebuild)
Cross-repo: both params accept owner/repo#number
```

### 5.5 `dep_remove`

```
Input:  DepRemoveParams { source: String, target: String }
Output: DepRemoveResult { removed: bool }

Validation:
  - source: valid IssueRef
  - target: valid IssueRef

Flow:
  1. Resolve both IssueRefs
  2. Validate edge exists
  3. removeBlockedBy mutation
  4. Rebuild graph, recompute ready states
  5. If source now has zero open blockers: update Status→ready
  6. Update cache

API calls: 0-2 (resolve) + 1 (mutation) + 0-2 (fields) + 1+ (rebuild)
Cross-repo: both params accept owner/repo#number
```

### 5.6 `update`

```
Input:  UpdateParams {
  id: u64,
  title: Option<String>,
  body: Option<String>,
  status: Option<String>,
  priority: Option<String>,
  labels_add: Option<Vec<String>>,
  labels_remove: Option<Vec<String>>,
  assignees_add: Option<Vec<String>>,
  assignees_remove: Option<Vec<String>>,
  milestone: Option<String>,
  story_points: Option<u32>,
  defer_until: Option<String>,
  agent: Option<String>,
  description: Option<String>,      // body section
  design_notes: Option<String>,     // body section
  acceptance_criteria: Option<String>, // body section
}
Output: UpdateResult { issue: IssueSummary }

Validation:
  - id: positive integer
  - At least one field to update
  - title: non-empty, max 500 chars if present
  - priority: P0-P4 if present
  - status: valid Status variant if present
  - defer_until: valid ISO date if present

Flow:
  1. Fetch issue, validate not closed (unless reopening via status)
  2. If body sections changed: parse existing body, merge sections, write back
  3. If REST fields changed (title, body, labels, assignees, milestone): PATCH issue
  4. If Project fields changed (status, priority, agent, story_points, defer_until): batch_update_fields
  5. Invalidate cache + rebuild

API calls: 1 (fetch) + 0-1 (PATCH) + 0-N (field updates) + 1+ (rebuild)
```

### 5.7 `reopen`

```
Input:  ReopenParams { id: u64 }
Output: ReopenResult { issue: IssueSummary, blocked: bool }

Validation:
  - id: positive integer

Flow:
  1. Fetch issue, validate IssueState == Closed → else IssueNotClosed or IssueAlreadyOpen
  2. Reopen issue (REST PATCH state=open)
  3. Rebuild graph to evaluate blocking status
  4. If issue has open blockers: Status→blocked
  5. Else: Status→ready
  6. Update cache

API calls: 1 (fetch) + 1 (reopen) + 2 (fields) + 1+ (rebuild)
```

### 5.8 `comment`

```
Input:  CommentParams { id: u64, body: String }
Output: CommentResult { created: bool }

Validation:
  - id: positive integer
  - body: non-empty

Flow:
  1. POST comment to GitHub
  2. NO cache invalidation — comments don't affect the graph

API calls: 1
Cache: NO invalidation
```

### 5.9 `init`

```
Input:  InitParams { title: Option<String> }
Output: InitResult { project_number: u64 }

Flow:
  1. Check if project already exists → return existing if so
  2. Detect owner type (org vs user)
  3. Create Projects V2 board (GraphQL mutation)
  4. Store project_id and project_number in client
  5. Return

API calls: 1 (check) + 1 (create)
```

### 5.10 `setup`

```
Input:  SetupParams { project: Option<u64>, dry_run: Option<bool>, migrate: Option<bool> }
Output: SetupResult { fields_created: Vec<String>, views_created: Vec<String>, migrated_count: Option<usize> }

Flow:
  1. Resolve project (param or auto-detect)
  2. Query existing fields
  3. Create missing fields (7 total, skip existing) — idempotent
  4. Detect owner type (org vs user)
  5. Query existing views (GraphQL)
  6. Discover field IDs (REST GET /fields — integer IDs)
  7. Create missing views (REST POST /views — up to 5) — idempotent
  8. If migrate: add existing open issues to project, set defaults
  9. Report

API calls: 1 (fields query) + 0-7 (create fields) + 1 (views query) + 1 (REST fields)
           + 0-5 (create views) + 0-N (migrate)
Idempotent: safe to run multiple times
```

---

## 6. Tool Specifications — Phase 02 Tools

### 6.1 `reconcile`

```
Input:  ReconcileParams {
  fix: Option<bool>,              // default: false
  stale_claim_hours: Option<u64>, // default: 24
}
Output: ReconcileOutput { report: DriftReport }

Flow:
  1. Fresh fetch — ALWAYS bypasses cache. fetch_graph_data()
  2. Build graph + compute ready set
  3. Check missing project fields (7 required)
  4. Run ReconcileEngine::analyse() — pure, no I/O
  5. If fix:
     a. StaleStatus → update Status field
     b. UncascadedClosure → update downstream Status to ready + audit comment
     c. StaleClaim → log warning only (no auto-repair)
     d. CycleDetected → add to errors (no auto-repair)
     e. Others → log warning (no auto-repair)
  6. Populate cache with fresh graph (does NOT call invalidate first)
  7. Return report

API calls: 1+ (fetch) + 0-N (repairs) + 0-N (comments)
Cache: bypasses; populates after analysis
```

### 6.2 `commit_context`

```
Input:  CommitContextParams {
  issue_number: u64,
  summary: String,
  body: Option<String>,
}
Output: CommitContextOutput { message: String }

Validation:
  - issue_number: positive integer
  - summary: non-empty, max 72 chars (conventional commit subject line)

Flow:
  1. Fetch issue (always fresh, like show)
  2. Validate exists → IssueNotFound if not
  3. Build message:
     "{summary}\n\n{body}\n\nUnblock-Issue: #{number}\nUnblock-Agent: {agent}\nUnblock-Status: {status}\nUnblock-Priority: {priority}"
  4. Return

API calls: 1 (fetch)
Cache: not used (read-only, always fresh)
```

### 6.3 `doctor`

```
Input:  DoctorParams { fix: Option<bool> }
Output: DoctorOutput { checks: Vec<HealthCheck>, healthy: bool }

Flow — 7 health checks in order:
  1. GitHub connectivity:    GET /user → Ok/Error
  2. Repository access:      GET /repos/{o}/{r} → Ok/Error
  3. Project exists:         project_id is Some → Ok/Error. Fix: init flow
  4. Required fields:        7 fields exist with correct types → Ok/Warning/Error. Fix: setup flow
  5. Cache health:           report Fresh/Stale/Empty + age → Ok (informational)
  6. Graph integrity:        detect cycles → Ok/Warning
  7. Drift check:            reconcile(fix=false) → Ok/Warning. Fix: reconcile(fix=true)

  healthy = all checks are Ok

API calls: 2 (connectivity) + 0-1 (field query) + 0-N (repairs)
Cache: read-only (except drift check)
```

---

## 7. Body Section Parsing

### 7.1 Algorithm

```
from_markdown(body: &str) → BodySections:

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
      current_section = None  // unknown section, skip
      CONTINUE

    IF current_section IS SOME:
      sections[current_section].push(line)

  RETURN BodySections {
    description: sections.description.trim(),
    design_notes: sections.design_notes.trim(),
    acceptance_criteria: sections.acceptance_criteria.trim(),
  }
```

### 7.2 `to_markdown(sections) → String`

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

### 7.3 Merge algorithm (for `update`)

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

### 7.4 Edge cases

- **No headings in body:** Entire body is treated as description.
- **Extra headings:** Unknown `## Foo` headings MUST be preserved during round-trip. `from_markdown` captures them as opaque `(heading, content)` pairs with their relative position. `to_markdown` emits them in original order after the three known sections. Without this, an `update` that modifies one body section would silently delete unrecognised headings added by humans or other tools.
- **Empty sections:** A heading with no content below it → empty string for that section.
- **Nested headings:** `### Sub-heading` within a section → treated as section content, not a new section.

---

## 8. Status Update Algorithm

### 8.1 After every write that invalidates cache

```
update_status_fields(state, issues, ready_set) → Result<()>:

  // Diff: computed ready set vs current Status field values
  updates = []

  FOR each issue in issues:
    expected = compute_expected_status(issue, ready_set)
    IF issue.status != expected:
      updates.push((issue.item_id, expected))

  // Batch update changed fields
  FOR each (item_id, new_status) in updates:
    state.github.update_field(item_id, "Status", SingleSelect(new_status.option_id()))

  tracing::info!(updated = updates.len(), "Status fields synchronised")


compute_expected_status(issue, ready_set) → Status:
  IF issue.state == Closed:
    RETURN Closed
  IF issue.status == InProgress:
    RETURN InProgress  // claimed — preserve, do not override
  IF issue.status == Deferred:
    RETURN Deferred    // deferred — preserve, do not override
  IF issue.qualified_id IN ready_set:
    RETURN Ready
  RETURN Blocked
```

### 8.2 Edge cases

- **No changes:** If all fields match, zero API calls. Common on read-heavy workloads.
- **Issue not in project:** Cannot update field. Skip with warning.
- **Batch size:** If many fields changed (e.g., large cascade), individual calls may hit rate limits. Batching via GraphQL aliases mitigates.

---

## 9. Error Mapping

### 9.1 Domain → MCP

```
impl From<Error> for McpError:
  Domain(IssueNotFound { number }) → McpError { code: -32602, message: "Issue not found: #{number}" }
  Domain(AlreadyClaimed { number, agent }) → McpError { code: -32602, message: "Issue #{number} already claimed by {agent}" }
  Domain(IssueBlocked { number, blockers }) → McpError { code: -32602, message: "Issue #{number} is blocked by: {blockers}" }
  Domain(CircularDependency { source, target }) → McpError { code: -32602, message: "Circular dependency: #{source} → #{target}" }
  Domain(Validation { message }) → McpError { code: -32602, message }
  GitHubApi { message } → McpError { code: -32603, message: "GitHub API error: {message}" }
  GitHubUnavailable { .. } → McpError { code: -32603, message: "GitHub unavailable" }
  RateLimited → McpError { code: -32603, message: "GitHub rate limit exceeded" }
  CircuitBreakerOpen → McpError { code: -32603, message: "GitHub API circuit breaker open — retry later" }
```

### 9.2 MCP error codes

| Code | Category | Examples |
|---|---|---|
| `-32602` | Invalid params / business rule | NotFound, AlreadyClaimed, Blocked, Cycle, Validation |
| `-32603` | Internal error / infrastructure | GitHub API, network, rate limit, circuit breaker |

---

## 10. Invariants

1. **Every write tool invalidates + rebuilds + updates fields.** No write tool leaves the cache or Status fields in an inconsistent state. Exception: `comment` (no graph impact).
2. **`show` is always fresh.** Never served from cache. Comments contain structured markers required for session context reconstruction.
3. **`search` bypasses cache.** Uses GitHub Search API directly. Cache has no search index.
4. **`reconcile` bypasses but populates cache.** Fresh fetch, analyse, optional repair, then cache gets the fresh graph.
5. **Validation before mutation.** All tools validate input before calling GitHub. No partial mutations from validation failures.
6. **Body sections are round-trippable.** `to_markdown(from_markdown(body))` preserves the three known sections. Unknown sections may be reordered.
7. **Cascade is complete.** After `close`, all dependents whose blockers are now all closed appear in `unblocked` and have Status updated to `ready`.
8. **Tool responses are structured JSON.** No formatted text. Agents parse JSON, not markdown (except `prime.context` which is intentionally markdown for agent injection).

---

## 11. Open Questions

1. **`update` body section merge vs replace.** Currently, updating a body section replaces it entirely. Should we support appending? Current answer: no — replace is simpler and the agent can read-modify-write.

2. **`claim` re-claim.** Can the same agent re-claim an issue it already owns? Currently returns `AlreadyClaimed`. Should it be a no-op? Current answer: error — explicit unclaim + reclaim if needed.

3. **`close` cascading field updates batching.** Large cascades (10+ unblocked issues) generate many API calls. Should we batch all Status updates into a single GraphQL mutation? Current answer: yes, implemented via `batch_update_fields`. Comments still require individual REST calls.

4. **`ready` result stability.** If two consecutive `ready` calls return different results (due to concurrent mutation between calls), is this a problem? Current answer: no — agents should tolerate eventual consistency. The ready set reflects GitHub's state at query time.

---

*This spec defines MCP tool contracts, execution flows, and edge cases. Graph algorithms are in [01-spec-graph-engine.md](./01-spec-graph-engine.md). GitHub API details are in [02-spec-github-client.md](./02-spec-github-client.md).*
