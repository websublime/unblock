# Unblock — Architecture Specification

**Dependency-aware task tracking for AI agents, powered by GitHub.**

| | |
|---|---|
| **Version** | 1.1.0-draft |
| **Author** | Miguel Ramos |
| **Role** | Architect |
| **Org** | websublime |
| **Repo** | `websublime/unblock` |
| **Date** | March 2026 |
| **Status** | Draft |
| **Depends on** | unblock-prd-github.md v1.0.0-draft |

---

## Table of Contents

1. [System Context](#1-system-context)
2. [Design Principles](#2-design-principles)
3. [High-Level Architecture](#3-high-level-architecture)
4. [Component Architecture](#4-component-architecture)
5. [Type System](#5-type-system)
6. [Graph Engine](#6-graph-engine)
7. [Cache Layer](#7-cache-layer)
8. [GitHub API Layer](#8-github-api-layer)
9. [MCP Server](#9-mcp-server)
10. [Tool Specifications](#10-tool-specifications)
11. [Claude Code Plugin](#11-claude-code-plugin)
12. [Configuration System](#12-configuration-system)
13. [Error Architecture](#13-error-architecture)
14. [Observability](#14-observability)
15. [Resilience Patterns](#15-resilience-patterns)
16. [Security](#16-security)
17. [Build and Distribution](#17-build-and-distribution)
18. [Testing Architecture](#18-testing-architecture)

---

## 1. System Context

### 1.1 Context Diagram

```
                                    ┌─────────────────────────────┐
                                    │       Human Developer       │
                                    │  (GitHub UI / gh CLI)       │
                                    └──────────┬──────────────────┘
                                               │ HTTPS
                                               ▼
┌──────────────────┐   MCP/stdio   ┌──────────────────┐   HTTPS   ┌──────────────────┐
│   Claude Code    │◄─────────────►│   unblock-mcp    │◄─────────►│   GitHub API     │
│   (or any MCP    │               │   (Rust binary)  │           │   GraphQL + REST │
│    client)       │               │                  │           │                  │
└──────────────────┘               └──────────────────┘           └──────────────────┘
                                           │
                                    In-memory graph
                                    cache (petgraph)
```

### 1.2 Actors

| Actor | Interface | Operations |
|---|---|---|
| AI Agent | MCP protocol (stdio) | All 17 tools |
| Orchestrator | MCP protocol (stdio) | `claim --agent X`, `ready --agent X`, `blocked` |
| Human Developer | GitHub UI, `gh` CLI | Issue CRUD, Projects boards, labels, milestones |

### 1.3 Boundaries

| Boundary | Inside | Outside |
|---|---|---|
| **Unblock system** | MCP server, graph engine, GitHub client, plugin | GitHub API, Claude Code, git |
| **Trust boundary** | Unblock process | GitHub API (authenticated via PAT/App token) |
| **Data boundary** | In-memory cache (ephemeral) | GitHub (persistent, source of truth) |
| **Compute boundary** | Graph algorithms, ready set calculation | GitHub stores data, Unblock computes over it |

### 1.4 Key Constraints

- GitHub is the **single source of truth**. The MCP server stores nothing persistently.
- The MCP server is **stateless across restarts**. In-memory cache is ephemeral and reconstructable from GitHub in a single API call.
- **One repo = one project.** The MCP server scopes all operations to the repo detected from git remote (or configured via env var).
- **No SSE/WebSocket from GitHub to MCP server.** Consistency relies on TTL-based cache invalidation and explicit invalidation after writes.

---

## 2. Design Principles

### P1: GitHub stores, Rust computes

GitHub holds the data and provides the UI. The MCP server holds the intelligence: graph computation, ready-state calculation, cascade logic, cycle detection. Zero custom storage.

### P2: Every write invalidates and recomputes

After every write operation that affects the dependency graph (create with deps, close, depends, dep_remove), the server invalidates the cache, recomputes the graph, and updates Ready State fields in GitHub. Reads are always consistent after writes.

### P3: Fail open for reads, fail safe for writes

If the graph can't be refreshed (GitHub is slow), serve reads from stale cache with a `stale: true` flag. For writes, always validate before mutating — never create inconsistent state.

### P4: Zero config for the common case

Auto-detect repo from git remote, auto-detect Project from linked projects. The only required config is `GITHUB_TOKEN`. Everything else has sensible defaults.

### P5: The agent is always one command away from productive work

`prime` → `ready` → `claim` must complete in under 2 seconds. If it's slower, agents waste context tokens waiting.

### P6: Testable without GitHub

The graph engine (`unblock-core`) is fully testable with in-memory data. No network calls in unit tests. Integration tests use a real GitHub repo.

---

## 3. High-Level Architecture

### 3.1 Crate Dependency Graph

```
┌───────────────────────────────────────────────────┐
│                   unblock-mcp                      │
│            (binary — MCP server)                   │
│                                                    │
│  ┌──────────┐  ┌───────────┐  ┌────────────────┐ │
│  │  tools/   │  │ server.rs │  │   github/      │ │
│  │  (17)     │  │ (MCP      │  │ (API client)   │ │
│  │           │  │  handler)  │  │                │ │
│  └─────┬─────┘  └─────┬─────┘  └───────┬────────┘ │
│        │               │                │          │
│        └───────────────┼────────────────┘          │
│                        │                           │
└────────────────────────┼───────────────────────────┘
                         │ depends on
                ┌────────▼────────┐
                │  unblock-core   │
                │  (library)      │
                │                 │
                │  types.rs       │
                │  graph.rs       │
                │  cache.rs       │
                │  config.rs      │
                │  errors.rs      │
                └─────────────────┘
```

### 3.2 Module Responsibilities

| Module | Crate | Responsibility |
|---|---|---|
| `types` | core | Domain types: Issue, Dependency, Milestone. Mapped from GitHub, no GitHub-specific fields |
| `graph` | core | Directed dependency graph: build, ready set, unblock cascade, cycle detection, tree traversal |
| `cache` | core | In-memory graph cache with TTL, invalidation, freshness check |
| `config` | core | Configuration loading: env vars → defaults, auto-detection |
| `errors` | core | Domain error types (snafu) |
| `server` | mcp | MCP protocol setup, tool registration, request routing (rmcp) |
| `errors` | mcp | Infrastructure errors, MCP error conversion |
| `tools/*` | mcp | 17 tool implementations, each in its own module |
| `github/client` | mcp | HTTP client wrapper, auth, auto-detect repo |
| `github/graphql` | mcp | Read queries: issues, blocking edges, project field values |
| `github/mutations` | mcp | Write operations: create, update, close, reopen, addBlockedBy, comments |
| `github/project` | mcp | Projects V2 field management: create fields, read/write field values |

---

## 4. Component Architecture

### 4.1 unblock-core

```rust
// crates/unblock-core/src/lib.rs
pub mod types;    // Domain models
pub mod graph;    // Dependency graph engine
pub mod cache;    // Graph cache with TTL
pub mod config;   // Configuration
pub mod errors;   // Error types
```

Depends on: `serde`, `chrono`, `petgraph`, `snafu`, `tracing`. No network crates. No GitHub-specific code.

### 4.2 unblock-mcp

```rust
// crates/unblock-mcp/src/main.rs
mod server;
mod errors;
mod tools;
mod github;
```

Depends on: `unblock-core`, `rmcp`, `reqwest`, `tokio`, `schemars`.

---

## 5. Type System

All domain types are plain Rust structs. No GitHub-specific field names — the GitHub client handles mapping. Types are designed to be backend-agnostic so the graph engine works identically regardless of data source.

```rust
// crates/unblock-core/src/types.rs

/// An issue in the dependency graph.
/// Mapped from GitHub Issue + Projects V2 field values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    pub number: u64,               // GitHub issue number (#42)
    pub node_id: String,           // GitHub GraphQL node ID (opaque, for mutations)
    pub title: String,
    pub issue_type: Option<IssueType>,
    pub status: Status,            // From Projects V2 custom field
    pub priority: Priority,        // From Projects V2 custom field
    pub agent: Option<String>,     // From Projects V2 custom field (free text)
    pub claimed_at: Option<DateTime<Utc>>,  // From Projects V2 custom field
    pub ready_state: ReadyState,   // From Projects V2 custom field (MCP writes, never reads for logic)
    pub story_points: Option<i32>, // From Projects V2 custom field
    pub defer_until: Option<NaiveDate>,     // From Projects V2 custom field
    pub labels: Vec<String>,
    pub milestone: Option<String>, // Milestone title (epic equivalent)
    pub assignees: Vec<String>,    // GitHub usernames (human assignment)
    pub state: IssueState,         // GitHub native: Open/Closed
    pub body: Option<String>,      // Full markdown body
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub url: String,               // HTML URL for linking
}

/// GitHub native issue state. Separate from our workflow Status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
}

/// Workflow status — stored as Projects V2 single-select field.
/// Finer-grained than GitHub's binary Open/Closed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    Open,
    InProgress,
    Blocked,
    Deferred,
    Closed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Priority {
    P0, P1, P2, P3, P4,
}

impl Priority {
    /// Sort key for priority ordering (P0=0, P4=4).
    #[must_use]
    pub fn as_sort_key(&self) -> u8 { /* ... */ }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReadyState {
    Ready, Blocked, NotReady, Closed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueType {
    Task, Bug, Feature, Epic, Chore, Spike,
}

/// A blocking edge in the dependency graph.
/// Mapped from GitHub's native blockedBy relationship.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockingEdge {
    pub source: u64,  // Issue number that is blocked
    pub target: u64,  // Issue number that blocks it
}

/// Summary of an issue for list/ready responses (lighter than full Issue).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueSummary {
    pub number: u64,
    pub title: String,
    pub issue_type: Option<IssueType>,
    pub status: Status,
    pub priority: Priority,
    pub agent: Option<String>,
    pub milestone: Option<String>,
    pub story_points: Option<i32>,
    pub labels: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub url: String,
}

/// Parsed sections from the issue body markdown.
/// Three sections only — each data type lives in the correct GitHub primitive.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BodySections {
    pub description: Option<String>,
    pub design_notes: Option<String>,
    pub acceptance_criteria: Option<String>,
}

impl BodySections {
    /// Parse structured sections from markdown body.
    /// Looks for ## Description, ## Design Notes, ## Acceptance Criteria headers.
    #[must_use]
    pub fn from_markdown(body: &str) -> Self { /* ... */ }

    /// Render sections back to markdown body.
    #[must_use]
    pub fn to_markdown(&self) -> String { /* ... */ }
}
```

**Key design notes:**

- `Issue.number` is `u64`, not a custom `IssueId` struct — GitHub issue numbers are native, universal, and need no wrapper.
- `Issue.state` (GitHub native Open/Closed) is separate from `Issue.status` (Projects V2 workflow). The MCP server uses `status` for logic. `state` is synchronised when closing/reopening.
- `Issue.node_id` is the GraphQL opaque ID needed for mutations. It's stored but never displayed to the user.
- `BlockingEdge` is minimal: just two issue numbers. No dep type — GitHub only has one type of blocking.
- `BodySections` has three sections only. Work progress lives in **comments** (append-only, timestamped, attributed). Cross-references live in **GitHub auto-links** (mentions `#N` in comments/body create bidirectional links automatically). Each data type lives in the correct GitHub primitive, not duplicated in markdown.

---

## 6. Graph Engine

Pure Rust, no network, fully testable with in-memory data.

### 6.1 Data Structure

```rust
// crates/unblock-core/src/graph.rs

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::algo::{has_path_connecting, tarjan_scc};
use std::collections::{HashMap, HashSet};

/// The dependency graph for a single repository.
/// Nodes are issue numbers, edges are blocking relationships.
/// Edge direction: blocked_issue → blocking_issue
/// (source depends on target)
pub struct DependencyGraph {
    graph: DiGraph<u64, ()>,
    node_map: HashMap<u64, NodeIndex>,
    issue_status: HashMap<u64, Status>,
    issue_state: HashMap<u64, IssueState>,
}

impl DependencyGraph {
    /// Build graph from issues and blocking edges.
    pub fn build(issues: &[Issue], edges: &[BlockingEdge]) -> Self { /* ... */ }
}
```

### 6.2 Ready Set Calculation

```rust
impl DependencyGraph {
    /// Compute the set of issues that are ready to work on.
    /// An issue is ready if:
    /// 1. Status == Open (not InProgress, Blocked, Deferred, Closed)
    /// 2. GitHub state == Open (not Closed)
    /// 3. No active blocking dependencies (all blockers are closed)
    ///
    /// Note: Defer Until filtering is a post-filter in the tool, not here.
    /// The graph doesn't know about dates.
    pub fn compute_ready_set(&self) -> HashSet<u64> { /* ... */ }
}
```

**Note on blocking evaluation:** GitHub's native blocking is simpler than Fibery's. An issue is blocked if ANY of its blockers is open. A blocker is resolved when its GitHub state is Closed. No conditional-blocks, no failure reason evaluation. The `is_blocked` check is a single `any()` — cleaner than the Fibery version.

### 6.3 Unblock Cascade

```rust
impl DependencyGraph {
    /// Given an issue that was just closed, compute which issues
    /// become unblocked as a result.
    /// Returns issue numbers whose blocking dependencies are now all resolved.
    pub fn compute_unblock_cascade(&self, closed_number: u64) -> HashSet<u64> { /* ... */ }
}
```

### 6.4 Cycle Detection

```rust
impl DependencyGraph {
    /// Check if adding a blocking edge source → target would create a cycle.
    /// Call BEFORE creating the relationship in GitHub.
    pub fn would_create_cycle(&self, source: u64, target: u64) -> bool { /* ... */ }

    /// Detect all cycles in the graph.
    /// Returns a list of cycles, each as a Vec of issue numbers.
    pub fn detect_all_cycles(&self) -> Vec<Vec<u64>> { /* ... */ }

    /// Build a dependency tree from a given issue.
    /// Used by `show` to display deps and by `dep_cycles` for targeted checks.
    pub fn dependency_tree(
        &self,
        root: u64,
        direction: TraversalDirection,
        max_depth: usize,
    ) -> DependencyTree { /* ... */ }
}

pub enum TraversalDirection {
    Upstream,    // What does this issue depend on?
    Downstream,  // What depends on this issue?
    Both,
}

pub struct DependencyTree {
    pub root: u64,
    pub upstream: Vec<TreeNode>,
    pub downstream: Vec<TreeNode>,
}

pub struct TreeNode {
    pub number: u64,
    pub status: Status,
    pub state: IssueState,
    pub depth: usize,
    pub children: Vec<TreeNode>,
}
```

---

## 7. Cache Layer

```rust
// crates/unblock-core/src/cache.rs

use std::sync::RwLock;
use std::time::{Duration, Instant};
use std::collections::HashSet;

pub struct GraphCache {
    ttl: Duration,
    inner: RwLock<Option<CacheEntry>>,
}

struct CacheEntry {
    graph: DependencyGraph,
    ready_set: HashSet<u64>,
    built_at: Instant,
}

impl GraphCache {
    pub fn new(ttl_seconds: u64) -> Self { /* ... */ }

    /// Get the cached ready set if fresh.
    #[must_use]
    pub fn get_ready_set(&self) -> Option<HashSet<u64>> { /* ... */ }

    /// Get the cached graph for cycle checks and cascade computation.
    #[must_use]
    pub fn get_graph(&self) -> Option<&DependencyGraph> { /* ... */ }

    /// Update the cache with a freshly built graph.
    pub fn update(&self, graph: DependencyGraph) { /* ... */ }

    /// Invalidate the cache (after a write operation).
    pub fn invalidate(&self) { /* ... */ }

    /// Check if cache is fresh.
    #[must_use]
    pub fn is_fresh(&self) -> bool { /* ... */ }
}
```

**Cache lifecycle:**

```
Write operation (close, depends, dep_remove, create+blocked_by, update, reopen)
  │
  ├─→ Execute write in GitHub (mutation)
  ├─→ Invalidate cache
  ├─→ Fetch all open issues + blocking edges (1 GraphQL query)
  ├─→ Build new DependencyGraph
  ├─→ Compute ready set
  ├─→ Diff against current Ready State field values
  ├─→ Batch update changed Ready State fields in GitHub
  └─→ Update cache

Read operation (ready, prime, stats)
  │
  ├─→ Cache fresh? → return cached data (0 API calls)
  └─→ Cache stale/empty → fetch + build + cache + return (1 API call)
```

### 7.1 Field Validation at Boot

After `resolve_project()` completes, the server validates all 7 required Projects V2 fields exist with correct types and option values:

| Field | Type | Required Options |
|---|---|---|
| Status | Single Select | open, in_progress, blocked, deferred, closed |
| Priority | Single Select | P0, P1, P2, P3, P4 |
| Agent | Text | — |
| Claimed At | Date | — |
| Ready State | Single Select | ready, blocked, not_ready, closed |
| Story Points | Number | — |
| Defer Until | Date | — |

- **Missing field** → hard error, server refuses to start. Directs user to run `setup`.
- **Wrong option values** (e.g. missing "deferred" in Status) → warning logged, server continues but may fail on writes to that option.

### 7.2 Concurrency Model

Single-process architecture. Multiple agents sharing the same MCP server instance (via separate stdio connections) share the in-memory cache. Last writer wins — no optimistic locking in v1. This is acceptable because:

- GitHub is the source of truth; cache is ephemeral
- Write operations always invalidate + rebuild from GitHub
- Conflicting field updates resolve via GitHub's own conflict semantics

### 7.3 API Call Optimization

Write operations batch multiple Projects V2 field mutations into a single GraphQL request where possible. Target: any tool completes in <2 seconds wall-clock time.

```rust
impl GitHubClient {
    /// Batch-update multiple fields on a single project item in one GraphQL mutation.
    pub async fn batch_update_fields(
        &self,
        item_id: &str,
        updates: &[(String, FieldValue)],
    ) -> Result<(), Error> { /* ... */ }
}
```

### 7.4 Migration Path

`setup --migrate` adds all existing open issues in the repository to the Project V2 board. Issues already in the project are skipped (idempotent). This allows adopting Unblock on repos with existing issues.

```
Flow:
  1. Fetch all open issues not in the project
  2. For each: addProjectV2Item
  3. Set default field values (Status=open, Priority=P2, Ready State=not_ready)
  4. Report count of migrated issues
```

---

## 8. GitHub API Layer

### 8.1 Client Architecture

```rust
// crates/unblock-mcp/src/github/client.rs

pub struct GitHubClient {
    http: reqwest::Client,
    token: String,
    api_base_url: String,            // GITHUB_API_URL (default: "https://api.github.com")
    owner: String,
    repo: String,
    project_number: Option<u64>,
    project_id: Option<String>,      // GraphQL node ID for the Project
    field_ids: Option<ProjectFieldIds>, // Cached field IDs after setup
}

/// Cached IDs for Projects V2 custom fields.
/// Resolved once at startup or on first tool call.
pub struct ProjectFieldIds {
    pub status: FieldMeta,
    pub priority: FieldMeta,
    pub agent: String,         // Field node ID (text field, no options)
    pub claimed_at: String,    // Field node ID (date field)
    pub ready_state: FieldMeta,
    pub story_points: String,  // Field node ID (number field)
    pub defer_until: String,   // Field node ID (date field)
}

/// Single-select field metadata: field ID + option IDs.
pub struct FieldMeta {
    pub field_id: String,
    pub options: HashMap<String, String>, // "open" → "option_id_xxx"
}

pub enum FieldValue {
    SingleSelect(String),  // Option ID
    Text(String),
    Date(String),          // ISO format
    Number(f64),
}

impl GitHubClient {
    /// Create client with auto-detection.
    /// 1. Token from GITHUB_TOKEN env var (required)
    /// 2. API base URL from GITHUB_API_URL env var (default: "https://api.github.com")
    /// 3. Repo from UNBLOCK_REPO env var or git remote
    /// 4. Project from UNBLOCK_PROJECT env var or first linked project
    pub async fn new(config: &Config) -> Result<Self, Error> { /* ... */ }

    /// Auto-detect owner/repo from git remote.
    fn resolve_repo(config: &Config) -> Result<(String, String), Error> { /* ... */ }

    /// Build a REST API URL. Works for both github.com and GHE.
    /// e.g. "{api_base_url}/repos/{owner}/{repo}/issues"
    fn rest_url(&self, path: &str) -> String {
        format!("{}{path}", self.api_base_url)
    }

    /// Build the GraphQL endpoint URL.
    /// github.com:   "https://api.github.com/graphql"
    /// GHE Server:   "https://<host>/api/graphql" (strips /v3 suffix)
    /// GHE Cloud:    "https://api.<host>/graphql"
    fn graphql_url(&self) -> String {
        let base = self.api_base_url.strip_suffix("/v3").unwrap_or(&self.api_base_url);
        format!("{base}/graphql")
    }
}
```

### 8.2 GraphQL Read Queries

```rust
// crates/unblock-mcp/src/github/graphql.rs

impl GitHubClient {
    /// Fetch all open issues with blocking relationships and Project field values.
    /// This is the primary read query — used to build the dependency graph.
    /// Single GraphQL query with pagination, returns everything needed for graph construction.
    pub async fn fetch_graph_data(&self) -> Result<(Vec<Issue>, Vec<BlockingEdge>), Error> { /* ... */ }

    /// Fetch a single issue with full details (for `show` tool).
    pub async fn fetch_issue(&self, number: u64) -> Result<Issue, Error> { /* ... */ }

    /// Low-level GraphQL request. Uses `self.graphql_url()` for endpoint resolution.
    async fn graphql(&self, query: &str, variables: serde_json::Value) -> Result<serde_json::Value, Error> { /* ... */ }
}
```

### 8.3 Mutations

```rust
// crates/unblock-mcp/src/github/mutations.rs

impl GitHubClient {
    /// Create a new issue via REST API.
    pub async fn create_issue(
        &self,
        title: &str,
        body: Option<&str>,
        labels: &[String],
        milestone: Option<u64>,
        issue_type: Option<&str>,
    ) -> Result<Issue, Error> { /* ... */ }

    /// Close an issue via REST API.
    pub async fn close_issue(&self, number: u64) -> Result<(), Error> { /* ... */ }

    /// Reopen a closed issue via REST API.
    pub async fn reopen_issue(&self, number: u64) -> Result<(), Error> { /* ... */ }

    /// Update issue fields via REST API (title, body, labels, assignees, milestone).
    pub async fn update_issue(
        &self,
        number: u64,
        updates: &IssueUpdates,
    ) -> Result<(), Error> { /* ... */ }

    /// Add a comment to an issue via REST API.
    pub async fn add_comment(&self, number: u64, body: &str) -> Result<(), Error> { /* ... */ }

    /// Add a blocking relationship via GraphQL.
    pub async fn add_blocked_by(&self, issue_id: &str, blocker_id: &str) -> Result<(), Error> { /* ... */ }

    /// Remove a blocking relationship via GraphQL.
    pub async fn remove_blocked_by(&self, issue_id: &str, blocker_id: &str) -> Result<(), Error> { /* ... */ }

    /// Add an issue as sub-issue of a parent via GraphQL.
    pub async fn add_sub_issue(&self, parent_id: &str, child_id: &str) -> Result<(), Error> { /* ... */ }
}
```

### 8.4 Projects V2 Field Management

```rust
// crates/unblock-mcp/src/github/project.rs

impl GitHubClient {
    /// Resolve Project V2 node ID and custom field IDs.
    /// Called once at startup. Caches results in self.project_id and self.field_ids.
    pub async fn resolve_project(&mut self) -> Result<(), Error> { /* ... */ }

    /// Update a Projects V2 field value on an issue.
    pub async fn update_field(
        &self,
        item_id: &str,  // ProjectV2Item ID (not issue ID)
        field_id: &str,
        value: FieldValue,
    ) -> Result<(), Error> { /* ... */ }

    /// Batch-update multiple fields on a single project item in one GraphQL mutation.
    pub async fn batch_update_fields(
        &self,
        item_id: &str,
        updates: &[(String, FieldValue)],
    ) -> Result<(), Error> { /* ... */ }

    /// Create custom fields on the project (setup tool).
    /// Idempotent — skips fields that already exist.
    pub async fn setup_fields(&self) -> Result<(), Error> { /* ... */ }

    /// Migrate existing open issues into the Project V2 board.
    /// Idempotent — skips issues already in the project.
    pub async fn migrate_issues(&self) -> Result<usize, Error> { /* ... */ }
}
```

---

## 9. MCP Server

### 9.1 Server Bootstrap

```rust
// crates/unblock-mcp/src/main.rs

use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> { /* ... */ }
```

The bootstrap sequence: load config → init tracing → create GitHub client (resolves repo, project, fields) → validate fields (see §7.1) → create cache → create server → serve on stdio.

### 9.2 Server State and Tool Registration

```rust
// crates/unblock-mcp/src/server.rs

use rmcp::{ServerHandler, tool, ServerInfo, ServerCapabilities};

pub struct ServerState {
    pub config: Config,
    pub github: GitHubClient,
    pub cache: GraphCache,
}

pub struct UnblockServer {
    state: Arc<ServerState>,
}

impl UnblockServer {
    pub fn new(state: ServerState) -> Self { /* ... */ }
}

// Tool registration via rmcp macros — 17 tools total.
// Pattern (representative examples):

#[tool(name = "ready", description = "Find issues that can be worked on right now — no active blockers")]
async fn ready(&self, params: ReadyParams) -> Result<ReadyResult, McpError> {
    tools::ready::execute(&self.state, params).await
}

#[tool(name = "close", description = "Close an issue and cascade-unblock dependents")]
async fn close(&self, params: CloseParams) -> Result<CloseResult, McpError> {
    tools::close::execute(&self.state, params).await
}

#[tool(name = "update", description = "Update issue fields (priority, status, labels, body sections, etc.)")]
async fn update(&self, params: UpdateParams) -> Result<UpdateResult, McpError> {
    tools::update::execute(&self.state, params).await
}

// ... remaining 14 tools follow the same pattern

impl ServerHandler for UnblockServer {
    fn get_info(&self) -> ServerInfo { /* ... */ }
}
```

### 9.3 Tool Execution Pattern

Every tool follows the same pattern:

```rust
pub async fn execute(state: &ServerState, params: P) -> Result<R, McpError> {
    // 1. Validate input
    // 2. Execute business logic (GitHub API calls)
    // 3. If write: invalidate cache + rebuild + update Ready State fields
    // 4. Return result

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

---

## 10. Tool Specifications

### 10.1 `ready`

```
Input:  ReadyParams { limit?, type?, priority?, milestone?, agent?, label?, include_claimed? }
Output: ReadyResult { issues: [IssueSummary], count, stale }

Flow:
  1. Check cache → fresh? filter + defer post-filter + return
  2. Stale/empty → fetch_graph_data (1 GraphQL query with pagination)
  3. Build graph → compute ready set → update cache
  4. Filter by params
  5. Post-filter: exclude defer_until > today
  6. Sort by priority ASC → created ASC
  7. Return top N

API calls: 0 (cache hit) | 1+ (rebuild, depends on pagination)
```

### 10.2 `claim`

```
Input:  ClaimParams { id, agent? }
Output: ClaimResult { issue }

Flow:
  1. Fetch issue (single issue query)
  2. Validate: Status=open, not blocked (check cache or graph), not deferred
  3. Update Project fields: Status→in_progress, Agent→name, Claimed At→now, Ready State→not_ready
  4. Add comment: "Claimed by {agent} at {timestamp}"
  5. Invalidate cache + rebuild + update Ready State fields

API calls: 1 (fetch) + 4 (field updates) + 1 (comment) + 1 (rebuild) = ~7
```

### 10.3 `close`

```
Input:  CloseParams { id, reason? }
Output: CloseResult { issue, unblocked: [number] }

Flow:
  1. Fetch issue, validate not closed
  2. Close issue (REST PATCH state=closed)
  3. Update Project fields: Status→closed, Ready State→closed
  4. Add comment: "Closed: {reason}"
  5. Rebuild graph, compute cascade
  6. For each unblocked: update Status→open, Ready State→ready, add comment
  7. Update cache

API calls: 1 (fetch) + 1 (close) + 2 (field updates) + 1 (comment)
           + 1 (rebuild) + N×3 (per unblocked: 2 fields + 1 comment)
```

### 10.4 `create`

```
Input:  CreateParams { title, type?, priority?, body?, labels?, milestone?, blocked_by?, parent?, story_points?, defer_until? }
Output: CreateResult { issue }

Flow:
  1. Create issue (REST POST)
  2. Add to Project (mutation addProjectV2Item)
  3. Set fields: Priority, Status=open, Ready State, Story Points, Defer Until
  4. If blocked_by: addBlockedBy + set Status→blocked, Ready State→blocked
  5. If parent: addSubIssue
  6. Invalidate cache + rebuild

API calls: 1 (create) + 1 (add to project) + 3-5 (field updates) + 0-2 (deps/parent)
```

### 10.5 `depends`

```
Input:  DependsParams { source, target }
Output: DependsResult { created: true }

Flow:
  1. Fetch both issues (or use cache)
  2. Cycle detection: would_create_cycle(source, target)
  3. addBlockedBy mutation
  4. Update source fields: Status→blocked, Ready State→blocked
  5. Invalidate cache + rebuild

API calls: 0-1 (cache or fetch) + 1 (mutation) + 2 (field updates) + 1 (rebuild)
```

### 10.6 `show`

```
Input:  ShowParams { id, include_comments?, include_deps? }
Output: ShowResult { issue with full details, parsed body sections, deps, comments }

Flow:
  1. Single GraphQL query (fetch_issue with comments, blockedBy, blocking, parent, subIssues, fields)
  2. Parse body sections (BodySections::from_markdown)
  3. Return

API calls: 1
```

### 10.7 `setup`

```
Input:  SetupParams { project?, dry_run?, migrate? }
Output: SetupResult { fields_created: [name], project_number, migrated_count? }

Flow:
  1. Resolve project (auto-detect or param)
  2. Query existing fields
  3. Create missing fields (7 total, skip existing)
  4. If migrate: add existing open issues to project (see §7.4)
  5. Report

API calls: 1 (query fields) + 0-7 (create missing fields) + 0-N (migrate issues)
Idempotent: safe to run multiple times.
```

### 10.8 `comment`

```
Input:  CommentParams { id, body }
Output: CommentResult { created: true }

Flow:
  1. Verify issue exists (optional — GitHub returns 404 if not)
  2. POST /repos/{owner}/{repo}/issues/{id}/comments with body

API calls: 1
No cache invalidation — comments don't affect the dependency graph.
```

### 10.9 `list`

```
Input:  ListParams { status?, priority?, type?, milestone?, agent?, label?, assignee?, sort?, limit?, offset? }
Output: ListResult { issues: [IssueSummary], total, stale }

Flow:
  1. Fetch graph data (or use cache)
  2. Filter by all params
  3. Sort by requested field
  4. Paginate with offset/limit
  5. Return

API calls: 0 (cache hit) | 1+ (rebuild)
```

### 10.10 `search`

```
Input:  SearchParams { query, limit? }
Output: SearchResult { issues: [IssueSummary], count }

Flow:
  1. GitHub search API: repo:{owner}/{repo} is:issue {query}
  2. Map results to IssueSummary
  3. Return

API calls: 1
```

### 10.11 `stats`

```
Input:  StatsParams { milestone? }
Output: StatsResult { total, by_status, by_priority, blocked_count, ready_count, cycle_count, agents }

Flow:
  1. Fetch graph data (or use cache)
  2. Aggregate counts
  3. Return

API calls: 0 (cache hit) | 1+ (rebuild)
```

### 10.12 `prime`

```
Input:  PrimeParams { }
Output: PrimeResult { context: String }

Flow:
  1. Fetch graph data (or use cache)
  2. Build context summary: repo, project, ready count, blocked count, in-progress, cycles
  3. Return markdown context blob for agent injection

API calls: 0 (cache hit) | 1+ (rebuild)
```

### 10.13 `dep_remove`

```
Input:  DepRemoveParams { source, target }
Output: DepRemoveResult { removed: true }

Flow:
  1. Validate edge exists
  2. removeBlockedBy mutation
  3. Rebuild graph, recompute ready states
  4. Update cache

API calls: 1 (mutation) + 1 (rebuild) + N (field updates)
```

### 10.14 `dep_cycles`

```
Input:  DepCyclesParams { id? }
Output: DepCyclesResult { cycles: [[number]], count }

Flow:
  1. Fetch graph data (or use cache)
  2. If id: targeted cycle check from that node
  3. Else: detect_all_cycles on full graph
  4. Return

API calls: 0 (cache hit) | 1+ (rebuild)
```

### 10.15 `update`

```
Input:  UpdateParams { id, priority?, status?, labels_add?, labels_remove?, assignees_add?, assignees_remove?, body_section?, milestone?, story_points?, defer_until? }
Output: UpdateResult { issue }

Flow:
  1. Fetch issue (single issue query)
  2. Validate issue exists and is open
  3. Update REST fields (labels, assignees, milestone, body) via PATCH
  4. Update Project V2 fields (priority, status, story_points, defer_until) via GraphQL
  5. If body_section: parse existing body → update section → PATCH body
  6. Invalidate cache + rebuild + update Ready State fields

API calls: 1 (fetch) + N (updates) + 1 (rebuild)
```

### 10.16 `reopen`

```
Input:  ReopenParams { id, reason? }
Output: ReopenResult { issue }

Flow:
  1. Fetch issue (single issue query)
  2. Validate issue is closed (error if already open: IssueAlreadyOpen)
  3. Reopen issue (REST PATCH state=open)
  4. Rebuild graph to evaluate blocking status
  5. If has active blockers: set Status→blocked, Ready State→blocked
  6. Else: set Status→open, Ready State→ready
  7. Add comment: "Reopened: {reason}"
  8. Invalidate cache + rebuild + update Ready State fields

API calls: 1 (fetch) + 1 (reopen) + 2-3 (field updates) + 1 (comment) + 1 (rebuild)
```

### 10.17 `doctor`

```
Input:  DoctorParams { fix? }
Output: DoctorResult { status: "healthy" | "degraded" | "broken", checks: [Check] }

Check = { name, status: "pass" | "warn" | "fail", message?, fixed? }

Flow:
  1. Verify project linked to repo (project_linked)
  2. Verify all 7 fields exist with correct types/options (fields_valid)
  3. Check all open issues are in the project (all_issues_in_project)
  4. Run cycle detection on full graph (no_cycles)
  5. Check for orphaned blocking edges referencing closed/deleted issues (no_orphaned_edges)
  6. Verify cache freshness and consistency (cache_fresh)
  7. If fix=true: attempt repairs (add missing issues to project, clear orphaned edges, rebuild cache)

Checks: project_linked, fields_valid, all_issues_in_project, no_cycles, no_orphaned_edges, cache_fresh
API calls: 2-3 (queries) + 0-N (repairs if fix=true)
```

---

## 11. Claude Code Plugin

### 11.1 Structure

```
plugin/
├── .claude-plugin/
│   └── plugin.json
├── .mcp.json
├── marketplace.json
├── commands/
│   ├── ready.md
│   ├── claim.md
│   ├── close.md
│   ├── create.md
│   ├── depends.md
│   ├── dep-remove.md
│   ├── dep-cycles.md
│   ├── show.md
│   ├── list.md
│   ├── search.md
│   ├── stats.md
│   ├── prime.md
│   ├── comment.md
│   ├── setup.md
│   ├── update.md
│   ├── reopen.md
│   └── doctor.md
└── skills/
    └── unblock-workflow.md
```

### 11.2 Plugin Metadata

```json
{
  "name": "unblock",
  "version": "1.0.0",
  "description": "Dependency-aware task tracking for AI agents, powered by GitHub",
  "author": { "name": "websublime", "url": "https://github.com/websublime" },
  "homepage": "https://github.com/websublime/unblock",
  "license": "MIT",
  "keywords": ["task-tracking", "dependencies", "github", "agents"],
  "category": "productivity"
}
```

### 11.3 MCP Server Config

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

Auto-detect handles repo scoping. One plugin install works across all repos.

**GitHub Enterprise Server example:**

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

For GHE Cloud with dedicated subdomain, use `"GITHUB_API_URL": "https://api.ghe-corp.github.com"`.

### 11.4 Slash Command Example

```markdown
<!-- commands/ready.md -->
---
name: ready
description: Show issues ready to work on (no active blockers)
---

Find and display unblocked issues using the `ready` MCP tool.
Show results ordered by priority (P0 first).
If no issues are ready, say so clearly.
```

### 11.5 Workflow Skill

```markdown
<!-- skills/unblock-workflow.md -->
---
name: unblock-workflow
description: Workflow instructions for Unblock task tracking
---

## Task Tracking — Unblock

Backend: GitHub Issues + Projects V2

### Session Flow

1. Context auto-injected at session start via `prime`
2. Use `ready` to find unblocked work — NEVER work on blocked issues
3. `claim #id` before starting — NEVER work unclaimed
4. During work: `create --blocked_by #current` for discovered bugs/tasks
5. If A needs B first: `depends #A #B`
6. Leave notes: `comment #id "Found edge case in..."`
7. When done: `close #id "completed"`
8. Include in commits: `git commit -m "Implement auth (#42)"`

### Rules

- ALWAYS `claim` before working
- ALWAYS check `ready` first
- ALWAYS track discoveries with `--blocked_by`
- NEVER work on blocked issues
- NEVER skip the dependency graph
```

---

## 12. Configuration System

### 12.1 Loading

```rust
// crates/unblock-core/src/config.rs

pub struct Config {
    pub token: String,              // GITHUB_TOKEN (required)
    pub api_base_url: String,       // GITHUB_API_URL (default: "https://api.github.com")
    pub repo: Option<String>,       // UNBLOCK_REPO (auto-detect from git remote)
    pub project_number: Option<u64>,// UNBLOCK_PROJECT (auto-detect from linked projects)
    pub agent: String,              // UNBLOCK_AGENT (default: "agent")
    pub cache_ttl: u64,             // UNBLOCK_CACHE_TTL (default: 30)
    pub log_level: String,          // UNBLOCK_LOG_LEVEL (default: "info")
    pub otel_endpoint: Option<String>, // UNBLOCK_OTEL_ENDPOINT
}

impl Config {
    /// Convenience wrapper — reads from `std::env::var`.
    pub fn load() -> Result<Self, DomainError> {
        Self::load_from(|key| std::env::var(key))
    }

    /// Accepts a custom env reader so tests can supply a HashMap-backed
    /// closure instead of mutating process-global state (`std::env::set_var`
    /// is `unsafe` in edition 2024).
    pub fn load_from(
        env: impl Fn(&str) -> Result<String, VarError>,
    ) -> Result<Self, DomainError> {
        // ...
        let api_base_url = env("GITHUB_API_URL")
            .unwrap_or_else(|_| "https://api.github.com".to_string())
            .trim_end_matches('/')
            .to_string();
        // ...
    }
}
```

No config file. Environment variables only. The GitHub version is simpler — no TOML parsing, no file search, no merge logic. The `load_from` pattern avoids `unsafe` in edition 2024 and eliminates flaky tests from concurrent env-var mutation.

**GitHub Enterprise compatibility:** `GITHUB_API_URL` follows the convention used by `gh` CLI (`GH_HOST`) and GitHub Actions (`GITHUB_API_URL`). For GHE Server the value is `https://<host>/api/v3`; for GHE Cloud with data residency it is `https://api.<host>`. The trailing-slash normalisation avoids double-slash bugs in URL construction.

**URL resolution by environment:**

| Environment | `GITHUB_API_URL` | REST example | GraphQL endpoint |
|---|---|---|---|
| github.com (default) | `https://api.github.com` | `{base}/repos/o/r/issues` | `{base}/graphql` |
| GHE Server | `https://<host>/api/v3` | `{base}/repos/o/r/issues` | `{base}/../graphql`¹ |
| GHE Cloud (dedicated) | `https://api.<host>` | `{base}/repos/o/r/issues` | `{base}/graphql` |

> ¹ GHE Server GraphQL lives at `https://<host>/api/graphql`, not under `/api/v3`. The `GitHubClient::graphql_url()` method handles this: if `api_base_url` ends with `/v3`, it strips the suffix before appending `/graphql`.

---

## 13. Error Architecture

### 13.1 Domain Errors (unblock-core)

```rust
// crates/unblock-core/src/errors.rs

use snafu::prelude::*;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum DomainError {
    #[snafu(display("Issue not found: #{number}"))]
    IssueNotFound { number: u64 },

    #[snafu(display("Issue #{number} is already claimed by {agent}"))]
    AlreadyClaimed { number: u64, agent: String },

    #[snafu(display("Issue #{number} is blocked by: {blockers:?}"))]
    IssueBlocked { number: u64, blockers: Vec<u64> },

    #[snafu(display("Issue #{number} is deferred until {until}"))]
    IssueDeferred { number: u64, until: String },

    #[snafu(display("Issue #{number} is already closed"))]
    IssueClosed { number: u64 },

    #[snafu(display("Issue #{number} is not closed — cannot reopen"))]
    IssueNotClosed { number: u64 },

    #[snafu(display("Issue #{number} is already open"))]
    IssueAlreadyOpen { number: u64 },

    #[snafu(display("Circular dependency: adding #{source} → #{target} creates cycle"))]
    CircularDependency { source: u64, target: u64 },

    #[snafu(display("Blocking relationship already exists: #{source} → #{target}"))]
    DuplicateDependency { source: u64, target: u64 },

    #[snafu(display("Field not found: {name}"))]
    FieldNotFound { name: String },

    #[snafu(display("Validation: {message}"))]
    Validation { message: String },
}

impl DomainError {
    #[must_use]
    pub fn status_code(&self) -> u16 { /* ... */ }
}
```

**Error code mapping:**

| Error | Code | Trigger |
|---|---|---|
| `IssueNotFound` | 404 | Any tool with invalid issue number |
| `AlreadyClaimed` | 409 | `claim` on in-progress issue |
| `IssueBlocked` | 409 | `claim` on blocked issue |
| `IssueDeferred` | 409 | `claim` on deferred issue |
| `IssueClosed` | 409 | `close`, `claim`, `update` on closed issue |
| `IssueNotClosed` | 409 | `reopen` on an issue that is not closed |
| `IssueAlreadyOpen` | 409 | `reopen` on an already-open issue |
| `CircularDependency` | 422 | `depends` that would create cycle |
| `DuplicateDependency` | 409 | `depends` on existing edge |
| `FieldNotFound` | 404 | Boot validation or `update` referencing missing field |
| `Validation` | 400 | Any input validation failure |

### 13.2 Infrastructure Errors (unblock-mcp)

```rust
// crates/unblock-mcp/src/errors.rs

use snafu::prelude::*;
use unblock_core::errors::DomainError;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("{source}"))]
    #[snafu(context(false))]
    Domain { source: DomainError },

    #[snafu(display("GitHub API error: {message}"))]
    GitHubApi { message: String },

    #[snafu(display("GitHub GraphQL error: {errors}"))]
    GitHubGraphQL { errors: String },

    #[snafu(display("Cannot connect to GitHub"))]
    GitHubUnavailable { source: reqwest::Error },

    #[snafu(display("GitHub rate limit exceeded"))]
    RateLimited,

    #[snafu(display("Circuit breaker open — GitHub consistently failing"))]
    CircuitBreakerOpen,

    #[snafu(display("Projects V2 not configured — run `setup` first"))]
    ProjectNotConfigured,

    #[snafu(display("Failed to detect git remote"))]
    GitRemote { source: std::io::Error },
}

/// Convert to MCP protocol error.
impl From<Error> for McpError { /* ... */ }
```

---

## 14. Observability

### 14.1 Logging

All via `tracing` crate. JSON to stderr (stdio is MCP protocol).

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

### 14.2 Log Levels

| Level | Content |
|---|---|
| `error` | Failed operations, GitHub errors, circuit breaker trips |
| `warn` | Stale cache served, skipped blocking edges with unknown issues |
| `info` | Tool invocations, result summaries, setup progress, repo/project detection |
| `debug` | GitHub request/response details (token redacted), graph computation |
| `trace` | MCP protocol messages, cache operations |

### 14.3 Metrics (OpenTelemetry, optional)

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

## 15. Resilience Patterns

### 15.1 Circuit Breaker

```rust
pub struct CircuitBreaker {
    inner: Mutex<CircuitBreakerInner>,
}

struct CircuitBreakerInner {
    state: CircuitState,
    failure_count: usize,
    failure_threshold: usize,     // 5
    cooldown: Duration,           // 10s
    last_state_change: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitState { Closed, Open, HalfOpen }

impl CircuitBreaker {
    /// Check if requests are allowed. Returns error if circuit is open.
    pub fn check(&self) -> Result<(), Error> { /* ... */ }

    /// Record a successful request (resets failure count, closes circuit).
    pub fn record_success(&self) { /* ... */ }

    /// Record a failed request (increments count, opens circuit at threshold).
    pub fn record_failure(&self) { /* ... */ }
}
```

### 15.2 Retry

```rust
pub struct RetryPolicy {
    pub max_retries: usize,       // 3
    pub base_delay: Duration,     // 500ms
    pub max_delay: Duration,      // 5s
}

impl RetryPolicy {
    /// Execute an async operation with exponential backoff + jitter.
    /// Only retries on RateLimited and GitHubUnavailable errors.
    pub async fn execute<F, Fut, T>(&self, mut op: F) -> Result<T, Error>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, Error>>,
    { /* ... */ }
}
```

---

## 16. Security

### 16.1 Token Handling

- `GITHUB_TOKEN` loaded from environment variable only
- Never logged (redacted in debug output)
- Never included in MCP tool responses
- Never embedded in binary
- Plugin `.mcp.json` uses `${GITHUB_TOKEN}` expansion

### 16.2 Input Validation

- Issue numbers: positive integers
- Titles: non-empty, max 500 chars
- Agent names: non-empty, max 100 chars
- Priority: must be P0-P4
- Dates: valid ISO format

### 16.3 Transport

stdio is process-local. No network exposure. Only the spawning process (Claude Code) can communicate.

---

## 17. Build and Distribution

### 17.1 Workspace

```toml
[workspace]
members = [
    "crates/unblock-core",
    "crates/unblock-github",
    "crates/unblock-mcp",
    "crates/unblock-app",
]
resolver = "2"

[workspace.package]
edition = "2024"
license = "MIT"
repository = "https://github.com/websublime/unblock"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
reqwest = { version = "0.12", features = ["json"] }
petgraph = "0.7"
chrono = { version = "0.4", features = ["serde"] }
snafu = "0.8"
anyhow = "1"
rmcp = { version = "1.0", features = ["server", "transport-io"] }
schemars = "1"
rand = "0.9"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
missing_errors_doc = "allow"

[workspace.lints.rust]
unsafe_code = "deny"
missing_docs = "warn"
```

### 17.2 Release Targets

| Platform | Target |
|---|---|
| Linux x86_64 | `x86_64-unknown-linux-musl` |
| Linux ARM64 | `aarch64-unknown-linux-musl` |
| macOS x86_64 | `x86_64-apple-darwin` |
| macOS ARM64 | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

### 17.3 CI/CD

CI, release workflows, distribution channels, Homebrew tap, npm wrapper, and signing are defined in the standalone CI/CD architecture document:

**→ See `unblock-cicd-architecture.md`** for:
- CI workflow (`ci.yml`): fmt, clippy, test, coverage — split by product
- MCP release via `cargo-dist` (auto-generated `release.yml`, tag `unblock-mcp-v*`)
- Desktop release via custom workflow (`desktop-release.yml`, tag `unblock-app-v*`)
- Versioning strategy with `cargo-release` (independent versions per binary)
- Distribution: GitHub Releases, Homebrew (formula + cask), npm, shell/PowerShell installers
- Secrets and signing (Apple certificates, notarisation, npm token)

---

## 18. Testing Architecture

### 18.1 Strategy

| Layer | Type | What | GitHub Required? |
|---|---|---|---|
| `unblock-core` | Unit | Graph engine, cache, types | No |
| `unblock-core` | Property | Graph invariants (proptest) | No |
| `unblock-mcp` | Unit | Body section parsing, error conversion | No |
| `unblock-mcp` | Integration | Full tool flows against real repo | Yes |

### 18.2 Quality Gate

Coverage target: **>80% for Phase 1-2, 100% from Phase 3 onwards.** Phase 1-2 focuses on core graph engine and API integration where rapid iteration is expected. Phase 3+ enforces full coverage as the architecture stabilizes.

### 18.3 Unit Test Examples

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_issues() -> Vec<Issue> { /* ... */ }
    fn sample_edges() -> Vec<BlockingEdge> { /* ... */ }

    #[test]
    fn test_ready_set_excludes_blocked() { /* ... */ }

    #[test]
    fn test_unblock_cascade() { /* ... */ }

    #[test]
    fn test_cycle_detection() { /* ... */ }

    #[test]
    fn test_body_sections_roundtrip() { /* ... */ }
}
```

### 18.4 Integration Tests

```rust
#[tokio::test]
#[ignore] // Requires GITHUB_TOKEN and a test repo
async fn test_full_workflow() { /* ... */ }
```

### 18.5 Property Tests

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn ready_set_never_contains_blocked_issues(
        issues in vec(arb_issue(), 1..100),
        edges in vec(arb_edge(), 0..200),
    ) { /* ... */ }
}
```
