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

`unblock-cli` **depends on** `unblock-mcp`. The CLI owns the `unblock` binary (incl. `unblock serve`); `unblock-mcp` is a **library** that exposes `serve(session: Arc<Session>, opts: ServeOptions) -> Result<(), McpServerError>` and the tool/resource/prompt registry. **`serve` signature (LIVE — D27/AD-4, reconciled spine-first against T2.2/PR #387):** it is the **2-arg** `serve(Arc<Session>, ServeOptions)` — the transport is bound **internally** to `stdio()` (the caller does NOT pass a transport), and shutdown is a `tokio_util::sync::CancellationToken` carried in `ServeOptions.cancel` (`.cancel()` drains in-flight work and returns `Ok(())`). The earlier 3-arg `serve(session, transport, shutdown)` / bespoke `ShutdownToken` sketch **never shipped** and is superseded here. The direction is fixed **cli → mcp** and **never mcp → cli** — this is the single L7↔L7 edge that determines acyclicity, and it is now a decision (not an assumption). The cli plan's Open Question Q1 is **RESOLVED** by this line. README §2 and §0 draw this edge as settled and are correct.

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
- **bd import-normalize repairs (NOTE — D24/FR-26).** The one-shot `bd` import applies bd's 7 legacy-repair steps (dep-type legacy-underscore repair — a **GENERAL** rule per bd `normalize_issue` (`temp/beads_rust-main/src/sync/mod.rs:3822-3832`): for any `Custom` `dep_type`, `replace('_','-')` and adopt iff it parses to a known non-`Custom` `DependencyType` — repairing `parent_child`/`conditional_blocks`/`waits_for`/`discovered_from`/`replies_to`/`relates_to`/`caused_by` (all land in `Custom` because `DependencyType::parse_lowercased` recognizes only kebab forms); `parent_child`→`ParentChild` is the canonical EXAMPLE, not the whole rule; dependency dedup keep-latest; terminal-status text aliases `done`/`complete`/`completed`/`finished`/`resolved`→`Closed`; `-wisp-` id→`ephemeral=true`; `closed_at`=`updated_at` when terminal & none; `closed_at` cleared when non-terminal; `external_ref` blank/whitespace→`None` else trimmed) in `unblock-sync::bd_import` (L3) **BEFORE** the `content_hash` recompute and the funnel into the `import.rs` apply path. **Inter-repair ORDER is bd's SOURCE ORDER and is normative (SF-2):** the status-alias repair (`mod.rs:3873-3881`) MUST run BEFORE the `closed_at` set/clear repair (`mod.rs:3888-3896`), which tests `is_terminal()` — an aliased-terminal issue whose alias was not yet mapped keeps `closed_at` unset and diverges `sync_equals`. **Hash-recompute composition (SF-4):** after its 7 repairs `bd_import` calls the SHARED `jsonl::normalize()` (`crates/unblock-sync/src/jsonl.rs:97`) — which does labels sort/dedup + `updated_at>=created_at` clamp + the `content_hash` recompute — so the recompute includes the same shared steps bd folds into the SAME `normalize_issue` pass (`mod.rs:3816-3820`/`:3908-3913`) and the recomputed hash matches bd's stored hash byte-for-byte; `bd_import` does NOT recompute standalone. This is REQUIRED for FR-26 idempotency + cross-tool dedup: bd computes its stored hash after these repairs, so unblock must apply them before recompute to get a byte-identical hash. Repaired fields that are **hashed** (the spine §1.8 field set): `status`, `external_ref`. Repaired fields in **`sync_equals`** only: `dep_type`, `closed_at`, `ephemeral`, dependency count. The shared `jsonl::normalize()` order for the GENERIC `import_jsonl` path is **UNCHANGED** (it imports only unblock's own canonical exports, never legacy bd forms).
- **Tombstone** — delete sets `status = Tombstone` + `deleted_at`/`deleted_by`/`delete_reason` (and `original_type` preserved). `is_expired_tombstone(retention_days: Option<u64>) -> bool` (TTL helper). Invariant: **import NEVER resurrects a tombstone** — a non-tombstone JSONL line for an id that is tombstoned in the DB is rejected/skipped, not applied.
- **Restore (D20)** — the live, audited **INVERSE of delete** (satisfies FR-1c "recoverable"). A dedicated `Storage::restore_issue`/`Session::restore` un-tombstones a SOFT-deleted issue: it **clears `original_type`** (→ `None`), **NEVER touches `issue_type`** (the live `issue_type` on a tombstone is already correct — `into_tombstone` only snapshots, never mutates it), sets `status` **best-effort via `closed_at`** (`closed_at.is_some() ? Closed : Open` — only `closed_at` survives as the was-Closed signal; pre-delete status is not preserved), and **clears `closed_at` on a restore-to-non-terminal** (the Closed branch keeps it — both the signal and the CHECK-constraint satisfier). A round-trip helper `Issue::restore_from_tombstone` (the pure inverse of `into_tombstone`, no clock) backs it. TTL-aware refusal of an expired tombstone is a **v1.1 seam** (`deletions_retention_days` is reserved/unenforced in v1). Restore is a **live engine op, NOT an import** — it is **DISJOINT** from the import no-resurrection invariant above (which stays unchanged and import-path-scoped). See §3/§3.2.1/§4.1.

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

These types are **defined normatively in `unblock-model`** so that crates which cannot depend on `unblock-storage` can still reference them. `unblock-policy` needs `ListFilters`/`CountGroupBy` for its filter-fingerprint (CF-C); Any field ADDED to `ListFilters` (e.g. `include_tombstone`, D23) MUST also be folded into `unblock-policy::filters_fingerprint` so the fingerprint stays INJECTIVE (two filter sets differing only in the new field must produce distinct fingerprints) — see the policy crate plan `cache_key.rs`. `unblock-render` (model + error only) needs the display/result DTOs and `OutputFormat` to format output (CF-A/CF-J); `DiagnosticReport`/`DiagnosticKind`/`DiagnosticFinding` are referenced by engine/render/mcp and previously had no home (CF-B). The **full owned set** is: `ListFilters, CountGroupBy, OutputFormat, CountBucket, GraphEdge, DepTree, CloseOutcome, ImportReport, ExportReport, DiagnosticReport, DiagnosticFinding, DiagnosticKind`. Every other crate (`unblock-storage`, `unblock-engine`, `unblock-render`, `unblock-config`, `unblock-sync`, `unblock-mcp`) **re-exports** these via `pub use unblock_model::{...}` — none redefines them.

**Derive policy for §1.10 (NORMATIVE — G-1):** every type below flows to an L7 consumer (the MCP §5.3 per-tool outputs/`QueryInput`, engine results, render parse-back, policy serialization). They therefore ALL derive `Debug, Clone, Serialize, Deserialize, JsonSchema`, plus `PartialEq, Eq` where round-trip/equality tests need it. The `derive` lines below are normative; crate plans (unblock-model `filters.rs`/`results.rs`, the mcp §5.3 output family, engine, render) must match exactly. `PathBuf` derives `JsonSchema` (serialized as a string) — `ExportReport.path` is schema-valid.

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
    /// Include `status = 'tombstone'` (soft-deleted) rows. Default `false` — the default-visibility,
    /// `include_deferred`, and `include_closed` branches all EXCLUDE tombstones, so a caller must opt
    /// in explicitly. Set `true` by the `unblock-sync` full-corpus export (FORK-1/D23) and the mcp
    /// `issues/{id}` not-found suggestion corpus (T2.6/D25 — a read-only error-path scan): tombstones
    /// must be exported so import-side tombstone-non-resurrection (FR-8, spine §1.8) is round-trippable.
    /// Orthogonal to `include_closed`: export sets BOTH true. list/ready/blocked/search/count/stale keep
    /// it `false` (no query TOOL sets it; the mcp `issues/{id}` not-found suggestion scan is the one
    /// agent-facing consumer — T2.6/D25).
    pub include_tombstone: bool,
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
pub struct ImportReport  { pub imported: usize, pub skipped: usize, pub dependencies: usize, pub comments: usize, pub dropped_fields: Vec<String> }
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

`ExportReport.written` counts every serialized issue line in the export corpus, which — per FORK-1/D23 — INCLUDES closed and tombstone rows (`ListFilters { include_closed: true, include_tombstone: true, .. }`) and EXCLUDES ephemeral / `-wisp-` rows. All emitted `DateTime<Utc>` fields are rendered via `unblock_model::fmt_ts_secs` (CF-TS) so export bytes are deterministic and byte-coherent with render (D-OQ-B).

**`ImportReport.dependencies`/`.comments` (D24/F1)** count the relation/comment rows migrated by the one-shot `bd` import, tallied on the POST-repair, POST-dedup record (faithful to bd's `record_imported_relation_counts`, `temp/beads_rust-main/src/sync/mod.rs:4611-4614`); both default to `0` on the generic `import_jsonl` path (it never tallies them). **Count plumbing (MF-2, option (a)):** the shared `unblock-sync::apply_records` (D24/F5) builds+returns the report with `imported`/`skipped`/`dropped_fields` set and `dependencies:0, comments:0`; EACH CALLER finalizes the two counts on the returned report — `import_jsonl` leaves them `0`, `import_bd` sets them to its tallies — so the seam carries no deps/comments params (see the sync `import.rs` row). Since `ImportReport` has NO `Default` derive, the two added fields force an Implement ripple: re-bless the JsonSchema golden `crates/unblock-model/tests/snapshots/schema_snapshots__import_report.snap` (3→5 properties) + update every 3-field struct literal (`import.rs:167`/`:180`, `results.rs:157-161`, `output.rs:57-61`, the new `bd_import.rs` constructor). Definition/re-export home = `unblock-model` (spine §1.10). Additive; NOT part of any MCP tool input `JsonSchema` (it rides in the mcp-owned `SyncOutput`), so **no `CONTRACT_VERSION` bump** — verified against `schema_bundle()` (the golden re-bless is the model snapshot, not the `schema_bundle` hash). **Forward note (T2.6/D25):** superseded going forward — the D25 bundle carries the per-tool output schemas (`sync` output = `SyncOutput`), so ANY future `ImportReport`/`ExportReport` field change moves `CONTRACT_HASH` and forces a `CONTRACT_VERSION` bump. The T2.5 ruling stays valid for its time.

**Canonical timestamp helper (CF-TS — NORMATIVE, D-OQ-B / FORK-4 / D23).** The single source of truth for rendering a `DateTime<Utc>` as an RFC-3339 string lives in **`unblock-model`** (L0), in a new module `src/time.rs`, re-exported flat from `lib.rs`:

```rust
use chrono::{DateTime, SecondsFormat, Utc};
/// Canonical RFC-3339 rendering: UTC, SECOND precision, `Z` suffix (e.g. `2026-01-02T03:04:05Z`).
/// The ONLY path any crate may use to stringify a `DateTime<Utc>` for output/export. No crate may
/// call `to_rfc3339()` directly (it emits sub-seconds + numeric offset, breaking byte-determinism
/// and the render↔export byte-coherence the T2.4 JSONL export depends on).
#[must_use]
pub fn fmt_ts_secs(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}
```

**`unblock-render::fmt_ts` is REWIRED to call it** (`fmt_ts(dt) == unblock_model::fmt_ts_secs(dt)` — render keeps the `fmt_ts` name as a thin wrapper/re-export for its backends; the body becomes the model call, not a second `to_rfc3339_opts`). **`unblock-sync` calls `fmt_ts_secs` directly** on export (sync depends on `unblock-model` at L0, so it needs NO render dep — the `sync→render` layering violation is avoided by construction). DRY end-state: exactly ONE `to_rfc3339_opts(SecondsFormat::Secs, true)` in the workspace (in `unblock-model`). Byte form is `<date>T<time>Z` at second precision — byte-identical to the current `fmt_ts` (verified verbatim at `crates/unblock-render/src/format.rs:146-148` this session). `content_hash` is unaffected (spine §1.8 excludes all timestamps from the hash), so this is hash-safe. Render's doctest (`format.rs:138-143`) and unit test (`format.rs:236-244`) stay green unchanged.

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

**`RenderError` follows this pattern too (D27/AF-4, T3.1 — additive).** `unblock-render` already computes an inherent `code()`; T3.1 adds `impl unblock_error::CodedError for RenderError { fn code(&self) -> ErrorCode { self.code() } }` (delegating to the inherent map — one error → one code, §6.5 unchanged) so the uniform `(&err).into()` L7 bridge covers it like every other per-crate enum. The 4th render variant `RenderError::UnknownFormat { name: String }` (added at T3.1 — `parse_format`'s unknown-name arm) maps to `ErrorCode::ValidationFailed`, the same family as `UnsupportedFormat`/`FieldUnknown`; the §2.3 exit table is UNCHANGED (no new ErrorCode). **`McpServerError` is the deliberate exception:** it does NOT impl `CodedError` — the cli `exit.rs` maps it EXPLICITLY (`Transport`/`RunLoop` → `ErrorCode::InternalError`, exit 1; a serve run-loop/transport failure is an INTERNAL condition, not a user IoError) via `StructuredError::from_code(InternalError, err.to_string())` (which already routes through `sanitize_message`). See §5b.

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
    pub const fn hint_shape(self) -> HintShape;       // D25/FORK-4B — static per-code hint SHAPE
    //   exact map (grounded in the REAL production hint sites; no invented hints):
    //     SimilarIds     -> IssueNotFound                       (the unblock://issues/{id} not-found
    //                       fold, D25/FORK-3A — find_similar_ids + context["similar_ids"])
    //     StaticText     -> InvalidPriority | InvalidStatus | InvalidType
    //                       (ModelError::hint() -> the pinned PRIORITY_DETAIL_HINT /
    //                        VALID_STATUS_HINT / VALID_TYPE_HINT constants, hints.rs)
    //     ContextualText -> ValidationFailed                     (site-composed guidance: the mcp
    //                       over-quota + bulk-markdown-parse hints)
    //     None           -> every other code (the remaining 30 of 35)
    pub const fn static_hint(self) -> Option<&'static str>; // the fixed text for StaticText codes,
    //   None otherwise. The SINGLE source of the fixed hint texts (`ModelError::hint` delegates
    //   here), so `hint_shape() == StaticText ⟺ static_hint().is_some()` holds by construction.
}

/// D25/FORK-4B — the static per-code hint SHAPE (FR-12): what KIND of `hint` a code carries WHEN
/// one is present. Presence stays per-instance (`StructuredError.hint: Option<String>`, §2.4);
/// this is the machine-advertised taxonomy surfaced in the mcp `capabilities()` error map (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HintShape {
    None,           // no production site attaches a hint to this code
    StaticText,     // a fixed, code-determined constant (the hints.rs *_HINT consts)
    ContextualText, // free-form guidance composed at the failure site (content varies per site)
    SimilarIds,     // fuzzy near-miss id suggestions via find_similar_ids (context["similar_ids"]),
                    // with a list-discovery fallback when no candidate is close
}
impl HintShape { pub const fn as_str(self) -> &'static str; } // "none" | "static_text" | ...
```

**Honesty rule (NORMATIVE):** a code may move off `HintShape::None` only when a real production
hint-construction site ships in the same change; shapes are never aspirational. The full
`code → (exit_code, retryable, hint_shape)` map is golden-pinned in unblock-error alongside the
§2.3 exit-code table (the quadruple golden), and re-surfaced (version-coupled) in the mcp
`capabilities()` error map — changing any `hint_shape` moves the D25 `CONTRACT_HASH` gate. An enum
(not `&'static str`, not a payload-carrying variant) is deliberate: exhaustive matches force every
future `ErrorCode` variant to declare its shape; a `StaticText(&'static str)` payload would break
`Deserialize` (the FR-12 e2e parses `Capabilities` client-side) — the fixed text is reachable via
the paired `static_hint()` instead; and the L0 discipline holds (no new deps — serde/schemars
only, same as `ErrorCode`).

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

The L7 boundary converts any composed crate error → `StructuredError` (CLI: serialize to stdout + `process::exit(exit_code)`; MCP: attach as error data, §5.6). Output is **always valid JSON even on error** (FR-11). `tracing` on `unblock.reliability` records the L7-boundary error, while the reliability GUARD emissions (external-path use, conflict-marker rejection, force-override) are emitted in `unblock-sync` at L3 with the standardized `operation`/`path`/`result`/`reason` field set (NFR-13, D30); `unblock-engine/src/logging.rs` owns only the idempotent subscriber init; structured output strictly stdout, diagnostics stderr (NFR-14).

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
    async fn schema_version(&self) -> Result<i64, StorageError>;           // D27/AF-2 (T3.1) — PRAGMA user_version, PURE READ
    //  Read the current on-disk schema version (a fresh/unstamped DB reports 0; a migrated DB at the current
    //  baseline reports CURRENT_SCHEMA_VERSION). A pure read (no write permit, no migration side-effect) so the
    //  engine can report `migrate`'s from→to delta without re-opening. Backend-agnostic `i64` (the on-disk value is
    //  a PRAGMA integer; libsql's internal helper is i32 but the trait keeps the wider type so no backend width
    //  leaks — the libsql impl widens with `i64::from(current_user_version(..))`). Every `Storage` impl states it
    //  (no default fn — a defaulted answer would silently mislead a versioning backend); the test stubs return a
    //  constant (`NoopStorage` → 0, the other doubles → 1). Backs `Session::migrate` (§4.1).

    // --- issue CRUD (mutations carry actor + optional Tier-1 attribution; write Event(s) transactionally) ---
    async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError>; // returns id
    async fn create_issues(&self, issues: &[Issue], actor: &str) -> Result<(), StorageError>;  // D22/T2.3 — ATOMIC bulk insert
    //  Inserts the WHOLE slice in ONE `BEGIN IMMEDIATE` tx: every row + its `Event(Created)` + per-relation
    //  events + the seeded dependency edges + child-counter bumps, committed ONCE. ANY failure on ANY record
    //  (id/`external_ref` collision, FK/CHECK violation, backend error) ROLLS BACK the entire tx — ZERO rows
    //  persisted (no partial batch). The engine `Session::create_bulk` (§4.1) mints all ids + resolves intra-batch
    //  deps under the write permit BEFORE calling this, so storage receives fully-formed `Issue`s with resolved
    //  ids/edges. `create_issue` (single) and `create(&Issue)` (import) are UNCHANGED. §3.2.1.
    async fn get_issue(&self, id: &str) -> Result<Option<Issue>, StorageError>;               // hydrated (labels/deps)
    async fn get_issues(&self, ids: &[String]) -> Result<Vec<Issue>, StorageError>;
    async fn update_issue(&self, id: &str, patch: &IssuePatch, actor: &str) -> Result<Issue, StorageError>;
    async fn delete_issue(&self, plan: &DeletePlan, actor: &str) -> Result<DeletePlan, StorageError>; // DryRun mutates nothing
    async fn restore_issue(&self, id: &str, actor: &str) -> Result<Issue, StorageError>; // FR-1c (D20) — un-tombstone (single id);
    //   already-active → idempotent no-op Ok(issue) (no event, no updated_at bump); missing/hard-deleted id → IssueNotFound. §3.2.1.

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
    async fn detect_cycles(&self, blocking_only: bool) -> Result<Vec<Vec<String>>, StorageError>;
    //  each = an ORDERED cycle-path witness (NOT a sorted node set): a multi-node cycle is
    //  [start, …, start] (repeats the start); a self-loop is [node, node] (§3.2.1).
    //  blocking_only=true → the 4 gating types only (= `affects_ready_work`); =false → ALL
    //  dependency types (integrity/lint view) — D19, faithful to original detect_blocking_cycles
    //  (true) / detect_all_cycles (false).

    // --- hierarchical-id child allocation (FR-1a, D21) — the READ-half the engine allocator consumes ---
    async fn next_child_number(&self, parent_id: &str) -> Result<u32, StorageError>; // high-water+1 (§3.2.1)
    //  PRODUCTION method (T1.8): returns the next free child number for `parent.N` minting — the
    //  `child_counters` high-water mark + 1, falling back to a LIKE-ESCAPE legacy scan. DISTINCT from
    //  the testkit-only `testkit_child_high_water` seam. The engine reads this under the SAME write
    //  permit as the in-tx counter bump (create_issue), so the `parent.N` counter cannot race.

    // --- events (audit; append-only) ---
    async fn list_events(&self, issue_id: &str) -> Result<Vec<Event>, StorageError>;

    // --- diagnostics support (FR-15, pure-DB; no git) ---
    // D26 (T2.7): changelog/lint/orphans add NO new Storage method — the engine composes them from the
    //   reads already declared here + list/ready/blocked/count/dependency_tree. `closed_since` is already
    //   `since`-windowed; `orphan_candidates` is already status-agnostic. The bd-faithful `stats` diagnostic
    //   is the ONE exception: `epics_eligible_for_closure` needs a per-epic parent-child child rollup that
    //   no existing read composes, so a faithful port adds ONE purely-additive pure-DB aggregate primitive —
    //   `epic_child_rollup() -> Vec<(String,(usize,usize))>` (below): per-epic (child_total,
    //   child_closed_or_tombstone), Vec sorted by epic id in SQL (ORDER BY — bd's get_epic_counts returns a
    //   non-deterministic HashMap, sqlite.rs:6978; unblock sorts for NFR-14). bd's get_epic_counts ported 1:1
    //   (JOIN dependencies d JOIN issues i ON d.issue_id = i.id WHERE d.type='parent-child' AND the CHILD is
    //   non-template; child-closed = status IN ('closed','tombstone')). The epic-side active + non-template
    //   filter (issue_type==Epic ∧ ¬terminal ∧ ¬template, stats.rs:441-446) is applied IN-MEMORY in the
    //   engine — both filters live at their respective sites, do not conflate. `pinned` and the per-status /
    //   tombstone tallies are NOT primitives: `pinned` is composed in-memory (issue.pinned || status==Pinned,
    //   stats.rs:436) over the widest-visibility list_issues pass, and the per-status tally + tombstone count
    //   + tombstone-excluded total come from the EXISTING count_issues (with include_tombstone:true for the
    //   tally). The rollup is a bare Vec of tuples — NO StatsRollup model DTO — so §1.10 does not grow a
    //   type. It is an internal aggregate; NOT a wire/schema DTO, so NO mcp `CONTRACT_HASH` impact.
    //   (The full-taxonomy `diagnostic_probe`/`diagnostic_probes` remain the commented [v1.1] CF-E seams
    //   below.)
    async fn epic_child_rollup(&self) -> Result<Vec<(String, (usize, usize))>, StorageError>; // stats: per-epic (child_total, child_closed_or_tombstone), ORDER BY epic id
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
  `Event(DependencyAdded)`/`Event(Commented)` are recorded for any seeded relations. **Storage receives an
  already-allocated `Issue.id` — it does NOT mint (D21).** For the interactive create path the id is minted
  by the **engine** id-allocator (D21) under the write permit and the resulting `Issue` is handed here; for
  the import/internal path the caller supplies the id (`Session::create`). The id-collision guard above is the
  storage-side backstop that makes the engine's mint→probe→insert atomic when both run under the single permit
  (a candidate that races in still surfaces `IdCollision`). The in-tx `child_counters` bump for a hierarchical
  id is the write-half whose read-half (`next_child_number`, §3.2 / crate plan §3.3) the engine allocator
  consumes in production (D21) — the two halves run under the SAME permit so the `parent.N` counter cannot race.
- **`create_issues` (net-new; D22/T2.3) — the ATOMIC bulk INSERT (the all-or-nothing one-tx primitive).** Opens
  ONE `BEGIN IMMEDIATE` tx (the same `with_immediate_tx` chokepoint as every other mutation) and, FOR EACH `Issue`
  in the slice, does exactly the per-record work `create_issue` does inside that single shared tx: the **id-collision
  guard** (`IdCollision{id}`) + the **`external_ref`-collision guard** (backend error), the row INSERT (binding the
  computed `content_hash`), the in-tx `child_counters` bump for a hierarchical id, the deduped label/dependency/comment
  inserts with their per-relation `Event(LabelAdded)`/`Event(DependencyAdded)`/`Event(Commented)`, and the defining
  `Event(Created)`. It does **no minting and no validation** (the engine `Session::create_bulk` mints every id +
  runs the full `IssueValidator::validate` on each built `Issue` BEFORE calling this — storage stays validation-free,
  like `create_issue`). **Atomicity is the whole point:** the loop runs inside ONE tx, so a failure on record #k
  (a raced `IdCollision`, an `external_ref` clash, an FK/CHECK violation, any backend error) returns `Err` and the
  tx ROLLS BACK — records 1..k-1 staged in the same tx are discarded, ZERO rows persist (`with_immediate_tx` already
  rolls back on `Err`; an uncommitted libsql `Transaction` also rolls back on drop). The dependency edges the engine
  resolved intra-batch are carried on each `Issue.dependencies` and inserted by the same per-record path, so an edge
  pointing at a sibling minted earlier in the SAME batch resolves correctly (both rows live in the one uncommitted
  tx). Same-parent siblings in the batch arrive with ALREADY-DISTINCT `parent.N` ids (the engine `create_bulk` mints
  them via its in-batch per-parent counter, §4.1 step 2 — `next_child_number` reads only committed state, so two
  same-parent siblings would otherwise both mint the same `parent.N` and collide in-tx); storage just runs the per-record
  `child_counters` UPSERT (high-water MAX), which lands the row's `N` monotonically regardless of insert order. The **id-collision guard inside the tx is the atomicity backstop**: the engine's pre-tx `get_issue` probe
  (against committed state + the in-batch minted set) avoids collisions, but an out-of-band writer that races a row
  in between the probe and this commit still surfaces `IdCollision` here — and because it is inside the one tx, it
  rolls back the WHOLE batch (never a partial commit). This is the design that closes the partial-batch hole a loop
  over single `create_issue` calls would leave; the single `create_issue`/`create` paths are UNCHANGED.
- **`update_issue` (sqlite.rs:2496–2509, 2572–2870) — per-field event granularity; empty diff is a
  full skip.** An empty patch (or one that changes nothing) returns the issue unchanged and writes **no
  `SET`, no `updated_at`, no `Event`** (`if set_clauses.is_empty() { return Ok }`). `updated_at` advances
  and `content_hash` is recomputed **only** when at least one stored column changes. Per-field events
  (see the EventType-per-mutation table below) are emitted **only when the field's value actually
  changes** (e.g. patching `status`→its current value emits no `StatusChanged`). **Tombstone-patch guard
  (crud.rs:332-334, SSOT):** a patch targeting a **tombstone** is rejected with **`IssueNotFound`** before any
  `SET` — a tombstone cannot be reopened/edited via `update`, so the only un-tombstone path is `restore`
  (see the `restore_issue` carve-out below). This guard is what makes `restore` STRUCTURALLY separate from the
  reopen=update mapping (§5.2) and is cited by the §3.2.1 `restore_issue` step, the §4.1 `Session::restore` seam
  note, and the §5.2 `Reopen`/`Restore` notes — all of which point HERE as the normative source.
- **`delete_issue` (sqlite.rs:2952–3015) — delegate to the model `Issue::into_tombstone`** (which sets
  `status = Tombstone` + `deleted_*` and preserves `original_type`), then bump `updated_at = now` and
  recompute `content_hash`. **`Event(Deleted)` is written ONLY when the prior status was non-terminal**
  (`!was_terminal`): tombstoning an already-`Closed` issue records **no** `Deleted` event. An
  already-tombstone target is a **no-op `Ok`**. One tx. (The model-B schema drops the original's
  `close_metadata` table, so its cleanup `DELETE` is removed.)
- **`restore_issue` (net-new; D20) — the audited live INVERSE of `delete_issue`'s soft tombstone.** One
  `BEGIN IMMEDIATE` tx, TOCTOU-safe: **load the row inside the tx**. (1) **Missing → `IssueNotFound`** (this
  bounds FR-1c "recoverable" to SOFT deletes — a Hard-deleted row is gone; restore does **not** mint a new
  `NotATombstone` ErrorCode, keeping the frozen unblock-error golden / exit-code table / retryable set
  untouched). (2) **Not a tombstone → idempotent no-op `Ok(issue)`** — **no event, no `updated_at` bump**
  (mirrors delete's already-tombstone no-op and claim's same-actor short-circuit; retry-safe). (3) **Real
  tombstone → one `UPDATE`** delegating to the model `Issue::restore_from_tombstone` (the pure inverse of
  `into_tombstone`), which:
  - sets `status` **best-effort via `closed_at`** — `if closed_at.is_some() { Closed } else { Open }` (D20,
    DECISION 1). The pre-delete status is NOT preserved (only `original_type` survives); `closed_at` being
    set is the **only** signal the issue was Closed before deletion. Open and Closed round-trip exactly;
    InProgress/Blocked/Deferred collapse to Open (that information is genuinely lost — acceptable).
  - **leaves `issue_type` UNTOUCHED** (DECISION 3): `into_tombstone` never mutates `issue_type` (it only
    snapshots `original_type` from the live `issue_type`), so the live `issue_type` on a tombstone is
    ALREADY correct. Writing `original_type`→`issue_type` would be a no-op for local deletes and would
    **CORRUPT** imported rows where the serde-carried `original_type` diverges from `issue_type`.
  - **clears `original_type` → `None`** (the empty-string DB column → `None` on load), returning the row to a
    clean active issue.
  - **clears the tombstone fields**: `deleted_at` → NULL, `deleted_by` → `''`, `delete_reason` → `''`.
  - **`closed_at` handling is CORRECTNESS-CRITICAL** (driven by the issues-table CHECK constraint, below):
    the **Open** branch sets `closed_at` → NULL (defensive; it is already NULL on a tombstone); the **Closed**
    branch **KEEPS** the existing `closed_at` (it is both the status signal AND what satisfies the CHECK for
    `status='closed'`).
  - bumps `updated_at = now` and **recomputes `content_hash`** from the restored fields (the hash includes
    `status`, so for a was-Closed issue it legitimately differs from the pre-delete hash — correct, not a bug).
  - appends a single **`Event(Restored)`** (D20, DECISION 2 — see the EventType table carve-out; restore is a
    DEDICATED path and emits ONLY `Restored`, never `StatusChanged`/`Reopened`).
  Returns the restored `Issue` (the impl re-reads via `get_issue` so labels/deps/comments hydrate). **The
  re-read runs in-tx (or post-commit while still holding the write permit); since the row was just written, a
  `None` re-read is an INTERNAL INVARIANT VIOLATION → a `Backend` error, NEVER `IssueNotFound`** (mirroring how
  `claim_issue` re-`SELECT`s the holder in-tx — a just-written row that reads back absent is corruption, not a
  caller-facing "not found"). A restored
  issue **re-enters the dependency graph with its surviving edges** (soft tombstone keeps deps/labels/comments
  rows) — it may be **immediately blocked-by/blocking** live issues (the inverse of close's newly-unblocked);
  no relation-rehydration logic is added (`policy never_includes_tombstoned` already filters tombstones from
  `ready`; the live-adjacent-to-tombstone case is distinct and fine). **CHECK-constraint correctness:** the
  issues-table constraint `(status='closed' AND closed_at IS NOT NULL) OR (status='tombstone') OR (status NOT
  IN ('closed','tombstone') AND closed_at IS NULL)` makes the `closed_at` handling correctness-critical, not
  hygiene — a missed clear/keep → CHECK fails → tx rollback → silent restore failure. **Single-target only
  (D20, DECISION 4):** `restore_issue` is scalar — NO `--cascade`, NO ancestor guard (the tombstone records no
  "deleted-by-cascade" provenance, so a blanket cascade-restore would over-revive children tombstoned
  independently); cascade-restore is a **v1.1 seam** (needs a delete-batch identity). The `deletions_retention_days`
  TTL is **reserved/unenforced in v1**; a TTL-aware refusal of an expired tombstone is a v1.1 seam.
- **`claim_issue` (sqlite.rs:2888–2935) — assignee-ONLY guard, no status predicate.** The atomic claim
  `UPDATE` carries `WHERE id = ? AND (assignee IS NULL OR TRIM(assignee) = '' OR assignee = ?<actor>)`
  — there is **no** `status NOT IN (...)` predicate. 0 rows affected → re-`SELECT` the current holder
  **within the same tx** → `AlreadyClaimed{by}`. A **same-actor re-claim short-circuits BEFORE the
  `UPDATE`** (idempotent `Ok`, no `updated_at`, no event).
- **`list_issues` — composes the full `ListFilters` set.** `status`-OR, `issue_type`-OR, inclusive
  `priority_min`/`priority_max`, `assignee`, `labels_all` (AND, per-label `EXISTS`) / `labels_any`
  (OR, single `EXISTS … IN`), `text_contains` (title `LIKE ? ESCAPE '\'`, distinct from the `search`
  needle), and closed/deferred visibility (default excludes `closed`/`tombstone`/`deferred`;
  `include_closed`/`include_deferred` widen it). `include_tombstone` (D23; the sync full-corpus export + the mcp `issues/{id}` not-found suggestion scan, T2.6/D25) additionally
  widens the baseline to include `status='tombstone'` rows; no agent QUERY TOOL sets it (the mcp resource error-path scan is the one non-export consumer).
  `ORDER BY priority ASC, created_at DESC, id ASC` —
  note `created_at` **DESC** (newest-first), **deliberately distinct from `ready`'s `created_at ASC`**
  pre-sort (oldest-first within a priority bucket; policy `ready.rs:16-17`). Default-complete unless
  `limit` set; `offset`-without-`limit` uses `LIMIT -1 OFFSET n`. Authoritative order for render/MCP
  snapshots (T2.1/T2.3), NFR-14. (§3.2.1 previously had no entry for `list_issues`, `count_issues`,
  `stale_issues`, or `get_issue`; the first three are pinned here/below because their order is
  render-snapshot authoritative. `get_issue` is intentionally entry-less: a single-row lookup by id
  with no ordering surface.)
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
  id ASC` (Miguel). **Facets NARROW the blocked set (FR-4 "filters compose", D18).**
  `blocked_issues` applies the same *narrowing* facets as `list_issues` — status-OR, `issue_type`-OR,
  inclusive `priority_min`/`priority_max`, `assignee`, `labels_all` (AND) / `labels_any` (OR), and
  `text_contains` (title `LIKE ? ESCAPE '\'`) — to the candidate rows **before** the live blocked-set
  membership test. The three-pass blocked detection and the `ORDER BY` are unchanged (facets only
  filter; they never alter blocked-ness or order). Crucially, `blocked` does **NOT** inherit
  `list_issues`' default visibility: its baseline `status NOT IN ('closed','tombstone')` is
  **deferred-INCLUSIVE**, so `include_closed`/`include_deferred` are **no-ops** here (closed/tombstone
  can never be blocked-visible; deferred is always shown).
- **`search_issues` (sqlite.rs:4543–4727) — substring `instr(lower(col))` over title+description+id.**
  The needle is lowercased and matched with
  `instr(lower(title), ?) > 0 OR instr(lower(description), ?) > 0 OR instr(lower(id), ?) > 0` (no LIKE
  escaping on the needle). A `filters.text_contains` FILTER (distinct from the search needle) keeps the
  `LIKE ? ESCAPE '\'` form over `title`. Cap **50** when `filters.limit` is unset. `ORDER BY priority
  ASC, created_at DESC, id ASC` (the no-explicit-sort tail). The `sort`/`reverse` branches are deferred
  to v1.x; FTS5 to v1.4.
- **`count_issues` — `ORDER BY k ASC`** over the group key (`status` / `issue_type` /
  `COALESCE(assignee,'')` / `CAST(priority AS TEXT)` / `labels.label`); ungrouped (`by=None`) returns a
  single `key="total"` bucket. The `Label` group-by JOINs the labels table and therefore
  **double-counts** multi-label issues vs the ungrouped total. Default visibility = `list_issues`
  default (excludes closed/tombstone/deferred; `include_*` widen). Priority keys are numeric strings
  `'0'..'4'` sorted **lexically** by `k ASC` (fine for single digits). Deterministic order is
  render-snapshot authoritative (T2.1), NFR-14.
- **`stale_issues` — `ORDER BY updated_at ASC, id ASC`** (oldest-updated first); composes the full
  `ListFilters` set plus `updated_at < older_than`. Default visibility = `list_issues` default.
  Deterministic order is render-snapshot authoritative (T2.1), NFR-14.

The dependency ops (FR-5) — source-verified vs the original cycle machinery:

- **`add_dependency` (cycle rejection: `would_create_cycle`/`check_cycle` sqlite.rs:2286,2440;
  `find_cycle_graph_path` sqlite.rs:10664) — rejects a *gating* cycle with the REAL ordered path.**
  Guards `SelfDependency` then `DuplicateDependency`, then builds the gating graph **including the
  prospective edge** (private `petgraph`, `would_cycle_in_tx`) over the 4 `affects_ready_work` types.
  If the new edge closes a gating cycle it is rejected with `CycleDetected { path }` where `path` is
  the **actual ordered cycle, naming every node** (e.g. `a -> b -> c -> a`), reconstructed by a private
  `find_cycle_path` DFS over the just-built graph (which already contains the prospective edge) — NOT a
  synthetic `a -> … -> a` placeholder (FR-5 AC). On success: insert + transactional
  `Event(DependencyAdded)`. (The reparent cycle-check, `crud.rs`, routes through the same
  `would_cycle_in_tx`, so the orientation fix below lands once.)
- **`would_cycle_in_tx` edge orientation (NORMATIVE — D4 correctness pin; `check_cycle`
  sqlite.rs:2440–2453 + `load_dependency_cycle_graph` sqlite.rs:11379–11387).** When building the
  gating cycle graph, the three blocking types (`blocks`/`conditional-blocks`/`waits-for`) are inserted
  **FORWARD** (`issue_id -> depends_on_id`), but `parent-child` edges are inserted **REVERSED**
  (`parent depends_on_id -> child issue_id`). This matches `load_dependency_cycle_graph` *and* unblock's
  own blocked-set propagation (parent blocked → child blocked, `blocked_issues` pass 3 above), so the
  cycle detector is consistent with unblock's own blocking direction; a uniform-forward graph would
  mis-detect mixed parent-child + blocks/waits-for/conditional-blocks cycles. **The prospective edge is
  oriented by the SAME per-type rule as an existing row** — a `parent-child` prospective edge is
  inserted REVERSED (`depends_on_id -> issue_id`), every other gating type FORWARD
  (`issue_id -> depends_on_id`) — then the cycle path is the `find_cycle_path` DFS from the prospective
  edge's graph-`from` back through its graph-`to`. This is the **orientation-consistent** reading of the
  original: the original `check_cycle` (sqlite.rs:2457–2476) treated the prospective edge as
  standard-forward regardless of type, a latent bug that let a pure `parent-child` cycle close
  undetected at add-time (the original's own `test_get_ready_issues_recursive_parent_cycle` adds three
  `parent-child` edges that close a cycle, yet all three `add_dependency` calls succeed). unblock pins
  the consistent reversal so a `parent-child`-only **or** mixed gating cycle is rejected at add-time, in
  line with the FR-5 AC and the `reparent_*_cycle_is_rejected` regression guards. Both `add_dependency`
  (carrying `dep.dep_type`) and `apply_reparent` (always `parent-child`) pass the prospective edge's
  type so the orientation is applied; the add-time guard is otherwise always gating (= original hardwired
  `blocking_only=true`).
- **`detect_cycles(blocking_only)` (`detect_cycles` sqlite.rs:11321; `load_dependency_cycle_graph`
  sqlite.rs:11379; `cycle_witnesses_from_graph` sqlite.rs:11410) — ORDERED traversal witnesses over
  SCCs, NOT sorted node sets (D3/D19).** Loads the cycle graph with the **same orientation** as
  `would_cycle_in_tx` (parent-child reversed, others forward), finds the strongly-connected components,
  then emits **one ordered witness per cyclic component**: a multi-node cycle is `[start, …, start]`
  (repeats the start), a self-loop is `[node, node]`; an acyclic graph returns `[]`. **The `…` here is
  META-notation for the interior nodes, which ARE named in the real witness** (e.g. a 3-node cycle is
  `["a", "b", "c", "a"]`) — it is NOT the literal D2-rejected placeholder string `a -> … -> a`, which
  never named the interior. `blocking_only=true`
  restricts the graph to the 4 gating types (= `affects_ready_work`; the original `detect_blocking_cycles`);
  `blocking_only=false` includes **all** dependency types (the original `detect_all_cycles`, the
  integrity/lint view) — **D19**. The witness shape (not a `path.sort()`'d node set, not a single-element
  self-loop) is the contract the `dep cycles` MCP action (T2.2) renders. **The `Storage` trait and the
  `Session` forward take a bare `bool` `blocking_only` (no Rust-level default); the default-TRUE
  (gating-only) lives ONLY on the MCP wire (`DepToolInput::Cycles`, §5.2 `#[serde(default = "default_true")]`)**
  — the same input-default-vs-method-arg asymmetry as `DiagnosticsInput::Changelog{since}` (wire
  `#[serde(default)]`) vs the bare `since` arg on `diagnostics(kind, since)` (D26/OQ-1).
- **`diagnostics(kind, since)` (FR-15, pure-DB — D26; no git, NFR-6).** The 7-kind read path composes
  ONLY existing `Storage` reads for changelog/lint/orphans — NO new trait method there. **`stats`** = the
  bd-faithful `StatsSummary` (`temp/beads_rust-main/src/cli/commands/stats.rs:376-499`): `list_issues` +
  `count_issues` over the widest-visibility filters → per-status tallies
  (open/in_progress/closed/deferred/draft/tombstone), `pinned` (`pinned=1` col OR `Pinned` status),
  `total` EXCLUDING tombstones, `blocked` = `Status::Blocked` id set ∪ the dependency-blocked active id set
  (DEDUPED by id — a manual-Blocked issue that is also dependency-blocked counts once; the `Status::Blocked`
  id SET comes from `list_issues(status=Blocked)` or `blocked_issues` membership — `count_issues` gives a
  COUNT not ids), `ready` (via `ready_issues`), `epics_eligible_for_closure`
  (non-template non-terminal epics whose parent-child children are all closed/tombstone, child-count>0),
  `average_lead_time_hours` (mean `closed_at − created_at` over closed, emitted only when `Some`) — **MINUS**
  bd's git-derived `RecentActivity` block (`git_recent_activity`, EXCLUDED per NFR-6). **TOMBSTONE VISIBILITY
  (not over-count):** today's `stats` ALREADY excludes tombstones from `total` (`count_issues` with
  `include_tombstone:false` → `visibility_branch`'s `else if include_closed` arm emits `AND status !=
  'tombstone'`, `libsql/query.rs`) and emits NO `tombstone` bucket; the gap is the ABSENT tombstone COUNTER.
  T2.7 sources the per-status tally from `count_issues` with `include_tombstone:true` (a live read-path flag
  since T2.6/D25) → the distinct `tombstone` finding is surfaced and `total` stays the non-tombstone sum
  (`total` needs no change). **STORAGE-SURFACE NOTE (OQ-5 seam — design-Review RATIFIED the NARROW primitive;
  §3.2 note above):** only `epics_eligible_for_closure`'s per-epic child rollup CANNOT be composed from the
  existing trait; a faithful port adds ONE purely-additive, pure-DB read primitive
  (`epic_child_rollup() -> Vec<(String,(usize,usize))>` — per-epic `(child_total,
  child_closed_or_tombstone)`, `ORDER BY` epic id in SQL for NFR-14, bd's `get_epic_counts` ported 1:1). It
  is an internal aggregate, NOT a wire/schema DTO, so NO `CONTRACT_HASH` impact, and returns a bare Vec of
  tuples — NO `StatsRollup` model type (§1.10 does not grow). `pinned` is DROPPED from the primitive —
  computed in-memory (`issue.pinned || status == Pinned`, `stats.rs:436`) over the widest-visibility
  `list_issues` pass — and the per-status/tombstone tallies come from the existing `count_issues`. Dropping
  `epics_eligible`/`pinned` to avoid the primitive would thin the faithful port (forbidden — never-simplify
  hard rule). **Documented seams (NOT silent bd divergences):** `ready`'s external-blocker exclusion is a
  vacuously-empty v1.1 config seam (unblock v1 has no external-project layer, so bd's `external_blockers`
  term is empty) and bd's wisp filter is spine-DROPPED (Miguel). **`lint`** = the bd template-section rules
  (`temp/beads_rust-main/src/cli/commands/lint.rs`): required `## …` sections per type (Bug ⇒
  Steps-to-Reproduce + Acceptance-Criteria; Task/Feature ⇒ Acceptance-Criteria; Epic ⇒ Success-Criteria;
  other ⇒ none), a case-insensitive heading-substring test over `description` only (prefix `## `/`# `
  stripped), over non-template/non-terminal rows, one finding per missing section — REPLACING the prior
  `blocked=<n>`-lite finding (which bd's `lint` never computes). The lint CANDIDATE set is the active
  non-template set (`ListFilters::default()` = the ACTIVE, NON-TERMINAL, NON-DEFERRED set — SQL
  `status NOT IN ('closed','tombstone','deferred')`, so ALSO admits `blocked`/`draft`/`pinned`/custom-active,
  not just `open`+`in_progress`); bd's default is
  `status=open` only, so unblock's broader active set is a defensible status-agnostic superset (the
  section-presence test is status-agnostic — pin this so the snapshot is intentional). **`changelog`** =
  `closed_since(since)` (window-capable — `since=None` ⇒ all closed), THEN the engine `changelog()`
  composition filters out `is_template` rows (faithful to bd's `list_changelog_issues` template exclusion,
  `sqlite.rs:4014`; `closed_since` stays shared/unchanged — the template filter is an engine-side composition
  step, NOT a widening of the shared read). **`orphans`** = `orphan_candidates()` — **status-agnostic**
  (every row whose `external_ref` matches the commit-hash shape; NOT bd's `status IN ('open','in_progress')`
  narrowing — the faithful FR-15 reading). Every kind emits generic `DiagnosticFinding{label,detail}` rows
  (§1.10 / §5.3), so the enrichment does NOT touch the mcp schema bundle (no `CONTRACT_VERSION` bump —
  §5.4/D25). **Emission order (NFR-14 insta):** stats findings in the fixed order `open, in_progress,
  blocked, closed, ready, deferred, draft, tombstone, pinned, epics_eligible, [avg_lead_time_hours], total`
  (`avg_lead_time_hours` ABSENT when `None`); lint findings outer = issue id ASC, inner = missing sections in
  the fixed required-section DECLARATION order (Bug: `## Steps to Reproduce` THEN `## Acceptance Criteria`);
  the epic rollup `ORDER BY` epic id in SQL (not HashMap-iterated). `since` is a bare method arg; the wire
  default lives only on `DiagnosticsInput::Changelog{since}` (§5.2), the D19 `detect_cycles(blocking_only)`
  precedent.
- **`dependency_tree` / `dependency_graph(roots)` (unblock is the reference — the original has no
  `DepTree`/`GraphEdge` builder).** Forward-edge reachability (`issue_id -> depends_on_id`) over the
  dependency table: `dependency_tree(id)` returns the subtree rooted at `id`; `dependency_graph(roots)`
  returns the union of the reachable subgraphs for a non-empty `roots`, or the whole graph for empty
  `roots`. The emitted edges are sorted by `(from, to, dep_type)` for snapshot stability (NFR-14,
  T2.1) — an unblock-only determinism choice.
- **`list_dependencies(id)` — direct edges of `id`, `ORDER BY depends_on_id ASC, type ASC`**
  (deterministic; render-snapshot authoritative, NFR-14). Backs the `dep list` MCP action (§5.3 `Deps`)
  via the `Session::list_dependencies` forward (§4.1). `remove_dependency` deletes the exact
  `(issue_id, depends_on_id, dep_type)` edge → `DependencyNotFound` if absent; on success
  `Event(DependencyRemoved)`.

The hierarchical-id child allocation (FR-1a, D21) — the read-half of `parent.N` minting:

- **`next_child_number(parent_id)` (T0.6 impl; PRODUCTION-CONSUMED from T1.8, D21) — returns the next
  free child number (high-water + 1) for `parent.N`.** Reads the `child_counters` high-water mark for
  `parent_id` and returns it **+ 1**; on a missing counter row (e.g. imported data with no counter
  seeded) it falls back to a **`LIKE`-ESCAPE legacy scan** (`id LIKE ? ESCAPE '\'` over the existing
  `parent.N` children, the wildcards in `parent_id` escaped) and returns max-child + 1, or 1 when the
  parent has no children yet. **This is the READ-half** whose WRITE-half is the in-tx `child_counters`
  bump inside `create_issue`; the engine allocator runs both under the SAME write permit (D14) so two
  concurrent creates under one parent cannot mint the same `parent.N`. **DISTINCT from the testkit-only
  `testkit_child_high_water` seam** (which exposes the raw high-water mark for tests): `next_child_number`
  is the production method on the trait, and the libsql impl (`unblock-storage/src/libsql/ids.rs`)
  already exists — T1.8 Implement promotes it from `pub(super)` to the trait impl and removes its
  `allow(dead_code)`.

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
| restore (tombstone → active) | `Restored` |
| restore (already-active / missing target) | **none** |
| add / remove dependency | `DependencyAdded` / `DependencyRemoved` |
| comment | `Commented` |
| add / remove label | `LabelAdded` / `LabelRemoved` |

> **NOTE (restore carve-out — the T0.7 oracle is normative here).** `restore` (D20) is a **DEDICATED** path
> that emits **ONLY `Restored`**. Although a tombstone→active restore is a terminal→non-terminal transition,
> restore does **NOT** traverse the generic `update status` rule above: it emits **no `Reopened`** and **no
> `StatusChanged`** (mirroring the way `delete`/`tombstone_one` emits a single `Deleted`, not `StatusChanged`).
> The generic `Reopened` is reserved for the `update`-patch path (a tombstone cannot be patched via `update` —
> the **tombstone-patch guard** fires first, see the `update_issue` bullet above, crud.rs:332-334, the SSOT), so
> the two never collide. This carve-out is the test oracle — do not let an implementer reuse the update
> terminal→non-terminal logic for restore.

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
// It carries the ADDITIVE `id_prefix` field (D21/T1.8, default "ub", `normalize_prefix`-normalized): the
// engine id-allocator reads `ctx.config.id_prefix` at mint time to render `ub-<hash>`/`ub-<slug>-<hash>`
// (the prefix is config-derived, NOT a constant — faithful to the original `IdConfig::with_prefix`).
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

// EngineError source set (additive — NORMATIVE): v1 = 3 transparent sources (StorageError / PolicyError /
// ModelError) + engine-local variants (incl. FeatureNotWired). At T2.4 (D23), a 4th transparent source is added
// additively, cfg-gated behind the default-on `sync` feature:
// `#[cfg(feature="sync")] #[snafu(transparent)] Sync { source: unblock_sync::SyncError }`, forwarding
// `source.code()`/`hint()`/`context()` (CodedError). NON-BREAKING (no v1 signature changes; export_jsonl/import_jsonl
// stop returning FeatureNotWired{"sync"} once wired, import_bd keeps it until T2.5). The `Health { source: HealthError }`
// source lands the same way at T3.3. No CONTRACT_VERSION bump (engine-internal, not an MCP schema change).

// NewIssue is ENGINE-owned (defined in unblock-engine; D21) — the input to the MINTING create path
// `Session::create_issue(NewIssue)`. It carries the domain fields of an interactive create MINUS the
// id (the engine mints it). The fields mirror IssueInput::Create (§5.2) minus the wire-only knobs
// (`quick`/`attribution` are L7 adapter concerns, not engine state). The engine builds an `Issue`
// from these + the minted id under the write permit; it is NOT a model DTO and NOT the import shape.
#[derive(Debug, Clone, Default)]
pub struct NewIssue {
    pub title: String,
    pub description: Option<String>,
    pub issue_type: Option<IssueType>,            // None -> model default
    pub priority: Option<Priority>,               // None -> model default
    pub labels: Vec<String>,
    pub parent: Option<String>,                   // Some -> hierarchical `parent.N` via next_child_number
    pub deps: Vec<Dependency>,                    // added as edges in/after the same tx
    pub due_at: Option<DateTime<Utc>>,
    pub defer_until: Option<DateTime<Utc>>,
    pub estimated_minutes: Option<i32>,
    pub slug: Option<String>,                     // Some -> root id is `ub-<slug>-<hash>` (D21)
    pub ephemeral: bool,
    // --- markdown-captured content fields (D22) — the bulk-markdown parser sets these (faithful to
    //     `markdown_import.rs::apply_section_to_issue`), so scalar create + bulk are full-fidelity.
    //     `create_issue` maps each onto the built `Issue` field of the same name (the domain `Issue`
    //     §1.6 ALREADY carries all four — no model change). `notes`/`owner` are deliberately NOT here:
    //     the markdown has no `### Notes`/`### Owner` section, so the create surface stays exactly as
    //     wide as the markdown authority. They remain reachable via `update` (PatchInput §5.2).
    pub design: Option<String>,                   // <- `### Design`
    pub acceptance_criteria: Option<String>,      // <- `### Acceptance Criteria` / `### Acceptance`
    pub assignee: Option<String>,                 // <- `### Assignee`
    pub agent_context: Option<String>,            // <- `### Agent Context` / `agent-context` / `agent_context`
    // --- bulk symbolic-ref carriers (D22/T2.3) — populated ONLY by the bulk-markdown path; the engine
    //     `create_bulk` resolves them under the write permit. Single `create_issue` leaves them empty
    //     (stand_in_id=None, dep_refs=[]) and keeps using the resolved `deps`/`parent` above (BYTE-
    //     UNCHANGED). The carriers hold the VERBATIM parsed symbolic references the engine resolves:
    pub stand_in_id: Option<String>,              // <- the `### ID` symbolic intra-file handle (NOT the minted id)
    pub dep_refs: Vec<String>,                    // <- the verbatim `### Dependencies` reference strings (type:id / bare / external: / blocked-by / title / stand-in)
    //     The bulk path ALSO sets `parent` to the SYMBOLIC `### Parent` ref (a title / `### ID` stand-in)
    //     when it is an intra-file ref; `create_bulk` resolves `dep_refs` + the symbolic `parent` +
    //     `stand_in_id` against the in-batch title/stand-in maps + committed storage (stand-in → title →
    //     storage order; `blocked-by`→`blocks` flipped at the edge-build step), then merges the resolved
    //     edges with the already-resolved `deps`. §5.2 (`CreateBulk` adapter) builds these; §4.1
    //     `create_bulk` resolves them.
}

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
    pub async fn list_dependencies(&self, id: &str) -> Result<Vec<Dependency>, EngineError>; // backs dep `list` action (§5.3 Deps)
    pub async fn dependency_tree(&self, id: &str) -> Result<DepTree, EngineError>;
    pub async fn dependency_graph(&self, roots: &[String]) -> Result<DepTree, EngineError>; // backs dep `graph` action (§5.2); empty roots = whole graph
    pub async fn detect_cycles(&self, blocking_only: bool) -> Result<Vec<Vec<String>>, EngineError>; // backs dep `cycles` action (§5.2); blocking_only filter (D19)
    pub async fn diagnostics(&self, kind: DiagnosticKind, since: Option<DateTime<Utc>>) -> Result<DiagnosticReport, EngineError>; // FR-15; `since` = changelog window (bare arg, default lives only on the MCP wire — D26/OQ-1, the D19 detect_cycles precedent)
    pub async fn integrity_check(&self) -> Result<Vec<String>, EngineError>; // D27/AF-1 (T3.1) — FR-16 doctor-lite input
    //   Surfaces the existing `Storage::integrity_check` (`PRAGMA integrity_check`): a healthy DB returns an empty
    //   `Vec`; any strings are integrity problems. The ONE corruption signal reachable at T3.1 (the full
    //   Healthy/Drifted/Recoverable/Unsafe taxonomy + `--repair` land ADDITIVELY over `doctor()`/`recover()` at
    //   v1.1; `doctor()` itself is wired lite at T3.3 — D29). A read:
    //   never acquires the write permit (FR-10). BUILD-now, like `diagnostics`. The cli `doctor` command composes
    //   this + `diagnostics(Stats|Lint|Info)` into a doctor-lite report; a non-empty result maps to
    //   ErrorCode::DatabaseError (exit 2) at the cli boundary (spine §2.3 exit table unchanged).

    // --- mutations: each acquires the write permit for its whole tx ---
    pub async fn create(&self, issue: &Issue) -> Result<String, EngineError>;
    //   CALLER-SUPPLIED id — the IMPORT/INTERNAL path. It does NOT mint: it validates the given `Issue`
    //   (incl. its `id`) and inserts it, preserving the id so `content_hash`-keyed import idempotency
    //   (FR-26) is byte-stable. The bulk-markdown / JSONL / bd-import paths build an `Issue` and call this.
    //   Tombstones/imported rows reach storage with their original ids ONLY through here. STAYS (D21).
    pub async fn create_issue(&self, new: NewIssue) -> Result<Issue, EngineError>; // D21 — the MINTING create path
    //   INTERACTIVE create (MCP/CLI quick-create + full create). MINTS the id under the write permit (D21):
    //   - root id `ub-<hash>` (faithful `bd` adaptive-base36) or, with `new.slug`, `ub-<slug>-<hash>` — the slug
    //     is `normalize_slug`'d then `normalize_slug_for_prefix(slug, prefix)`'d to fit `<prefix>-<slug>` within
    //     `MAX_ID_PREFIX_LEN` (=64) or drop to hash-only; the prefix is CONFIG-DERIVED (read from the Session's
    //     held `ResolvedConfig.id_prefix`, default "ub", `normalize_prefix`-normalized; D21), NOT a constant; the
    //     seed carries the resolved actor as `creator`;
    //   - with `new.parent`, the hierarchical `parent.N` via the `Storage::next_child_number(parent)` trait method (§3.2);
    //   the candidate is probed against storage via `get_issue(id).await?.is_some()` (there is no `Storage::exists`) and the mint→probe→insert is ATOMIC under the SAME
    //   permit (so two concurrent creates under one parent cannot mint the same `parent.N` — this is WHY
    //   minting is the engine's job, NOT an L7 adapter's; FR-9 single mutation home). It resolves `new.deps`
    //   into edges added in/after the same tx, then returns the created `Issue` (the MCP quick-create extracts
    //   `.id`). It maps the markdown-captured fields `design`/`acceptance_criteria`/`assignee`/`agent_context`
    //   (D22) onto the built `Issue` fields of the same name (the domain `Issue` §1.6 already carries them — no
    //   model change). `Session::create(&Issue)` is UNCHANGED (the id-preserving import path already accepts a
    //   fully-built `Issue` with those fields). NAME: `create_issue` parallels `Storage::create_issue` (engine mints + delegates to storage);
    //   the two live in DIFFERENT namespaces (`Session::` vs the `Storage` trait), so the name does not clash.
    //   The pure candidate compute (hash/seed/adaptive-length/slug-normalize) lives in unblock-model `id.rs`;
    //   the stateful collision-retry loop + the existence probe (`get_issue(id).await?.is_some()`, NOT a `Storage::exists`) and the `next_child_number` read live in the engine allocator.
    pub async fn create_bulk(&self, records: Vec<NewIssue>) -> Result<Vec<Issue>, EngineError>; // D22/T2.3 — the ATOMIC bulk MINTING create path
    //   The all-or-nothing bulk sibling of `create_issue` — it backs the MCP `create_bulk` action (§5.2) and exists
    //   BECAUSE the minting create path is non-idempotent: a loop of N independent `create_issue` calls that fails on
    //   record #k leaves a partial batch, and re-running re-mints the survivors as DUPLICATES (the import path is
    //   `content_hash`-idempotent; the minting path is NOT). So the whole batch MUST be one atomic unit. It:
    //   (1) acquires the write permit ONCE for the entire batch (NOT once per record);
    //   (2) MINTS every id under the held permit via the SAME engine allocator (`ids.rs`) — the `get_issue` probe
    //       consults BOTH committed storage AND an in-memory already-minted set (intra-batch dedup: two records in
    //       the batch cannot mint the same id). **In-batch per-parent child-counter (D22/T2.3 — the gating fix):** for
    //       hierarchical (`parent.N`) ids the mint phase keeps an in-memory `HashMap<parent_id, u32>` next-child counter.
    //       The FIRST child of a parent seeds from `storage.next_child_number(parent)` (the committed high-water); each
    //       subsequent SAME-parent sibling in the batch uses the INCREMENTED in-memory value — so siblings get DISTINCT
    //       `parent.1, parent.2, …` even though the committed `child_counters` row (read by `next_child_number`) only
    //       reflects committed state and is bumped once, by the single `storage.create_issues` tx. (Without this, two
    //       same-parent siblings would BOTH read the same committed high-water and mint the SAME `parent.N` → an in-tx
    //       `IdCollision` → guaranteed whole-batch rollback — the bulk would be UNBUILDABLE for any 2+ same-parent
    //       children.) The in-tx `IdCollision` guard remains the backstop for an out-of-band racer. **Mint ORDER
    //       (parent-before-child, topological):** a record whose parent is ANOTHER record in the same batch (an
    //       intra-file `### Parent` title / `### ID` stand-in ref) can only mint `parent.N` AFTER its parent's id is
    //       minted, so the mint phase processes records in TOPOLOGICAL order over the intra-batch parent edges (parent
    //       before child); a record whose parent resolves to a pre-existing storage id has no intra-batch parent edge and
    //       mints in file order. A **parent cycle** among intra-batch records (A's parent is B, B's parent is A) → reject
    //       the WHOLE batch with one `ValidationFailed` (it cannot be topologically ordered — there is no valid mint
    //       order). This is faithful to the original's resolution order (`create.rs:855`–`1121` create issues in file
    //       order but DEFER an intra-file parent to a pre-existing-id resolution, then wire it in Phase 2; because each
    //       original `create_issue` COMMITTED before the next, `next_child_number` there saw the just-committed sibling —
    //       the atomic one-tx design loses that, so the in-memory counter + topological order reproduce it);
    //   (3) resolves the 2-phase intra-file deps/parent (stand-in `### ID` / title → the just-minted ids) IN MEMORY
    //       against the minted set;
    //   (4) builds N fully-formed `Issue`s (minted id + fields + engine defaults + the resolved dependency edges)
    //       and runs the FULL `IssueValidator::validate` on each (the same gate `create_issue` runs);
    //   (5) calls `storage.create_issues(&issues, actor)` — the ONE `BEGIN IMMEDIATE` tx (§3.2.1) — and returns the
    //       created `Issue`s.
    //   ON ANY FAILURE (mint exhaustion, an unresolved ref that slipped validation, a raced `IdCollision`, any storage
    //   error) the whole tx ROLLS BACK → ZERO issues persisted → the caller gets ONE `EngineError`/`StructuredError`.
    //   This is the TRUE all-or-nothing (parse-validation AND mint/insert atomicity), NEVER a partial commit. The MCP
    //   `create_bulk` adapter (§5.2) calls THIS — NOT a loop over `create_issue`. The single-record `create_issue`/
    //   `create(&Issue)` paths are UNCHANGED (this is additive).
    pub async fn update(&self, id: &str, patch: &IssuePatch) -> Result<Issue, EngineError>;
    pub async fn delete(&self, plan: &DeletePlan) -> Result<DeletePlan, EngineError>;
    pub async fn restore(&self, id: &str) -> Result<Issue, EngineError>; // FR-1c recovery (D20) — un-tombstone
    //   (acquires the write permit for its whole tx; delegates to storage.restore_issue; the engine supplies
    //   the actor). SEAM: restore is STRUCTURALLY distinct from the reopen=update mapping (§5.2). A tombstone
    //   CANNOT be patched via `update` — the update tombstone-patch guard (crud.rs:332-334) fires first (SSOT:
    //   §3.2.1 `update_issue`), so reopen=update never reaches a tombstone; restore is the dedicated
    //   terminal(tombstone)→active path and emits ONLY `Event(Restored)` (§3.2.1 carve-out). Do NOT unify them.
    pub async fn claim(&self, id: &str, assignee: &str) -> Result<Issue, EngineError>;      // FR-2
    pub async fn defer(&self, id: &str, until: DateTime<Utc>) -> Result<Issue, EngineError>;
    pub async fn undefer(&self, id: &str) -> Result<Issue, EngineError>;
    pub async fn add_dep(&self, dep: &Dependency) -> Result<(), EngineError>;
    pub async fn remove_dep(&self, issue_id: &str, on: &str, ty: &DependencyType) -> Result<(), EngineError>;
    pub async fn close_with_suggestions(&self, id: &str, reason: Option<String>)
        -> Result<CloseOutcome, EngineError>; // returns newly-unblocked issues (FR-11)

    // --- interchange (FR-7/FR-8/FR-26), delegates to unblock-sync ---
    // T2.4: the export/import BODIES delegate to `unblock_sync::{export_jsonl,import_jsonl}` (cfg-gated behind the
    // default-on `sync` feature); the engine maps its public `ImportOptions{dry_run}` into sync's internal
    // `ImportOptions{dry_run, allow_external, on_collision}` at the call site (§4.1 `ImportOptions` note above).
    // T2.5 wires `import_bd`: a `#[cfg(feature="sync")]` body acquires the D14 write permit (MF-4) then calls
    // `unblock_sync::import_bd(&*self.storage, path, &self.unblock_dir, self.actor())`;
    // `#[cfg(not(feature="sync"))]` keeps `FeatureNotWired{"sync"}`. NO `opts` — Skip-only production semantics;
    // funnels through sync's `import_bd` → the shared `apply_records` (D24/F5).
    pub async fn export_jsonl(&self, path: &Path) -> Result<ExportReport, EngineError>; // atomic temp+fsync+rename
    pub async fn import_jsonl(&self, path: &Path, opts: ImportOptions) -> Result<ImportReport, EngineError>;
    pub async fn import_bd(&self, path: &Path) -> Result<ImportReport, EngineError>;     // D16, idempotent via content_hash

    // --- lifecycle / ops ---
    // MigrateOutcome (D27/AF-2, T3.1) — the outcome of an idempotent `Session::migrate`. Engine-local (NOT a
    //   §1.10 DTO; no JsonSchema; the cli maps it onto a DiagnosticReport per D27/AD-2). `from`/`to` are the
    //   on-disk PRAGMA user_version observed before/after; `applied = from != to`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MigrateOutcome { pub from: i64, pub to: i64, pub applied: bool }
    pub async fn migrate(&self) -> Result<MigrateOutcome, EngineError>; // D27/AF-2 (T3.1) — additive engine passthrough
    //   Ensure the schema is at the current baseline, idempotently, and report the from→to delta. Runs UNDER the
    //   single write permit (D14 — migration is a write-path op): reads `from = storage.schema_version()`, runs the
    //   idempotent `storage.migrate()` (a no-op on a current DB), re-reads `to`, returns `{from, to, applied: from!=to}`.
    //   A DB stamped NEWER than this build surfaces the transparent `StorageError::SchemaMismatch` (→ exit 2), never a
    //   fake success. Because the config open facade migrates on open (FR-9 single open path), `applied` is normally
    //   `false` post-open — an honest idempotent signal, not a phantom applied-list. Backs the cli `migrate` command.
    //
    // (OQ-2 RESOLVED: doctor + recover ARE part of the public Session surface.)
    // PRECISION NOTE (T2.2): the mcp `diagnostics` TOOL (§5.1, the 7-kind read path) maps to
    //   `Session::diagnostics(kind, since)` above (BUILD-now, pure-DB; `since` threads the changelog window
    //   — D26/OQ-1) — it is DISTINCT from `doctor()`/`recover()`
    //   here (the T3.3 health seam, FeatureNotWired{"health"} until then). The mcp diagnostics tool does NOT
    //   route through doctor/recover.
    // CLI DOCTOR (D27/AF-1 @ T3.1 → refined by D29 @ T3.3, reconciled spine-first): at **T3.1** the cli
    //   `doctor` command does NOT call `doctor()`/`recover()` (they are the FeatureNotWired{"health"} seam then);
    //   it composes a doctor-LITE report from the BUILD-now reads `diagnostics(Stats|Lint|Info)` + the
    //   `integrity_check()` read above (integrity is the ONE corruption signal reachable at T3.1). At **T3.3
    //   (HEALTH-LITE, D29)** `doctor()` IS wired (the lite aggregation below) and the cli `doctor` ROUTES THROUGH
    //   the wired `doctor()` for OUTPUT — surfacing file-state anomalies too — while PRESERVING the D27/AF-1
    //   exit-2-on-corruption derivation via a SEPARATE, auxiliary `Session::integrity_check()` read used ONLY
    //   for the exit code (F4 mechanism = OPTION (a), orchestrator-pinned): the cli RENDERS `doctor()`'s
    //   `DiagnosticReport` (integrity + file-state findings) for output but DERIVES exit from
    //   `integrity_check()`'s `Vec<String>` so the mutation-proven `doctor_exit(&integrity: &[String])` stays
    //   BYTE-IDENTICAL (non-empty integrity → ErrorCode::DatabaseError exit 2; else exit 0; Lint/file-state
    //   findings stay advisory, NO exit flip). The exit MUST NOT be derived by string-matching the flattened
    //   Info findings; the second `integrity_check()` is a cheap PRAGMA on the already-open DB — zero-regression
    //   preservation of the D27/AF-1 exit asset. The FULL
    //   Healthy/Drifted/Recoverable/Unsafe taxonomy + `--repair` + `.unblock/.recovery/` evidence land ADDITIVELY
    //   over the `doctor()`/`recover()` seam at **v1.1**; `recover()` stays FeatureNotWired through v1. (Earlier
    //   prose put the cli→doctor() routing at T3.1 — that is the T3.3 refinement, not a T3.1 fact.)
    pub async fn doctor(&self) -> Result<DiagnosticReport, EngineError>;  // FR-15/FR-16. v1 pre-T3.3 = SIGNATURE only (returns EngineError::FeatureNotWired{feature:"health"}); **T3.3 (HEALTH-LITE, D29) wires the LITE aggregation** — integrity_check rows + pure file-state classification via unblock-health `run_doctor` → DoctorReport, mapped onto DiagnosticReport REUSING DiagnosticKind::Info (NO new model variant, NO §1.10/CONTRACT_HASH change — F2). The cli doctor routes through this from T3.3 (see the note above).
    pub async fn recover(&self) -> Result<DiagnosticReport, EngineError>; // attempt repair (WAL checkpoint, reindex; reports actions taken). STAYS EngineError::FeatureNotWired{feature:"health"} through v1 (F1/D29) — its body (`--repair` + the `.unblock/.recovery/` evidence writer + the rich repair taxonomy) is **v1.1**, NOT T3.3; wiring a hollow "nothing repaired" report would be the faked success FeatureNotWired forbids.
    pub async fn shutdown(&self) -> Result<(), EngineError>; // flush + close libsql cleanly (FR-17)
}

// CloseOutcome / ImportReport / ExportReport are defined in unblock-model §1.10 and
// re-exported here (CF-A) — NOT redefined. CountBucket / GraphEdge / DepTree /
// DiagnosticReport / DiagnosticFinding / DiagnosticKind likewise come from unblock-model
// via the same re-export. SessionConfig + ImportOptions + NewIssue (D21) + MigrateOutcome
// (D27/AF-2) are engine-owned (above); MigrateOutcome is a plain engine-local return (no
// JsonSchema, NOT a §1.10 DTO), exported from unblock-engine like the peer ImportOptions
// (the TRUE engine-local peer — a plain engine-defined return, no JsonSchema; contrast
// CloseOutcome, which IS a §1.10 model DTO the engine merely re-exports).
```

### 4.2 Write-Semaphore contract (D14 — normative)

- One `Arc<tokio::sync::Semaphore>` with **1 permit** per `Session`. Every mutation `acquire()`s the single permit for the **entire** storage transaction, then releases — serializing all in-process writers (linearizable per FR-9).
- **Reads NEVER touch the permit** (FR-10): they run concurrently against libsql WAL readers while a write holds the permit.
- Scope is **in-process only**: the supported topology is exactly one `unblock serve` per workspace. Concurrent external writers (CLI `migrate`/`doctor` while serve runs, multiple serve) are best-effort via WAL + `busy_timeout`, **not** supported.
- Permit acquisition is **uncancel-safe across the tx boundary**: a dropped future before commit must release the permit and leave the DB committed-or-rolled-back (no partial state) — verified by the SIGTERM-mid-write failure-injection test (NFR-5).
- Property test (FR-9): interleaved mutations through the engine are linearizable; MCP and CLI produce identical results for the same op.

---

## 5. MCP schemas — `unblock-mcp` (L7)

**rmcp 1.7** (`server`, `transport-io`) stdio server (`unblock serve`), thin adapter over `Session`. **7 consolidated tools** (target ≤ 8), resources, prompts. Every tool input/output derives `JsonSchema` + `Serialize`/`Deserialize` — inputs AND outputs ride the schema bundle as per-tool `{input, output}` pairs (D25, §5.3/§5.4); args are schemars-validated with size/rate limits (NFR-18). Discovery (`capabilities`/`schema`) carries `contract_version` (FR-12), and BOTH discovery documents are covered by the single pinned `CONTRACT_HASH` drift gate (D22 clause 8 widened by D25 — §5.4).

### 5.1 Tool taxonomy (7 tools)

| # | Tool | Discriminator | Maps to |
|---|---|---|---|
| 1 | `issue` | `action: create\|create_bulk\|show\|update\|close\|reopen\|delete\|restore` (D22 `create_bulk` is the 8th `issue` ACTION — a discriminator arm, so the **tool** count stays 7 ≤ 8, §6.6) | FR-1a/1b/1c |
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
        #[serde(default)] design: Option<String>,            // D22 — markdown `### Design`; maps to NewIssue.design
        #[serde(default)] acceptance_criteria: Option<String>, // D22 — `### Acceptance Criteria`/`### Acceptance`
        #[serde(default)] assignee: Option<String>,          // D22 — `### Assignee`
        #[serde(default)] agent_context: Option<String>,     // D22 — `### Agent Context`/`agent-context`/`agent_context`
        #[serde(default)] ephemeral: bool,
        #[serde(default)] quick: bool,                  // quick-create -> output is id only
        #[serde(flatten)] attribution: Attribution,     // agent_name/harness/model (capture-only)
    },
    // D21 mapping: the `issue`-tool adapter maps `Create` → the engine-owned `NewIssue` (§4.1) and calls the
    // MINTING `Session::create_issue(NewIssue)` (the ENGINE mints `ub-<hash>` / `ub-<slug>-<hash>` / `parent.N`
    // under the write permit). `quick=true` -> output is `.id` only. NOT `Session::create(&Issue)` — that is
    // the id-PRESERVING import/internal path (FR-26), which never mints. The `design`/`acceptance_criteria`/
    // `assignee`/`agent_context` fields (D22) mirror the new `NewIssue` fields 1:1 (CreateInput field == NewIssue
    // field); `notes`/`owner` are NOT on `Create` (no markdown section sets them — D22 — they stay update-only).
    CreateBulk { markdown: String },   // D22 — {action:"create_bulk", markdown:"..."}; inline document content (NOT a path)
    // D22 mapping: `CreateBulk` is the bulk-markdown import surface — a NEW discriminator on the EXISTING `issue`
    // tool (keeps the tool count at 7, ≤ 8, §6.6 "extend before add"). The adapter:
    //   (1) caps the parsed record count at `Quotas::max_batch` at the preflight (BEFORE any mint — the args-validation
    //       rule pinned in this section's intro above, `PRD NFR-18`);
    //   (2) parses + validates the WHOLE document via a pure mcp-owned `parse_bulk_markdown(&str)` helper —
    //       a byte-faithful port of `temp/beads_rust-main/src/util/markdown_import.rs::parse_markdown_content`
    //       (H2 record / H3 section grammar; implicit-description quirk; `type:id`/bare/`external:`/`blocked-by`
    //       dep encoding; bulleted/checkbox list items) — ALL-OR-NOTHING PRE-MUTATION (FR-1a "rejected
    //       pre-mutation"): a single malformed/unresolvable block rejects the ENTIRE batch with ONE
    //       `StructuredError{code: ValidationFailed, hint, context}` and ZERO writes (deviation from the original's
    //       best-effort per-issue `continue` — the PRD's safe-import discipline wins, NFR-8);
    //   (3) builds a `Vec<NewIssue>` (field-faithful: title/parent/priority/type/description + the D22 markdown-captured
    //       `design`/`acceptance_criteria`/`assignee`/`agent_context`, plus the symbolic `### Dependencies`/`### Parent`
    //       refs carried for the engine to resolve) and calls the ATOMIC `Session::create_bulk(Vec<NewIssue>)` (§4.1) —
    //       NOT a loop over `create_issue`. The ENGINE owns the 2-phase intra-file resolution (faithful port): under ONE
    //       write permit it mints every id (the `get_issue` probe consulting committed state + the in-batch minted set),
    //       resolves each deferred stand-in `### ID` / title → minted id (the `lookup_import_reference` order:
    //       **stand-in id → title → pre-existing storage id**, case-insensitive — faithful to `create.rs:1194`/`:1347`)
    //       IN MEMORY, then inserts the whole batch in ONE `storage.create_issues` tx (§3.2.1) — rollback-on-any-failure
    //       (mint exhaustion, a raced `IdCollision`, any backend error) → ZERO writes. WHOLE-BATCH PRE-MUTATION REJECTION
    //       SET (all-or-nothing, faithful-but-STRICTER than the original's per-record `continue`/`eprintln!` skip): the
    //       engine rejects the ENTIRE batch with ONE `StructuredError{code: ValidationFailed}` (ZERO writes) on ANY of —
    //       (a) an **ambiguous** intra-file ref (a title/stand-in matching >1 record, original `create.rs:1131`/`:1174`);
    //       (b) an **unresolved** ref (no stand-in/title/storage match, original `create.rs:1135`/`:1216`);
    //       (c) a **self-dependency** (a record's resolved dep id == its own minted id, original `create.rs:1227` skip);
    //       (d) a **self-parent** (a record's resolved parent id == its own minted id, original `create.rs:1144` skip);
    //       (e) a **marker-only / empty** dep ref (a `-`/`*`/`+` token or empty after strip, original `create.rs:1234`/
    //       `is_marker_only_dependency`:1376 skip — most are dropped by the parser's `is_marker_only_token`, but any that
    //       survive to resolution reject the batch). The original SKIPPED each of (a)–(e) per-record and created the rest;
    //       unblock refuses the whole batch (NFR-8 safe-import discipline wins over the port). The `blocked-by` dep-type
    //       alias is flipped to `blocks` at THIS engine edge-resolution step (when the edge is built — original
    //       `create.rs:1190`), NOT in the pure parser (the parser preserves the reference string verbatim). bulk-markdown
    //       is INTERACTIVE create (mints fresh ids; does NOT preserve ids, unlike JSONL/bd import which loops
    //       `Session::create(&Issue)`);
    //   (4) output reuses `IssueOutput::Issues` (§5.3, D25) — the Vec of created issues.
    // The bulk-create primitive lives on the `Session`/`Storage` surface BY DESIGN (`Session::create_bulk` over
    // `Storage::create_issues`, §4.1/§3.2) — it is the ONLY way to get one-tx all-or-nothing atomicity (an L7 loop over
    // single `create_issue` calls would commit each independently and could leave a partial batch — fatal here because
    // the mint is non-idempotent: a re-run would duplicate the survivors).
    // ADDING this arm (and the 4 `Create` fields above) changes the `issue` tool's `JsonSchema`, so it BUMPS
    // `CONTRACT_VERSION` (the FR-12 drift gate fires by design — §5.4 / the contract test).
    Show   { id: String },
    Update { ids: Vec<String>, #[serde(flatten)] patch: PatchInput, #[serde(flatten)] attribution: Attribution },
    Close  { id: String, #[serde(default)] reason: Option<String>, #[serde(default)] suggest_next: bool,
             #[serde(flatten)] attribution: Attribution },
    Reopen { id: String, #[serde(flatten)] attribution: Attribution },
    // Seam note (Q2 — T1.4): `Reopen` maps to `Session::update(id, { status: <non-terminal>, .. })`
    // (storage emits the `Reopened` event on a terminal→non-terminal transition, `crud.rs:416-423`).
    // There is deliberately no `Session::reopen` — reopen is an update patch (consistent with the
    // single-id update surface).
    Delete { ids: Vec<String>, #[serde(default)] mode: DeleteModeInput, #[serde(flatten)] attribution: Attribution },
    Restore { id: String, #[serde(flatten)] attribution: Attribution },
    // Seam note (D20 — FR-1c "recoverable"): `Restore` is SINGLE-ID (scalar, non-cascading per D20 DECISION 4 —
    // no `ids: Vec<_>`, unlike `Delete`) and maps to `Session::restore(id)` — NOT to `Session::update`. A
    // tombstone cannot be reopened via the update patch (the tombstone-patch guard fires first — §3.2.1
    // `update_issue`, crud.rs:332-334, the SSOT), so restore is the dedicated un-tombstone path emitting only
    // `Event(Restored)` (§3.2.1 `restore_issue` / §4.1 `Session::restore`). The interface
    // lands now; the `issue`-tool MCP adapter wires this action at T2.2.
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
    Cycles { #[serde(default = "default_true")] blocking_only: bool },  // default TRUE = gating-only (the FR-5 ready view); false = all dep types (integrity/lint, D19) — T2.2 wires it to Session::detect_cycles
    Graph  { #[serde(default)] roots: Vec<String> },
}
fn default_true() -> bool { true }   // serde default for DepToolInput::Cycles.blocking_only (wire-only; the trait/Session take a bare bool)

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
                            priority_min/max, text_contains, include_deferred, include_closed, limit, offset.
                            `include_tombstone` is deliberately NOT mirrored — it never rides the WIRE mirror
                            (no `QueryInput` field); its two in-process consumers are the sync export and the mcp
                            `issues/{id}` not-found suggestion scan (T2.6/D25); the mirror stays intentionally
                            NON-surjective. Do NOT add it to the
                            wire (no CONTRACT_VERSION bump; FORK-2 guarantee). */ }
```

### 5.3 Output shapes (D25/FORK-1B — per-tool, MATERIALIZED, NORMATIVE)

The output surface is a family of REAL, mcp-owned types — the single output authority, not documentation.
Tool bodies construct their structured success payload AS an arm of their tool's union (or as the tool's
single output type). All unions are `#[serde(untagged)]` ⇒ the wire bytes are IDENTICAL to serializing the
arm's value directly, so materializing changes NO wire byte and NO golden except the schema bundle. `Box` is
serde- and schemars-transparent (wire bytes + published schema unchanged); the boxed arms (`Issue`, `Close`)
keep `clippy::large_enum_variant` clean under CI `-D warnings` (`ci.yml:63`) — `CloseOutcome` inlines a full
41-field `Issue` (`crates/unblock-model/src/results.rs:46-51`). Each
tool's `schema_for!(<output>)` is its §5.4 `ToolSchemas.output`: a new output shape must join its tool's
union to be returnable, and joining it moves the D25 gate (`CONTRACT_HASH` → `CONTRACT_VERSION` bump).
*(Supersedes the pre-D25 single `ToolOutput` union sketch, which also missed the landed `delete`/`added`/
`removed` shapes; the name `ToolOutput` survives only in historical decision/task records.)*

```rust
// issue — the 5 success shapes of the 8 actions:
#[derive(Serialize, JsonSchema)] #[serde(untagged)]
pub enum IssueOutput {
    Id(IdOnly),                        // quick-create
    Issue(Box<Issue>),                 // create / show / reopen / restore
    Issues(Vec<Issue>),                // multi-id update; ALSO create_bulk (D22 — N created issues)
    Close(Box<CloseOutcome>),          // close — suggest_next -> newly_unblocked (FR-11)
    Delete(DeletePlanOutput),          // the resolved delete plan (was the ad-hoc delete_plan_json)
}
#[derive(Serialize, JsonSchema)] pub struct IdOnly { pub id: String }
#[derive(Serialize, JsonSchema)]
pub struct DeletePlanOutput { pub mode: DeleteModeOutput, pub targets: Vec<String>, pub cascade_children: Vec<String> }
#[derive(Serialize, JsonSchema)] #[serde(rename_all = "snake_case")]
pub enum DeleteModeOutput { Tombstone, Cascade, Hard, DryRun }   // From<DeleteMode>; wire == the old strings

// claim / defer — output = Issue (no union needed).

// query:
#[derive(Serialize, JsonSchema)] #[serde(untagged)]
pub enum QueryOutput { Issues(Vec<Issue>), Counts(Vec<CountBucket>) }  // Issues = list/ready/blocked/search/stale

// dep:
#[derive(Serialize, JsonSchema)] #[serde(untagged)]
pub enum DepOutput {
    Added(DepAdded),                   // {"added":true}   (was ad-hoc json!)
    Removed(DepRemoved),               // {"removed":true} (was ad-hoc json!)
    Deps(Vec<Dependency>),
    Tree(DepTree),                     // tree AND graph (Session::dependency_graph returns DepTree)
    Cycles(Vec<Vec<String>>),          // ordered cycle-path witnesses (§3.2.1, D19)
}
#[derive(Serialize, JsonSchema)] pub struct DepAdded { pub added: bool }
#[derive(Serialize, JsonSchema)] pub struct DepRemoved { pub removed: bool }

// sync — output = SyncOutput (G-23a): mcp-owned wrapper over the two model report DTOs.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutput { Export(ExportReport), Import(ImportReport) }

// diagnostics — output = DiagnosticReport (§1.10). v1 per-kind findings are ADVISORY generic
// DiagnosticFinding{label,detail} rows (D26/OQ-2): stats/lint/changelog/orphans express every
// counter/warning/entry as {label,detail}, so the taxonomy enrichment stays inside the existing
// schema (NO CONTRACT_VERSION bump). A richer/nested per-kind DTO is a v1.1 structure seam — it
// WOULD enter the hashed bundle (§5.4) and force a version bump, so it is deliberately deferred.

// The in-band ERROR output is NOT an arm of any union: every tool may return a `StructuredError` with
// `is_error = true` (FR-11 — always valid JSON even on error). It is published ONCE, bundle-level, as
// `SchemaBundle.error` (§5.4) — the rmcp `is_error` flag is the channel discriminator (§5.6).
```

### 5.4 Resources

```
unblock://issues/{id}        -> Issue            (FR-4)
unblock://issues/ready       -> Vec<Issue>       (default-complete ready set; agent entrypoint)
unblock://issues/blocked     -> Vec<Issue>
unblock://capabilities       -> Capabilities     (FR-12; tools/resources/prompts + error/exit-code/hint-shape map)
unblock://schema             -> SchemaBundle     (FR-12; JsonSchema per tool I/O — per-tool {input, output} pairs + the shared error schema, D25)
```

**`{id}` not-found (D25/FORK-3A — NORMATIVE, faithful to the original `issue_not_found_resource`):**
a missing/unknown `{id}` yields a `StructuredError{code: IssueNotFound}` whose hint folds fuzzy
near-miss suggestions via the public `unblock_error::find_similar_ids(id, <full id corpus>, 3)` —
candidate corpus = every issue id in the DB, closed/tombstoned included (the original `get_all_ids`
semantics; one read-only fetch on the error path, no write permit, FR-10; the crate plan pins the
exact `Session` read — `include_deferred`+`include_closed`+`include_tombstone` all true reach the
full corpus, D23), cap 3 (the original `structured.rs::issue_not_found`), with the
"Did you mean …?" / list-discovery-fallback hint family and `context{searched_id, similar_ids}`;
a FAILED corpus scan surfaces the scan error, not the not-found (the original's pinned behaviour).
At the rmcp boundary, not-found (unknown URI or `IssueNotFound`) maps to
`ErrorData::resource_not_found` (**-32002**) carrying the `StructuredError` as data; true internal
faults stay `INTERNAL_ERROR` (-32603).

```rust
// `Deserialize` on `Capabilities`/`ErrorCodeDescriptor` is REQUIRED, not illustrative: the FR-12
// e2e parses both documents CLIENT-side (§5.4 gate / the T2.6 drift e2e); the landed code already
// derives it — materializing this sketch exactly must not regress the parse path.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    pub contract_version: String,                  // bumped when EITHER discovery document changes (FR-12/D25 gate)
    pub tools: Vec<ToolDescriptor>,
    pub resources: Vec<ResourceDescriptor>,
    pub prompts: Vec<PromptDescriptor>,
    pub error_codes: Vec<ErrorCodeDescriptor>,     // code -> exit_code, retryable, hint_shape (D25/FORK-4B)
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ErrorCodeDescriptor {
    pub code: String,          // "ISSUE_NOT_FOUND", ... (§2.2 as_str)
    pub exit_code: u8,         // 0..=8 (§2.3 parity)
    pub retryable: bool,       // == ErrorCode::is_retryable
    pub hint_shape: HintShape, // §2.2 — the static per-code hint shape (snake_case string on the wire)
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ToolSchemas {
    pub input: Value,          // schema_for!(<Tool>Input) — draft 2020-12
    pub output: Value,         // schema_for!(<tool's §5.3 output>) — the SUCCESS shape(s)
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SchemaBundle {
    pub contract_version: String,
    // 7 × per-tool {input, output} (§5.1 order). Outputs (§5.3): issue=IssueOutput, claim=Issue,
    // defer=Issue, query=QueryOutput, dep=DepOutput, sync=SyncOutput, diagnostics=DiagnosticReport.
    pub issue: ToolSchemas,
    pub claim: ToolSchemas,
    pub defer: ToolSchemas,
    pub query: ToolSchemas,
    pub dep: ToolSchemas,
    pub sync: ToolSchemas,
    pub diagnostics: ToolSchemas,
    // D25 — the shared in-band error output every tool may return with `is_error = true` (FR-11):
    // schema_for!(StructuredError), published ONCE (the rmcp `is_error` flag is the channel
    // discriminator — folding it into each per-tool union would misstate the discriminator and
    // duplicate one shape 7 times). Transitively via $defs the bundle pins IdOnly, SyncOutput,
    // ExportReport/ImportReport, CountBucket, CloseOutcome, DepTree, Dependency, DiagnosticReport,
    // StructuredError, and Issue — so the resource payloads above are pinned too.
    pub error: Value,
}
```

**The FR-12 drift gate (D22 clause 8, widened by D25 — NORMATIVE).** ONE pinned digest,
`CONTRACT_HASH` (unblock-mcp `options.rs`, superseding `SCHEMA_BUNDLE_HASH`, in lockstep with
`CONTRACT_VERSION`), = SHA-256 over `serde_json::to_vec(&(capabilities(), schema_bundle()))` — the
ordered two-document tuple. The contract test asserts (i) both builders stamp `CONTRACT_VERSION`
and (ii) the recomputed digest equals `CONTRACT_HASH`. **Non-vacuous both directions:** any byte
change in EITHER discovery document moves the digest (edit without bump → fail), and
`contract_version` is a stamped field of BOTH documents (a bump alone also moves the digest — a
bump without re-pin fails too). **Version-coupled artifact set = exactly the two discovery
documents** + their transitive JsonSchema `$defs` closure (so `unblock-error` `ErrorCode`-set /
exit / retryable / hint-shape changes, edits to the capabilities-document descriptor copies, and
ANY output-DTO field change force a `CONTRACT_VERSION` bump — the D25 reversal of D24 clause 3's
forward rule). **Golden-only set
(re-blessable, NEVER version-coupled):** the 3 prompt rendered-message snapshots (§5.5 — prompt
content is guidance, not machine contract; prompt NAMES are version-coupled — live-vs-builder
parity + hash; tool/resource/prompt DESCRIPTION strings are version-coupled only in their
capabilities-document copies) and the redundant per-crate schema/exit-table goldens.

### 5.5 Prompts

```
triage                 -> guided triage workflow
plan_next_work         -> drives ready -> claim selection
close_with_suggestions -> close + surface newly-unblocked
```

### 5.6 Error mapping at the MCP boundary

Any `EngineError` → `StructuredError` (§2.4) attached as rmcp tool error **data** (`code`/`message`/`hint`/`retryable`/`context`), parallel to the CLI 0–8 exit codes. A failed tool call still returns **valid JSON** (the shared in-band error output — `SchemaBundle.error`, `is_error=true`; §5.3/§5.4 D25). Oversized/invalid args are rejected by schemars validation before reaching the engine (NFR-18); blast radius confined to the workspace.

---

## 5b. CLI lifecycle surface — `unblock-cli` (L7)

`unblock-cli` owns the `unblock` binary and depends on `unblock-mcp` (§0.1). Lifecycle/ops commands (NOT the issue-data verbs, which go through MCP tools / the engine): the v1 command set is **`serve, migrate, doctor, version, init, agents, update`** — all lifecycle/ops. (This widens the PRD D3 list, which named only `serve/migrate/doctor/version`; `init`/`agents`/`update` ship in cli at M3 per the cli plan / T3.1 / T3.6.) The T3.1 command behaviours below are ratified by **D27 (PRD §4)** and reconciled spine-first against the live surface on `main` @ b384103.

```rust
// commands/update.rs — the v1 self-update command (FR-25 / D17). Command token is `unblock update`
// EVERYWHERE (Command::Update, UpdateArgs, help snapshots). The Cargo FEATURE is named "self-update"
// (the "self-update" feature enables the "unblock update" command — feature name ≠ command name by design).
pub struct UpdateArgs { /* --check, --version <tag>, --yes */ }
```

**The CLI is a pure `CliOverrides` forwarder (D27/AD-3).** `unblock-config` owns ALL layering (CLI > env `UNBLOCK_*` > `.unblock/config.toml` > defaults), `.unblock/` discovery, path confinement, and prefix normalization; the CLI does NOT re-implement precedence. The single CLI-owned resolution seam is **clap `env`**: `--dir`→`UNBLOCK_DIR` and `--actor`→`UNBLOCK_ACTOR` bind via clap `env` (so `--flag > UNBLOCK_*` is free) and `GlobalArgs::to_overrides()` is the ONE place clap types cross into `CliOverrides`. `UNBLOCK_OUTPUT_FORMAT` is parsed strictly by config's env layer (the single strict parse site); the `--output/-o` flag forwards via `CliOverrides.output_format` so `--flag > env` still holds inside config's resolver. `CliOverrides` has NO `id_prefix` field, so `init --prefix` is NOT forwarded — it is written into the scaffold `config.toml` text (see `init`).

**serve (FR-20 / D27/AD-4).** Opens a `WorkspaceContext` via `open_with_storage_with_cli`, builds a `SessionConfig { jsonl_export: ctx.config.jsonl_export, import_on_open: false, remote: false }` (`import_on_open` MUST stay false in v1 — `true` returns `FeatureNotWired{"sync"}`, exit 1), installs the FR-17 shutdown handle, opens the `Session` + `with_shutdown_flag`, then calls the LIVE **2-arg** `unblock_mcp::serve(Arc<Session>, ServeOptions { cancel, quotas: Quotas::default(), instructions })` (§0.1 — transport is internal `stdio()`). On EOF/first signal the `CancellationToken` cancels → `serve` returns `Ok` → `session.shutdown()` (drain the permit, clean libsql close). stdout carries ONLY MCP framing (logging is stderr-only, NFR-14). Single-serve-per-workspace (D14).

**migrate (D27/AF-2).** Opens the context (the facade already migrates on open), opens the `Session`, calls the NEW `Session::migrate() -> MigrateOutcome` (§4.1) under the write permit, builds a CLI-local `MigrateReport { database, schema_from, schema_to, applied }`, maps it onto a `DiagnosticReport { kind: Info, findings }` and emits via `Renderer::diagnostics`. Exit 0 on success; a newer-than-build DB → transparent `SchemaMismatch` → exit 2. Idempotent (`applied` normally `false` post-open).

**doctor (D27/AF-1 — doctor-LITE).** Opens the `Session` and composes `diagnostics(Stats|Lint|Info)` + the NEW `Session::integrity_check()` read (§4.1) into a CLI-local `DoctorReport`, mapped onto a `DiagnosticReport { kind: Info, findings }`. At T3.1 it does NOT call `Session::doctor()`/`recover()` (the `health` seam); at **T3.3 (HEALTH-LITE, D29/F4)** it ROUTES through the now-wired `Session::doctor()` (adding file-state anomalies). **Non-zero exit only on detected corruption:** a non-empty `integrity_check` → `ErrorCode::DatabaseError` (exit 2; §2.3 unchanged, no new code); Lint/orphan findings are advisory (no exit flip); else exit 0. `--repair` + the full taxonomy land at **v1.1**.

**version (D27/AD-5).** Runs with NO workspace. Emits `VersionReport { version, build, commit: Option<_>, rustc: Option<_>, target: Option<_>, features }` from `build.rs`-emitted `option_env!("UNBLOCK_BUILD_*")` (absent = `None`) — NO git invocation / git crate / network / GitHub update-check (NFR-6/D13; the update-check lives only in `unblock update`). Rendered via the same to-`DiagnosticReport` path (kind `Version`).

**init (D27/AF-3).** Creates exactly `.unblock/config.toml` (hand-written TOML — `ProjectConfig` is `Deserialize`-only — seeded with the `unblock_model::normalize_prefix`-normalized `--prefix`, default `ub`; the CLI takes a direct `unblock-model` dep) + a migrated empty `unblock.db` opened through `open_with_storage_with_cli` (FR-9 no-drift). NO `.gitignore`/`metadata.json`/`issues.jsonl` (D13/NFR-6/model-B). **Clobber guard:** refuse if `config.toml` OR `unblock.db` is already present without `--force` → a CLI-local `CliError::AlreadyInitialized` → `ErrorCode::AlreadyInitialized` (exit 2; `ConfigError` has no such variant). Reports a CLI-local `InitReport`.

**agents (FR-14).** A pure file op (SEPARATE from init): resolve-only open (`open_workspace_with_cli`, no DB) to find `workspace_dir`, then merge an idempotent managed AGENTS.md block (delimited markers) describing the MCP wiring (`unblock serve`, stdio transport, tool set). Writes a terse "wrote X" note to stderr.

**error boundary (D27/AF-4).** `exit.rs` owns the 0–8 cast (there is no `From<ExitCode> for std::process::ExitCode` in `unblock-error`). Transparent-`CodedError` sources (`EngineError`/`ConfigError`/, with AF-4, `RenderError`) bridge via `(&err).into()`. `McpServerError` (`Transport`/`RunLoop`, `#[non_exhaustive]`) is mapped EXPLICITLY to `ErrorCode::InternalError` (exit 1) — a serve failure is internal, not a user IoError (NOT exit 8). CLI-local variants: `AlreadyInitialized` (exit 2), scaffold/agents `Io` (exit 8). **NFR-14 + FR-11 split:** in json/robot the structured error renders to STDOUT (always valid JSON even on error); in plain a human `error[CODE]: message` line goes to STDERR.

**Self-update seam (FR-25, D17):** the `unblock update` command uses **`axoupdater` as a library dependency of `unblock-cli`** (NOT a separate `unblock-update` crate). Updates are verified via **GitHub artifact attestations** (NFR-17), not an embedded key. Gated behind the **`self-update`** Cargo feature (default-on); `--no-default-features` drops the feature and thus the `unblock update` command and its network surface (CF-K).

- The CLI maps each `EngineError`/boundary error → `StructuredError` and exits with its 0–8 exit code (§2.4); structured output to stdout, diagnostics to stderr (NFR-14).
- **Report render (D27/AD-2):** the four report structs (`VersionReport`/`MigrateReport`/`DoctorReport`/`InitReport`) are CLI-LOCAL private types (deriving `serde::Serialize`; NOT §1.10 contract types — §6.1 binds only re-exported §1.10 DTOs). Each maps onto a `DiagnosticReport` and is rendered by `Renderer::diagnostics` (the ONE live lifecycle-render path, all five formats, FR-11 uniform) — NOT a generic `render<T>`.

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
MCP tool count stays ≤ 8; new domain surface extends existing tools by discriminator before adding tools (RK-3). D22's `create_bulk` is a NEW `action` arm on the existing `issue` tool (NOT a new tool) — the live `list_tools` golden (T2.3) keeps the count at 7.

### 6.7 Safety / no-git / no-default-network
`forbid(unsafe_code)`, no git crate / `Command::new("git")` anywhere (NFR-6/NFR-9); network/TLS only behind the non-default `remote` feature (D15).
