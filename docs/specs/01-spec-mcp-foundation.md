# Spec 01 — MCP Foundation (v0.1.0)

> Phase: 01
> Crates: `unblock-core`, `unblock-github`, `unblock-mcp`
> Source: [SPEC](../SPEC.md) · [PRD](../PRD.md) · [MANIFESTO](../MANIFESTO.md)
> Plan: [01-plan-mcp-foundation](../plans/01-plan-mcp-foundation.md)
> Status: APPROVED
> Last updated: 2026-04-30 (unblock-wgj amendment)

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
15. [Appendix A — `unblock-1zj` Amendment Notes](#appendix-a--unblock-1zj-amendment-notes)
16. [Appendix B — `unblock-wgj` Amendment Notes](#appendix-b--unblock-wgj-amendment-notes)

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
    Backlog,
    Ready,
    InProgress,
    Blocked,
    Deferred,
    Closed,
}
```

Projects V2 custom field. Unified workflow + readiness state. **Six variants** (was five pre-`unblock-1zj`); `Backlog` was added as the default state at issue creation time and as a sticky-until-explicit-transition state alongside `Deferred`.

**Canonical option board order (display + serialisation order):**
`Backlog`, `Ready`, `In Progress`, `Blocked`, `Deferred`, `Closed`. This order is the contract consumed by §5.7 (`REQUIRED_FIELDS` Status options) and the §5.8 view filters. The Projects V2 option values are the **TitleCase** strings shown in the order above (NOT lowercase / `snake_case`).

**Transition rules:**
- `Backlog` (initial) → `Ready`/`Blocked`: created issues land in `Backlog`; the server promotes them to `Ready` or `Blocked` ONLY when the user/agent explicitly transitions (e.g. via `update`, `claim`, or first edge change). The graph-computed `Ready` ↔ `Blocked` flip in §10.2 NEVER auto-promotes a `Backlog` issue.
- `Ready` ↔ `Blocked`: computed automatically by MCP server from dependency graph (after the issue has left `Backlog`).
- → `InProgress`: on `claim` (agent/human set; transitions out of `Backlog` if applicable).
- → `Deferred`: on `update` with `defer_until` (agent/human set).
- → `Closed`: on `close` (agent/human set).
- `Blocked`/`Ready` → re-evaluated: on `reopen` (graph-computed; reopened issues do NOT go back to `Backlog` — they re-enter `Ready` or `Blocked` per the graph).

**Who sets what:**
- MCP server manages `Ready` ↔ `Blocked` transitions (graph-computed) for issues that have ALREADY left `Backlog`.
- Agent/human sets `Backlog` (implicit on create), `InProgress`, `Deferred`, `Closed` (preserved by server — never overridden).
- `Backlog` and `Deferred` are both **sticky** preserved states — see §3.3 Filter 2 and §10.2 `compute_expected_status`.

**Projects V2 option values (TitleCase, board-order):** `Backlog`, `Ready`, `In Progress`, `Blocked`, `Deferred`, `Closed`. **No lowercase / `snake_case` variants exist on the wire.** All previous spec references to `ready`, `in_progress`, `blocked`, `deferred`, `closed` (lowercase) and to `Done`/`Todo` (legacy GitHub-default options) are obsolete; the canonical strings are owned by the helper described next.

**Single source of truth — `Status::option_name`** (`unblock-core/src/types.rs`, introduced by `unblock-1zj`):

```rust
impl Status {
    /// Canonical Projects V2 option name (TitleCase, board-order matched).
    /// Single source of truth consumed by:
    ///  - §5.7 REQUIRED_FIELDS Status option list (`unblock-github`)
    ///  - `parse_status_field` in `unblock-github::graphql`
    ///  - `status_slug` previously in `unblock-mcp/src/tools/stats.rs:147-161`
    ///    (REMOVED — callers now route through `Status::option_name`)
    ///  - every literal in `server.rs`, `reopen.rs`, `dep_remove.rs`,
    ///    `setup.rs` (REMOVED — same)
    pub const fn option_name(self) -> &'static str {
        match self {
            Status::Backlog    => "Backlog",
            Status::Ready      => "Ready",
            Status::InProgress => "In Progress",
            Status::Blocked    => "Blocked",
            Status::Deferred   => "Deferred",
            Status::Closed     => "Closed",
        }
    }
}
```

**Discipline (normative).** Every layer that needs a Status string MUST go through `Status::option_name`. No literal `"Ready"`, `"In Progress"`, `"ready"`, `"in_progress"`, `"Done"`, etc. is allowed in `unblock-github` (§5.7 spec list, parse_status_field), `unblock-mcp` (server.rs, reopen.rs, dep_remove.rs, setup.rs, stats.rs), or any test fixture beyond the helper's own unit tests. The previous duplicate `status_slug` helper at `crates/unblock-mcp/src/tools/stats.rs:147-161` is REMOVED — its callers route through `Status::option_name` directly. This collapses six historical literal sites into one and is enforced by a clippy-style scan in the implementation PR (the implementation supervisor adds a CI grep guard or equivalent).

**API change (BREAKING — library crate `unblock-core`).** Adding `Status::Backlog` is an additive enum variant change; existing `match Status { ... }` arms in downstream code (currently only in-workspace) must add a `Backlog` arm. Per the project's `#[non_exhaustive]` discipline (CLAUDE.md "Coding Standards"), `Status` SHOULD carry `#[non_exhaustive]` as part of this change so future additions don't require coordination — this is not a behaviour change but a forward-compat hardening. The implementation PR MUST carry a `BREAKING CHANGE:` footer for the variant addition AND an `API:` line for `Status::option_name`.

### 2.4 `Priority`

```rust
#[non_exhaustive]
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

**API change (BREAKING — library crate `unblock-core`).** Adding `#[non_exhaustive]` to `Priority` is the same forward-compat hardening applied to `Status` in `unblock-1zj` and `IssueType` in `unblock-wgj`. Existing exhaustive `match Priority { … }` arms in downstream code (currently only in-workspace) MUST add a wildcard `_` arm or a per-variant arm for new entries. Per CLAUDE.md "Coding Standards" `#[non_exhaustive]` is mandatory on growable public enums in library crates. The implementation PR for `unblock-q2x` MUST carry a `BREAKING CHANGE:` footer for the `#[non_exhaustive]` addition AND an `API:` footer for the additive helpers `Priority::ALL`, `Priority::canonical_name`, and `Priority::short_code` defined below.

**Single source of truth — `Priority` canonical helpers** (`unblock-core/src/types.rs`, introduced by `unblock-q2x`). Mirrors the `Status::option_name` discipline (§2.3) and the `IssueType::canonical_name` discipline (§2.6): every layer that needs a Projects V2 Priority option string MUST go through these helpers. No literal `"P0 - Critical"`, `"P1 - High"`, `"P2 - Medium"`, `"P3 - Low"`, `"P4 - Backlog"`, `"P0"`, `"P1"`, `"P2"`, `"P3"`, or `"P4"` is permitted in `unblock-github` (`projects.rs` Priority entry of `REQUIRED_FIELDS`, `graphql.rs` `parse_priority_field`), `unblock-mcp` (`server.rs` `create`/`update` validators, `tools/stats.rs` `seed_priority_buckets`, `tools/list.rs` / `tools/ready.rs` / `tools/search.rs` doc-comments and fixtures), or any test fixture beyond the helpers' own unit tests.

```rust
impl Priority {
    /// All canonical `Priority` variants in declared (sort-key) order.
    pub const ALL: [Priority; 5] = [
        Priority::P0,
        Priority::P1,
        Priority::P2,
        Priority::P3,
        Priority::P4,
    ];

    /// Canonical Projects V2 single-select option name (smushed
    /// `<short_code> - <label>`). Single source of truth consumed by:
    ///  - `REQUIRED_FIELDS` in `unblock-github` (compile-time
    ///    derivation — see §5.7).
    ///  - `parse_priority_field` in `unblock-github` (round-trip
    ///    contract).
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Priority::P0 => "P0 - Critical",
            Priority::P1 => "P1 - High",
            Priority::P2 => "P2 - Medium",
            Priority::P3 => "P3 - Low",
            Priority::P4 => "P4 - Backlog",
        }
    }

    /// Canonical short code (`"P0"` .. `"P4"`). Single source of truth
    /// consumed by:
    ///  - `option_id_by_prefix` lookups in `unblock-github`
    ///    (`projects.rs`).
    ///  - The `create` and `update` tool's priority validation step
    ///    in `crates/unblock-mcp/src/server.rs`.
    ///  - `tools/stats.rs::seed_priority_buckets` (HashMap key set).
    pub const fn short_code(self) -> &'static str {
        match self {
            Priority::P0 => "P0",
            Priority::P1 => "P1",
            Priority::P2 => "P2",
            Priority::P3 => "P3",
            Priority::P4 => "P4",
        }
    }
}
```

`Display` is preserved as-is on `Priority` and emits the `short_code` (`"P0"` .. `"P4"`) — the `ready` tool's priority filter and the `tools/stats.rs::seed_priority_buckets` HashMap-key set depend on this byte-stable token set. The split between `Display` (variant identifier) and `canonical_name` (Projects V2 wire format) mirrors §2.3 (`Status::Display` / `Status::option_name`).

### 2.5 `PipelineStage`

```rust
#[non_exhaustive]
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

**API change (BREAKING — library crate `unblock-core`).** Adding `#[non_exhaustive]` to `PipelineStage` is the same forward-compat hardening as §2.4 / §2.6. Existing exhaustive `match PipelineStage { … }` arms in downstream code MUST add a wildcard `_` arm. The implementation PR for `unblock-q2x` MUST carry a `BREAKING CHANGE:` footer for the `#[non_exhaustive]` addition AND an `API:` footer for the additive helpers `PipelineStage::ALL` and `PipelineStage::canonical_name` defined below.

**Single source of truth — `PipelineStage` canonical helpers** (`unblock-core/src/types.rs`, introduced by `unblock-q2x`). Mirrors §2.3 / §2.4 / §2.6. Every layer that needs a Projects V2 PipelineStage option string MUST go through these helpers. No literal `"investigation"`, `"implementation"`, `"review"`, `"refactoring"`, `"qa"`, or `"done"` is permitted in `unblock-github` (`projects.rs` PipelineStage entry of `REQUIRED_FIELDS`, `graphql.rs` `parse_pipeline_stage_field`) or any test fixture beyond the helper's own unit tests.

```rust
impl PipelineStage {
    /// All canonical `PipelineStage` variants in declared (lifecycle) order.
    pub const ALL: [PipelineStage; 6] = [
        PipelineStage::Investigation,
        PipelineStage::Implementation,
        PipelineStage::Review,
        PipelineStage::Refactoring,
        PipelineStage::Qa,
        PipelineStage::Done,
    ];

    /// Canonical Projects V2 single-select option name (LOWERCASE,
    /// per the `setup_fields` byte-exact contract — see §5.7).
    /// Single source of truth consumed by:
    ///  - `REQUIRED_FIELDS` in `unblock-github` (compile-time
    ///    derivation — see §5.7).
    ///  - `parse_pipeline_stage_field` in `unblock-github` (round-trip
    ///    contract).
    pub const fn canonical_name(self) -> &'static str {
        match self {
            PipelineStage::Investigation => "investigation",
            PipelineStage::Implementation => "implementation",
            PipelineStage::Review        => "review",
            PipelineStage::Refactoring   => "refactoring",
            PipelineStage::Qa            => "qa",
            PipelineStage::Done          => "done",
        }
    }
}
```

`Display` on `PipelineStage` continues to emit the variant identifier (`"Investigation"`, `"Implementation"`, …) — it is NOT the wire format. For the on-the-wire option name (lowercase), use `PipelineStage::canonical_name`. This mirrors §2.3 (`Status::Display` vs `Status::option_name`).

### 2.6 `IssueType`

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueType {
    // Pre-existing (Phase 01 baseline)
    Task,
    Bug,
    Feature,
    Spike,
    // NEW (introduced by `unblock-wgj`)
    Epic,
    Chore,
    Refactor,
    Docs,
}
```

GitHub's native org-level issue type. **NOT a Projects V2 custom field.** Read from GraphQL `issueType { name }` on each issue. Epic issues serve as parent containers for sub-issues.

**Eight canonical variants (introduced by `unblock-wgj`).** The enum lists the full canonical issue-type taxonomy:

| Variant | Name | Color | Description |
|---|---|---|---|
| `Task` | `Task` | `yellow` | A specific piece of work |
| `Bug` | `Bug` | `red` | An unexpected problem or behavior |
| `Feature` | `Feature` | `blue` | A request, idea, or new functionality |
| `Spike` | `Spike` | `purple` | Time-boxed investigation or research |
| `Epic` | `Epic` | `green` | A large body of work that contains sub-issues |
| `Chore` | `Chore` | `gray` | Maintenance, cleanup, or non-feature work |
| `Refactor` | `Refactor` | `orange` | Internal restructuring with no behavior change |
| `Docs` | `Docs` | `pink` | Documentation work (README, guides, API docs) |

The Status helper precedent (§2.3) requires `#[non_exhaustive]` on this enum so future variants can be added without coordinating with downstream consumers.

**API change (BREAKING — library crate `unblock-core`).** Adding `#[non_exhaustive]` to `IssueType` is the same forward-compat hardening applied to `Status` in `unblock-1zj`. Existing exhaustive `match IssueType { ... }` arms in downstream code (currently only in-workspace) MUST add a wildcard `_` arm or a per-variant arm for new entries. Per CLAUDE.md "Coding Standards" `#[non_exhaustive]` is mandatory on growable public enums in library crates; precedent is `unblock_github::Error`, `unblock_core::DomainError`, `unblock_core::Status`, and `unblock_core::reconcile::DriftKind`. The implementation PR for `unblock-wgj` MUST carry a `BREAKING CHANGE:` footer for the `#[non_exhaustive]` addition AND an `API:` footer for the additive helpers `IssueType::canonical_name`, `IssueType::canonical_color`, `IssueType::canonical_description` defined below. The four NEW variants (`Epic`, `Chore`, `Refactor`, `Docs`) are also additive enum variants — under `#[non_exhaustive]` they MAY be added without a separate `BREAKING CHANGE:` footer (the `#[non_exhaustive]` footer covers the forward-compat envelope), but exhaustive matches in-workspace MUST be updated in the same PR.

**Single source of truth — `IssueType` canonical helpers** (`unblock-core/src/types.rs`, introduced by `unblock-wgj`). Mirrors the `Status::option_name` discipline (§2.3): every layer that needs an issue-type display name, color, or description MUST go through these helpers. No literal `"Task"`, `"feature"`, `"#1f883d"`, etc. is allowed in `unblock-github` (`projects.rs` IssueType ensure-and-heal — see §5.7), `unblock-mcp` (`tools/create.rs` validation — see §8.3 / Appendix B DRIFT-2), or any test fixture beyond the helpers' own unit tests.

```rust
impl IssueType {
    /// Canonical GitHub IssueType name (TitleCase, matches GitHub's
    /// org-level issue type display).
    /// Single source of truth consumed by:
    ///  - `REQUIRED_ISSUE_TYPES` in `unblock-github` (compile-time
    ///    derivation — see §5.7)
    ///  - the `create` tool's IssueType validation step in
    ///    `crates/unblock-mcp/src/tools/create.rs` (or `server.rs`,
    ///    whichever holds the validator — see Appendix B DRIFT-2)
    ///  - the `update` tool's IssueType validation step
    ///    (DRIFT-3 in Appendix B)
    ///  - `parse_issue_type` (or equivalent) deserialiser of the
    ///    GraphQL `issueType { name }` field
    pub const fn canonical_name(self) -> &'static str {
        match self {
            IssueType::Task     => "Task",
            IssueType::Bug      => "Bug",
            IssueType::Feature  => "Feature",
            IssueType::Spike    => "Spike",
            IssueType::Epic     => "Epic",
            IssueType::Chore    => "Chore",
            IssueType::Refactor => "Refactor",
            IssueType::Docs     => "Docs",
        }
    }

    /// Canonical color name for the issue type, used by the
    /// `setup_fields` IssueType ensure-and-heal step (§5.7) when
    /// allocating a missing org-level issue type via
    /// `POST /orgs/{org}/issue-types`. Per the GitHub REST API
    /// (<https://docs.github.com/rest/orgs/issue-types#create-issue-type-for-an-organization>),
    /// the `color` field is a lowercase string drawn from the closed
    /// set `gray`, `blue`, `green`, `yellow`, `orange`, `red`, `pink`,
    /// `purple`. Submitting the GraphQL-style uppercase form is
    /// rejected with HTTP 422 `"Invalid property /color"`. The values
    /// are stable across `setup` runs and never overwrite an existing
    /// org-side color (the ensure-and-heal step skips pre-existing
    /// types).
    pub const fn canonical_color(self) -> &'static str {
        match self {
            IssueType::Task     => "yellow",
            IssueType::Bug      => "red",
            IssueType::Feature  => "blue",
            IssueType::Spike    => "purple",
            IssueType::Epic     => "green",
            IssueType::Chore    => "gray",
            IssueType::Refactor => "orange",
            IssueType::Docs     => "pink",
        }
    }

    /// Canonical short description for the issue type, used by the
    /// `setup_fields` IssueType ensure-and-heal step (§5.7) when
    /// allocating a missing org-level issue type. Descriptions are
    /// human-readable, terse, and stable across `setup` runs.
    pub const fn canonical_description(self) -> &'static str {
        match self {
            IssueType::Task     => "A specific piece of work",
            IssueType::Bug      => "An unexpected problem or behavior",
            IssueType::Feature  => "A request, idea, or new functionality",
            IssueType::Spike    => "Time-boxed investigation or research",
            IssueType::Epic     => "A large body of work that contains sub-issues",
            IssueType::Chore    => "Maintenance, cleanup, or non-feature work",
            IssueType::Refactor => "Internal restructuring with no behavior change",
            IssueType::Docs     => "Documentation work (README, guides, API docs)",
        }
    }
}
```

**Discipline (normative).** The exact color and description values above are normative — `setup_fields` (§5.7) MUST pass these strings verbatim when creating a missing IssueType so two independent invocations of `setup` against an empty org produce byte-identical IssueType definitions. Future amendments to the color/description palette go through a spec amendment (same as `Status::option_name`); the implementation does NOT carry a duplicated literal anywhere in the workspace.

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

    // Filter 2: skip preserved states (set by agent/human, or sticky default)
    //   Backlog is sticky — issues created without an explicit transition stay
    //   in Backlog regardless of blocker state. The graph-computed
    //   Ready ↔ Blocked flip in §10.2 NEVER auto-promotes a Backlog issue.
    //   Deferred and InProgress are agent/human set and equally preserved.
    //   Closed is a degenerate guard (issue.state covers it via Filter 1, but
    //   Status==Closed without state==Closed is a drift the engine refuses to
    //   self-heal — §10.2 leaves it Closed).
    IF issue.status == Backlog:
      CONTINUE
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
- Issue not in graph: has zero blockers → ready if local-owned AND not in a preserved state (`Backlog`/`InProgress`/`Deferred`/`Closed` all skip)
- Backlog issue with zero blockers: NOT in ready set (Filter 2 skips). The agent/user must explicitly transition out of Backlog (e.g. via `update status=Ready`) before the engine treats the issue as ready candidate. Rationale: see §2.3 sticky semantics — Backlog is the create-time default and represents "not yet promoted into the active workflow", distinct from "Ready but waiting on blockers".
- Cross-repo source issue: dropped by Filter 3 regardless of blocker state — never in the ready set
- All blockers closed: every outgoing edge leads to a closed issue → ready (if local-owned AND not Backlog/preserved)
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

**`setup_fields(project_id)`** — idempotent field creation + IssueType ensure-and-heal:
1. Query existing fields
2. For each of 7 required fields: if missing, create with correct type and options
3. **IssueType ensure-and-heal (introduced by `unblock-wgj`).** GitHub's native org-level issue types are NOT a Projects V2 custom field (§2.6) and are managed via the GraphQL org-level IssueType API, not via `ProjectV2Field` mutations. `setup_fields` MUST extend its idempotent posture to also ensure all eight canonical `IssueType` variants exist on the org:
   a. Query the org's existing issue types (GraphQL `Organization.issueTypes`).
   b. For each `IssueType` variant in declared order (`Task`, `Bug`, `Feature`, `Spike`, `Epic`, `Chore`, `Refactor`, `Docs`):
      - Compute `canonical_name`, `canonical_color`, `canonical_description` from the helpers in §2.6.
      - If an existing org-level issue type whose name matches `canonical_name` (case-insensitive, byte-trim) exists: SKIP — leave it alone (color and description on the org side are user-editable and `setup` MUST NOT overwrite them).
      - If missing: create it via the GraphQL IssueType-creation mutation with the canonical name, color, and description from §2.6 verbatim.
   c. Append created entries to `SetupReport.issue_types_created` (see below).
   d. **Org-only scope.** This step is a no-op when the configured owner is a `User` rather than an `Organization` — GitHub's native issue types are an org-level feature only. `setup_fields` detects the owner type via `detect_owner_type()` (§5.4) and skips the IssueType step for user-owned repos with a single info-level log line; `SetupReport.issue_types_created` is `vec![]` for that branch.
4. Return `SetupReport { created, skipped, healed, issue_types_created }`

**`SetupReport.issue_types_created` (NEW field, additive).** `Vec<String>` of canonical IssueType names that were CREATED (not pre-existing) by step 3 above. Empty vector when all eight canonical types already existed on the org or when the owner is a `User`. Mirrors the existing `created` / `skipped` / `healed` buckets so downstream tooling (`SetupResult` in §8.10, the `unblock://setup` reporting in `tools/setup.rs`) can disclose the IssueType ensure-and-heal outcome distinctly from Projects V2 field creation. This is an additive change to the `unblock-github` pub API; the implementation PR carries an `API:` footer naming the new field.

**7 required fields with their type and options:**

| Field | Type | Options (for Single Select) |
|---|---|---|
| Status | Single Select | `Backlog`, `Ready`, `In Progress`, `Blocked`, `Deferred`, `Closed` (board order; **6 entries**, TitleCase — sourced from `Status::option_name`, §2.3) |
| Priority | Single Select | `P0 - Critical`, `P1 - High`, `P2 - Medium`, `P3 - Low`, `P4 - Backlog` |
| Pipeline Stage | Single Select | `investigation`, `implementation`, `review`, `refactoring`, `qa`, `done` |
| Agent | Text | — |
| Claimed At | Date | — |
| Story Points | Number | — |
| Defer Until | Date | — |

**Status canonical list — single source of truth.** The `Status` row above is generated from `Status::option_name` (§2.3) iterated in declaration order. The `unblock-github` crate MUST NOT carry a duplicated literal list — `REQUIRED_FIELDS`'s Status spec is built from the `Status` enum at compile time (e.g. via a `const` array of variants → `option_name`). This closes the §10.2 `compute_expected_status` contract and the §5.8 view filter against drift.

**Priority canonical list — single source of truth (`unblock-q2x`).** Same discipline as Status. `unblock-github` declares a `PRIORITY_OPTION_NAMES` constant whose entries are derived at compile time from the `Priority` enum via `Priority::canonical_name` (§2.4) — a `const` array materialised via `Priority::ALL` and the `const fn canonical_name` helper. The `REQUIRED_FIELDS` Priority entry references this constant directly. There is NO duplicated literal list of Priority option strings anywhere in the workspace. Adding a future variant to `Priority` (allowed because of `#[non_exhaustive]`, §2.4) is the single edit site — `PRIORITY_OPTION_NAMES` and `setup_fields` pick it up automatically.

**PipelineStage canonical list — single source of truth (`unblock-q2x`).** Same discipline as Status / Priority. `unblock-github` declares a `PIPELINE_STAGE_OPTION_NAMES` constant whose entries are derived at compile time from the `PipelineStage` enum via `PipelineStage::canonical_name` (§2.5) — `const` array materialised via `PipelineStage::ALL`. The `REQUIRED_FIELDS` PipelineStage entry references this constant directly. The lowercase canonical strings are byte-stable across `setup` runs and parser invocations (`parse_pipeline_stage_field`), preserving the existing live-board contract. No duplicated literal list anywhere in the workspace.

**IssueType canonical list — single source of truth (`unblock-wgj`).** Same discipline as Status / Priority / PipelineStage. `unblock-github` declares a `REQUIRED_ISSUE_TYPES` constant whose entries are derived at compile time from the `IssueType` enum via `IssueType::canonical_name`, `canonical_color`, and `canonical_description` (§2.6) — for example, a `const` array of all `IssueType` variants paired with their helper outputs. There is NO duplicated literal list of issue type names, colors, or descriptions anywhere in the workspace. The IssueType ensure-and-heal loop in step 3 above iterates this constant in declared `IssueType` order. Adding a future variant to `IssueType` (allowed because of `#[non_exhaustive]`, §2.6) is the single edit site for adding a new canonical issue type — `REQUIRED_ISSUE_TYPES` and `setup_fields` pick it up automatically without any second-list bookkeeping.

**Auto-heal semantics — case-insensitive + `snake_case` → `TitleCase` normalization** (`heal_select_field_options` at `crates/unblock-github/src/projects.rs:1077-1090`).

The current matcher reuses an existing option's GraphQL ID **only on byte-exact `name` match** against the canonical spec entry. As of `unblock-1zj` it MUST be upgraded to a normalised matcher so existing options carry their IDs through the §2.3 rename:

1. **Normalisation function (single helper, named e.g. `normalize_option_name`):**
   - Trim outer whitespace.
   - Lowercase.
   - Replace each `_` with a single space.
   - Collapse runs of internal whitespace to a single space.
   - Result: a comparable canonical key. Examples:
     - `"in_progress"` → `"in progress"`
     - `"In Progress"` → `"in progress"`
     - `"IN_PROGRESS"` → `"in progress"`
     - `"Backlog"` → `"backlog"`
     - `"ready"` → `"ready"`
2. **Matching rule (deterministic, declared-order, first-consumed).** Iterate the spec options in declared (board) order — `Backlog`, `Ready`, `In Progress`, `Blocked`, `Deferred`, `Closed`. For each spec option:
   - Compute `normalize_option_name(spec_entry)` once.
   - Scan `existing.options` (in their existing iteration order) for the FIRST entry whose `normalize_option_name(existing_name)` equals the spec key AND has not already been consumed by an earlier spec option in this loop. If found, reuse that existing option's GraphQL `id` (and color, per the existing color-preservation contract) but set `name` to the canonical TitleCase spec value (an in-place GitHub-side rename without losing the option ID), and mark that existing option consumed so a later spec option cannot match it.
   - If no normalised match is found, the spec option has no `id` — GitHub assigns a fresh ID at mutation time.
   - Existing options that remain unconsumed at the end of the loop fall through to GitHub's standard "options not in the input list get deleted" behaviour. No special handling.
3. **Result for the `unblock-1zj` migration path.** A board bootstrapped before this change carries options `[ready, in_progress, blocked, deferred, closed]` (lowercase / `snake_case`). After this matcher upgrade, running `setup` against that board:
   - Reuses all 5 existing option IDs (each normalises to its TitleCase counterpart), renaming them in place to `Ready`, `In Progress`, `Blocked`, `Deferred`, `Closed`.
   - Allocates 1 fresh option ID for the new `Backlog` entry.
   - Reports the field in the `healed` bucket of `SetupReport`.
   - **No item assignments are lost** — the rename is in-place per the GitHub `updateProjectV2Field` contract (see existing doc-comment at `projects.rs:1063-1097`).
4. **Color preservation.** The existing color-preservation rule (per `unblock-aa2` finding S1) continues to apply — the matched existing option's color forwards through the rename. The newly-allocated `Backlog` option defaults to `GRAY` per the existing rule.

This auto-heal upgrade is the only path that preserves option IDs through the `unblock-1zj` lowercase → TitleCase migration. Without it, every healed board would lose all five existing option IDs (and silently delete every item assignment to those options) on the first post-migration `setup` run — unacceptable for any project that has already adopted `unblock` pre-`unblock-1zj`.

### 5.8 View management

5 views created via REST API (`X-GitHub-Api-Version: 2026-03-10`):

| View | Layout | Filter |
|---|---|---|
| `UNBLOCK://ready` | Board | `Status:"Ready"` (TitleCase, sourced from `Status::Ready.option_name()` per §2.3) |
| `UNBLOCK://team` | Board | — (grouped by Agent) |
| `UNBLOCK://pipeline` | Board | — (grouped by Pipeline Stage) |
| `UNBLOCK://roadmap` | Table | — |
| `UNBLOCK://timeline` | Roadmap | — |

**Filter string source (normative).** The `Status:"Ready"` filter literal MUST be produced by interpolating `Status::Ready.option_name()` (§2.3) — no hand-rolled `"ready"` or `"Ready"` literal in `unblock-github::projects` view-creation code. This closes the §5.7 ↔ §5.8 round-trip: the filter string MUST byte-match an option name created by the field-setup helper, and both ends are now derived from the same `Status` helper.

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

**`state.agent_kind_str() → Option<&'static str>` (introduced by `unblock-wgj`).** Convenience accessor on `ServerState` that maps the (Phase 02) detected `AgentKind` to its canonical string display (e.g. `AgentKind::ClaudeCode → "claude-code"`, `AgentKind::Cursor → "cursor"`, etc.). Returns `None` when `agent_kind` is unset (Phase 01 default, or Phase 02 detection inconclusive). Consumed by §8.1 `claim`'s Agent precedence chain. The helper does NOT consult `config.agent` — it is purely a getter on the `agent_kind: OnceLock<AgentKind>` cell. Implementations that already carry an equivalent helper (Phase 02 code) may rename to `agent_kind_str` or expose a new alias; the spec name is normative for the §8.1 reference.

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
    pub agent: Option<String>,  // see Agent precedence below
}

pub struct ClaimResult {
    pub issue: IssueSummary,
}
```

**Validation:**
- `id`: positive integer
- `agent`: non-empty if present

**Agent precedence (introduced by `unblock-wgj`).** The Agent value written to the Projects V2 Agent field (and interpolated into the claim comment in step 4 below) follows this precedence chain, evaluated at handler entry:

1. **`params.agent` is `Some(name)`** (and validates non-empty) — use `name` verbatim. The caller's explicit choice always wins; there is NO mechanism for the caller to suppress an `agent_kind_str()` default once they have provided an explicit agent.
2. **`params.agent` is `None` AND `state.agent_kind_str()` returns `Some(kind)`** — use `kind` (the Phase 02 detected agent kind from `ServerState.agent_kind`, §6.1). Common case: the MCP client identifies as a known agent (e.g. `"claude-code"`, `"cursor"`) and the caller did not bother to pass an explicit `agent`.
3. **`params.agent` is `None` AND `state.agent_kind_str()` returns `None`** — leave the Agent field EMPTY (do NOT write the field, do NOT substitute `config.agent` or any other fallback). The claim still proceeds; the issue is claimed (Status → InProgress, Claimed At → now) but the Agent field is left blank for the user/admin to fill in. The claim comment in step 4 renders as `"Claimed at {timestamp}"` (no `by {agent}` substring) when the Agent is empty.

This supersedes the prior "defaults to `config.agent`" comment in `ClaimParams`; `config.agent` is no longer consulted by `claim`'s default chain (§12 retains the `UNBLOCK_AGENT` env var for legacy / test reasons but it does not feed into `claim` precedence). Tests cover edge cases by running with and without `agent_kind` set on `state`.

**Flow:**
1. Fetch issue (single query, always fresh)
2. Validate:
   a. `IssueState == Open` → else `IssueClosed`
   b. `Status != InProgress` → else `AlreadyClaimed`
   c. Not blocked: check graph → else `IssueBlocked { blockers }`
   d. Not deferred: `defer_until <= today` → else `IssueDeferred`
3. Resolve effective Agent value via the precedence chain above. Update fields: Status → `Status::InProgress.option_name()` (= `"In Progress"`, TitleCase per §2.3 — supersedes the prior `in_progress` lowercase literal), Agent → effective value (or SKIP the Agent field update if the precedence chain resolved to "leave empty"), Claimed At → now. Note: `claim` is one of the explicit transitions OUT of Backlog — there is no Backlog-stickiness skip here, by design.
4. Add comment: `"Claimed by {agent} at {timestamp}"` if effective Agent is non-empty, else `"Claimed at {timestamp}"` (no `by {agent}` substring).
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
4. Update fields: Status → `Status::Closed.option_name()` (= `"Closed"`, TitleCase per §2.3)
5. Add comment: `"Closed: {reason}"` (or `"Closed"` if no reason)
6. For each unblocked (from step 2 — cascade list captured PRE-close):
   a. **If the dependent's current `status == Backlog`: SKIP the Status update.** Backlog is sticky (§2.3, §3.3 Filter 2, §10.2) — a graph-driven cascade does NOT promote a Backlog issue out of Backlog. Still emit the unblock comment (step 6.b).
   b. Otherwise: Update Status → `Status::Ready.option_name()` (= `"Ready"`, TitleCase per §2.3).
   c. Add comment: `"Unblocked — blocker #{id} was closed"` (always, regardless of Status update).
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
- `issue_type`: valid IssueType name if present (case-insensitive + byte-trim per §5.7 normaliser, routed through `IssueType::canonical_name`)
- `defer_until`: valid ISO date if present

**Create-time defaults (introduced by `unblock-wgj`).** The handler MUST resolve every default-bearing Projects V2 field via a deterministic precedence step BEFORE issuing any field write — defaults are computed once at handler entry and the field-write loop in step 4 below is a pure function of the resolved values. Precedence is `explicit caller param (Some) > canonical default > omit-empty`:

| Field | If `params.<x>` is `Some(v)` | If `params.<x>` is `None` | Notes |
|---|---|---|---|
| `Status` | (no param — server-managed) | `Status::Backlog` (canonical default per §2.3 sticky semantics) | Set unconditionally. Backlog is sticky and §10.2 will not promote it on rebuild — see step 5 below. |
| `Priority` | use `v` (validated P0–P4) | `Priority::P2` (canonical default = Medium) | Set unconditionally. P2 is the canonical "Medium" default and matches the `P2 - Medium` Projects V2 option (§5.7). |
| `Agent` | use `v` (validated non-empty) | `state.agent_kind_str()` if `Some(kind)`, else **omit** | Same precedence chain as `claim` (§8.1) — explicit param > detected agent kind > leave Agent field empty (no field write). `config.agent` is NOT consulted. |
| `IssueType` | use `IssueType::canonical_name`-validated value | `IssueType::Task` (canonical default) | Routes through the GitHub native `issueType` REST/GraphQL parameter (NOT a Projects V2 custom field per §2.6). |
| `Story Points` | use `v` | omit (no field write) | No canonical default — leave unset. |
| `Defer Until` | use `v` (validated ISO date) | omit (no field write) | No canonical default — leave unset. |

The "omit" outcome means the field-update mutation for that field is SKIPPED entirely (the field stays unset on the new issue), distinct from writing an empty string. This mirrors the §8.1 Agent precedence "skip the field update" semantics.

**Flow:**
1. **Resolve create-time defaults (NEW step, deterministic, no I/O).** Apply the precedence table above to compute the effective `(status, priority, agent, issue_type, story_points, defer_until)` tuple. This step is a pure function of `params` + `state` and runs BEFORE any GitHub call. Validation (per "Validation" above) runs as part of this resolution — `priority`/`issue_type`/`defer_until` validation rejects bad values with `DomainError::Validation` before any mutation.
2. Create issue: REST POST. The `issueType` is sent as part of the create payload (resolved from step 1, default `Task`).
3. Add to project: `addProjectV2Item`.
4. Set fields (Projects V2): write Priority, Status, Story Points, Defer Until, Agent per the resolved tuple from step 1. **Omit-empty rule:** fields whose resolved value is "omit" (Story Points / Defer Until when params absent; Agent when both `params.agent` and `agent_kind_str()` are `None`) MUST NOT issue a field-update mutation. Fields with canonical defaults (Status, Priority) ARE always written (so the new project item shows the correct option on the board).
5. If `blocked_by`:
   a. For each blocker: resolve IssueRef, `would_create_cycle` check, `add_blocked_by`.
   b. **Status remains `Backlog`** — adding blockers to a freshly-created issue does NOT auto-flip it to `Blocked`, because Backlog is sticky (§2.3, §3.3 Filter 2, §10.2). The blockers are recorded; the next explicit transition out of Backlog will land the issue in `Ready` or `Blocked` per the graph at that time.
6. If `parent`: resolve IssueRef, `add_sub_issue`.
7. If `labels`: `ensure_labels` (auto-create missing) + `add_labels_to_issue`.
8. If `milestone`: resolve milestone by title, `update_issue_milestone`.
9. Invalidate cache + rebuild.

**Note on prior posture.** The pre-`unblock-1zj` default of Status → `Ready` (or `Blocked` if has blockers) is REMOVED — `unblock-1zj` makes Backlog the universal create-time default. The user/agent must explicitly transition out of Backlog (via `update status=Ready`, `claim`, etc.) for the issue to participate in the ready set or the graph-computed Ready ↔ Blocked flip. The `unblock-wgj` amendment formalises the precedence table above — Priority/Agent/IssueType were under-specified before and are now normative.

**API calls:** 1 (create) + 1 (add to project) + 2-5 (fields — Status + Priority always; Agent/Story Points/Defer Until only when resolved-non-omit) + 0-N (deps) + 0-1 (parent) + 1+ (rebuild)
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
5. **If source's current `status == Backlog`: SKIP the Status update.** Backlog is sticky (§2.3) and adding a blocker MUST NOT auto-promote out of Backlog. Otherwise: update source Status → `Status::Blocked.option_name()` (= `"Blocked"`, TitleCase per §2.3).
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
    pub issue_type: Option<String>,            // NEW (unblock-wgj) — see §2.6 + Appendix B DRIFT-3
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
- `status`: valid Status variant if present (case-insensitive + byte-trim, routed through `Status::option_name`)
- `defer_until`: valid ISO date if present
- `agent`: non-empty if present (validation only — no precedence chain; the value is used verbatim, see "Agent + IssueType absence-leaves-unmodified rule" below)
- `issue_type`: must match a `REQUIRED_ISSUE_TYPES` name when present (case-insensitive + byte-trim per §5.7 normaliser, routed through `IssueType::canonical_name`). Rejected with `DomainError::Validation` otherwise. The eight canonical names are `Task`, `Bug`, `Feature`, `Spike`, `Epic`, `Chore`, `Refactor`, `Docs` (§2.6).
- `body` and body section params (`description`, `design_notes`, `acceptance_criteria`) are **mutually exclusive**. If `body` is provided, section-level params MUST be absent — validation rejects the call otherwise. `body` replaces the entire issue body; section params merge into the existing body via §9.3

**Agent + IssueType absence-leaves-unmodified rule (introduced by `unblock-wgj`, DRIFT-3 closure).** The `agent` and `issue_type` params are both `Option<String>` and follow a uniform precedence rule that is INTENTIONALLY DIFFERENT from `claim` (§8.1) and `create` (§8.3):

| Param | If `params.<x>` is `Some(v)` | If `params.<x>` is `None` |
|---|---|---|
| `agent` | use `v` verbatim — write the Agent Projects V2 field | leave the Agent field UNMODIFIED (no field write) |
| `issue_type` | use `v` (validated via `IssueType::canonical_name`) — issue the GitHub native `issueType` mutation | leave the IssueType UNMODIFIED (no mutation) |

Rationale: `update` is an explicit-edit tool. Unlike `claim` (which has a defaulting precedence chain to fill in Agent on a fresh claim), `update` only touches a field when the caller explicitly opts in. There is NO fallback to `state.agent_kind_str()` or any canonical default in `update` — absence means "do not touch this field". This preserves the principle of least surprise: a caller who passes `update(id=42, title="…")` must NOT see Agent or IssueType change as a side effect.

**Flow:**
1. Fetch issue, validate not closed (unless reopening via status).
2. If body sections changed: parse existing body, merge sections (§9.3), write back.
3. If REST fields changed (title, body, labels, assignees, milestone): PATCH issue.
4. If `issue_type` is `Some`: issue the GitHub native IssueType update mutation (uses the `IssueType::canonical_name`-validated value from validation). This is NOT a Projects V2 field — it routes through GitHub's native `issueType` API per §2.6.
5. If Project fields changed (status, priority, agent, story_points, defer_until): `set_project_fields`. The `agent` field is included iff `params.agent` is `Some`; absence skips the Agent write per the rule above.
6. Invalidate cache + rebuild.

**Reflection in `UpdateResult.updated_fields`.** Each field that was actually written (REST patch, Projects V2 update, or IssueType mutation) appends a stable canonical token to `updated_fields` in the `key=value` shape — for example `"status=Ready"`, `"priority=P2"`, `"agent=ada"`, `"issue_type=Bug"`, `"story_points=3"`, `"defer_until=2026-06-01"`, `"milestone=v1.0"`, `"labels_add=bug,refactor"`, `"labels_remove=stale"`, `"assignees_add=alice"`, `"assignees_remove=bob"`, `"body_section=Acceptance"`. Fields whose param was `None` and therefore left unmodified MUST NOT appear in `updated_fields`. The `key=value` shape mirrors the existing token vocabulary used by the sibling tools, so tooling that consumes `UpdateResult.updated_fields` can split on `=` to extract per-key write outcomes uniformly. This lets the caller assert post-conditions without re-fetching the issue.

**API calls:** 1 (fetch) + 0-1 (PATCH REST) + 0-1 (IssueType mutation) + 0-N (Projects V2 field updates) + 1+ (rebuild)
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
4. If issue has open blockers: Status → `Status::Blocked.option_name()` (= `"Blocked"`, TitleCase per §2.3)
5. If no open blockers: Status → `Status::Ready.option_name()` (= `"Ready"`, TitleCase per §2.3)
6. Update cache

**Reopen never restores `Backlog`.** A closed issue that was previously in Backlog and then closed comes back as either `Ready` or `Blocked` per the graph, NOT Backlog. Rationale: Backlog represents "not yet promoted into the active workflow"; once an issue has been closed it has by definition left that pre-workflow state, and reopening puts it back in the active Ready/Blocked rotation. This is consistent with §2.3 "reopened issues do NOT go back to Backlog".

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
    pub issue_types_created: Vec<String>,  // NEW (unblock-wgj) — see §5.7
    pub migrated_count: Option<usize>,
}
```

**Flow:**
1. Resolve project (param or auto-detect)
2. Query existing fields
3. Create 7 missing fields (skip existing) — idempotent
4. Detect owner type (org vs user)
5. **IssueType ensure-and-heal (introduced by `unblock-wgj`, §5.7 step 3).** If owner is `Organization`: query org's existing issue types, create the missing canonical eight (`Task`, `Bug`, `Feature`, `Spike`, `Epic`, `Chore`, `Refactor`, `Docs`) using `IssueType::canonical_name`/`canonical_color`/`canonical_description`. Names matched case-insensitively + byte-trim; existing types are SKIPPED (color/description on the org side are user-editable and `setup` does NOT overwrite). If owner is `User`: skip with info-level log line. Outcome surfaced in `SetupResult.issue_types_created` (canonical names of types created in this run; empty when all eight already existed or owner is User).
6. Query existing views (GraphQL)
7. Discover field IDs (REST GET /fields — integer IDs)
8. Create 5 missing views (REST POST /views) — idempotent
9. If `migrate`: add existing open issues to project, set default field values
10. Return report

**Idempotent:** safe to run multiple times. Skips existing fields, views, and issue types.
**API calls:** 1 (field query) + 0-7 (create fields) + 1 (org issue types query) + 0-8 (create issue types) + 1 (views query) + 1 (REST fields) + 0-5 (create views) + 0-N (migrate)

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

  // Preserved states — set by agent/human (or sticky default),
  // never overridden by server. Backlog is the create-time default and
  // sticky — issues stay in Backlog until an explicit user/agent transition
  // (e.g. `update status=Ready`, `claim`, etc.) moves them out. The server
  // NEVER auto-promotes Backlog → Ready/Blocked from a graph rebuild.
  IF issue.status == Backlog:
    RETURN Backlog
  IF issue.status == InProgress:
    RETURN InProgress
  IF issue.status == Deferred:
    RETURN Deferred

  // Graph-computed states (only applies to issues that have ALREADY left
  // Backlog — i.e. status is currently Ready or Blocked).
  IF issue.qualified_id IN ready_set:
    RETURN Ready
  RETURN Blocked
```

**Sticky-Backlog invariant.** §3.3 Filter 2 already excludes `Backlog` issues from the `ready_set`, so the `IN ready_set` branch above will never fire for a Backlog issue — the explicit `IF issue.status == Backlog → Backlog` short-circuit is defensive and documents the contract. The two filters MUST stay in lock-step: removing one without the other (e.g. dropping Filter 2's Backlog skip but keeping the §10.2 Backlog short-circuit) would leave Backlog issues in the ready_set projection while the engine refused to update their Status — creating exactly the silent drift §14 Invariant 13 forbids.

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
8. **Backlog stickiness (§2.3, §3.3 Filter 2, §10.2; introduced by `unblock-1zj`).** Two clauses:
   a. For any input where every issue has `status == Backlog` and `state == Open`, `compute_ready_set(...)` returns the empty vector — Filter 2 skips Backlog regardless of blocker state.
   b. For any input issue with `status == Backlog` and `state == Open`, `compute_expected_status(issue, ready_set)` returns `Backlog` regardless of `ready_set` membership — the §10.2 short-circuit prevents server-side auto-promotion.
9. **Status helper round-trip (§2.3; introduced by `unblock-1zj`).** For every `Status` variant `s`, parsing `s.option_name()` back through `parse_status_field` yields `s`. The TitleCase canonical strings are the only wire-format Status values produced or consumed by the system.
10. **IssueType helper round-trip (§2.6; introduced by `unblock-wgj`).** For every `IssueType` variant `t`, parsing `t.canonical_name()` back through the IssueType deserialiser (case-insensitive + byte-trim per the §5.7 ensure-and-heal matcher) yields `t`. `canonical_color(t)` and `canonical_description(t)` are non-empty for every variant. The canonical strings are the only wire-format IssueType values produced by `setup_fields` and consumed by `create`'s validation step.
11. **Agent precedence (§8.1; introduced by `unblock-wgj`).** Three branches MUST be covered by integration tests over `claim`: (a) `params.agent = Some("alice")` always produces Agent="alice" regardless of `state.agent_kind_str()`; (b) `params.agent = None` AND `state.agent_kind_str() = Some("claude-code")` produces Agent="claude-code"; (c) `params.agent = None` AND `state.agent_kind_str() = None` produces an empty Agent field (no field update issued for Agent) AND a comment of the form `"Claimed at {timestamp}"` with NO `by {agent}` substring.

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
15. **Backlog stickiness (§2.3, §3.3 Filter 2, §10.2; introduced by `unblock-1zj`).** Two clauses, both MUST hold:
    - **15(a) — Ready-set exclusion.** `compute_ready_set` (§3.3) NEVER emits an issue whose `status == Backlog`. Filter 2 is the single chokepoint.
    - **15(b) — No server-side promotion.** `compute_expected_status` (§10.2) returns `Backlog` for any input issue with `status == Backlog` and `state == Open`, regardless of `ready_set` membership. The server NEVER auto-flips Backlog → Ready/Blocked from a graph rebuild; only explicit user/agent transitions (e.g. `update`, `claim`) move issues out of Backlog. Property tests MUST cover both clauses.
16. **Status helper is the single source of truth (§2.3; introduced by `unblock-1zj`).** Every Projects V2 Status string produced by the system (REQUIRED_FIELDS spec list, view filters, field-update mutations, comments referencing Status names) is sourced from `Status::option_name`. `parse_status_field(Status::X.option_name())` returns `Status::X` for every variant `X`. No literal `"Ready"`, `"In Progress"`, `"ready"`, `"in_progress"`, `"Done"`, or `"Todo"` exists outside `Status::option_name`'s definition site and its unit tests. Enforced by a CI grep guard added in the `unblock-1zj` PR.
17. **IssueType helpers are the single source of truth (§2.6; introduced by `unblock-wgj`).** Every IssueType name, color, and description string produced by the system (`REQUIRED_ISSUE_TYPES` in `unblock-github`, `setup_fields` ensure-and-heal step, `create` tool validation, GraphQL deserialisers) is sourced from `IssueType::canonical_name` / `canonical_color` / `canonical_description`. The `unblock-github` crate MUST NOT carry a duplicated literal list of issue type names, colors, or descriptions — `REQUIRED_ISSUE_TYPES` is generated from the `IssueType` enum at compile time. Adding a future `IssueType` variant (allowed by `#[non_exhaustive]`, §2.6) is the single edit site for adding a new canonical issue type. Enforced by a CI grep guard mirrored on the `unblock-1zj` Status discipline.
18. **Agent precedence chain (§8.1; introduced by `unblock-wgj`).** The `claim` tool resolves the effective Agent value via the three-step chain in §8.1: `params.agent` (explicit caller choice) → `state.agent_kind_str()` (detected agent kind) → empty (no fallback to `config.agent`). The chain is monotonic: (a) explicit caller `Some(name)` always wins; (b) `None` caller falls through to `agent_kind_str()` ONLY when `agent_kind_str()` returns `Some(_)`; (c) `None` from both yields an EMPTY Agent field (the field update is SKIPPED, not written as empty string). The claim comment renders `"Claimed by {agent} at {timestamp}"` when Agent is non-empty and `"Claimed at {timestamp}"` (no `by {agent}` substring) when Agent is empty. No other tool consults this chain — `config.agent` is no longer consulted by `claim` (§12 retains `UNBLOCK_AGENT` for legacy/test reasons but it does not feed into `claim` precedence).

---

## Appendix A — `unblock-1zj` Amendment Notes

This appendix tracks the spec amendment delivered in bead `unblock-1zj` (2026-04-30) so the implementation supervisor sees the full scope of the PR in one place.

### A.1 Decisions captured (user-approved)

1. **Auto-heal matcher upgrade (Decision 1 / §5.7).** `heal_select_field_options` matches existing option names case-insensitively and tolerates `snake_case` ↔ `TitleCase` so the `unblock-1zj` rename preserves option IDs.
2. **Backlog is sticky (Decision 2 / §2.3 + §3.3 Filter 2 + §10.2 + §8.3 + §8.4 + §8.2).** Backlog is the create-time default and is preserved by the engine — only explicit user/agent transitions move an issue out of Backlog. The Status canonical option list grows from 5 to 6 entries: `Backlog`, `Ready`, `In Progress`, `Blocked`, `Deferred`, `Closed` (board order).
3. **Centralised Status name helper (Decision 3 / §2.3).** `Status::option_name` in `unblock-core/src/types.rs` is the single source of truth for the TitleCase wire-format strings. The duplicate `status_slug` helper at `crates/unblock-mcp/src/tools/stats.rs:147-161` and every literal in `server.rs`, `reopen.rs`, `dep_remove.rs`, `setup.rs`, and `parse_status_field` route through it. The `unblock-github` `REQUIRED_FIELDS` Status spec is generated from the `Status` enum (no duplicated literal list).

### A.2 Drifts to close in the same PR

The implementation supervisor MUST land both of these in the same commit / PR as the §2.3 / §5.7 / §10.2 spec changes — the spec amendment is not coherent without them.

| Drift | Location | Fix |
|---|---|---|
| **DRIFT-1 (plan)** | `docs/plans/01-plan-mcp-foundation.md` GAP-07 (lines 716-724) and GAP-10 (lines 761-768) | Mark RESOLVED. GAP-07's `closed`/`Done` and `ready`/`Backlog` divergence is replaced by the canonical TitleCase 6-entry list sourced from `Status::option_name`; GAP-10's "needs verification" Status option set is now fully specified by §5.7 + §2.3. Both gaps collapse into Decisions 2 + 3 above. |
| **DRIFT-2 (code)** | `crates/unblock-mcp/src/tools/update.rs:50` doc-comment for `pub status: Option<String>` | Currently reads `"New status: Backlog, In Progress, Done, Blocked, Deferred."` — replace with the canonical 6-entry TitleCase list `"New status: Backlog, Ready, In Progress, Blocked, Deferred, Closed."` matching `Status::option_name` for every variant. The legacy `Done` token is removed (it was a residue of the GitHub-default `[Todo, In Progress, Done]` field; `unblock` has never used `Done` as a Status value). |

### A.3 Test obligations introduced

In addition to existing coverage, the implementation PR MUST add:

1. A `unblock-core` unit test asserting `Status::option_name` returns the exact TitleCase canonical string for every variant (round-trip via `parse_status_field` covered too — §14 Invariant 16, mirrored in §13.3 graph-invariant 9).
2. Two `unblock-core` property tests for §14 Invariant 15 (Backlog stickiness — clauses 15(a) and 15(b)), mirrored in §13.3 graph-invariant 8.
3. A `unblock-github` integration test (via `MockGitHubClient` or schema fixture) for the auto-heal normaliser: input field with options `[ready, in_progress, blocked, deferred, closed]` heals to `[Backlog, Ready, In Progress, Blocked, Deferred, Closed]` with the 5 existing option IDs preserved through the rename, 1 fresh ID for `Backlog`, and the field marked `healed` in `SetupReport`.
4. A `unblock-mcp` integration test asserting `create` lands a fresh issue in `Status::Backlog` (no auto-promotion to `Ready`/`Blocked` even with blockers).
5. A `unblock-mcp` integration test asserting the `close` cascade SKIPS the Status update for a dependent currently in `Backlog` while still emitting the unblock comment.

### A.4 Scope NOT covered by this amendment

This amendment is deliberately confined to the three user-decided design choices and the two named drifts. Items NOT in scope and therefore NOT changed by `unblock-1zj`:

- The `IssueRef`-typed cross-repo error variants (already specified in §11.1 by `unblock-eos`).
- The §11.4 cross-repo response contract (already complete).
- The widened `fetch_graph_data` `states: [OPEN, CLOSED]` query (already shipped as `unblock-a36`).
- Any new Phase 02 features (reconcile, doctor, circuit breaker — explicitly out of scope per §1.2).

---

## Appendix B — `unblock-wgj` Amendment Notes

This appendix tracks the spec amendment delivered in bead `unblock-wgj` (2026-04-30) so the implementation supervisor sees the full scope of the PR in one place. Sherlock's investigation report (bd comments on `unblock-wgj`) carries the precise file paths the supervisor will use.

### B.1 Decisions captured (user-approved, 2026-04-30)

1. **`IssueType` is `#[non_exhaustive]` + canonical helpers + 8-variant taxonomy (Decision 1 / §2.6).** `IssueType` carries `#[non_exhaustive]` (BREAKING CHANGE on `unblock-core`), grows from 4 to **8 canonical variants** (`Task`, `Bug`, `Feature`, `Spike` pre-existing; `Epic`, `Chore`, `Refactor`, `Docs` NEW), and grows three `pub const fn` helpers — `canonical_name`, `canonical_color`, `canonical_description` — defined on the enum in `unblock-core/src/types.rs`. Same precedent as `Status::option_name` from `unblock-1zj` (§2.3). Color palette (lowercase per GitHub REST `POST /orgs/{org}/issue-types`): Task=yellow, Bug=red, Feature=blue, Spike=purple, Epic=green, Chore=gray, Refactor=orange, Docs=pink.
2. **`REQUIRED_ISSUE_TYPES` derived at compile time (Decision 2 / §5.7).** `unblock-github` declares `REQUIRED_ISSUE_TYPES` as a `const` array derived from the `IssueType` enum via the §2.6 helpers. NO duplicated literal list of issue type names, colors, or descriptions exists anywhere in the workspace. Adding a new `IssueType` variant is the single edit site for adding a canonical issue type.
3. **`setup_fields` ensures + heals org-level IssueTypes (Decision 3 / §5.7 + §8.10).** `setup_fields` extends its idempotent posture to also ensure all eight canonical `IssueType` variants exist on the org (case-insensitive name match, byte-trim, SKIP existing types — color/description on the org side are user-editable and `setup` MUST NOT overwrite). Outcome surfaced via NEW `SetupReport.issue_types_created: Vec<String>` field (additive — `API:` footer in commit body) and propagated to `SetupResult.issue_types_created` in §8.10. Org-only scope: no-op for User-owned repos with an info-level log line.
4. **`claim` Agent precedence chain (Decision 4 / §8.1).** `params.agent: Option<String>` where `None` means "use default = `state.agent_kind_str()` if present, else leave Agent empty". Three branches: explicit caller choice always wins; `None` caller falls through to `agent_kind_str()`; `None` from both yields an EMPTY Agent field (the field update is SKIPPED). `config.agent` is no longer consulted by `claim` precedence. Claim comment renders `"Claimed by {agent} at {timestamp}"` when Agent is non-empty, `"Claimed at {timestamp}"` (no `by {agent}` substring) when empty. Tests cover edge cases by running with and without `agent_kind` set on `state`.
5. **`create` create-time defaults precedence table (Decision 5 / §8.3).** Status=Backlog (sticky default per §2.3), Priority=P2 (canonical Medium default), IssueType=Task (canonical default), Agent follows the §8.1 precedence chain (`params.agent` > `state.agent_kind_str()` > omit). Defaults resolution is a deterministic step BEFORE any field write — see the §8.3 normative precedence table.
6. **`update` agent + issue_type params with absence-leaves-unmodified semantics (Decision 6 / §8.6, DRIFT-3 closure).** `agent: Option<String>` and `issue_type: Option<String>` on `UpdateParams`. Both follow a uniform "explicit param flows through unchanged; absence leaves field unmodified" rule. `issue_type` validation matches `IssueType::canonical_name` (case-insensitive + byte-trim) and is rejected with `DomainError::Validation` when the value isn't in `REQUIRED_ISSUE_TYPES`. INTENTIONALLY DIFFERENT from `claim` (§8.1) and `create` (§8.3) — `update` never falls back to `state.agent_kind_str()` or any canonical default.

### B.2 Drifts to close in the same PR

The implementation supervisor MUST land all three of these in the same commit / PR as the §2.6 / §5.7 / §8.1 / §8.3 / §8.6 / §8.10 spec changes — the spec amendment is not coherent without them.

| Drift | Location | Fix |
|---|---|---|
| **DRIFT-1 (plan)** | `docs/plans/01-plan-mcp-foundation.md` GAP-03 (lines ~660-675) and Task 03.05 (line ~441) | Update to reflect Decisions 1–3 above. GAP-03 references `IssueType` as an EXTRA Projects V2 field that was removed; Task 03.05 enumerates the seven Projects V2 custom fields. Neither captures the org-level IssueType ensure-and-heal that `setup_fields` now performs. The plan patch tracks this work via a new GAP entry (see §B.5) and adds an explicit acceptance bullet to Task 03.05. |
| **DRIFT-2 (code)** | The `create` tool's IssueType validation step in `crates/unblock-mcp/src/tools/create.rs` (or `server.rs`, whichever holds the validator — Sherlock's investigation report on `unblock-wgj` carries the precise file path) | Currently validates `issue_type` against a literal list (or accepts any string). Replace with validation routed through `IssueType::canonical_name` (case-insensitive + byte-trim per the §5.7 normaliser) → `Result<IssueType, DomainError::Validation>`. The implementation supervisor lands the fix at the exact site identified in Sherlock's report; the validator name is generic in this spec because the precise function signature/location is captured in the bd comments on `unblock-wgj`. |
| **DRIFT-3 (code)** | The `update` tool params + handler in `crates/unblock-mcp/src/tools/update.rs` (and the equivalent `GitHubApi` mutation surface for issue-type changes) | `UpdateParams` currently lacks an `issue_type` field, and the existing `agent` field has no normative absence-semantics. Add `pub issue_type: Option<String>` per §8.6 and document the uniform "explicit param flows through unchanged; absence leaves field unmodified" rule for both `agent` and `issue_type`. `issue_type` validation MUST route through `IssueType::canonical_name` (case-insensitive + byte-trim per the §5.7 normaliser) and reject unknown names with `DomainError::Validation`. Wire a GitHub native IssueType update mutation (NOT a Projects V2 field write) — this is the additional `GitHubApi` surface the implementation supervisor must add or extend. The `update` flow MUST emit `"agent"` and/or `"issue_type"` tokens in `UpdateResult.updated_fields` only when the corresponding param was `Some`. |

### B.3 Test obligations introduced

In addition to existing coverage, the implementation PR MUST add:

1. A `unblock-core` unit test asserting `IssueType::canonical_name`, `canonical_color`, and `canonical_description` return the exact canonical strings for every variant (round-trip via the IssueType deserialiser covered too — §14 Invariant 17, mirrored in §13.3 graph-invariant 10). The test MUST cover all 8 variants (`Task`, `Bug`, `Feature`, `Spike`, `Epic`, `Chore`, `Refactor`, `Docs`) and assert the §2.6 color/description palette byte-for-byte.
2. A `unblock-github` integration test (via `MockGitHubClient` or schema fixture) for the `setup_fields` IssueType ensure-and-heal step:
   - Org with three pre-existing types (`task`, `BUG`, `Feature`) → `setup` matches them case-insensitively, leaves them alone, and creates the missing five (`Spike`, `Epic`, `Chore`, `Refactor`, `Docs`) using the §2.6 canonical color/description verbatim. `SetupReport.issue_types_created` contains exactly `["Spike", "Epic", "Chore", "Refactor", "Docs"]` in declared order.
   - User-owned repo → step is a no-op, `SetupReport.issue_types_created` is `vec![]`, info-level log line emitted.
3. A `unblock-mcp` integration test asserting the `create` tool rejects an unknown `issue_type` with `DomainError::Validation` and accepts canonical names case-insensitively (e.g. `"task"`, `"TASK"`, `"Task"` all resolve to `IssueType::Task`; `"docs"`, `"Docs"`, `"DOCS"` all resolve to `IssueType::Docs`).
4. Three `unblock-mcp` integration tests for the §8.1 Agent precedence chain (§14 Invariant 18, mirrored in §13.3 graph-invariant 11):
   - `params.agent = Some("alice")` AND `state.agent_kind_str() = Some("claude-code")` → Agent="alice", comment includes `"by alice"`.
   - `params.agent = None` AND `state.agent_kind_str() = Some("claude-code")` → Agent="claude-code", comment includes `"by claude-code"`.
   - `params.agent = None` AND `state.agent_kind_str() = None` → Agent field update SKIPPED, comment is `"Claimed at {timestamp}"` (no `by` substring).
5. **`update` tool: agent + issue_type absence semantics (DRIFT-3 closure, §8.6).** Six `unblock-mcp` integration tests:
   - `params.agent = Some("alice")` → Agent field WRITTEN to `"alice"`; `UpdateResult.updated_fields` contains `"agent"`.
   - `params.agent = None` (other params populate the call) → Agent field LEFT UNMODIFIED (no field-update mutation issued for Agent); `UpdateResult.updated_fields` does NOT contain `"agent"`.
   - `params.issue_type = Some("Docs")` → IssueType native mutation issued for `Docs`; `UpdateResult.updated_fields` contains `"issue_type"`.
   - `params.issue_type = Some("docs")` → same outcome (case-insensitive resolution to `IssueType::Docs`).
   - `params.issue_type = Some("NotAType")` → rejected with `DomainError::Validation`; no mutation issued; cache NOT invalidated.
   - `params.issue_type = None` (other params populate the call) → IssueType LEFT UNMODIFIED; `UpdateResult.updated_fields` does NOT contain `"issue_type"`.
6. **Live test fixture rewrite (introduced by `unblock-wgj`).** The implementation PR MUST replace existing synthetic placeholder titles in live integration test fixtures (e.g. `"[test] remove_blocked_by issue A"`, `"[test] dep_remove issue B"`, and any other `[test]`-prefixed scaffolding) with realistic scenario titles that exercise the full 8-variant taxonomy, the create-time defaults precedence (§8.3), and the Agent absence chain (§8.1 / §8.6). The rewrite MUST cover at least the following fixtures:

   | Title | IssueType | Priority | Notes |
   |---|---|---|---|
   | `Fix authentication bypass in /login endpoint` | Bug | P0 | High-severity bug; exercises `Bug` + `P0`. |
   | `Migrate auth middleware to async` | Refactor | P1 | Exercises NEW `Refactor` variant. |
   | `Investigate flaky checkout test` | Spike | P2 | Exercises canonical Priority default (P2). |
   | `Document Projects V2 setup workflow` | Docs | P3 | Exercises NEW `Docs` variant. |
   | `Bump dependency versions` | Chore | P4 | Exercises NEW `Chore` variant. |
   | `Implement OAuth login flow` | Feature | P1 | **Epic parent** — paired with two sub-issues below. |
   | `Add OAuth callback handler` | Task | P2 | Sub-issue of Epic parent (`add_sub_issue`). |
   | `Add OAuth token validation` | Task | P2 | Sub-issue of Epic parent (`add_sub_issue`). |

   **Required coverage clauses (normative):**
   - **Epic + Task hierarchy.** At least one fixture pair MUST model the Epic + sub-Task hierarchy via `add_sub_issue`. The "Implement OAuth login flow" fixture above is the canonical exemplar — it carries `IssueType::Epic` (or `Feature` per the choice in the table) and at least two `IssueType::Task` children. The fixture rewrite test MUST assert the parent/child relationship round-trips through the API.
   - **Agent omitted post-creation (edge case for the §8.1 / §8.3 / §8.6 precedence chain).** At least one fixture MUST be CREATED and persisted WITHOUT setting Agent — i.e. the `create` call has `params.agent = None` AND `state.agent_kind_str()` returns `None`, exercising the "omit" leg of the §8.3 precedence table. The fixture's `Issue.agent` MUST round-trip as `None` and a follow-up `update` call WITHOUT `agent` MUST leave it `None` (DRIFT-3 absence-leaves-unmodified test).
   - **All 8 IssueType variants exercised.** The combined fixture set MUST exercise every variant in the §2.6 enum at least once across the integration test corpus — no variant is allowed to be untouched.

   The rewrite is a code change in test files (not a spec change); it is listed here as a normative test obligation so the implementation supervisor cannot land DRIFT-1/2/3 closures without also rewriting the fixtures. Synthetic `[test]`-prefixed titles are explicitly prohibited going forward.

### B.4 README change (note for implementation supervisor)

The `README.md` token-scopes section MUST be updated in the same PR as the spec amendment to document the GraphQL scopes required by the new `setup_fields` IssueType ensure-and-heal step:

- Reading org-level issue types requires `read:org` (already documented).
- Creating org-level issue types requires `admin:org` (NEW requirement — currently undocumented). The README amendment notes that `admin:org` is required ONLY when running `setup` against an org that does not already have all eight canonical issue types defined; once they are in place subsequent `setup` runs only need `read:org`.

The README change is the implementation supervisor's responsibility within the same PR — the spec does NOT carry the README diff. The spec amendment commit body (`unblock-wgj` commit) MUST mention the README change in plain text and the implementation acceptance criteria MUST list it as a checkbox so the supervisor doesn't miss it. Per Miguel's 2026-04-30 direction (Decision answer 1).

### B.5 Plan patch — GAP entry

A new GAP entry in `docs/plans/01-plan-mcp-foundation.md` tracks the org-level IssueType ensure-and-heal work:

- **GAP-16 — Org-level IssueType ensure-and-heal (`unblock-wgj`).** `setup_fields` does not currently ensure the eight canonical `IssueType` variants exist on the org. After `unblock-wgj` lands, the function ensures them via the GraphQL org-level IssueType API per §5.7 step 3, surfaces the outcome via `SetupReport.issue_types_created`, and propagates that into `SetupResult` (§8.10). DRIFT type. Resolution: implementation bead under `unblock-wgj`.

### B.6 Commit-message footer template

The `unblock-wgj` implementation commit MUST carry both a `BREAKING CHANGE:` and an `API:` footer per CLAUDE.md "Pub API Change Tracking":

```
BREAKING CHANGE: IssueType is now #[non_exhaustive] in unblock-core.
Existing exhaustive `match IssueType { ... }` arms in downstream code
must add a wildcard `_` arm or per-variant arms for new entries. Same
forward-compat hardening previously applied to Status (unblock-1zj).

API: IssueType gains three additive `pub const fn` helpers in
unblock-core: canonical_name(), canonical_color(),
canonical_description(). SetupReport (unblock-github) gains a new
additive `issue_types_created: Vec<String>` field. setup_fields
extends its idempotent posture with an org-level IssueType
ensure-and-heal step. See SPEC §2.6 / §5.7 / §8.10 / §8.1 and
Appendix B for the full closure.
```

The `BREAKING CHANGE:` footer is mandatory because `#[non_exhaustive]` on a public enum is an incompatible change at the type level. The `API:` footer is mandatory for the additive helpers and the new `SetupReport` field. Scope: library crates `unblock-core` and `unblock-github` (binary `unblock-mcp` is excluded from the rule per CLAUDE.md, but it consumes both libraries and its tests must be updated alongside).

### B.7 Scope NOT covered by this amendment

This amendment is deliberately confined to the four user-decided design choices and the two named drifts. Items NOT in scope and therefore NOT changed by `unblock-wgj`:

- The `Status` 6-entry TitleCase canonical list (already specified in §2.3 / §5.7 by `unblock-1zj`).
- The `IssueRef`-typed cross-repo error variants (already specified in §11.1 by `unblock-eos`).
- The §11.4 cross-repo response contract (already complete).
- The Phase 02 `AgentKind` / `AgentClient` detection wiring itself — `unblock-wgj` consumes `state.agent_kind_str()` as an existing helper but does NOT specify how detection populates `agent_kind`. That stays a Phase 02 concern per §1.2.
- Any new Phase 02 features (reconcile, doctor, circuit breaker — explicitly out of scope per §1.2).

---

*This spec defines everything needed to implement Phase 01 (v0.1.0). The governing principles are in the [MANIFESTO](../MANIFESTO.md). The product scope is in the [PRD](../PRD.md). The full technical architecture is in the [SPEC](../SPEC.md). The implementation plan and gap analysis are in the [Phase 01 Plan](../plans/01-plan-mcp-foundation.md).*
