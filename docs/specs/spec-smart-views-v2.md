# SPEC: Smart Projects V2 Board Views

**Status:** DRAFT
**Author:** Ada (architect)
**Date:** 2026-03-27
**Source PRD:** `/Users/ramosmig/Public/WS-Labs/unblock/docs/research/unblock-extensions-plan.md`

## Overview

Feasibility assessment and integration plan for adding 4 pre-configured GitHub Projects V2 views to the existing `://setup` tool flow.

---

## 1. Feasibility Assessment

### Verdict: FEASIBLE WITH CAVEATS

The feature is architecturally sound and fits naturally into the existing setup flow. However, there is one critical API limitation that must be investigated before committing to the plan, and the effort estimate of 1 day is optimistic.

---

## 2. GitHub Projects V2 View API Analysis

### 2.1 What the API Supports

The GitHub GraphQL API provides these mutations for view management:

| Mutation | Purpose | Status |
|---|---|---|
| `createProjectV2View` | Create a view with name + layout | **GA** |
| `updateProjectV2View` | Update filter, sort, group-by, visible fields | **GA** |
| `deleteProjectV2View` | Delete a view | **GA** |

The `createProjectV2View` input accepts:
- `projectId: ID!`
- `name: String`
- `layout: ProjectV2ViewLayout` (BOARD_LAYOUT, TABLE_LAYOUT, ROADMAP_LAYOUT)

The `updateProjectV2View` input accepts:
- `projectViewId: ID!`
- `name: String`
- `layout: ProjectV2ViewLayout`
- `filter: ProjectV2ViewFilter` (filter expression string)
- `sortBy: [ProjectV2ViewSortBy!]` (field ID + direction)
- `groupBy: [ID!]` (field IDs to group by)
- `visibleFields: [ID!]` (field IDs to show)

### 2.2 Critical Risk: Swimlanes

The `://team` board specifies **swimlanes** (grouping rows by Agent field within Status columns). The GitHub Projects V2 API calls this "slice by" or "row group by." As of March 2026, the GraphQL API's `updateProjectV2View` mutation does **not** expose a `sliceBy` or equivalent parameter for swimlane configuration. Swimlanes can only be configured through the GitHub UI.

**Impact:** The `://team` view can be created with correct layout, filter, sort, and group-by, but the swimlane-by-Agent configuration would need to be applied manually by the user after setup. The setup tool should document this in its output.

### 2.3 Filter Syntax

GitHub Projects V2 filters use a text-based query language. The relevant filters:

- `://ready`: `field:ReadyState,Ready` (filter to ReadyState = Ready)
- `://team`: `is:open` (filter to open state)
- `://pipeline`: `is:open`
- `://roadmap`: no filter (show all)

Filter syntax must be validated against the actual GitHub Projects V2 filter grammar. The field reference format may require the field name or field ID depending on the API version.

---

## 3. Integration Points

### 3.1 Crate: `unblock-github` (projects.rs)

This is where the bulk of the work goes. New functions needed:

| Function | Purpose |
|---|---|
| `setup_views(&self, project_id: &str, field_ids: &ProjectFieldIds) -> Result<ViewSetupReport, Error>` | Orchestrator: creates 4 views idempotently |
| `fetch_existing_views(&self, project_id: &str) -> Result<HashMap<String, String>, Error>` | Query existing view names to node IDs |
| `create_view(&self, project_id: &str, spec: &ViewSpec) -> Result<String, Error>` | Create a single view, return view node ID |
| `configure_view(&self, view_id: &str, spec: &ViewSpec, field_ids: &ProjectFieldIds) -> Result<(), Error>` | Apply filter, sort, group-by, visible fields |

New types needed in `projects.rs`:

```
struct ViewSpec {
    name: &'static str,
    layout: &'static str,        // "BOARD_LAYOUT" or "TABLE_LAYOUT"
    group_by_field: &'static str, // field name from ProjectFieldIds
    filter: Option<&'static str>,
    sort_field: Option<&'static str>,
    sort_direction: &'static str, // "ASC" or "DESC"
    visible_fields: &'static [&'static str],
}

pub struct ViewSetupReport {
    pub created: Vec<String>,
    pub skipped: Vec<String>,
}
```

The 4 view specs would be defined as a `const REQUIRED_VIEWS: &[ViewSpec]` array, following the same pattern as `REQUIRED_FIELDS`.

### 3.2 Crate: `unblock-mcp` (tools/setup.rs)

Minimal changes:

1. Add `views_created: Vec<String>` and `views_skipped: Vec<String>` to `SetupResult`
2. In dry-run mode, add `views_missing: Vec<String>` (views that would be created)

### 3.3 Crate: `unblock-mcp` (server.rs)

The setup tool handler needs one additional call after `setup_fields`:

```
// Step 4: Create views (after fields, because views reference field IDs)
let view_report = client.setup_views(&project_info.id, &report.field_ids).await
    .map_err(crate::errors::github_error_to_mcp)?;
```

This fits naturally after field creation because views depend on field IDs for group-by, sort, and visible-fields configuration.

### 3.4 Crate: `unblock-github` (graphql.rs)

No changes needed. The existing `graphql()` helper is sufficient for view mutations. No preview headers required.

### 3.5 Crate: `unblock-core`

No changes needed. Views are purely a GitHub Projects V2 concern with no domain model impact.

---

## 4. Idempotency Strategy Assessment

The plan's idempotency strategy is **sound but needs refinement**:

**Current proposal:** Skip creation if a view with the same name exists.

**Assessment:** This is the right approach. However, there is a subtlety:

- **Create vs. Configure**: If a view named `://ready` already exists but was created in a previous version with different configuration (e.g., different filter), should `://setup` update its configuration?
- **Recommendation:** Match the field strategy -- skip entirely if name exists. The user may have customized the view. Never overwrite. This is consistent with EX2 in the extensions plan.
- **Query mechanism:** Use `node(id: projectId) { ... on ProjectV2 { views(first: 50) { nodes { id name } } } }` to fetch existing views. This mirrors the `fetch_existing_fields` pattern.

---

## 5. GraphQL Cost and Rate Limits

### API Calls for View Creation (worst case, all 4 views are new)

| Step | Calls | Mutation |
|---|---|---|
| Fetch existing views | 1 | Query |
| Create 4 views | 4 | `createProjectV2View` |
| Configure 4 views | 4 | `updateProjectV2View` |
| **Total** | **9** | |

### Combined with Existing Setup Flow

The current setup flow already makes up to ~15 API calls (1 project resolve + 1 field query + up to 7 field creates + field option configs). Adding 9 more brings the total to ~24.

**Rate limit impact:** GitHub's GraphQL API has a point-based rate limit (5,000 points/hour). Simple mutations cost 1 point each. 24 points per setup run is negligible. No concern here.

**However:** Each mutation is a separate HTTP round-trip. On slow connections, 9 additional round-trips add noticeable latency. Consider batching view creates into a single GraphQL request with aliases if performance becomes a concern.

---

## 6. Risks and Mitigations

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| R1 | Swimlane API not available via GraphQL | **HIGH** | Degrade `://team` view gracefully -- create without swimlanes, document in setup output that swimlane-by-Agent must be configured manually in GitHub UI |
| R2 | Filter syntax mismatch | **MEDIUM** | Validate filter strings against a test project before hardcoding. GitHub's filter grammar is underdocumented. Consider building filters programmatically if field-ID-based filtering is required |
| R3 | Partial failure mid-setup (2 of 4 views created) | **LOW** | Idempotency handles this -- re-running setup will skip the 2 created views and retry the 2 missing ones. Same pattern as field creation |
| R4 | `updateProjectV2View` sort/group parameters may require field IDs, not names | **MEDIUM** | The `setup_views` function receives `ProjectFieldIds` which contains all resolved IDs. Map field names to IDs at configuration time |
| R5 | View limit per project | **LOW** | GitHub Projects V2 allows up to 50 views per project. 4 views is well within the limit |
| R6 | `visibleFields` parameter may not include built-in fields (Title, Labels, Milestone) | **MEDIUM** | Built-in fields (Title, Assignees, Labels, Milestone) have special IDs that must be queried from the project. The existing `fetch_existing_fields` query may need expansion to capture built-in field IDs as well |

---

## 7. Effort Estimate Re-Assessment

The extensions plan estimates 1 day. My assessment:

| Task | Estimate |
|---|---|
| Research: validate GraphQL view mutations against a real project | 2-3 hours |
| Implement `fetch_existing_views` + `create_view` + `configure_view` | 4-5 hours |
| Handle built-in field ID resolution for `visibleFields` | 2 hours |
| Update `SetupResult` and setup tool handler | 1 hour |
| Tests (unit + integration) | 3-4 hours |
| Handle `://team` swimlane degradation + documentation | 1 hour |
| **Total** | **~2 days** |

The 1-day estimate is realistic only if the swimlane issue is punted entirely and the filter syntax works on the first try. A safe estimate is **2 days**.

---

## 8. Recommended Implementation Approach

### Phase 1: API Validation (prerequisite)

Before writing any Rust code, validate the following against a real GitHub Projects V2 project using GraphQL Explorer or curl:

1. `createProjectV2View` with BOARD_LAYOUT and TABLE_LAYOUT
2. `updateProjectV2View` with filter, sortBy, groupBy, visibleFields
3. Confirm filter syntax for `ReadyState = Ready` and `is:open`
4. Confirm whether `visibleFields` requires built-in field IDs
5. Confirm swimlane API availability (or lack thereof)

### Phase 2: Implementation

1. Add `ViewSpec` type and `REQUIRED_VIEWS` constant to `projects.rs`
2. Implement `fetch_existing_views` (query pattern mirrors `fetch_existing_fields`)
3. Implement `create_view` (create-only, returns view ID)
4. Implement `configure_view` (updateProjectV2View with full config)
5. Implement `setup_views` orchestrator (idempotent loop over REQUIRED_VIEWS)
6. Update `SetupResult` in `tools/setup.rs` to include view status
7. Wire `setup_views` call into the setup tool handler in `server.rs`
8. Update dry-run path to report existing/missing views

### Phase 3: Testing

1. Unit tests for `ViewSpec` definitions
2. Integration test for `setup_views` against a test project
3. Idempotency test (run setup twice, second run skips all views)

---

## 9. Suggested Changes to the Plan

1. **Drop swimlanes from `://team` view spec** until GitHub exposes the API. Document it as a known limitation with a manual configuration step.

2. **Add `://roadmap` layout clarification**: The plan says "Table" layout but the `ROADMAP_LAYOUT` enum value exists in the API. If the intent is a roadmap/timeline view, use `ROADMAP_LAYOUT`. If it is a flat table, use `TABLE_LAYOUT`. These are different in the GitHub UI.

3. **Add built-in field discovery** to the implementation section. The `visibleFields` parameter likely requires node IDs for built-in fields like Title and Labels, which are not part of the custom fields resolved by `setup_fields`.

4. **Consider adding view status to `SetupReport`** at the `unblock-github` level (not just MCP level) so the library is reusable by the future desktop app.

---

## 10. Implementation Tasks

1. **API validation spike** -- validate GraphQL view mutations against real project -> rust-supervisor
2. **Add view types and specs to `projects.rs`** -> rust-supervisor
3. **Implement view CRUD in `projects.rs`** (fetch_existing_views, create_view, configure_view, setup_views) -> rust-supervisor
4. **Update `SetupResult` and wire into setup handler** -> rust-supervisor
5. **Add tests** -> rust-supervisor

All tasks are rust-supervisor scope. No frontend or infra work required.

## Dependencies

- Task 1 (API validation) must complete before tasks 2-5 begin.
- Tasks 2-4 are sequential (types before CRUD before wiring).
- Task 5 can partially overlap with tasks 3-4.
