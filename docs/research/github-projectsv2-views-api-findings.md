# GitHub Projects V2 Views API — Research Findings

**Date:** 2026-03-27
**Sources:**
- `docs/research/schema.docs.graphql` (full GitHub GraphQL schema)
- GitHub REST API OpenAPI spec `api.github.com.2026-03-10.json`
- `docs/research/github-rest-projectsv2-views.json` (extracted REST view endpoints)

---

## Critical Finding: REST API Supports View Creation (GraphQL Does Not)

**The GitHub GraphQL API does NOT provide mutations for views — but the REST API (v2026-03-10) DOES support creating views.**

After exhaustive search of the full schema (70k+ lines), the following mutations exist for Projects V2:

### Available ProjectV2 Mutations (complete list)
| Mutation | Purpose |
|---|---|
| `createProjectV2` | Create a new project |
| `updateProjectV2` | Update project metadata (title, description, public, closed) |
| `deleteProjectV2` | Delete a project |
| `copyProjectV2` | Copy a project |
| `createProjectV2Field` | Create a custom field |
| `updateProjectV2Field` | Update a custom field |
| `deleteProjectV2Field` | Delete a custom field |
| `updateProjectV2ItemFieldValue` | Set a field value on an item |
| `clearProjectV2ItemFieldValue` | Clear a field value on an item |
| `updateProjectV2ItemPosition` | Reorder items |
| `deleteProjectV2Item` | Remove an item |
| `addProjectV2ItemById` | Add an issue/PR to a project |
| `addProjectV2DraftIssue` | Add a draft issue |
| `updateProjectV2DraftIssue` | Update a draft issue |
| `createProjectV2StatusUpdate` | Create a status update |
| `updateProjectV2StatusUpdate` | Update a status update |
| `deleteProjectV2StatusUpdate` | Delete a status update |
| `updateProjectV2Collaborators` | Manage collaborators |
| `deleteProjectV2Workflow` | Delete a workflow |

**Notably absent:**
- ~~`createProjectV2View`~~ — DOES NOT EXIST
- ~~`updateProjectV2View`~~ — DOES NOT EXIST
- ~~`deleteProjectV2View`~~ — DOES NOT EXIST

This means **the extensions plan's core assumption is invalid** — views cannot be created or configured programmatically through the public GraphQL API.

---

## 1. View Object Structure (Read-Only)

The `ProjectV2View` type exists and is **queryable** (read-only). Located at schema line 41035.

```graphql
type ProjectV2View implements Node {
  createdAt: DateTime!
  databaseId: Int @deprecated       # Use fullDatabaseId
  fullDatabaseId: BigInt
  id: ID!
  layout: ProjectV2ViewLayout!
  name: String!
  number: Int!
  project: ProjectV2!
  updatedAt: DateTime!
  filter: String                    # Free-text filter string (like UI filter bar)

  # Field visibility (current API)
  fields(orderBy: ProjectV2FieldOrder): ProjectV2FieldConfigurationConnection

  # Group-by (current API)
  groupByFields(orderBy: ProjectV2FieldOrder): ProjectV2FieldConfigurationConnection

  # Sort-by (current API)
  sortByFields: ProjectV2SortByFieldConnection

  # Vertical group-by / swimlanes (current API)
  verticalGroupByFields(orderBy: ProjectV2FieldOrder): ProjectV2FieldConfigurationConnection

  # DEPRECATED connections (still in schema but scheduled for removal)
  groupBy: ProjectV2FieldConnection @deprecated          # Use groupByFields
  sortBy: ProjectV2SortByConnection @deprecated           # Use sortByFields
  verticalGroupBy: ProjectV2FieldConnection @deprecated   # Use verticalGroupByFields
  visibleFields: ProjectV2FieldConnection @deprecated     # Use fields
}
```

### Accessing Views from a Project

```graphql
# Single view by number
ProjectV2.view(number: Int!): ProjectV2View

# All views with pagination and ordering
ProjectV2.views(
  after: String
  before: String
  first: Int
  last: Int
  orderBy: ProjectV2ViewOrder = {field: POSITION, direction: ASC}
): ProjectV2ViewConnection!
```

---

## 2. Layout Types

```graphql
enum ProjectV2ViewLayout {
  BOARD_LAYOUT      # Kanban board
  ROADMAP_LAYOUT    # Timeline/roadmap
  TABLE_LAYOUT      # Spreadsheet table
}
```

Three layouts available. The extensions plan references BOARD and TABLE, which are both valid.

---

## 3. Configuration Matrix

| Capability | Query (Read) | Mutate (Write) | Schema Location | Notes |
|---|---|---|---|---|
| View CRUD | YES (query views) | **NO** | lines 38882-38932 | Can list/read views, CANNOT create/update/delete |
| View layout | YES (read `layout`) | **NO** | line 41160 | `ProjectV2ViewLayout` enum readable |
| View name | YES (read `name`) | **NO** | line 41165 | String, read-only |
| View filter | YES (read `filter`) | **NO** | line 41082 | Free-text `String` (e.g., `"is:open label:bug"`) |
| Visible fields | YES (read `fields`) | **NO** | lines 41052-41077 | Returns `ProjectV2FieldConfigurationConnection` |
| Group-by | YES (read `groupByFields`) | **NO** | lines 41125-41150 | Returns `ProjectV2FieldConfigurationConnection` |
| Sort-by | YES (read `sortByFields`) | **NO** | lines 41208-41228 | Returns `ProjectV2SortByFieldConnection` |
| Vertical group-by (swimlanes) | YES (read `verticalGroupByFields`) | **NO** | lines 41271-41296 | Returns `ProjectV2FieldConfigurationConnection` |
| Field CRUD | YES | YES | lines 8621-8667, 67470-67519 | `createProjectV2Field`, `updateProjectV2Field`, `deleteProjectV2Field` |
| Item field values | YES | YES | lines 67562-67600 | `updateProjectV2ItemFieldValue`, `clearProjectV2ItemFieldValue` |
| Project metadata | YES | YES | lines 67525-67560 | `updateProjectV2` (title, description, public, closed, readme) |

---

## 4. Sort, Filter, Group, Slice

### Sort
- **Type:** `ProjectV2SortByField` — contains `direction: OrderDirection!` and `field: ProjectV2FieldConfiguration!`
- **Connection:** `ProjectV2SortByFieldConnection` (paginated)
- **Read:** YES via `view.sortByFields`
- **Write:** NO — no mutation to configure sort
- **Deprecated predecessor:** `ProjectV2SortBy` (uses `ProjectV2Field` instead of `ProjectV2FieldConfiguration`)

### Filter
- **Type:** `String` (free-text) on `ProjectV2View.filter`
- **Format:** Same syntax as the GitHub Projects UI filter bar (e.g., `"status:\"In Progress\" label:bug"`)
- **Read:** YES
- **Write:** NO

### Group-by (horizontal columns)
- **Connection:** `ProjectV2FieldConfigurationConnection` via `view.groupByFields`
- **Returns:** The field(s) used for grouping (typically Status or another single-select)
- **Read:** YES
- **Write:** NO

### Vertical Group-by (swimlanes)
- **Connection:** `ProjectV2FieldConfigurationConnection` via `view.verticalGroupByFields`
- **Returns:** The field(s) used for vertical grouping/swimlanes
- **Read:** YES
- **Write:** NO
- **Note:** The extensions plan's `://team` board uses Agent field as swimlanes — this is the `verticalGroupByFields` capability

### ProjectV2Filters (separate type)
```graphql
input ProjectV2Filters {
  state: ProjectV2State   # OPEN or CLOSED — filters the project list, NOT view items
}
```
This is for filtering projects themselves (at the `projectsV2` connection level), NOT for filtering view items.

---

## 5. Field Types & Built-in Fields

### Type Hierarchy
```
interface ProjectV2FieldCommon
  ├── type ProjectV2Field               # Simple fields (TEXT, NUMBER, DATE, TITLE, and built-in read-only)
  ├── type ProjectV2SingleSelectField   # Single-select with options
  └── type ProjectV2IterationField      # Iteration with configuration

union ProjectV2FieldConfiguration = ProjectV2Field | ProjectV2IterationField | ProjectV2SingleSelectField
```

### ProjectV2FieldType Enum (all possible data types)
| Type | Custom-Creatable | Notes |
|---|---|---|
| `TEXT` | YES | Free-text field |
| `NUMBER` | YES | Numeric field |
| `DATE` | YES | Date field |
| `SINGLE_SELECT` | YES | Single-select with options |
| `ITERATION` | YES | Sprint/iteration field |
| `TITLE` | NO | Built-in, always exists |
| `ASSIGNEES` | NO | Built-in |
| `LABELS` | NO | Built-in |
| `MILESTONE` | NO | Built-in |
| `REPOSITORY` | NO | Built-in |
| `LINKED_PULL_REQUESTS` | NO | Built-in |
| `TRACKS` | NO | Built-in |
| `TRACKED_BY` | NO | Built-in |
| `REVIEWERS` | NO | Built-in |
| `ISSUE_TYPE` | NO | Built-in (GitHub-native issue type) |
| `PARENT_ISSUE` | NO | Built-in |
| `SUB_ISSUES_PROGRESS` | NO | Built-in |

### Custom Field Creation Types
```graphql
enum ProjectV2CustomFieldType {
  DATE
  ITERATION
  NUMBER
  SINGLE_SELECT
  TEXT
}
```
Only these 5 types can be created via `createProjectV2Field`.

### Looking Up Fields by Name
```graphql
ProjectV2.field(name: String!): ProjectV2FieldConfiguration
```
Direct name-based lookup exists on the project — returns the field configuration (or null). Our codebase currently uses `fields(first: 50)` to bulk-fetch all fields.

### Single-Select Options
```graphql
type ProjectV2SingleSelectFieldOption {
  color: ProjectV2SingleSelectFieldOptionColor!
  description: String!
  descriptionHTML: String!
  id: String!
  name: String!
  nameHTML: String!
}

enum ProjectV2SingleSelectFieldOptionColor {
  BLUE | GRAY | GREEN | ORANGE | PINK | PURPLE | RED | YELLOW
}
```

### ProjectV2FieldValue (for setting values)
```graphql
input ProjectV2FieldValue {
  date: Date
  iterationId: String
  number: Float
  singleSelectOptionId: String
  text: String
}
```
Only one field should be set at a time.

---

## 6. Ordering Types

### View Ordering
```graphql
input ProjectV2ViewOrder {
  direction: OrderDirection!
  field: ProjectV2ViewOrderField!
}

enum ProjectV2ViewOrderField {
  CREATED_AT
  NAME
  POSITION
}
```

### Field Ordering
```graphql
input ProjectV2FieldOrder {
  direction: OrderDirection!
  field: ProjectV2FieldOrderField!
}

enum ProjectV2FieldOrderField {
  CREATED_AT
  NAME
  POSITION
}
```

---

## 7. Existing Codebase Capabilities

### `crates/unblock-github/src/projects.rs`
- `resolve_project_info()` — finds project by number, returns `ProjectInfo { id, number }`
- `setup_fields(project_id)` — creates 7 custom fields (idempotent), returns `SetupReport`
- `query_setup_status(project_id)` — dry-run check for field existence
- `update_field(project_id, item_id, field_id, value)` — sets field value on an item
- `fetch_existing_fields(project_id)` — internal helper, queries `fields(first: 50)` with inline fragments for `ProjectV2SingleSelectField` and `ProjectV2Field`

### `crates/unblock-github/src/mutations.rs`
- `create_issue()` — REST + best-effort project add
- `close_issue()` — REST PATCH
- `add_comment()` — REST POST
- `add_blocked_by()` / `remove_blocked_by()` — GraphQL dependency mutations
- `add_sub_issue()` — GraphQL sub-issue mutation

### What We Already Have
- Project ID resolution
- All 7 custom field IDs cached
- Field value updates working
- GraphQL client with error handling

### What We Would Need (if views were writable)
- View creation mutations — **DO NOT EXIST**
- View configuration mutations — **DO NOT EXIST**

---

## 8. Impact on Extensions Plan

### The Plan Is Blocked

The extensions plan (`docs/research/unblock-extensions-plan.md`) assumes the existence of `createProjectV2View` and `updateProjectV2View` mutations. These mutations **do not exist** in the GitHub GraphQL API.

Specifically:
1. Section 3.1 references `createProjectV2View(input: { projectId, name, layout })` — **this mutation does not exist**
2. Section 3.1 references `updateProjectV2View` for configuring group-by, filter, sort, visible fields — **this mutation does not exist**
3. The entire "4 pre-configured views" concept **cannot be implemented** via the public API

### Resolution: REST API Provides View Creation

The REST API (api version `2026-03-10`) **does support** view creation. See Section 8b below for full details.

The plan is **unblocked** — implementation should use the REST API for view creation instead of GraphQL mutations.

---

## 9. New Opportunities (What We CAN Do)

Even though we cannot create views, the read-only query capabilities unlock several useful features:

### 9.1 Query Existing Views
We can query all views on a project and report their configuration:
```graphql
query {
  node(id: $projectId) {
    ... on ProjectV2 {
      views(first: 20) {
        nodes {
          id
          name
          number
          layout
          filter
          fields(first: 20) { nodes { ... on ProjectV2FieldCommon { name dataType } } }
          groupByFields(first: 5) { nodes { ... on ProjectV2FieldCommon { name } } }
          sortByFields(first: 5) { nodes { direction field { ... on ProjectV2FieldCommon { name } } } }
          verticalGroupByFields(first: 5) { nodes { ... on ProjectV2FieldCommon { name } } }
        }
      }
    }
  }
}
```

### 9.2 Setup Validation
Extend `://setup` to **validate** whether the recommended views exist, and provide copy-pasteable instructions if they don't.

### 9.3 View Status Reporting
Add an MCP tool that reports the current view configuration, comparing against the recommended setup.

### 9.4 Single Field Lookup
`ProjectV2.field(name: String!)` allows direct field lookup by name — more efficient than our current bulk fetch of `fields(first: 50)`.

### 9.5 Status Updates
`createProjectV2StatusUpdate` / `updateProjectV2StatusUpdate` — we could create project status updates as part of reporting.

---

## 8a. Validated Findings (tested against real project 2026-03-27)

Tested against `websublime` org, project #4 (Websublime Platform).

### Confirmed Working

| Test | Result | Details |
|---|---|---|
| `GET /fields` | **OK** | Returns all fields (built-in + custom) with integer `id` values |
| `POST /views` (board, no filter) | **OK** | View created successfully |
| `POST /views` (filter: `"Status":"📋 Backlog"`) | **OK** | Quoted field:value syntax works |
| `POST /views` (filter: `Status:📋 Backlog`) | **OK** | Unquoted syntax also works |
| `POST /views` (filter: `is:open`) | **OK** | Built-in filter works |
| `POST /views` (table + visible_fields) | **OK** | Integer field IDs accepted and returned |
| `POST /views` (roadmap) | **OK** | Roadmap layout creates successfully |

### Confirmed Limitations

| Limitation | Evidence |
|---|---|
| `group_by` always `[]` on creation | Cannot be set via API — always empty in response |
| `sort_by` always `[]` on creation | Cannot be set via API — always empty in response |
| `vertical_group_by` always `[]` on creation | Swimlanes cannot be set via API |
| No `PATCH /views` endpoint | Views cannot be updated after creation |
| No `DELETE /views` endpoint | Views cannot be deleted via API |
| No `GET /views` (list) endpoint | Must use GraphQL for idempotency checks |
| Closed projects reject view creation | Error: "Cannot create view for closed project" |

### Surprises vs OpenAPI Spec

| Field | OpenAPI Spec Says | Actual API Returns |
|---|---|---|
| `options[].name` | `string` | `{"raw": "Todo", "html": "Todo"}` — nested object, not string |
| `visible_fields` (not specified) | "default visible fields" | Returns 5 default field IDs automatically |
| `filter` | "filter query string" | Accepts both quoted (`"Field":"Value"`) and unquoted (`Field:Value`) syntax |

### GraphQL Idempotency Query

The `viewer { projectV2(number: N) }` query does **not** work for org-owned projects. Must use:

```graphql
query {
  organization(login: "websublime") {
    projectV2(number: 4) {
      views(first: 30) {
        nodes { name number layout }
      }
    }
  }
}
```

Or for user-owned projects: `user(login: "...") { projectV2(number: N) { ... } }`

### Risk R2 Status: RESOLVED

Filter syntax is confirmed working. Both `"Field":"Value"` and `Field:Value` are accepted. The API stores the filter string as-is — no validation or rejection of unknown field names. This means:
- Filters work but are not validated server-side
- A typo in field name won't cause an error at creation — it just won't match anything
- Our implementation should validate field names client-side before creating views

---

## 8b. REST API — View Creation (the path forward)

**API Version:** `2026-03-10`
**Source:** GitHub REST API OpenAPI spec

### Available Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/orgs/{org}/projectsV2/{project_number}/views` | Create view (org-owned project) |
| `POST` | `/users/{user_id}/projectsV2/{project_number}/views` | Create view (user-owned project) |
| `GET` | `/orgs/{org}/projectsV2/{project_number}/views/{view_number}/items` | List items with view filter |
| `GET` | `/users/{username}/projectsV2/{project_number}/views/{view_number}/items` | List items with view filter |
| `GET` | `/orgs/{org}/projectsV2/{project_number}/fields` | List all fields (get IDs for visible_fields) |
| `POST` | `/orgs/{org}/projectsV2/{project_number}/fields` | Create field |
| `GET` | `/orgs/{org}/projectsV2/{project_number}/fields/{field_id}` | Get single field |

**Missing:** No PATCH/PUT (update view), no DELETE (delete view), no GET (list views).

### Create View — Request Body

```json
{
  "name": "Sprint Board",          // required — string
  "layout": "board",               // required — "table" | "board" | "roadmap"
  "filter": "is:issue is:open",    // optional — free-text filter string
  "visible_fields": [123, 456]     // optional — array of integer field IDs (not for roadmap)
}
```

### Create View — Response Schema

```json
{
  "id": 1,                          // integer — unique view ID
  "number": 2,                      // integer — view number within project
  "name": "Sprint Board",           // string
  "layout": "board",                // "table" | "board" | "roadmap"
  "node_id": "PVV_...",             // string — GraphQL node ID
  "project_url": "https://api.github.com/orgs/octocat/projectsV2/1",
  "html_url": "https://github.com/orgs/octocat/projects/1/views/2",
  "creator": { /* simple-user */ },
  "created_at": "2022-04-28T12:00:00Z",
  "updated_at": "2022-04-28T12:00:00Z",
  "filter": "is:issue is:open",     // nullable string
  "visible_fields": [123, 456],     // array of integer field IDs
  "sort_by": [[123, "asc"]],        // array of [field_id, direction] tuples (READ-ONLY)
  "group_by": [456],                // array of field IDs (READ-ONLY)
  "vertical_group_by": [789]        // array of field IDs — swimlanes (READ-ONLY)
}
```

### Field Discovery — Response Schema (`projects-v2-field`)

```json
{
  "id": 123,                         // integer — USE THIS for visible_fields
  "issue_field_id": 456,             // integer — issue-level field ID
  "node_id": "PVTF_...",             // string — GraphQL node ID
  "name": "Priority",                // string
  "data_type": "single_select",      // see enum below
  "options": [                        // only for single_select
    { "name": "P0", "color": "RED", "description": "Critical" }
  ],
  "configuration": { ... },          // only for iteration fields
  "created_at": "...",
  "updated_at": "..."
}
```

**`data_type` enum (all possible values):**
`assignees`, `linked_pull_requests`, `reviewers`, `labels`, `milestone`, `repository`, `title`, `text`, `single_select`, `number`, `date`, `iteration`, `issue_type`, `parent_issue`, `sub_issues_progress`

Built-in fields (assignees, labels, milestone, title, etc.) are returned alongside custom fields — they all have integer `id` values usable in `visible_fields`.

### Configuration Matrix (REST vs GraphQL)

| Capability | REST Create | REST Read | GraphQL Write | GraphQL Read |
|---|---|---|---|---|
| **View name** | YES | YES (response) | NO | YES |
| **Layout** | YES (table/board/roadmap) | YES | NO | YES |
| **Filter** | YES (free-text string) | YES | NO | YES |
| **Visible fields** | YES (integer IDs) | YES | NO | YES |
| **Sort by** | NO (read-only in response) | YES | NO | YES |
| **Group by** | NO (read-only in response) | YES | NO | YES |
| **Vertical group by (swimlanes)** | NO (read-only in response) | YES | NO | YES |
| **Update view** | NO (no PATCH endpoint) | — | NO | — |
| **Delete view** | NO (no DELETE endpoint) | — | NO | — |
| **List views** | NO (no GET /views) | — | — | YES (GraphQL) |

---

## 10. Revised Impact on Extensions Plan

### What's Possible (via REST API)

The extensions plan **is feasible** with adjustments. The REST API supports creating views with:

| View | Layout | Filter | Visible Fields | Group By | Sort By | Swimlanes |
|---|---|---|---|---|---|---|
| `://ready` | board | YES — filter string | YES — field IDs | **NO** (not in create) | **NO** | **NO** |
| `://team` | board | YES | YES | **NO** | **NO** | **NO** |
| `://pipeline` | board | YES | YES | **NO** | **NO** | **NO** |
| `://roadmap` | table | YES | **NO** (not for roadmap) | **NO** | **NO** | **NO** |

### What Needs to Change in the Plan

1. **Group By, Sort By, Swimlanes cannot be set at creation.** These appear read-only in the response but are not accepted as create input parameters. The views will be created with correct layout/filter/visible_fields, but grouping/sorting/swimlanes must be configured manually in the GitHub UI post-creation.

2. **Idempotency requires GraphQL.** The REST API has no `GET /views` (list views) endpoint. To check if a view already exists before creating it, we must use the GraphQL `ProjectV2.views` query. This means a **hybrid approach**: GraphQL to query existing views, REST to create new ones.

3. **Field IDs are integers (REST) not node_ids (GraphQL).** The REST API uses integer `id` for `visible_fields`. Our current codebase works with GraphQL node IDs (`PVTF_...`). We need the REST `GET /fields` endpoint to discover integer IDs, OR we need to extract `databaseId`/`fullDatabaseId` from our existing GraphQL queries.

4. **Dual-path for org vs user projects.** REST has separate endpoints for org-owned vs user-owned projects. We need to detect project ownership and route accordingly.

5. **`://roadmap` should use `table` layout, not `roadmap`.** The `roadmap` layout doesn't support `visible_fields` and is a timeline view (date-range based). The plan's `://roadmap` is described as a table grouped by milestone — so `table` layout with milestone filter is more appropriate. Alternatively, we could leverage the actual `roadmap` layout for a date-based timeline view (a different but potentially useful perspective).

### Recommended Adjusted Strategy

```
://setup flow (revised):
  1. Resolve project (GraphQL)                    ← existing
  2. Create/verify 7 custom fields (GraphQL)      ← existing
  3. List existing views (GraphQL query)           ← NEW — for idempotency
  4. Discover field IDs (REST GET /fields)         ← NEW — for visible_fields
  5. Create missing views (REST POST /views)       ← NEW — name + layout + filter + visible_fields
  6. Report: created views + manual config needed  ← NEW — tell user to set group_by/sort/swimlanes
```

### New View Opportunities

Given the REST API supports `roadmap` layout, we could add a 5th view:

| View | Layout | Purpose |
|---|---|---|
| `://timeline` | roadmap | Date-based timeline of issues with date fields — visual sprint/milestone planning |

This would complement the `://roadmap` table view with a true timeline visualisation.

---

## 11. Appendix: Full ProjectV2 Type Fields

For reference, the complete `ProjectV2` type includes these connections:
- `field(name: String!)` — single field lookup by name
- `fields(first, after, orderBy)` — all fields
- `items(first, after, orderBy, filterBy)` — all items (with filter support via `ProjectV2Filters`)
- `view(number: Int!)` — single view by number
- `views(first, after, orderBy)` — all views
- `repositories(first, after, orderBy)` — linked repos
- `teams(first, after, orderBy)` — linked teams
- `statusUpdates(first, after, orderBy)` — status updates
- `workflows(first, after, orderBy)` — automation workflows
- `owner` — project owner
- `url`, `resourcePath` — web URLs
- `title`, `shortDescription`, `readme` — metadata
- `public`, `closed`, `template` — flags
