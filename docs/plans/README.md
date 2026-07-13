# unblock — Plan Index

- **Status:** Phase-2 planning complete; this index + consistency report produced by the coordinator pass.
- **Date:** 2026-06-19
- **Source of truth:** PRD APPROVED v1.1 (`docs/PRD.md`); `docs/plans/implementation-plan.md` (v1 walking skeleton). The spine (`01-design-spine.md`) is the **authoritative interface contract** for v1: no per-crate plan may drift from it without amending the spine first.

This directory holds the full planning corpus for the multi-crate Rust rewrite. Read top-down: roadmap (when) → spine (what the interfaces are) → per-crate file plans (how each crate is built).

---

## 1. Document index

| Doc | One-line description |
|---|---|
| [`00-roadmap.md`](00-roadmap.md) | Version roadmap v1 → v2+: theme/FRs/crates-touched per release; v1+v1.1 LOCKED, v1.2+ PROPOSED. Feature-to-version + crate-impact matrices. |
| [`01-design-spine.md`](01-design-spine.md) | **AUTHORITATIVE** cross-crate interface contract for v1: domain types, error taxonomy + 0–8 exit table, `Storage` trait, `Session` API, MCP 7-tool schemas, conformance rules. |
| [`crates/unblock-model.md`](crates/unblock-model.md) | L0 leaf. Pure domain types (Issue/enums/Dependency/Event), content-hash / sync-equality / tombstone semantics, `IssueValidator`, `CacheKey`. No I/O. |
| [`crates/unblock-error.md`](crates/unblock-error.md) | L0 leaf. Shared boundary vocabulary: `ErrorCode`, 0–8 exit table, `StructuredError`, `CodedError` trait, `ModelError`, message sanitization. Zero internal deps. |
| [`crates/unblock-policy.md`](crates/unblock-policy.md) | L1 pure. Versioned decision contracts: ready hybrid-sort comparator, scheduler/coordination/gates evaluators, inheritance, S3-FIFO cache kernel. Depends on model+error only. |
| [`crates/unblock-storage.md`](crates/unblock-storage.md) | L2. Backend-agnostic `Storage` trait + the only libsql impl (WAL + native busy_timeout non-spin, transactional audit, contention lab). Depends on model+error only. |
| [`crates/unblock-sync.md`](crates/unblock-sync.md) | L3. Light JSONL export/import (atomic write, path-confinement, conflict-marker + malformed reject, tombstone-non-resurrection) + one-shot `bd` import. No git/merge. |
| [`crates/unblock-health.md`](crates/unblock-health.md) | L3. Workspace health: v1 `integrity_check` + file-state `doctor`; v1.1 full Healthy/Drifted/Recoverable/Unsafe taxonomy + `.unblock/.recovery/` evidence. |
| [`crates/unblock-config.md`](crates/unblock-config.md) | L4. Layered TOML config (CLI > env > project > defaults), `.unblock/` discovery, the open-a-workspace facade the engine consumes. |
| [`crates/unblock-engine.md`](crates/unblock-engine.md) | L5. The single mutation home — `Session` composing storage+policy(+sync/health), tokio `Semaphore(1)` write serialization (D14), reads bypass it (FR-10), cooperative shutdown. |
| [`crates/unblock-render.md`](crates/unblock-render.md) | L6. Output formatting (json/robot/plain/csv/markdown; toon feature-gated). Reduced under D7 (no rich-terminal stack). Pure; returns Strings, never writes I/O. |
| [`crates/unblock-mcp.md`](crates/unblock-mcp.md) | L7 (PRIMARY). rmcp 1.7 stdio server — 7-tool taxonomy + resources + prompts, schemars-validated under quotas, `contract_version`-stamped. Thin adapter over `Session`. |
| [`crates/unblock-cli.md`](crates/unblock-cli.md) | L7. Reduced `unblock` binary — lifecycle/ops only (mcp/migrate/doctor/version/init/agents/`unblock update`), 0–8 exit-code boundary, cooperative shutdown. No domain features (D3). |
| [`crates/unblock-fuzz.md`](crates/unblock-fuzz.md) | aux (unpublished). cargo-fuzz targets over the ingestion surfaces (content-hash, JSONL/bd import, config TOML, query filters, claim race). Sink crate; nothing depends on it. |
| [`implementation-plan.md`](implementation-plan.md) | v1 walking-skeleton milestones M0–M3, task DAG (T-M.n), per-task acceptance criteria, MCP tool taxonomy. |
| [`ci-cd-and-distribution.md`](ci-cd-and-distribution.md) | CI quality gate (from M0) + `dist` release pipeline at v1 GA: 6 target triples, shell+powershell installers, GitHub attestations, `axoupdater`-backed `unblock update` (D17). |

**Total plan docs: 16** (1 roadmap + 1 spine + 12 crate plans + implementation plan + CI/CD plan).

---

## 2. Layering / dependency diagram (ASCII, acyclic — NFR-15, PRD §8.1)

```
                 L0  ┌──────────────┐      ┌──────────────┐
                     │ unblock-model│◄─────│ unblock-error│   (model → error;
                     └──────┬───────┘      └──────▲───────┘    error has no
                            │                     │            in-workspace deps)
                 L1         ▼                     │
                     ┌──────────────┐             │
                     │unblock-policy├─────────────┤   (model + error only)
                     └──────┬───────┘             │
                 L2         ▼                     │
                     ┌──────────────┐             │
                     │unblock-storage├────────────┤   (model + error only;
                     └──────┬───────┘             │    libsql is PRIVATE)
                 L3         ▼                     │
            ┌──────────────┐  ┌──────────────┐    │
            │ unblock-sync │  │unblock-health│────┤   (sync→storage;
            └──────┬───────┘  └──────┬───────┘    │    health→sync+model+error)
                   │   ▲             │            │
                   │   └─────────────┘            │
                 L4 ▼ ▼                            │
                     ┌──────────────┐             │
                     │unblock-config│─────────────┤   (storage+sync+health+model+error)
                     └──────┬───────┘             │
                 L5         ▼                     │
                     ┌──────────────┐             │
                     │unblock-engine│─────────────┘   (config+sync+storage+
                     └──────┬───────┘                  policy+health+model+error)
                 L6         ▼
                     ┌──────────────┐
                     │unblock-render│  (model + error only — see CF-A)
                     └──────┬───────┘
                 L7         ▼
            ┌──────────────┐   ┌──────────────┐
            │ unblock-mcp  │   │ unblock-cli  │   (cli → mcp, never mcp → cli;
            └──────────────┘◄──┴──────────────┘    both → engine/render/policy)

           aux:  unblock-fuzz  → model, error, storage, sync (+config v1.1)   [sink]
```

Edges point from dependent to dependency (downward). Every arrow is a strict forward (downward) edge — no back-edges, no cycles. `model | error` are co-equal L0 siblings with a single one-directional `model → error` edge.

---

## 3. Per-version overview (which crates / files change)

Legend: ● new/major work · ◐ extended/hardened · (—) untouched.

| Crate | v1 (LOCKED) | v1.1 (LOCKED) | v1.2 (PROPOSED) | v1.4 (PROPOSED) |
|---|---|---|---|---|
| `unblock-model` | ● all v1 types, hash/sync-eq/validation | ● Comment/EpicStatus populated | ◐ conditional `sync_state.rs` | ◐ `compaction.rs` helpers |
| `unblock-error` | ● ErrorCode/StructuredError/ModelError | ● additive hints/context | ◐ remote ErrorCode variants | ◐ ClaimExpired/CompactionConflict |
| `unblock-policy` | ● ready.rs/cache_key/contract | ● scheduler/coordination/gates/inheritance/saved-query | (—) | ● scheduler_v2/reclaim/cache kernel |
| `unblock-storage` | ● trait + libsql impl + contention lab | ◐ label/comment/epic/config-table methods | ● `remote.rs` (feature `remote`) | ● archival, perf indexes, 1M gate |
| `unblock-sync` | ● export/import/bd/path/conflict/atomic | ◐ `audit.rs` (FR-22) | ◐ reconciliation seam (if any) | ◐ compaction round-trip |
| `unblock-health` | ● lite: level/file_state/doctor/paths | ● full taxonomy: anomaly/classify/audit/recovery | ◐ `sync_health.rs` | ◐ `scale.rs` |
| `unblock-config` | ● discovery/merge/schema/env/cli/paths/context | ● db_layer/user_config/policy_path | ● `remote.rs` (feature `remote`) | (—) |
| `unblock-engine` | ● Session lifecycle/read/write/interchange/shutdown | ● organization/coordination/gates/saved_queries/audit | ◐ `sync_topology.rs` | ◐ `compaction.rs` |
| `unblock-render` | ● renderer/format/backends/sanitize | ◐ toon + v1.1 DTO views | (—) | ◐ `stream.rs` |
| `unblock-mcp` | ● 7 tools + resources + prompts + server | ● discriminator growth + coordination resource | ◐ sync_status resource (feature) | ◐ batch/streaming/changes |
| `unblock-cli` | ● mcp/migrate/doctor/version/init/agents + `unblock update` (FR-25, D17; axoupdater dep) | ● completions | (—) | (—) |
| `unblock-fuzz` | ● content_hash/jsonl/sync_cycle/bd/query/config-smoke | ● config full + claim_race | ◐ remote_sync (feature) | ◐ scale_ingest |

> **Note (2026-07-07 resequence):** this per-crate table predates the full v1.2–v1.5 shape. Its last column — relabeled **v1.4 (PROPOSED)** — tracks the scale / swarm-coordination / MCP-richness work formerly numbered v1.3; the **new v1.3 planning layer** (milestones + goals) and the **v1.5 local TUI** are not yet represented as columns here. `00-roadmap.md` §3–§7 (and the roadmap §9 crate-impact table) is authoritative for the v1.2–v1.5 crate shape.

**Total files planned across the 12 crate plans:** ≈ **190 plan-enumerated files** (source + test + bench + fuzz-target rows in the FILE BREAKDOWN tables). Approximate per-crate counts: model ~24, error ~13, policy ~18, storage ~22, sync ~14, health ~17, config ~20, engine ~24, render ~21, mcp ~30, cli ~24, fuzz ~18. (Counts include `Cargo.toml`, every `src/` module, `tests/`, `benches/`, and fuzz targets listed for traceability; the exact number shifts as conditional/`[v1.x]` files are confirmed.)

---

## 4. How to use these plans

1. **Picking up a crate?** Read in this order: (a) the spine sections your crate produces/consumes, (b) your crate's file plan, (c) the roadmap row for the version you target. The bead/task description is **never** authoritative — the plan + spine are.
2. **The spine wins.** If a crate plan and the spine disagree on a signature/type/field, the spine is correct; the discrepancy is a planning bug to fix (see §5). Any intentional interface change amends `01-design-spine.md` first, then the affected crate plans.
3. **Version discipline.** Implement only the version you are assigned. v1/v1.1 are LOCKED (restate the PRD); v1.2+ are PROPOSED direction — do not pull a v1.2-feature dependency into a v1 file (the `remote` feature must stay off the default build, D15/NFR-10).
4. **Acyclic layering is invariant.** Never introduce a dependency edge that points upward in §2. `unblock-storage` depends on **model + error only**; shared types both a lower and a sibling crate need live in `unblock-model` (the CF-11 rule).
5. **Open questions (`Q*`/`OQ*`) per crate** are listed at the bottom of each plan and aggregated into the cross-crate items in §5 where they touch more than one crate. Resolve cross-crate Qs at the spine level before the dependent crate starts.

---

## 5. Consistency report

The 16 documents were cross-checked for: (a) consumed APIs actually produced by a dependency and matching spine signatures; (b) acyclic graph matching PRD §8.1 (esp. storage = model+error only); (c) version coherence (no v1 file depending on a v1.2 feature); (d) MCP taxonomy matching the spine; (e) no duplicated/contradictory type definitions.

**Headline:** the dependency graph is **acyclic and matches PRD §8.1**; the MCP taxonomy **matches the spine** (7 tools ≤ 8, same discriminators); version tags are **coherent** (every `remote`/sync/scale item is correctly fenced to v1.2/v1.4 behind a non-default feature). The issues below were all **type-placement / spine-completeness** problems where multiple crate plans independently flagged the *same* gap and converged on the *same* fix — they are planning-level corrections to the spine, not contradictions between workers. **3 HIGH, 5 MEDIUM, 3 LOW — all 11 RESOLVED.** The CF-A (display-DTO relocation, incl. `DiagnosticFinding` re-export) and CF-J (`OutputFormat` single home) interface fixes are now **landed in the spine + crate docs** (re-verified by the G-6/G-7 close), so they are RESOLVED-and-landed, not RESOLVED-but-pending. **All 16 plan docs cross-checked** (1 roadmap + 1 spine + 12 crate plans + impl-plan + ci-cd); a CI `doc-lint` job (ci-cd §2 / §2.1) now mechanically guards the D-id / FR-tier / command-token / stamp / cross-ref / doc-count drift classes going forward.

> **Update — 2026-06-19:** all 11 items below (CF-A..CF-K) are **RESOLVED**. The fixes were applied to the spine (`01-design-spine.md`) and to the affected crate docs: display/filter DTOs (`CountBucket`, `GraphEdge`, `DepTree`, `CloseOutcome`, `ExportReport`, `ImportReport`, `DiagnosticReport`/`DiagnosticFinding`/`DiagnosticKind`, `ListFilters`, `CountGroupBy`, `OutputFormat`) relocated to `unblock-model` with storage/engine re-exporting; workspace-open ownership pinned to `unblock-config` via `WorkspaceContext`; and the v1.1 `Storage` config/diagnostic-probe seams reserved as additive trait methods.

> **Update — 2026-06-19 (gaps/drifts reconciliation):** a subsequent six-lens gap/drift review (24 findings, G-1..G-24; the standalone report was retired once all were resolved — see git history) confirmed CF-A and CF-J were the two "RESOLVED-in-README-but-not-landed" cases (G-6/G-7) — both are **now actually landed** in the spine §1.10 and the crate docs (render/config `pub use unblock_model::OutputFormat`; sync re-exports `Export/ImportReport`; engine re-export list explicitly includes `DiagnosticFinding`, G-10). Additionally: the spine §1.10 derive set is now NORMATIVE (Serialize/Deserialize/JsonSchema on all DTOs, G-1); the `unblock-cli → unblock-mcp` L7 edge is settled NORMATIVE in spine §0.1 (G-8, cli Q1 RESOLVED); and `DepInput2 → DepToolInput` (G-23b) + the `AttributionPolicy` vs capture-only `Attribution` split (G-23e) landed in `unblock-mcp.md`/`unblock-policy.md`. All 24 findings RESOLVED; verdict **GO** for T0.1/T0.2.

### HIGH

- **CF-A — [RESOLVED] Render-visible DTOs are placed above the crate that must format them (render = model+error only).** *Resolution: `CountBucket`, `GraphEdge`, `DepTree`, `CloseOutcome`, `ExportReport`, `ImportReport`, `DiagnosticReport`/`DiagnosticKind` relocated to `unblock-model`; storage/engine re-export them, so render imports only model+error.*
  *Crates/files:* `unblock-render` (its Public-API + Cross-crate-deps sections, OQ-3) vs spine §3.1 (`CountBucket`, `DepTree`/`GraphEdge` in storage L2) and spine §4.1 (`CloseOutcome`, `ExportReport`, `ImportReport`, `DiagnosticReport` in engine L5).
  *Problem:* `unblock-render` (L6) depends on **model + error only** (PRD §8.1) but its `Renderer` trait must format `CountBucket`, `DepTree`, `CloseOutcome`, sync reports and `DiagnosticReport`. Those types live in storage (L2) and engine (L5). Render cannot import storage or engine without widening its dependency edge (engine is L5 = below render L6 in the layer order, but PRD §8.1 still forbids the edge).
  *Fix:* **Amend the spine to relocate the display-DTOs into `unblock-model`** — `CountBucket`, `GraphEdge`, `DepTree`, `CloseOutcome`, `ExportReport`, `ImportReport`, `DiagnosticReport`/`DiagnosticKind`. (`StructuredError` is already in `unblock-error`.) Storage/engine then re-export them from model. This is the CF-11 pattern ("shared contract type lives in model") applied to display types. Decide before render T-work and before engine's report.rs.

- **CF-B — [RESOLVED] `DiagnosticReport` / `DiagnosticKind` are referenced everywhere but defined nowhere in the spine.** *Resolution: both defined in `unblock-model` (spine §1); engine populates them, mcp/render name them via model. Closed jointly with CF-A.*
  *Crates/files:* spine §4.1 (`Session::diagnostics -> DiagnosticReport`), spine §5.3 (`ToolOutput::Diagnostics(DiagnosticReport)`), `unblock-engine` (`src/diagnostics.rs` defines it locally at L5), `unblock-render` (OQ-2 needs the concrete type), `unblock-mcp` (`tools/diagnostics.rs`).
  *Problem:* engine plans to define `DiagnosticReport`/`DiagnosticKind` at L5; render (L6) and the spec for `ToolOutput` need to name them; render cannot import engine.
  *Fix:* Define `DiagnosticReport` + `DiagnosticKind` in **`unblock-model`** (spine §1) and have engine populate them. Resolves jointly with CF-A.

- **CF-C — [RESOLVED] `ListFilters` / `CountGroupBy` placement blocks policy's filter-fingerprint and is re-exported by three crates.** *Resolution: `ListFilters` + `CountGroupBy` relocated to `unblock-model` (storage re-exports); policy now owns `filters_fingerprint(&ListFilters)`. `IssuePatch`/`DeletePlan`/`DeleteMode` stay write-side in storage. Spine §3.1 amended.*
  *Crates/files:* spine §3.1 (defines `ListFilters`, `CountGroupBy`, `IssuePatch`, `DeletePlan` in storage L2); `unblock-policy` (Q3, `cache_key.rs` needs `ListFilters` but cannot depend on storage); `unblock-engine` (re-exports them for L7); `unblock-mcp` (`FilterInput → ListFilters`).
  *Problem:* Policy (L1) is below storage (L2) and needs `ListFilters` to mint deterministic cache keys / fingerprints, but may not depend on storage (CF-11). The current workaround (policy takes a pre-built `&str` fingerprint) splits the canonicalizer across two crates.
  *Fix:* **Relocate `ListFilters` + `CountGroupBy` to `unblock-model`** (storage re-exports). Then policy can own the canonical `filters_fingerprint(&ListFilters)` outright. `IssuePatch`/`DeletePlan`/`DeleteMode` are storage-write types and can stay in storage (engine/mcp re-export). Amend spine §3.1.

### MEDIUM

- **CF-D — [RESOLVED] Workspace-open ownership overlaps between config and engine.** *Resolution: workspace-open owned by `unblock-config` — it resolves config+paths and constructs the `Arc<dyn Storage>`, exposed as a `WorkspaceContext`; `Session::open` consumes that context and only does migrate-if-needed + Semaphore setup. Spine §4.1 and config `context.rs` updated.*
  *Crates/files:* `unblock-config` (Q4, `context.rs::open_with_storage` opens + migrates libsql) vs spine §4.1 / `unblock-engine` (`Session::open` "discover `.unblock/`, open libsql, migrate-if-needed") and engine Q*.
  *Problem:* Both plans claim discovery + storage construction. If both open storage, the `Arc<dyn Storage>` is built twice or ownership is ambiguous.
  *Fix:* Pin the seam in the spine: **config resolves config + paths and constructs the `Arc<dyn Storage>` via the `Storage` constructor; engine's `Session::open(cfg)` consumes a `WorkspaceContext` (config + paths + storage handle)** and does migrate-if-needed + Semaphore setup. Update spine §4.1 `SessionConfig`/`open` text and config `context.rs` to match.

- **CF-E — [RESOLVED] `Storage` trait is missing v1.1 seams that config and health already depend on.** *Resolution: v1.1 Storage seams reserved as additive (default-method) trait methods in spine §3.2 — `read_config()`/`write_config()` plus the diagnostic-probe set (`duplicate_rows`, `null_in_not_null`, `write_probe`, `child_count_drift`); no v1 break, lands in storage v1.1.*
  *Crates/files:* `unblock-config` (Q5, `db_layer.rs` needs `Storage::read_config()`), `unblock-health` (Q1, v1.1 db-state probes need diagnostic-probe methods), spine §3.2 (declares neither); `unblock-storage` (v1.1 "DB config-table accessors", but no method named).
  *Problem:* Two v1.1 consumers (config DB-layer, health full-taxonomy) require `Storage` methods the spine does not yet declare. Storage's v1.1 section promises them generically.
  *Fix:* **Reserve the names now in spine §3.2 as v1.1 additive trait methods:** `read_config()/write_config()` (config-table) and a small diagnostic-probe set (`duplicate_rows`, `null_in_not_null`, `write_probe`, `child_count_drift`) returning backend-agnostic rows. Additive (default-method) so no v1 break. Land in storage v1.1.

- **CF-F — [RESOLVED] `Attribution` is defined in two crates with different meanings.** *Resolution: policy gate type renamed to `AttributionGate`; the single capture-only `Attribution` lives in `unblock-model` and is shared by engine/mcp/event. Noted in spine §1.7 + §5.2.*
  *Crates/files:* `unblock-policy` (its gates section: a `struct Attribution` inside `PolicyDocument`/gate evaluation) and `unblock-mcp` (`tools/dto.rs` `struct Attribution` capture-only agent_name/harness/model, per spine §5.2), plus the capture-only attribution fields on `model::Event`.
  *Problem:* Same type name, two distinct concepts (gate-policy attribution requirement vs MCP capture-only agent metadata). Risk of confusion / accidental coupling.
  *Fix:* Rename the policy gate one to `AttributionGate` (or `RequireAttribution`) and keep the MCP/event capture-only one as the single `Attribution` carried in `unblock-model` (so engine/mcp/event share one definition). Note in spine §1.7 (Event attribution) + §5.2.

- **CF-G — [RESOLVED] `ModelError` home is a recommendation, not yet pinned in the spine.** *Resolution: spine §2 now states `ModelError` is defined in `unblock-error` and `model → error` is the single sanctioned L0 edge; both Qs closed.*
  *Crates/files:* `unblock-model` (Q1) and `unblock-error` (OQ-5) both recommend `ModelError` lives in `unblock-error` so `model → error` is the only model edge; PRD §8.1 literally says model depends on `—`.
  *Problem:* The two plans **agree** (no contradiction), but the spine §1.1/§2 does not explicitly state where `ModelError` is defined, leaving a documented-but-unresolved Q on the critical L0 pair.
  *Fix:* **Amend spine §2 to state `ModelError` is defined in `unblock-error`** and that `model → error` is the single sanctioned L0 edge (PRD §8.1's `—` is satisfied transitively-acyclically since error has no in-workspace deps). One-line spine edit closes both Qs.

- **CF-H — [RESOLVED] Export scope (closed/tombstone inclusion) is assumed, not specified, and round-trip identity depends on it.** *Resolution: spine states export includes all non-ephemeral issues (incl. closed + tombstones) and excludes `ephemeral`; confirms sync Q3 default and makes the round-trip property well-defined.*
  *Crates/files:* `unblock-sync` (Q3, export "include closed + tombstones?") vs spine §1.8 (tombstone-non-resurrection) and the engine/sync round-trip property (`export→import` identity under `sync_equals`).
  *Problem:* The tombstone-non-resurrection invariant only matters if tombstones are exported; the round-trip property tests assume they are. The spine does not state export contents.
  *Fix:* Add to spine (§4.1 export or a new §1.8 note): **export includes all non-ephemeral issues incl. closed + tombstones, excludes `ephemeral`.** Confirms the sync Q3 default and makes the round-trip property well-defined.

### LOW

- **CF-I — [RESOLVED] `DepInput2` naming is awkward and split across spine + mcp.** *Resolution: tool enum renamed to `DepToolInput` (G-23b); `DepInput` kept as the edge DTO. Spine §5.2 + `unblock-mcp.md` `tools/dep.rs` updated.*
  *Crates/files:* spine §5.2 (`enum DepInput2` for the `dep` tool, plus a separate `DepInput` edge struct used in `IssueInput::Create.deps`), `unblock-mcp` (`tools/dep.rs` uses `DepInput2`, `tools/dto.rs` defines `DepInput`).
  *Problem:* `DepInput2` reads like a leftover; two near-identical names invite confusion. Not a contradiction (both plans agree), but a naming smell on a public-ish schema.
  *Fix:* Rename the tool enum to `DepAction` (or `DepToolInput`) and keep `DepInput` as the edge DTO. Cosmetic spine §5.2 edit.

- **CF-J — [RESOLVED] `OutputFormat` is defined in both render and config.** *Resolution: `OutputFormat` defined once in `unblock-model`; both config and render import it (Toon variant remains feature-gated in render). No more lock-step drift.*
  *Crates/files:* `unblock-config` (`schema.rs` `enum OutputFormat { Json, Robot, Plain, Csv, Markdown }`) and `unblock-render` (`format.rs` `enum OutputFormat { Json, Robot, Plain, Csv, Markdown, #[cfg] Toon }`).
  *Problem:* Two definitions of the same enum across L4 and L6; config cannot depend on render (render is L6, above config L4), so they cannot share by import directly. Values must be kept in lock-step manually.
  *Fix:* **Define `OutputFormat` once in `unblock-model`** (or `unblock-error`-adjacent leaf) and have both config and render import it — avoids drift. Alternatively render defines it and config imports render (forbidden by layering), so model is the home. Minor spine addition.

- **CF-K — [RESOLVED via D17] Self-update ships in v1 via the `axoupdater` library (a dependency of `unblock-cli`), not a separate crate.** *Resolution: the `unblock update` command embeds `axoupdater`; artifacts are verified via GitHub artifact attestations before execution (NFR-17); behind the default-on `self-update` feature so `--no-default-features` drops the network surface. No `unblock-update` crate exists; PRD §8.1's 12-crate list is unchanged. Distribution + updater are generated by `dist` (see `ci-cd-and-distribution.md`).*

### What is consistent (verified, no change needed)

- **Acyclic graph = PRD §8.1.** Every crate's declared `Depends-on` forms a strict forward edge set; `unblock-storage` depends on **model + error only** (CF-11 honored — `CacheKey` correctly placed in model); `unblock-fuzz` is a sink. No cycles, including the `model | error` L0 pair (single `model → error` edge, error has no in-workspace deps).
- **MCP taxonomy = spine.** mcp plan ships exactly the spine §5.1 seven tools (`issue/claim/defer/query/dep/sync/diagnostics`) with matching discriminators, ≤ 8 budget, resources (§5.4) and prompts (§5.5) as specified; v1.1 grows by discriminator not by new tools (RK-3 honored).
- **Version coherence.** No v1 file depends on a v1.2/v1.4 feature: `remote` is consistently a non-default Cargo feature across storage/config/engine/health/mcp/fuzz; scale/1M and compaction are fenced to v1.4; the contention lab is correctly the M0 exit gate before any crate depends on storage.
- **Error contract.** `ErrorCode` (incl. new `AlreadyClaimed`, dropped `YamlError`), the 0–8 exit table, `StructuredError` and `CodedError` are used identically by every per-crate snafu enum; the golden exit-code snapshot is dual-pinned (error crate + cli).
- **Write-serialization contract (D14).** Exactly one home — engine's `Semaphore(1)`; storage relies on WAL + native busy_timeout; mcp/cli are thin adapters with no second lock. Reads bypass the permit everywhere (FR-10). Consistent across storage/engine/mcp/cli.
