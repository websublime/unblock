# Unblock — Extensions Plan

**Three extensions to the MCP server: project bootstrapping, smart views, and cross-repo dependencies.**

| | |
|---|---|
| **Version** | 0.3.0 |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Date** | March 2026 |
| **Status** | Draft (validated against live API) |
| **Depends on** | `unblock-prd-github.md`, `unblock-architecture-github.md` |
| **Research** | `github-projectsv2-views-api-findings.md`, `github-rest-projectsv2-views.json` |

---

## 1. Overview

Three extensions to the existing MCP server:

| Extension | Tool | Purpose |
|---|---|---|
| **A. Project Bootstrap** | `://init` (NEW) | Create org-level or user-level Projects V2 project if it doesn't exist |
| **B. Smart Views** | `://setup` (extended) | Add 5 pre-configured board/table/roadmap views after field creation |
| **C. Cross-Repo Dependencies** | `://depends` (extended) | Support `owner/repo#number` references for cross-repo blocking relationships |

No additional infrastructure. No hosted services. Built on the GitHub REST API (`v2026-03-10`) and existing GraphQL mutations.

---

## 2. Extension A — `://init` (Project Bootstrap)

### 2.1 Purpose

Create a GitHub Projects V2 project if it doesn't exist. One-time bootstrapping command that sets up the project container before `://setup` configures fields and views.

### 2.2 Scope Detection

`://init` receives the scope as a parameter — no persistent config needed:

```
://init --scope=org     → creates project at org level (default)
://init --scope=user    → creates project at user level
```

The scope determines:

| | `org` (default) | `user` |
|---|---|---|
| API path | `POST /orgs/{org}/projects` | `POST /users/{user_id}/projects` (GraphQL) |
| Issues from | Any repo in the org | Any repo the user has access to |
| Visibility | Org members (configurable) | Owner only (configurable) |
| Best for | Teams, multi-repo | Solo dev, single repo |

### 2.3 Parameters

| Parameter | Required | Default | Description |
|---|---|---|---|
| `--scope` | No | `org` | `org` or `user` — where the project lives |
| `--title` | No | `"Unblock — {repo}"` | Project title |
| `--description` | No | `"Dependency-aware task tracking powered by ://unblock"` | Project description |
| `--public` | No | `false` | Whether the project is publicly visible |

### 2.4 Flow

```
://init flow:
  1. Detect owner type (org vs user) from repo owner    ← GET /orgs/{owner} (200 = org, 404 = user)
  2. Check if project already exists                     ← GraphQL: org/user → projectsV2
  3. If exists → report project URL + number, done
  4. If not → create project                             ← GraphQL: createProjectV2
  5. Link project to repo                                ← addProjectV2ItemById or repo settings
  6. Output: project number + URL
  7. Hint: "Run ://setup to configure fields and views"
```

### 2.5 Idempotency

If a project with the same title already exists on the org/user, skip creation and return the existing project's number and URL. The user can then run `://setup` to configure it.

### 2.6 Auto-detection after init

After `://init` creates a project, `://setup` auto-detects it. No need for `UNBLOCK_PROJECT` env var — the setup tool finds the project linked to the repo. If multiple projects exist, `UNBLOCK_PROJECT` disambiguates.

### 2.7 GraphQL: Create Project

```graphql
mutation CreateProject($ownerId: ID!, $title: String!) {
  createProjectV2(input: {
    ownerId: $ownerId
    title: $title
  }) {
    projectV2 {
      id
      number
      url
    }
  }
}
```

The `ownerId` is the org or user node ID, resolved from the owner name.

---

## 3. Extension B — Smart Views

### 3.1 API Constraints (validated 2026-03-27)

The REST API (`v2026-03-10`) supports creating views with **name**, **layout**, **filter**, and **visible_fields**. The following are **read-only** and cannot be set at creation:

- **Group by** — defaults to Status for board layouts
- **Sort by** — defaults to manual ordering
- **Vertical group by (swimlanes)** — not configurable via API

Users fine-tune these in the GitHub UI. The setup report includes guidance on recommended adjustments.

### 3.2 Views

#### 3.2.1 `://ready` board

The agent's `://ready` command as a visual board.

| Setting | Value | API Support |
|---|---|---|
| Layout | Board | YES |
| Filter | `"Ready State":"ready"` | YES |
| Fields shown | Title, Priority, Story Points, Agent, Labels | YES (visible_fields) |
| Group by | Priority | Manual — user sets in UI after creation |
| Sort | Created date ASC | Manual — user sets in UI after creation |

#### 3.2.2 `://team` board

Who is working on what.

| Setting | Value | API Support |
|---|---|---|
| Layout | Board | YES |
| Filter | `is:open` | YES |
| Fields shown | Title, Priority, Agent, Story Points, Claimed At | YES (visible_fields) |
| Group by | Status | Manual — defaults to Status on board, likely correct |
| Swimlanes | Agent field | Manual — user sets in UI after creation |

#### 3.2.3 `://pipeline` board

Classic kanban — the full workflow.

| Setting | Value | API Support |
|---|---|---|
| Layout | Board | YES |
| Filter | `is:open` | YES |
| Fields shown | Title, Priority, Agent, Labels, Ready State | YES (visible_fields) |
| Group by | Status | Manual — defaults to Status on board, likely correct |

#### 3.2.4 `://roadmap` table

Epic-level progress grouped by milestone.

| Setting | Value | API Support |
|---|---|---|
| Layout | Table | YES |
| Filter | (none) | YES |
| Fields shown | Title, Type, Priority, Status, Agent, Story Points, Defer Until | YES (visible_fields) |
| Group by | Milestone | Manual — user sets in UI after creation |

#### 3.2.5 `://timeline` roadmap

Date-based timeline — visual sprint and milestone planning.

| Setting | Value | API Support |
|---|---|---|
| Layout | Roadmap | YES |
| Filter | (none) | YES |
| Fields shown | (not applicable for roadmap layout) | N/A |

### 3.3 Implementation

#### Hybrid API Strategy

1. **GraphQL** — query existing views (idempotency check)
2. **REST API** — create new views (only write path available)
3. **REST API** — list project fields (discover integer IDs for `visible_fields`)

#### REST: Create View

```
POST /orgs/{org}/projectsV2/{project_number}/views
POST /users/{user_id}/projectsV2/{project_number}/views

{
  "name": "://ready",
  "layout": "board",
  "filter": "\"Ready State\":\"ready\"",
  "visible_fields": [101, 102, 103, 104, 105]
}
```

#### REST: List Fields (for ID discovery)

```
GET /orgs/{org}/projectsV2/{project_number}/fields
GET /users/{username}/projectsV2/{project_number}/fields
```

Returns all fields (built-in + custom) with integer `id` values. Built-in fields (Title, Labels, Milestone) included alongside custom fields.

**Note:** `options[].name` returns `{"raw": "Todo", "html": "Todo"}` (nested object), not a plain string. Parse `.raw`.

#### GraphQL: Query Existing Views (idempotency)

For org-owned projects:
```graphql
query {
  organization(login: $org) {
    projectV2(number: $projectNumber) {
      views(first: 30) {
        nodes { name number layout }
      }
    }
  }
}
```

For user-owned projects:
```graphql
query {
  user(login: $user) {
    projectV2(number: $projectNumber) {
      views(first: 30) {
        nodes { name number layout }
      }
    }
  }
}
```

> **Note:** `viewer { projectV2(number: N) }` does NOT work for org-owned projects.

#### Integration with `://setup`

```
://setup flow (revised):
  1. Resolve project (GraphQL)                     ← existing
  2. Create/verify 7 custom fields (GraphQL)       ← existing
  3. Detect owner type (org vs user)               ← NEW
  4. Query existing views (GraphQL)                ← NEW — idempotency check
  5. Discover field IDs (REST GET /fields)         ← NEW — integer IDs for visible_fields
  6. Create missing views (REST POST /views)       ← NEW — up to 5 views
  7. Report results + manual config guidance       ← NEW
```

#### Setup Report Output

```
Views created:
  ✓ ://ready (board) — https://github.com/org/projects/1/views/2
  ✓ ://team (board) — https://github.com/org/projects/1/views/3
  ✓ ://pipeline (board) — https://github.com/org/projects/1/views/4
  ✓ ://roadmap (table) — https://github.com/org/projects/1/views/5
  ✓ ://timeline (roadmap) — https://github.com/org/projects/1/views/6

Recommended manual adjustments:
  ://ready    → Group by: Priority, Sort by: Created date ASC
  ://team     → Sort by: Priority ASC, Swimlanes: Agent field
  ://pipeline → Sort by: Priority ASC
  ://roadmap  → Group by: Milestone, Sort by: Priority ASC
```

#### API Version Requirement

```
X-GitHub-Api-Version: 2026-03-10
```

Current client uses `2022-11-28`. REST view/field endpoints require `2026-03-10`. Add version selectively per-request or upgrade globally.

---

## 4. Extension C — Cross-Repo Dependencies

### 4.1 Purpose

Enable dependency relationships between issues in different repositories. This is essential for org-level projects that aggregate issues from multiple repos.

### 4.2 Current Limitation

The current `add_blocked_by()` and `remove_blocked_by()` methods resolve issues by number from the **configured repo only**:

```rust
// Current: only resolves from self.owner()/self.repo()
let blocker_id = self.resolve_node_id(blocked_by_number).await?;

// resolve_node_id uses:
query ResolveNodeId($owner: String!, $repo: String!, $number: Int!) {
    repository(owner: $owner, name: $repo) {
        issue(number: $number) { id }
    }
}
```

This means `://depends 5 blocked-by 10` only works if both #5 and #10 are in the same repo.

### 4.3 Solution: Cross-Repo Issue References

Introduce a reference format that supports both same-repo and cross-repo issues:

| Format | Meaning | Example |
|---|---|---|
| `#123` or `123` | Issue #123 in the current repo | `://depends 5 blocked-by 123` |
| `owner/repo#123` | Issue #123 in a different repo | `://depends 5 blocked-by websublime/api#42` |

### 4.4 Implementation

#### 4.4.1 New type: `IssueRef`

```rust
/// A reference to an issue, either in the current repo or a cross-repo reference.
pub enum IssueRef {
    /// Issue in the current repo (just a number).
    Local(u64),
    /// Issue in a different repo (owner, repo, number).
    CrossRepo {
        owner: String,
        repo: String,
        number: u64,
    },
}

impl IssueRef {
    /// Parse from string: "123", "#123", or "owner/repo#123"
    pub fn parse(s: &str) -> Result<Self, DomainError> { ... }
}
```

#### 4.4.2 Updated `resolve_node_id`

```rust
/// Resolves an IssueRef to a GraphQL node ID.
/// For Local refs, uses self.owner()/self.repo().
/// For CrossRepo refs, uses the specified owner/repo.
pub async fn resolve_issue_ref(&self, issue_ref: &IssueRef) -> Result<String, Error> {
    let (owner, repo, number) = match issue_ref {
        IssueRef::Local(n) => (self.owner().to_owned(), self.repo().to_owned(), *n),
        IssueRef::CrossRepo { owner, repo, number } => {
            (owner.clone(), repo.clone(), *number)
        }
    };
    // Same GraphQL query, different owner/repo
    ...
}
```

#### 4.4.3 Updated `add_blocked_by` / `remove_blocked_by`

Signatures change from:

```rust
pub async fn add_blocked_by(&self, issue_number: u64, blocked_by_number: u64) -> Result<(), Error>
```

To:

```rust
pub async fn add_blocked_by(&self, issue: &IssueRef, blocked_by: &IssueRef) -> Result<(), Error>
```

#### 4.4.4 Updated MCP tool: `://depends`

The `://depends` tool handler parses issue references:

```
://depends 5 blocked-by websublime/api#42
://depends websublime/frontend#10 blocked-by 5
://depends websublime/frontend#10 blocked-by websublime/api#42
```

All three forms work. The GitHub `addIssueDependency` mutation accepts any two Issue node IDs — cross-repo dependencies are native to GitHub.

### 4.5 Graph Engine Impact

The `unblock-core` graph engine currently uses issue numbers as node identifiers. With cross-repo dependencies, we need a unique identifier that includes the repo:

| Approach | Format | Pros | Cons |
|---|---|---|---|
| **A. Qualified ID** | `owner/repo#123` | Unambiguous, human-readable | Longer keys, string comparisons |
| **B. Node ID** | `I_kwDOxx...` | GitHub-native, globally unique | Opaque, not human-readable |
| **C. Numeric with repo index** | `(repo_idx, number)` | Fast comparison | Requires repo registry |

**Recommendation: A (Qualified ID)** — `owner/repo#123` for cross-repo, `#123` displayed for local repo. The graph engine uses the qualified form internally, display layer shortens local refs.

### 4.6 Fetch Issues from Multiple Repos

For the `://ready` command to work across repos in an org project, we need to fetch issues from all repos linked to the project — not just the configured repo.

#### Current flow:
```
1. Fetch issues from self.owner()/self.repo()
2. Build dependency graph
3. Return ready issues
```

#### Updated flow:
```
1. Query project items (all repos)               ← via ProjectV2.items
2. For each item, resolve repo + issue number
3. Build dependency graph with qualified IDs
4. Return ready issues (with repo context)
```

The GraphQL `ProjectV2.items` connection returns items from all linked repos — this is the natural entry point for multi-repo support.

---

## 5. Design Decisions

| # | Decision | Rationale |
|---|---|---|
| EX1 | `://init` is separate from `://setup` | Bootstrapping (create project) is a one-time decision with different parameters than configuration (fields/views). Separation prevents long-term coupling |
| EX2 | `://init` auto-detects org vs user | `GET /orgs/{owner}` check — no config needed. Scope parameter overrides if specified |
| EX3 | Views created by `://setup`, not `://init` | Init creates the container, setup configures it. Views depend on fields being created first |
| EX4 | Idempotent — skip if exists | Both init and setup are safe to re-run. Never overwrite manual changes |
| EX5 | `://` prefix in view names | Brand consistency. Views identifiable as Unblock-created |
| EX6 | 5 views cover 5 personas | Ready (agent), Team (tech lead), Pipeline (everyone), Roadmap (planning), Timeline (visual) |
| EX7 | Hybrid REST + GraphQL | GraphQL has no view mutations; REST has no view listing. Use each API's strengths |
| EX8 | Report manual config steps | Group-by, sort, swimlanes can't be set via API. Transparent guidance |
| EX9 | API version `2026-03-10` | Views + fields REST endpoints require this version |
| EX10 | Cross-repo refs use `owner/repo#number` | Human-readable, unambiguous, parseable. Local refs stay as plain numbers |
| EX11 | `IssueRef` type for polymorphic references | Clean abstraction — local and cross-repo share the same interface |
| EX12 | Graph engine uses qualified IDs internally | `owner/repo#123` prevents collision between issue #5 in repo A and #5 in repo B |
| EX13 | No `UNBLOCK_PROJECT_SCOPE` config | Init detects scope, setup detects project owner type. Zero config overhead |

---

## 6. Risks

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| R1 | Group by / Sort / Swimlanes not API-configurable | Medium | Report manual config steps. Board views default to Status grouping |
| R2 | ~~Filter syntax underdocumented~~ | RESOLVED | Both `"Field":"Value"` and `Field:Value` work. Validate field names client-side |
| R3 | No view update/delete endpoints | Low | Create-only. User manages in GitHub UI. Idempotency prevents duplicates |
| R4 | API version `2026-03-10` may evolve | Low | Pin version header. Monitor changelog |
| R5 | Field ID integers differ per project | None | Discovered per-project via GET /fields |
| R6 | Org vs user routing | Low | Auto-detect from owner type |
| R7 | `options[].name` is `{raw, html}` not string | Low | Parse `name.raw` — differs from OpenAPI spec |
| R8 | Closed projects reject view creation | Low | Check project state before attempting |
| R9 | Filters not validated server-side | Medium | API accepts any string. Validate client-side |
| R10 | Cross-repo `resolve_node_id` requires read access to target repo | Medium | Token must have `repo` scope for all referenced repos. Report clear error if access denied |
| R11 | Graph engine refactor for qualified IDs | Medium | Breaking change to node identifiers. Must update all graph operations, cache keys, and display formatting |
| R12 | Multi-repo fetch increases API calls | Low | ProjectV2.items returns all repos in one query. Pagination may be needed for large projects |
| R13 | `BREAKING CHANGE` to `add_blocked_by` signature | Medium | `u64` → `IssueRef` is a breaking API change. Coordinate with MCP tool handlers |

---

## 7. Implementation Order

```
Phase 1: ://init (project bootstrap)
  └── Create project if not exists
  └── Auto-detect org vs user
  └── Output project number + URL

Phase 2: ://setup views (smart views)
  └── Depends on: Phase 1 (project must exist)
  └── Query existing views (GraphQL)
  └── Discover field IDs (REST)
  └── Create 5 views (REST)
  └── Report + manual config guidance

Phase 3: Cross-repo dependencies
  └── IssueRef type + parser
  └── Updated resolve_node_id
  └── Updated add_blocked_by / remove_blocked_by
  └── Updated ://depends tool handler
  └── Graph engine qualified IDs
  └── Multi-repo fetch for ://ready
```

### Effort Estimates

| Phase | Estimate | Notes |
|---|---|---|
| Phase 1 | 1 day | GraphQL createProjectV2 + auto-detect + idempotency |
| Phase 2 | 2 days | Hybrid REST/GraphQL + field ID discovery + 5 views + report |
| Phase 3 | 3 days | IssueRef type, graph engine refactor, multi-repo fetch, tool handler updates |
| **Total** | **6 days** | |
