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

Pure types, no I/O. Derives target: `Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema` unless noted. `chrono::{DateTime, Utc}` for time. Open enums (`Status`/`IssueType`/`DependencyType`/`EventType`) keep a `Custom(String)` tail variant and **hand-roll all three of `Serialize` (via `as_str`), `Deserialize` (unknown string → `Custom`), and `JsonSchema` (a plain string)** — they derive neither `Serialize`/`Deserialize`/`JsonSchema` nor carry any `#[serde(...)]` attribute (a `#[serde(untagged)]` `Custom` would conflict with the hand-rolled `Deserialize`). Each also has `as_str`/`Display`/`FromStr` (`Err = unblock_error::ModelError`).

### 1.1 Status

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]   // Serialize/Deserialize/JsonSchema hand-rolled (as string)
pub enum Status {
    #[default] Open,
    InProgress,            // serializes "in_progress"
    Blocked,
    Deferred,
    Draft,
    Closed,
    Tombstone,
    Pinned,
    Custom(String),        // open-enum tail — NOT `#[serde(untagged)]`
}
// Serialize: hand-rolled `serialize_str(self.as_str())` (snake_case known strings; the raw
//   original-case string for Custom). Deserialize: hand-rolled — case-insensitive known parse,
//   unknown string -> Custom(original-case). JsonSchema: hand-rolled as a plain `string`.
//   No derived serde and no `#[serde(...)]` attribute (a `#[serde(untagged)]` Custom would
//   conflict with the hand-rolled Deserialize — intentionally omitted).
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]   // Serialize/Deserialize/JsonSchema hand-rolled (as string)
pub enum IssueType {
    #[default] Task, Bug, Feature, Epic, Chore, Docs, Question,
    Custom(String),        // open-enum tail — NOT `#[serde(untagged)]`
}
impl IssueType {
    pub fn as_str(&self) -> &str;
    pub const fn is_standard(&self) -> bool; // !Custom
}
// Serialize: hand-rolled `serialize_str(self.as_str())` (snake_case known; raw original-case for
//   Custom). Deserialize: hand-rolled — case-insensitive known parse, unknown -> Custom(original-case).
//   JsonSchema: hand-rolled string. No derived serde / no `#[serde(...)]` attribute.
// epic participates in EpicStatus rollups [v1.1]. Default = Task.
```

### 1.4 DependencyType

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]   // Serialize/Deserialize/JsonSchema hand-rolled (as string)
pub enum DependencyType {
    Blocks, ParentChild, ConditionalBlocks, WaitsFor,   // <- the four that gate ready-work
    Related, DiscoveredFrom, RepliesTo, RelatesTo,
    Duplicates, Supersedes, CausedBy,
    Custom(String),        // open-enum tail — NOT `#[serde(untagged)]`
}
impl DependencyType {
    pub fn as_str(&self) -> &str;            // "blocks" | "parent-child" | ...
    pub const fn affects_ready_work(&self) -> bool; // Blocks|ParentChild|ConditionalBlocks|WaitsFor
    pub const fn is_blocking(&self) -> bool;        // same set as affects_ready_work
}
// Serialize: hand-rolled `serialize_str(self.as_str())` (kebab-case known strings). Deserialize:
//   hand-rolled — case-insensitive kebab-case known parse, else Custom (NOTE: unlike Status/IssueType,
//   DependencyType lowercases the value BEFORE storing it in Custom). JsonSchema: hand-rolled string.
//   No derived serde / no `#[serde(...)]` attribute. DiscoveredFrom is the agent flywheel edge.
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
  - **Canonical byte stream (NORMATIVE — frozen; Q4 = KEEP, the model crate plan `unblock-model.md`).** Each field above is appended to the SHA-256 stream as `bytes(value) ++ 0x00` (a single `0x00` separator after *every* field, including the last). `None` → empty string `""`; a `false` boolean flag → empty string `""` while a `true` flag → its label (`"pinned"` / `"template"`); `priority` is the decimal integer of `priority.0` (e.g. `"2"`, **not** `"P2"`); the final digest is rendered lowercase `{:02x}` per byte (64 hex chars). **After `is_template`** the stream appends a **frozen 17-field Go-bd zero-value padding tail** so a Rust `content_hash` stays byte-for-byte identical to a `bd`-exported hash (FR-26 / D16 idempotent one-shot `bd` import). The tail, in exact order, is: `"" , false-flag("crystallizes")→"" , "" , "" , "0" , "" , "" , "" , "" , "" , "" , "" , "" , "" , "" , "" , ""` — i.e. **15 empty strings, one `"0"` (the Go `timeout` duration zero value) at position 5, and the single `false` crystallizes-flag at position 2** (which, being `false`, also serializes as `""`). Go-bd source-field correspondence (documentation only — Rust does not model these): `quality_score, crystallizes, await_type, await_id, timeout, holder, hook_bead, role_bead, agent_state, role_type, rig, mol_type, work_type, event_kind, actor, target, payload`. This tail is **frozen** and is locked by a golden `insta` hash snapshot (`tests/golden_hash.rs`); changing it would break `bd` import idempotency and is a breaking change requiring a spine amendment.
- **`sync_equals(&self, other) -> bool`** — semantic equality for import/export boundaries. Compares the full synced payload (incl. `due_at`, `defer_until`, tombstone fields, compaction fields, and relations **order-independent**: labels deduped+sorted; deps and comments sorted by a fixed key tuple). Treats `compaction_level == None` as `0`. Ignores volatile audit-only fields. This is the import "is this line a no-op?" predicate, not derived `PartialEq`.
- **Tombstone** — delete sets `status = Tombstone` + `deleted_at`/`deleted_by`/`delete_reason` (and `original_type` preserved). `is_expired_tombstone(retention_days: Option<u64>) -> bool` (TTL helper). Invariant: **import NEVER resurrects a tombstone** — a non-tombstone JSONL line for an id that is tombstoned in the DB is rejected/skipped, not applied.

### 1.9 Validation + shared contract types

```rust
pub struct IssueValidator; // pure; title 1..=500, priority 0..=4, enum coherence, reparent-cycle check input.
impl IssueValidator { pub fn validate(issue: &Issue) -> Result<(), unblock_error::ModelError>; }

// Single-home actor bounding (the model owns the rule; config is its v1 caller — Seam A, lands T1.3).
// Bounds the RESOLVED actor: <= ACTOR_MAX_CHARS = 200 chars (chars().count(), NOT bytes) + rejects NUL
// + rejects other control chars. CLI/MCP become later callers; the rule lives once here.
pub fn validate_actor(actor: &str) -> Result<(), unblock_error::FieldError>;

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

Each crate's enum implements **`unblock_error::CodedError`** (the merged L0 bridge: `code()` required; `hint()`/`retryable()`/`context()` defaulted, with `retryable()` tracking `code().is_retryable()`) — this is the concrete mechanism that satisfies the older inherent-`code()` prose in the sketch above, and it is what lets the boundary build a `StructuredError` uniformly (via the blanket `From<&E>` / `StructuredError::from_coded`). Upward composition uses snafu source nesting; the engine's error is the union surfaced to L7.

**`ModelError` aggregate validation (D-E1 — NORMATIVE).** `unblock-error` owns the one concrete per-crate enum that `unblock-model` returns (spine §1.1/§1.2/§1.9: `Status::FromStr`, `Priority::FromStr`, `IssueValidator::validate`). It keeps **scalar** variants for the single-field `FromStr` paths *and* one **aggregate carrier** that holds every failure an `IssueValidator::validate` run found, so the boundary still emits exactly one `ErrorCode` while preserving multi-field detail (FR-11 agent self-correction):

```rust
// unblock-error
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FieldError { pub field: String, pub reason: String }

#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum ModelError {
    InvalidPriority { value: String },          // -> ErrorCode::InvalidPriority
    InvalidStatus   { value: String },          // -> ErrorCode::InvalidStatus
    InvalidType     { value: String },          // -> ErrorCode::InvalidType
    InvalidId       { id: String },             // -> ErrorCode::InvalidId
    RequiredField   { field: &'static str },    // -> ErrorCode::RequiredField (empty/whitespace title)
    ReparentCycle   { path: String },           // -> ErrorCode::CycleDetected
    ValidationFailed { fields: Vec<FieldError> }, // -> ErrorCode::ValidationFailed (aggregate carrier)
}
```

The §1.9 signature `IssueValidator::validate -> Result<(), unblock_error::ModelError>` stays VERBATIM (single error — the aggregate lives **inside** it: a multi-failure run returns `Err(ModelError::ValidationFailed { fields })`; a single-field `FromStr` returns the matching scalar variant).

**`ConfigError` — the per-crate snafu enum owned by `unblock-config` (L4).** Following the §2.1 pattern, `unblock-config` defines its own `#[derive(Debug, Snafu)] pub enum ConfigError` implementing `unblock_error::CodedError` (`code() -> ErrorCode`); like every per-crate enum it maps each variant to **an existing `ErrorCode`** from §2.2 (it never introduces a new code). The **T1.3a minimal v1 variant set** is intentionally small (the full layered-resolution variants — parse/unknown-key/invalid-value/credential paths — are added **additively at T1.3**, mapping to the same exit-7/exit-8 codes). v1 minimal map (every right-hand side is a real §2.2 variant):

```rust
// unblock-config (per-crate snafu enum; concrete field set in the crate plan)
#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum ConfigError {
    WorkspaceNotFound { start: PathBuf },          // -> ErrorCode::NotInitialized  (no .unblock/ found by upward discovery)
    DbOpenFailed { source: StorageError },         // -> source.code()              (wraps open_local; typically Backend -> DatabaseError)
    MigrationFailed { source: StorageError },      // -> source.code()              (wraps migrate(); Migration/SchemaMismatch -> SchemaMismatch, Backend -> DatabaseError)
    ActorUnresolved,                               // -> ErrorCode::RequiredField   (no actor from UNBLOCK_ACTOR / $USER / default)
}
// ConfigError implements the L0 bridge trait (NOT a bespoke inherent code()), matching the landed
// StorageError convention — the L7 blanket `From<&E: CodedError> for StructuredError` needs the trait impl.
impl unblock_error::CodedError for ConfigError {
    fn code(&self) -> unblock_error::ErrorCode {
        match self {
            Self::WorkspaceNotFound { .. } => unblock_error::ErrorCode::NotInitialized,
            // forward the inner StorageError's own code — do NOT hardcode (so Backend stays DatabaseError)
            Self::DbOpenFailed { source } | Self::MigrationFailed { source } => source.code(),
            Self::ActorUnresolved => unblock_error::ErrorCode::RequiredField,
        }
    }
}
```

> **ErrorCode-mapping rationale (T1.3a, for design Review to validate).**
> - `WorkspaceNotFound` → **`NotInitialized`** (exit 2). Closest *existing* code: discovery failing means there is no
>   initialized `.unblock/` workspace at/above `start` — the precise meaning of `NOT_INITIALIZED` ("Workspace not
>   initialized"). The exit-7 `ConfigNotFound` ("Config **file** not found") is narrower (a missing `config.toml`,
>   which is *not* an error in T1.3a — config defaults), and `IssueNotFound`/`InvalidId`/`InvalidArgument` are about
>   issue ids, not workspaces (and `InvalidArgument` is not in the §2.2 set at all). `NotInitialized` is the honest
>   match and pairs with the CLI `init` path (an un-discovered workspace is the "run `init` first" condition).
> - `DbOpenFailed` → **`source.code()`** (the inner `StorageError`'s own code — config does **not** hardcode it).
>   `DbOpenFailed` wraps `LibsqlStorage::open_local`; an open failure is typically `StorageError::Backend →
>   DatabaseError` (exit 2), the backend already absorbed opaquely (spine §3.3). Forwarding means a lock/backend
>   cause keeps its honest code rather than being flattened.
> - `MigrationFailed` → **`source.code()`** (forwarded, NOT hardcoded `SchemaMismatch`). `MigrationFailed` wraps
>   `Storage::migrate()`: a genuine `StorageError::Migration`/`SchemaMismatch` cause forwards to `SchemaMismatch`
>   (consistent with `StorageError::Migration → SchemaMismatch` pinned at T0.5), while a `StorageError::Backend`
>   cause stays `DatabaseError`. Forwarding avoids mis-labelling a backend failure as a schema problem. (Every such
>   StorageError code is exit 2 — the **exit code is unchanged** regardless of which inner variant fired.)
> - `ActorUnresolved` → **`RequiredField`** (exit 4) — the engine requires a non-empty `actor` (spine §4); with the
>   `UNBLOCK_ACTOR → $USER → "unblock"` chain this is effectively unreachable in T1.3a (the final default always
>   resolves), but the variant exists so a future strict-actor mode (T1.3) has its code reserved.
>
> The set grows **additively at T1.3** (e.g. `Parse → ConfigParseError` (the **variant identifier is `Parse`**;
> the `ErrorCode::ConfigParseError` mapping is unchanged), `InvalidValue → ConfigError`,
> `Io → IoError`) — no T1.3a code is renumbered or removed.

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
    // `ErrorCode` is a 1-byte `Copy` enum, so these views take `self` by value (a `&self`
    // signature would trip clippy `trivially_copy_pass_by_ref`).
    pub const fn as_str(self) -> &'static str;        // "ISSUE_NOT_FOUND", ...
    pub const fn exit_code(self) -> u8;               // 0..=8 per table below
    pub const fn is_retryable(self) -> bool;
    //   exact retryable set (no glob): DatabaseLocked | AlreadyClaimed | ValidationFailed
    //   | InvalidStatus | InvalidType | InvalidPriority | RequiredField | AmbiguousId.
    //   (matches error.md; the retryable exit-4 members are the five retryable Validation* members
    //   — ValidationFailed/InvalidStatus/InvalidType/InvalidPriority/RequiredField; PolicyViolation
    //   is exit-4 but non-retryable.)
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

**`ModelError::ValidationFailed { fields }` → `context` mapping (D-E1 — NORMATIVE).** When the aggregate validation carrier (§2.1) is bridged to a `StructuredError` via `CodedError`, its per-field detail surfaces as a `context["fields"]` array of `{ "field": String, "reason": String }` objects (the serialized `FieldError` shape). This keeps the boundary emitting exactly one `ErrorCode` (`VALIDATION_FAILED`) while the agent self-correction surface retains every failing field. The scalar `ModelError` variants (e.g. `InvalidPriority`) carry their single value in `context` as their own keys; only the aggregate uses the `fields` array.

**Message + hint sanitization chokepoint (NORMATIVE).** Every `StructuredError` constructor and builder — `from_code`, `from_coded`, the blanket `From<&E>` bridge, and `with_hint` — routes **both** the `message` and the `hint` through `unblock_error::sanitize_message` (the `\n`/`\t`-preserving terminal sanitizer, ported from `format/text.rs::sanitize_terminal_text`). Both fields are attacker-influenceable — the `hint` in particular folds in suggested ids from `find_similar_ids`, where the not-found id is raw, pre-validation input — so a composed error whose `Display` *or* hint carries ESC/BEL/control bytes still yields a sanitized `.message`/`.hint` no matter which entry point built it (single L0 chokepoint, NFR-14). **`context` values are terminal-safe only via JSON encoding:** the JSON/MCP surface escapes them, but any text/plain render of a context value (e.g. in `unblock-render`) MUST route that value through a sanitizer — this pairs with the `unblock-render` hand-off note. The stricter inline variant that escapes `\n`/`\t` (`sanitize_terminal_inline`) lives in `unblock-render` for single-line display fields, **not** here.

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

// IssuePatch field set = Option B (full enumeration of model-backed updatable `Issue` columns).
// Outer Option = "present in this patch / leave unchanged"; the inner Option on nullable text
// fields distinguishes clear (Some(None)) from set (Some(Some)). Derives `Default` (all-None =
// patch nothing). `defer_until` is intentionally OUT — defer/undefer own it. No invented fields:
// every field below maps 1:1 to a real updatable `unblock-model` Issue column (or to labels/parent
// relations that update_issue mutates), cross-checked against §1.6.
#[derive(Debug, Clone, Default)]
#[allow(clippy::option_option)] // outer=present-in-patch; inner=clear-vs-set on nullable columns
pub struct IssuePatch {
    pub title: Option<String>,                          // NOT NULL column -> plain Option
    // nullable text columns -> Option<Option<String>> (None=leave / Some(None)=clear / Some(Some)=set)
    pub description: Option<Option<String>>,
    pub design: Option<Option<String>>,
    pub acceptance_criteria: Option<Option<String>>,
    pub notes: Option<Option<String>>,
    pub owner: Option<Option<String>>,
    pub external_ref: Option<Option<String>>,
    pub assignee: Option<Option<String>>,
    pub close_reason: Option<Option<String>>,           // close reason (persisted to the close_reason column)
    // plain Option<T> for NOT-NULL / scalar columns
    pub status: Option<Status>,
    pub priority: Option<Priority>,
    pub issue_type: Option<IssueType>,
    pub estimated_minutes: Option<i32>,
    pub due_at: Option<DateTime<Utc>>,
    // label relation ops (applied by update_issue)
    pub labels_add: Vec<String>, pub labels_remove: Vec<String>, pub labels_set: Option<Vec<String>>,
    pub parent: Option<Option<String>>,                 // reparent; cycle-checked
}
```

**`close_reason` persistence (T1.2 Verify-gate, NORMATIVE).** `close_reason` is the nullable-text tri-state (`None` = leave unchanged; `Some(None)` = clear to the column default `''`; `Some(Some(s))` = set). `update_issue` persists it to the existing `close_reason TEXT DEFAULT ''` column (already projected by `ISSUE_COLUMNS`; `create_issue` already binds it from the `Issue`). The engine's `close_with_suggestions(id, reason)` (§4.1) builds a `status = Closed` patch carrying `close_reason` and persists it through `update_issue` under the write permit — the reason is **stored**, not tracing-only. The `close_reason` column is **not** part of the frozen `content_hash` (spine §1.8), so persisting it does not perturb import idempotency (FR-26).

**`StorageError` (storage-owned; the §2.1 sketch made concrete — NORMATIVE).** The full v1 variant set and its `ErrorCode` mapping. It implements `unblock_error::CodedError` (NOT a bespoke inherent `code()`; §2.1 note), so the L7 boundary bridges it via the blanket `From<&E>` like every other crate enum. `Migration` is defined **concretely and minimally, model-backed**: `Migration { from: i32, to: i32, reason: String }` (`from`/`to` are `PRAGMA user_version` values, `i32` to match the schema-version type). `Backend { source: BackendOpaque }` absorbs the libsql error opaquely — no libsql type is ever public (spine §6 rule 2). `BackendOpaque` sanitizes its message **at construction** via `unblock_error::sanitize_message` and exposes only `Debug`/`Display`.

```rust
#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum StorageError {
    IssueNotFound { id: String },        // -> IssueNotFound
    AmbiguousId { id: String },          // -> AmbiguousId
    IdCollision { id: String },          // -> IdCollision
    InvalidId { id: String },            // -> InvalidId
    DatabaseLocked,                      // -> DatabaseLocked
    SchemaMismatch { found: i32, expected: i32 },  // -> SchemaMismatch
    NotInitialized,                      // -> NotInitialized
    AlreadyInitialized,                  // -> AlreadyInitialized
    AlreadyClaimed { id: String, by: String },     // -> AlreadyClaimed (FR-2 loser; `by` = current holder, re-read in-tx)
    CycleDetected { path: String },      // -> CycleDetected
    DependencyNotFound,                  // -> DependencyNotFound
    HasDependents { id: String },        // -> HasDependents
    SelfDependency,                      // -> SelfDependency
    DuplicateDependency,                 // -> DuplicateDependency
    Backend { source: BackendOpaque },   // -> DatabaseError (libsql absorbed; opaque)
    Migration { from: i32, to: i32, reason: String }, // -> SchemaMismatch (model-backed)
    IntegrityFailed { messages: Vec<String> },        // -> DatabaseError (PRAGMA integrity_check failures)
}
// impl unblock_error::CodedError for StorageError { code() per the map; retryable() = default
//   (code().is_retryable()); context() surfaces the structured payload agents need —
//   AlreadyClaimed{by} -> context["holder"]; CycleDetected{path} -> context["cycle_path"];
//   SchemaMismatch{found,expected}; IssueNotFound{id}; HasDependents{id}; IntegrityFailed{messages}; ... }
//
// pub struct BackendOpaque(String); // private inner; from_message() runs sanitize_message at construction;
//   Debug + manual Display (sanitized text) + impl std::error::Error. No From<libsql::Error> until T0.6.
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

#### 3.2.1 Method semantics (T0.6, normative — source-verified vs `temp/beads_rust-main/src/storage/sqlite.rs`)

The terse signatures above are made concrete here. The libsql impl reproduces the *behaviour* of the
original `bd` SQLite storage (cited line ranges), not its monolith shape. The cited contradictions
between an earlier prose description and the source are resolved **in favour of the source**.

- **`create_issue` (sqlite.rs:2206–2238) — NO content-hash dedup.** Create guards only on (a) **id
  collision** → `IdCollision{id}` and (b) **`external_ref` collision** → backend error; it computes and
  **stores** `content_hash` but **never short-circuits on it**. The FR-26 idempotency note belongs to the
  **import path** (`unblock-sync`), not to `create_issue`. Inserts the row + `Event(Created)` in one tx;
  hierarchical ids bump the `child_counters` row in-tx; per-relation `Event(LabelAdded)`/
  `Event(DependencyAdded)`/`Event(Commented)` are recorded for any seeded relations.
- **`update_issue` (sqlite.rs:2496–2509, 2572–2870) — per-field event granularity; empty diff is a
  full skip.** An empty patch (or one that changes nothing) returns the issue unchanged and writes **no
  `SET`, no `updated_at`, no `Event`** (`if set_clauses.is_empty() { return Ok }`). `updated_at` advances
  and `content_hash` is recomputed **only** when at least one stored column changes. Per-field events
  (see the EventType-per-mutation table below) are emitted **only when the field's value actually
  changes** (e.g. patching `status`→its current value emits no `StatusChanged`).
- **`delete_issue` (sqlite.rs:2952–3015) — delegate to the model `Issue::into_tombstone`** (which sets
  `status = Tombstone` + `deleted_*` and preserves `original_type`), then bump `updated_at = now` and
  recompute `content_hash`. **`Event(Deleted)` is written ONLY when the prior status was non-terminal**
  (`!was_terminal`): tombstoning an already-`Closed` issue records **no** `Deleted` event. An
  already-tombstone target is a **no-op `Ok`**. One tx. (The model-B schema drops the original's
  `close_metadata` table, so its cleanup `DELETE` is removed.)
- **`claim_issue` (sqlite.rs:2888–2935) — assignee-ONLY guard, no status predicate.** The atomic claim
  `UPDATE` carries `WHERE id = ? AND (assignee IS NULL OR TRIM(assignee) = '' OR assignee = ?<actor>)`
  — there is **no** `status NOT IN (...)` predicate. 0 rows affected → re-`SELECT` the current holder
  **within the same tx** → `AlreadyClaimed{by}`. A **same-actor re-claim short-circuits BEFORE the
  `UPDATE`** (idempotent `Ok`, no `updated_at`, no event).
- **`ready_issues` (sqlite.rs:4988–5048) — mirrors `idx_issues_ready`.**
  `status = 'open' AND id NOT IN <live blocked set> AND (defer_until IS NULL OR defer_until <= now) AND
  (pinned = 0 OR pinned IS NULL) AND (ephemeral = 0 OR ephemeral IS NULL) AND
  (is_template = 0 OR is_template IS NULL)`. `ORDER BY priority ASC, created_at ASC, id ASC`. The
  original's `AND id NOT LIKE '%-wisp-%'` wisp filter is **DROPPED** (Miguel). Default-complete unless
  `limit` set; the hybrid re-rank stays in the engine (CF-11).
- **`blocked_issues` (sqlite.rs:5720–5746, 5886–5912, 6076–6090, 6258–6311, 6371–6398) — `status NOT
  IN ('closed', 'tombstone')` (INCLUDES `in_progress`/`deferred`); blocked via THREE live-computed
  passes, NOT one 4-type SQL.** An issue is blocked iff it is in the union of (1) **direct 3-type
  blockers** — a `'blocks'`/`'conditional-blocks'`/`'waits-for'` edge on a blocker that is not
  `closed`/`tombstone` (`external:%` and template blockers excluded; `LEFT JOIN` so a missing blocker
  id is treated as unresolved); (2) **open epic-rollup children** — a `'parent-child'` edge where the
  parent issue's `issue_type = 'epic'` and the child's `status NOT IN ('closed', 'tombstone')` marks
  the **parent** blocked (a separate pass, not folded into the direct query); and (3) **transitive
  children of blocked parents (NORMATIVE)** — a **fixpoint** down-propagation over the `'parent-child'`
  tree: starting from the blocked set of passes (1)+(2), every issue with a `'parent-child'` edge to an
  already-blocked **parent** is itself blocked, iterating until no new id is added (mirrors
  `propagate_blocked_parents`, sqlite.rs:6369–6398; edges per `load_local_parent_child_edges_impl`,
  sqlite.rs:6165–6191 — `parent = depends_on_id`, `child = issue_id`, `external:%` excluded on both
  ends; the propagation is purely structural over the edge). `ORDER BY priority ASC, created_at DESC,
  id ASC` (Miguel).
- **`search_issues` (sqlite.rs:4543–4727) — substring `instr(lower(col))` over title+description+id.**
  The needle is lowercased and matched with
  `instr(lower(title), ?) > 0 OR instr(lower(description), ?) > 0 OR instr(lower(id), ?) > 0` (no LIKE
  escaping on the needle). A `filters.text_contains` FILTER (distinct from the search needle) keeps the
  `LIKE ? ESCAPE '\'` form over `title`. Cap **50** when `filters.limit` is unset. `ORDER BY priority
  ASC, created_at DESC, id ASC` (the no-explicit-sort tail). The `sort`/`reverse` branches are deferred
  to v1.x; FTS5 to v1.3.

**EventType-per-mutation (the T0.7 oracle).** Model `EventType` = 15 named (Created, Updated,
StatusChanged, PriorityChanged, AssigneeChanged, Commented, Closed, Reopened, DependencyAdded,
DependencyRemoved, LabelAdded, LabelRemoved, Compacted, Deleted, Restored) + `Custom` — **no `Deferred`,
no `Claimed`**. Each mutation emits exactly:

| Mutation | Event(s) (in order) |
|---|---|
| create | `Created` (+ `LabelAdded`/`DependencyAdded`/`Commented` per seeded relation) |
| update `title` | `Updated` |
| update `status` (changed) | `StatusChanged` (+ `Closed` on →`closed` from non-terminal · + `Reopened` on terminal→non-terminal · + `Deleted` on →`tombstone` from non-terminal) |
| update `priority` (changed) | `PriorityChanged` |
| update `assignee` (changed) | `AssigneeChanged` |
| update body fields (`description`/`design`/`acceptance_criteria`/`notes`/`owner`/`estimated_minutes`/`external_ref`/`issue_type`/`source_repo`/`agent_context`/`close_reason`) | **none** |
| no-op update (nothing changed) | **none** |
| claim (won) | `AssigneeChanged` + `StatusChanged` |
| claim (same-actor re-claim) | **none** |
| defer / undefer | `Updated` |
| delete (tombstone from non-terminal) | `Deleted` |
| delete (from terminal / already-tombstone) | **none** |
| add / remove dependency | `DependencyAdded` / `DependencyRemoved` |
| comment | `Commented` |
| add / remove label | `LabelAdded` / `LabelRemoved` |

### 3.3 libsql impl notes (normative for `unblock-storage`)

- **WAL** journal mode; **`busy_timeout = 5000 ms` (native, `Connection::busy_timeout`)** — `const BUSY_TIMEOUT_MS: u64 = 5000`. This is the **sanctioned INVERSE of beads**, which set `busy_timeout = 0` + a hand-rolled flock + sleep backoff to dodge *frankensqlite*'s hot-spin. libsql ships **real SQLite**, whose native `busy_timeout` is sleep-based (it blocks, it never spins), so a non-zero native timeout resolves fsqlite-243 **by construction**. Do **not** port the beads `=0`/backoff/flock machinery.
- **Pragmas (read schema.rs:606–643):** `foreign_keys = ON`, `synchronous = NORMAL`, `temp_store = MEMORY`, `cache_size = -8000`, `journal_size_limit = 33554432` on every connection; the **WAL-only** pragmas — `journal_mode = WAL` and **`wal_autocheckpoint = 0`** (+ a **manual `wal_checkpoint(TRUNCATE)`** on fresh-bootstrap) — are applied **on the file-backed path only**. A shared-cache `:memory:` DB **cannot** use WAL (it always reports `journal_mode = memory`), so asserting WAL there is both a no-op AND an intermittent "API misuse"/`DatabaseLocked` flake under parallel opens; it is skipped for `open_in_memory`. **Periodic in-flight checkpointing (RESOLVED at T0.8):** a **passive** `wal_checkpoint(PASSIVE)` fires on the **held write connection** every **50 committed mutations** (`CHECKPOINT_EVERY_N_MUTATIONS = 50`) — **never `TRUNCATE` in the write path** (an exclusive lock there would manufacture contention). This is distinct from the one-shot fresh-bootstrap `wal_checkpoint(TRUNCATE)` above: that runs **once at migration time on an empty DB** (no concurrent writers to block), whereas the steady-state write path uses PASSIVE only. PASSIVE folds committed frames back into the main DB without blocking, so the WAL file's space is reused in place and stays **bounded** (it does not shrink to zero — PASSIVE reuses, it does not truncate). The T0.8 contention lab asserts the `-wal` sidecar stays bounded under sustained multi-instance contention with this cadence on, and a `#[ignore]`d negative control shows it **breaches** the ceiling with it off.
- **Transactions:** every **mutating** tx uses **`BEGIN IMMEDIATE`** (`transaction_with_behavior(TransactionBehavior::Immediate)`); reads use the default **Deferred** behaviour.
- **OQ-5 (RESOLVED — Miguel + design Review): two connections, not one.** `LibsqlStorage` holds a **serialized WRITE connection** (writes go through `BEGIN IMMEDIATE`; the engine's D14 `Semaphore` serializes writers at L5) **AND a separate READ connection** for the read fast path, so WAL gives concurrent MVCC reader snapshots vs the single writer (FR-10). For `open_in_memory`, both connections must see the **same** in-memory database — a bare `:memory:` is connection-private — so the impl opens a **named shared-cache in-memory URI** (`file:<unique>?mode=memory&cache=shared`, valid because libsql-ffi compiles SQLite with `SQLITE_USE_URI`); this path is **shared-cache, NOT WAL** (see the pragmas bullet). Public constructors: `open_local(&Path)` and `open_in_memory()`. (Earlier OQ-5 wording said "single connection" — superseded.) **Real WAL + native `busy_timeout` concurrency is validated by the T0.8 contention lab on a FILE DB, not an in-memory one** (the in-memory shared-cache path cannot exercise WAL).
- **Default build = local file / bundled only.** Remote/embedded-replica is a non-default Cargo feature `remote` (TLS/HTTP transitive surface kept off the normal path; D15/NFR-10). When `remote`, app-level jittered retry (`backon`/`tokio-retry`, **not** archived `backoff 0.4`) guards only that path; `wiremock` for tests.
- Mutations are **transactional**: issue rows + audit `Event` rows committed together inside one tx.
- libsql/SQLite errors are absorbed into `StorageError::Backend { .. }` (opaque) and surfaced only as `ErrorCode` — no backend type in the public API. The single `From<libsql::Error>` bridge maps `SqliteFailure(code, _)` with `(code & 0xff) ∈ {5, 6}` (SQLITE_BUSY / SQLITE_LOCKED) to `DatabaseLocked`; everything else becomes `Backend{..}` (the catch-all arm is required — `libsql::Error` is `#[non_exhaustive]`).
- Backed by a backend-independent **contract suite** (NFR-16) exercising every trait method; the contention lab (NFR-3, M0 gate) drives N concurrent writers asserting correctness + no 100% CPU hot-spin.

---

## 4. Engine session API — `unblock-engine` (L5)

The single mutation home (FR-9). Composes storage + policy + (optional) sync/health. MCP and CLI are thin adapters; behaviour cannot drift. **In-process write serialization via a tokio `Semaphore(1)`** (D14); reads bypass it (FR-10).

**Workspace-open ownership (CF-D — normative):** discovery of the workspace dir (named **`.unblock` OR `_unblock`** — the monorepo alias for dot-dir-hostile environments, FORK-2/D8) and construction of the `Arc<dyn Storage>` is owned by **`unblock-config`**. A **symlinked** workspace dir is allowed but **canonicalized**, and the resolved `db_path`/`jsonl_path` are **confined within the canonicalized `unblock_dir`** (FORK-3, NFR-18 blast-radius — never rejected outright). `WorkspaceContext`, `ResolvedContext`, `ResolvedConfig`, and `ConfigPaths` are **DEFINED in `unblock-config`** (not engine). `unblock-config` exposes **two facades** (G-5 option b), each with a T1.3-additive `_with_cli` overload (FORK-1):

```rust
// in unblock-config:

// ConfigPaths is config-owned (DEFINED in unblock-config) — config OWNS path resolution from T1.3a
// (single source of truth). Both contexts below embed it by value. Derived from the discovered
// workspace + the ResolvedConfig filenames; the concrete shape is pinned by the unblock-config crate
// plan (`docs/plans/crates/unblock-config.md` §2/§3).
#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub unblock_dir: PathBuf,   // the discovered/created `.unblock/` directory (= workspace_dir.join(".unblock"))
    pub db_path: PathBuf,       // unblock_dir.join(ResolvedConfig.db_filename)    (T1.3a default "unblock.db")
    pub jsonl_path: PathBuf,    // unblock_dir.join(ResolvedConfig.jsonl_filename) (T1.3a default "issues.jsonl")
}

// ResolvedConfig is config-owned (DEFINED in unblock-config) — the resolved, validated config
// VALUES the engine/Session reads (NOT paths, NOT actor: actor is the top-level context field per
// §4.1; paths live in ConfigPaths). Both contexts below embed it by value. Its concrete v1 field set
// is pinned by the unblock-config crate plan (`docs/plans/crates/unblock-config.md` §2/§3): it is
// DEFAULTED in the T1.3a minimal subset and RESOLVED for real (layered TOML/env/CLI) at T1.3.
pub struct ResolvedConfig { /* config-owned; see the unblock-config crate plan for the pinned field set */ }

// (1) resolve-only — NO storage; discovery + resolved config only (for `where`, doctor pre-checks,
//     completions, and anything that must not open/migrate the DB).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    pub workspace_dir: PathBuf,        // project root (the dir that CONTAINS `.unblock/`)
    pub actor: String,                 // authoritative actor (§4.1) — NOT inside ResolvedConfig
    pub config: ResolvedConfig,        // config-owned (DEFINED in unblock-config)
    pub paths: ConfigPaths,            // config-owned: resolved `.unblock/` + db/jsonl paths (T1.3a)
}
pub async fn open_workspace(start: &Path) -> Result<ResolvedContext, ConfigError>;

// (2) storage-bearing — discovery + open/migrate libsql; the field is NON-OPTIONAL.
#[derive(Clone)]
pub struct WorkspaceContext {
    pub storage: Arc<dyn Storage>,     // NON-OPTIONAL (G-5): always present once built
    pub workspace_dir: PathBuf,        // project root (the dir that CONTAINS `.unblock/`)
    pub actor: String,                 // authoritative actor (§4.1) — NOT inside ResolvedConfig
    pub config: ResolvedConfig,        // config-owned (DEFINED in unblock-config)
    pub paths: ConfigPaths,            // config-owned: resolved `.unblock/` + db/jsonl paths (T1.3a)
}
pub async fn open_with_storage(start: &Path) -> Result<WorkspaceContext, ConfigError>;

// (3) T1.3-ADDITIVE CLI overloads (FORK-1 — OVERLOAD model). The `&Path` facades above are PERMANENT and
//     UNCHANGED; each DELEGATES to its `_with_cli` form passing `start` as the WALK-UP START parameter, NOT as
//     `cli.dir`: `discover_unblock_dir(Some(start), &CliOverrides::default())`. (`cli.dir` is the EXPLICIT
//     `--dir`/`UNBLOCK_DIR` override — NO walk-up; `start` is the separate walk-up START — so the facades must
//     leave `cli.dir` unset and thread `start` through the start param.)
//     `CliOverrides` (the typed top precedence layer, config-owned) threads --dir/--db/--actor/--output-format
//     through resolution. These return the SAME result types the engine consumes — NO signature swap, NO break.
pub async fn open_workspace_with_cli(cli: &CliOverrides) -> Result<ResolvedContext, ConfigError>;
pub async fn open_with_storage_with_cli(cli: &CliOverrides) -> Result<WorkspaceContext, ConfigError>;
```

> **NOTE (T1.3a minimal subset — build split, NORMATIVE sequencing).** The **T1.3a** task delivers EXACTLY these
> types (`WorkspaceContext`, `ResolvedContext`, `ResolvedConfig`, `ConfigPaths`, `ConfigError`) plus the two facades,
> with `ResolvedConfig` populated from **defaults** (no layered resolution yet). **Config OWNS path resolution from
> T1.3a — the single source of truth:** `paths.unblock_dir` is the discovered/created `.unblock/` directory
> (`= workspace_dir.join(".unblock")`), and `db_path`/`jsonl_path` are **derived** from `unblock_dir` + the
> `ResolvedConfig` filenames (`db_filename`/`jsonl_filename`). `workspace_dir` (the project **root** that contains
> `.unblock/`) and `paths.unblock_dir` (= `workspace_dir/.unblock`) are **distinct and both intentional**. The task
> performs **workspace upward discovery** from `start` (a dir named **`.unblock` OR `_unblock`** — the monorepo alias
> for dot-dir-hostile environments, FORK-2/D8), libsql open + migrate via
> `unblock_storage::LibsqlStorage::open_local`, `Arc<dyn Storage>` construction, path resolution into `ConfigPaths`,
> and actor resolution (T1.3a: `UNBLOCK_ACTOR` env → `$USER` → `"unblock"`). The **full layered resolution**
> (CLI > env `UNBLOCK_*` > project `config.toml` > defaults) lands **additively at T1.3** — it replaces the
> defaulting internals and **adds the `_with_cli` facade overloads** (FORK-1, see the facade-signature note),
> extends actor precedence to the global order (FORK-4: `--actor` > `UNBLOCK_ACTOR` > `config.toml [actor]` > `$USER`
> > `"unblock"`), and **canonicalizes a symlinked workspace dir and confines** `db_path`/`jsonl_path` within the
> canonicalized `unblock_dir` (FORK-3, NFR-18 blast-radius) — touching **no public type or `&Path` signature**
> pinned here. T1.3a sequences **before** T1.2: the engine *consumes* config's `WorkspaceContext`, and config is
> **L4** so it **cannot** depend on the engine at **L5** (`cargo xtask check-layering` would reject that back-edge).

> **NOTE (facade signatures — OVERLOAD model, FORK-1, NORMATIVE).** The two `&Path` facades above
> (`open_workspace(start: &Path)` / `open_with_storage(start: &Path)`) are **PERMANENT and UNCHANGED** — T1.3a ships
> exactly them, and T1.3 keeps them verbatim. The richer CLI-override forwarding the config crate plan describes (a
> `&CliOverrides` parameter that threads `--dir`/`--db`/`--actor`/`--output-format` down through resolution) lands at
> T1.3 as **two ADDITIVE overloads** — `open_workspace_with_cli(cli: &CliOverrides)` /
> `open_with_storage_with_cli(cli: &CliOverrides)` (block item (3) above) — **not** as a signature swap. The `&Path`
> facades **delegate** to their `_with_cli` form passing `start` as the **walk-up START** parameter via
> `discover_unblock_dir(Some(start), &CliOverrides::default())` — **NOT** as `cli.dir`. (`cli.dir` is the EXPLICIT
> `--dir`/`UNBLOCK_DIR` override, which **does not walk up**; the `&Path` facades want the walk-up-from-`start`
> behaviour, so they leave `cli.dir` unset and route `start` through the discovery start parameter.) Every existing
> caller keeps compiling unchanged and the engine (which binds to the **result** type
> `WorkspaceContext`, never to a facade signature) is unaffected. (This reconciles the spine `&Path` ↔ config-plan
> `&CliOverrides` drift by **overload addition**, not by sequencing/swapping the parameter — the `&Path` API never
> goes away.)

**`unblock-engine` CONSUMES** a `WorkspaceContext` — it does **not** construct storage itself, and never sees an `Option<Arc<dyn Storage>>`. `Session::open` takes the already-built storage-bearing context; because `storage` is non-optional there is no unwrap and no None-path mismatch. The resolve-only `ResolvedContext` is for callers that must not touch the DB.

**Result DTO ownership (CF-A — normative):** `CloseOutcome`, `ImportReport`, `ExportReport`, `CountBucket`, `GraphEdge`, `DepTree`, and `DiagnosticReport`/`DiagnosticFinding`/`DiagnosticKind` are **defined in `unblock-model` §1.10** and **re-exported** by `unblock-engine` (`pub use unblock_model::{CloseOutcome, ImportReport, ExportReport, CountBucket, GraphEdge, DepTree, DiagnosticReport, DiagnosticFinding, DiagnosticKind};`). **`DiagnosticFinding` is included explicitly** (it travels inside `DiagnosticReport`, so any consumer reaching it via the engine must see it re-exported — G-10). The engine does **not** redefine them, so `unblock-render` (model + error only) can format engine results without depending on the engine.

### 4.1 Session surface

```rust
pub struct Session { /* storage: Arc<dyn Storage>, write_permit: Arc<tokio::sync::Semaphore>, config, actor, paths, shutdown */ }
// NO `policy` field (OQ-1 RESOLVED, T1.2): `unblock-policy` ships only FREE FUNCTIONS (`cmp_ready` etc.) —
// there is no policy struct/trait-object to hold; `ready()` calls `unblock_policy::cmp_ready` directly.
// A `dyn`-pluggable-policy handle is a v2 additive seam (it would add a field then; it does not exist now).

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
    pub async fn ready(&self, filters: &ListFilters) -> Result<Vec<Issue>, EngineError>; // hybrid sort via policy (NORMATIVE: "hybrid sort via policy" == `unblock-policy::cmp_ready` == `ready_hybrid_bucket(priority.0<=1)` ASC, then created_at ASC, then id ASC — byte-faithful to `sort_ready_hybrid` sqlite.rs:10444 / `ready_hybrid_bucket` sqlite.rs:10515. The §3.2.1 `ready_issues` SQL (`ORDER BY priority ASC, created_at ASC, id ASC`) is the candidate **pre-sort** (created_at ASC, NOT the list DESC order); the engine re-ranks it via `cmp_ready` — which buckets P0/P1 together so the final order differs from the SQL pre-sort — per CF-11. §3.2.1 SQL is unchanged.)
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
    pub async fn doctor(&self) -> Result<DiagnosticReport, EngineError>;  // FR-15/FR-16. v1 = SIGNATURE only; body seamed to unblock-health (T3.3) — returns EngineError::FeatureNotWired{feature:"health"} until then (the integrity DiagnosticKind variant + DoctorReport→DiagnosticReport mapping are designed at T3.3; the landed DiagnosticKind has no integrity variant)
    pub async fn recover(&self) -> Result<DiagnosticReport, EngineError>; // attempt repair (WAL checkpoint, reindex; reports actions taken). v1 = SIGNATURE only; body seamed to unblock-health (T3.3) — returns EngineError::FeatureNotWired{feature:"health"} until then (the rich repair + evidence dir are T3.3)
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

> The seven rules below are individually addressable as `spine §6.1`..`§6.7` so a crate plan can cite
> the exact rule it conforms to (e.g. RK-3 = §6.6). Each `### 6.N` carries the full normative rule.

### 6.1 Exact shapes (amend-first)
Per-crate plans MUST use these exact type/field/signature shapes; deviations require amending this file first.

### 6.2 No backend type in public API
No backend (libsql) type may appear in any public API outside `unblock-storage`'s private impl (NFR-15).

### 6.3 content_hash recomputed on load
`content_hash` is `#[serde(skip)]` and recomputed on load; it is the import idempotency key (FR-26).

### 6.4 Single mutation path
All mutations flow through `Session` (FR-9) and the single write permit (D14); reads never acquire it (FR-10).

### 6.5 One error → one code → one exit code
Every error surfaced at L7 maps to exactly one `ErrorCode` and one 0–8 exit code per §2.3 (golden-snapshot pinned).

### 6.6 MCP tool-count budget
MCP tool count stays ≤ 8; new domain surface in v1.1 extends existing tools by discriminator before adding tools (RK-3).

### 6.7 Safety / no-git / no-default-network
`forbid(unsafe_code)`, no git crate / `Command::new("git")` anywhere (NFR-6/NFR-9); network/TLS only behind the non-default `remote` feature (D15).
