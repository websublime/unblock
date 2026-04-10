# Plan 02 — MCP Complete (v0.2.0)

> Phase: 02  
> Version: v0.2.0  
> Crates: `unblock-core`, `unblock-github`, `unblock-mcp`  
> Depends on: Phase 01 (MCP Foundation) ✅  
> Required by: Phase 03 (MCP Production)  
> Status: partially implemented (Epics 01, 02, 08 done early during Phase 01)  
> Companion spec: [03-spec-mcp-tools.md](../specs/03-spec-mcp-tools.md)

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [Rust Idioms & Rules](#2-rust-idioms--rules)
3. [Public API Surface](#3-public-api-surface)
4. [Priority & Dependency Legend](#4-priority--dependency-legend)
5. [Epics](#5-epics)
   - [Epic 01 — Reconciliation Engine](#epic-01--reconciliation-engine)
   - [Epic 02 — Reconcile Tool Handler](#epic-02--reconcile-tool-handler)
   - [Epic 03 — Commit Context Tool](#epic-03--commit-context-tool)
   - [Epic 04 — Doctor Tool](#epic-04--doctor-tool)
   - [Epic 05 — Circuit Breaker](#epic-05--circuit-breaker)
   - [Epic 06 — Retry with Exponential Backoff](#epic-06--retry-with-exponential-backoff)
   - [Epic 07 — OpenTelemetry Metrics](#epic-07--opentelemetry-metrics)
   - [Epic 08 — Agent Client Detection](#epic-08--agent-client-detection)
   - [Epic 09 — Prime Integration & Drift Warnings](#epic-09--prime-integration--drift-warnings)
   - [Epic 10 — GitHub Actions Reconciliation Sentinel](#epic-10--github-actions-reconciliation-sentinel)
6. [Definition of Done](#6-definition-of-done)

---

## 1. Purpose

Phase 02 hardens the MCP server for production use. Phase 01 delivers the complete agent workflow loop — `prime` → `ready` → `claim` → work → `close` → cascade — with 17 tools, a graph engine, and a TTL cache. Phase 02 addresses the gaps that emerge when the server operates in the real world:

1. **Semantic drift.** GitHub is an open system. A human can close an issue, remove a blocking edge, or edit a Projects V2 field via the GitHub UI. The in-memory graph diverges. The `reconcile` tool detects and repairs 7 drift types — `StaleReadyState`, `UncascadedClosure`, `OrphanedBlockingEdge`, `MalformedAgentField`, `MissingProjectField`, `CycleDetected`, `StaleClaim`.

2. **Operational health.** The `doctor` tool provides health checks with self-repair capability. The `commit_context` tool produces structured commit messages with git trailers for audit trail.

3. **Resilience.** A circuit breaker prevents cascading failures when GitHub is unavailable. Retry with exponential backoff handles transient rate limits (429) and server errors (503).

4. **Observability.** OpenTelemetry metrics provide actionable dashboards for tool latency, API duration, cache performance, and graph size.

5. **Agent awareness.** The server detects which AI client is connected (Claude Code, Copilot, Cursor, etc.) and surfaces this in logs, `prime` output, and tracing spans — without changing tool behaviour.

**Phase 02 does not:**
- Change the transport (still stdio only)
- Add new crates (still 3: core, github, mcp)
- Modify the existing 17 tools beyond resilience integration
- Introduce persistent storage

---

## 2. Rust Idioms & Rules

These rules supplement the workspace-wide rules in the CLAUDE.md and SPEC §17.2.

### 2.1 Pure engines, impure handlers

The reconciliation engine (`ReconcileEngine`) is **pure** — no I/O, no `async`, no GitHub calls. It receives pre-fetched data and returns a `DriftReport`. This makes it fully testable with in-memory data. The tool handler (`handle_reconcile`) is the impure shell: it fetches, calls the engine, and performs repairs.

This pattern applies to all new logic in this phase. Domain computation lives in `unblock-core`. I/O lives in `unblock-mcp`.

### 2.2 `DateTime<Utc>` injection

All time-dependent logic accepts a `now: DateTime<Utc>` parameter. Never call `Utc::now()` inside domain code. This makes stale-claim detection, circuit breaker cooldowns, and retry delays deterministic in tests.

### 2.3 Circuit breaker as middleware

The circuit breaker wraps `GitHubClient` calls — it does not modify `GitHubClient` internals. It is a composable layer, not an inheritance hierarchy. The breaker lives in `unblock-github` because it is a concern of the HTTP client, not the domain.

### 2.4 Feature-gated test helpers

Test-only code paths (e.g., `set_circuit_breaker_state()`, `inject_drift()`) are gated behind `#[cfg(feature = "test-hooks")]`. Never enabled in production builds.

### 2.5 OpenTelemetry is optional

All OTel code is behind the `otel` cargo feature flag. When disabled, the code compiles and runs — metric calls become no-ops. The server must never fail to start because OTel is not configured.

---

## 3. Public API Surface

### 3.1 New files in `unblock-core`

```
unblock-core/src/
  reconcile.rs         ← DriftKind, DriftReport, ReconcileEngine
  client.rs            ← AgentKind, AgentClient
  detection.rs         ← ClientDetector
  lib.rs               ← add: pub mod reconcile; pub mod client; pub mod detection;
```

### 3.2 New files in `unblock-github`

```
unblock-github/src/
  resilience.rs        ← CircuitBreaker, CircuitState, RetryPolicy
  lib.rs               ← add: pub mod resilience;
```

### 3.3 New files in `unblock-mcp`

```
unblock-mcp/src/
  tools/
    reconcile.rs       ← ReconcileParams, ReconcileOutput, handle_reconcile
    commit_context.rs  ← CommitContextParams, CommitContextOutput, handle_commit_context
    doctor.rs          ← DoctorParams, DoctorOutput, handle_doctor
  metrics.rs           ← OTel setup, metric definitions (feature = "otel")
```

---

## 4. Priority & Dependency Legend

### Priority levels

| Level | Meaning |
|---|---|
| **P0** | Absolute blocker — nothing moves forward until this is done |
| **P1** | Critical for the phase to be functional — happy path |
| **P2** | Important but does not block the happy path |
| **P3** | Quality, ergonomics, extra coverage |
| **P4** | Nice to have — included if time permits, does not delay done |

### Dependency fields

Every task carries three metadata fields:

- **Priority** — P0 through P4 as defined above
- **Depends on** — task IDs within this plan that must be complete before this task starts
- **Blocked by** — external blockers (other phases, tools, decisions outside this plan)

---

## 5. Epics

---

### Epic 01 — Reconciliation Engine

**Goal:** A pure Rust engine that detects 7 types of semantic drift between the computed dependency graph and GitHub reality. No I/O, no async — receives pre-fetched data, returns a `DriftReport`.

**Crate:** `unblock-core`

**Status:** ✅ Implemented early during Phase 01. `DriftKind` (7 variants), `DriftReport`, `ReconcileEngine` with 6-pass analysis exist in `unblock-core/src/reconcile.rs`.

---

#### Task 01.01 — Define `DriftKind` enum and `DriftReport` struct

> **Priority:** P0  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `src/reconcile.rs`

Define the drift taxonomy. 7 variants cover all realistic external mutation scenarios.

Requirements:
- `DriftKind` enum with 7 variants: `StaleReadyState`, `UncascadedClosure`, `OrphanedBlockingEdge`, `MalformedAgentField`, `MissingProjectField`, `CycleDetected`, `StaleClaim`
- Each variant carries full context for diagnosis and repair (issue IDs, field values, timestamps)
- Derives: `Debug`, `Clone`, `Serialize`, `Deserialize`
- `DriftReport` struct: `repo`, `reconciled_at`, `issues_scanned`, `edges_scanned`, `drift_found: Vec<DriftKind>`, `repaired: Vec<DriftKind>`, `errors: Vec<String>`, `clean: bool`
- `DriftReport::clean` is `true` if and only if `drift_found.is_empty()`
- `impl Display for DriftReport` — human-readable summary

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DriftKind {
    /// Ready State field in GitHub diverges from what the graph computes.
    StaleReadyState {
        issue: QualifiedId,
        field_says: ReadyState,
        graph_says: ReadyState,
    },

    /// Issue closed via UI — downstream issues should have received a cascade.
    UncascadedClosure {
        closed_issue: QualifiedId,
        should_have_unblocked: Vec<QualifiedId>,
    },

    /// Blocking edge references an issue that does not exist or is inaccessible.
    OrphanedBlockingEdge {
        source: QualifiedId,
        missing_target: QualifiedId,
    },

    /// Agent field has invalid format (must be `username:supervisor`).
    MalformedAgentField {
        issue: QualifiedId,
        raw_value: String,
    },

    /// Required Projects V2 field is missing.
    MissingProjectField {
        field_name: String,
    },

    /// Cycle detected in the graph.
    CycleDetected {
        cycle: Vec<QualifiedId>,
    },

    /// Issue in `in_progress` state for too long without update.
    StaleClaim {
        issue: QualifiedId,
        claimed_at: DateTime<Utc>,
        hours_stale: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftReport {
    pub repo: String,
    pub reconciled_at: DateTime<Utc>,
    pub issues_scanned: usize,
    pub edges_scanned: usize,
    pub drift_found: Vec<DriftKind>,
    pub repaired: Vec<DriftKind>,
    pub errors: Vec<String>,
    pub clean: bool,
}
```

**Tests:**
- `drift_report_clean_when_no_drift`
- `drift_report_not_clean_when_drift_present`
- `drift_kind_serialises_round_trip` — all 7 variants through JSON
- `drift_report_display_format_readable`

---

#### Task 01.02 — Implement `ReconcileEngine::analyse()`

> **Priority:** P0  
> **Depends on:** Task 01.01  
> **Blocked by:** nothing

**File:** `src/reconcile.rs`

The engine analyses all 7 drift types in a single pass. Pure function — no I/O, no `async`.

Requirements:
- `ReconcileEngine::new(stale_claim_threshold_hours: u64)` — configurable threshold (default: 24)
- `ReconcileEngine::analyse(&self, graph: &DependencyGraph, issues: &HashMap<QualifiedId, Issue>, computed_ready_set: &HashSet<QualifiedId>, now: DateTime<Utc>) -> DriftReport`
- Detection order:
  1. **Stale Ready State** — for each issue, compare `issue.ready_state` with graph-computed state. Closed issues must have `ReadyState::Closed`. Open issues in the ready set must have `ReadyState::Ready`. All others must have `ReadyState::Blocked`
  2. **Uncascaded closures** — for each closed issue, call `graph.compute_unblock_cascade()`. Filter to downstream issues that are open but not marked as `ReadyState::Ready`. Non-empty → drift
  3. **Orphaned blocking edges** — for each edge in the graph, check target exists in `issues`. Missing → drift
  4. **Cycles** — `graph.detect_all_cycles()`. Each SCC with length > 1 → drift
  5. **Stale claims** — issues with `Status::InProgress` and `claimed_at` older than threshold
  6. **Malformed agent fields** — agent field present but doesn't contain `:` separator
- Missing Projects V2 field detection (variant 7) is handled in the tool handler, not the engine, because it requires field validation data from `ProjectFieldIds`
- `repo` field in `DriftReport` is left empty — filled by the tool handler

**Note on API alignment:** The engine references methods that may need to be added to `DependencyGraph`:
- `all_edges() -> Vec<BlockingEdge>` — trivial wrapper over `petgraph::edge_references()`
- `edge_count() -> usize` — trivial wrapper over `petgraph::edge_count()`

If these methods don't exist yet, add them as part of this task (they are internal helpers, not a public API change).

```rust
pub struct ReconcileEngine {
    stale_claim_threshold_hours: u64,
}

impl ReconcileEngine {
    pub fn new(stale_claim_threshold_hours: u64) -> Self {
        Self { stale_claim_threshold_hours }
    }

    pub fn analyse(
        &self,
        graph: &DependencyGraph,
        issues: &HashMap<QualifiedId, Issue>,
        computed_ready_set: &HashSet<QualifiedId>,
        now: DateTime<Utc>,
    ) -> DriftReport { /* see detection order above */ }
}
```

**Tests:**
- `analyse_no_drift_on_consistent_state`
- `analyse_detects_stale_ready_state_on_closed_issue`
- `analyse_detects_stale_ready_state_on_unblocked_issue`
- `analyse_detects_uncascaded_closure`
- `analyse_detects_orphaned_blocking_edge`
- `analyse_detects_cycle`
- `analyse_detects_stale_claim`
- `analyse_detects_malformed_agent_field`
- `analyse_multiple_drift_types_in_single_pass`
- `analyse_stale_claim_respects_threshold_hours`
- `analyse_agent_field_with_colon_is_valid`

---

#### Task 01.03 — Graph helper methods for reconciliation

> **Priority:** P0  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `unblock-core/src/graph.rs`

Add helper methods required by `ReconcileEngine` if they don't already exist on `DependencyGraph`.

Requirements:
- `pub fn all_edges(&self) -> Vec<BlockingEdge>` — returns all edges as `(source: QualifiedId, target: QualifiedId)` pairs. Wraps `petgraph::edge_references()`
- `pub fn edge_count(&self) -> usize` — wraps `petgraph::edge_count()`
- `BlockingEdge` struct: `pub source: QualifiedId, pub target: QualifiedId`

```rust
#[derive(Debug, Clone)]
pub struct BlockingEdge {
    pub source: QualifiedId,
    pub target: QualifiedId,
}
```

**Tests:**
- `all_edges_returns_empty_for_empty_graph`
- `all_edges_returns_correct_edges`
- `edge_count_matches_all_edges_length`

---

### Epic 02 — Reconcile Tool Handler

**Goal:** MCP tool handler that fetches fresh data from GitHub, runs the reconciliation engine, optionally repairs drift, and updates the cache.

**Crate:** `unblock-mcp`

**Status:** ✅ Implemented early during Phase 01. `ReconcileParams`, `ReconcileOutput`, and handler exist in `unblock-mcp/src/tools/reconcile.rs`. Tool registered in server. Auto-repair stubs present — repair logic needs completion.

---

#### Task 02.01 — Define `ReconcileParams` and `ReconcileOutput`

> **Priority:** P0  
> **Depends on:** Task 01.01  
> **Blocked by:** nothing

**File:** `src/tools/reconcile.rs`

Requirements:
- `ReconcileParams`: `fix: bool` (default: `false`), `stale_claim_hours: u64` (default: `24`)
- `ReconcileOutput`: wraps `DriftReport`
- Both derive `Deserialize`, `Serialize`, `JsonSchema`

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReconcileParams {
    /// If true, automatically repairs detected drift.
    #[serde(default)]
    pub fix: bool,

    /// Hours without update before considering a claim stale. Default: 24.
    #[serde(default = "default_stale_hours")]
    pub stale_claim_hours: u64,
}

fn default_stale_hours() -> u64 { 24 }

#[derive(Debug, Serialize)]
pub struct ReconcileOutput {
    pub report: DriftReport,
}
```

**Tests:**
- `reconcile_params_defaults` — deserialise `{}` → `fix: false, stale_claim_hours: 24`

---

#### Task 02.02 — Implement `handle_reconcile`

> **Priority:** P0  
> **Depends on:** Task 01.02, Task 02.01  
> **Blocked by:** nothing

**File:** `src/tools/reconcile.rs`

The handler is the impure shell: fetch, analyse, repair, cache.

Requirements:
1. **Fresh fetch** — always bypasses cache. Calls `state.github.fetch_graph_data()`. Does NOT call `cache.invalidate()`
2. **Build graph** — `DependencyGraph::build()` from fetched issues and edges
3. **Compute ready set** — `graph.compute_ready_set()`, collect into `HashSet<QualifiedId>`
4. **Analyse** — `ReconcileEngine::new(params.stale_claim_hours).analyse()`
5. **Check missing project fields** — validate `state.github.field_ids()` against expected 7 fields. Missing → add `DriftKind::MissingProjectField` to report
6. **Repair (if `fix`):**
   - `StaleReadyState` → update Ready State field via `state.github.update_field()`
   - `UncascadedClosure` → update Ready State to `Ready` for each downstream issue + add audit comment on closed issue
   - `StaleClaim` → log warning, do NOT auto-repair (agent or human decides)
   - `CycleDetected` → add to `errors`, do NOT auto-repair (manual resolution required)
   - `OrphanedBlockingEdge`, `MalformedAgentField`, `MissingProjectField` → log warning, do NOT auto-repair
7. **Populate cache** — `state.cache.update(graph)` with the fresh graph
8. **Return** — `ReconcileOutput { report }`

**Cache behaviour:** `reconcile` always does a fresh fetch, bypassing the cache. After analysis and optional repair, it populates the cache with the fresh graph. It does NOT call `cache.invalidate()` — it replaces the cache directly. See SPEC §4.4 invalidation matrix.

```rust
pub async fn handle_reconcile(
    params: ReconcileParams,
    state: &ServerState,
) -> Result<ReconcileOutput, McpError> { /* steps 1-8 above */ }
```

**Tests (integration):**
- `reconcile_reports_clean_on_consistent_repo`
- `reconcile_detects_stale_ready_state`
- `reconcile_repairs_stale_ready_state_when_fix`
- `reconcile_repairs_uncascaded_closure_with_audit_comment`
- `reconcile_does_not_repair_stale_claim`
- `reconcile_populates_cache_after_analysis`

---

#### Task 02.03 — Register reconcile tool in MCP server

> **Priority:** P1  
> **Depends on:** Task 02.02  
> **Blocked by:** nothing

**File:** `src/server.rs`

Add `reconcile` to the MCP tool registry with proper schema and description.

Requirements:
- Tool name: `reconcile`
- Description: "Detect and optionally repair semantic drift between the computed dependency graph and GitHub state"
- Register in the server's tool list alongside existing 17 tools

**Tests:**
- `server_lists_reconcile_tool`

---

### Epic 03 — Commit Context Tool

**Goal:** A read-only tool that produces structured commit messages with git trailers linking work to issues and agents.

**Crate:** `unblock-mcp`

---

#### Task 03.01 — Define `CommitContextParams` and `CommitContextOutput`

> **Priority:** P1  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `src/tools/commit_context.rs`

Requirements:
- `CommitContextParams`: `issue_number: u64`, `summary: String` (one-line commit subject), `body: Option<String>` (extended description)
- `CommitContextOutput`: `message: String` (full commit message with trailers)
- Git trailers format (appended after body, blank line separated):
  - `Unblock-Issue: #<number>`
  - `Unblock-Agent: <agent_name>` (from `ServerState.config.agent`)
  - `Unblock-Status: <status>` (issue status at time of commit)
  - `Unblock-Priority: <priority>` (issue priority)

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommitContextParams {
    /// Issue number this commit relates to.
    pub issue_number: u64,
    /// One-line commit subject.
    pub summary: String,
    /// Optional extended description.
    pub body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CommitContextOutput {
    /// Full commit message with git trailers.
    pub message: String,
}
```

**Tests:**
- `commit_context_params_requires_issue_and_summary`

---

#### Task 03.02 — Implement `handle_commit_context`

> **Priority:** P1  
> **Depends on:** Task 03.01  
> **Blocked by:** nothing

**File:** `src/tools/commit_context.rs`

Requirements:
1. Fetch issue via `state.github.fetch_issue(params.issue_number)` — always fresh (like `show`)
2. Validate issue exists → `IssueNotFound` if not
3. Construct commit message:
   ```
   <summary>

   <body if present>

   Unblock-Issue: #<number>
   Unblock-Agent: <agent>
   Unblock-Status: <status>
   Unblock-Priority: <priority>
   ```
4. Does NOT invalidate cache (read-only)
5. Return `CommitContextOutput { message }`

```rust
pub async fn handle_commit_context(
    params: CommitContextParams,
    state: &ServerState,
) -> Result<CommitContextOutput, McpError> { /* ... */ }
```

**Tests:**
- `commit_context_produces_message_with_trailers`
- `commit_context_includes_body_when_present`
- `commit_context_omits_body_when_absent`
- `commit_context_returns_not_found_for_missing_issue`

---

#### Task 03.03 — Register commit_context tool in MCP server

> **Priority:** P2  
> **Depends on:** Task 03.02  
> **Blocked by:** nothing

**File:** `src/server.rs`

Register `commit_context` in the tool catalogue.

**Tests:**
- `server_lists_commit_context_tool`

---

### Epic 04 — Doctor Tool

**Goal:** Operational health checks with optional self-repair capability. Diagnoses configuration, GitHub connectivity, project setup, and field integrity.

**Crate:** `unblock-mcp`

---

#### Task 04.01 — Define `DoctorParams` and `DoctorOutput`

> **Priority:** P1  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `src/tools/doctor.rs`

Requirements:
- `DoctorParams`: `fix: bool` (default: `false`) — whether to attempt auto-repair
- `DoctorOutput`: `checks: Vec<HealthCheck>`, `healthy: bool`
- `HealthCheck`: `name: String`, `status: CheckStatus`, `message: String`, `repaired: bool`
- `CheckStatus` enum: `Ok`, `Warning`, `Error`

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DoctorParams {
    /// If true, attempt to repair detected issues.
    #[serde(default)]
    pub fix: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    pub repaired: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum CheckStatus { Ok, Warning, Error }

#[derive(Debug, Serialize)]
pub struct DoctorOutput {
    pub checks: Vec<HealthCheck>,
    pub healthy: bool,
}
```

**Tests:**
- `doctor_output_healthy_when_all_checks_ok`
- `doctor_output_unhealthy_when_any_check_error`

---

#### Task 04.02 — Implement `handle_doctor`

> **Priority:** P1  
> **Depends on:** Task 04.01  
> **Blocked by:** nothing

**File:** `src/tools/doctor.rs`

Requirements — health checks in order:
1. **GitHub connectivity** — `GET /user` with configured token. `Ok` if 200. `Error` if unreachable or 401
2. **Repository access** — `GET /repos/{owner}/{repo}`. `Ok` if 200. `Error` if 404 or 403
3. **Project exists** — `state.github.project_id` is `Some`. `Error` if `None`. If `fix` → call `init` flow
4. **Required fields** — validate all 7 Projects V2 fields exist with correct types and option values. `Warning` for wrong options, `Error` for missing fields. If `fix` → call `setup` flow
5. **Cache health** — report cache state: `Fresh`, `Stale`, or `Empty` with age. Always `Ok` (informational)
6. **Graph integrity** — detect cycles via `dep_cycles`. `Warning` if cycles exist, `Ok` if clean
7. **Drift check** — run `ReconcileEngine::analyse()` (read-only). `Warning` if drift, `Ok` if clean. If `fix` → run `reconcile --fix`

```rust
pub async fn handle_doctor(
    params: DoctorParams,
    state: &ServerState,
) -> Result<DoctorOutput, McpError> { /* ... */ }
```

**Tests (integration):**
- `doctor_all_checks_pass_on_healthy_setup`
- `doctor_detects_missing_project`
- `doctor_detects_missing_fields`
- `doctor_repairs_missing_fields_when_fix`
- `doctor_reports_drift_as_warning`

---

#### Task 04.03 — Register doctor tool in MCP server

> **Priority:** P2  
> **Depends on:** Task 04.02  
> **Blocked by:** nothing

**File:** `src/server.rs`

Register `doctor` in the tool catalogue.

**Tests:**
- `server_lists_doctor_tool`

---

### Epic 05 — Circuit Breaker

**Goal:** Graceful degradation when the GitHub API is unavailable. Prevents cascading failures and wasted rate limit budget on a failing endpoint.

**Crate:** `unblock-github`

---

#### Task 05.01 — Define `CircuitBreaker` and `CircuitState`

> **Priority:** P0  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `src/resilience.rs`

Requirements:
- `CircuitState` enum: `Closed` (normal operation), `Open` (fail fast), `HalfOpen` (one probe allowed)
- `CircuitBreaker` struct:
  - `state: CircuitState`
  - `failure_count: usize`
  - `failure_threshold: usize` — default: `5`
  - `cooldown: Duration` — default: `10s`
  - `last_failure: Option<Instant>` — tracks when the circuit opened
- Thread-safe: wrap internals in `Mutex<CircuitBreakerInner>` — acceptable because it's held briefly per-call
- Derives: `Debug`

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct CircuitBreaker {
    inner: Mutex<CircuitBreakerInner>,
}

struct CircuitBreakerInner {
    state: CircuitState,
    failure_count: usize,
    failure_threshold: usize,
    cooldown: Duration,
    last_failure: Option<Instant>,
}
```

**Tests:**
- `circuit_breaker_starts_closed`
- `circuit_state_display`

---

#### Task 05.02 — Implement `CircuitBreaker` state machine

> **Priority:** P0  
> **Depends on:** Task 05.01  
> **Blocked by:** nothing

**File:** `src/resilience.rs`

Requirements:
- `CircuitBreaker::new(failure_threshold: usize, cooldown: Duration) -> Self`
- `CircuitBreaker::check(&self) -> Result<(), Error>` — returns `Err(CircuitBreakerOpen)` if state is `Open` and cooldown hasn't elapsed. Transitions `Open → HalfOpen` if cooldown elapsed. Returns `Ok(())` if `Closed` or `HalfOpen`
- `CircuitBreaker::record_success(&self)` — resets failure count to 0. `HalfOpen → Closed`
- `CircuitBreaker::record_failure(&self)` — increments failure count. If count >= threshold: `Closed → Open`, set `last_failure`. In `HalfOpen`: back to `Open`
- `CircuitBreaker::state(&self) -> CircuitState` — current state (for logging/metrics)
- `CircuitBreaker::reset(&self)` — force back to `Closed` (for `doctor --fix`)

State machine:
```
         success
Closed ◄────────── HalfOpen
  │                    ▲
  │ failure_count      │ cooldown elapsed
  │ >= threshold       │
  ▼                    │
 Open ─────────────────┘
```

**Tests:**
- `circuit_breaker_stays_closed_below_threshold`
- `circuit_breaker_opens_at_threshold`
- `circuit_breaker_rejects_when_open`
- `circuit_breaker_transitions_to_half_open_after_cooldown`
- `circuit_breaker_closes_on_half_open_success`
- `circuit_breaker_reopens_on_half_open_failure`
- `circuit_breaker_reset_closes_from_any_state`
- `circuit_breaker_failure_count_resets_on_success`

---

#### Task 05.03 — Integrate circuit breaker with `GitHubClient`

> **Priority:** P1  
> **Depends on:** Task 05.02  
> **Blocked by:** nothing

**File:** `unblock-github/src/client.rs`

Requirements:
- Add `circuit_breaker: CircuitBreaker` field to `GitHubClient`
- Before every GitHub API call (GraphQL and REST): `self.circuit_breaker.check()?`
- After successful response: `self.circuit_breaker.record_success()`
- After network error or 5xx response: `self.circuit_breaker.record_failure()`
- 4xx responses (except 429) are NOT circuit breaker failures — they are application errors
- 429 (rate limit) triggers `record_failure()` because sustained rate limiting is a circuit breaker scenario
- The circuit breaker wraps all calls in `graphql_request()` and `rest_request()` — the two lowest-level methods

**Tests:**
- `client_returns_circuit_breaker_open_after_threshold_failures`
- `client_recovers_after_cooldown`

---

### Epic 06 — Retry with Exponential Backoff

**Goal:** Automatic retry for transient GitHub API errors with exponential backoff and jitter.

**Crate:** `unblock-github`

---

#### Task 06.01 — Define `RetryPolicy`

> **Priority:** P0  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `src/resilience.rs`

Requirements:
- `RetryPolicy` struct: `max_retries: usize` (default: `3`), `base_delay: Duration` (default: `500ms`), `max_delay: Duration` (default: `5s`)
- `RetryPolicy::default()` returns `{ max_retries: 3, base_delay: 500ms, max_delay: 5s }`
- `RetryPolicy::compute_delay(&self, attempt: usize) -> Duration` — exponential: `base_delay * 2^attempt`, capped at `max_delay`, with ±25% jitter
- Jitter implementation: `delay * (0.75 + rand::random::<f64>() * 0.5)` — uniform in `[0.75, 1.25]` of the computed delay

```rust
pub struct RetryPolicy {
    pub max_retries: usize,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl RetryPolicy {
    pub fn compute_delay(&self, attempt: usize) -> Duration {
        let base = self.base_delay.as_millis() as u64 * 2u64.pow(attempt as u32);
        let capped = base.min(self.max_delay.as_millis() as u64);
        let jitter_factor = 0.75 + rand::random::<f64>() * 0.5;
        Duration::from_millis((capped as f64 * jitter_factor) as u64)
    }
}
```

**Tests:**
- `retry_policy_default_values`
- `retry_policy_delay_increases_exponentially`
- `retry_policy_delay_capped_at_max`
- `retry_policy_delay_has_jitter` — run 100 times, assert not all equal

---

#### Task 06.02 — Implement retry wrapper

> **Priority:** P0  
> **Depends on:** Task 06.01  
> **Blocked by:** nothing

**File:** `src/resilience.rs`

A generic async retry function that wraps any fallible operation.

Requirements:
- `async fn retry_with_backoff<F, Fut, T>(policy: &RetryPolicy, should_retry: impl Fn(&Error) -> bool, operation: F) -> Result<T, Error>`
  - where `F: Fn() -> Fut`, `Fut: Future<Output = Result<T, Error>>`
- Only retries when `should_retry(&error)` returns `true`
- Sleeps `policy.compute_delay(attempt)` between retries
- After `max_retries` exhausted, returns the last error
- Logs each retry attempt at `warn` level with attempt number and delay

**Retryable conditions:**
- `Error::RateLimited` (HTTP 429) — always retryable
- `Error::GitHubUnavailable` where status is 503 — retryable
- All other errors — NOT retryable, propagate immediately

```rust
pub async fn retry_with_backoff<F, Fut, T>(
    policy: &RetryPolicy,
    should_retry: impl Fn(&Error) -> bool,
    operation: F,
) -> Result<T, Error>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{ /* ... */ }

pub fn is_retryable(error: &Error) -> bool {
    matches!(error, Error::RateLimited | Error::GitHubUnavailable { .. })
}
```

**Tests:**
- `retry_succeeds_on_first_attempt`
- `retry_succeeds_after_transient_failure`
- `retry_gives_up_after_max_retries`
- `retry_does_not_retry_non_retryable_errors`
- `retry_respects_delay_between_attempts` — verify sleep was called

---

#### Task 06.03 — Integrate retry with `GitHubClient`

> **Priority:** P1  
> **Depends on:** Task 06.02, Task 05.03  
> **Blocked by:** nothing

**File:** `unblock-github/src/client.rs`

Requirements:
- Add `retry_policy: RetryPolicy` field to `GitHubClient`
- Wrap `graphql_request()` and `rest_request()` with `retry_with_backoff()`
- Retry happens INSIDE the circuit breaker check — sequence: `circuit_breaker.check()` → `retry_with_backoff(operation)` → success/failure → `record_success()`/`record_failure()`
- The circuit breaker sees the final result after all retries are exhausted

**Tests:**
- `client_retries_on_429`
- `client_retries_on_503`
- `client_does_not_retry_on_404`
- `client_circuit_breaker_sees_final_result_after_retries`

---

### Epic 07 — OpenTelemetry Metrics

**Goal:** Optional metric export via OpenTelemetry for operational dashboards.

**Crate:** `unblock-mcp`

---

#### Task 07.01 — Add `otel` cargo feature flag

> **Priority:** P2  
> **Depends on:** nothing  
> **Blocked by:** nothing

**Files:** `unblock-mcp/Cargo.toml`, `unblock-github/Cargo.toml`

Requirements:
- Add `otel` feature to `unblock-mcp/Cargo.toml`:
  ```toml
  [features]
  otel = ["opentelemetry", "opentelemetry-otlp", "opentelemetry_sdk", "tracing-opentelemetry"]
  ```
- Feature is NOT in `default` — OTel is opt-in
- When `otel` is not enabled, metric code compiles to no-ops

**Tests:**
- `cargo check -p unblock-mcp` — compiles without `otel`
- `cargo check -p unblock-mcp --features otel` — compiles with `otel`

---

#### Task 07.02 — Define metric instruments

> **Priority:** P2  
> **Depends on:** Task 07.01  
> **Blocked by:** nothing

**File:** `src/metrics.rs`

Requirements — 8 metrics matching SPEC §13.3:

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

- Use `opentelemetry::metrics::Meter` for instrument creation
- Provide a `Metrics` struct that holds all instruments
- `Metrics::noop()` — returns a no-op instance when `otel` feature is disabled (via `cfg`)

```rust
pub struct Metrics {
    pub tool_duration: Histogram<f64>,
    pub github_request_duration: Histogram<f64>,
    pub cache_hits: Counter<u64>,
    pub cache_misses: Counter<u64>,
    pub graph_nodes: Gauge<u64>,
    pub graph_edges: Gauge<u64>,
    pub graph_cycles: Gauge<u64>,
    pub graph_recalculations: Counter<u64>,
}
```

**Tests:**
- `metrics_noop_does_not_panic` — all instruments callable
- `metrics_creation_with_real_meter` (feature = "otel")

---

#### Task 07.03 — Initialize OTel exporter

> **Priority:** P2  
> **Depends on:** Task 07.02  
> **Blocked by:** nothing

**File:** `src/metrics.rs`

Requirements:
- Read `UNBLOCK_OTEL_ENDPOINT` from config
- If set: initialize OTLP exporter with the endpoint, create `Meter` and `Metrics`
- If not set: use `Metrics::noop()`
- Shutdown gracefully on server exit: `opentelemetry::global::shutdown_tracer_provider()`
- Never fail to start — OTel initialization errors are logged at `warn`, fallback to noop

```rust
pub fn init_metrics(config: &Config) -> Metrics {
    match config.otel_endpoint.as_ref() {
        Some(endpoint) => { /* initialize OTLP exporter */ },
        None => Metrics::noop(),
    }
}
```

**Tests:**
- `init_metrics_returns_noop_without_endpoint`
- `init_metrics_does_not_panic_on_invalid_endpoint`

---

#### Task 07.04 — Instrument tool handlers

> **Priority:** P2  
> **Depends on:** Task 07.03  
> **Blocked by:** nothing

**Files:** All `src/tools/*.rs`

Requirements:
- Add `metrics: Arc<Metrics>` to `ServerState`
- In each tool handler, record:
  - `tool_duration` — `Instant::now()` at start, record on completion
  - `cache_hits` / `cache_misses` — when cache is consulted
  - `graph_nodes`, `graph_edges`, `graph_cycles` — after graph rebuild
  - `graph_recalculations` — when graph is rebuilt (with `trigger` label)
- In `GitHubClient`: record `github_request_duration` for each API call

**Tests:**
- `tool_handler_records_duration_metric` — verify metric value changes after tool call

---

### Epic 08 — Agent Client Detection

**Goal:** Identify which AI client is connected and surface this in logs, `prime` output, and tracing spans — without affecting tool behaviour.

**Crate:** `unblock-core` (types), `unblock-mcp` (integration)

**Status:** ✅ Implemented early during Phase 01. `AgentKind`, `AgentClient` in `unblock-core/src/client.rs`. `ClientDetector` in `unblock-core/src/detection.rs`. Integrated in `ServerState` via `OnceLock<AgentKind>`.

---

#### Task 08.01 — Define `AgentKind` and `AgentClient` types

> **Priority:** P1  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `unblock-core/src/client.rs`

Requirements:
- `AgentKind` enum: `ClaudeCode`, `Copilot`, `Cursor`, `Cline`, `Aider`, `Unknown(String)`
- `AgentKind::from_client_name(&str) -> Self` — case-insensitive substring match: `"claude"` → `ClaudeCode`, `"copilot"` → `Copilot`, `"cursor"` → `Cursor`, `"cline"` → `Cline`, `"aider"` → `Aider`, other → `Unknown(name.to_owned())`
- `AgentKind::as_str() -> &str` — stable lowercase identifiers: `"claude-code"`, `"copilot"`, `"cursor"`, `"cline"`, `"aider"`, `name` for unknown
- `impl Display for AgentKind` — delegates to `as_str()`
- `AgentClient` struct: `name: String`, `version: String`
- `AgentClient::kind(&self) -> AgentKind` — derives from `self.name`
- Derives for `AgentKind`: `Debug`, `Clone`, `PartialEq`
- Derives for `AgentClient`: `Debug`, `Clone`

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum AgentKind {
    ClaudeCode,
    Copilot,
    Cursor,
    Cline,
    Aider,
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct AgentClient {
    pub name: String,
    pub version: String,
}
```

**Tests:**
- `agent_kind_from_claude_code` — `"Claude Code"` → `ClaudeCode`
- `agent_kind_from_copilot` — `"GitHub Copilot"` → `Copilot`
- `agent_kind_from_cursor` — `"Cursor"` → `Cursor`
- `agent_kind_from_cline` — `"Cline"` → `Cline`
- `agent_kind_from_aider` — `"aider"` → `Aider`
- `agent_kind_from_unknown` — `"SomeOtherClient"` → `Unknown("SomeOtherClient")`
- `agent_kind_case_insensitive` — `"CLAUDE CODE"` → `ClaudeCode`
- `agent_kind_display_matches_as_str`
- `agent_client_kind_derives_from_name`

---

#### Task 08.02 — Implement `ClientDetector`

> **Priority:** P1  
> **Depends on:** Task 08.01  
> **Blocked by:** nothing

**File:** `unblock-core/src/detection.rs`

Requirements:
- `ClientDetector::from_env() -> Option<AgentKind>` — checks environment variables in order:
  1. `CLAUDE_CODE_ENTRYPOINT` → `AgentKind::ClaudeCode`
  2. `GITHUB_COPILOT_TOKEN` → `AgentKind::Copilot`
  3. `CURSOR_TRACE_ID` → `AgentKind::Cursor`
  4. None → `None`
- `ClientDetector::resolve(mcp_client: Option<&AgentClient>) -> AgentKind` — priority: MCP `clientInfo` → env vars → `Unknown("unknown")`
- Both methods `#[must_use]`
- No `VSCODE_PID` — too broad (any VS Code session). See design decision D6
- Pure function, no `async`, no I/O side effects

```rust
pub struct ClientDetector;

impl ClientDetector {
    #[must_use]
    pub fn from_env() -> Option<AgentKind> { /* ... */ }

    #[must_use]
    pub fn resolve(mcp_client: Option<&AgentClient>) -> AgentKind { /* ... */ }
}
```

**Tests:**
- `detector_from_env_claude_code` — set `CLAUDE_CODE_ENTRYPOINT`
- `detector_from_env_copilot` — set `GITHUB_COPILOT_TOKEN`
- `detector_from_env_cursor` — set `CURSOR_TRACE_ID`
- `detector_from_env_none` — no env vars set
- `detector_resolve_mcp_overrides_env` — MCP says Cursor, env says Claude → Cursor
- `detector_resolve_falls_back_to_env`
- `detector_resolve_unknown_when_no_signal`

---

#### Task 08.03 — MCP `initialize` capture and `OnceLock` storage

> **Priority:** P1  
> **Depends on:** Task 08.02  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/server.rs`

Requirements:
- Add `agent_kind: OnceLock<AgentKind>` to `ServerState`
- Override `initialize()` in `ServerHandler` implementation:
  1. Extract `client_info` from `InitializeRequestParams`
  2. Construct `AgentClient { name, version }`
  3. Resolve via `ClientDetector::resolve(Some(&agent_client))`
  4. Store in `OnceLock` — `let _ = self.state.agent_kind.set(kind.clone())`
  5. Emit `tracing::info!` with `client.name`, `client.version`, `client.kind` fields
  6. Delegate to rmcp default for the rest of the initialize flow

```rust
pub struct ServerState {
    pub config: Config,
    pub github: GitHubClient,
    pub cache: GraphCache,
    pub agent_kind: OnceLock<AgentKind>,
}
```

**Tests (integration):**
- `initialize_stores_agent_kind_in_once_lock`
- `initialize_with_missing_client_info_falls_back_to_env`
- `initialize_emits_tracing_info_event`

---

#### Task 08.04 — `agent.client` and `agent.kind` span fields

> **Priority:** P2  
> **Depends on:** Task 08.03  
> **Blocked by:** nothing

**Files:** All tool handlers in `src/tools/`

Requirements:
- Read `AgentKind` from `state.agent_kind.get()` (returns `Option<&AgentKind>`)
- Add `agent.client` and `agent.kind` as fields on the root `tracing::info_span!` for each tool call
- If `agent_kind` not yet set (shouldn't happen after `initialize`), use `"unknown"`
- Token MUST NOT be leaked into spans

**Tests:**
- `tool_span_includes_agent_kind_fields` — log-capture integration test

---

### Epic 09 — Prime Integration & Drift Warnings

**Goal:** Integrate reconciliation and agent detection into the `prime` tool for session context enrichment.

**Crate:** `unblock-mcp`

---

#### Task 09.01 — Add `SessionMeta` to `PrimeResult`

> **Priority:** P1  
> **Depends on:** Task 08.03  
> **Blocked by:** nothing

**File:** `src/tools/prime.rs`

Requirements:
- `SessionMeta` struct: `agent_client: String` (raw name), `agent_kind: String` (resolved kind), `agent_field: Option<String>` (`UNBLOCK_AGENT` value), `connected_at: DateTime<Utc>`
- Add `session: SessionMeta` to `PrimeResult`
- Read agent kind from `ServerState.agent_kind` (`OnceLock`)
- Read raw client name from rmcp's `Peer<RoleServer>.peer_info()` if available

```rust
#[derive(Debug, Serialize)]
pub struct SessionMeta {
    pub agent_client: String,
    pub agent_kind: String,
    pub agent_field: Option<String>,
    pub connected_at: DateTime<Utc>,
}
```

**Tests:**
- `prime_includes_session_meta`
- `session_meta_populates_agent_kind`

---

#### Task 09.02 — Add drift warnings to `prime` output

> **Priority:** P1  
> **Depends on:** Task 02.02, Task 09.01  
> **Blocked by:** nothing

**File:** `src/tools/prime.rs`

Requirements:
- After building prime context, run a **read-only** reconciliation in background: `tokio::spawn(handle_reconcile(ReconcileParams { fix: false, stale_claim_hours: 24 }))`
- Await result with timeout (2s) — if timeout, skip drift warnings
- If drift detected: add `drift_warnings: Option<Vec<String>>` to `PrimeResult` with human-readable summaries
- The reconciliation does NOT delay prime — it runs concurrently and results are best-effort

```rust
// In PrimeResult:
pub drift_warnings: Option<Vec<String>>,
```

**Tests:**
- `prime_includes_drift_warnings_when_drift_detected`
- `prime_completes_without_warnings_when_no_drift`
- `prime_completes_even_if_drift_check_times_out`

---

### Epic 10 — GitHub Actions Reconciliation Sentinel

**Goal:** Automatic drift detection and repair when the MCP server is not running and a human makes mutations via the GitHub UI.

**Crate:** N/A (GitHub Actions workflow)

---

#### Task 10.01 — Create `unblock-reconcile.yml` workflow

> **Priority:** P3  
> **Depends on:** Task 02.02  
> **Blocked by:** nothing

**File:** `.github/workflows/unblock-reconcile.yml`

Requirements:
- Trigger: `issues` events (`closed`, `reopened`, `edited`, `deleted`) and `issue_comment.created`
- Skip if actor is `github-actions[bot]` (prevent infinite loops)
- Install `unblock-mcp` binary via shell installer
- Run `unblock-mcp reconcile --fix --output json`
- Log drift items found

```yaml
name: Unblock Reconcile
on:
  issues:
    types: [closed, reopened, edited, deleted]
  issue_comment:
    types: [created]

jobs:
  reconcile:
    runs-on: ubuntu-latest
    if: github.actor != 'github-actions[bot]'
    steps:
      - uses: actions/checkout@v4
      - name: Install unblock
        run: curl -fsSL https://get.unblock.dev | sh
      - name: Reconcile
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: unblock-mcp reconcile --fix --output json
```

**Tests:**
- Manual validation — workflow syntax passes `actionlint`

---

## 6. Definition of Done

Phase 02 is complete when:

1. **All 10 epics are implemented** — reconcile engine, reconcile tool, commit_context tool, doctor tool, circuit breaker, retry, OpenTelemetry, agent detection, prime integration, Actions sentinel
2. **Quality gate passes:**
   ```bash
   cargo fmt --check --all                                    # zero diffs
   cargo clippy --workspace --all-targets -- -D warnings      # zero warnings
   cargo test --workspace                                     # all pass
   cargo doc --no-deps --workspace                            # zero warnings
   ```
3. **`reconcile` detects 100% of 7 drift types** in the test corpus (unit tests with synthetic drift)
4. **Circuit breaker activates** within 60s of sustained GitHub API failure (integration test)
5. **Retry with backoff** correctly handles 429 and 503, propagates other errors immediately
6. **OpenTelemetry export** produces metrics when `UNBLOCK_OTEL_ENDPOINT` is configured (integration test with in-memory exporter)
7. **Agent detection** correctly identifies Claude Code, Copilot, Cursor, Cline, Aider from MCP `clientInfo` and env fallback
8. **`prime` includes `SessionMeta`** and drift warnings in output
9. **Coverage target:** >80% for all new code
10. **No regressions** — all existing Phase 01 tests continue to pass
11. **20 MCP tools registered** — existing 17 + reconcile + commit_context + doctor

---

*This plan defines what to build in Phase 02. The why is in the PRD §7 Phase 02. The how is in the SPEC §6, §12–14. Detailed algorithms and edge cases are in the companion specs.*
