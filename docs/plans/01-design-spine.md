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

`unblock-cli` **depends on** `unblock-mcp`. The CLI owns the `unblock` binary (incl. `unblock mcp`); `unblock-mcp` is a **library** that exposes `run_mcp_server(session: Arc<Session>, opts: McpServerOptions) -> Result<(), McpServerError>` and the tool/resource/prompt registry. **`run_mcp_server` signature (LIVE — D27/AD-4, reconciled spine-first against T2.2/PR #387):** it is the **2-arg** `run_mcp_server(Arc<Session>, McpServerOptions)` — the transport is bound **internally** to `stdio()` (the caller does NOT pass a transport). **Since D43 that internal binding is a DUPLICATE-KEY-SCANNING transport wrapping the stdio byte streams** (it owns the read framing, because detection must happen before `serde_json` collapses a duplicated key); the **public 2-arg contract is UNCHANGED** — the byte-level bound change is confined to the private `run_mcp_server_handler` and the two `test-util` duplex helpers, and shutdown is a `tokio_util::sync::CancellationToken` carried in `McpServerOptions.cancel`. **Cancel semantics (NORMATIVE — D38, corrects the earlier unconditional "`.cancel()` … returns `Ok(())`" claim):** `.cancel()` drains in-flight work and returns `Ok(())` **only if the rmcp `initialize` handshake already completed**; rmcp's `serve_server_with_ct` wraps the WHOLE handshake in a `select!` against `ct.cancelled()`, so a cancel landing **during** the handshake returns `Err(ServerInitializeError::Cancelled)` → surfaces as `McpServerError::Transport`. The contract is therefore: **`run_mcp_server` returns `Ok(())` OR `Err(Transport(Cancelled))` on cancel, depending on the handshake phase — BOTH are normal cooperative-shutdown outcomes**, both MUST reach `session.shutdown()`, and the caller MUST NOT treat the `Err` as an independent fault (see §5b `mcp` for the exit rule + the no-hang invariant). The earlier 3-arg `run_mcp_server(session, transport, shutdown)` / bespoke `ShutdownToken` sketch **never shipped** and is superseded here. The direction is fixed **cli → mcp** and **never mcp → cli** — this is the single L7↔L7 edge that determines acyclicity, and it is now a decision (not an assumption). The cli plan's Open Question Q1 is **RESOLVED** by this line. README §2 and §0 draw this edge as settled and are correct.

The same edge also carries `pub fn agents_digest() -> AgentsDigest` (+ its `ToolDigest`/`ToolAction`/`ResourceDigest`/`PromptDigest`/`ErrorCodeDigest` sub-types, §5.4, D33) — a pure derived-view helper consumed only by `unblock-cli`'s `unblock agents` renderer; it is NOT a new edge (same `cli → mcp` direction) and is NOT part of the hashed `CONTRACT_HASH` tuple.

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
    CommentEdited, CommentRedacted,   // D37 (v1) — comment update (provenance-preserving) / delete (soft-redact); wire "comment_edited"/"comment_redacted"
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub comments: Vec<Comment>,        // populated v1 (D37 — hydrated on all 7 read paths)
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
pub struct Comment {                       // v1 surface (D37); FLAT (no threading — FORK-T)
    pub id: i64,
    pub issue_id: String,
    pub author: String,                    // = the session actor at the MCP surface (FORK-M1b)
    #[serde(rename = "text")] pub body: String,   // JSON key "text" (bd parity); masked to "" on redact
    pub created_at: DateTime<Utc>,
    // D37 — provenance-preserving edit (D-D): None = never edited; Some = last-edit instant. `add` leaves this
    //       NULL (`add`'s own INSERT is create-time-only — see §3.2.1 MUST-1 SCOPE); ONLY `update` sets it = now.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub updated_at: Option<DateTime<Utc>>,
    // D37 — soft-redact (D-E): None = live; Some = redacted. The PRESENCE is the "is redacted" flag (mirrors the
    //       tombstone `deleted_at`); on redact the row is KEPT + `body` masked to "". Wire redact form =
    //       `redacted_at` present + `"text":""` (NO extra top-level bool). The CommentRedacted audit Event
    //       RETAINS the original body (provenance — FORK-redact-wire).
    #[serde(default, skip_serializing_if = "Option::is_none")] pub redacted_at: Option<DateTime<Utc>>,
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
- **`sync_equals(&self, other) -> bool`** — semantic equality for import/export boundaries. Compares the full synced payload (incl. `due_at`, `defer_until`, tombstone fields, compaction fields, and relations **order-independent**: labels deduped+sorted; deps and comments sorted by a fixed key tuple). Treats `compaction_level == None` as `0`. Ignores volatile audit-only fields. This is the import "is this line a no-op?" predicate, not derived `PartialEq`. **D37 (comments) — normative:** the comment comparator COMPARES `body` + `redacted_at` (real synced state) but IGNORES `updated_at` (volatile-audit-like, exactly as `Issue.updated_at` is ignored); `comment_sort_key` gains `redacted_at` (FORK-M2). `content_hash` is UNAFFECTED — comments are already excluded (above) — so a comment add/edit/redact NEVER moves it (FR-26 idempotency intact).
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

// Comment validation (FR-6/D37) — the model owns the comment rule set ONCE (the §1.9 single-home rule).
// TWO PUBLIC entry points because the two engine call sites carry different field sets (spine §4.1):
//   - `add_comment(issue_id, body)` has issue_id + author (= self.actor) + body -> validate_comment.
//   - `update_comment(comment_id, body)` has ONLY a comment id + body — there is no issue_id on that path
//     and no `Storage::get_comment(comment_id)` (§3.2) to fetch one — so it calls validate_body.
// COMPOSITION (NORMATIVE — validate_comment must NOT call validate_body): validate_body returns an
// ALREADY-SEALED Result, so the natural `validate_body(body)?` inside validate_comment would be
// FAIL-FAST — the FR-11 wire `context["fields"]` would carry ONE entry where IssueValidator carries N,
// silently breaking the D-E1 UNIFORM AGGREGATE carrier (§2.1). Instead BOTH public entry points call the
// PRIVATE `body_rules`, each sealing its OWN aggregate: the body rule set stays single-homed AND every
// CommentValidator error is a full multi-fault aggregate, exactly like `IssueValidator::validate`.
pub struct CommentValidator; // pure; no I/O.
impl CommentValidator {
    // THE single home of the body rule set. Pushes into the CALLER's aggregate; never seals.
    // body non-empty when trimmed [FieldError { field: "content" }] + reject NUL (SQLite compat);
    // body otherwise UNBOUNDED (the L7 MCP `Quotas.max_string_len` 64 KiB is the transport cap).
    fn body_rules(body: &str, fields: &mut Vec<unblock_error::FieldError>); // PRIVATE — mirrors the
    // `fields: &mut Vec<FieldError>` helper shape IssueValidator already uses (one seal at the end).
    // The update path: body only; seals its own ValidationFailed { fields }.
    pub fn validate_body(body: &str) -> Result<(), unblock_error::ModelError>;
    // The add path: `body_rules` + author + issue_id ALL collected into ONE vec, sealed ONCE.
    //   author  -> DELEGATES the bound/NUL/control-char rules to `validate_actor` above (their single
    //     home), RELABELS the returned FieldError's `field` "actor" -> "author" (FieldError.field is a
    //     pub String), and ADDS the non-empty-when-trimmed rule `validate_actor` deliberately does NOT
    //     enforce (its contract: the RESOLVED actor is already non-empty via the config precedence
    //     chain). So the bound stays ACTOR_MAX_CHARS = 200 CHARS — a deliberate adaptation of bd's
    //     `len() > 200` BYTES-on-untrimmed rule; bd's `id <= 0` rule is DROPPED (the id is storage-minted
    //     here, never caller-supplied).
    //   issue_id -> non-empty when trimmed.
    // The FieldError NAMES are WIRE CONTRACT (FR-11 `context["fields"][].field`, D-E1 §2.3; bd-compatible):
    //   body -> "content"  ·  author -> "author"  ·  issue_id -> "issue_id".
    pub fn validate_comment(issue_id: &str, author: &str, body: &str) -> Result<(), unblock_error::ModelError>;
}

// Shared contract type that BOTH policy and storage need lives here (CF-11):
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(pub String); // ready/blocked projection cache key contract.

// --- external dependency TARGET (NORMATIVE — D45; the SINGLE Rust home) ---
// Until D45 this concept had no definition anywhere: no FR, no NFR and no D-id named it, and its
// only normative statements were three parentheticals (§3.2.1 `blocked_issues`, §3.2.1's D44 scope
// note, §4.1 `NewDep.depends_on_id`). It was open-coded in two DISAGREEING dialects — Rust
// `starts_with("external:")` (case-SENSITIVE, `crates/unblock-engine/src/session/bulk.rs:271`) and
// SQL `NOT LIKE 'external:%'` (ASCII-case-INSENSITIVE, `crates/unblock-storage/src/libsql/query.rs:292`,
// `:316`, `:317`, `:350`, `:351`) — so `EXTERNAL:jira-1` was an external blocker to the ready query
// and an ordinary id to the bulk parser. D45 pins ONE predicate and one case rule.

/// The reserved dependency-TARGET prefix, exactly these 9 bytes, lowercase in its canonical form.
/// A `depends_on_id` carrying it names a blocker OUTSIDE this workspace (a ticket in another
/// system). It is a LEGITIMATE target that no row can ever satisfy — which is why
/// `dependencies.depends_on_id` deliberately carries NO foreign key (§3.2.1) and why the
/// ready/blocked passes exclude it (§3.2.1 `blocked_issues`).
pub const EXTERNAL_TARGET_PREFIX: &str = "external:";

/// Whether a dependency TARGET (`Dependency.depends_on_id`, `NewDep.depends_on_id`, a parsed
/// `dep_ref`) names an external blocker.
///
/// **ASCII-case-INSENSITIVE (NORMATIVE — D45):** `external:`, `External:` and `EXTERNAL:` are the
/// same target class. This is not a taste choice — it is forced by invariant (4) below.
/// The byte comparison is EXACTLY equivalent to the SQL twin `depends_on_id LIKE 'external:%'`:
/// SQLite's `LIKE` folds ASCII only, the prefix contains no non-ASCII byte, and an ASCII byte is
/// always exactly one character — so the two accept precisely the same set of strings, including
/// on non-ASCII near-misses (fullwidth `ＥＸＴＥＲＮＡＬ:`, dotless `ı`), which BOTH reject.
#[must_use]
pub fn is_external_target(target: &str) -> bool {
    let p = EXTERNAL_TARGET_PREFIX.as_bytes();
    let t = target.as_bytes();
    t.len() >= p.len() && t[..p.len()].eq_ignore_ascii_case(p)
}
```

**External-target invariants (NORMATIVE — D45).**

1. **`unblock-model` (L0) is the only lawful home.** The predicate must be reachable from BOTH `unblock-storage` (L2 — the in-transaction write guard, §3.2.1), `unblock-sync` (L3 — the export blocker-closure walk, which must NOT follow an `external:` target because that is not a row, §1.10) and `unblock-engine` (L5 — the bulk dep-ref resolver and the dangling-dependency diagnostic, §4.1). `unblock-storage` may depend on model + error ONLY (`xtask/src/layering.rs`), so any home above L0 is a layering violation. It sits in `unblock-model` `src/id.rs`, beside `parse_id`, and is re-exported flat from `lib.rs` like every other model helper.
2. **`parse_id` is UNCHANGED.** `parse_id("external:jira-123")` yields prefix `external:jira` and keeps doing so (`crates/unblock-model/src/id.rs:457-458`): an external target is not an unblock id and is never parsed as one. `is_external_target` is orthogonal to id parsing — it classifies a dependency TARGET STRING, never an issue id.
3. **A SQL twin REMAINS, because SQL cannot call Rust — so the single home is PARTIAL BY CONSTRUCTION, and both halves agree only BY CONTRACT.** The five `NOT LIKE 'external:%'` predicates in `crates/unblock-storage/src/libsql/query.rs` (`:292`, `:316`, `:317`, `:350`, `:351`) stay; the spine states the split rather than pretending the Rust `fn` is the only site. The two halves are kept honest by an obligation, not by hope: the NFR-16 Storage contract suite MUST carry an EQUIVALENCE cell that, for a fixed probe corpus, asserts `is_external_target(s)` equals the verdict the DATABASE ITSELF returns for `SELECT ?1 LIKE 'external:%'`. The corpus is pinned here so it cannot silently shrink: `""`, `"external"`, `"external:"`, `"external:jira-1"`, `"EXTERNAL:jira-1"`, `"ExTeRnAl:jira-1"`, `"externally:x"`, `"externaL:"`, `" external:x"` (leading space), `"ub-external:x"`, and two non-ASCII near-misses (`"ＥＸＴＥＲＮＡＬ:x"`, `"externaı:x"`). A future change to either side that breaks the equivalence turns that cell red.
4. **The write guard is NEVER stricter than the read side (the invariant that decides the case rule).** For every target string `t`: if the ready/blocked SQL treats `t` as external and therefore never blocking (§3.2.1 `blocked_issues`), then `is_external_target(t)` MUST be true. A guard that rejected a `t` the read side already treats as a legitimate external blocker would refuse a write the store is happy to serve — the exact asymmetry the two open-coded dialects had shipped. Since the SQL side is ASCII-case-insensitive TODAY and is not being narrowed in a patch release (that would be a behaviour change on a GA-frozen read path, D35), the Rust predicate is case-insensitive. The invariant is DIRECTIONAL: the read side may be looser than the guard in the future, never the reverse.
5. **Every open-coded recognition is retired — and the swap at the PARSER is a REFACTOR, not the behaviour change.** `crates/unblock-engine/src/session/bulk.rs:271` calls `is_external_target` instead of `starts_with("external:")`. **Stated precisely, because an implementer who edits only this line ships nothing:** for `EXTERNAL:jira-1` the shipped `parse_dependency` (`bulk.rs:270-284`) already falls to its else-branch, splits on `:`, finds `validate_dependency_type("EXTERNAL")` FALSE (it resolves to `DependencyType::Custom`, `bulk.rs:249-255`), and returns `("blocks", "EXTERNAL:jira-1")` — byte-identical to the external branch modulo `trim`. So the swap is observationally a NO-OP at this site; it is required because a single predicate is the only way to stop the two dialects re-diverging, not because it changes an output. **What actually delivers the case-insensitivity and the external relaxation is the RESOLVER carve-out** specified at §5.2 rejection-set item (b) and the matching skip in the engine's pre-transaction probe (`crates/unblock-engine/src/session/write.rs:497-522`): an `is_external_target` ref is carried VERBATIM and never resolved against anything. That is the behaviour change; this invariant is its precondition.
6. **The carve-out is per-TARGET, never per-EDGE-TYPE.** `is_external_target` is applied to every dependency target on every path, including a `parent-child` target — so an `external:` PARENT is legal (§3.2.1 `update_issue`). A carve-out honoured for some edge types and refused for others would recreate exactly the two-dialect split this clause abolishes.

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
pub enum DiagnosticKind { Stats, Info, Where, Version, Lint, Changelog, Orphans, Dangling } // mirrors §5.2 DiagnosticsInput kinds
// D45 — the `///` doc comment on the `Dangling` variant is CONTRACT BYTES here too (schemars lifts it
// into the variant `description` that rides `schema_bundle()`); it is pinned byte-for-byte by the contract
// snapshot, and re-wording it re-cuts `CONTRACT_HASH` — see the matching note on §5.2's `DiagnosticsInput`.
// D45 — `Dangling` (wire `"dangling"`, the plain-noun form the seven siblings use) is APPENDED,
// never inserted mid-list: schemars emits the variants in declaration order and `CONTRACT_HASH`
// digests those bytes, so a mid-list insertion would move the digest for a reason unrelated to the
// new kind (the same rule §5.4 pins for `SchemaBundle` field position). §5.2's `DiagnosticsInput`
// gains its arm LAST for the same reason, keeping the two mirrored. The variant is MINTED rather
// than reusing `Lint`: both options bump the contract anyway (§5.4 D45), and reusing `Lint` would
// make `DiagnosticReport.kind` DECLARE a kind the report is not — a lie on a published field.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DiagnosticReport { pub kind: DiagnosticKind, pub findings: Vec<DiagnosticFinding> }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DiagnosticFinding { pub label: String, pub detail: String } // generic key/value finding row
```

`ExportReport.written` counts every serialized issue line in the export corpus, which — per FORK-1/D23 — INCLUDES closed and tombstone rows (`ListFilters { include_closed: true, include_tombstone: true, .. }`) and EXCLUDES ephemeral / `-wisp-` rows — **the exclusion being CONDITIONAL since D45: an ephemeral / `-wisp-` row that stands in a non-external dependency relation with an exported row — in EITHER direction, so notably one that BLOCKS an exported row — stops counting as excluded, transitively (the blocker closure below, which SUPERSEDES D23's unconditional exclusion).**

**Export is CLOSED UNDER ITS BLOCKERS (NORMATIVE — D45; Miguel's ruling, 2026-08-01). The exporter DROPS NOTHING — the CORPUS widens.** The corpus filter drops ROWS; until D45 it did not drop the EDGES POINTING AT THEM, so an edge from a kept issue to an ephemeral or `-wisp-` issue was still serialized on the kept issue's line while its target's line was gone (`crates/unblock-sync/src/export.rs:76-77` — `issues.retain(…)` filters rows only).

**Dropping that edge was examined and REJECTED, on measured evidence.** `live_blocked_ids` pass 1 (`crates/unblock-storage/src/libsql/query.rs:288-294`) is a LEFT JOIN whose predicate is `(i.status NOT IN ('closed','tombstone') OR i.id IS NULL)` with **no ephemeral exclusion**, so an issue whose blocker is ephemeral is BLOCKED today. Pass 2 (`:305-317`) carries no ephemeral exclusion either, so an EPIC with an ephemeral open child is BLOCKED today as well — the shape the rule below has to follow the edge BACKWARDS to preserve. An exporter that dropped either edge would hand the destination workspace a READY issue that is not ready — a data-integrity tool silently converting blocked work into available work, on the one promise the tool exists to keep.

**The rule, stated over ROWS — because blockedness is NOT a function of a row's own edge list (Miguel's ruling, 2026-08-01).** A row the D23 retain excluded **stops counting as excluded** the moment it stands in a non-external dependency relation with a row in the working set, **in EITHER direction** — most importantly, **an ephemeral or `-wisp-` row that BLOCKS something exported is exported**. AFTER the D23 row retain and BEFORE serialization the exporter therefore computes the **TRANSITIVE CLOSURE of the kept set over non-external dependency edges, followed in BOTH directions**, over the rows of the pre-retain `list_issues` result:

- **OUT** — a row in the working set carries a `depends_on_id` for which `unblock_model::is_external_target` is FALSE (§1.9) and which names an excluded row: that row is ADDED.
- **IN** — an excluded row carries a `depends_on_id` (likewise non-external) naming a row in the working set: that excluded row is ADDED.

Every newly added row is then examined the same way, in BOTH directions, until a full pass adds nothing. The final corpus is serialized in `id ASC` order exactly as before.

**The IN direction is FORCED by measurement, not added for symmetry, and an OUT-only closure is NON-CONFORMING.** An edge is stored on exactly ONE row — `dependencies.issue_id` — and hydration is `FROM dependencies WHERE issue_id = ?1` (`crates/unblock-storage/src/libsql/crud.rs:408`), so an edge is serialized ONLY on that row's line. Blockedness, however, flows along an edge in both directions: `live_blocked_ids` **pass 2** (`crates/unblock-storage/src/libsql/query.rs:305-317`) marks the **PARENT** (`d.depends_on_id`) blocked because it is an epic with a non-terminal CHILD, and — exactly like pass 1 — it carries **no ephemeral exclusion**, while the `parent-child` edge lives on the **CHILD's** line. So a KEPT epic with an EXCLUDED open child is BLOCKED in the source workspace; its own dependency list is empty; an OUT-only closure has nothing to follow, the child never travels, and the epic arrives **READY** in the destination — after which **pass 3** (`propagate_blocked_to_children`, `query.rs:341`) propagates that ready-ness down to every kept child of the epic. That is the same silent blocked→ready conversion this whole clause exists to forbid, arriving through the other side of the edge. **The IN direction is also what keeps the "drops nothing" claim literally true of EDGES:** an edge stored on an excluded row's line and pointing INTO the corpus would otherwise vanish with its row, and `export → import → export` would not reach a fixed point.

**Both directions are UNIFORM over every dependency type — no gating carve-out.** Restricting the IN direction to the `affects_ready_work` types would reintroduce exactly the per-edge-type special-casing §1.9 and §3.2.1 refuse, and it would still lose a non-gating edge stored on the excluded row's line. The closure is over ids that could denote rows, not over a privileged subset of edge kinds.

Three properties are NORMATIVE:

1. **Termination is structural, not incidental.** The working set only GROWS and the pre-retain row set is finite, so a pass that adds nothing ends the walk and a dependency CYCLE through excluded rows terminates by construction. **The specified shape is a WORKLIST over the still-excluded rows, re-scanned whenever the working set grows** — an OUT-only queue drained once per newly-added id is NOT sufficient any more, because under the IN direction a row becomes eligible when some OTHER row is pulled in, not when its own edges are visited. A naive recursive re-walk of the whole set is likewise not the specified shape. **COST, stated rather than discovered later — the same discipline the `Session::doctor()` row (§4.1) applies to the sibling fold.** The specified shape is a re-scan, so its cost is **O(passes · (rows + edges))**, and a pass is only guaranteed to add ONE row: `passes` is bounded by the excluded-row count, which makes the WORST CASE **quadratic in the excluded rows** — it is NOT `O(rows + edges)`. The two maps it builds first (`index_of`, `targeted_by`) are `O(rows + edges)` and are built once; the quadratic term is the re-scan alone. **Measured** (`crates/unblock-sync/src/export.rs`, through the real `export_jsonl` over the in-crate `FakeStorage`, release profile, whole-export wall time): the ADVERSARIAL corpus costs **327ms at 4k, 1.21s at 8k, 5.06s at 16k, 24.6s at 32k** excluded rows, against a linear CONTROL of **59 / 75 / 123 / 238ms** at the same row and edge counts — ~103× at 32k, and ~4× per doubling, i.e. quadratic as derived. **Reproduced independently on the DEV profile** (a second measurer, same corpus shape, same code path): **3.42s / 13.67s / 55.97s / 244.1s** adversarial against **101 / 193 / 378 / 753ms** control, giving 4.00× / 4.09× / 4.36× per doubling — the SHAPE is confirmed twice over. **Provenance, stated because a number nobody reproduced is a weaker claim than one two people measured:** the four RELEASE absolutes above are SINGLE-SOURCED — the second measurer could not build release (the machine exhausted its disk mid-run), so the release row is the author's alone while the dev row and the quadratic shape are corroborated. Re-derive the release row before quoting it as a budget. **The adversarial shape, named so it can be reproduced and so nobody mistakes it for a synthetic curiosity:** one kept row plus N ephemeral rows in a single dependency CHAIN whose ids ascend ALONG the chain, so `list_issues`' `id ASC` order is the exact reverse of eligibility order and each pass pulls in exactly the last row it scans. It is constructible entirely through the public API (chained `ephemeral: true` rows), and `sync export` is a command this repository runs on every commit. The CONTROL is the same N ephemeral rows each pointing DIRECTLY at the kept row, which the first pass drains whole. **The rewrite that removes the quadratic term — a both-direction STACK seeded from the kept set over the two maps already built — is DELIBERATELY NOT in the D45 commit, and this paragraph is not a licence to make it:** the worklist shape is prescribed by this very property, so replacing it is a SPECIFICATION CHANGE that amends this clause FIRST and is Miguel's call under the no-simplify rule. Until that amendment lands, the shape above is normative and the cost above is its disclosed price.
2. **A pulled-in row is serialized VERBATIM — its `ephemeral` flag included.** The closure changes WHICH rows are written, never WHAT a row says. Rewriting `ephemeral: true` to `false` on the way out would make the destination workspace disagree with the source about a stored field, and the next export from the destination would then legitimately keep the row for a different reason.
3. **An `external:` id pulls NOTHING and is pulled by NOTHING**, in either direction, because it is not a row at all (§1.9) — there has never been an issue row for it to serialize, and no row can be reached through it. The closure is over ids that could denote rows.

**This is what reconciles the D45 write guard with D5 portability, and it is the reason the guard may live in the shared per-record insert body at all.** The resulting property is the one an import needs, stated exactly: **every file `sync export` produces is importable into an EMPTY workspace under the D45 guard, PROVIDED the source workspace holds no dangling edge**, because every surviving edge points either at a row inside the SAME file or at an external target — and the guard's batch arm (§3.2.1) accepts an intra-file target regardless of line order, so the property holds without any ordering requirement on the export. A SECOND property rides with it and is the one the measured evidence forces: **an issue BLOCKED before the export is BLOCKED after importing that export.** **That property is TRUE AS STATED only because the closure runs in BOTH directions, and the derivation is short enough to write down:** blockedness has exactly three sources (`live_blocked_ids`), and each is now preserved. Pass 1 reads the blocked row's OWN edge, which is serialized on its line, and the OUT direction guarantees the target row travels with it. Pass 2 reads an edge stored on the CHILD's line to mark the PARENT blocked, and the IN direction guarantees an excluded child of a kept epic travels. Pass 3 propagates only from rows already blocked by passes 1–2 along `parent-child` edges stored on the child's line, so it reproduces once those two do. A closure that followed OUT-edges alone would satisfy the first source and silently break the other two. Without the closure, the guard would refuse files this repository's own exporter produces — the exact hazard §3.2.1's D44 scoping clause named ("moving the guard into the shared body could make an already-exported D5 record un-importable"). D45 does not wave that hazard away; it removes its cause.

**DISCLOSED CONSEQUENCE — the exporter does not repair, so it CAN emit a file the guarded import refuses.** A workspace that ALREADY carries a dangling edge (an edge whose target names no row anywhere) exports that edge unchanged; the closure has nothing to pull for it in EITHER direction, since a dangling target is not a row. The import then REFUSES the whole file with `BlockerNotFound` → `ISSUE_NOT_FOUND`, naming the FIRST offending `(dependent, target)` pair — both ids are already carried by the variant (§3.1), so the message is sufficient to repair the source file by hand. **That refusal is the correct behaviour and is stated as a rule, not as an accident: the EXPORTER may WIDEN its corpus (it is closing a file it owns, under its own corpus rule); the IMPORTER may never INVENT one (it is ingesting a claim it cannot verify).** An exporter permitted to silently repair would put an edge-dropper on the write side while D45 refuses one on the ingest side, and it would launder precisely the class the `dangling` diagnostic (§3.2.1) exists to surface. The named remedy is that diagnostic: enumerate the offending edges, fix them, then export.

**SECOND DISCLOSED CONSEQUENCE — an export file may now carry ephemeral / `-wisp-` LINES.** That is an observable change to `sync export` bytes and it is why D45 **SUPERSEDES** D23 sub-decision (1)'s unconditional `exclude ephemeral/-wisp-` rather than refining it (PRD §4, D45 clause (5) and the reciprocal note on the D23 row). `ExportReport.written` is unchanged in MEANING — it still counts serialized issue LINES — but its value can now be larger for the same retained set. **The IN direction widens it FURTHER than out-edges alone would:** an excluded row is now written not only when something exported depends on it, but also when IT depends on something exported — the epic's ephemeral child being the shape that forces it.

**An `external:` target is NOT a dangling edge, and it is ABSENT FROM EVERY EXPORT BY CONSTRUCTION** — not because it is filtered out, but because there has never been an issue ROW for it to serialize: it names a ticket in another system. So the closure property above is "every surviving edge points inside the file **or** is external", never "every surviving edge has a line in the file", and the D45 diagnostic (§3.2.1 `dangling`) never reports one.

**NO new interchange LOSS (a claim the earlier draft got backwards, corrected here rather than left standing).** Because the target now travels with the edge, `export → import → export` PRESERVES an edge whose target is ephemeral or `-wisp-`; D45 adds nothing to the two already-disclosed losses (a dependency `metadata` of literal `"{}"` reading back as `None`; the `bd` import dropping both comment fields, §3.2.1). It does NOT touch the D42 dependency fixed point (§3.2.1: `None → NULL → None` round-trips exactly) — that clause is about a KEPT edge's columns and is unchanged; nor the `include_tombstone` round-trip clause above, which is about ROWS (and a tombstone TARGET counts as existing under the D45 guard, so it was never at risk). What the PRD carve-out records instead is the REFUSAL consequence above: an export of an already-dangling workspace produces a file the guarded import rejects.

**Observability, contract-neutral.** `ExportReport` does NOT gain a field — it is a §1.10 contract DTO surfaced through `SyncOutput` (§5.3), and any field change moves schema bytes (§5.4). The closure is instead reported once per export on the existing NFR-13/D30 `unblock.reliability` target from `unblock-sync`, with the standardized field set (`operation = "export"`, `path`, `result = "blocker-closure-widened"`, `reason = "<n> row(s) outside the corpus filter retained as dependency targets"`). **FIRE CONDITION, PINNED:** the emission is CONDITIONAL on `n > 0`, exactly like the sibling `external-path-force-override` emission in the same file. An unconditional emission would write a `0 row(s)` INFO on every export, and this repository re-exports `.unblock/issues.jsonl` on every commit — the literal reading of "reported once per export" is therefore not the specified one, and this sentence exists so nobody implements it.

**NFR-16 consequence — the round-trip obligation, with a HOME (an obligation whose only named suite cannot host it is prose).** The property is: **for any corpus the tool itself produced, `export → import into an EMPTY workspace` SUCCEEDS under the D45 guard, the KEPT edge set is preserved, and any issue that was BLOCKED before the export is BLOCKED after it.** It is asserted in **`crates/unblock-sync/tests/contract.rs`** — the crate-level integration suite that already drives the REAL `unblock-storage` libsql impl end to end — and it needs **TWO** blockedness cells, not one, because one cell per closure DIRECTION is what makes the property non-vacuous: (a) a kept issue blocked by an EXCLUDED blocker (the OUT direction, a `live_blocked_ids` pass-1 shape), and (b) **a kept EPIC with an excluded, non-terminal `parent-child` CHILD** (the IN direction, a pass-2 shape) — cell (b) passes in BOTH worlds unless the closure follows incoming edges, so it is the one that kills the OUT-only mutant. Each asserts the dependent is still in `blocked`, never `ready`, after the round trip. The NFR-16 storage contract suite carries the guard-side half (a `create_issues` batch whose record declares a target present neither in the batch nor in storage is refused whole-batch, ZERO rows). **`crates/unblock-sync/tests/roundtrip_proptest.rs` CANNOT host it, and "constrain its generator" is a NO-OP:** that suite's `parse_of_serialize_is_sync_equals` (`:115`) is a pure `serialize_issue_line` → `parse_issue_line` → `sync_equals` identity that never touches `Storage`, never exports and never imports, so its `arb_dependency` (`:28`) already emits targets that are essentially always dangling and nothing goes red — no guard runs there. That generator therefore stays as it is; the strengthened property lives where storage, export and import are all real.

All emitted `DateTime<Utc>` fields are rendered via `unblock_model::fmt_ts_secs` (CF-TS) so export bytes are deterministic and byte-coherent with render (D-OQ-B).

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

**`RenderError` follows this pattern too (D27/AF-4, T3.1 — additive).** `unblock-render` already computes an inherent `code()`; T3.1 adds `impl unblock_error::CodedError for RenderError { fn code(&self) -> ErrorCode { self.code() } }` (delegating to the inherent map — one error → one code, §6.5 unchanged) so the uniform `(&err).into()` L7 bridge covers it like every other per-crate enum. The 4th render variant `RenderError::UnknownFormat { name: String }` (added at T3.1 — `parse_format`'s unknown-name arm) maps to `ErrorCode::ValidationFailed`, the same family as `UnsupportedFormat`/`FieldUnknown`; the §2.3 exit table is UNCHANGED (no new ErrorCode). **`McpServerError` is the deliberate exception:** it does NOT impl `CodedError` — the cli `exit.rs` maps it EXPLICITLY (`Transport`/`RunLoop` → `ErrorCode::InternalError`, exit 1; an MCP-server run-loop/transport failure is an INTERNAL condition, not a user IoError) via `StructuredError::from_code(InternalError, err.to_string())` (which already routes through `sanitize_message`) — **absent a recorded signal AND absent a pre-`initialize` disconnect**. When the FR-17 handle recorded a signal, the signal exit `128+signo` takes precedence over this map (D38) and the `McpServerError` is rendered as a **diagnostic only**; and `resolve_mcp_exit` intercepts the unsignalled `Transport{ConnectionClosed(_)}` pre-handshake disconnect (`is_pre_handshake_disconnect()`) BEFORE this `InternalError` cast, delegating the exit code to the teardown → exit 0 on a clean shutdown (D40). See §5b (`mcp`).

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
    RateLimited, // RateLimited new for NFR-18 rate-limit, T3.5/D34
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
    //   | InvalidStatus | InvalidType | InvalidPriority | RequiredField | AmbiguousId | RateLimited.
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
    //     None           -> every other code (the remaining 31 of 36)
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
| **2** | Database | `DatabaseNotFound`, `DatabaseLocked`, `SchemaMismatch`, `DatabaseError`, `NotInitialized`, `AlreadyInitialized`, `RateLimited` |
| **3** | Issue / operational | `IssueNotFound`, `AmbiguousId`, `IdCollision`, `InvalidId`, `NothingToDo`, `AlreadyClaimed` |
| **4** | Validation / policy | `ValidationFailed`, `InvalidStatus`, `InvalidType`, `InvalidPriority`, `RequiredField`, `PolicyViolation` |
| **5** | Dependency | `CycleDetected`, `DependencyNotFound`, `HasDependents`, `SelfDependency`, `DuplicateDependency` |
| **6** | Sync / JSONL | `JsonlParseError`, `PrefixMismatch`, `ImportCollision`, `SyncConflict`, `ConflictMarkers`, `PathTraversal` |
| **7** | Config | `ConfigError`, `ConfigNotFound`, `ConfigParseError` |
| **8** | I/O | `IoError`, `JsonError` |

*(exit 2 also carries the retryable transient-busy `RateLimited` — an MCP-surface concurrency cap, not a DB fault; grouped with `DatabaseLocked` by retry semantics, D34.)*

*(**Signal exit is a separate axis.** `128+signo` (FR-17 cooperative shutdown / second-signal escalation) is NOT an `ErrorCode` and is NOT in this table; it is produced only by the `mcp` command's signal path and TAKES PRECEDENCE over the 0–8 cast on the two cooperative-shutdown returns (`run_mcp_server` — `Ok` **and** a post-cancel `Err` — and `session.shutdown()`) whenever a signal was recorded; a failure raised BEFORE the run loop starts (e.g. `Session::open`) is not a consequence of the cancellation and still casts through this table — D38, `01-design-spine.md` §5b. The table itself is unchanged and stays semver-stable from GA (D35).)*

*(**Unsignalled pre-`initialize` disconnect → exit 0 (D40 — additive, no new code).** With NO signal recorded, an `Err(McpServerError::Transport{ConnectionClosed(_)})` (the peer closed the connection before completing the `initialize` handshake) exits via the **Success** row (exit 0), NOT the exit-1 `InternalError` cast: `resolve_mcp_exit` intercepts it (`is_pre_handshake_disconnect()`) and delegates to the `session.shutdown()` teardown — clean → exit 0, a failing teardown still decides via its own 0–8 code. This adds NO `ErrorCode` and touches NO row in this table; it is a routing carve-out in the `mcp` command, spec'd in §5b.)*

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

The L7 boundary converts any composed crate error → `StructuredError` (CLI: serialize to stdout + `process::exit(exit_code)`; MCP: attach as error data, §5.6). Output is **always valid JSON even on error** (FR-11). `tracing` on `unblock.reliability` records the L7-boundary error, while the reliability GUARD emissions (external-path use / force-override — ONE event — and conflict-marker rejection) are emitted in `unblock-sync` at L3 with the standardized `operation`/`path`/`result`/`reason` field set (NFR-13, D30). The tracing target name is the single const `RELIABILITY_TARGET = "unblock.reliability"`, hoisted to the L0 crate `unblock-error` (D30): `unblock-sync` (L3), `unblock-engine` (L5) and `unblock-cli` (L7) all reference the ONE const (engine re-exports it, sync imports it), so `init_tracing`'s `EnvFilter` directive and every guard emit-target can never diverge (a by-value duplicate would risk the filter-target ≠ emit-target silent-drop hazard). `unblock-engine/src/logging.rs` owns only the idempotent subscriber init; structured output strictly stdout, diagnostics stderr (NFR-14).

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

**`WriteLockGuard` (storage-owned; D31 — the cross-process advisory write-lock RAII guard, NORMATIVE).** A public opaque RAII guard returned by `Storage::acquire_write_lock` (§3.2). It owns the locked advisory `std::fs::File` handle on `.unblock/.write.lock` (`Send`, `'static`) and releases the flock on `Drop` (with an explicit `unlock()` backstop). It carries **no** public fields and names **no** libsql/backend type (spine §6 rule 2). `Storage::acquire_write_lock` returns `Ok(None)` on the file-less in-memory path (no lock is needed there — connection-private shared cache, no cross-process sharing). **Re-entrancy (MF4 — ported beads `write_lock_already_held`):** an in-memory held-marker on the L2 lock primitive makes a NESTED acquire by the current holder return a **re-entrant no-op guard** (owns nothing, releases nothing on drop) instead of opening a fresh fd that would self-contend on the process's own advisory lock (per-open-fd) and surface a spurious `DatabaseLocked`; the marker is set only after the flock is truly held and cleared when the real guard drops (the L5 `Semaphore` serializes the check-then-set to one in-process writer; nesting is stack-disciplined). The engine holds the guard across the whole mutation (allocation READ + write tx), then drops it inner-first, before the write permit.

**`close_reason` persistence (T1.2 Verify-gate, NORMATIVE).** `close_reason` is the nullable-text tri-state (`None` = leave unchanged; `Some(None)` = clear to the column default `''`; `Some(Some(s))` = set). `update_issue` persists it to the existing `close_reason TEXT DEFAULT ''` column (already projected by `ISSUE_COLUMNS`; `create_issue` already binds it from the `Issue`). The engine's `close_with_suggestions(id, reason)` (§4.1) builds a `status = Closed` patch carrying `close_reason` and persists it through `update_issue` under the write permit — the reason is **stored**, not tracing-only. The `close_reason` column is **not** part of the frozen `content_hash` (spine §1.8), so persisting it does not perturb import idempotency (FR-26).

**`StorageError` (storage-owned; the §2.1 sketch made concrete — NORMATIVE).** The full v1 variant set and its `ErrorCode` mapping. **`CommentNotFound { id: i64 }` (FR-6/D37) is a StorageError-level variant that maps ONTO the EXISTING `ErrorCode::IssueNotFound` — the two levels are deliberately NOT 1:1 here, and this is not a bug to "fix" back.** FORK-E1 constrains the **`ErrorCode` taxonomy** (no `CommentNotFound` *code*: the taxonomy stays at 36, no exit-code-table re-bless, no `oneOf`/error-golden movement) — it does not constrain the internal `StorageError` enum, and adding this variant satisfies FORK-E1 literally because it grows no `ErrorCode`. The variant exists because reuse at the StorageError level would force `IssueNotFound { id: comment_id.to_string() }`, whose `context()` key is `"id"` and whose `Display` renders `issue 42 not found` when it was **comment** 42 that was missing — actively misleading in an agent-first tracker. The `i64` field matches the comment row's own id type (`Comment.id`, §1.6); the field-bearing analog is `IssueNotFound`, **not** the fieldless `DependencyNotFound`. It implements `unblock_error::CodedError` (NOT a bespoke inherent `code()`; §2.1 note), so the L7 boundary bridges it via the blanket `From<&E>` like every other crate enum. `Migration` is defined **concretely and minimally, model-backed**: `Migration { from: i32, to: i32, reason: String }` (`from`/`to` are `PRAGMA user_version` values, `i32` to match the schema-version type). `Backend { source: BackendOpaque }` absorbs the libsql error opaquely — no libsql type is ever public (spine §6 rule 2). `BackendOpaque` sanitizes its message **at construction** via `unblock_error::sanitize_message` and exposes only `Debug`/`Display`. **`BlockerNotFound { issue_id: String, depends_on_id: String }` (D45) is the second variant of the same shape, and for the same reason.** It maps onto the EXISTING `ErrorCode::IssueNotFound`; FORK-E1 is satisfied literally, because the `ErrorCode` taxonomy does not grow (it stays at 36 — no exit-code-table re-bless, no `capabilities().error_codes` movement, no `CONTRACT_HASH` movement **from this variant**). It carries BOTH ids because both are load-bearing on a batch path: on an import of 500 records, `depends_on_id` alone would name the phantom without naming which record declared it. `Display` renders **`issue {issue_id} declares a dependency target that does not exist: {depends_on_id}`** — deliberately NEUTRAL about the edge KIND, because the guard runs over the DISTINCT target set of every declared dependency (`DependencyType` has 11 named variants plus `Custom`, of which only 4 gate ready work) and over `apply_reparent`, whose target is a PARENT. Rendering "blocker" there would be misleading in exactly the way this same paragraph rejects two sentences above when justifying `CommentNotFound`. The VARIANT keeps the name `BlockerNotFound` (it is internal, and the never-ready blocker case is the class's motivation); the user-visible STRING does not claim it. `context()` surfaces `context["issue_id"]` and `context["blocker_id"]` — the key is `blocker_id`, NOT `id`, so the payload stays honest about WHICH entity was missing (the same discipline `CommentNotFound` uses with `context["comment_id"]`). Adding context KEYS moves no schema byte: `StructuredError.context` is a free-form `serde_json::Map` (§2.4), the same mechanism D43 used for its `context.kind` discriminator (§5.4).

```rust
#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum StorageError {
    IssueNotFound { id: String },        // -> IssueNotFound
    CommentNotFound { id: i64 },         // -> IssueNotFound (FR-6/D37; reuses the code, mints no new one)
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
    BlockerNotFound { issue_id: String, depends_on_id: String }, // -> IssueNotFound (D45; reuses the code, mints none)
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
//   SchemaMismatch{found,expected}; IssueNotFound{id}; CommentNotFound{id} -> context["comment_id"]
//   (code() = IssueNotFound, but the context key stays honest about WHICH entity was missing);
//   BlockerNotFound{issue_id,depends_on_id} -> context["issue_id"] + context["blocker_id"] (D45 —
//   same discipline: code() = IssueNotFound, the keys name the declaring row AND the phantom target);
//   HasDependents{id}; IntegrityFailed{messages}; ... }
//
// pub struct BackendOpaque(String); // private inner; from_message() runs sanitize_message at construction;
//   Debug + manual Display (sanitized text) + impl std::error::Error. No From<libsql::Error> until T0.6.
```

**Why `BlockerNotFound` rides `IssueNotFound` and not the other two (NORMATIVE rationale — D45; the choice is a CONTRACT, because it decides the exit code a CLI caller observes).** No new `ErrorCode` is minted: post-GA the spine already calls that a BREAKING contract change shipped in a patch release (§5.4, D43's stated reason for refusing), and both live v1.0.1 precedents refused. Three existing codes were genuinely in play:

- **`DependencyNotFound` (exit 5).** Its published doc already says "the dependency target was not found", which reads like an exact fit — and it is the trap. **The name is already taken by a different meaning:** it is what `remove_dependency` returns when the DELETE matches zero rows (§3.2.1 `list_dependencies`/`remove_dependency`; `crates/unblock-storage/src/libsql/deps.rs`), i.e. "the EDGE does not exist". Reusing it would make one code mean both "the edge you asked me to delete is not there" and "the issue you named as a blocker does not exist", leaving an agent no way to tell them apart — a machine-filterability regression on a code that is currently unambiguous. REJECTED.
- **`ValidationFailed` (exit 4).** What D44's own wire rejection uses and what `create_bulk`'s existing unresolved-reference rejection already produces. REJECTED on two grounds: it is published as **retryable** (§2.2 retryable set), which is a lie here — retrying the identical call cannot succeed, nothing is transient; and its published `HintShape` is `ContextualText`, so it cannot carry the one hint that actually helps.
- **`IssueNotFound` (exit 3). CHOSEN.** (i) It is the FORK-E1 precedent applied verbatim — the sanctioned shape for a not-found sibling is a new `StorageError` variant onto this code, and `CommentNotFound` already rides it. (ii) It is the ONLY code whose published `HintShape` is `SimilarIds` (§2.2), i.e. the did-you-mean family backed by the real `find_similar_ids` site — and a typo'd or hallucinated blocker id is this defect's dominant cause, so the one code with a near-miss suggestion shape is the one that fits. (iii) It is non-retryable, which is the honest signal. (iv) Exit 3 (Issue/operational) is a defensible bucket: the fault is that an ISSUE was not found, which is exactly what exit 3 means. **The trade, stated plainly:** a caller filtering on the CODE alone cannot distinguish "the issue you addressed does not exist" from "the blocker you named does not exist" — that distinction lives in `context["blocker_id"]`, whose presence is the discriminator. That is the same cost `CommentNotFound` already pays, accepted for the same reason: a taxonomy break in a patch release costs more.

**Hint (NORMATIVE, so no shape moves either way).** Attaching a hint on this path is OPTIONAL in this cut; IF one is attached it MUST be the `SimilarIds` family already published for `IssueNotFound` (a `find_similar_ids` fold over the blocker id, `context["similar_ids"]`). No `hint_shape` byte moves in either case, so §2.2's honesty rule — a code may move off `HintShape::None` only when a real production hint site ships — is not engaged.

**The one divergence, stated rather than hidden.** `Session::create_bulk` rejects an UNRESOLVABLE dependency REFERENCE at L5, before storage is reached, and keeps `ValidationFailed` (§5.2 rejection-set item (b)). That is a different question from D45's: a bulk `### Dependencies` entry may be a title, a stand-in handle or an id, and "this reference resolves to no target at all" is a parse/resolution fault. `BlockerNotFound`/`ISSUE_NOT_FOUND` fires for a RESOLVED id that names no row. The two coexist deliberately; neither is a fallback for the other.

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

    // --- cross-process write serialization (D31 — the restored advisory write lock) ---
    async fn acquire_write_lock(&self) -> Result<Option<WriteLockGuard>, StorageError>;
    //  D31 (D14 amendment): acquire the cross-process advisory `.unblock/.write.lock` EXCLUSIVE for the
    //  WHOLE mutation. The engine (L5) holds the returned guard across the `next_child_number` allocation
    //  READ AND the write tx — the SAME span as the L5 Semaphore(1) permit (a legal L5→L2 down-call, NOT a
    //  back-edge; the lock path is derived from the db-file parent, no unblock-config dep). Composes BELOW
    //  the permit and ABOVE the write-conn Mutex + `BEGIN IMMEDIATE`. Bounded + non-spinning (NFR-3): one
    //  native `try_lock` fast-path, else an async `tokio::time::sleep(25ms)` poll to the store's
    //  `write_lock_timeout_ms` (threaded down at open; default 30000); a timeout → retryable
    //  StorageError::DatabaseLocked → ErrorCode::DatabaseLocked (NO new ErrorCode). `Ok(None)` on the
    //  file-less in-memory path (no lock needed). RAII: `WriteLockGuard` owns the locked `std::fs::File`
    //  (Send, 'static) and releases the flock on Drop (explicit `unlock()`). Reads take NO lock (WAL MVCC).
    //  A distinct file from the vestigial `.unblock.lock` OrphanedLockFile detector target, which stays
    //  never-written (unblock-health UNCHANGED). `migrate` acquires the SAME exclusive lock internally with
    //  timeout=0 (fail-fast) for the WHOLE command, UNCONDITIONALLY — acquired before the version check, and it
    //  bypasses `with_immediate_tx` (§3.3).

    // --- issue CRUD (mutations carry actor + optional Tier-1 attribution; write Event(s) transactionally) ---
    async fn create_issue(&self, issue: &Issue, actor: &str) -> Result<String, StorageError>; // returns id
    //  ONE `BEGIN IMMEDIATE` tx: the row + `Event(Created)` + the child-counter bump + the SEEDED
    //  relations — labels, `Issue.dependencies` and comments — with their per-relation events. The
    //  dependency edges are RE-ANCHORED: the INSERT binds `issue.id` as the source column and IGNORES
    //  `Dependency.issue_id`, so a seeded edge can never land on another issue. Carries the D44
    //  create-specific duplicate + gating-cycle guards, which `create_issues` deliberately does NOT
    //  (§3.2.1). SIGNATURE UNCHANGED by D44 — the edges ride `Issue.dependencies`, so no `impl
    //  Storage` block gains, loses or re-types a method and every implementor's METHOD SET stands.
    //  What DOES move is the shipped libsql BODY of this method (`crud.rs:31-52`): it is where the
    //  create-specific guards land. A doc that says "no impl moves" without that distinction is the
    //  same omission the pre-D44 doc-comment made.
    async fn create_issues(&self, issues: &[Issue], actor: &str) -> Result<(), StorageError>;  // D22/T2.3 — ATOMIC bulk insert
    //  Inserts the WHOLE slice in ONE `BEGIN IMMEDIATE` tx: every row + its `Event(Created)` + per-relation
    //  events + the seeded dependency edges + child-counter bumps, committed ONCE. ANY failure on ANY record
    //  (id/`external_ref` collision, FK/CHECK violation, backend error) ROLLS BACK the entire tx — ZERO rows
    //  persisted (no partial batch). The engine `Session::create_bulk` (§4.1) mints all ids + resolves intra-batch
    //  deps under the write permit BEFORE calling this, so storage receives fully-formed `Issue`s with resolved
    //  ids/edges. `create_issues` is UNCHANGED by D44 (it is also the JSONL/`bd` import body); the earlier
    //  "`create_issue` (single) and `create(&Issue)` (import) are UNCHANGED" clause is SUPERSEDED by D44,
    //  which gives the SINGLE create the same all-or-nothing property plus the create-specific guards. §3.2.1.
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

    // --- comments (FR-6, D37; analog of add_dependency/list_dependencies; each mutation writes its Event in the same tx) ---
    async fn add_comment(&self, issue_id: &str, author: &str, body: &str, actor: &str)
        -> Result<Comment, StorageError>;                                     // guard issue EXISTS (else IssueNotFound); Event(Commented); updated_at stays NULL
    async fn list_comments(&self, issue_id: &str) -> Result<Vec<Comment>, StorageError>; // ORDER BY created_at ASC, id ASC
    async fn update_comment(&self, comment_id: i64, body: &str, actor: &str)
        -> Result<Comment, StorageError>;                                     // provenance-preserving (D-D): set updated_at=now; Event(CommentEdited)
    async fn delete_comment(&self, comment_id: i64, actor: &str)
        -> Result<Comment, StorageError>;                                     // soft-redact (D-E): KEEP row, mask body="", set redacted_at=now; Event(CommentRedacted); idempotent if already redacted
    //  EXISTENCE guard (FORK-3): add_comment requires the target issue to exist (reject non-existent/tombstoned →
    //  StorageError::IssueNotFound → ErrorCode::IssueNotFound, FORK-E1 — NO new IssueNotFound-sibling ErrorCode);
    //  a CLOSED issue is allowed (post-mortem).
    //  update/delete guard the comment ROW exists → else StorageError::CommentNotFound { id: comment_id } (§3.1),
    //  which maps to the SAME ErrorCode::IssueNotFound at L7 — the StorageError level names the missing entity
    //  honestly; the ErrorCode taxonomy does not grow (FORK-E1). author threaded for the in-tx Event
    //  (bd parity — the import/seed path carries comment.author; the engine passes author = self.actor, FORK-M1b).

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
    //   reads already declared here + list/ready/blocked/count/dependency_tree. D45's `dangling` kind
    //   follows the SAME pattern and likewise adds NO trait method (it differences `dependency_graph(&[])`
    //   against a fully-inclusive `list_issues` id set — §3.2.1), so no `impl Storage` and no test fake moves. `closed_since` is already
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
  `Event(DependencyAdded)`/`Event(Commented)` are recorded for any seeded relations. **Seeded
  `Issue.dependencies` are persisted in THAT SAME tx, RE-ANCHORED (NORMATIVE — D42 wrote the 7 columns;
  D44 makes the anchoring a contract).** The edge INSERT binds `issue.id` as the `issue_id` (source)
  column and reads ONLY `depends_on_id`/`dep_type`/`created_at`/`created_by`/`metadata`/`thread_id` from
  the element — a `Dependency.issue_id` carried on the object is IGNORED, never written, and can never
  reach another issue's graph. This is not incidental: it is the property that makes the single create
  path safe, and it MUST be pinned by the NFR-16 contract suite (a `create_issue` whose
  `dependencies[0].issue_id` deliberately names ANOTHER existing issue lands the row on `issue.id` and
  leaves that issue's edge set byte-unchanged).
  **Create-specific guard set (NORMATIVE — D44).** Both guards live inside THIS method's own tx — i.e.
  in the `create_issue` wrapper, NOT in the shared per-record body. **Their ordering requirements
  DIFFER and are specified separately; they are not one phase.**
  **(a) Duplicate edge — an IN-MEMORY scan over the declared list; it reads NO transaction state.** A
  `depends_on_id` repeated within the ONE `Issue.dependencies` list being created is rejected with
  `DuplicateDependency` — NOT silently skipped. The key is the SEMANTICS `add_dependency` uses (the
  (source, target) pair, type-INSENSITIVE: one target may appear at most once whatever its `dep_type`),
  but explicitly **NOT its SQL**. `add_dependency`'s key is a transaction QUERY
  (`crates/unblock-storage/src/libsql/deps.rs:61-70`), and transliterating that query onto this path is
  NON-CONFORMING and unimplementable in BOTH orderings: run before staging it sees nothing; run after
  staging it matches the row the same tx just staged, so it would fire on EVERY create — and it can
  never see the real offender either way, because the shared body SKIPS the duplicate on the way in
  (`crud.rs:145-147` `continue`s on a repeated target), so the transaction never holds evidence of the
  second copy. The in-memory list is the ONLY place that evidence exists. Its PLACEMENT in the sequence
  is fixed by the published precedence below (it runs after the staging step, so an `IdCollision`, an
  `external_ref` collision or a `SelfDependency` still wins) — but placement is all staging gives it:
  the scan itself queries no tx, which is precisely why post-staging placement is sound for (a) and
  fatal for a transliterated SQL key.
  **(b) Gating cycle — evaluated AFTER the row and all its edges are staged in that tx.** Each seeded
  GATING edge (the 4 `affects_ready_work` types) is checked with the SAME `would_cycle_in_tx`
  `add_dependency` uses, so the D4 orientation and the REAL ordered cycle path land once. THIS guard is
  the sole reason the post-staging ordering exists, and the next sentence is that reason.
  **The check is specified as a PROPERTY, not a call site: every gating edge is checked
  against a tx-visible graph that ALREADY CONTAINS the create's OTHER edges.** A per-element pre-check
  run before anything is staged is NON-CONFORMING and unsound — a create carrying a `parent-child` dep
  (an IN-edge `P -> N` under the D4 reversal) plus a gating dep (an OUT-edge `N -> X`) closes a cycle
  whenever `X -> … -> P` already exists, and neither element sees the other. Order in `deps[]` is
  therefore irrelevant. `SelfDependency` is UNCHANGED — the shared body already compares
  `dep.depends_on_id == issue.id`, which is the correct comparison, and it fires during the staging step,
  so the published precedence is `IdCollision` → `external_ref` collision → `SelfDependency` →
  `DuplicateDependency` → `CycleDetected`, matching `add_dependency`'s self → duplicate → cycle with the
  id guards still first. **AMENDED by D45 (v1.0.1): the chain gains ONE link — `BlockerNotFound` is
  INSERTED between `SelfDependency` and `DuplicateDependency`** (`IdCollision` → `external_ref` collision →
  `SelfDependency` → **`BlockerNotFound`** → `DuplicateDependency` → `CycleDetected`); every pair D44
  published keeps its order (`SelfDependency` still beats `DuplicateDependency`), no shipped rejection is
  re-ranked, and the rationale — including why the rank is FORCED by where D45 bodies the guard, and why
  the alternative placement was rejected — is in the **Dependency-TARGET existence** bullet below. NO new `StorageError` variant and NO new `ErrorCode` are minted **by D44** (D45 mints
  the internal `StorageError::BlockerNotFound` and still mints no `ErrorCode` — §3.1), and the
  rejection does not name the offending array index: with all-or-nothing there is no prefix to describe,
  and element-level detail, if ever wanted, is an L7 `hint` concern. ANY of these rejections rolls the
  whole tx back → ZERO rows: no issue, no edges, no events. **SCOPE — both engine callers, neither
  import leg:** these guards are a property of `Storage::create_issue`, so they apply to the minting
  `Session::create_issue` AND to the id-preserving `Session::create(&Issue)` (§4.1). They do NOT reach
  bulk create or the JSONL/`bd` import, which route through `create_issues` (§4.1 `create_bulk`;
  `unblock-sync` calls `Storage::create_issues` directly). **The shared per-record body is UNCHANGED** —
  its dedup-and-continue and its absence of a cycle check STAY, because `create_issues` is that body's
  other caller: `create_bulk` today commits a genuine mutual cycle atomically, and moving the guard into
  the shared body could make an already-exported D5 record un-importable. Changing bulk/import semantics
  is explicitly OUT of D44's scope.
  **AMENDED by D45 (v1.0.1) — PARTIALLY SUPERSEDED; kept here as the record of what D44 scoped, never
  silently overwritten.** The dedup-and-continue and the absence of a CYCLE check still stay, exactly as
  written. What changes is the blanket "UNCHANGED": the shared body now DOES carry the D45
  dependency-target existence guard (the **Dependency-TARGET existence** bullet below), which is what
  makes that guard total over all five edge-writing entry points. The un-importability hazard this clause
  named is not waved away — D45 removes its CAUSE by widening the export corpus to the transitive closure
  of its blockers (§1.10), so every file the exporter produces satisfies the guard whenever the source
  workspace itself holds no dangling edge. **Storage receives an
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
  over single `create_issue` calls would leave. **The D22 "the single `create_issue`/`create` paths are UNCHANGED" clause is SUPERSEDED by D44**
  (PRD §4): D44 extends the same all-or-nothing property to the SINGLE create by
  seeding the edges onto the built `Issue` instead of writing them in a follow-up pass after the insert.
  `create_issues` itself — and therefore bulk create and the JSONL/`bd` import leg — is UNCHANGED by D44,
  including its dedup-and-continue edge handling and its deliberate absence of a cycle check — see the
  D45 bullet below for the ONE guard that body DID gain afterwards.
- **Dependency-TARGET existence (NORMATIVE — D45; the ONE guard that binds EVERY edge-writing path).**
  A `depends_on_id` that names no issue and is not an external target is REJECTED with
  `StorageError::BlockerNotFound { issue_id, depends_on_id }` → `ErrorCode::IssueNotFound` (exit 3, §3.1).
  The check is bodied in the SHARED per-record insert body (`insert_issue_in_tx`,
  `crates/unblock-storage/src/libsql/crud.rs:157`), which is what makes it TOTAL: that body is the only
  edge-writing site `Storage::create_issue` (the minting create AND the id-preserving
  `Session::create(&Issue)`) and `Storage::create_issues` (bulk create AND the JSONL/`bd` import leg) all
  pass through. The two sibling edge-writing bodies — `add_dependency`
  (`crates/unblock-storage/src/libsql/deps.rs`) and `apply_reparent` (`crud.rs:972-1032`) — carry the SAME
  rule at their own sites (their rows below), which is what closes the count at **FIVE entry points**:
  `issue create {deps}`, `dep add`, `issue update {parent}` (reparent), `issue create_bulk`, and the
  JSONL/`bd` import leg. **This SUPERSEDES the D44 scoping clause that deliberately left the shared body
  alone — the reciprocal note sits on that clause in the `create_issue` row above, which is KEPT as the
  record of D44's scope.**
  - **PREDICATE (batch-aware — a per-record check is NON-CONFORMING).** A target is acceptable iff
    `unblock_model::is_external_target(target)` (§1.9, ASCII-case-insensitive) **OR** a row with that id is
    visible to the CALLER'S transaction **OR** the id belongs to ANY record staged by the same transaction.
    The third arm is not a convenience: `create_issues` stages records sequentially inside ONE transaction,
    so an intra-batch edge resolves only because the sibling was minted EARLIER. A naive per-record
    in-transaction `SELECT` therefore accepts a BACKWARD reference and rejects a FORWARD one (record A
    declaring an edge to record Z later in the same file) — an ordering-dependent refusal of a legal
    import, which no caller can predict and no file author can avoid. Mechanically, the batch arm is the
    id set of the slice handed to `create_issues`, computed BEFORE the transaction opens and passed into
    the shared body — **never the id set of the parsed FILE**, a silently WEAKER reading: the D5 import
    hands `create_issues` the SKIP-FILTERED `create_subset` (`crates/unblock-sync/src/import.rs:279`), which
    is safe ONLY because every Skip reason implies the row already exists and is therefore covered by the
    database arm. That dependency is stated so a future Skip reason for a NON-existent row cannot turn a
    legal file into an order-dependent refusal. `create_issue` passes the singleton set of the one id it is
    inserting; **that arm is provably DEAD on the singleton path** (the only target it could match is the
    issue's own id, which `SelfDependency` rejects first during staging) — specified for uniformity of the
    shared body, and deliberately NOT worth a test.
  - **IN-TRANSACTION is load-bearing.** The check runs inside the caller's already-open `BEGIN IMMEDIATE`
    transaction, never as a pre-transaction probe. A probe (the shape the engine's `probe_storage_dep_refs`
    uses, `crates/unblock-engine/src/session/write.rs`) is racy against a concurrent delete/tombstone: the
    D14 in-process permit and the D31 `.write.lock` narrow that window but do not close it, because the
    SUPPORTED topology is child-per-client (§4.2), i.e. another PROCESS may legitimately be writing.
    `libsql::Transaction` never leaves `unblock-storage` (§6.2), so an in-transaction check is bodied at
    **L2 by construction**; only the POLICY of which paths run it could ever live higher, and D45 answers
    that with "all of them".
  - **A TOMBSTONE TARGET COUNTS AS EXISTING** — deliberately, and deliberately UNLIKE `add_comment`'s
    FORK-3 rule (§3.2.1 `add_comment`, which rejects a tombstoned issue). The export corpus includes
    tombstones (`include_tombstone: true`, D23, §1.10), so an edge to a tombstoned blocker is a normal,
    round-trippable fact; refusing it would make a conforming export un-importable — the precise failure
    D45 also addresses on the export side by widening the corpus (§1.10). The existence query is therefore status-agnostic:
    `SELECT 1 FROM issues WHERE id = ?1 LIMIT 1`.
  - **DISTINCT targets, evaluated as ONE post-staging pass.** The pass walks the DISTINCT `depends_on_id`
    set of the record's declared `Issue.dependencies`, so a repeated target is reported once, and it runs
    AFTER the record's staging loop has finished — which is what makes the published precedence below TRUE
    rather than approximate (a check interleaved into the staging loop would let element 1's missing target
    beat element 2's self-dependency).
  - **An EMPTY or whitespace `depends_on_id` is refused by this guard** (it is not external and names no
    row) — the storage-side half of the empty-string edge hazard. The SOURCE half stays where it is: §5.2's
    PROHIBITED clause forbidding `DepInput.issue_id` from ever being defaulted to `""` is a WIRE rule and
    is NOT superseded by this guard.
  - **RANK IN THE PUBLISHED PRECEDENCE CHAIN (NORMATIVE — precedence is OBSERVABLE BEHAVIOUR, so it is
    PUBLISHED, never appended silently, and the published rank MUST be one the specified placement can
    produce).** The chain becomes, on every path that has the relevant guards:
    `IdCollision` → `external_ref` collision → `SelfDependency` → **`BlockerNotFound`** →
    `DuplicateDependency` → `CycleDetected`.
    The rank is CHOSEN, and the FIRST reason is implementability: (i) it sits AFTER `SelfDependency`
    because a self-edge names the very row being created — reporting it as a missing blocker would be a
    lie, and D44 already pins `SelfDependency` as firing during staging; (ii) it sits BEFORE
    `DuplicateDependency` because **that is the only rank the placement in the shared body can produce.**
    D44's duplicate and cycle guards live in the `create_issue` WRAPPER, AROUND the shared-body call
    (`crates/unblock-storage/src/libsql/crud.rs:57` `insert_issue_in_tx`, then `:61`
    `reject_duplicate_declared_edges`, then `:62` `reject_declared_gating_cycles`), so on the create path
    a missing target NECESSARILY fires before a duplicate. Publishing the opposite pair would publish an
    order the code cannot produce, and an input that both repeats a target and names a missing one is
    trivially constructible, so the discrepancy is observable rather than theoretical. The alternative was
    examined and REJECTED on the merits: moving `reject_duplicate_declared_edges` ahead of the
    `insert_issue_in_tx` call would put `DuplicateDependency` ahead of `IdCollision`, the `external_ref`
    collision AND `SelfDependency` — all three fire inside the shared body — inverting THREE published
    pairs in order to preserve one. **No already-published pair moves under the chosen rank:**
    `SelfDependency` → `DuplicateDependency` is preserved exactly; the new link is INSERTED between them.
    The rank is also defensible on its own terms — existence of an endpoint is prior to any relational
    question about the edge SET, and a duplicate declaration is a statement about that set; (iii) it sits
    BEFORE `CycleDetected` because a cycle is a RELATIONAL question about a graph and presupposes that both
    endpoints denote real nodes — a target that does not exist cannot participate in a cycle, so a cycle
    witness naming a phantom node would report a derived defect while hiding the primary one; (iv) it sits
    after the two id guards because a row must exist before its edges mean anything. The rank is UNIFORM
    across paths, and uniformity is ACHIEVED, not assumed: on `create_issue` it falls out of the shared-body
    placement; on `add_dependency` it is achieved by placing the existence query AFTER the self check and
    BEFORE the duplicate query; on `apply_reparent` there is no duplicate guard, so the chain reads
    self → `BlockerNotFound` → cycle. On the import/bulk leg the chain simply ends at `BlockerNotFound`
    (that path has neither the duplicate nor the cycle guard — `create_issues` above).
    **PER-RECORD SCOPING (stated because an NFR-16 cell written from an absolute reading is record-order
    flaky).** The rank orders the rejections WITHIN one record. `create_issues` runs each record's full body
    before the next begins, so record 1's `BlockerNotFound` legitimately beats record 2's `SelfDependency`
    — already true of `IdCollision`, hence not a regression, but a cell asserting a cross-record winner must
    fix the record order it asserts about.
    **ONE SHIPPED REJECTION CHANGES CODE, and it is named rather than discovered later:** on
    `add_dependency`, re-adding an edge that ALREADY exists and whose target is dangling now returns
    `IssueNotFound` where GA returned `DuplicateDependency`. It is reachable only on already-corrupt data —
    the population D45 exists to surface — and it is listed in the §5.4 ledger.
    **REQUIRED LANDING — the D44 doc-comment that publishes this chain in PROSE.**
    `crates/unblock-storage/src/libsql/crud.rs:34-40` states the old chain and adds "the first three fire
    inside the shared body while it stages, the last two after"; both halves move with D45 (the shared body
    now also fires `BlockerNotFound`, AFTER staging and before the wrapper's two guards). Left unedited it
    becomes a false comment in a green suite, which is why it is a gate landing and not an implementer's
    discretion.
  - **ANY rejection rolls the whole transaction back → ZERO rows** — no issue, no edges, no events, and on
    the batch paths no partial batch. This is the existing `with_immediate_tx` property, not a new one.
  - **No `Storage` trait signature changes.** The batch id set is an internal `pub(super)` parameter of the
    shared body; every `impl Storage` METHOD SET is unchanged, so no test fake moves.
  - **The schema is UNCHANGED** — no foreign key, no `CHECK`, no trigger on `dependencies.depends_on_id`.
    The guard is APPLICATION-LEVEL by necessity: an external target is a legitimate value that no row can
    ever satisfy, so a foreign key would forbid the very thing §1.9 defines as legal.
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
  **Reparent target existence (NORMATIVE — D45; the 4th edge-writing entry point).** `apply_reparent`
  (`crates/unblock-storage/src/libsql/crud.rs:972-1032`) writes a `parent-child` edge to whatever string
  the patch carries, guarded today only by self and cycle. It now carries the same in-transaction
  existence guard, in the same precedence position (self → **`BlockerNotFound`** → cycle — the reparent
  path has no duplicate guard, so the link lands directly before the cycle check), using the `row_exists`
  helper already present at `crud.rs:1328` and already used in-transaction there. **Honest scoping:** a
  dangling `parent-child` edge does NOT produce the never-ready symptom — `blocked_issues` pass 1
  restricts to `blocks`/`conditional-blocks`/`waits-for`, pass 2 is an INNER join that yields no row for a
  missing parent, and pass 3 seeds strictly from the already-blocked set, so a phantom parent is never a
  seed. It is nevertheless a real integrity defect: it is hydrated onto the issue, it is exported, it is
  written with `isError:false`, and the D45 `dangling` diagnostic LISTS it — so leaving it unguarded would
  let the tool that reports the defect be used to create the defects it reports. **`external:` as a
  PARENT is LEGAL:** `is_external_target` (§1.9) applies here too — one shared predicate, no
  per-edge-type special-casing — so an `external:` parent remains representable exactly as it is today.
  Narrowing it (refusing an external parent as nonsense) would be a NEW restriction on a GA-shipped path
  and would fork the predicate into per-edge-type dialects, which §1.9 invariant (6) forbids.
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
  to v1.x; FTS5 to v1.5.
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
  Guards `SelfDependency` then `DuplicateDependency` — **REORDERED by D45 (v1.0.1): the sequence becomes
  `SelfDependency` (UNMOVED — it stays exactly where it ships, pre-transaction; see the SOURCE bullet
  below), then the SOURCE existence check, then `BlockerNotFound` (the same in-transaction
  TARGET-existence rule the shared insert body carries, with the same `is_external_target` carve-out
  (§1.9) and the same status-agnostic query; its "batch" is the single prospective edge), then
  `DuplicateDependency`.** The target check sits BEFORE the duplicate query so ONE published chain
  describes every path (the create path cannot produce the other order — the RANK bullet above), which
  costs two queries swapped inside one transaction and nothing else. Its one observable consequence is
  named in the §5.4 ledger: re-adding an ALREADY-PRESENT edge whose target is dangling now returns
  `ISSUE_NOT_FOUND` instead of `DuplicateDependency`, reachable only on already-corrupt data. Then builds the gating graph **including the
  prospective edge** (private `petgraph`, `would_cycle_in_tx`) over the 4 `affects_ready_work` types.
  If the new edge closes a gating cycle it is rejected with `CycleDetected { path }` where `path` is
  the **actual ordered cycle, naming every node** (e.g. `a -> b -> c -> a`), reconstructed by a private
  `find_cycle_path` DFS over the just-built graph (which already contains the prospective edge) — NOT a
  synthetic `a -> … -> a` placeholder (FR-5 AC). On success: insert + transactional
  `Event(DependencyAdded)`. (The reparent cycle-check, `crud.rs`, routes through the same
  `would_cycle_in_tx`, so the orientation fix below lands once. **Since D44 there is a THIRD caller** —
  the create-specific guard inside `Storage::create_issue`'s own tx, which checks each seeded gating
  edge with the SAME function and therefore inherits the same D4 orientation and the same REAL ordered
  cycle path. It is deliberately NOT in the shared per-record insert body, so `create_issues` — bulk
  create and the JSONL/`bd` import leg — keeps its current absence of a CYCLE check. **NARROWED by D45:
  that body is no longer "guard-free" in general — since D45 it carries the dependency-target existence
  guard (the `Dependency-TARGET existence` bullet in this section). This sentence is about the CYCLE
  guard and stays true of it.**)
  **The edge SOURCE is guarded TOO, in this cut (NORMATIVE — D45; the design-Review open question, CLOSED
  here, not shipped half-open).** The schema puts `FOREIGN KEY (issue_id) REFERENCES issues(id) ON DELETE
  CASCADE` on the SOURCE column under `PRAGMA foreign_keys = ON`, so as shipped a non-existent SOURCE is
  refused as an opaque backend error → `ErrorCode::DatabaseError` (exit 2, and NOT retryable — the
  `is_retryable` set at `crates/unblock-error/src/code.rs:335-348`, enumerated in §2.3, does not contain
  it) while a
  non-existent TARGET now yields `ISSUE_NOT_FOUND` (exit 3, also non-retryable). Leaving that means ONE tool
  call with ONE typo'd id returns two different codes and two different exit codes depending on which
  FIELD carried the typo — and only the source's looks unretryable-by-accident rather than by design, on
  the path an agent uses most. `add_dependency` therefore probes the SOURCE first, inside the same
  transaction, and returns the **EXISTING `StorageError::IssueNotFound { id }`** — no variant is minted,
  because the missing thing genuinely IS the addressed issue, so that variant's `Display` and its
  `context["id"]` are already honest (the very test `CommentNotFound` failed and had to mint for).
  **It ranks IMMEDIATELY AFTER `SelfDependency`, NOT first, and the rank published here is the rank the
  code executes.** The shipped self check returns at `crates/unblock-storage/src/libsql/deps.rs:53-55`,
  BEFORE `with_immediate_tx` opens at `:59`, so a probe specified in-transaction cannot precede it;
  publishing "source first" would publish an order no implementation can produce, and a `dep add` naming
  the SAME non-existent id in both fields would return `SELF_DEPENDENCY` where the text promised
  `ISSUE_NOT_FOUND`. **`SelfDependency` is deliberately NOT relocated into the transaction — stated so
  the next reader does not re-derive that wrong fix:** it needs no transaction to answer, moving it would
  invert a pair D44 already published in order to rescue a published sentence, and it would falsify the
  "two queries swapped inside one transaction and nothing else" cost claim above. The two guards sit
  adjacent because they ask the same kind of question — is this edge well-formed at all: first "are the
  two ids distinct", then "does the source row exist" — and a row must still exist before its edges mean
  anything, which is why the source probe precedes every RELATIONAL question about the edge set
  (`BlockerNotFound`, `DuplicateDependency`, `CycleDetected`). **Mechanically it is
  `row_exists`** (`crates/unblock-storage/src/libsql/crud.rs:1328`), already present and already used
  in-transaction — but declared with NO visibility modifier, hence private to `crud`. **Promoting it to
  `pub(super)` (or inlining an equivalent one-line query in `deps.rs`) is a LANDING of this change, not an
  inference left to the implementer.** The new rejection is recorded in the §5.4 behavioural-change ledger.
- **Dependency edge PERSISTENCE (NORMATIVE — D42; the spine was previously SILENT here, which is
  exactly why a 5-column INSERT never read as a bug).** `add_dependency` and `create_issue` persist the
  **FULL 7-column** `Dependency`: `issue_id`, `depends_on_id`, `dep_type`, `created_at`, `created_by`,
  **`metadata`** and **`thread_id`** (the row above said "6-field" while listing seven). The read projection was always 7-column; the write side bound 5,
  so `metadata`/`thread_id` were accepted, typed, schema-published — and DISCARDED. **`None` is stored
  as SQL NULL**, never `'{}'`/`''`, so `None → NULL → None` round-trips exactly; a stored `'{}'` reads
  back as `None` (deliberate legacy tolerance — do not remove that filter). Both columns are
  **BASELINE-v1**: they are in the original `SCHEMA_SQL`, so no forward migration is needed and a
  future migration must NOT `ALTER TABLE ADD COLUMN` either one (it would hard-error on every existing
  database). `apply_reparent` and the storage testkit helper deliberately stay 5-column — they
  synthesise an edge with no user `Dependency` object, so the column DEFAULTs are correct there.
  **CLOSED by D44 (v1.0.1) — this REPLACES the D42 "BOUND" carve-out, which is RETIRED.** D42 recorded
  that `Session::create_issue` inserted the issue and THEN wrote each declared edge through a separate
  `Storage::add_dependency` call, and scoped the consequences out. That carve-out was also INCOMPLETE:
  it named only the foreign-key symptom and never the silent class. For the record, the pre-D44
  behaviour had THREE outcomes, all reproduced live on 1.0.1-rc.2 — (i) a `deps[].issue_id` naming a
  NON-EXISTENT issue returned a database error naming a foreign-key failure with the issue row ALREADY
  COMMITTED, orphaned with zero edges and immediately offered by `ready`; (ii) a `deps[].issue_id`
  naming an EXISTING but UNRELATED issue returned `isError:false` while the edge landed on that
  unrelated issue, silently dropping it out of the ready set without moving its `updated_at` or
  `content_hash`, and the newly created issue got ZERO edges; (iii) every non-foreign-key rejection
  (cycle / self / duplicate) still committed the issue row plus the PREFIX of edges before the failing
  element, under one error naming neither the index nor the count. **D44 makes the single create path
  atomic and correctly anchored:** the engine seeds the built `Issue`'s `dependencies` and
  `Storage::create_issue` writes the row, its labels, EVERY edge and every event in the ONE
  `BEGIN IMMEDIATE` transaction it already opens — the same seeded per-record body the bulk primitive
  uses, which BINDS `issue.id` as the edge source and never reads `dep.issue_id`. So a `deps` element
  can only ever attach to the issue being created; a failure on ANY element rolls back the WHOLE create
  (ZERO rows — no orphan issue, no prefix of edges). The engine writes no follow-up edge pass after the
  insert. `dep.metadata` and `dep.thread_id` now round-trip on this path too (the 7-column bind above is
  the same code), so the D42 no-round-trip disclaimer for `issue create {deps:[…]}` is retired with it.
  Wire consequence (§5.2): `deps[].issue_id` becomes OPTIONAL and MUST be omitted — the create arm
  sources the edge implicitly (D44 strict implicit ownership). Guard parity on this path is NORMATIVE
  and specified in the `create_issue` row of this section — restoring it is part of D44, not a
  follow-up. **CLOSED by D45 (v1.0.1) — the `ub-lp9.25` forward reference is DISCHARGED, not deleted.**
  For the record of what D44 shipped: `depends_on_id` has NO foreign key (deliberately — an external
  target is legitimate, and since D45 it is DEFINED, §1.9), so at D44 a NON-EXISTENT BLOCKER id was
  still accepted on this and on every other edge-writing path, and D44 claimed nothing about it. D45
  closes the class in the SAME v1.0.1 cut, as Miguel ruled, because D44 widened it by removing the
  `issue_id` foreign-key failure that used to mask a bogus target. The rule now lives in the
  **Dependency-TARGET existence** bullet of this section (application-level, in-transaction, batch-aware,
  with the §1.9 external carve-out); the schema is UNCHANGED — no foreign key, no CHECK, no trigger.
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
- **`diagnostics(kind, since)` (FR-15, pure-DB — D26; no git, NFR-6).** The read path — **7 kinds through
  D44, EIGHT since D45's `dangling` (below)** — composes
  ONLY existing `Storage` reads for changelog/lint/orphans/dangling — NO new trait method there. **`stats`** = the
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
  narrowing — the faithful FR-15 reading). **`dangling` (D45)** = the dependency edges whose target denotes
  nothing — the read view of the class the D45 write guard now refuses, so a workspace that already
  accumulated such edges can enumerate them. **Composed in the ENGINE from TWO EXISTING `Storage` reads —
  NO new trait method** (the D26 composition pattern applied literally, so no `Storage` signature moves and
  no test fake churns): `dependency_graph(&[])` (empty roots = the WHOLE graph, this section) whose loader
  is a bare `SELECT issue_id, depends_on_id, type FROM dependencies` with NO join — which is precisely why
  dangling edges SURVIVE into the returned `DepTree` — differenced against the id set from `list_issues`. A
  `GraphEdge` is a finding iff its `to` is neither in that id set nor `is_external_target` (§1.9 — an
  external target is a legitimate blocker, never a finding).
  - **TRAP, pinned normatively: the id set MUST come from FULLY-INCLUSIVE filters** —
    `ListFilters { include_closed: true, include_deferred: true, include_tombstone: true, .. }`. With the
    DEFAULT filters (which exclude closed/tombstone) every CLOSED blocker would be reported as dangling — a
    diagnostic that fabricates its own findings.
    **This corpus is DELIBERATELY WIDER than the EXPORT corpus, and the two must NEVER be conflated.**
    The export corpus is the D23 retain PLUS D45's blocker closure (§1.10) — still narrower than "every
    row", since an ephemeral / `-wisp-` row nothing depends on is still not exported. This set is every row
    in the database, full stop. Consequently **an edge whose target is an ephemeral / `-wisp-` row is NOT a
    dangling finding** — the row exists. An implementer who reads this set as "the export corpus" reports
    every such edge as a false finding, which is precisely the self-fabricating diagnostic this trap exists
    to prevent.
  - **FINDING SHAPE (NORMATIVE — it is snapshot-pinned output).** `DiagnosticFinding` has exactly two
    `String` fields (§1.10), so the three facts a reader needs — the dependent, the phantom target, and the
    EDGE TYPE — are encoded into them. The edge type is not decoration: it is what distinguishes a
    permanently-stuck issue (`blocks`/`conditional-blocks`/`waits-for`) from a merely-phantom parent
    (`parent-child`), which does not gate ready work at all. Pinned format:
    `label` = the DEPENDENT issue id (the row carrying the broken edge); `detail` =
    `format!("{dep_type} -> {target}")`, where `{dep_type}` is `DependencyType::as_str()` (e.g. `blocks`,
    `parent-child`) and `{target}` is the raw `depends_on_id`. Example: label `ub-lp9`, detail
    `blocks -> ub-ghost`. One finding per dangling EDGE.
  - **ORDER (PINNED for NFR-14):** the findings are sorted by **`(issue_id, dep_type, depends_on_id)`** —
    dependent id ASC, then dependency type ASC, then target ASC. This is a DELIBERATE re-sort in the
    engine, NOT the order `dependency_graph` returns (that read sorts by `(from, to, dep_type)`, this
    section) — the engine re-sorts so the output groups a dependent's broken edges by kind. The triple is a
    TOTAL order over the result set because the `dependencies` primary key is `(issue_id, depends_on_id)`,
    so no two rows share all three components.
  - **ADVISORY, no write permit** (FR-10): it is a read. The race window between the two reads is
    acceptable for an advisory report.
  - **COST, measured or a v1.1 seam.** Two reads plus O(N) memory, versus the one-query alternative (the
    `blocked_issues` LEFT-JOIN shape with the predicate inverted — `i.id IS NULL` AND not external), which
    would cost a new trait method and every fake implementation. The composition is chosen for this cut; if
    a workspace at the scale the 250k ready-sort bench targets shows a real cost, the single-query
    primitive is an ADDITIVE v1.1 seam.
  Every kind emits generic `DiagnosticFinding{label,detail}` rows
  (§1.10 / §5.3), so the per-kind ENRICHMENT does NOT touch the mcp schema bundle (no `CONTRACT_VERSION`
  bump — §5.4/D25). **D45 is the one exception, and it is an exception about the KIND ENUM, not about the
  finding rows:** adding the `dangling` kind grows `DiagnosticKind` (§1.10) and `DiagnosticsInput` (§5.2),
  which ARE hashed bundle bytes — hence the `unblock.mcp.v1.8` bump recorded in the §5.4 ledger. The
  finding ROWS stay generic, exactly as this clause says. **Emission order (NFR-14 insta):** stats findings in the fixed order `open, in_progress,
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

The comment methods (FR-6, D37 — normative; every mutation runs inside one `with_immediate_tx` so the row +
its `Event` commit together, FR-9):

- **`add_comment(issue_id, author, body, actor)`** — **EXISTENCE guard first (FORK-3):** the target issue MUST
  exist (a non-existent/tombstoned id → `StorageError::IssueNotFound` → `ErrorCode::IssueNotFound`, FORK-E1 —
  NO new ErrorCode); a CLOSED issue is ALLOWED (post-mortem). `INSERT INTO comments(issue_id, author, text,
  created_at) VALUES(…)` with `created_at = now` and **`updated_at` left NULL** (the create path is create-time
  only — MUST-1: only `update` ever sets `updated_at`); `append_event_in_tx(Commented, actor)`; bump
  `issues.updated_at` (FORK-S1, feeds `stale`; NOT hashed); re-`SELECT` → the created `Comment`.
  **MUST-1 SCOPE (NORMATIVE — read this before touching any other INSERT):** MUST-1 constrains **`add_comment`
  ONLY**. It says nothing about the create/bulk/**import** seed path (`crud.rs::insert_issue_in_tx`), which
  persists caller-supplied `Comment` values verbatim and therefore MUST bind `updated_at` **and** `redacted_at`
  from the `Comment` it is given. Over-applying MUST-1 to that seed INSERT — leaving it 4-column — silently
  drops both fields, so a redacted comment imports back **un-redacted** and the §3.2.1 round-trip guarantee
  below (and the D37 import AC) become unreachable. The two paths are distinct: `add_comment` MINTS a new
  comment (now, no updated_at); the seed path REPLAYS an existing one (whatever state it carries).
- **`update_comment(comment_id, body, actor)` (provenance-preserving, D-D)** — guard the comment row exists →
  else `StorageError::CommentNotFound { id: comment_id }` (§3.1), which maps to `ErrorCode::IssueNotFound` at L7
  (FORK-E1 — the code is REUSED; the StorageError variant is not);
  `UPDATE comments SET text=?, updated_at=now WHERE id=?`;
  `append_event_in_tx(CommentEdited, old=Some(old_body), new=Some(new_body))`; bump `issues.updated_at`; re-`SELECT`
  → the edited `Comment`. **In-place-replace-without-provenance is FORBIDDEN** — the `updated_at` bump + the
  `CommentEdited` event ARE the provenance.
- **`delete_comment(comment_id, actor)` (soft-redact, D-E)** — guard exists (idempotent no-op if ALREADY redacted,
  mirroring `restore_issue`'s already-active no-op); `UPDATE comments SET redacted_at=now, text='' WHERE id=?`
  (mask/clear the body, **KEEP the row**); `append_event_in_tx(CommentRedacted, old=Some(old_body), new=None)` — the
  Event RETAINS the original body for provenance (FORK-redact-wire); bump `issues.updated_at`; re-`SELECT` → the
  redacted `Comment` (`redacted_at` present + `"text":""`). A SINGLE deletion op, NOT hard-delete.
- **`list_comments(issue_id)`** — `SELECT … FROM comments WHERE issue_id=? ORDER BY created_at ASC, id ASC`
  (canonical order); reads on the read conn (no permit).
- **Read hydration (all 7 read paths, D37).** `get_issue`/`get_issues`/list/ready/blocked/search/stale populate
  `Issue.comments` (ordered `created_at ASC, id ASC`) via the shared batch `hydrate_ids` / single `hydrate`
  accumulators — exactly how labels + deps hydrate today (T3.5.1). JSONL export now emits a non-empty
  `Issue.comments` (previously always empty for lack of hydration), consistent with `ImportReport.comments` (§1.10,
  D24); the redacted state (`redacted_at` + `"text":""`) serializes and round-trips.

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

**EventType-per-mutation (the T0.7 oracle).** Model `EventType` = 17 named (Created, Updated,
StatusChanged, PriorityChanged, AssigneeChanged, Commented, Closed, Reopened, DependencyAdded,
DependencyRemoved, LabelAdded, LabelRemoved, Compacted, Deleted, Restored, **CommentEdited**, **CommentRedacted**
— the last two D37/v1) + `Custom` — **no `Deferred`, no `Claimed`**. Each mutation emits exactly:

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
| comment add | `Commented` |
| comment update (edit, D-D) | `CommentEdited` |
| comment delete (soft-redact, D-E) | `CommentRedacted` |
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

- **WAL** journal mode; **`busy_timeout = 5000 ms` (native, `Connection::busy_timeout`)** — `const BUSY_TIMEOUT_MS: u64 = 5000`. This is the **sanctioned INVERSE of beads**, which set `busy_timeout = 0` + a hand-rolled flock + sleep backoff to dodge *frankensqlite*'s hot-spin. libsql ships **real SQLite**, whose native `busy_timeout` is sleep-based (it blocks, it never spins), so a non-zero native timeout resolves fsqlite-243 **by construction**. The beads `busy_timeout=0` + hand-rolled SQLITE_BUSY backoff-spin machinery stays **REJECTED** (native timeout blocks, never spins); **the cross-process advisory `.write.lock` flock IS restored (D31)** as a SEPARATE cross-process write serializer composing ABOVE this native `busy_timeout` — via `std::fs::File::try_lock` + a bounded async SLEEP-poll (`tokio::time::sleep(25ms)`, never a busy-spin, NFR-3), NOT the beads `busy_timeout=0`+backoff dodge for frankensqlite's hot handler (see the D31 advisory-lock bullet below).
- **Pragmas (read schema.rs:606–643):** `foreign_keys = ON`, `synchronous = NORMAL`, `temp_store = MEMORY`, `cache_size = -8000`, `journal_size_limit = 33554432` on every connection; the **WAL-only** pragmas — `journal_mode = WAL` and **`wal_autocheckpoint = 0`** (+ a **manual `wal_checkpoint(TRUNCATE)`** on fresh-bootstrap) — are applied **on the file-backed path only**. A shared-cache `:memory:` DB **cannot** use WAL (it always reports `journal_mode = memory`), so asserting WAL there is both a no-op AND an intermittent "API misuse"/`DatabaseLocked` flake under parallel opens; it is skipped for `open_in_memory`. **Periodic in-flight checkpointing (RESOLVED at T0.8):** a **passive** `wal_checkpoint(PASSIVE)` fires on the **held write connection** every **50 committed mutations** (`CHECKPOINT_EVERY_N_MUTATIONS = 50`) — **never `TRUNCATE` in the write path** (an exclusive lock there would manufacture contention). This is distinct from the one-shot fresh-bootstrap `wal_checkpoint(TRUNCATE)` above: that runs **once at migration time on an empty DB** (no concurrent writers to block), whereas the steady-state write path uses PASSIVE only. PASSIVE folds committed frames back into the main DB without blocking, so the WAL file's space is reused in place and stays **bounded** (it does not shrink to zero — PASSIVE reuses, it does not truncate). The T0.8 contention lab asserts the `-wal` sidecar stays bounded under sustained multi-instance contention with this cadence on, and a `#[ignore]`d negative control shows it **breaches** the ceiling with it off.
- **Transactions:** every **mutating** tx uses **`BEGIN IMMEDIATE`** (`transaction_with_behavior(TransactionBehavior::Immediate)`); reads use the default **Deferred** behaviour.
- **Cross-process advisory write lock (D31 — normative).** Every mutation acquires the advisory `.unblock/.write.lock` EXCLUSIVE at the WHOLE-MUTATION scope via `Storage::acquire_write_lock` (§3.2) — the engine holds the guard across the allocation READ + the write tx, composing BELOW the L5 `Semaphore` permit and ABOVE the write-conn `Mutex` + `BEGIN IMMEDIATE` (acquire order: permit → `.write.lock` → `Mutex` → `BEGIN IMMEDIATE`, release inner-first; deadlock-free — one in-process writer past the Semaphore, one cross-process resource). Acquire = a native `std::fs::File::try_lock` fast-path, then a bounded async `tokio::time::sleep(25ms)` poll to `write_lock_timeout_ms` (threaded DOWN from `unblock-config` L4 at open — `open_local(path, lock_timeout_ms)`; default 30000; **NO** L2→L4 back-edge, the lock path is derived from the db-file parent, like the health file-state paths). A timeout → retryable `StorageError::DatabaseLocked` (NO new ErrorCode). The lock file is opened `create(true).truncate(false)` with **NO content written** — a pure flock target, kernel-released on process death (never orphans), **DISTINCT** from the vestigial `.unblock.lock` `OrphanedLockFile` detector target (which stays never-written; `unblock-health`/F5 UNCHANGED). **`migrate` bypasses `with_immediate_tx`**, so it takes the SAME `.write.lock` EXCLUSIVE **explicitly with timeout=0 for the WHOLE command, UNCONDITIONALLY** (single-try fail-fast: a concurrent mid-mutation writer makes migrate fail fast with `DatabaseLocked`, never corrupting) — acquired BEFORE the version check + migration run, so both happen UNDER the held lock. Taking it unconditionally (rather than only when the schema advances) removes the lock-free pre-read TOCTOU. Residual (accepted): migrate is "tightened, still best-effort for an already-open pre-migrate connection" — NOT fully enforced (a single-owner model is the full fix, deferred). The in-memory shared-cache path takes NO lock (`acquire_write_lock` → `Ok(None)` — connection-private, no cross-process sharing). The in-process `Semaphore` (L5) and write-conn `Mutex` (L2) BOTH STAY — the file lock is an ADDITIONAL cross-process layer, never a replacement. NFS/SMB/9p void it (documented residual — advisory locks + WAL `-shm` break; no fs-type detection).
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

// WorkspaceSource is config-owned (DEFINED in unblock-config) — which precedence tier of workspace
// discovery bound the dir (D39). Carried by BOTH contexts so the CLI can report the binding + tier
// at startup (spine §4 discovery block, clause 3). ADDITIVE: not on the MCP wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSource {
    ExplicitDir,        // `--dir` / `UNBLOCK_DIR` (cli.dir)
    ExplicitDb,         // `--db` derived the dir
    ProjectDir,         // `CLAUDE_PROJECT_DIR` project-root probe (D39)
    WalkUp,             // the guarded cwd walk-up
}

// (1) resolve-only — NO storage; discovery + resolved config only (for `where`, doctor pre-checks,
//     completions, and anything that must not open/migrate the DB).
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    pub workspace_dir: PathBuf,        // project root (the dir that CONTAINS `.unblock/`)
    pub actor: String,                 // authoritative actor (§4.1) — NOT inside ResolvedConfig
    pub config: ResolvedConfig,        // config-owned (DEFINED in unblock-config)
    pub paths: ConfigPaths,            // config-owned: resolved `.unblock/` + db/jsonl paths (T1.3a)
    pub source: WorkspaceSource,       // which discovery tier bound the dir (D39 — ADDITIVE)
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
    pub source: WorkspaceSource,       // which discovery tier bound the dir (D39 — ADDITIVE)
}
pub async fn open_with_storage(start: &Path) -> Result<WorkspaceContext, ConfigError>;

// (3) T1.3-ADDITIVE CLI overloads (FORK-1 — OVERLOAD model). The `&Path` facades above are PERMANENT and
//     UNCHANGED; each DELEGATES to its `_with_cli` form passing `start` as the WALK-UP START parameter, NOT as
//     `cli.dir`: `discover_unblock_dir(Some(start), &CliOverrides::default(), &ProcessEnv)` (the third arg is the
//     `EnvSource` for `CLAUDE_PROJECT_DIR`/`$HOME` — D39; internal signature only, the `&Path`/`_with_cli` public
//     surface is unchanged). (`cli.dir` is the EXPLICIT
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
> for dot-dir-hostile environments, FORK-2/D8; the cwd walk-up is bounded by the D39 guard — see the discovery block
> below), libsql open + migrate via
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
> `discover_unblock_dir(Some(start), &CliOverrides::default(), &ProcessEnv)` — **NOT** as `cli.dir` (the third arg
> is the `EnvSource` for `CLAUDE_PROJECT_DIR`/`$HOME`, D39 — internal signature only). (`cli.dir` is the EXPLICIT
> `--dir`/`UNBLOCK_DIR` override, which **does not walk up**; the `&Path` facades want the walk-up-from-`start`
> behaviour, so they leave `cli.dir` unset and route `start` through the discovery start parameter.) Every existing
> caller keeps compiling unchanged and the engine (which binds to the **result** type
> `WorkspaceContext`, never to a facade signature) is unaffected. (This reconciles the spine `&Path` ↔ config-plan
> `&CliOverrides` drift by **overload addition**, not by sequencing/swapping the parameter — the `&Path` API never
> goes away.)

> **Workspace discovery precedence + walk-up guard + startup visibility (D39 — normative).** Discovery of the
> workspace dir (named **`.unblock` OR `_unblock`**, FORK-2/D8) resolves through a single, total precedence chain
> owned by `unblock-config::discover_unblock_dir` (the one home — MCP and CLI cannot drift, CF-D). Internally
> `discover_unblock_dir` gains a THIRD parameter `env: &dyn EnvSource` (production `ProcessEnv`; tests `MapEnv`,
> never the process-global env — NFR-16) and returns the winning tier alongside the dir (an internal
> `DiscoveredWorkspace { unblock_dir, source: WorkspaceSource }`) so `unblock-config` can populate the context
> `source` field. **The two `&Path` facades and the two `_with_cli` overloads are SIGNATURE-STABLE (FORK-1/MF-2):
> the `env` and the `source` are carried BELOW/INSIDE them, never as a new public facade parameter.** From highest
> to lowest:
>
> 1. **`--db`** (`cli.db`): an explicit database path under a `.unblock`/`_unblock` component **derives** that dir
>    (original beads parity) — source `ExplicitDb`. **No walk-up.**
> 2. **`--dir` / `UNBLOCK_DIR`** (`cli.dir`): the **explicit** workspace dir — used directly if itself named
>    `.unblock`/`_unblock`, else treated as a workspace **root** whose `.unblock`/`_unblock` child is used (MF-2) —
>    source `ExplicitDir`. **No walk-up.** `--dir` and `UNBLOCK_DIR` are **one slot**: the CLI binds `--dir` with
>    clap `env = "UNBLOCK_DIR"`, so `--dir` > `UNBLOCK_DIR` is resolved before discovery and both arrive as `cli.dir`.
> 3. **`CLAUDE_PROJECT_DIR`** (read from the **process environment** via the injectable `EnvSource`): the project
>    root injected by an MCP host (e.g. Claude Code) into the spawned stdio child so a server can resolve
>    project-relative paths without depending on the child's arbitrary working directory. It is treated as a
>    workspace **ROOT** — its `.unblock`/`_unblock` child is probed with the SAME child-probe as a `--dir` root
>    (`_unblock` alias + Seam C canonicalization), **no walk-up** — source `ProjectDir`. It is an **ambient hint**,
>    not a per-invocation user choice: on a **miss** (root exists but has no `.unblock`/`_unblock` child) discovery
>    **falls through to (4)** rather than hard-erroring. `CLAUDE_PROJECT_DIR` is read **only** here via the
>    process-env seam — it is a foreign (non-`UNBLOCK_`) key and is **not** part of the `UNBLOCK_*`/`config.toml`/CLI
>    layered config, so it never enters `EnvOverrides`/`ResolvedConfig`.
> 4. **cwd walk-up** (the fallback): when (1)–(3) are all unset/miss, discovery walks **up the ancestors of the
>    process current working directory** for the nearest `.unblock`/`_unblock` — source `WalkUp`. **The walk-up START
>    is the process cwd** when no explicit `start` is supplied — the `_with_cli` overloads pass `start = None` (⇒ cwd)
>    and the `&Path` facades pass their `start` as the walk-up seed. *(This closes a documented GAP: the prior spine
>    never stated that the default walk-up origin is the process cwd.)*
>
> **Walk-up GUARD (normative).** The cwd walk-up (4) is **bounded**, not unbounded to the filesystem root. Ascending,
> discovery **probes each ancestor** for `.unblock`/`_unblock` **FIRST** and then **stops at the first boundary**:
> (i) an ancestor that is a **repository root** — detected by a plain filesystem existence check of a `.git` entry
> (`dir.join(".git").exists()`, directory OR file — the latter catches worktree/submodule gitdir pointers) — and/or
> (ii) the user's **home directory** (`$HOME`, `%USERPROFILE%` on Windows), read via the same `EnvSource` seam. The
> boundary dir is itself **probed before the stop** (INCLUSIVE), so a repository-root `.unblock` and a deliberate
> `$HOME/.unblock` **stay usable**; the guard forbids only ascending **above** the boundary. The `.git` check is a
> `std::fs` stat on a path that happens to be named `.git`, **not** a git operation and links **no** git library —
> D13/NFR-6 ("no git operations, no git library linked") is upheld. **Rationale (integrity):** an unbounded walk-up
> silently binds a distant, unrelated `.unblock` (e.g. a stray `$HOME/.unblock`) when the current project has none —
> a *silent-wrong-DB* hazard for a data-integrity tool. Bounding the ascent turns that into an explicit
> `WorkspaceNotFound` (unchanged variant → `ErrorCode::NotInitialized`, exit 2). **Root markers = `.git` only in v1**
> (`.hg`/`.svn`/`.jj` are an additive v1.1 follow-up). **Accepted residuals:** a `.unblock` in a parent ABOVE an
> intermediate `.git` needs explicit `--dir`/`UNBLOCK_DIR`; `$HOME` is best-effort (a sparse GUI env with no
> `HOME`/`USERPROFILE` disables only that arm; `.git` + the filesystem root still bound the walk); an at-boundary
> stray `.unblock` still binds-then-reports (the startup line is its safeguard, not a hard reject).
>
> **Startup VISIBILITY (normative, NFR-14).** Whichever tier binds, `unblock mcp` reports the resolved workspace dir
> and the winning tier at startup on **stderr** (never stdout — on `unblock mcp` stdout is MCP framing only, spine
> §5b) via a single **unconditional** line (an `info!` is silent at the default WARN level), so an operator can
> always see *which DB was bound and how*. It carries the `WorkspaceSource` populated by discovery (the `source`
> field on both contexts above) and MUST NOT contain the substring `error[` (the mcp e2e asserts
> `!stderr.contains("error[")`).
>
> **Threading — NO facade break (additive, NOT a D35 semver event).** `CLAUDE_PROJECT_DIR` and `$HOME` are read
> **inside** resolution via the existing `EnvSource` seam exactly as actor resolution already reads
> `UNBLOCK_ACTOR`/`$USER`. A newly-honored env var (nothing removed), the walk-up merely bounded (an integrity fix
> on undefined-in-spec behaviour, not a stable-surface change), no new `ErrorCode`, the 0–8 exit table unchanged,
> `CONTRACT_HASH`/`CONTRACT_VERSION` unmoved, and no new CLI flag. `roots` (SEP-2577) is rejected.

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
    pub deps: Vec<NewDep>,                        // D44 — SOURCE-LESS edges; seeded onto the built
                                                  // `Issue.dependencies` and written in the SAME tx as
                                                  // the row (never a follow-up edge pass).
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

// NewDep is ENGINE-owned (defined in unblock-engine, next to NewIssue; D44) — a dependency edge declared
// on a create, with the SOURCE deliberately ABSENT. The source is the issue being created, whose id the
// engine mints (D21), so the field could only ever hold one correct value and the client cannot derive
// it. Making the type incapable of carrying a source is the STRUCTURAL half of D44: a foreign `issue_id`
// cannot reach L5 at all, so the misattachment class is UNREPRESENTABLE rather than merely unreached.
// `create_issue` stamps `issue_id = <minted id>`, `created_at = now`, `created_by = <session actor>` and
// `thread_id = None` when it builds each `Issue.dependencies` element — so the actor/timestamp stamping
// moves from the L7 adapter to L5, where the Session owns the actor. The model `Dependency` (§1.7) KEEPS
// its `issue_id`: it is the persisted + read shape, where the source is real. `create_bulk` builds its
// `Issue.dependencies` from the SAME type (it merges `record.deps` with its resolved `dep_refs`, both
// stamped with the minted id), which also removes a pre-D44 mismatch INSIDE the engine: `create_bulk`
// copies `record.deps` VERBATIM into the BUILT `Issue` it hands to storage
// (`crates/unblock-engine/src/session/write.rs:367`), so a caller-supplied `issue_id` reached L2 on that
// object while the INSERT re-anchored the row on the minted id anyway — the built object and the
// persisted row disagreed. NO claim is made about the RETURNED object: `create_bulk` discards the built
// issues and returns rows re-read from storage (`write.rs:272-283` — `get_issues(&ids)` then a by-id
// projection), so a supplied `issue_id` never reached a caller and never could.
#[derive(Debug, Clone)]
pub struct NewDep {
    pub depends_on_id: String,        // the BLOCKER (target). An external target stays legal and is now
                                      // DEFINED by `unblock_model::is_external_target` (§1.9, D45,
                                      // ASCII-case-insensitive). Target EXISTENCE is GUARDED since D45:
                                      // any other target must be visible to the create's own transaction
                                      // or staged by it, else `StorageError::BlockerNotFound` ->
                                      // `ISSUE_NOT_FOUND` and ZERO rows (§3.2.1). D44's open forward
                                      // reference to `ub-lp9.25` is DISCHARGED by D45.
    pub dep_type: DependencyType,
    pub metadata: Option<String>,     // round-trips since D42's 7-column bind; since D44 on THIS path too
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
    pub async fn list_comments(&self, id: &str) -> Result<Vec<Comment>, EngineError>; // FR-6/D37 — backs `comment list` (§5.2); no permit (FR-10)
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
    //   (FR-26) is byte-stable. It delegates to `Storage::create_issue` — the SINGLE-record tx — so since
    //   D44 it also carries that method's create-specific duplicate + gating-cycle guards (§3.2.1) and its
    //   seeded edges are re-anchored to the supplied id. **CORRECTION (D44, stale since D22/T2.3): the
    //   bulk-markdown, JSONL and `bd`-import paths do NOT call this.** `Session::create_bulk` (below) and
    //   `unblock-sync`'s `import_jsonl`/`import_bd` both call the ATOMIC `Storage::create_issues`, which is
    //   why D44's create-specific guards provably cannot change bulk or import semantics. This method is
    //   the id-preserving SINGLE-record path, and tombstones/imported rows reach storage with their
    //   original ids through it or through `create_issues`. STAYS (D21).
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
    //   minting is the engine's job, NOT an L7 adapter's; FR-9 single mutation home). **Dependency edges
    //   (NORMATIVE — D44):** it SEEDS each `new.deps` element onto the built `Issue.dependencies`, stamping
    //   `issue_id = the just-minted id` (plus `created_at = now`, `created_by = actor`, `thread_id = None`) —
    //   the source is ALWAYS the issue being created and is NEVER taken from the client, which is why
    //   `NewDep` has no source field at all. `storage.create_issue` then writes the row, its labels, EVERY
    //   edge and every event in ONE transaction, and applies the create-specific duplicate + gating-cycle
    //   guards (§3.2.1), so the create is all-or-nothing: any rejection (self, duplicate, cycle, backend)
    //   leaves ZERO rows — no orphan issue, no prefix of edges. There is NO post-insert edge pass; the
    //   pre-D44 shape wrote each edge in its own INDEPENDENT transaction and is DELETED.
    //   `Storage::create_issue` re-anchors seeded edges anyway (§3.2.1), so the two halves agree by
    //   construction. It then returns the created `Issue`, hydrated — `dependencies[i].issue_id` equals the
    //   returned `.id` and `metadata` round-trips (the MCP quick-create extracts `.id`).
    //   It maps the markdown-captured fields `design`/`acceptance_criteria`/`assignee`/`agent_context`
    //   (D22) onto the built `Issue` fields of the same name (the domain `Issue` §1.6 already carries them — no
    //   model change). `Session::create(&Issue)` is UNCHANGED **BY D22** (the id-preserving import path already
    //   accepts a fully-built `Issue` with those fields) — D44 DOES change it: it now carries
    //   `Storage::create_issue`'s create-specific duplicate + gating-cycle guards (its own row above; §3.2.1).
    //   NAME: `create_issue` parallels `Storage::create_issue` (engine mints + delegates to storage);
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
    //   `create_bulk` adapter (§5.2) calls THIS — NOT a loop over `create_issue`. **The "the single-record
    //   `create_issue`/`create(&Issue)` paths are UNCHANGED" clause is SUPERSEDED by D44** — the single create is
    //   now all-or-nothing too (edges seeded onto the built `Issue`, one tx; §3.2.1 + the `create_issue` row there).
    //   `create_bulk`'s OWN semantics are UNCHANGED by D44: it keeps the shared body's dedup-and-continue on a
    //   repeated `depends_on_id` and it still has NO cycle guard, deliberately — that body is also the JSONL/`bd`
    //   import body, and adding a guard there could make an already-exported D5 record un-importable.
    //   **AMENDED by D45 (v1.0.1) — PARTIALLY SUPERSEDED (kept as the record of D44's scope).** The
    //   dedup-and-continue and the absence of a CYCLE guard still stand; the shared body now DOES carry the
    //   D45 dependency-target existence guard (§3.2.1), so `create_bulk` and the import leg are covered too.
    //   The un-importability hazard is not waved away — D45 removes its CAUSE by closing the export under its
    //   edges (§1.10). D45 also RELAXES one shipped `create_bulk` rejection: a correctly-spelled `external:`
    //   dependency reference is now ACCEPTED verbatim instead of rejecting the whole batch (§5.2 item (b)).
    //   Step (4)'s
    //   built edges now carry the engine-owned `NewDep` (§4.1), so a bulk record's edges are stamped with the
    //   MINTED id in memory as well as on the row (they were already re-anchored on the row).
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
    // --- comments (FR-6, D37) — each mutation acquires the write permit (D14) + the D31 `.write.lock` for its whole tx;
    //     the body is validated (non-empty trimmed / NUL-rejected) BEFORE the mutation; author = self.actor (FORK-M1b) ---
    pub async fn add_comment(&self, issue_id: &str, body: &str) -> Result<Comment, EngineError>;   // author = self.actor
    pub async fn update_comment(&self, comment_id: i64, body: &str) -> Result<Comment, EngineError>; // provenance-preserving (D-D)
    pub async fn delete_comment(&self, comment_id: i64) -> Result<Comment, EngineError>;            // soft-redact (D-E)
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
    // PRECISION NOTE (T2.2): the mcp `diagnostics` TOOL (§5.1, the 7-kind read path — EIGHT kinds since
    //   D45 added `dangling`, §3.2.1) maps to
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
    pub async fn doctor(&self) -> Result<DiagnosticReport, EngineError>;  // FR-15/FR-16. v1 pre-T3.3 = SIGNATURE only (returns EngineError::FeatureNotWired{feature:"health"}); **T3.3 (HEALTH-LITE, D29) wires the LITE aggregation** — integrity_check rows + pure file-state classification via unblock-health `run_doctor` → DoctorReport, mapped onto DiagnosticReport REUSING DiagnosticKind::Info (NO new model variant, NO §1.10/CONTRACT_HASH change — F2). The cli doctor routes through this from T3.3 (see the note above). **D45 — the dangling-dependency findings are FOLDED IN HERE, in the ENGINE.** `Session::doctor()` additionally awaits the SAME engine-side composition `diagnostics(Dangling)` uses (ONE home, §3.2.1 — not a second implementation) and APPENDS its findings, in that composition's pinned `(issue_id, dep_type, depends_on_id)` order, to the `DiagnosticKind::Info` report AFTER the file-state anomalies (deterministic overall order, NFR-14). The report's `kind` stays `Info` — the fold changes no §1.10 byte; the `Dangling` KIND exists for the `diagnostics` tool arm, where the response must declare what it is. **`unblock_health::run_doctor` is NOT given a third argument and its signature does NOT change, and `unblock-health` gains NO `unblock-storage` dependency:** the list is DB-derived, and D29 clause F3 makes `run_doctor` PURE, non-async and storage-free — that clause is PRESERVED, not reversed. Composing in the engine fold is exactly how the engine already folds in the pure file-state anomalies; passing DB rows into `run_doctor` would REVERSE a shipped clause and would need its own decision id, which D45 deliberately does not mint (it already carries one reversal — the shared-insert-body placement). **D45 — COST, stated rather than discovered later.** The fold is UNCONDITIONAL on every `doctor()` call, and the composition differences a whole-graph edge load against a FULLY-INCLUSIVE `list_issues` (closed + deferred + tombstone — the default filters would report every CLOSED blocker as dangling), which hydrates labels, dependencies and comments for every row merely to derive an id SET. So `doctor()` gains O(rows + edges) work and O(rows) peak memory it did not have, on a command whose whole point is to be safe to run on a sick workspace. The single-query alternative (one `LEFT JOIN … WHERE i.id IS NULL` excluding external targets, costing a new `Storage` method and 8+ test fakes) is DEFERRED to v1.1 **with an obligation, not an opinion**: the implementation commit MEASURES the composed path on the existing large-workspace fixture and records the number, because the repo's only bench gate covers the ready-sort at 250k issues and reaches nothing on this path. **THE OBLIGATION IS DISCHARGED — the measured numbers, recorded here rather than in a report that can go stale.** Fixture: the EXISTING large-workspace fixture `crates/unblock-engine/tests/scale.rs` (NFR-2, the storage-direct 250k `seed_corpus`), dev profile, run as the CI `scale` job runs it (`cargo test -p unblock-engine --features testkit --test scale`); the measurement is a REPORTING pair of timings inside `run_scale` (`diagnostics(Dangling)` = the fold's exact added work, since `doctor()` awaits the SAME composition; then `doctor()` = the composed total), so it is re-derived on every `scale` run instead of living only in prose. **At 250k rows, three runs: the fold costs 4.51s / 4.55s / 4.65s and the composed `doctor()` costs 7.00s / 7.05s / 7.12s, of which the pre-D45 half (`integrity_check` + the pure file-state classification) is 2.47s / 2.48s / 2.53s — so the fold roughly TRIPLES `doctor()` at 250k, and it is the dominant term.** The macOS/laptop absolute values are not a budget (the cell asserts only the existing generous boundedness guard, never a ceiling); the RATIO and the shape are the finding. **Two honesty bounds on that number, stated so a later reader does not over-trust it:** (a) `seed_corpus` writes rows with NO dependencies, labels or comments, so the 250k corpus has an EMPTY `dependencies` table — the measured 4.5s is the ROW half (the fully-inclusive `list_issues` hydration) alone, and the edge half is measured at zero; a workspace with a real edge graph pays MORE, never less; (b) the fold is a read pair with no write permit, so the cost is latency on the caller, not lock hold time. This is the evidence the v1.1 deferral rests on: at the corpus size this repo commits to supporting, `doctor()` triples — the single-query alternative is worth its `Storage` method, and it is not worth it before v1.1. **Feature-gate placement, previously MIS-stated and corrected here:** `Session::doctor()` is NOT `#[cfg]`-gated. The method is declared UNCONDITIONALLY (`crates/unblock-engine/src/session/lifecycle.rs:167-184`); only its two BODY blocks carry the gate — a `#[cfg(feature = "health")]` block that composes the report and a `#[cfg(not(feature = "health"))]` block that returns `EngineError::FeatureNotWired { feature: "health" }` — which is why `crates/unblock-cli/src/commands/doctor.rs:43` calls it with no `cfg` of its own. **So the fold lands INSIDE the existing `#[cfg(feature = "health")]` body block, immediately before its `Ok(…)`**, and the `not(health)` block keeps returning `FeatureNotWired` untouched. **The shared dangling COMPOSITION itself is UN-GATED, and that is load-bearing rather than incidental:** it is the same engine-side composition the `diagnostics {kind:"dangling"}` arm calls, and that arm's dispatch (`crates/unblock-engine/src/diagnostics.rs:49-57`) carries no feature gate at all — putting the composition under the `health` cfg would fail to COMPILE the new arm in a `--no-default-features` build. The cost note above is therefore a statement about a `health`-enabled `doctor()` run; the MCP action pays the same cost in every build.
    pub async fn recover(&self) -> Result<DiagnosticReport, EngineError>; // attempt repair (WAL checkpoint, reindex; reports actions taken). STAYS EngineError::FeatureNotWired{feature:"health"} through v1 (F1/D29) — its body (`--repair` + the `.unblock/.recovery/` evidence writer + the rich repair taxonomy) is **v1.1**, NOT T3.3; wiring a hollow "nothing repaired" report would be the faked success FeatureNotWired forbids.
    pub async fn shutdown(&self) -> Result<(), EngineError>; // flush + close libsql cleanly (FR-17). D38: MUST be reached on BOTH cooperative-shutdown returns of run_mcp_server (Ok AND a post-cancel Err(Transport{Cancelled})) — an Err(Cancelled) never skips the clean libsql close (§0.1/§5b).
}

// CloseOutcome / ImportReport / ExportReport are defined in unblock-model §1.10 and
// re-exported here (CF-A) — NOT redefined. CountBucket / GraphEdge / DepTree /
// DiagnosticReport / DiagnosticFinding / DiagnosticKind likewise come from unblock-model
// via the same re-export. SessionConfig + ImportOptions + NewIssue (D21) + NewDep (D44) + MigrateOutcome
// (D27/AF-2) are engine-owned (above); MigrateOutcome is a plain engine-local return (no
// JsonSchema, NOT a §1.10 DTO), exported from unblock-engine like the peer ImportOptions
// (the TRUE engine-local peer — a plain engine-defined return, no JsonSchema; contrast
// CloseOutcome, which IS a §1.10 model DTO the engine merely re-exports).
```

### 4.2 Write-serialization contract (D14 + D31 — normative)

- **(L5, in-process)** One `Arc<tokio::sync::Semaphore>` with **1 permit** per `Session`. Every mutation `acquire()`s the single permit for the **entire MUTATION** — every storage call it makes, reads included — then releases, serializing all in-process writers (linearizable per FR-9). **One-transaction invariant (NORMATIVE — D44).** The WRITE side of a mutation is EXACTLY ONE storage transaction; a mutation may run read probes before or after it, but never a SECOND write transaction. Rationale: the permit and the `.write.lock` give mutual exclusion ONLY — no rollback, no undo log, no compensating action (see the cancel-safety bullet) — so a mutation spanning two write transactions can be interrupted between them and leave a half-state that no lock and no cancel-safety property can undo. `Session::create_issue` was the ONE violation (the row insert, then one independent edge transaction per declared dep) and D44 collapses it into the single `Storage::create_issue` transaction. **`Session::migrate` is the ONE carve-out**: it deliberately bypasses `with_immediate_tx` and holds the EXCLUSIVE `.write.lock` (timeout=0, fail-fast) for the whole command (§3.2), so its multi-statement shape is governed by the migration contract, not by this bullet.
- **(L2, cross-process — D31)** Under the permit, the engine ALSO acquires the storage advisory `.unblock/.write.lock` guard (`Storage::acquire_write_lock`, spine §3.2) for the **WHOLE mutation** — the SAME span as the permit, covering the `next_child_number` allocation READ AND the write tx — so two MCP-server children (the SUPPORTED child-per-client topology, D31) cannot both mint the same `parent.N` or interleave writes across processes. Acquire order: permit → `.write.lock` → write-conn Mutex → `BEGIN IMMEDIATE`; release inner-first. The permit and the file lock COMPOSE (both retained); the Semaphore is NOT dropped. A NESTED acquire by the current in-process holder is a re-entrant no-op guard (owns/releases nothing) via the MF4 in-memory held-marker (`WriteLock.held`, the faithful port of beads `write_lock_already_held`), so the holder never self-contends on its own per-fd advisory lock (an internal L2 detail, no public-surface change).
- **Reads NEVER touch either lock** (FR-10): they run concurrently against libsql WAL readers while a write holds the permit + the file lock.
- **Supported topology = child-per-client (multiple MCP servers, D31).** The D14 "in-process only / exactly one MCP server per workspace / multiple MCP servers not supported" clause is RETIRED — cross-process write serialization is the restored `.unblock/.write.lock` advisory lock (an L2 storage primitive), with `BEGIN IMMEDIATE` + native `busy_timeout` as the WAL-level backstop. `migrate` acquires the EXCLUSIVE lock (timeout=0 fail-fast) for the whole command, unconditionally; the v1-lite read-only `doctor` takes no lock. NFS/SMB/9p void it (documented residual — no fs-type detection).
- Permit acquisition is **uncancel-safe across the tx boundary**: a dropped future before commit must release the permit AND the file lock (RAII on both guards) and leave the DB committed-or-rolled-back (no partial state) — verified by the SIGTERM-mid-write failure-injection test (NFR-5). **"No partial state" is a per-MUTATION claim, and it holds only because of the one-transaction invariant above.** Pre-D44 it held per transaction and FAILED per operation on `Session::create_issue`: a future cancelled between two edge transactions committed the issue plus the edges written so far and returned NO error to anyone at all. Any future mutation that would need two write transactions must be redesigned, not documented around. *(This is cancel-safety of the write **tx**. Its peer — cancel-safety of the **process exit** (the D38 no-hang invariant: no return path may block on a runtime drop with a parked stdin read) — is specified in §5b, `mcp`.)*
- Property test (FR-9): interleaved mutations through the engine are linearizable; MCP and CLI produce identical results for the same op.

---

## 5. MCP schemas — `unblock-mcp` (L7)

**rmcp 1.7** (`server`, `transport-io`) stdio server (`unblock mcp`), thin adapter over `Session`. **8 consolidated tools** (the ≤ 8 RK-3 ceiling — the D37 `comment` tool landed at T3.9 as the 8th, §5.1 row 8 / §6.6, so the budget is now **FULL**). Resources, prompts. Every tool input/output derives `JsonSchema` + `Serialize`/`Deserialize` — inputs AND outputs ride the schema bundle as per-tool `{input, output}` pairs (D25, §5.3/§5.4); args are **duplicate-key-scanned at the transport (D43)**, quota-checked and strictly deserialized with size/rate limits (NFR-18). Discovery (`capabilities`/`schema`) carries `contract_version` (FR-12), and BOTH discovery documents are covered by the single pinned `CONTRACT_HASH` drift gate (D22 clause 8 widened by D25 — §5.4).

### 5.1 Tool taxonomy (8 tools — `comment` is the 8th, landed with the T3.9/D37 code)

| # | Tool | Discriminator | Maps to |
|---|---|---|---|
| 1 | `issue` | `action: create\|create_bulk\|show\|update\|close\|reopen\|delete\|restore` (D22 `create_bulk` is the 8th `issue` ACTION — a discriminator arm, so it does NOT grow the **tool** count, §6.6) | FR-1a/1b/1c |
| 2 | `claim` | (none) | FR-2 |
| 3 | `defer` | `action: defer\|undefer` | FR-3 |
| 4 | `query` | `kind: list\|ready\|blocked\|search\|count\|stale` | FR-4 |
| 5 | `dep` | `action: add\|remove\|list\|tree\|cycles\|graph` | FR-5 |
| 6 | `sync` | `action: export\|import\|import_bd` | FR-7/8/26 |
| 7 | `diagnostics` | `kind: stats\|info\|where\|version\|lint\|changelog\|orphans\|dangling` (D45 — `dangling` is the 8th `diagnostics` KIND, a discriminator arm, so it does NOT grow the **tool** count, §6.6; no ninth tool is created) | FR-15 |
| 8 | `comment` *(lands T3.9, D37 — the DEDICATED comment tool, D-B; a distinct verb, NOT an `issue` arm)* | `action: add\|list\|update\|delete` | FR-6 |

> **D37 — the 8th tool (`comment`).** A DEDICATED tool (D-B), the deliberate §6.6 exception (RK-3 budget now FULL at
> 8 ≤ 8). It SUPERSEDES the earlier "`issue comment` sub-action" sketch (the `unblock-mcp.md` plan). Landing at
> **T3.9** flipped the live count 7→8 (re-blessing the `capabilities`/`schema_bundle` goldens + the
> `agents_digest` ripple; `list_tools` is NOT a golden — it is the LIVE asserts in `tests/contract_suite.rs`, an
> EDIT) and bumped `CONTRACT_VERSION` `unblock.mcp.v1.4`→`v1.5` with `CONTRACT_HASH` re-pinned — the
> version-string flips + golden re-bless were T3.9 code deliverables, NOT the docs-only spec cascade.

### 5.2 Input shapes (schemars sketches)

```rust
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]   // §5.2a — inputSchema root MUST be `type: object` (CD-1)
pub enum IssueInput {
    Create {
        title: String,
        #[serde(default)] description: Option<String>,
        #[serde(default)] issue_type: Option<IssueType>,
        #[serde(default)] priority: Option<Priority>,
        #[serde(default)] labels: Vec<String>,
        #[serde(default)] parent: Option<String>,
        #[serde(default)] deps: Vec<DepInput>,   // D44 — each element's SOURCE is implicit (the issue
                                                 // being created); see the DepInput block below.
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
    // tool, so it does NOT grow the tool count (≤ 8, §6.6 "extend before add"). The adapter:
    //   (1) caps the parsed record count at `Quotas::max_batch` at the preflight (BEFORE any mint — the args-validation
    //       rule pinned in this section's intro above, `PRD NFR-18`);
    //   (2) parses + validates the WHOLE document via a pure mcp-owned `parse_bulk_markdown(&str)` helper —
    //       a byte-faithful port of `temp/beads_rust-main/src/util/markdown_import.rs::parse_markdown_content`
    //       (H2 record / H3 section grammar; implicit-description quirk; `type:id`/bare/`external:`/`blocked-by`
    //       dep encoding; bulleted/checkbox list items) — ALL-OR-NOTHING PRE-MUTATION (FR-1a "rejected
    //       pre-mutation"): a single malformed/unresolvable block rejects the ENTIRE batch with ONE
    //       `StructuredError{code: ValidationFailed, hint, context}` and ZERO writes (deviation from the original's
    //       best-effort per-issue `continue` — the PRD's safe-import discipline wins, NFR-8);
    //       **PARSE-SIDE REJECTION SET (NORMATIVE — D42, v1.0.1; SUPERSEDES D22 clause 2).** "A single
    //       malformed block" is no longer a hand-wave: `parse_bulk_markdown` rejects the WHOLE document, in-band
    //       with an enumerating `hint` and ZERO writes, on ANY of the following FIVE causes (the `context.kind`
    //       is the stable discriminator) —
    //         (1) an **unrecognized `### ` section** header (`kind = "unknown_section"`);
    //         (2) an **EMPTY `### ` header** (`kind = "empty_section_header"`, a distinct message);
    //         (3) a **`### ` section before the first `## `** (`kind = "section_before_issue"`; it must WIN over (1));
    //         (4) an **invalid `### Priority` value** (`kind = "section_value"` — raised at the `issue.rs` mapping
    //             step, NOT in the parser; previously it silently defaulted to P2);
    //         (5) an **UNTERMINATED fenced code block** (`kind = "unterminated_code_fence"`, naming the OPENING line).
    //       Causes (1)–(4) reject only input GA DESTROYED IN SILENCE, so they are a 1.x bug fix (D37 precedent).
    //       **Cause (5) is NOT: it rejects documents GA v1.0.0 ACCEPTED AND IMPORTED** — GA's parser had no fence
    //       tracking at all, so an unclosed fence was invisible to it. The deviation is therefore from SHIPPED GA
    //       BEHAVIOUR, not merely from CommonMark's "an unclosed fence runs to end of document" reading; it is a
    //       behavioural break in a PATCH release, RATIFIED (PRD D42 clause 4(iii)) because that CommonMark reading
    //       is itself the silent swallow D42 exists to kill — it would consume every later `## `/`### ` into one
    //       section's body with `isError:false`.
    //       **FENCE-AWARENESS is the necessary companion to (1)–(3), not an extra:** between an opening fence
    //       (≤ 3 leading spaces, then a run of ≥ 3 backticks or tildes, then an optional info string — a backtick
    //       fence whose info string contains a backtick does NOT open) and its closer (SAME marker, run at least as
    //       long, NO info string), a `## `/`### ` line is CONTENT, never a header. Without it (1)–(3) would fire
    //       FALSELY on any document embedding a markdown code sample, and a KNOWN section name inside a fence would
    //       tear the sample in half and relocate its bytes into another field with `isError:false`. INDENTED code
    //       blocks need no tracking (`strip_prefix("### ")` already fails on an indented line).
    //       **PUBLISHED:** the `create_bulk` tool description AND the `markdown` field description enumerate all
    //       FIVE rejections + the fence grammar + the closed section-name set (§5.4 ledger, D42);
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
    //       (b) an **unresolved** ref (no stand-in/title/storage match, original `create.rs:1135`/`:1216`) —
    //       **EXCEPT an EXTERNAL target (NORMATIVE — D45): a `dep_ref` for which
    //       `unblock_model::is_external_target` holds (§1.9) is NOT resolved against anything. It is carried
    //       VERBATIM as the edge target and can never be "unresolved".** This is a stated RELAXATION of a
    //       GA-shipped, normatively-pinned rejection on a spine-pinned path, not a clarification: today
    //       `parse_dependency` keeps the whole `external:…` string as the id, the engine's storage probe
    //       probes it as an issue id, misses, and the resolver rejects the ENTIRE batch — so `create_bulk` is
    //       the one path that currently refuses a legitimate external blocker, contradicting the
    //       external-targets-are-legitimate premise the rest of the system is built on. No test covers that
    //       behaviour, so nothing in CI will go red to announce the change; it is recorded HERE and in the
    //       §5.4 D45 ledger entry instead. The engine's pre-transaction probe
    //       (`crates/unblock-engine/src/session/write.rs`) SKIPS external targets accordingly, and the
    //       remaining unresolved-reference rejection keeps `ValidationFailed` (§3.1 — a resolution fault,
    //       distinct from D45's resolved-but-absent id, which is `BlockerNotFound`/`ISSUE_NOT_FOUND`);
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
    //   (4) output reuses `IssueOutput::Issues` (§5.3, D25) — the CD-2 object-wrapped `IssueList` of created issues.
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

// DepInput — the ONLY wire shape for a dependency edge declared on `issue create`.
//
// EXCLUSIVITY IS A SCHEMA CLAIM, NOT A RUST ONE — the distinction is load-bearing and D44 states both
// halves. Schema half (TRUE, and the half the contract rides on): `$defs/DepInput` is `$ref`-ed from
// exactly ONE site, the `issue` tool's `create.deps` array items, so the `dep` tool's published flat
// `Add`/`Remove` arms do NOT move. Rust half (FALSE pre-D44, and the reason this block is normative):
// the TYPE is constructed in code by the `dep` tool — `crates/unblock-mcp/src/tools/dep.rs:120-126`
// builds a `DepInput` literal out of `DepToolInput::Add`'s own REQUIRED `issue_id: String` and calls
// `DepInput::into_dependency(&actor, now)` (`crates/unblock-mcp/src/tools/dto.rs:75-90`), which assigns
// `self.issue_id` straight into the model `Dependency`'s `String` field (`dto.rs:79`).
//
// RESOLUTION (NORMATIVE — D44 decides this; it is NOT left to the implementer). Retyping `issue_id` to
// `Option<String>` is a COMPILE BREAK at that call site, and the mechanical repair an implementer
// reaches for — `unwrap_or_default()` — would write an EMPTY-STRING edge source on the `dep add` path,
// re-introducing the exact class D44 exists to close, on a path that is currently correct. So:
//   * The `dep` tool STOPS routing through `DepInput`. `DepToolInput::Add` builds the model
//     `Dependency` DIRECTLY from its own fields — where `issue_id` is a real, required, client-KNOWN,
//     pre-existing issue id, which is why that arm keeps it (§5.2 `DepToolInput`).
//   * `DepInput::into_dependency` is DELETED with its last PRODUCTION caller. Its OTHER production
//     caller is the create arm (`crates/unblock-mcp/src/tools/issue.rs:353`), which D44 replaces with
//     the `DepInput` -> `NewDep` map. A THIRD call site exists and is named rather than elided: the
//     unit test at `dto.rs:218`, item (f) below, which MOVES to the new construction site — so after
//     D44 the method has no callers at all. Deleting it is what makes the misattachment
//     class UNREPRESENTABLE rather than merely documented: no function exists that can turn a
//     `DepInput` into a model `Dependency` at L7.
//   * EVERY SURVIVING MENTION RIDES ALONG IN THE SAME CHANGE. Enumerated site by site rather than
//     summarised as a count, because THERE IS NO `cargo doc`/RUSTDOC STEP ANYWHERE IN
//     `.github/workflows/` — a broken intra-doc link left behind by this deletion does not even warn,
//     let alone block, so nothing but this list will catch one.
//     The mention set is exactly `git grep -n into_dependency -- crates/` MINUS the `fn` signature that
//     is being deleted (`dto.rs:77`) — SEVEN lines, (a)..(g) below — plus ONE non-mention ride-along,
//     (h), which the deletion strands even though it never names the method. Each is dispositioned:
//       (a) `crates/unblock-mcp/src/tools/dep.rs:126` — CALL SITE. Retired by the clause above: the
//           `dep` tool builds the model `Dependency` directly from `DepToolInput::Add`'s own fields.
//       (b) `crates/unblock-mcp/src/tools/issue.rs:353` — CALL SITE. Retired by the clause above: the
//           create arm becomes the `DepInput` -> `NewDep` map.
//       (c) `crates/unblock-mcp/src/tools/dto.rs:8` — the MODULE doc's intra-doc link, today
//           "[`DepInput::into_dependency`] builds the model [`unblock_model::Dependency`] under a
//           supplied actor/timestamp". REWRITTEN (not merely unlinked) to describe `DepInput` as what
//           it becomes: the create-arm edge input the `issue` adapter maps to the source-less `NewDep`.
//       (d) `crates/unblock-mcp/src/tools/dto.rs:53-57` — the STRUCT doc. **THIS ONE IS PUBLISHED
//           CONTRACT BYTES, NOT AN ORDINARY COMMENT.** `JsonSchema` emits it VERBATIM as
//           `$defs/DepInput.description`; it is pinned in the golden at (e). Its live sentence — that
//           the type is "used both as a `query`/`dep`-tool edge input" AND as an element of the `issue`
//           tool's `create.deps` array, its `created_at`/`created_by` "supplied by the adapter via
//           [`DepInput::into_dependency`]" — is FALSE on BOTH halves once this resolution lands, and
//           the attribution is spelled out because part of it does NOT belong to D44: the `query`
//           mention is ALREADY false at HEAD (`git grep -w DepInput -- crates/unblock-mcp/src` finds no
//           `query.rs` use at all — a pre-existing doc defect this rewrite also repairs); D44 makes the
//           `dep` half false, since that tool stops routing through the type; and D44 deletes the
//           method the second half names. After D44 the type is an element of that ONE array and
//           nothing else. Rewriting it MOVES the
//           `schema_bundle()` bytes,
//           so it is part of the clause-(6) `unblock.mcp.v1.7` bump + golden re-bless, NOT an
//           incidental edit. A reader who treats it as an ordinary comment leaves a FALSE description
//           in the PUBLISHED schema — the precise class D44 clause (6) exists to refuse.
//       (e) `crates/unblock-mcp/tests/snapshots/contract_suite__schema_bundle.snap:37` — the golden
//           that carries (d) verbatim. Re-blessed WITH the bump, never silently (clause 6).
//       (f) `crates/unblock-mcp/src/tools/dto.rs:218` — inside the unit test
//           `dep_input_builds_dependency_with_actor_and_ts` (`dto.rs:209-224`), which MOVES to cover
//           the new `dep.rs` construction site. The cited range runs to `:224` on purpose: `:219-223`
//           are the five assertions that actually have to move, and a range stopping at the
//           `.into_dependency(…)` call at `:218` would relocate the fixture without its checks.
//       (g) `crates/unblock-storage/src/testkit.rs:3136-3137` — a doc reference explaining that
//           `into_dependency` hardcodes `thread_id: None`; RE-POINTED at the new construction site.
//       (h) the D42 `thread_id: None` rationale comment (`dto.rs:85-89`) — it names no method, so no
//           grep for `into_dependency` finds it, yet it lives INSIDE the deleted body. It moves
//           VERBATIM to the new construction site in `dep.rs`: it exists to stop a reader deleting a
//           bind that is otherwise read as dead code, and losing it re-opens that hazard.
//   * `deny_guard.rs:236`'s `assert_denies_unknown_fields::<DepInput>` STAYS and still compiles: an
//     `Option` field deserializes a PRESENT value happily — rejecting it is the adapter's job, never
//     serde's, which is exactly why clause (2)'s rule is an adapter rule.
//   * **PROHIBITED, named because it is the patch of least resistance:** `DepInput.issue_id` must NEVER
//     be defaulted to `""` — no `unwrap_or_default()`, no `unwrap_or("")`, no `Option::take().unwrap_or`
//     — anywhere. The field is read for EXACTLY ONE purpose: `is_some()` -> reject (clause 2). It is
//     never read for an edge source, on any path.
// NORMATIVE — D44:
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]        // D42 — NOT recursive; every nested container carries its own
pub struct DepInput {
    /// The dependent issue id. **OPTIONAL, and on `issue create` it MUST be omitted.** The edge is
    /// sourced on the issue this call is creating, whose id the SERVER mints (D21) — so the field has
    /// exactly one correct value and the client cannot derive it. A PRESENT value is REJECTED with
    /// `VALIDATION_FAILED` and a field hint naming `deps[i].issue_id`; it is NEVER silently ignored and
    /// NEVER applied to the issue it names. An explicit JSON `null` is an ABSENCE, and is accepted.
    #[serde(default)] issue_id: Option<String>,
    /// The blocker issue id (target).
    depends_on_id: String,
    dep_type: DependencyType,
    #[serde(default)] metadata: Option<String>,   // round-trips since D42's 7-column bind; since D44 on
                                                  // the create path too
}
// D44 mapping (dependency edges): the `issue`-tool adapter maps each `DepInput` to the engine-owned,
// SOURCE-LESS `NewDep` (§4.1). Because `NewDep` cannot carry a source, a present `issue_id` has nowhere to
// go and is rejected AT THE ADAPTER — before any mint, before the write permit — so a rejected create
// persists nothing BY CONSTRUCTION, not by rollback. The rule is SYNTACTIC (any present value), because
// the id is minted at L5 AFTER the adapter has built `NewIssue`, so an equality test against the minted id
// is not a question the boundary can ask. ON THE CREATE ARM ONLY, `created_at`/`created_by` are no longer
// stamped at L7: the engine stamps them from the session actor when it seeds `Issue.dependencies` (§4.1).
// The `dep` tool's own `add` arm still stamps both at L7 (`dep.rs:126` — `now` + the session actor), and
// D44 does not move that; it is a cross-issue edge on an EXISTING source, not a create. The one direction a single
// call can express is therefore "the new issue is BLOCKED BY an existing one"; "create an issue that BLOCKS
// an existing one" is a create followed by `dep {action:add}` — the dedicated cross-issue tool, which
// anchors correctly by design, and which the rejection hint must name. That cost is ACCEPTED and stated
// openly (PRD D44): it is not a regression, because that direction is not expressible on ANY create surface
// today (the bulk surface has no way to state it either, and every such single-create payload today is a
// foreign-key error with a committed orphan or a silent misattachment).

#[derive(Deserialize, JsonSchema)]
pub struct ClaimInput { pub id: String, pub assignee: String, #[serde(flatten)] pub attribution: Attribution }

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]   // §5.2a — inputSchema root MUST be `type: object` (CD-1)
pub enum DeferInput {
    Defer   { id: String, until: DateTime<Utc>, #[serde(flatten)] attribution: Attribution },
    Undefer { id: String, #[serde(flatten)] attribution: Attribution },
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]   // §5.2a — inputSchema root MUST be `type: object` (CD-1)
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
#[schemars(extend("type" = "object"))]   // §5.2a — inputSchema root MUST be `type: object` (CD-1)
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
#[schemars(extend("type" = "object"))]   // §5.2a — inputSchema root MUST be `type: object` (CD-1)
pub enum SyncInput {
    Export   { #[serde(default)] path: Option<String> },     // default .unblock/issues.jsonl; path-confined
    Import   { path: String, #[serde(default)] dry_run: bool },
    ImportBd { path: String },                                // D16
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]   // §5.2a — inputSchema root MUST be `type: object` (CD-1)
pub enum DiagnosticsInput {
    Stats {}, Info {}, Where {}, Version {}, Lint {}, Changelog { #[serde(default)] since: Option<DateTime<Utc>> }, Orphans {},
    Dangling {},   // D45 — wire kind `"dangling"`; NO parameters (the report is always workspace-wide);
                   // declared LAST (hash-visible position, §1.10/§5.4), mirroring `DiagnosticKind::Dangling`.
                   // D45 — the `///` DOC COMMENT on this arm is CONTRACT BYTES, exactly as the sibling arms'
                   // are: schemars lifts a variant doc comment into the arm's `description`, which rides
                   // `schema_bundle()`, which `CONTRACT_HASH` digests. It is therefore PINNED byte-for-byte
                   // by the contract snapshot on BOTH sides — here and on `DiagnosticKind::Dangling` (§1.10)
                   // — and re-wording it, even harmlessly, RE-CUTS the hash and is a contract change, never
                   // a comment tidy-up. Pinning the variant NAME and the wire spelling while leaving its
                   // description unpinned is precisely the omission that lets prose drift inside a hash the
                   // suite still calls stable.
}
// D45 tool DESCRIPTION (contract bytes, duplicated: the `#[tool(description)]` wire literal AND the
// `capabilities()` descriptor copy, which is version-coupled — §5.4): it becomes
// "Diagnostics: stats, info, where, version, lint, changelog, orphans, or dangling." Both copies move
// together, with the `capabilities`/`schema_bundle` goldens and the live (name, description) assert.

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

**§5.2a — Tool `inputSchema` root type (NORMATIVE — MCP-conformance drift CD-1, 2026-07-09).** Every published tool input schema MUST have a root `"type": "object"` — both the live rmcp
`tools/list` `inputSchema` AND the `unblock://schema` `ToolSchemas.input` (§5.4). schemars renders the six
`#[serde(tag = …)]` tagged-enum inputs of §5.2 (`IssueInput`, `DeferInput`, `QueryInput`, `DepToolInput`,
`SyncInput`, `DiagnosticsInput`) as a root `oneOf` with **no** root `type`; strict MCP clients reject the
WHOLE `tools/list` at discovery (the TypeScript SDK's
`ToolSchema.inputSchema = z.object({ type: z.literal("object") }).passthrough()` throws `invalid_value at
inputSchema.type`, and it parses the tool array with `.parse()`, so ONE bad element takes every tool — the
conformant `ClaimInput` struct included — dark). rmcp 1.7 guards the tool *output* schema root type
(`schema_for_output`) but **not** the input, so this invariant is unblock-owned at the L7 tool-registration
boundary: each tagged-enum input carries `#[schemars(extend("type" = "object"))]` (schemars_derive lowers it
to a post-mutator inserting `"type": "object"` AFTER the derived `oneOf` body). The `oneOf` discriminated
union is **preserved verbatim** — every branch is already `type: object` (the tag is a `const` property), so
instance validation is UNCHANGED; the root keyword is a **structural requirement of the MCP `inputSchema`
contract**, not a validation change (no flattening, no lost per-variant `required` sets). `ClaimInput` (a plain
struct) is already `type: object` and is untouched. Enforced by a conformance assertion that
`tools[*].inputSchema.type == "object"` for all 8 tools over the live builder-vs-router duplex (§6.6) and by
strengthening the bundle test `every_tool_schema_is_an_object` to assert `input.type == "object"` (not merely
`is_object()`, which a `oneOf` root passes vacuously). The injected root key changes the published
`schema_bundle().<tool>.input` bytes, so it moves `CONTRACT_HASH` and forces a `CONTRACT_VERSION` bump (§5.4,
jointly with the §5.3 output change).

**`comment` input (FR-6, D37 — the 8th tool; lands T3.9).** A tagged-enum input modelled EXACTLY on the other
tool inputs, carrying the CD-1 root `type:object` injection (§5.2a):

```rust
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]   // §5.2a — inputSchema root MUST be `type: object` (CD-1)
pub enum CommentToolInput {
    Add    { issue_id: String, body: String, #[serde(flatten)] attribution: Attribution },
    List   { issue_id: String },
    Update { comment_id: i64,  body: String, #[serde(flatten)] attribution: Attribution },
    Delete { comment_id: i64,               #[serde(flatten)] attribution: Attribution }, // soft-redact (D-E)
}
```

The tool body runs `self.preflight(&input)?` (NFR-18 quota) → match arm → the `Session` method (§4.1) →
`ok_json`/`engine_err_json`. Author over MCP is `self.session.actor()` (no per-comment author — FORK-M1b). The
`Attribution` flatten (capture-only `agent_name`/`harness`/`model`) mirrors the other mutating inputs. Body
validation (non-empty trimmed / NUL-rejected) runs in the engine before the mutation (→ `ValidationFailed`).

### 5.3 Output shapes (D25/FORK-1B — per-tool, MATERIALIZED, NORMATIVE)

The output surface is a family of REAL, mcp-owned types — the single output authority, not documentation.
Tool bodies construct their structured success payload AS an arm of their tool's union (or as the tool's
single output type). All unions are `#[serde(untagged)]` ⇒ the wire bytes are IDENTICAL to serializing the
arm's value directly, so materializing changes NO wire byte and NO golden except the schema bundle (the ONE
exception is the CD-2 object-wrap of the list arms below — a deliberate, wire-visible structural fix). `Box` is
serde- and schemars-transparent (wire bytes + published schema unchanged); the boxed arms (`Issue`, `Close`)
keep `clippy::large_enum_variant` clean under CI `-D warnings` (`ci.yml:63`) — `CloseOutcome` inlines a full
41-field `Issue` (`crates/unblock-model/src/results.rs:46-51`). Each
tool's `schema_for!(<output>)` is its §5.4 `ToolSchemas.output`: a new output shape must join its tool's
union to be returnable, and joining it moves the D25 gate (`CONTRACT_HASH` → `CONTRACT_VERSION` bump).
*(Supersedes the pre-D25 single `ToolOutput` union sketch, which also missed the landed `delete`/`added`/
`removed` shapes; the name `ToolOutput` survives only in historical decision/task records.)*

**Output `structuredContent` root MUST be an object (NORMATIVE — MCP-conformance drift CD-2, 2026-07-09).**
A tool's structured success payload rides the rmcp `CallToolResult.structuredContent`, whose MCP type is an
object (`{[key: string]: unknown}`; corroborated by `Tool.outputSchema.type = const "object"`). The
list-shaped arms below therefore MUST NOT serialize as a bare top-level array: each `Vec` arm is wrapped in a
single-field, mcp-owned object struct — `IssueList{ issues }` (shared by `IssueOutput::Issues` and
`QueryOutput::Issues`), `CountList{ counts }`, `DepList{ deps }`, `CycleList{ cycles }` — so its wire value is
`{"issues":[…]}` / `{"counts":[…]}` / `{"deps":[…]}` / `{"cycles":[…]}`, never `[…]`. This is the ONE place
where materializing DOES change wire bytes (the `query`/`dep`/`issue` list results were bare arrays before) — a
deliberate structural fix, wire-visible, so it moves `CONTRACT_HASH` and forces a `CONTRACT_VERSION` bump
(§5.4, jointly with §5.2a). The scalar/object arms (`IdOnly`, `Issue`, `CloseOutcome`, `DeletePlanOutput`,
`DepAdded`/`DepRemoved`, `DepTree`, `SyncOutput`, `DiagnosticReport`) already serialize as objects and are
UNCHANGED; the enums stay `#[serde(untagged)]` (each arm remains transparent — only a wrapped arm's OWN value
shape changes). The §5.4 resource read bodies (`unblock://issues/ready`/`blocked` → `Vec<Issue>`) are NOT
affected: a resource read returns TEXT content (`ReadResourceResult.contents[].text`), not `structuredContent`,
so a bare-array JSON string there is spec-legal. Enforced by an assertion that each tool's live
`structuredContent` is a JSON object.

```rust
// issue — the 5 success shapes of the 8 actions:
#[derive(Serialize, JsonSchema)] #[serde(untagged)]
pub enum IssueOutput {
    Id(IdOnly),                        // quick-create
    Issue(Box<Issue>),                 // create / show / reopen / restore
    Issues(IssueList),                 // multi-id update; ALSO create_bulk (D22 — N); {"issues":[…]} object-wrap (CD-2)
    Close(Box<CloseOutcome>),          // close — suggest_next -> newly_unblocked (FR-11)
    Delete(DeletePlanOutput),          // the resolved delete plan (was the ad-hoc delete_plan_json)
}
#[derive(Serialize, JsonSchema)] pub struct IdOnly { pub id: String }
#[derive(Serialize, JsonSchema)] pub struct IssueList { pub issues: Vec<Issue> }   // CD-2 object-wrap: {"issues":[…]} (shared by IssueOutput + QueryOutput)
#[derive(Serialize, JsonSchema)]
pub struct DeletePlanOutput { pub mode: DeleteModeOutput, pub targets: Vec<String>, pub cascade_children: Vec<String> }
#[derive(Serialize, JsonSchema)] #[serde(rename_all = "snake_case")]
pub enum DeleteModeOutput { Tombstone, Cascade, Hard, DryRun }   // From<DeleteMode>; wire == the old strings

// claim / defer — output = Issue (no union needed).

// query:
#[derive(Serialize, JsonSchema)] #[serde(untagged)]
pub enum QueryOutput { Issues(IssueList), Counts(CountList) }  // Issues = list/ready/blocked/search/stale; CD-2 object-wrap
#[derive(Serialize, JsonSchema)] pub struct CountList { pub counts: Vec<CountBucket> }   // CD-2 object-wrap: {"counts":[…]}

// dep:
#[derive(Serialize, JsonSchema)] #[serde(untagged)]
pub enum DepOutput {
    Added(DepAdded),                   // {"added":true}   (was ad-hoc json!)
    Removed(DepRemoved),               // {"removed":true} (was ad-hoc json!)
    Deps(DepList),                     // {"deps":[…]} object-wrap (CD-2)
    Tree(DepTree),                     // tree AND graph (Session::dependency_graph returns DepTree)
    Cycles(CycleList),                 // {"cycles":[…]} object-wrap (CD-2); ordered cycle-path witnesses (§3.2.1, D19)
}
#[derive(Serialize, JsonSchema)] pub struct DepAdded { pub added: bool }
#[derive(Serialize, JsonSchema)] pub struct DepRemoved { pub removed: bool }
#[derive(Serialize, JsonSchema)] pub struct DepList { pub deps: Vec<Dependency> }        // CD-2 object-wrap: {"deps":[…]}
#[derive(Serialize, JsonSchema)] pub struct CycleList { pub cycles: Vec<Vec<String>> }   // CD-2 object-wrap: {"cycles":[…]}

// comment (FR-6, D37; lands T3.9) — one scalar arm (add/update/delete return the single affected Comment,
// like IssueOutput::Issue covers create/show/reopen/restore) + one CD-2 object-wrapped list arm. `Comment` is
// small (no Box needed). Redact wire form = the returned Comment with `redacted_at` present + `"text":""` (NO
// extra top-level "redacted" bool — presence is the flag, mirroring the tombstone). `Comment` joins the
// `unblock_model::{…}` import.
#[derive(Serialize, JsonSchema)] #[serde(untagged)]
pub enum CommentOutput {
    Comment(Comment),            // add / update / delete(redact) — the single affected comment
    Comments(CommentList),       // list — {"comments":[…]} object-wrap (CD-2, never a bare array)
}
#[derive(Serialize, JsonSchema)] pub struct CommentList { pub comments: Vec<Comment> }   // CD-2 object-wrap: {"comments":[…]}

// sync — output = SyncOutput (G-23a): mcp-owned wrapper over the two model report DTOs.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutput { Export(ExportReport), Import(ImportReport) }

// diagnostics — output = DiagnosticReport (§1.10). v1 per-kind findings are ADVISORY generic
// DiagnosticFinding{label,detail} rows (D26/OQ-2): stats/lint/changelog/orphans/dangling express every
// counter/warning/entry as {label,detail}, so the taxonomy enrichment stays inside the existing
// schema (NO CONTRACT_VERSION bump). D45's `dangling` kind reuses that generic ROW shape unchanged
// (label = the dependent issue id, detail = "<dep_type> -> <missing target id>", §3.2.1) — but it DOES
// add an enum member to `DiagnosticKind`, which is a hashed bundle byte, hence the §5.4 D45 bump. A richer/nested per-kind DTO is a v1.1 structure seam — it
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
    // 8 × per-tool {input, output} (§5.1 order — D37 added `comment` LAST, after `diagnostics`).
    // Outputs (§5.3): issue=IssueOutput, claim=Issue, defer=Issue, query=QueryOutput, dep=DepOutput,
    // sync=SyncOutput, diagnostics=DiagnosticReport, comment=CommentOutput.
    // NORMATIVE — the `comment` field POSITION is hash-visible: `SchemaBundle` is a struct (not a map), so
    // serde emits its fields in DECLARATION order and `CONTRACT_HASH` digests those bytes. Declaring
    // `comment` anywhere other than last reorders the serialized document and moves the hash for a reason
    // unrelated to the new tool. It must be declared last, matching the §5.1 tool order and the
    // `agents_digest` pairs array that walks this struct field-by-field (D33).
    pub issue: ToolSchemas,
    pub claim: ToolSchemas,
    pub defer: ToolSchemas,
    pub query: ToolSchemas,
    pub dep: ToolSchemas,
    pub sync: ToolSchemas,
    pub diagnostics: ToolSchemas,
    pub comment: ToolSchemas,   // D37 (FR-6) — the 8th tool; LAST (hash-visible position, see above)
    // D25 — the shared in-band error output every tool may return with `is_error = true` (FR-11):
    // schema_for!(StructuredError), published ONCE (the rmcp `is_error` flag is the channel
    // discriminator — folding it into each per-tool union would misstate the discriminator and
    // duplicate one shape 8 times). Transitively via $defs the bundle pins IdOnly, the CD-2 list
    // wrappers (IssueList/CountList/DepList/CycleList), SyncOutput, ExportReport/ImportReport,
    // CountBucket, CloseOutcome, DepTree, Dependency, DiagnosticReport, StructuredError, and Issue —
    // so the resource payloads above are pinned too.
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

**MCP-conformance wire change (2026-07-09 — the gate firing as designed).** Two live-probe conformance
drifts against strict MCP clients are fixed spine-first: the §5.2a input-root-type injection (CD-1) AND the
§5.3 output object-wrap (CD-2). Each changes the bytes of the schema-bundle tool schemas (`schema_for!` of
the six tagged-enum inputs / of `IssueOutput`/`QueryOutput`/`DepOutput`), so BOTH move `CONTRACT_HASH`. They
are delivered JOINTLY as ONE bump — `CONTRACT_VERSION` `unblock.mcp.v1.2` → `unblock.mcp.v1.3` (unblock-mcp
`options.rs`) — with `CONTRACT_HASH` re-pinned and the `schema_bundle` + `capabilities` goldens re-blessed
(splitting them would force a wasteful `v1.2→v1.3→v1.4` double bump + double re-pin). This is the FR-12 drift
gate proving itself again: an intentional schema change moves the digest by design. *(Spec-first: this clause
lands first; the code — the `extend` attributes, the wrapper structs, the version bump/re-pin/re-bless, and
the strengthened conformance tests — follows in the paired implementation change.)*

**T3.5 `RateLimited` mint (2026-07-13 — D34/F-3, the gate firing as designed).** T3.5 adds
`ErrorCode::RateLimited` (the MCP-handler concurrency-cap reject). Because the version-coupled set is exactly
the two discovery documents + their transitive `$defs` closure, this changes BOTH: `capabilities().error_codes`
gains one `ErrorCodeDescriptor`, and `schema_bundle()`'s shared `error` schema (`schema_for!(StructuredError)`)
gains an `ErrorCode` enum variant. So `CONTRACT_HASH` re-pins and `CONTRACT_VERSION` bumps
`unblock.mcp.v1.3` → `unblock.mcp.v1.4` (unblock-mcp `options.rs`), with both discovery goldens + the
`unblock-error` `exit_code_table.rs` quadruple golden re-blessed. Spec-first: this clause lands first; the code
follows in the paired implementation change (P2).

**D37 comment surface (2026-07-16 — the gate firing as designed; lands T3.9).** D37 pulls the comment surface
into v1 (§1.6/§1.7/§3.2/§4.1/§5.1/§5.2/§5.3). The version-coupled set moves on TWO axes: (1) the NEW dedicated
`comment` tool adds a per-tool `{input,output}` pair (`schema_for!(CommentToolInput)` / `schema_for!(CommentOutput)`)
to `schema_bundle()` and an 8th tool descriptor to `capabilities()`; (2) the `$defs/Comment` embedded inside the
EXISTING `issue`/`query` OUTPUT schemas gains 2 properties (`updated_at`/`redacted_at`) — so EXISTING schema bytes
move too (not merely a new pair). Both re-pin `CONTRACT_HASH` and bump `CONTRACT_VERSION`
`unblock.mcp.v1.4` → `unblock.mcp.v1.5` (unblock-mcp `options.rs`), with the `capabilities`/`schema_bundle`
goldens + the D33 `agents_digest()` ripple re-blessed (`list_tools` is a LIVE assert, not a golden) and the
`contract_suite` re-run. `content_hash`
is UNAFFECTED (§1.8) → FR-26 idempotency intact. Spec-first: this D37 clause + the §1.6/§1.7/§3.2/§4.1/§5.x design
land FIRST (the docs-only cascade); the code + the version-string flip + the goldens re-bless follow at **T3.9**.

**D42 (v1.0.1) — the contract bumps to `unblock.mcp.v1.6`.** A SIBLING entry appended to this ledger; the D37
clause above records what GA shipped and is NOT renumbered. `#[serde(deny_unknown_fields)]` on all 13 input
containers makes schemars emit `additionalProperties: false` per `oneOf` arm AND inline the tagged-enum newtype
variant (`IssueInput::Create`) instead of `$ref`-ing `$defs/CreateInput`; the `create_bulk` doc-comment and the
`markdown` field description are rewritten to publish the closed section-name set, the code-fence grammar,
and ALL **FIVE** `create_bulk` rejections — including the UNTERMINATED-FENCE rejection, which rejects
documents GA v1.0.0 ACCEPTED (D42 clause 4(iii)), so the break is discoverable from `tools/list` alone.
All of that moves `schema_bundle()` bytes → `CONTRACT_HASH` re-pinned + the `schema_bundle` golden re-blessed.
`capabilities()` moves only by its `contract_version` field (a FIELD description is not a TOOL description).
Per D35 an additive `.M` bump inside 1.x is NON-breaking, so this ships in a PATCH release. D42 adds **no**
public API surface.

**D43 (v1.0.1) — the contract does NOT bump; `unblock.mcp.v1.6` stands.** A SIBLING entry to the
D42 clause above, recorded explicitly so a future reader does not infer that every defect fix bumps.
D43 rejects a DUPLICATE JSON KEY anywhere inside the `tools/call` `params` subtree, but it mints **no**
`ErrorCode` and moves **no** schema byte: the duplicate-key kind rides `context.kind` inside the
already-published `StructuredError` shape, and `context` is a free-form map. `ErrorCode::ALL` is
therefore unchanged, so `capabilities().error_codes` is unchanged, so **`CONTRACT_HASH` is NOT
re-pinned** and neither golden is re-blessed. Minting a dedicated code was considered and rejected:
under the D35 GA freeze it would be a BREAKING contract change shipped in a patch release, for a
defect fix. The cost is that the class is filterable only by `context.kind == "duplicate_key"` — a
v1.1 candidate, disclosed rather than hidden. D43 adds **no** public API surface (the public
`run_mcp_server(Arc<Session>, McpServerOptions)` signature is unchanged; the byte-level bound change is
confined to a private function and two `test-util` helpers).

**D44 (v1.0.1) — the contract bumps to `unblock.mcp.v1.7`.** A SIBLING entry appended to this ledger; the
D42 and D43 clauses above record what shipped before and are NOT renumbered. D44 makes the single
`issue create {deps:[…]}` path atomic and correctly anchored (§3.2.1, §4.1) and, on the wire, relaxes
`$defs/DepInput.issue_id` from REQUIRED to OPTIONAL with a rewritten description saying it MUST be omitted
on the create arm. `$defs/DepInput` is `$ref`-ed from exactly ONE site — the `issue` tool's `create.deps`
array items — so the `dep` tool's published schema does NOT move; but a `required` list and a property
description are both schema bytes, so `schema_bundle()` moves → `CONTRACT_HASH` re-pinned + the
`schema_bundle` golden re-blessed. `capabilities()` moves only by its `contract_version` field (a FIELD
description is not a TOOL description). `agents_digest()` is a pure derived view (D33, below), and the managed `AGENTS.md` capabilities table is
BYTE-IDENTICAL after D44 — it does NOT regenerate. `effective_params`
(`crates/unblock-mcp/src/resources/agents_digest.rs:232-253`) merges a node's own
`properties`/`required[]` with the arm-ROOT `$ref` only; a PROPERTY-level `$ref` — which is what
`create.deps`'s items are — is never resolved, only the property KEY is collected (its own doc-comment
says so at `:224-228`). So the digest never descends into `$defs`, `AGENTS.md:32` already lists `deps`
as an OPTIONAL param of `issue create` and has never published `DepInput`'s internal fields, and D44
moves only `$defs/DepInput`'s requiredness. The ONE `AGENTS.md` byte that moves is the contract line
(`AGENTS.md:8`), via `AgentsDigest.contract_version` — derived, therefore NOT an extra version event. D44 mints **no** `ErrorCode` (the rejection rides the
existing `VALIDATION_FAILED`; the storage-side guards reuse `SelfDependency`/`DuplicateDependency`/
`CycleDetected`), so `ErrorCode::ALL`, `capabilities().error_codes` and the 0–8 exit-code table are all
unchanged. D44 adds **no** public API surface: `Storage`'s signatures are untouched — the edges ride
`Issue.dependencies`, which `Storage::create_issue` already persists — so no `impl Storage` block gains,
loses or re-types a method and every implementor's METHOD SET is unchanged. That is a claim about
SURFACES only: the shipped libsql `create_issue` BODY (`crud.rs:31-52`) does gain the create-specific
guards, and the engine's `RaceInjector` test double gains an `add_dependency` counter
(`crates/unblock-engine/tests/common/mod.rs:899-900`); the
engine-side change is the new engine-owned `NewDep` plus `NewIssue.deps`'s element type, both inside
unpublished workspace-internal crates. Per D35 an additive `.M` bump inside 1.x is NON-breaking, so this
ships in a PATCH release — the same reasoning D42 clause 4(iii) used to ship a fence-aware `create_bulk`
rejection of documents GA v1.0.0 accepted. **The ratified behavioural change is stated openly:** a create
whose `deps[].issue_id` names a third party now returns `VALIDATION_FAILED` instead of silently rewriting
that party's dependency graph, and one whose `deps[].issue_id` names a non-existent id now returns
`VALIDATION_FAILED` with NOTHING persisted instead of a foreign-key error with a committed orphan.

**D45 (v1.0.1) — the contract bumps to `unblock.mcp.v1.8`.** A SIBLING entry appended to this ledger; the
D42, D43 and D44 clauses above record what shipped before and are NOT renumbered. Neither the D44 version
nor this bump has been released, so the cost is the re-pin and the goldens, not a client migration. D45
guards the dependency TARGET on every edge-writing path (§3.2.1), defines the `external:` predicate once in
`unblock-model` (§1.9), widens the export corpus to the transitive closure of its blockers (§1.10), and adds a `dangling` diagnostics kind
(§1.10 / §5.2 / §3.2.1). **What moves:** (1) `schema_bundle()` moves on TWO axes — the `diagnostics` tool
INPUT gains a `oneOf` arm (`{"kind":"dangling"}`) and the `diagnostics` tool OUTPUT's
`$defs/DiagnosticKind` gains an enum member, so EXISTING schema bytes move, not merely a new arm;
(2) `capabilities()` moves by more than `contract_version` this time — the `diagnostics` TOOL DESCRIPTION
is rewritten to name the new kind, and a tool description IS version-coupled in its capabilities-document
copy. So `CONTRACT_HASH` is re-pinned and `CONTRACT_VERSION` bumps to `unblock.mcp.v1.8` (unblock-mcp
`options.rs`), with the `capabilities` and `schema_bundle` goldens re-blessed, the live
`(name, description)` tool assert in `contract_suite.rs` updated (an assert, not a golden), and the
corresponding `unblock-model` `DiagnosticReport`/`DiagnosticKind` schema golden re-blessed.
**`agents_digest()` DOES move here — unlike D44.** It is a pure derived view outside the version-coupled
set, but it walks each tool's `oneOf` arms to publish actions and parameters (D33), so the managed
`AGENTS.md` capabilities table gains a `dangling` action row in addition to the derived contract line and
the rewritten tool description. `unblock agents` must be re-run and the regenerated `AGENTS.md` must land
in the SAME commit; D44's "BYTE-IDENTICAL after the change, it does NOT regenerate" claim is specific to
D44 and does not carry over. D45 mints **no** `ErrorCode` (the guard rides the existing `ISSUE_NOT_FOUND`
via the new internal `StorageError::BlockerNotFound`, §3.1), so `ErrorCode::ALL` stays at 36,
`capabilities().error_codes` is unchanged and the 0–8 exit-code table (§2.3) is untouched. D45 adds **no**
MCP tool — the new surface is a KIND arm on the existing `diagnostics` tool, so the RK-3 budget (§6.6)
stands at 8 ≤ 8, FULL and unmoved. D45 adds **no** public API surface: every `Storage` trait signature is
unchanged (the batch id set is an internal `pub(super)` parameter of the shared insert body),
`unblock_health::run_doctor`'s signature is unchanged (D29 clause F3 — pure, non-async, storage-free —
STANDS, the DB-derived findings being composed in the ENGINE instead, §4.1), and `unblock_model` gains one
`const` + one `fn` in a workspace-internal crate. Per D35 an additive `.M` bump inside 1.x is
NON-breaking, so this ships in a PATCH release. **The ratified behavioural changes are stated openly, not
footnoted:** (i) a create, a `dep add`, a reparent or a JSONL/`bd` import naming a blocker
that does not exist now returns `ISSUE_NOT_FOUND` with NOTHING persisted, instead of `isError:false` and a
permanently unresolvable blocker. **`issue create_bulk` is DELIBERATELY ABSENT from (i), and the omission
is the accurate statement:** that path ALREADY refuses an unknown reference today, whole-batch, with
`ValidationFailed` from the L5 resolver (`crates/unblock-engine/src/session/bulk.rs:378-388` — "no batch or
storage match"), which is exactly the batch-aware predicate (batch set ∪ storage) D45 generalises to every
path; nothing is persisted there today either. `create_bulk` is therefore D45's TEMPLATE, not a hole. What
D45 changes there is (a) the §3.2.1 guard closes the TOCTOU race between that PRE-transaction probe
(`crates/unblock-engine/src/session/write.rs:497-522`) and the commit, and (b) change (ii) below relaxes
its one real defect. **Its user-visible rejection for a genuinely unknown id STAYS `ValidationFailed`** —
the resolver runs first and is unchanged for that case (the "one divergence" paragraph in §3.1 and §5.2
rejection-set item (b) say so); publishing `ISSUE_NOT_FOUND` there would name a code the path cannot
return, and the acceptance cell written from it would be unwritable without a race harness;
(ii) `issue create_bulk` now ACCEPTS a correctly-spelled `external:`
dependency reference, which it rejects today (whole-batch) — a RELAXATION of a normatively-pinned
rejection on a GA-shipped path, covered by no test today, so no test will go red to announce it;
(iii) an `external:` target is recognised case-INSENSITIVELY everywhere (§1.9), so `EXTERNAL:jira-1` in a
bulk `### Dependencies` section is now an external ref rather than an ordinary id — delivered by the
RESOLVER carve-out (§5.2 item (b)), not by the parser swap, which is observationally a no-op (§1.9
invariant 5); (iv) `sync export` now WRITES ephemeral / `-wisp-` rows standing in a non-external dependency relation with a kept row IN EITHER DIRECTION — not merely the ones a kept row depends on, since the `parent-child` edge lives on the CHILD while the blocked-set query blocks the epic PARENT through it (the
blocker closure, §1.10), so an export file may carry lines it never carried before — and an export of a
workspace that ALREADY holds a dangling edge produces a file the guarded import REFUSES whole-batch,
naming the first offending `(dependent, target)` pair. The exporter drops nothing and repairs nothing;
(v) a foreign `bd`/JSONL file carrying a dangling edge is now REJECTED whole-batch
rather than imported (`bd_import`'s repairs do not drop such an edge). **Item (v) is DECIDED — REJECT, not
repair — on a stated principle rather than by omission: the EXPORTER may WIDEN its corpus (it is closing a
file it owns); the IMPORTER may never INVENT one (it is ingesting a claim it cannot verify).** Repair would
put a silent edge-dropper on an INGEST path, which is the same silence D45 exists to close. The accepted
cost: a one-shot `bd` migration can now fail whole-batch on data the user cannot edit inside unblock, with
no `--repair` escape in this cut — which is why the refusal MUST name the first offending
`(dependent, target)` pair (both ids are already in the variant, §3.1), so the source file is repairable
from the message alone. `sync import {dry_run:true}` still reports a clean plan for such a file, because
the dry-run arm returns before `create_issues` (`crates/unblock-sync/src/import.rs:265-274` vs `:279`); the
divergence is ACCEPTED and disclosed for this cut, with the `dangling` action as the real pre-flight;
(vi) `dep {action:"add"}` now guards its edge SOURCE as well, returning the EXISTING
`ISSUE_NOT_FOUND` (exit 3) where GA returned an opaque `DATABASE_ERROR` (exit 2) from the source-column
foreign key — the asymmetry a single typo could otherwise expose, closed in the same cut (§3.2.1
`add_dependency`); and (vii) on `add_dependency`, re-adding an ALREADY-PRESENT edge whose target is
dangling now returns `ISSUE_NOT_FOUND` where GA returned `VALIDATION_FAILED`/`DuplicateDependency`,
because the target check is ranked before the duplicate query so ONE precedence chain describes every path
(§3.2.1 RANK bullet). It is reachable only on already-corrupt data. **The guard newly refuses NON-GATING
edges too:** a `dep add` of a `related` (or any of the 11 named types plus `Custom`) edge to a
non-existent id is a NEW rejection, not only the `blocks` family — stated here because the class is
introduced as a never-ready defect, and most edge types do not gate ready work at all.

**`agents_digest()` — a pure DERIVED VIEW, not a wire resource (T3.4.3/D33).** `unblock-mcp` additionally
exposes `pub fn agents_digest() -> AgentsDigest` next to `schema_bundle()`: a CLI-friendly typed digest (the
8 tools with their `oneOf`-derived actions + each action's FULL parameter surface — its required AND
optional params, derived structurally from the `oneOf` arm and resolving an arm-root `$ref` one level for
the delegated payload, e.g. `issue create` → `title` + its optional fields — the 5 resources, the 3
prompts, and the `error_codes` map) computed STRUCTURALLY from `capabilities()` + `schema_bundle()`. It is
NOT an `unblock://` resource and carries NO URI; it is consumed only by the CLI `unblock agents` command to
render the managed `AGENTS.md` capabilities table (FR-14). Being a pure derived view over the two hashed
discovery documents, it is drift-free BY CONSTRUCTION and is explicitly OUTSIDE the `CONTRACT_HASH`
version-coupled set (like the §5.5 golden-only prompt snapshots) — adding or changing it does NOT bump
`CONTRACT_VERSION`.

### 5.5 Prompts

```
triage                 -> guided triage workflow
plan_next_work         -> drives ready -> claim selection
close_with_suggestions -> close + surface newly-unblocked
```

### 5.6 Error mapping at the MCP boundary

Any `EngineError` → `StructuredError` (§2.4) attached as rmcp tool error **data** (`code`/`message`/`hint`/`retryable`/`context`), parallel to the CLI 0–8 exit codes. A failed tool call still returns **valid JSON** (the shared in-band error output — `SchemaBundle.error`, `is_error=true`; §5.3/§5.4 D25). **Argument-boundary contract (NORMATIVE, D42).** `schemars` is **codegen-only** — it publishes the `inputSchema` and there is **no runtime validator anywhere in the process**. Enforcement is **three** explicit steps: **(0) a byte-level DUPLICATE-KEY SCAN inside an OWNED `Transport<RoleServer>` (D43), BEFORE `serde_json::from_slice`** — a duplicated key is collapsed last-wins while the frame is decoded, so no layer downstream of the parse can observe it, and neither a `Transport` DECORATOR (`receive()` hands it an already-parsed message) nor the argument extractor (which receives an already-deduplicated `JsonObject`) is a possible home. The verdict is carried in `rmcp::model::Extensions`, which is **wire-unforgeable**: it has no `Serialize`/`Deserialize` impl at all, so no wire field can name it. **The scan is scoped to the WHOLE `params` value of every decoded request, `_meta` INCLUDED — NOT to `params.arguments` alone.** The scan itself is UNIVERSAL (`resources/*`, `prompts/*` included — it runs on the raw bytes before rmcp classifies the method), but the **verdict is consulted at exactly ONE site, `call_tool`**, because only `tools/call` has an in-band channel; gating another method would have to answer out-of-band and reopen the `-32602` arm this contract exists to keep shut. Non-`tools/call` methods therefore carry a stamped-but-unenforced verdict. **The verdict rule is FAIL-CLOSED: absent OR indeterminate ⇒ REJECT.** An empty `Extensions` is the DEFAULT state, so encoding "absent ⇒ clean" would make any path reaching a handler without traversing the scanning transport fail OPEN. The one documented verdict-less path is TEST-ONLY: `mcp_server_duplex_unclamped_for_test`, the CD-6 raw-rmcp pin, which deliberately installs no scan and whose calls the fail-closed arm refuses. **(1)** `enforce_quota` over the whole `tools/call` `params`, once, pre-dispatch, inside the rate-limit permit; **(2)** a typed parse under `#[serde(deny_unknown_fields)]` inside the tool body, reached via a crate-local DEFERRING `Parameters<T>` extractor whose `FromContextPart` impl is infallible — which is what makes rmcp's out-of-band `invalid_params` arm structurally unreachable for our 8 tools. Oversized, unknown-field and malformed **arguments** are therefore rejected **in-band** (`isError:true` + the FR-11 `StructuredError`, `retryable: true`) before reaching the engine; blast radius confined to the workspace. **A duplicate JSON key inside `params` (anywhere, `_meta` included) is NOT a member of the scoped list below** — it is rejected IN-BAND by step (0)/`call_tool` (D43). Note also that duplicated ENVELOPE fields are not caught by serde duplicate-field detection, contrary to a natural reading: `params.name`/`params.arguments` are reached through rmcp's `#[serde(flatten)]`, so the typed variant merely FAILS and the untagged fallback wins, yielding `CustomRequest` → `-32601`; only `jsonrpc`, `params`, `method` and `params._meta` (the KEY itself) hard-fail to `-32700`. **SCOPED — these stay out-of-band protocol faults, because no seam under our control reaches them:** an unknown tool name, a **non-object `arguments`** (rmcp fails to deserialize `CallToolRequestParams` itself, so `call_tool` is never entered), and a present **`params.task`** (rejected by rmcp's default `TaskSupport::Forbidden` before `call_tool`). `ErrorData` remains reserved for exactly that class.

**Rate-limit chokepoint (NFR-18 / D34-F5 — normative).** The `Quotas.max_concurrent_requests` cap is enforced
by a single `Arc<Semaphore>` field on `UnblockServer` (built ONCE in `new`; the type derives `Clone`, so the
field is `Arc`), the permit acquired around the tool-router dispatch AND the `read_resource` path — gating the
tool + resource surface (and any future tool by construction). `try_acquire` failure → for a **tool** call, an
**in-band** `CallToolResult` carrying `StructuredError { code: RateLimited, retryable: true }`; for a
**resource** read (which has no in-band channel — `read_resource` returns `Result<_, ErrorData>`, `server.rs:220-232`),
an **`ErrorData`** (JSON-RPC error) carrying the same `RateLimited` code. Exit 2 (§2.3); never dropped or
backpressured (fast-fail is deterministic and bounds a pipelining agent immediately). The rate-limit
`Semaphore(64)` sits STRICTLY ABOVE the engine write `Semaphore(1)` + the `.write.lock` (§4.2) — different
semaphores, strict ordering, deadlock-free vs D14/D31. Prompts are excluded (pure builders, no `Session`). The
per-request SIZE caps stay `enforce_quota` checks — run ONCE in `call_tool` over the WHOLE `tools/call` `params` (`name` + `arguments` + `_meta` + `task`), inside the rate-limit permit and before dispatch (D42). NOTE this bounds what a request may DO, not the parsing work an oversized message costs: rmcp deserializes the whole message off the transport before any handler runs — mapping to `ValidationFailed` (a DISTINCT
mechanism).

---

## 5b. CLI lifecycle surface — `unblock-cli` (L7)

`unblock-cli` owns the `unblock` binary and depends on `unblock-mcp` (§0.1). Lifecycle/ops commands (NOT the issue-data verbs, which go through MCP tools / the engine): the v1 command set is **`mcp, migrate, doctor, version, init, agents, update`** — all lifecycle/ops. (This widens the PRD D3 list, which named only `mcp/migrate/doctor/version`; `init`/`agents`/`update` ship in cli at M3 per the cli plan / T3.1 / T3.6.) The T3.1 command behaviours below are ratified by **D27 (PRD §4)** and reconciled spine-first against the live surface on `main` @ b384103.

```rust
// commands/update.rs — the v1 self-update command (FR-25 / D17). Command token is `unblock update`
// EVERYWHERE (Command::Update, UpdateArgs, help snapshots). The Cargo FEATURE is named "self-update"
// (the "self-update" feature enables the "unblock update" command — feature name ≠ command name by design).
pub struct UpdateArgs { /* --check, --version <tag>, --yes */ }
```

**The CLI is a pure `CliOverrides` forwarder (D27/AD-3).** `unblock-config` owns ALL layering (CLI > env `UNBLOCK_*` > `.unblock/config.toml` > defaults), `.unblock/` discovery, path confinement, and prefix normalization; the CLI does NOT re-implement precedence. The single CLI-owned resolution seam is **clap `env`**: `--dir`→`UNBLOCK_DIR` and `--actor`→`UNBLOCK_ACTOR` bind via clap `env` (so `--flag > UNBLOCK_*` is free) and `GlobalArgs::to_overrides()` is the ONE place clap types cross into `CliOverrides`. `UNBLOCK_OUTPUT_FORMAT` is parsed strictly by config's env layer (the single strict parse site); the `--output/-o` flag forwards via `CliOverrides.output_format` so `--flag > env` still holds inside config's resolver. `CliOverrides` has NO `id_prefix` field, so `init --prefix` is NOT forwarded — it is written into the scaffold `config.toml` text (see `init`).

**mcp (FR-20 / D27/AD-4).** Opens a `WorkspaceContext` via `open_with_storage_with_cli`, builds a `SessionConfig { jsonl_export: ctx.config.jsonl_export, import_on_open: false, remote: false }` (`import_on_open` MUST stay false in v1 — `true` returns `FeatureNotWired{"sync"}`, exit 1), installs the FR-17 shutdown handle, opens the `Session` + `with_shutdown_flag`, then calls the LIVE **2-arg** `unblock_mcp::run_mcp_server(Arc<Session>, McpServerOptions { cancel, quotas: Quotas::default(), instructions })` (§0.1 — transport is internal `stdio()`; since D43 that internal transport is the duplicate-key-scanning one, and the CLI contract is unchanged). On EOF/first signal the `CancellationToken` cancels; `run_mcp_server` then returns **`Ok(())` (handshake already complete) or `Err(McpServerError::Transport{Cancelled})` (cancel landed DURING the rmcp `initialize` handshake — §0.1)**. **Both** are normal cooperative-shutdown outcomes and **both** MUST run `session.shutdown()` (drain the permit, clean libsql close — FR-17); an `Err(Cancelled)` never skips the clean libsql close.

- **The signal wins, on the cooperative-shutdown returns (NORMATIVE — D38).** The CLI consults the FR-17 handle's recorded signal (`signal_exit_code()`) **BEFORE** propagating a `run_mcp_server` **or** a `session.shutdown()` error. If a signal was recorded → the command yields **`128+signo`** even when the return is `Err`; that error is reported by `commands/mcp.rs` itself as a **diagnostic only** — never deciding the exit code: a teardown error observed after a signal is a consequence of the cancellation, not an independent fault. **Labelling (NORMATIVE — D38 clause (1a)):** a reported error is routed by CLASS — the cancellation class (`McpServerError::is_cancellation()`, matching **only** `Transport{Cancelled}`) is recorded via `tracing::debug!` (silent at the default level, surfaced by `-vv`), while a GENUINE error keeps its human `error[CODE]: message` **stderr** line (NFR-14). Neither branch may DROP the error. **The same reporting duty binds the UNSIGNALLED path:** if run loop AND teardown both fail, the run-loop error decides the code and the **displaced teardown error is still reported** (`run.and(teardown)` dropped it — a swallowing bug). **Delivery (RATIFIED — D38 design gate, not open):** `mcp::run` returns **`Ok(Some(128+signo))`** and `run_with` (§5.1) casts it straight to `ExitCode`; there is **NO** in-command `std::process::exit` on the first-signal path. **Scope:** the precedence binds those two returns ONLY — a failure raised BEFORE the run loop starts (e.g. `Session::open`) is not a consequence of the cancellation and still casts through the §2.3 0–8 table, so a signal cannot mask an unrelated DB fault. With **no** signal recorded, a genuine `Err` keeps `ErrorCode::InternalError` → **exit 1** (the D27/AF-4 map below) — **EXCEPT the unsignalled pre-`initialize` client disconnect (NORMATIVE — D40):** an `Err(McpServerError::Transport{ServerInitializeError::ConnectionClosed(_)})` (the peer closed the connection before completing the `initialize` handshake) is a routine lifecycle event, so `resolve_mcp_exit` intercepts it via the additive `McpServerError::is_pre_handshake_disconnect()` predicate (matching **only** `Transport{ConnectionClosed(_)}`, mirroring `is_cancellation()`) and **delegates the exit code to the `session.shutdown()` teardown** — a clean teardown → `Ok(None)` (exit 0), a failing teardown still decides via its own 0–8 code (a libsql-close fault → exit 8 is NEVER masked into exit 0). The disconnect is reported as a `tracing::debug!` diagnostic (surfaced by `-vv`), and `diagnostic_route` demotes `ConnectionClosed` exactly as it demotes `Cancelled`; nothing is swallowed. This unifies with the already-blessed post-handshake EOF → exit 0. **FR-11 scope:** on the signal path the `Ok(Some(_))` arm bypasses `exit::into_exit` entirely, and mcp stdout is MCP-framing only, so FR-11's always-valid-JSON-on-stdout rule binds only the **unsignalled** genuine-`Err` path (which excludes the D40 disconnect → exit 0).
- **Second-signal escalation (NORMATIVE — D27/AD-4, D38).** A second signal → `std::process::exit(128+signo)` (no unsafe; signal-hook delivers on a normal thread); Windows is a cfg no-op. Until D38 this branch was **load-bearing** (it was the only exit for a signal delivered before the handshake completed); after the no-hang fix it is a **backstop**, not the primary exit mechanism.
- **No-hang invariant (NORMATIVE — D38, all paths).** **No** return path of the `mcp` command — `Ok`, `Err`, signal-exit, **or a panic unwind** — may fall through to a drop of the tokio runtime while the stdin blocking-pool read is parked: `Runtime::drop` → `BlockingPool::shutdown` blocks forever on it. Every exit path must dispose of the runtime non-blockingly or otherwise guarantee termination. This binds the **error** path, not only the signal-success path, and — since `panic = "unwind"` — the **panic** path too: a `shutdown_background()` placed after `block_on` is dead code on an unwind, so the unwind half needs its own guard (`ManuallyDrop`, whose `Drop` is a no-op). **Measured scope (T3.2.1 Verify gate):** the parked read — hence the hang — is what the **pre-handshake signal** window and any post-handshake signal produce; the **unsignalled run-loop `Err`** does NOT park a read (rmcp returns after a complete message without re-issuing `receive()`) and does NOT hang. The invariant still binds it (defensively), but its non-vacuity is carried by the signal cases. **The INVARIANT is normative; the delivery is the ratified mechanism:** the D38 design gate (2026-07-17) settled it as `main` owning the runtime explicitly (`Builder::new_multi_thread().enable_all()`, `build()`'s `Result` handled — no `unwrap`/`expect`) and calling `rt.shutdown_background()` (consumes `self` → the blocking `Drop` never runs) **after** `block_on` — `#[tokio::main]` expands the runtime to a temporary, so that blocking drop is otherwise structurally unavoidable. Nothing load-bearing is lost: rmcp flushes framing per-message before returning, `session.shutdown()` closes libsql inside `block_on`, tracing writes to `std::io::stderr` with no `non_blocking` guard to flush, and `exit.rs` renders synchronously. Rebinding stdin off the blocking pool is the true root fix but re-opens the §0.1 2-arg contract at GA → **v1.1 seam**; closing the fd needs `libc` → barred by `#![forbid(unsafe_code)]`. (§4.2 specifies cancel-safety of the write tx; this is the peer invariant for the process exit.)

stdout carries ONLY MCP framing (logging is stderr-only, NFR-14). **Supported topology = child-per-client, multiple MCP servers (D31)** — the D14 single-MCP-server-per-workspace clause is RETIRED (§4.2); cross-process writes serialize on the `.unblock/.write.lock`.

**migrate (D27/AF-2).** Opens the context (the facade already migrates on open), opens the `Session`, calls the NEW `Session::migrate() -> MigrateOutcome` (§4.1) under the write permit, builds a CLI-local `MigrateReport { database, schema_from, schema_to, applied }`, maps it onto a `DiagnosticReport { kind: Info, findings }` and emits via `Renderer::diagnostics`. Exit 0 on success; a newer-than-build DB → transparent `SchemaMismatch` → exit 2. Idempotent (`applied` normally `false` post-open).

**doctor (D27/AF-1 — doctor-LITE).** Opens the `Session` and composes `diagnostics(Stats|Lint|Info)` + the NEW `Session::integrity_check()` read (§4.1) into a CLI-local `DoctorReport`, mapped onto a `DiagnosticReport { kind: Info, findings }`. At T3.1 it does NOT call `Session::doctor()`/`recover()` (the `health` seam); at **T3.3 (HEALTH-LITE, D29/F4)** it ROUTES through the now-wired `Session::doctor()` (adding file-state anomalies). **Since D45 that same route also carries the dangling-dependency findings** — composed in the ENGINE (§4.1 `doctor()`; `unblock-health` is NOT touched, D29 clause F3 preserved), so the CLI report lists exactly what the `diagnostics {kind:"dangling"}` MCP action lists, in the same pinned order, with no second implementation. **Non-zero exit only on detected corruption:** a non-empty `integrity_check` → `ErrorCode::DatabaseError` (exit 2; §2.3 unchanged, no new code); Lint/orphan **and D45 dangling-dependency** findings are advisory (no exit flip); else exit 0. The advisory classification is deliberate: a dangling edge is a repairable DATA fact, not database corruption, and flipping the exit would change the mutation-pinned `doctor_exit` behaviour on a GA-frozen CLI surface in a patch release (D35). `--repair` + the full taxonomy land at **v1.1**.

**version (D27/AD-5).** Runs with NO workspace. Emits `VersionReport { version, build, commit: Option<_>, rustc: Option<_>, target: Option<_>, features }` from `build.rs`-emitted `option_env!("UNBLOCK_BUILD_*")` (absent = `None`) — NO git invocation / git crate / network / GitHub update-check (NFR-6/D13; the update-check lives only in `unblock update`). Rendered via the same to-`DiagnosticReport` path (kind `Version`).

**init (D27/AF-3).** Creates exactly `.unblock/config.toml` (hand-written TOML — `ProjectConfig` is `Deserialize`-only — seeded with the `unblock_model::normalize_prefix`-normalized `--prefix`, default `ub`; the CLI takes a direct `unblock-model` dep) + a migrated empty `unblock.db` opened through `open_with_storage_with_cli` (FR-9 no-drift). NO `.gitignore`/`metadata.json`/`issues.jsonl` (D13/NFR-6/model-B). **Clobber guard:** refuse if `config.toml` OR `unblock.db` is already present without `--force` → a CLI-local `CliError::AlreadyInitialized` → `ErrorCode::AlreadyInitialized` (exit 2; `ConfigError` has no such variant). Reports a CLI-local `InitReport`.

**agents (FR-14, D33).** A pure file op (SEPARATE from init): resolve-only open (`open_workspace_with_cli`, no DB) to find `workspace_dir`, then merge an idempotent managed AGENTS.md block (delimited markers) describing the MCP wiring (`unblock mcp`, stdio transport, tool set). The block is a FULL capabilities table rendered by the zero-arg `managed_block() -> String`, a THIN markdown renderer over `unblock_mcp::agents_digest()` (§5.4) — descriptor tables for the 8 tools / 5 resources / 3 prompts, per-tool actions with their FULL required+optional parameter surface, the error-code → exit-code/retryable map, and pointers to `unblock://schema` + `unblock://capabilities`. Writes a terse "wrote X" note to stderr.

**error boundary (D27/AF-4).** `exit.rs` owns the 0–8 cast (there is no `From<ExitCode> for std::process::ExitCode` in `unblock-error`). Transparent-`CodedError` sources (`EngineError`/`ConfigError`/, with AF-4, `RenderError`) bridge via `(&err).into()`. `McpServerError` (`Transport`/`RunLoop`, `#[non_exhaustive]`) is mapped EXPLICITLY to `ErrorCode::InternalError` (exit 1) — an MCP-server failure is internal, not a user IoError (NOT exit 8) — **absent a recorded signal: once `signal_exit_code()` is `Some`, the `mcp` command yields `128+signo` for its two cooperative-shutdown returns (`run_mcp_server`, `session.shutdown()`) and this 0–8 cast is NOT consulted for them — it IS still consulted for a pre-run-loop failure (e.g. `Session::open`), which a coinciding signal must not mask (D38; see the `mcp` paragraph above)**. **D40 carve-out:** even with NO signal, `resolve_mcp_exit` intercepts a `Transport{ServerInitializeError::ConnectionClosed(_)}` (the pre-`initialize` peer disconnect, `is_pre_handshake_disconnect()`) BEFORE this cast and delegates the code to the teardown → exit 0 on a clean `session.shutdown()`; a failing teardown still casts via its own 0–8 code (never masked). CLI-local variants: `AlreadyInitialized` (exit 2), scaffold/agents `Io` (exit 8). **NFR-14 + FR-11 split:** in json/robot the structured error renders to STDOUT (always valid JSON even on error); in plain a human `error[CODE]: message` line goes to STDERR.

**Self-update seam (FR-25, D17):** the `unblock update` command uses **`axoupdater` as a library dependency of `unblock-cli`** (NOT a separate `unblock-update` crate). axoupdater runs the dist installer, which verifies each artifact's **SHA256 checksum** before the swap (`self_replace`) (NFR-17), not an embedded key; GitHub artifact attestations are publish-side provenance (`gh attestation verify`), not on the client update path. Gated behind the **`self-update`** Cargo feature (default-on); `--no-default-features` drops the feature and thus the `unblock update` command and its network surface (CF-K).

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
MCP tool count stays ≤ 8; new domain surface extends existing tools by discriminator before adding tools (RK-3). D22's `create_bulk` is a NEW `action` arm on the existing `issue` tool (NOT a new tool) — the live `list_tools` golden (T2.3) keeps the count at 7. **D37 (the `comment` tool):** the dedicated `comment` tool (D-B) is the deliberate exception to "extend before add" — a distinct domain verb, not an `issue` arm — bringing the count to **8 ≤ 8** at T3.9; the RK-3 budget is now **FULL** and any further domain surface must extend an existing tool by discriminator. **D45 is the first surface to land under that FULL budget and it obeys the rule literally:** the dangling-dependency listing is a new `kind` arm on the existing `diagnostics` tool (§5.1 row 7 / §5.2), NOT a ninth tool — the count stays **8 ≤ 8** and the live `list_tools` assert does not move.

### 6.7 Safety / no-git / no-default-network
`forbid(unsafe_code)`, no git crate / `Command::new("git")` anywhere (NFR-6/NFR-9); network/TLS only behind the non-default `remote` feature (D15) AND the default-on `self-update` axoupdater path (FR-25/D17), which the D5 no-network source-scan whitelists (NFR-10 names both).
