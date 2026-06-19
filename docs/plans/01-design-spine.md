# unblock — Cross-Crate Design Spine (v1)

- **Status:** AUTHORITATIVE interface contract — every per-crate plan MUST conform; APIs may not drift from this without amending this file first.
- **Date:** 2026-06-19
- **Source of truth:** `docs/PRD.md` (PRD APPROVED v1.1) + `docs/plans/implementation-plan.md` (v1 thin slice).
- **Sources-of-truth hierarchy:** PRD > **this spine** (all cross-crate interfaces) > crate plans. If a crate plan and this spine disagree on a type/field/signature, the spine wins and the crate plan is the bug.
- **Scope:** the shared interface surface (types + signatures + shapes) for the v1 walking skeleton. v1.1-deferred items appear only as reserved seams, marked `[v1.1]`.
- **Edition/MSRV:** Rust 2024, stable `1.96.0`. **Every crate:** `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`, clippy pedantic.

> This is a **contract**, not prose. Signatures and shapes are normative. Where a type is `[v1.1]` it is listed so the v1 layout reserves room (column, variant, field) without implementing behaviour.

**Layering (NFR-15, acyclic, enforced):**
`L0 model | error → L1 policy → L2 storage → L3 sync | health → L4 config → L5 engine → L6 render → L7 mcp | cli`
`unblock-storage` depends on **model + error only**. Any contract type both policy and storage need lives in `unblock-model`.

> The one-liner above is **layer-order only** — it does not show per-crate fan-in (e.g. engine depends on storage + policy + sync + health + config; the sanctioned `model → error` L0 edge per CF-G). For the full per-crate edge set see README §2 and each crate plan's "Depends on" line. Within L7 the one settled L7↔L7 edge is **`unblock-cli → unblock-mcp`** (never the reverse; see §0.1 and the render box maps to CF-A, not CF-3).

### 0.1 Settled L7 edge — `unblock-cli → unblock-mcp` (NORMATIVE; closes cli Q1)

`unblock-cli` **depends on** `unblock-mcp`. The CLI owns the `unblock` binary (incl. `unblock serve`); `unblock-mcp` is a **library** that exposes `serve(session, transport, shutdown)` and the tool/resource/prompt registry. The direction is fixed **cli → mcp** and **never mcp → cli** — this is the single L7↔L7 edge that determines acyclicity, and it is now a decision (not an assumption). The cli plan's Open Question Q1 is **RESOLVED** by this line. README §2 and §0 draw this edge as settled and are correct.

---

## 1. Domain types — `unblock-model` (L0)

Pure types, no I/O. Derives target: `Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema` unless noted. `chrono::{DateTime, Utc}` for time. Open enums (`Status`/`IssueType`/`DependencyType`/`EventType`) keep a `Custom(String)` tail variant with custom `Deserialize` (unknown string → `Custom`) and `as_str`/`Display`/`FromStr`.

### 1.1 Status

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default] Open,
    InProgress,            // serializes "in_progress"
    Blocked,
    Deferred,
    Draft,
    Closed,
    Tombstone,
    Pinned,
    #[serde(untagged)] Custom(String),
}
// custom Deserialize: unknown string -> Custom; case-insensitive known parse.
impl Status {
    pub fn as_str(&self) -> &str;            // "open" | "in_progress" | ...
    pub const fn is_terminal(&self) -> bool; // Closed | Tombstone
    pub const fn is_active(&self) -> bool;   // Open | InProgress
    pub const fn is_draft(&self) -> bool;    // Draft
}
// Display + FromStr (Err = unblock_error::ModelError).
```

### 1.2 Priority

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, JsonSchema)]
#[serde(transparent)]
pub struct Priority(pub i32);             // valid range 0..=4

impl Priority {
    pub const CRITICAL: Self = Self(0);
    pub const HIGH:     Self = Self(1);
    pub const MEDIUM:   Self = Self(2);    // Default
    pub const LOW:      Self = Self(3);
    pub const BACKLOG:  Self = Self(4);
}
// Default = MEDIUM. Display => "P{n}". FromStr parses "P0".."P4" / "0".."4"
// (case-insensitive); out-of-range/non-numeric => ModelError::InvalidPriority.
// Ordering is numeric (CRITICAL < HIGH < ... < BACKLOG) — used by hybrid ready sort.
```

### 1.3 IssueType

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueType {
    #[default] Task, Bug, Feature, Epic, Chore, Docs, Question,
    #[serde(untagged)] Custom(String),
}
impl IssueType {
    pub fn as_str(&self) -> &str;
    pub const fn is_standard(&self) -> bool; // !Custom
}
// epic participates in EpicStatus rollups [v1.1]. Default = Task.
```

### 1.4 DependencyType

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyType {
    Blocks, ParentChild, ConditionalBlocks, WaitsFor,   // <- the four that gate ready-work
    Related, DiscoveredFrom, RepliesTo, RelatesTo,
    Duplicates, Supersedes, CausedBy,
    #[serde(untagged)] Custom(String),
}
impl DependencyType {
    pub fn as_str(&self) -> &str;            // "blocks" | "parent-child" | ...
    pub const fn affects_ready_work(&self) -> bool; // Blocks|ParentChild|ConditionalBlocks|WaitsFor
    pub const fn is_blocking(&self) -> bool;        // same set as affects_ready_work
}
// custom Deserialize: kebab-case known parse, else Custom. DiscoveredFrom is the agent flywheel edge.
```

### 1.5 EventType

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]   // hand-rolled Serialize/Deserialize/JsonSchema (as String)
pub enum EventType {
    Created, Updated, StatusChanged, PriorityChanged, AssigneeChanged,
    Commented, Closed, Reopened, DependencyAdded, DependencyRemoved,
    LabelAdded, LabelRemoved, Compacted, Deleted, Restored,
    Custom(String),
}
impl EventType { pub fn as_str(&self) -> &str; } // snake_case strings; JsonSchema = String.
```

### 1.6 Issue (full field set)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Issue {
    pub id: String,                                  // prefix + optional slug + hash, e.g. "ub-abc123"

    #[serde(skip)]
    pub content_hash: Option<String>,                // canonical dedup; NEVER serialized to JSONL

    pub title: String,                               // 1..=500 chars (validator)
    #[serde(default, skip_serializing_if = "Option::is_none")] pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub design: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub acceptance_criteria: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub notes: Option<String>,

    #[serde(default)] pub status: Status,
    #[serde(default)] pub priority: Priority,
    #[serde(default)] pub issue_type: IssueType,

    #[serde(default, skip_serializing_if = "Option::is_none")] pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub estimated_minutes: Option<i32>,

    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub created_by: Option<String>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub closed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub close_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub closed_by_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub due_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub defer_until: Option<DateTime<Utc>>,

    #[serde(default, skip_serializing_if = "Option::is_none")] pub external_ref: Option<String>,     // orphans derive from this (FR-15)
    #[serde(default, skip_serializing_if = "Option::is_none")] pub source_system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub source_repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub source_repo_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent_context: Option<String>,    // canonical-JSON governance doc

    // Tombstone fields (delete semantics; tombstones NEVER resurrect on import)
    #[serde(default, skip_serializing_if = "Option::is_none")] pub deleted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub deleted_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub delete_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub original_type: Option<String>,

    // Compaction fields — KEPT for JSONL round-trip fidelity (D12), not Go-bd conformance.
    #[serde(default, serialize_with = "serialize_compaction_level")] pub compaction_level: Option<i32>, // None serializes as 0
    #[serde(default, skip_serializing_if = "Option::is_none")] pub compacted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub compacted_at_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub original_size: Option<i32>,

    // Messaging / context flags
    #[serde(default, skip_serializing_if = "Option::is_none")] pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")] pub ephemeral: bool,
    #[serde(default, skip_serializing_if = "is_false")] pub pinned: bool,
    #[serde(default, skip_serializing_if = "is_false")] pub is_template: bool,

    // Relations (hydrated for export/display; not always columns)
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub dependencies: Vec<Dependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub comments: Vec<Comment>,        // [v1.1] populated
}
```

### 1.7 Dependency, Comment, Event, EpicStatus

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Dependency {
    pub issue_id: String,                 // source (has the dependency)
    pub depends_on_id: String,            // target (depended upon)
    #[serde(rename = "type")] pub dep_type: DependencyType,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub created_by: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_metadata",
            skip_serializing_if = "Option::is_none")] pub metadata: Option<String>, // JSON; "" coerced to None
    #[serde(default, skip_serializing_if = "Option::is_none")] pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Comment {                       // [v1.1] surface; type defined now
    pub id: i64,
    pub issue_id: String,
    pub author: String,
    #[serde(rename = "text")] pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Event {                         // append-only audit; written transactionally inside mutate()
    pub id: i64,
    pub issue_id: String,
    pub event_type: EventType,
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub old_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub new_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    // Tier-1 attribution (capture-only, NEVER enforced)
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct EpicStatus {                    // [v1.1] derived rollup
    pub epic: Issue,
    pub total_children: usize,
    pub closed_children: usize,
    pub eligible_for_close: bool,
}
```

### 1.8 content_hash / sync_equals / tombstone semantics (normative)

- **`content_hash`** — `compute_content_hash(&self) -> String`. SHA-256, lowercase hex, over a stable ordered, null-separated field set: `title, description, design, acceptance_criteria, notes, status.as_str(), priority.0, issue_type.as_str(), assignee, owner, created_by, external_ref, source_system, pinned, is_template`. **Excludes** `id`, `content_hash` (circular), relations (labels/deps/comments), all timestamps, tombstone fields, `estimated_minutes`, `due_at`, `defer_until`, `close_reason`, `closed_by_session`. `#[serde(skip)]` → never appears in JSONL; recomputed on load. Used for import dedup/idempotency (FR-26) and sync equality fast-path.
- **`sync_equals(&self, other) -> bool`** — semantic equality for import/export boundaries. Compares the full synced payload (incl. `due_at`, `defer_until`, tombstone fields, compaction fields, and relations **order-independent**: labels deduped+sorted; deps and comments sorted by a fixed key tuple). Treats `compaction_level == None` as `0`. Ignores volatile audit-only fields. This is the import "is this line a no-op?" predicate, not derived `PartialEq`.
- **Tombstone** — delete sets `status = Tombstone` + `deleted_at`/`deleted_by`/`delete_reason` (and `original_type` preserved). `is_expired_tombstone(retention_days: Option<u64>) -> bool` (TTL helper). Invariant: **import NEVER resurrects a tombstone** — a non-tombstone JSONL line for an id that is tombstoned in the DB is rejected/skipped, not applied.

### 1.9 Validation + shared contract types

```rust
pub struct IssueValidator; // pure; title 1..=500, priority 0..=4, enum coherence, reparent-cycle check input.
impl IssueValidator { pub fn validate(issue: &Issue) -> Result<(), unblock_error::ModelError>; }

// Shared contract type that BOTH policy and storage need lives here (CF-11):
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(pub String); // ready/blocked projection cache key contract.
```

### 1.10 Query/result contract types (CF-A/CF-B/CF-C — defined here, re-exported by upper crates)

These types are **defined normatively in `unblock-model`** so that crates which cannot depend on `unblock-storage` can still reference them. `unblock-policy` needs `ListFilters`/`CountGroupBy` for its filter-fingerprint (CF-C); `unblock-render` (model + error only) needs the display/result DTOs and `OutputFormat` to format output (CF-A/CF-J); `DiagnosticReport`/`DiagnosticKind`/`DiagnosticFinding` are referenced by engine/render/mcp and previously had no home (CF-B). The **full owned set** is: `ListFilters, CountGroupBy, OutputFormat, CountBucket, GraphEdge, DepTree, CloseOutcome, ImportReport, ExportReport, DiagnosticReport, DiagnosticFinding, DiagnosticKind`. Every other crate (`unblock-storage`, `unblock-engine`, `unblock-render`, `unblock-config`, `unblock-sync`, `unblock-mcp`) **re-exports** these via `pub use unblock_model::{...}` — none redefines them.

**Derive policy for §1.10 (NORMATIVE — G-1):** every type below flows to an L7 consumer (MCP `ToolOutput`/`QueryInput`, engine results, render parse-back, policy serialization). They therefore ALL derive `Debug, Clone, Serialize, Deserialize, JsonSchema`, plus `PartialEq, Eq` where round-trip/equality tests need it. The `derive` lines below are normative; crate plans (unblock-model `filters.rs`/`results.rs`, mcp `ToolOutput`, engine, render) must match exactly. `PathBuf` derives `JsonSchema` (serialized as a string) — `ExportReport.path` is schema-valid.

```rust
// --- query inputs (CF-C: relocated from storage so policy can depend on them) ---
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ListFilters {
    pub status: Vec<Status>,            // OR within
    pub issue_type: Vec<IssueType>,
    pub assignee: Option<String>,
    pub labels_all: Vec<String>,        // AND
    pub labels_any: Vec<String>,        // OR
    pub priority_min: Option<Priority>,
    pub priority_max: Option<Priority>,
    pub text_contains: Option<String>,
    pub include_deferred: bool,
    pub include_closed: bool,
    pub limit: Option<usize>,           // None = unlimited (ready is default-complete)
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CountGroupBy { Status, Type, Assignee, Priority, Label }

// --- output format (CF-J: defined once HERE; render + config re-export, never redefine) ---
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    #[default] Json,        // structured stdout (default)
    Robot,                  // stable machine-parse line format
    Plain,                  // human terminal
    Csv,
    Markdown,
    #[cfg(feature = "toon")] Toon,   // [v1.1] feature name is exactly "toon" (pinned: spine == render.md == PRD)
}

// --- display / result DTOs (CF-A: relocated so unblock-render can format them) ---
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct CountBucket { pub key: String, pub count: usize }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct GraphEdge   { pub from: String, pub to: String, pub dep_type: DependencyType }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DepTree     { pub root: String, pub edges: Vec<GraphEdge> }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct CloseOutcome  { pub closed: Issue, pub newly_unblocked: Vec<Issue> }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ImportReport  { pub imported: usize, pub skipped: usize, pub dropped_fields: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ExportReport  { pub written: usize, pub path: PathBuf } // PathBuf: JsonSchema = string

// --- diagnostics (CF-B: defined normatively here; referenced by engine/render/mcp) ---
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticKind { Stats, Info, Where, Version, Lint, Changelog, Orphans } // mirrors §5.2 DiagnosticsInput kinds

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DiagnosticReport { pub kind: DiagnosticKind, pub findings: Vec<DiagnosticFinding> }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DiagnosticFinding { pub label: String, pub detail: String } // generic key/value finding row
```

---

## 2. Error taxonomy — `unblock-error` (L0)

**Approach (D4):** **snafu** per-crate error enums (no god-enum). Each library crate (`model`, `policy`, `storage`, `sync`, `health`, `config`, `engine`, `render`) defines its own `#[derive(Debug, Snafu)]` enum with context selectors. `unblock-error` owns the **shared boundary vocabulary**: `ErrorCode`, the exit-code table, `StructuredError`, and `From`/mapping glue. Per-crate enums **compose upward** via snafu `#[snafu(transparent)]` / `source` wrapping; **backend errors never leak** (libsql errors are absorbed at `StorageError` and re-exposed only as `ErrorCode`/message). Mapping to MCP error data / 0–8 exit codes happens **only at the L7 boundary**.

### 2.1 Per-crate enum shape (pattern)

```rust
// e.g. unblock-storage::StorageError
#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum StorageError {
    #[snafu(display("issue not found: {id}"))]
    IssueNotFound { id: String },
    #[snafu(display("database locked"))]
    DatabaseLocked,                                   // -> ErrorCode::DatabaseLocked
    #[snafu(display("dependency cycle: {path}"))]
    CycleDetected { path: String },
    #[snafu(display("backend failure"))]
    Backend { source: BackendOpaque },               // libsql error absorbed; NEVER public
    // ...
}
impl StorageError { pub fn code(&self) -> unblock_error::ErrorCode; } // each crate maps its variants -> ErrorCode
```

Each crate implements `fn code(&self) -> ErrorCode` (and a hint/retryable view) so the boundary can build a `StructuredError` uniformly. Upward composition uses snafu source nesting; the engine's error is the union surfaced to L7.

### 2.2 ErrorCode (stable, SCREAMING_SNAKE in JSON)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ErrorCode {
    // exit 2 — Database
    DatabaseNotFound, DatabaseLocked, SchemaMismatch, DatabaseError, NotInitialized, AlreadyInitialized,
    // exit 3 — Issue / operational
    IssueNotFound, AmbiguousId, IdCollision, InvalidId, NothingToDo, AlreadyClaimed, // AlreadyClaimed new for FR-2
    // exit 4 — Validation / policy
    ValidationFailed, InvalidStatus, InvalidType, InvalidPriority, RequiredField, PolicyViolation,
    // exit 5 — Dependency
    CycleDetected, DependencyNotFound, HasDependents, SelfDependency, DuplicateDependency,
    // exit 6 — Sync / JSONL
    JsonlParseError, PrefixMismatch, ImportCollision, SyncConflict, ConflictMarkers, PathTraversal,
    // exit 7 — Config
    ConfigError, ConfigNotFound, ConfigParseError,
    // exit 8 — I/O
    IoError, JsonError,                              // YamlError dropped (TOML now)
    // exit 1 — Internal
    InternalError,
}
impl ErrorCode {
    pub const fn as_str(&self) -> &'static str;       // "ISSUE_NOT_FOUND", ...
    pub const fn exit_code(&self) -> u8;              // 0..=8 per table below
    pub const fn is_retryable(&self) -> bool;
    //   exact retryable set (no glob): DatabaseLocked | AlreadyClaimed | ValidationFailed
    //   | InvalidStatus | InvalidType | InvalidPriority | RequiredField | AmbiguousId.
    //   (matches error.md; the §2.3 exit-4 validation variants are the four Validation* members.)
}
```

### 2.3 Exit-code table (0–8) — golden-snapshot pinned (FR-11)

| Exit | Category | ErrorCodes |
|---|---|---|
| **0** | Success | (no error) |
| **1** | Internal / unknown | `InternalError` |
| **2** | Database | `DatabaseNotFound`, `DatabaseLocked`, `SchemaMismatch`, `DatabaseError`, `NotInitialized`, `AlreadyInitialized` |
| **3** | Issue / operational | `IssueNotFound`, `AmbiguousId`, `IdCollision`, `InvalidId`, `NothingToDo`, `AlreadyClaimed` |
| **4** | Validation / policy | `ValidationFailed`, `InvalidStatus`, `InvalidType`, `InvalidPriority`, `RequiredField`, `PolicyViolation` |
| **5** | Dependency | `CycleDetected`, `DependencyNotFound`, `HasDependents`, `SelfDependency`, `DuplicateDependency` |
| **6** | Sync / JSONL | `JsonlParseError`, `PrefixMismatch`, `ImportCollision`, `SyncConflict`, `ConflictMarkers`, `PathTraversal` |
| **7** | Config | `ConfigError`, `ConfigNotFound`, `ConfigParseError` |
| **8** | I/O | `IoError`, `JsonError` |

### 2.4 Structured error payload (FR-11; mirrors MCP error data)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StructuredError {
    pub code: ErrorCode,           // serialized as "ISSUE_NOT_FOUND"
    pub message: String,           // human-readable, terminal-sanitized
    pub hint: Option<String>,      // agent self-correction guidance
    pub retryable: bool,           // == code.is_retryable()
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub context: serde_json::Map<String, serde_json::Value>, // optional structured detail
}
impl StructuredError {
    pub fn exit_code(&self) -> u8;                 // = self.code.exit_code()
    pub fn from_code(code: ErrorCode, message: impl Into<String>) -> Self;
}
```

The L7 boundary converts any composed crate error → `StructuredError` (CLI: serialize to stdout + `process::exit(exit_code)`; MCP: attach as error data, §5.6). Output is **always valid JSON even on error** (FR-11). `tracing` on `unblock.reliability` records the guard/error (NFR-13); structured output strictly stdout, diagnostics stderr (NFR-14).

---

## 3. Storage trait — `unblock-storage` (L2)

Async (`async_trait`), backend-agnostic error (`StorageError`, §2.1). The libsql impl is the only backend-aware code; remote/replica behind a non-default feature (D15). Depends on **model + error only**.

### 3.1 Supporting types

`ListFilters`, `CountGroupBy`, `CountBucket`, `GraphEdge`, `DepTree`, and (for the CF-E reserved diagnostic seams) `DiagnosticReport`/`DiagnosticFinding`/`DiagnosticKind` are **defined in `unblock-model` §1.10** (CF-A/CF-B/CF-C) and **re-exported** by `unblock-storage` (`pub use unblock_model::{ListFilters, CountGroupBy, CountBucket, GraphEdge, DepTree, DiagnosticReport, DiagnosticFinding, DiagnosticKind};`) — storage does **not** redefine them. The storage-owned types below remain defined here.

```rust
#[derive(Debug, Clone)]
pub struct DeletePlan { pub mode: DeleteMode, pub targets: Vec<String>, pub cascade_children: Vec<String> }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode { Tombstone, Cascade, Hard, DryRun }

#[derive(Debug, Clone)]
pub struct IssuePatch {                  // partial update; None = leave unchanged
    pub title: Option<String>, pub description: Option<Option<String>>, /* ...all updatable fields... */
    pub status: Option<Status>, pub priority: Option<Priority>, pub assignee: Option<Option<String>>,
    pub labels_add: Vec<String>, pub labels_remove: Vec<String>, pub labels_set: Option<Vec<String>>,
    pub parent: Option<Option<String>>,  // reparent; cycle-checked
}
```

### 3.2 The trait

```rust
#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    // --- lifecycle ---
    async fn migrate(&self) -> Result<(), StorageError>;
    async fn integrity_check(&self) -> Result<Vec<String>, StorageError>; // libsql integrity_check rows

    // --- issue CRUD (mutations carry actor + optional Tier-1 attribution; write Event(s) transactionally) ---
    async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError>; // returns id
    async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError>;               // hydrated (labels/deps)
    async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError>;
    async fn update_issue(&self, id: &str, patch: &IssuePatch, actor: &str) -> Result<Issue, StorageError>;
    async fn delete_issue(&self, plan: &DeletePlan, actor: &str) -> Result<DeletePlan, StorageError>; // DryRun mutates nothing

    // --- atomic claim (FR-2): single mutation sets assignee + in_progress, no race window ---
    async fn claim_issue(&self, id: &str, assignee: &str, actor: &str) -> Result<Issue, StorageError>;
    //   loser gets StorageError mapping to ErrorCode::AlreadyClaimed.

    // --- defer / undefer (FR-3) ---
    async fn defer_issue(&self, id: &str, until: DateTime<Utc>, actor: &str) -> Result<Issue, StorageError>;
    async fn undefer_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError>;

    // --- queries (FR-4) ---
    async fn list_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;
    async fn ready_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;  // unblocked+undeferred, default-complete
    async fn blocked_issues(&self, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>;
    async fn search_issues(&self, query: &str, filters: &ListFilters) -> Result<Vec<Issue>, StorageError>; // FTS, cap default 50
    async fn count_issues(&self, filters: &ListFilters, group_by: Option<CountGroupBy>)
        -> Result<Vec<CountBucket>, StorageError>;
    async fn stale_issues(&self, older_than: DateTime<Utc>, filters: &ListFilters)
        -> Result<Vec<Issue>, StorageError>;

    // --- dependencies (FR-5) ---
    async fn add_dependency(&self, dep: &Dependency, actor: &str) -> Result<(), StorageError>; // rejects cycle w/ path
    async fn remove_dependency(&self, issue_id: &str, depends_on_id: &str,
                               dep_type: &DependencyType, actor: &str) -> Result<(), StorageError>;
    async fn list_dependencies(&self, id: &str) -> Result<Vec<Dependency>, StorageError>;
    async fn dependency_tree(&self, id: &str) -> Result<DepTree, StorageError>;
    async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, StorageError>; // backs dep `graph`; empty roots = whole graph
    async fn detect_cycles(&self) -> Result<Vec<Vec<String>>, StorageError>; // each = a cycle path

    // --- events (audit; append-only) ---
    async fn list_events(&self, issue_id: &str) -> Result<Vec<Event>, StorageError>;

    // --- diagnostics support (FR-15, pure-DB; no git) ---
    async fn closed_since(&self, since: Option<DateTime<Utc>>) -> Result<Vec<Issue>, StorageError>; // changelog
    async fn orphan_candidates(&self) -> Result<Vec<Issue>, StorageError>;  // external_ref matches commit pattern

    // --- [v1.1] reserved seams (CF-E: additive; depended on by config db-layer + health full-taxonomy) ---
    //   These are reserved now so unblock-config can read persisted config from the DB layer
    //   and unblock-health can run the full diagnostic taxonomy through storage. v1 impls may
    //   return Err(ErrorCode::NothingToDo)/empty; the SIGNATURES are pinned so dependents compile.
    // [v1.1] async fn read_config(&self) -> Result<Vec<(String, String)>, StorageError>; // persisted key/value config rows
    // [v1.1] async fn diagnostic_probe(&self, kind: DiagnosticKind) -> Result<DiagnosticReport, StorageError>; // full-taxonomy probe
    // [v1.1] async fn diagnostic_probes(&self) -> Result<Vec<DiagnosticReport>, StorageError>; // batch probe for health
}
```

### 3.3 libsql impl notes (normative for `unblock-storage`)

- **WAL** journal mode; **`busy_timeout > 0`** (native, non-spinning — resolves fsqlite-243 hot-spin by construction, NFR-3). Never `busy_timeout=0` + hand-rolled backoff.
- **Default build = local file / bundled only.** Remote/embedded-replica is a non-default Cargo feature `remote` (TLS/HTTP transitive surface kept off the normal path; D15/NFR-10). When `remote`, app-level jittered retry (`backon`/`tokio-retry`, **not** archived `backoff 0.4`) guards only that path; `wiremock` for tests.
- Mutations are **transactional**: issue rows + audit `Event` rows committed together inside one tx.
- libsql/SQLite errors are absorbed into `StorageError::Backend { .. }` (opaque) and surfaced only as `ErrorCode` — no backend type in the public API.
- Backed by a backend-independent **contract suite** (NFR-16) exercising every trait method; the contention lab (NFR-3, M0 gate) drives N concurrent writers asserting correctness + no 100% CPU hot-spin.

---

## 4. Engine session API — `unblock-engine` (L5)

The single mutation home (FR-9). Composes storage + policy + (optional) sync/health. MCP and CLI are thin adapters; behaviour cannot drift. **In-process write serialization via a tokio `Semaphore(1)`** (D14); reads bypass it (FR-10).

**Workspace-open ownership (CF-D — normative):** discovery of `.unblock/` and construction of the `Arc<dyn Storage>` is owned by **`unblock-config`**. `WorkspaceContext` is **DEFINED in `unblock-config`** (not engine). `unblock-config` exposes **two facades** (G-5 option b):

```rust
// in unblock-config:

// (1) resolve-only — NO storage; discovery + resolved config only (for `where`, doctor pre-checks,
//     completions, and anything that must not open/migrate the DB).
#[derive(Debug, Clone)]
pub struct ResolvedContext { pub workspace_dir: PathBuf, pub actor: String, /* resolved config */ }
pub async fn open_workspace(start: &Path) -> Result<ResolvedContext, ConfigError>;

// (2) storage-bearing — discovery + open/migrate libsql; the field is NON-OPTIONAL.
#[derive(Clone)]
pub struct WorkspaceContext {
    pub storage: Arc<dyn Storage>,     // NON-OPTIONAL (G-5): always present once built
    pub workspace_dir: PathBuf,
    pub actor: String,
    /* resolved config */
}
pub async fn open_with_storage(start: &Path) -> Result<WorkspaceContext, ConfigError>;
```

**`unblock-engine` CONSUMES** a `WorkspaceContext` — it does **not** construct storage itself, and never sees an `Option<Arc<dyn Storage>>`. `Session::open` takes the already-built storage-bearing context; because `storage` is non-optional there is no unwrap and no None-path mismatch. The resolve-only `ResolvedContext` is for callers that must not touch the DB.

**Result DTO ownership (CF-A — normative):** `CloseOutcome`, `ImportReport`, `ExportReport`, `CountBucket`, `GraphEdge`, `DepTree`, and `DiagnosticReport`/`DiagnosticFinding`/`DiagnosticKind` are **defined in `unblock-model` §1.10** and **re-exported** by `unblock-engine` (`pub use unblock_model::{CloseOutcome, ImportReport, ExportReport, CountBucket, GraphEdge, DepTree, DiagnosticReport, DiagnosticFinding, DiagnosticKind};`). **`DiagnosticFinding` is included explicitly** (it travels inside `DiagnosticReport`, so any consumer reaching it via the engine must see it re-exported — G-10). The engine does **not** redefine them, so `unblock-render` (model + error only) can format engine results without depending on the engine.

### 4.1 Session surface

```rust
pub struct Session { /* storage: Arc<dyn Storage>, write_permit: Arc<tokio::sync::Semaphore>, policy, config, shutdown */ }

// SessionConfig is engine-owned. Post-CF-D `workspace_dir`/`actor` MOVED to WorkspaceContext
// (config-owned) and are NO LONGER here. SessionConfig carries engine-behaviour knobs only.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    pub jsonl_export: bool,            // auto-export JSONL after mutating ops (FR-7)
    pub import_on_open: bool,          // run import_jsonl during open() if the JSONL is newer (FR-8)
    pub remote: bool,                  // enable the non-default remote storage path (D15; off in v1)
    /* ...other engine knobs... */
}

// ImportOptions is ENGINE-owned (defined in unblock-engine, consumed by import_jsonl below).
// NOT a sync-owned type and NOT a model DTO.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions { pub dry_run: bool /* ...future import knobs... */ }

impl Session {
    // open: CONSUMES a WorkspaceContext built by unblock-config (discovery + Arc<dyn Storage>
    //       construction + migrate-if-needed happen in config, NOT here — CF-D). Engine only
    //       wires policy/sync/health + the write permit + optional import seam over the context.
    pub async fn open(ctx: WorkspaceContext, cfg: SessionConfig) -> Result<Self, EngineError>;

    // --- reads: fast path, NO write permit (FR-10) ---
    pub async fn get(&self, id: &str) -> Result<Option<Issue>, EngineError>;
    pub async fn list(&self, filters: &ListFilters) -> Result<Vec<Issue>, EngineError>;
    pub async fn ready(&self, filters: &ListFilters) -> Result<Vec<Issue>, EngineError>; // hybrid sort via policy
    pub async fn blocked(&self, filters: &ListFilters) -> Result<Vec<Issue>, EngineError>;
    pub async fn search(&self, query: &str, filters: &ListFilters) -> Result<Vec<Issue>, EngineError>;
    pub async fn count(&self, filters: &ListFilters, by: Option<CountGroupBy>) -> Result<Vec<CountBucket>, EngineError>;
    pub async fn stale(&self, older_than: DateTime<Utc>, filters: &ListFilters) -> Result<Vec<Issue>, EngineError>;
    pub async fn dependency_tree(&self, id: &str) -> Result<DepTree, EngineError>;
    pub async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, EngineError>; // backs dep `graph` action (§5.2); empty roots = whole graph
    pub async fn detect_cycles(&self) -> Result<Vec<Vec<String>>, EngineError>;
    pub async fn diagnostics(&self, kind: DiagnosticKind) -> Result<DiagnosticReport, EngineError>; // FR-15

    // --- mutations: each acquires the write permit for its whole tx ---
    pub async fn create(&self, issue: &Issue) -> Result<String, EngineError>;
    pub async fn update(&self, id: &str, patch: &IssuePatch) -> Result<Issue, EngineError>;
    pub async fn delete(&self, plan: &DeletePlan) -> Result<DeletePlan, EngineError>;
    pub async fn claim(&self, id: &str, assignee: &str) -> Result<Issue, EngineError>;      // FR-2
    pub async fn defer(&self, id: &str, until: DateTime<Utc>) -> Result<Issue, EngineError>;
    pub async fn undefer(&self, id: &str) -> Result<Issue, EngineError>;
    pub async fn add_dep(&self, dep: &Dependency) -> Result<(), EngineError>;
    pub async fn remove_dep(&self, issue_id: &str, on: &str, ty: &DependencyType) -> Result<(), EngineError>;
    pub async fn close_with_suggestions(&self, id: &str, reason: Option<String>)
        -> Result<CloseOutcome, EngineError>; // returns newly-unblocked issues (FR-11)

    // --- interchange (FR-7/FR-8/FR-26), delegates to unblock-sync ---
    pub async fn export_jsonl(&self, path: &Path) -> Result<ExportReport, EngineError>; // atomic temp+fsync+rename
    pub async fn import_jsonl(&self, path: &Path, opts: ImportOptions) -> Result<ImportReport, EngineError>;
    pub async fn import_bd(&self, path: &Path) -> Result<ImportReport, EngineError>;     // D16, idempotent via content_hash

    // --- lifecycle / ops (OQ-2 RESOLVED: doctor + recover ARE part of the public Session surface;
    //     the cli `doctor` command and mcp diagnostics both go through these, no separate path) ---
    pub async fn doctor(&self) -> Result<DiagnosticReport, EngineError>;  // integrity_check + diagnostic taxonomy (FR-15/FR-16)
    pub async fn recover(&self) -> Result<DiagnosticReport, EngineError>; // attempt repair (WAL checkpoint, reindex); reports actions taken
    pub async fn shutdown(&self) -> Result<(), EngineError>; // flush + close libsql cleanly (FR-17)
}

// CloseOutcome / ImportReport / ExportReport are defined in unblock-model §1.10 and
// re-exported here (CF-A) — NOT redefined. CountBucket / GraphEdge / DepTree /
// DiagnosticReport / DiagnosticFinding / DiagnosticKind likewise come from unblock-model
// via the same re-export. SessionConfig + ImportOptions are engine-owned (above).
```

### 4.2 Write-Semaphore contract (D14 — normative)

- One `Arc<tokio::sync::Semaphore>` with **1 permit** per `Session`. Every mutation `acquire()`s the single permit for the **entire** storage transaction, then releases — serializing all in-process writers (linearizable per FR-9).
- **Reads NEVER touch the permit** (FR-10): they run concurrently against libsql WAL readers while a write holds the permit.
- Scope is **in-process only**: the supported topology is exactly one `unblock serve` per workspace. Concurrent external writers (CLI `migrate`/`doctor` while serve runs, multiple serve) are best-effort via WAL + `busy_timeout`, **not** supported.
- Permit acquisition is **uncancel-safe across the tx boundary**: a dropped future before commit must release the permit and leave the DB committed-or-rolled-back (no partial state) — verified by the SIGTERM-mid-write failure-injection test (NFR-5).
- Property test (FR-9): interleaved mutations through the engine are linearizable; MCP and CLI produce identical results for the same op.

---

## 5. MCP schemas — `unblock-mcp` (L7)

**rmcp 1.7** (`server`, `transport-io`) stdio server (`unblock serve`), thin adapter over `Session`. **7 consolidated tools** (target ≤ 8), resources, prompts. Every tool input/output derives `JsonSchema` + `Serialize`/`Deserialize`; args are schemars-validated with size/rate limits (NFR-18). Discovery (`capabilities`/`schema`) carries `contract_version` (FR-12).

### 5.1 Tool taxonomy (7 tools)

| # | Tool | Discriminator | Maps to |
|---|---|---|---|
| 1 | `issue` | `action: create\|show\|update\|close\|reopen\|delete` | FR-1a/1b/1c |
| 2 | `claim` | (none) | FR-2 |
| 3 | `defer` | `action: defer\|undefer` | FR-3 |
| 4 | `query` | `kind: list\|ready\|blocked\|search\|count\|stale` | FR-4 |
| 5 | `dep` | `action: add\|remove\|list\|tree\|cycles\|graph` | FR-5 |
| 6 | `sync` | `action: export\|import\|import_bd` | FR-7/8/26 |
| 7 | `diagnostics` | `kind: stats\|info\|where\|version\|lint\|changelog\|orphans` | FR-15 |

### 5.2 Input shapes (schemars sketches)

```rust
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum IssueInput {
    Create {
        title: String,
        #[serde(default)] description: Option<String>,
        #[serde(default)] issue_type: Option<IssueType>,
        #[serde(default)] priority: Option<Priority>,
        #[serde(default)] labels: Vec<String>,
        #[serde(default)] parent: Option<String>,
        #[serde(default)] deps: Vec<DepInput>,
        #[serde(default)] due_at: Option<DateTime<Utc>>,
        #[serde(default)] defer_until: Option<DateTime<Utc>>,
        #[serde(default)] estimated_minutes: Option<i32>,
        #[serde(default)] slug: Option<String>,
        #[serde(default)] ephemeral: bool,
        #[serde(default)] quick: bool,                  // quick-create -> output is id only
        #[serde(flatten)] attribution: Attribution,     // agent_name/harness/model (capture-only)
    },
    Show   { id: String },
    Update { ids: Vec<String>, #[serde(flatten)] patch: PatchInput, #[serde(flatten)] attribution: Attribution },
    Close  { id: String, #[serde(default)] reason: Option<String>, #[serde(default)] suggest_next: bool,
             #[serde(flatten)] attribution: Attribution },
    Reopen { id: String, #[serde(flatten)] attribution: Attribution },
    Delete { ids: Vec<String>, #[serde(default)] mode: DeleteModeInput, #[serde(flatten)] attribution: Attribution },
}

#[derive(Deserialize, JsonSchema)]
pub struct ClaimInput { pub id: String, pub assignee: String, #[serde(flatten)] pub attribution: Attribution }

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DeferInput {
    Defer   { id: String, until: DateTime<Utc>, #[serde(flatten)] attribution: Attribution },
    Undefer { id: String, #[serde(flatten)] attribution: Attribution },
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryInput {
    List   { #[serde(flatten)] filters: FilterInput },
    Ready  { #[serde(flatten)] filters: FilterInput },   // default-complete unless limit set
    Blocked{ #[serde(flatten)] filters: FilterInput },
    Search { query: String, #[serde(default)] limit: Option<usize>, #[serde(flatten)] filters: FilterInput }, // cap 50 default
    Count  { #[serde(default)] group_by: Option<CountGroupBy>, #[serde(flatten)] filters: FilterInput },
    Stale  { older_than: DateTime<Utc>, #[serde(flatten)] filters: FilterInput },
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DepToolInput {                                                  // (was DepInput2; stray "2" removed)
    Add    { issue_id: String, depends_on_id: String, dep_type: DependencyType,
             #[serde(default)] metadata: Option<String>, #[serde(flatten)] attribution: Attribution },
    Remove { issue_id: String, depends_on_id: String, dep_type: DependencyType, #[serde(flatten)] attribution: Attribution },
    List   { id: String },
    Tree   { id: String },
    Cycles {},
    Graph  { #[serde(default)] roots: Vec<String> },
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SyncInput {
    Export   { #[serde(default)] path: Option<String> },     // default .unblock/issues.jsonl; path-confined
    Import   { path: String, #[serde(default)] dry_run: bool },
    ImportBd { path: String },                                // D16
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagnosticsInput {
    Stats {}, Info {}, Where {}, Version {}, Lint {}, Changelog { #[serde(default)] since: Option<DateTime<Utc>> }, Orphans {},
}

// MCP WIRE attribution (capture-only Tier-1 metadata on the wire). Distinct from the
// policy gate type (G-23e): unblock-policy's enforcement type is named `AttributionPolicy`
// (NOT `Attribution`) so the two never collide. This `Attribution` is mcp-owned, never enforced.
#[derive(Deserialize, JsonSchema, Default)]
pub struct Attribution { #[serde(default)] pub agent_name: Option<String>,
                         #[serde(default)] pub harness: Option<String>, #[serde(default)] pub model: Option<String> }

#[derive(Deserialize, JsonSchema, Default)]
pub struct FilterInput { /* mirrors ListFilters: status, issue_type, assignee, labels_all/any,
                            priority_min/max, text_contains, include_deferred, include_closed, limit, offset */ }
```

### 5.3 Output shapes

```rust
#[derive(Serialize, JsonSchema)] #[serde(untagged)]
pub enum ToolOutput {
    Issue(Issue),
    Issues(Vec<Issue>),
    Id(IdOnly),                         // quick-create
    Counts(Vec<CountBucket>),
    Close(CloseOutcome),               // close --suggest-next -> newly_unblocked
    Deps(Vec<Dependency>),
    Tree(DepTree),
    Cycles(Vec<Vec<String>>),
    Sync(SyncOutput),                  // ExportReport | ImportReport
    Diagnostics(DiagnosticReport),
    Error(StructuredError),            // always valid JSON even on error (FR-11)
}
#[derive(Serialize, JsonSchema)] pub struct IdOnly { pub id: String }

// SyncOutput (G-23a): mcp-owned wrapper over the two model report DTOs (re-exported from unblock-model).
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutput { Export(ExportReport), Import(ImportReport) }
```

### 5.4 Resources

```
unblock://issues/{id}        -> Issue            (FR-4)
unblock://issues/ready       -> Vec<Issue>       (default-complete ready set; agent entrypoint)
unblock://issues/blocked     -> Vec<Issue>
unblock://capabilities       -> Capabilities     (FR-12; tools/resources/prompts + error/exit-code map)
unblock://schema             -> SchemaBundle     (FR-12; JsonSchema per tool I/O)
```

```rust
#[derive(Serialize, JsonSchema)]
pub struct Capabilities {
    pub contract_version: String,                  // bumped when any tool schema changes (FR-12)
    pub tools: Vec<ToolDescriptor>,
    pub resources: Vec<ResourceDescriptor>,
    pub prompts: Vec<PromptDescriptor>,
    pub error_codes: Vec<ErrorCodeDescriptor>,     // code -> exit_code, retryable, hint
}
```

### 5.5 Prompts

```
triage                 -> guided triage workflow
plan_next_work         -> drives ready -> claim selection
close_with_suggestions -> close + surface newly-unblocked
```

### 5.6 Error mapping at the MCP boundary

Any `EngineError` → `StructuredError` (§2.4) attached as rmcp tool error **data** (`code`/`message`/`hint`/`retryable`/`context`), parallel to the CLI 0–8 exit codes. A failed tool call still returns **valid JSON** (`ToolOutput::Error`). Oversized/invalid args are rejected by schemars validation before reaching the engine (NFR-18); blast radius confined to the workspace.

---

## 5b. CLI lifecycle surface — `unblock-cli` (L7)

`unblock-cli` owns the `unblock` binary and depends on `unblock-mcp` (§0.1). Lifecycle/ops commands (NOT the issue-data verbs, which go through MCP tools / the engine): the v1 command set is **`serve, migrate, doctor, version, init, agents, update`** — all lifecycle/ops. (This widens the PRD D3 list, which named only `serve/migrate/doctor/version`; `init`/`agents`/`update` ship in cli at M3 per the cli plan / T3.1 / T3.6. The cli Q2 startup-vs-runtime partitioning is the only remaining cli open item.)

```rust
// commands/update.rs — the v1 self-update command (FR-25 / D17). Command token is `unblock update`
// EVERYWHERE (Command::Update, UpdateArgs, help snapshots). The Cargo FEATURE is named "self-update"
// (the "self-update" feature enables the "unblock update" command — feature name ≠ command name by design).
pub struct UpdateArgs { /* --check, --version <tag>, --yes */ }
```

- **Self-update seam (FR-25, D17):** the `unblock update` command uses **`axoupdater` as a library dependency of `unblock-cli`** (NOT a separate `unblock-update` crate). Updates are verified via **GitHub artifact attestations** (NFR-17), not an embedded key. Gated behind the **`self-update`** Cargo feature (default-on); `--no-default-features` drops the feature and thus the `unblock update` command and its network surface (CF-K).
- The CLI maps each `EngineError`/boundary error → `StructuredError` and exits with its 0–8 exit code (§2.4); structured output to stdout, diagnostics to stderr (NFR-14).

---

## 6. Conformance rules

1. Per-crate plans MUST use these exact type/field/signature shapes; deviations require amending this file first.
2. No backend (libsql) type may appear in any public API outside `unblock-storage`'s private impl (NFR-15).
3. `content_hash` is `#[serde(skip)]` and recomputed on load; it is the import idempotency key (FR-26).
4. All mutations flow through `Session` (FR-9) and the single write permit (D14); reads never acquire it (FR-10).
5. Every error surfaced at L7 maps to exactly one `ErrorCode` and one 0–8 exit code per §2.3 (golden-snapshot pinned).
6. MCP tool count stays ≤ 8; new domain surface in v1.1 extends existing tools by discriminator before adding tools (RK-3).
7. `forbid(unsafe_code)`, no git crate / `Command::new("git")` anywhere (NFR-6/NFR-9); network/TLS only behind the non-default `remote` feature (D15).
