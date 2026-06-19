# unblock-sync — File-level Plan

- **Status:** DRAFT (conforms to `docs/plans/01-design-spine.md` + `docs/PRD.md` PRD APPROVED v1.1 + `docs/plans/00-roadmap.md`). APIs here MUST NOT drift from the spine; any change amends the spine first.
- **One-line purpose:** Light JSONL export/import — atomic temp+fsync+rename write, path-confinement preflight, conflict-marker + malformed-JSON rejection (zero DB writes on reject), tombstone-non-resurrection, and the one-shot best-effort `bd`→unblock import. **No git, no merge, no 3-way reconciliation** (D5/D13/NFR-6).
- **Layer:** L3 (`sync` | `health`).
- **Depends on:** `unblock-storage` (L2), `unblock-model` (L0), `unblock-error` (L0). Acyclic per spine layering `model|error → policy → storage → sync|health → config → engine → render → mcp|cli`. **No dependency** on policy/config/engine/render/mcp/cli (would be cyclic). Transitively re-exports nothing from libsql (NFR-15: no backend type crosses this boundary — sync consumes only the `Storage` trait + model types).

> **Scope discipline (D5).** This crate is *shrunk* relative to the original `temp/beads_rust-main/src/sync/` (`mod.rs` ~325k, `path.rs` ~58k, `history.rs`, `witness.rs`). We deliberately **drop**: git-merge / 3-way merge, 4-phase collision detection, distributed locks, witness/history snapshots, base-snapshot temp files, data-loss-guard heuristics (empty-db-over-nonempty refusal beyond a simple opt-in flag), and the `.manifest.json`/`metadata.json` machinery. We **keep** behaviour fidelity for: atomic write (temp in same dir → `flush` → `sync_all` → atomic rename; remove temp + leave original intact on error), path-confinement preflight, conflict-marker scan (`<<<<<<<`/`=======`/`>>>>>>>`), per-line JSONL validation before any DB mutation, and tombstone-non-resurrection.

---

## Public API summary (what other crates import)

The **only** consumer is `unblock-engine` (L5), via `Session::export_jsonl` / `import_jsonl` / `import_bd` (spine §4.1). The engine holds the write permit (D14) and calls these; sync itself acquires no semaphore and owns no concurrency policy.

### v1 (LOCKED) — surfaced from `lib.rs`

```rust
// ---- reports (CF-A: DEFINED in unblock-model §1.10 with the full derive set; sync re-exports, never redefines) ----
pub use unblock_model::{ExportReport, ImportReport}; // shapes per spine §1.10/§4.1; sync produces these

// ---- options ----
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub dry_run: bool,                 // validate + plan, mutate nothing (FR-8 AC, MCP sync.import dry_run)
    pub allow_external: bool,          // opt-in to write/read outside .unblock/ (NFR-7); default false
    pub on_collision: CollisionPolicy, // id already present: Skip (default) | Error | OverwriteIfNewer (content_hash/updated_at)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CollisionPolicy { #[default] Skip, Error, OverwriteIfNewer }

#[derive(Debug, Clone)]
pub struct ExportOptions { pub allow_external: bool /* default false */ }

// ---- the two free functions sync exposes; engine passes its Arc<dyn Storage> + resolved confine root ----
pub async fn export_jsonl(
    storage: &dyn Storage, path: &Path, confine_root: &Path, opts: &ExportOptions,
) -> Result<ExportReport, SyncError>;

pub async fn import_jsonl(
    storage: &dyn Storage, path: &Path, confine_root: &Path, actor: &str, opts: &ImportOptions,
) -> Result<ImportReport, SyncError>;

pub async fn import_bd(
    storage: &dyn Storage, path: &Path, confine_root: &Path, actor: &str,
) -> Result<ImportReport, SyncError>;   // FR-26/D16; bd-export jsonl already produced upstream

// ---- preflight + scanning primitives (also used by unblock-health lite + fuzz) ----
pub fn validate_sync_path(path: &Path, confine_root: &Path, allow_external: bool) -> Result<PathBuf, SyncError>; // returns canonicalized confined path
pub fn scan_conflict_markers(path: &Path) -> Result<Vec<ConflictMarker>, SyncError>;
pub fn ensure_no_conflict_markers(path: &Path) -> Result<(), SyncError>; // -> ErrorCode::ConflictMarkers
pub struct ConflictMarker { pub line: usize, pub marker_type: ConflictMarkerType, pub branch: Option<String> }
pub enum ConflictMarkerType { Start, Separator, End }

// ---- error ----
pub enum SyncError { /* §error below; impl code()->ErrorCode */ }
```

`confine_root` is the absolute `.unblock/` dir; the engine/config resolves it. Sync never discovers the workspace itself (that is L4 `unblock-config`'s job) — passing it in keeps sync acyclic and pure.

### v1.1 (LOCKED) — additive

- `append_interaction(path, &Interaction)` for FR-22 flight recorder (append-only `interactions.jsonl`, capture-only Tier-1 attribution). New file `src/audit.rs`. Roadmap §2 lists FR-22 touching `sync`.
- Export gains comment lines (FR-6 organization) — no API shape change, only model hydration carries `comments`.

### v1.2 (PROPOSED) — additive seams only

- Reconciliation seams *if any* for the libsql remote/replica path (roadmap §3 "`unblock-sync` (reconciliation seams if any)"). Likely just a `SyncEqualsReport` helper exposing `model::sync_equals` decisions for sync-conflict diagnostics; **no merge logic** enters this crate (D5 stays in force). May add `SyncError::SyncConflict` usage. Kept behind no feature unless remote pulls it.

### v1.3 (PROPOSED) — additive

- Compaction round-trip support (roadmap §4 "Compaction interacts with JSONL round-trip fidelity (D12)"): export/import must remain lossless for compacted issues; adds proptest coverage + a `compact`-aware normalize path. No new public fn — extends `roundtrip` proptests and `normalize`.

> v2+ (`◐` DB-only option) is direction-only; not planned at file level here.

---

## FILE BREAKDOWN

| Path | Responsibility | Key items (reference spine §) | Version | Tests |
|---|---|---|---|---|
| `Cargo.toml` | Crate manifest. deps: `unblock-model`, `unblock-error`, `unblock-storage`, `tokio` (fs/io via the async export write path through engine permit — but file I/O is sync inside `spawn_blocking`), `serde`/`serde_json`, `chrono`, `snafu`, `tracing`, `dunce` (cross-platform canonicalize, as in original `path.rs`). dev-deps: `proptest`, `insta`, `tempfile`, `tokio` test macro. `#![forbid(unsafe_code)]`, clippy pedantic, `missing_docs=warn` (spine §0). | — | v1 | n/a |
| `src/lib.rs` | Crate root + module wiring + re-exports of the public API above. Crate-level docs stating the D5 light-scope and the "no git / no merge" invariant (NFR-6). `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`. | sync-owned re-exports: `export_jsonl`/`import_jsonl`/`import_bd`/`validate_sync_path`/`scan_conflict_markers`/`ensure_no_conflict_markers`/`SyncError`/`ImportOptions`/`ExportOptions`/`CollisionPolicy`/`ConflictMarker(Type)`. Model re-exports (CF-A): `pub use unblock_model::{ExportReport, ImportReport}`. | v1 | doctest: a confined export→import round-trip on a small in-memory `Storage` fake. |
| `src/error.rs` | `#[derive(Debug, Snafu)] pub enum SyncError` with context selectors; `impl SyncError { pub fn code(&self) -> ErrorCode }` mapping every variant to the exit-6/8 codes. Backend errors absorbed via `#[snafu(transparent)]` over `StorageError` (never re-exposes libsql). | Variants → `ErrorCode` (spine §2.2/§2.3): `ConflictMarkers`→`ConflictMarkers`(6); malformed line→`JsonlParseError`(6); symlink/`.git`/`..`/bad-ext→`PathTraversal`(6); id already present under `Error` policy→`ImportCollision`(6); prefix mismatch (bd/import)→`PrefixMismatch`(6); semantic conflict (v1.2)→`SyncConflict`(6); fs read/write/rename/fsync→`IoError`(8); serde encode→`JsonError`(8); storage passthrough→`StorageError.code()`. `is_retryable` follows `ErrorCode::is_retryable`. | v1 | unit: each variant → expected `ErrorCode` + `exit_code()`; snafu `Display` non-empty; transparent storage wrap preserves code. |
| `src/path.rs` | Path-confinement preflight (NFR-7). Lexical normalization (no `..`), reject `.git` component, reject symlink escape of an existing ancestor outside `confine_root`, allowed-extension/exact-name check, canonicalize via `dunce`. For non-existent target files, validate the parent dir is confined. `confine_root` canonicalized once. Distilled from original `path.rs` (drops manifest/metadata exact-names beyond `issues.jsonl`). | `validate_sync_path(path, confine_root, allow_external) -> Result<PathBuf>`; `validate_temp_path(temp, final, confine_root)`; `has_git_component`; `normalize_lexically`; const `ALLOWED_EXTENSIONS = ["jsonl"]` (+ pid-scoped `*.jsonl.<pid>.tmp` temp pattern); const `ALLOWED_EXACT_NAMES = ["issues.jsonl"]`. `allow_external=true` relaxes the confine-root prefix check but **never** the `.git`/`..`/symlink-escape checks (NFR-8: force never bypasses safety). | v1 | unit: confined path ok; `../escape` rejected (`PathTraversal`); `.git/x` rejected; symlinked-ancestor-escape rejected; `.txt` ext rejected; new (non-existent) file in confined dir ok; `allow_external` still rejects `.git`. proptest: random relative paths under root never escape after normalize (no normalized path starts outside `confine_root`). |
| `src/conflict.rs` | Conflict-marker scanner (FR-8/NFR-8). Streams lines (2 MiB `BufReader`), detects `<<<<<<<`/`=======`/`>>>>>>>` line-prefixes, records line number (1-based) + branch tail. `ensure_no_conflict_markers` errors with a ≤5-marker preview. Reads via a metadata-validated fd (size guard — see open Q1). | `ConflictMarker`, `ConflictMarkerType{Start,Separator,End}`, `scan_conflict_markers`, `ensure_no_conflict_markers`, `detect_conflict_marker(line) -> Option<(ConflictMarkerType, Option<String>)>`, consts `CONFLICT_START/SEPARATOR/END`. Mirrors original `mod.rs:1387-1480` behaviour. | v1 | unit: clean file → `[]`; each marker kind detected at right line/branch; mixed markers ordered; CRLF lines; `ensure_*` returns `ConflictMarkers` with preview; empty file ok. fuzz target consumes this (see fuzz). |
| `src/jsonl.rs` | Line-oriented JSONL read/parse + per-line validation (FR-7/FR-8). Parse each non-empty trimmed line as `model::Issue` (serde); `content_hash` recomputed on load (`#[serde(skip)]`, spine §1.8/CR-3); run `IssueValidator::validate`; reject duplicate ids within the file; collect per-line failures **before** any DB mutation. Also the deterministic **serializer** for export: one `Issue` per line, stable field order via serde, trailing `\n`, byte-deterministic for fixed DB state (FR-7 AC / NFR-4). | `serialize_issue_line(&Issue) -> Result<String, SyncError>`; `parse_issue_line(line, line_no) -> Result<Issue, SyncError>`; `validate_records(path) -> Result<JsonlValidationSummary, SyncError>`; `JsonlValidationSummary { record_count, failures: Vec<(usize,String)>, ids: Vec<String> }`; `normalize(&mut Issue)` (canonicalize before hash/compare; `compaction_level None==0`, spine §1.8). | v1 | unit: valid line round-trips; malformed JSON → `JsonlParseError` w/ line no; blank lines skipped; duplicate id in file → failure; invalid (title>500 / priority 5) → `ValidationFailed`; export line is byte-stable across runs. proptest: `parse(serialize(issue)) == issue` for arbitrary valid `Issue` (round-trip incl. all Option/relation fields, spine §1.6). |
| `src/atomic.rs` | Atomic write primitive (FR-7/NFR-4). Create pid-scoped temp `*.jsonl.<pid>.tmp` in the **same dir** as the target (path-validated), write all lines, `flush`, `File::sync_all` (fsync), then atomic `rename` over the target. On any error: remove temp, leave original untouched. Unix: set restrictive perms (0600) on temp before rename. All blocking fs ops run inside `tokio::task::spawn_blocking` so the async signature holds without blocking the runtime. | `pub async fn write_atomic(final_path, confine_root, lines: impl Iterator<Item=String>) -> Result<usize, SyncError>`; `temp_path_for(final_path, attempt) -> PathBuf` (pid + attempt suffix, collision retry like original `mod.rs:203`); `set_restrictive_perms` (cfg unix / no-op else). | v1 | unit: write creates target with content; killed-before-rename (inject error after temp write) leaves original intact + removes temp (NFR-4 failure-injection); temp lands in same dir; rename is atomic (no partial); perms 0600 on unix. (Full SIGTERM-mid-write e2e lives in engine M3, NFR-5 — this is the unit-level guarantee.) |
| `src/export.rs` | Orchestrates export: preflight path (`path::validate_sync_path`), pull all issues from `Storage::list_issues` (include_closed + tombstones for fidelity — see open Q3), hydrate relations, serialize each via `jsonl::serialize_issue_line`, hand to `atomic::write_atomic`, return `ExportReport{written, path}`. `tracing` on `unblock.reliability` (INFO on external-path use, NFR-13). | `export_jsonl(storage, path, confine_root, opts) -> Result<ExportReport, SyncError>` (the public fn). | v1 | unit (with `Storage` fake): N issues → file has N lines, report.written==N; external path without `allow_external` → `PathTraversal`; 0-issue DB exports empty file cleanly. insta: snapshot of a fixed 3-issue export (deterministic bytes, NFR-14). |
| `src/import.rs` | Orchestrates import preflight→apply (FR-8 order is normative): (1) `path::validate_sync_path`; (2) `conflict::ensure_no_conflict_markers`; (3) `jsonl::validate_records` (malformed/duplicate/validation) — **any failure aborts with zero DB writes**; (4) if `dry_run`, return planned `ImportReport` (imported counts what *would* apply, skipped explained) and stop; (5) apply per-id via `Storage`: dedup by `content_hash` (idempotent, FR-26), honour `CollisionPolicy`, and enforce **tombstone-non-resurrection** (a non-tombstone line for a DB-tombstoned id is skipped/rejected, spine §1.8); collect `dropped_fields`. | `import_jsonl(storage, path, confine_root, actor, opts) -> Result<ImportReport, SyncError>`; private `apply_issue(storage, existing: Option<Issue>, incoming: Issue, policy, actor) -> ApplyOutcome` (Imported|Skipped{reason}|Rejected); `is_tombstone_resurrection(existing, incoming) -> bool` (uses `model` tombstone helpers). | v1 | unit (Storage fake): clean file imports all; conflict-marker file → `ConflictMarkers`, **0 writes** asserted; malformed line → `JsonlParseError`, 0 writes; symlink-escape → refused at preflight, 0 writes; re-import is idempotent (content_hash dedup), imported==0 second run; tombstoned id + non-tombstone line → skipped, DB unchanged; `dry_run` mutates nothing but reports plan; `CollisionPolicy::Error` on existing id → `ImportCollision`. proptest: export-then-import is identity at the DB level (`sync_equals`, spine §1.8); import is idempotent under repetition. |
| `src/bd_import.rs` | FR-26/D16 one-shot best-effort `bd` import. Input is a `bd-export`-produced JSONL (the engine/CLI runs `bd-export` upstream; **this crate runs no external command** — NFR-6/D13). Maps bd field names → unblock `Issue`/`Dependency` where they differ, records unmapped/dropped fields, then funnels into the same `import.rs` apply path (so conflict/tombstone/dedup guarantees are reused). Prefix remap awareness (`PrefixMismatch`). | `import_bd(storage, path, confine_root, actor) -> Result<ImportReport, SyncError>`; `map_bd_record(serde_json::Value) -> Result<(Issue, Vec<String> /*dropped*/), SyncError>`; `BD_FIELD_MAP` table. | v1 | unit: a captured bd-export fixture maps to expected `Issue` set; dropped/unmapped fields reported; idempotent on rerun (dedup by content_hash); deps/comments counts reported. insta: snapshot of the mapping report (counts + dropped-field list) over a small fixture. |
| `src/audit.rs` | **[v1.1]** FR-22 flight recorder: append-only `interactions.jsonl` writer; one JSON line per interaction with capture-only Tier-1 attribution (`agent_name`/`harness`/`model`); never enforced; append uses `OpenOptions::append` + per-line flush (not the atomic-rewrite path — append-only). Path-confined like export. | `append_interaction(path, confine_root, &Interaction) -> Result<(), SyncError>`; `Interaction { ts, actor, tool, action, attribution, .. }` (or re-uses a model type if one is added). | v1.1 | unit: append adds exactly one line; concurrent appends don't interleave a partial line; path-confined; malformed dir refused. |
| `tests/contract.rs` | Crate-level integration: drives the full public surface against the real `unblock-storage` libsql impl (in-memory/temp file) — the export/import contract suite tie-in (NFR-16). round-trip, idempotency, reject-with-zero-writes, tombstone, dry-run, external-path. | uses `export_jsonl`/`import_jsonl`/`import_bd`. | v1 | integration: end-to-end export→import identity on real storage; reject paths leave DB + file untouched; `import_bd` over a real bd fixture (FR-26 AC dogfood-shaped). |
| `tests/atomic_failure.rs` | NFR-4 failure-injection integration: simulate write failure / process-kill before rename via an injected fault point; assert original file integrity + temp cleanup. | — | v1 | integration: pre-rename failure leaves original byte-identical; orphan temp removed; partial temp never visible as target. |
| `tests/roundtrip_proptest.rs` | Property suite: arbitrary `Vec<Issue>` → export → import → `sync_equals` identity; double-import idempotency; serialize/parse line round-trip for the full field set (spine §1.6) incl. compaction fields. | proptest strategies for `Issue` (shared with model where possible). | v1 (extended v1.3 for compaction) | proptest (256+ cases): DB-level identity; idempotency; no-panic on adversarial-but-valid inputs. |

> `unblock-fuzz` (separate workspace member, not owned here) adds a `cargo-fuzz` target over `conflict::scan_conflict_markers` + `jsonl::parse_issue_line` (NFR-16 "fuzz over the ingestion surface"). This plan only notes the seam; the targets live in the fuzz crate.

---

## Crate-level test & bench plan

- **Unit (per file):** as tabled. Every error variant asserted to map to the exit-6/8 `ErrorCode` (spine §2.3 golden table). `Storage` interactions use a lightweight in-crate fake (`struct FakeStorage` impl of the subset of `Storage` sync needs: `list_issues`, `get_issue`/`get_issues`, `create_issue`, `update_issue`) to keep unit tests backend-free.
- **proptest (NFR-16):** (1) line serialize/parse round-trip over arbitrary valid `Issue`; (2) export→import DB identity via `sync_equals`; (3) import idempotency; (4) path normalization never escapes `confine_root`. v1.3 extends (1)/(2) to compacted issues (D12 fidelity).
- **insta (NFR-14):** deterministic export bytes for a fixed issue set; bd-import mapping report; conflict-marker preview string. CI snapshot-check gate.
- **Integration contract (NFR-16):** `tests/contract.rs` runs the same scenarios against the **real libsql** storage to prove the trait contract, not just the fake.
- **Failure-injection (NFR-4/NFR-5):** `tests/atomic_failure.rs` — pre-rename fault leaves original intact; the SIGTERM-mid-write *whole-process* test is owned by `unblock-engine` M3 (this crate provides the unit-level atomic guarantee it relies on).
- **fuzz (NFR-16):** ingestion-surface targets registered in `unblock-fuzz` over `scan_conflict_markers` + `parse_issue_line`.
- **Bench:** no `criterion` owned here in v1 — NFR-1 export/import budgets (export 10k <500ms, import 10k <1s) are measured at the **engine** level (`Session::export_jsonl`/`import_jsonl`) since that is the user-facing op and includes storage. If profiling shows sync-internal serialize/parse dominates, add `benches/jsonl.rs` (serialize/parse 10k lines) in v1.1 — flagged as an open question.

---

## Open questions specific to this crate

1. **Input-size guard for the ingestion surface (NFR-18).** Should `scan_conflict_markers`/`validate_records` enforce a max file size / max line length before reading (the original validated fd metadata)? Proposed: yes — a configurable cap with a sane default, rejected at preflight as `IoError`/`PathTraversal`. Needs a default value decision (engine/config may own the limit since NFR-18 size/rate limits live at the MCP boundary).
2. **`CollisionPolicy` default + surface.** Spine §4.1's `ImportOptions` only names `dry_run` (MCP `sync.import` exposes `dry_run`). Is `on_collision` an internal-only knob (default `Skip` for idempotent re-import) or should it surface on the MCP `sync` tool? Proposed: internal default `Skip` for v1 (matches FR-26 idempotency); do **not** widen the MCP tool surface (RK-3). Confirm before exposing.
3. **What does export include?** FR-7 says "snapshot of DB state." Must export include closed + tombstone issues for round-trip fidelity (tombstone-non-resurrection only matters if tombstones are exported)? Proposed: export **all** non-ephemeral issues incl. closed + tombstones (fidelity), exclude `ephemeral`. Confirm against the bd dogfood corpus.
4. **bd field map authority (FR-26/D16).** The exact bd→unblock field mapping table (`BD_FIELD_MAP`) and which bd fields are intentionally dropped need to be derived from a real `bd-export` sample, not guessed. The worker MUST read a captured bd fixture and the spine §1.6 `Issue` shape; the bead description is not authoritative. Owner: confirm a canonical bd fixture lives under `tests/fixtures/`.
5. **v1.2 reconciliation seam shape.** Roadmap §3 hedges "reconciliation seams *if any*." Decide at v1.2 design time whether sync gains any read-only `sync_equals`-based conflict *diagnostic* helper, or whether all remote reconciliation stays inside `unblock-storage` (keeping sync purely local-JSONL). Default lean: keep it out of sync (D5).
6. **`spawn_blocking` vs sync API.** Public fns are `async` (engine calls them under tokio). File I/O is blocking; we wrap in `spawn_blocking`. Confirm the engine is fine with sync owning its own `spawn_blocking` rather than exposing blocking fns the engine wraps. Proposed: sync owns it (keeps the engine adapter thin).
