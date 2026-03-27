# Unblock — Extensions Plan

**Smart Projects V2 board views created automatically by `://setup`.**

| | |
|---|---|
| **Version** | 0.1.0-draft |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Date** | March 2026 |
| **Status** | Draft |
| **Depends on** | `unblock-prd-github.md`, `unblock-architecture-github.md` |

---

## 1. Overview

The MCP server creates 7 Projects V2 custom fields via `://setup`. This extension adds **4 pre-configured views** to the same setup — turning the raw field data into usable board layouts that surface Unblock intelligence through the native GitHub UI.

No additional infrastructure. No new commands. No hosted services. Just 4 GraphQL mutations added to the existing `://setup` tool.

---

## 2. Views

### 2.1 `://ready` board

The agent's `://ready` command as a visual board.

| Setting | Value |
|---|---|
| Layout | Board |
| Group by | Priority |
| Filter | `Ready State = ready` |
| Sort | Created date ASC (oldest first) |
| Fields shown | Title, Priority, Story Points, Agent, Labels |

P0, P1, P2, P3, P4 columns. Only ready issues. The team sees at a glance what's available to work on and at what priority.

### 2.2 `://team` board

Who is working on what.

| Setting | Value |
|---|---|
| Layout | Board |
| Group by | Status |
| Filter | `State = open` |
| Sort | Priority ASC |
| Swimlanes | Agent field (each developer gets a row) |
| Fields shown | Title, Priority, Agent, Story Points, Claimed At |

Tech lead view. Each developer is a swimlane within status columns. Empty swimlane = idle developer. Immediately answers "who is doing what right now?"

### 2.3 `://pipeline` board

Classic kanban — the full workflow in one view.

| Setting | Value |
|---|---|
| Layout | Board |
| Group by | Status |
| Filter | `State = open` |
| Sort | Priority ASC |
| Columns | open → in_progress → blocked → deferred → closed |
| Fields shown | Title, Priority, Agent, Labels, Ready State |

Issues flow left to right. Labels like `needs-review`, `approved`, `qa-passed` are visible as badges. The default "how is the project going?" view.

### 2.4 `://roadmap` table

Epic-level progress grouped by milestone.

| Setting | Value |
|---|---|
| Layout | Table |
| Group by | Milestone |
| Sort | Priority ASC, then Created ASC |
| Fields shown | Title, Type, Priority, Status, Agent, Story Points, Defer Until |

Tech lead sees progress per epic — how many tasks done, how many remaining, total story points. Sprint planning view.

---

## 3. Implementation

### 3.1 GraphQL

Projects V2 views are created via mutations:

```graphql
mutation {
  createProjectV2View(input: {
    projectId: "...",
    name: "://ready",
    layout: BOARD_LAYOUT
  }) {
    projectV2View { id }
  }
}
```

After view creation, field configurations (group by, filter, sort, visible fields) are set via `updateProjectV2View` and related mutations. The `://setup` tool already has the project ID and all field IDs — no additional API calls to discover them.

### 3.2 Integration with `://setup`

Added as the last step of `://setup`, after field creation and validation:

```
://setup flow:
  1. Resolve project                    ← existing
  2. Create/verify 7 custom fields      ← existing
  3. Create "Review Findings" milestone  ← existing
  4. Create 4 board views               ← NEW
  5. Migrate existing issues (if --migrate) ← existing
```

### 3.3 Idempotency

If a view with the same name already exists, skip creation. The developer can customize views after creation (rename, change filters, add fields). Unblock never overwrites existing views. Re-running `://setup` is safe.

### 3.4 Effort

4 GraphQL mutations with field configuration. Estimated: 1 day. All within the existing `unblock-github` crate — no new dependencies, no new infrastructure.

---

## 4. Design Decisions

| # | Decision | Rationale |
|---|---|---|
| EX1 | Views created by `://setup`, not a separate tool | No new command. Setup already runs once per repo. Views are additive |
| EX2 | Idempotent — skip if name exists | Developer can customize after creation. Unblock doesn't overwrite manual changes |
| EX3 | `://` prefix in view names | Brand consistency. Views are identifiable as Unblock-created in the Projects UI |
| EX4 | 4 views cover 4 personas | Ready (agent), Team (tech lead), Pipeline (everyone), Roadmap (planning) |
