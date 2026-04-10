# Spec 02 — GitHub API Client

> Companion: [SPEC §5](../SPEC.md#5-github-api-layer) · [SPEC §14](../SPEC.md#14-resilience) · Plans: [01-mcp-foundation](../plans/01-plan-mcp-foundation.md) · [02-mcp-complete](../plans/02-plan-mcp-complete.md) · [03-mcp-production](../plans/03-plan-mcp-production.md)  
> Crate: `unblock-github`  
> Status: draft  
> Last updated: 2026-04-10

---

## Table of Contents

1. [Scope](#1-scope)
2. [Types](#2-types)
3. [URL Resolution Algorithm](#3-url-resolution-algorithm)
4. [Authentication Algorithm](#4-authentication-algorithm)
5. [GraphQL Read Queries](#5-graphql-read-queries)
6. [Mutations](#6-mutations)
7. [Projects V2 Field Management](#7-projects-v2-field-management)
8. [View Management](#8-view-management)
9. [Circuit Breaker Algorithm](#9-circuit-breaker-algorithm)
10. [Retry Algorithm](#10-retry-algorithm)
11. [Pagination Algorithm](#11-pagination-algorithm)
12. [Error Catalogue](#12-error-catalogue)
13. [Invariants](#13-invariants)
14. [Open Questions](#14-open-questions)

---

## 1. Scope

This spec defines the **algorithms, API contracts, and edge cases** for the GitHub API client (`unblock-github`).

**In scope:** URL resolution for github.com / GHE Server / GHE Cloud, authentication (PAT and GitHub App), GraphQL read queries, REST mutations, Projects V2 field and view management, circuit breaker state machine, retry with exponential backoff, pagination.

**Out of scope:** graph computation (→ [01-spec-graph-engine.md](./01-spec-graph-engine.md)), MCP tool handler logic (→ [03-spec-mcp-tools.md](./03-spec-mcp-tools.md)).

---

## 2. Types

### 2.1 `GitHubClient`

```rust
pub struct GitHubClient {
    http: reqwest::Client,
    auth: Arc<dyn GitHubAuth>,
    api_base_url: String,
    github_url: String,
    owner: String,
    repo: String,
    project_number: Option<u64>,
    project_id: Option<String>,
    field_ids: Option<ProjectFieldIds>,
    circuit_breaker: CircuitBreaker,
    retry_policy: RetryPolicy,
}
```

### 2.2 `ProjectFieldIds`

```rust
pub struct ProjectFieldIds {
    pub status: FieldMeta,
    pub priority: FieldMeta,
    pub agent: String,          // text field — no options
    pub claimed_at: String,     // date field
    pub ready_state: FieldMeta,
    pub story_points: String,   // number field
    pub defer_until: String,    // date field
}

pub struct FieldMeta {
    pub field_id: String,
    pub options: HashMap<String, String>,  // "open" → "option_node_id"
}
```

### 2.3 `FieldValue`

```rust
pub enum FieldValue {
    SingleSelect(String),  // option node ID
    Text(String),
    Date(String),          // ISO 8601
    Number(f64),
}
```

### 2.4 `GitHubAuth` trait

```rust
#[async_trait]
pub trait GitHubAuth: Send + Sync + std::fmt::Debug {
    async fn token(&self) -> Result<String, Error>;
}
```

---

## 3. URL Resolution Algorithm

### 3.1 REST URL

```
rest_url(api_base_url, path) → String:
  RETURN "{api_base_url}/{path}"

  // Examples:
  // github.com:   "https://api.github.com/repos/o/r/issues"
  // GHE Server:   "https://ghe.corp.com/api/v3/repos/o/r/issues"
  // GHE Cloud:    "https://api.corp.github.com/repos/o/r/issues"
```

### 3.2 GraphQL URL

```
graphql_url(api_base_url) → String:

  IF api_base_url ends with "/v3":
    // GHE Server: GraphQL lives at /api/graphql, not /api/v3/graphql
    base = api_base_url.strip_suffix("/v3")
    RETURN "{base}/graphql"
  ELSE:
    RETURN "{api_base_url}/graphql"

  // Examples:
  // github.com:   "https://api.github.com"        → "https://api.github.com/graphql"
  // GHE Server:   "https://ghe.corp.com/api/v3"   → "https://ghe.corp.com/api/graphql"
  // GHE Cloud:    "https://api.corp.github.com"    → "https://api.corp.github.com/graphql"
```

### 3.3 HTML URL

```
html_url(github_url, path) → String:
  RETURN "{github_url}/{path}"

  // Used for issue links in comments and audit trails.
  // github.com:   "https://github.com/owner/repo/issues/42"
  // GHE Server:   "https://ghe.corp.com/owner/repo/issues/42"
```

### 3.4 Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `GITHUB_API_URL` | `https://api.github.com` | REST and GraphQL base |
| `GITHUB_URL` | `https://github.com` | HTML links |

Both stored without trailing slash (normalised at load time).

### 3.5 Edge cases

- **Trailing slash:** `https://api.github.com/` → normalised to `https://api.github.com`. Prevents double-slash in URLs.
- **GHE Server with custom port:** `https://ghe.corp.com:8443/api/v3` → GraphQL: `https://ghe.corp.com:8443/api/graphql`. Port preserved.
- **GHE Cloud data residency:** `https://api.us.github.com` → GraphQL: `https://api.us.github.com/graphql`. No `/v3` suffix → standard path.

---

## 4. Authentication Algorithm

### 4.1 Token Auth (PAT)

```
TokenAuth::token() → Result<String>:
  RETURN self.token  // static, never expires
```

### 4.2 App Auth (GitHub App installation token)

```
AppAuth::token() → Result<String>:

  // 1. Check cached token
  IF cached_token IS SOME AND cached_token.expires_at > now + 5 minutes:
    RETURN cached_token.token

  // 2. Generate JWT
  jwt = sign_rs256({
    iss: self.app_id,
    iat: now - 60s,       // clock skew tolerance
    exp: now + 600s,      // 10 minute max
  }, self.private_key)

  // 3. Create installation token
  response = POST "{api_base_url}/app/installations/{installation_id}/access_tokens"
    Authorization: Bearer {jwt}

  token = response.token
  expires_at = response.expires_at  // ~1 hour from creation

  // 4. Cache
  self.cached_token = CachedToken { token, expires_at }

  RETURN token
```

### 4.3 Auth selection at startup

```
select_auth(config) → Result<Arc<dyn GitHubAuth>>:

  IF config.app_id AND config.app_private_key AND config.app_installation_id:
    // All 3 present → GitHub App
    RETURN AppAuth::new(app_id, private_key, installation_id, api_base_url)

  IF config.token:
    // PAT
    RETURN TokenAuth::new(token)

  RETURN Err("No authentication configured. Set GITHUB_TOKEN or GITHUB_APP_* variables")
```

### 4.4 Private key handling

```
parse_private_key(value) → String:

  IF value starts with "-----BEGIN":
    RETURN value  // inline PEM

  // Treat as file path
  RETURN read_file(value)?
```

### 4.5 Edge cases

- **Expired JWT:** JWT has 10-minute lifetime. If the installation token request takes >10 minutes (extremely unlikely), the JWT expires mid-flight. Retry with new JWT.
- **Concurrent token refresh:** Two threads call `token()` simultaneously when cache is expired. `RwLock` ensures only one creates a new token; the other reads from cache.
- **Installation token near expiry:** Refreshed when within 5 minutes of expiry. This prevents a request failing mid-flight due to token expiry.
- **Rate limits with App auth:** 15,000 requests/hour (vs 5,000 for PAT). The circuit breaker and retry thresholds remain the same — they protect against outages, not rate limits.

---

## 5. GraphQL Read Queries

### 5.1 `fetch_graph_data()`

Primary read query. Fetches everything needed for graph construction in a single paginated query.

```graphql
query($owner: String!, $repo: String!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    issues(first: 100, after: $cursor, states: [OPEN]) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number
        title
        state
        createdAt
        labels(first: 10) { nodes { name } }
        milestone { number title }
        assignees(first: 5) { nodes { login } }
        issueType { name }
        trackedByIssues(first: 50) {
          nodes { number repository { owner { login } name } state }
        }
        trackedIssues(first: 50) {
          nodes { number repository { owner { login } name } state }
        }
        parent { number }
        projectItems(first: 5) {
          nodes {
            project { number }
            fieldValues(first: 20) {
              nodes {
                ... on ProjectV2ItemFieldSingleSelectValue { field { ... on ProjectV2SingleSelectField { name } } name }
                ... on ProjectV2ItemFieldTextValue { field { ... on ProjectV2Field { name } } text }
                ... on ProjectV2ItemFieldDateValue { field { ... on ProjectV2Field { name } } date }
                ... on ProjectV2ItemFieldNumberValue { field { ... on ProjectV2Field { name } } number }
              }
            }
          }
        }
      }
    }
  }
}
```

**Pagination:** Loop while `hasNextPage == true`, advancing `$cursor`. Each page returns up to 100 issues.

**Blocking edges:** Extracted from `trackedByIssues` (what blocks this issue) and `trackedIssues` (what this issue blocks). Both traversed to build complete edge set.

**Cross-repo:** `trackedByIssues.nodes[].repository` may differ from the queried repo. `QualifiedId` is constructed from the repository context on each edge.

### 5.2 `fetch_issue(number)`

Single issue by number in configured repo. Always fresh — never cached.

```graphql
query($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    issue(number: $number) {
      ... full issue fields including comments ...
    }
  }
}
```

### 5.3 `fetch_issue_ref(ref)`

Resolves `IssueRef` (local or cross-repo). For `CrossRepo`, queries the referenced `owner/repo`.

### 5.4 `fetch_recently_closed(hours)` (Phase 02)

Fetches issues closed within a time window. Used by the reconcile tool handler to supply closed-issue data to the reconciliation engine (see spec-01 §8.4).

```graphql
query($owner: String!, $repo: String!, $cursor: String, $since: DateTime!) {
  repository(owner: $owner, name: $repo) {
    issues(first: 100, after: $cursor, states: [CLOSED], filterBy: { since: $since }) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number
        title
        state
        closedAt
        trackedByIssues(first: 50) {
          nodes { number repository { owner { login } name } state }
        }
        projectItems(first: 5) {
          nodes {
            project { number }
            fieldValues(first: 20) {
              nodes {
                ... on ProjectV2ItemFieldSingleSelectValue { field { ... on ProjectV2SingleSelectField { name } } name }
                ... on ProjectV2ItemFieldTextValue { field { ... on ProjectV2Field { name } } text }
                ... on ProjectV2ItemFieldDateValue { field { ... on ProjectV2Field { name } } date }
              }
            }
          }
        }
      }
    }
  }
}
```

**Parameters:** `$since` = `now - hours` (ISO 8601 DateTime). Default: 24 hours (matches stale claim threshold).

**Returns:** `Vec<Issue>` with Project field values and blocking edge references. Does NOT include full comments (not needed for reconciliation).

**Edge cases:**
- **Large window:** A 24h window on an active repo may return many closed issues. Paginated same as `fetch_graph_data()`.
- **No recently closed:** Returns empty vec. Reconcile passes 1 and 2 operate only on open issues.

### 5.6 `fetch_ready_from_field()` (Phase 03)

Lightweight query — fetches only issues where Ready State field == `ready`.

```graphql
query($projectId: ID!, $cursor: String) {
  node(id: $projectId) {
    ... on ProjectV2 {
      items(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          fieldValueByName(name: "Ready State") {
            ... on ProjectV2ItemFieldSingleSelectValue { name }
          }
          content {
            ... on Issue { number title state createdAt }
          }
        }
      }
    }
  }
}
```

Client-side filter: only return items where Ready State value == `"ready"` and issue state == `OPEN`.

### 5.7 Edge cases

- **Pagination limit:** GitHub limits `first` to 100. For repos with >100 open issues, multiple pages required.
- **Rate limit:** Each page is one GraphQL point. 500-issue repo = 5 pages = 5 points out of 5000/hour budget.
- **Missing project items:** Issues not added to the project have no field values. They appear in the graph but with default field values.
- **Cross-repo tokens:** If the token lacks access to a cross-repo issue, the blocking edge is returned with limited data. The issue node may be `null`. Handled as `OrphanedBlockingEdge` in reconcile.

---

## 6. Mutations

### 6.1 REST mutations

| Method | Endpoint | Purpose |
|---|---|---|
| `POST` | `/repos/{o}/{r}/issues` | Create issue |
| `PATCH` | `/repos/{o}/{r}/issues/{n}` | Update issue (title, body, state, labels, assignees, milestone) |
| `POST` | `/repos/{o}/{r}/issues/{n}/comments` | Add comment |

All REST mutations use `X-GitHub-Api-Version: 2022-11-28` unless targeting Projects V2 view/field endpoints.

### 6.2 GraphQL mutations

```graphql
# Add blocking relationship
mutation($issueId: ID!, $blockerId: ID!) {
  addIssueDependency(input: { issueId: $issueId, dependsOnIssueId: $blockerId }) {
    clientMutationId
  }
}

# Remove blocking relationship
mutation($issueId: ID!, $blockerId: ID!) {
  removeIssueDependency(input: { issueId: $issueId, dependsOnIssueId: $blockerId }) {
    clientMutationId
  }
}

# Add sub-issue
mutation($parentId: ID!, $childId: ID!) {
  addSubIssue(input: { issueId: $parentId, subIssueId: $childId }) {
    clientMutationId
  }
}

# Update Projects V2 field
mutation($projectId: ID!, $itemId: ID!, $fieldId: ID!, $value: ProjectV2FieldValue!) {
  updateProjectV2ItemFieldValue(input: {
    projectId: $projectId, itemId: $itemId, fieldId: $fieldId, value: $value
  }) {
    projectV2Item { id }
  }
}
```

### 6.3 Cross-repo scope

| Operation | Cross-repo | Rationale |
|---|---|---|
| `depends` / `dep_remove` | ✅ | Dependencies are the core cross-repo use case |
| `show` / `fetch_issue_ref` | ✅ | Inspect cross-repo blockers |
| `close`, `reopen`, `update`, `claim`, `comment` | ❌ | Scoped to configured repo for safety |
| `create` (`blocked_by` param) | ✅ | Cross-repo deps at creation time |

### 6.4 Batch mutations

```
batch_update_fields(item_id, updates) → Result<()>:

  // Single GraphQL request with multiple updateProjectV2ItemFieldValue mutations
  // Using GraphQL aliases: update0, update1, update2, ...

  mutation_parts = []
  FOR (i, (field_id, value)) in updates.enumerate():
    mutation_parts.push("
      update{i}: updateProjectV2ItemFieldValue(input: {
        projectId: $projectId, itemId: $itemId,
        fieldId: \"{field_id}\", value: {value}
      }) { projectV2Item { id } }
    ")

  query = "mutation($projectId: ID!, $itemId: ID!) { {mutation_parts.join()} }"
  EXECUTE query
```

### 6.5 Edge cases

- **Concurrent field updates:** Two agents updating the same issue's fields. GitHub accepts both — last write wins. No optimistic locking in v1.
- **Issue not in project:** Attempting to update a field on an issue not added to the project. GraphQL returns error. Handler adds the issue to the project first (`addProjectV2Item`), then retries.
- **Rate limit on mutations:** REST mutations count against the 5000/hour REST limit (separate from GraphQL). Batch mutations in a single GraphQL request reduce GraphQL point usage.

---

## 7. Projects V2 Field Management

### 7.1 `resolve_project()` algorithm

```
resolve_project(client) → Result<()>:

  // 1. Find project number
  IF config.project_number IS SET:
    project_number = config.project_number
  ELSE:
    // Auto-detect: query repository's linked projects
    projects = graphql("repository { projectsV2(first: 10) { nodes { number title } } }")
    IF projects.is_empty():
      RETURN Err(ProjectNotConfigured)
    project_number = projects[0].number

  // 2. Resolve project node ID
  project_id = graphql("repository { projectV2(number: {n}) { id } }")

  // 3. Resolve field IDs and option IDs
  fields = graphql("node(id: projectId) { ... on ProjectV2 {
    fields(first: 30) { nodes {
      ... on ProjectV2SingleSelectField { id name options { id name } }
      ... on ProjectV2Field { id name dataType }
    }}
  }}")

  // 4. Map to ProjectFieldIds
  field_ids = map_fields(fields, EXPECTED_FIELDS)
  // EXPECTED_FIELDS: Status, Priority, Agent, Claimed At, Ready State, Story Points, Defer Until

  // 5. Validate
  FOR each expected_field in EXPECTED_FIELDS:
    IF field NOT FOUND:
      RETURN Err(FieldNotFound { name })
    IF field is SingleSelect AND missing required options:
      tracing::warn!("Field {name} missing option {option}")

  client.project_id = Some(project_id)
  client.field_ids = Some(field_ids)
```

### 7.2 `setup_fields()` algorithm

```
setup_fields(client) → Result<()>:

  existing = query_existing_fields()

  FOR each expected_field in EXPECTED_FIELDS:
    IF existing.contains(expected_field.name):
      // Validate type matches
      IF existing[name].type != expected_field.type:
        tracing::warn!("Field {name} has wrong type")
      CONTINUE

    // Create field
    MATCH expected_field.type:
      SingleSelect:
        create_single_select_field(name, options)
      Text:
        create_text_field(name)
      Date:
        create_date_field(name)
      Number:
        create_number_field(name)

  // Idempotent — safe to run multiple times
```

### 7.3 Field option values

| Field | Options |
|---|---|
| Status | `open`, `in_progress`, `blocked`, `deferred`, `closed` |
| Priority | `P0`, `P1`, `P2`, `P3`, `P4` |
| Ready State | `ready`, `blocked`, `not_ready`, `closed` |

---

## 8. View Management

### 8.1 Pre-configured views

| View | Layout | Filter | Purpose |
|---|---|---|---|
| `𝍄 UNBLOCK://ready` | Board | `"Ready State":"ready"` | Agent's ready queue |
| `𝍄 UNBLOCK://team` | Board | *(no filter)* | Who is working on what |
| `𝍄 UNBLOCK://pipeline` | Board | *(no filter)* | Classic kanban |
| `𝍄 UNBLOCK://roadmap` | Table | *(no filter)* | Epic-level progress |
| `𝍄 UNBLOCK://timeline` | Roadmap | *(no filter)* | Date-based timeline |

### 8.2 View creation via REST

```
create_view(client, view_config) → Result<()>:

  // REST API (2026-03-10)
  // Determine endpoint based on owner type
  IF owner_type == Org:
    url = "/orgs/{org}/projectsV2/{n}/views"
  ELSE:
    url = "/users/{user}/projectsV2/{n}/views"

  // Discover field integer IDs for visible_fields
  fields_response = GET "{owner_endpoint}/projectsV2/{n}/fields"
  field_int_ids = map fields by name → integer ID

  body = {
    "name": view_config.name,
    "layout": view_config.layout,
    "filter": view_config.filter,
    "visible_fields": [field_int_ids for requested fields]
  }

  POST url with body
  Header: X-GitHub-Api-Version: 2026-03-10
```

### 8.3 Edge cases

- **View already exists:** REST returns 422 or similar. Handler skips (idempotent).
- **`group_by`, `sort_by`:** Read-only in REST API — cannot be set programmatically. Document as manual configuration step.
- **Field integer IDs:** REST `/fields` returns integer IDs (not GraphQL node IDs). The response format for options is `{ "raw": "Todo", "html": "Todo" }` — parse `.raw`.

---

## 9. Circuit Breaker Algorithm

### 9.1 State machine

```
States: Closed, Open, HalfOpen

Transitions:
  Closed → Open:     failure_count >= failure_threshold (default: 5)
  Open → HalfOpen:   cooldown elapsed (default: 10s)
  HalfOpen → Closed: one successful request
  HalfOpen → Open:   one failed request
```

### 9.2 Algorithm

```
check() → Result<()>:
  lock inner
  MATCH state:
    Closed:
      RETURN Ok(())
    Open:
      IF now - last_failure >= cooldown:
        state = HalfOpen
        RETURN Ok(())  // allow one probe
      ELSE:
        RETURN Err(CircuitBreakerOpen)
    HalfOpen:
      RETURN Ok(())  // probe in progress

record_success():
  lock inner
  failure_count = 0
  state = Closed

record_failure():
  lock inner
  failure_count += 1
  last_failure = now
  MATCH state:
    Closed:
      IF failure_count >= failure_threshold:
        state = Open
    HalfOpen:
      state = Open  // probe failed
    Open:
      // already open
```

### 9.3 What counts as a failure

| Response | Circuit breaker | Rationale |
|---|---|---|
| Network error | `record_failure()` | GitHub unreachable |
| HTTP 429 | `record_failure()` | Sustained rate limiting |
| HTTP 500 | `record_failure()` | Server error |
| HTTP 502 | `record_failure()` | Bad gateway |
| HTTP 503 | `record_failure()` | Service unavailable |
| HTTP 200 | `record_success()` | Success |
| HTTP 4xx (except 429) | Neither | Application error, not infrastructure |

### 9.4 Integration with request flow

```
github_request(method, url, body) → Result<Response>:

  // 1. Circuit breaker check
  self.circuit_breaker.check()?

  // 2. Retry loop
  result = retry_with_backoff(&self.retry_policy, is_retryable, || {
    self.http.request(method, url).body(body).send()
  })

  // 3. Record outcome
  MATCH result:
    Ok(response) IF response.status().is_success():
      self.circuit_breaker.record_success()
    Ok(response) IF is_circuit_breaker_failure(response.status()):
      self.circuit_breaker.record_failure()
    Err(_):
      self.circuit_breaker.record_failure()

  RETURN result
```

---

## 10. Retry Algorithm

### 10.1 Exponential backoff with jitter

```
retry_with_backoff(policy, should_retry, operation) → Result<T>:

  last_error = None

  FOR attempt in 0..=policy.max_retries:
    result = operation()

    MATCH result:
      Ok(value):
        RETURN Ok(value)
      Err(error):
        IF NOT should_retry(&error):
          RETURN Err(error)  // non-retryable, propagate immediately

        IF attempt == policy.max_retries:
          RETURN Err(error)  // exhausted retries

        delay = compute_delay(policy, attempt)
        tracing::warn!(attempt, delay_ms = delay.as_millis(), "Retrying after error: {error}")
        sleep(delay)
        last_error = Some(error)

  RETURN Err(last_error.unwrap())
```

### 10.2 Delay computation

```
compute_delay(policy, attempt) → Duration:

  base_ms = policy.base_delay.as_millis() * 2^attempt
  capped_ms = min(base_ms, policy.max_delay.as_millis())

  // ±25% jitter: uniform in [0.75, 1.25]
  jitter_factor = 0.75 + random() * 0.5
  final_ms = capped_ms * jitter_factor

  RETURN Duration::from_millis(final_ms)

  // Example (base=500ms, max=5000ms):
  //   attempt 0:  500ms * [0.75, 1.25] = [375ms, 625ms]
  //   attempt 1: 1000ms * [0.75, 1.25] = [750ms, 1250ms]
  //   attempt 2: 2000ms * [0.75, 1.25] = [1500ms, 2500ms]
  //   attempt 3: 4000ms * [0.75, 1.25] = [3000ms, 5000ms]
  //   attempt 4+: 5000ms * [0.75, 1.25] = [3750ms, 5000ms] (capped)
```

### 10.3 Retryable conditions

| Error | Retryable | Rationale |
|---|---|---|
| `RateLimited` (HTTP 429) | ✅ | Transient — wait and retry |
| `GitHubUnavailable` (network) | ✅ | Transient — connection issue |
| `GitHubServerError` (HTTP 503) | ✅ | Transient — service unavailable, request not processed |
| `GitHubServerError` (HTTP 500, 502) | ❌ | Server may have partially processed request |
| `GitHubApi` (HTTP 4xx except 429) | ❌ | Application error — retrying won't help |
| `GitHubGraphQL` (GraphQL errors) | ❌ | Query error — retrying won't help |
| `Domain` errors | ❌ | Business logic — retrying won't help |
| `CircuitBreakerOpen` | ❌ | Fail fast — don't retry |

---

## 11. Pagination Algorithm

### 11.1 Cursor-based pagination

```
fetch_all_pages(query_template, variables) → Result<Vec<Node>>:

  all_nodes = []
  cursor = None

  LOOP:
    variables["cursor"] = cursor
    response = graphql(query_template, variables)

    nodes = response.data.repository.issues.nodes
    all_nodes.extend(nodes)

    page_info = response.data.repository.issues.pageInfo
    IF page_info.hasNextPage:
      cursor = Some(page_info.endCursor)
    ELSE:
      BREAK

  RETURN all_nodes
```

### 11.2 Edge cases

- **Empty repo:** Zero issues → zero pages → empty result. Valid.
- **Exactly 100 issues:** One page, `hasNextPage: false`.
- **101 issues:** Two pages. First: 100 issues, `hasNextPage: true`. Second: 1 issue, `hasNextPage: false`.
- **Rate limit mid-pagination:** If a page request hits 429, retry handles it. If circuit breaker opens, pagination aborts with error.
- **Concurrent mutations:** An issue created between page 1 and page 2 may be missed. Acceptable — the next cache rebuild catches it.

---

## 12. Error Catalogue

| Error | HTTP | Trigger | Retryable |
|---|---|---|---|
| `GitHubApi { message }` | 4xx (except 429) | REST/GraphQL application error | ❌ |
| `GitHubGraphQL { errors }` | 200 | GraphQL response with errors array | ❌ |
| `GitHubUnavailable { source }` | network | Connection failure, timeout, DNS | ✅ |
| `GitHubServerError { status, message }` | 500, 502, 503 | Server error | ✅ (503 only) |
| `RateLimited` | 429 | Rate limit exceeded | ✅ |
| `CircuitBreakerOpen` | — | Circuit breaker in Open state | ❌ |
| `ProjectNotConfigured` | — | No project ID resolved | ❌ |
| `GitRemote { message }` | — | Cannot parse git remote | ❌ |
| `ViewCreationFailed { message }` | — | REST view creation error | ❌ |
| `OwnerDetectionFailed { owner, message }` | — | Cannot detect org vs user | ❌ |
| `Domain { source }` | varies | Propagated from `unblock-core` | ❌ |

---

## 13. Invariants

1. **Token never logged.** GitHub token is redacted in all debug output. Never included in MCP responses. Remote: wrapped in `secrecy::SecretString`.
2. **All URLs go through centralised methods.** No hardcoded `api.github.com` outside `Config::default()`. GHE Server `/v3` stripping in one place.
3. **Circuit breaker sees final result.** After all retries are exhausted, the circuit breaker records the final outcome. Transient failures recovered by retry do not count.
4. **Read queries are idempotent.** Any GraphQL read can be retried safely.
5. **All requests (reads and writes) are retried on 429 and 503 only.** These are infrastructure-level transient errors where the server did not process the request. Application errors (4xx except 429) and GraphQL errors are never retried. HTTP 503 means "service unavailable" — the request was not processed, so retry is safe even for writes. HTTP 500/502 are NOT retried because the server may have partially processed the request.
6. **Pagination is complete.** `fetch_graph_data()` follows all pages until `hasNextPage: false`.
7. **Field IDs are resolved once.** `resolve_project()` caches `ProjectFieldIds`. No re-resolution per-request.
8. **Auth token refreshed transparently.** For App auth, callers never see expired tokens — `token()` handles caching and refresh internally.

---

## 14. Open Questions

1. **Write mutation retry.** ~~Should idempotent writes be retried on 503?~~ **Resolved:** All requests (reads and writes) are retried on 429 and 503. HTTP 503 means "service unavailable" — the server did not process the request, making retry safe. See invariant 5.

2. **GraphQL mutation batching for cascade.** When a cascade unblocks 10 issues, we currently update each issue's fields individually. Should we batch all field updates into a single GraphQL mutation? Current answer: yes, via `batch_update_fields`. But the cascade comment (one per unblocked issue) still requires individual REST calls.

3. **Token rotation.** For long-running sessions, should the server detect PAT rotation (user generates a new token)? Current answer: no — the server uses the token from startup. A new token requires a server restart.

---

*This spec defines GitHub API client algorithms, authentication, resilience patterns, and pagination. Graph computation is in [01-spec-graph-engine.md](./01-spec-graph-engine.md). Tool handler logic is in [03-spec-mcp-tools.md](./03-spec-mcp-tools.md).*
