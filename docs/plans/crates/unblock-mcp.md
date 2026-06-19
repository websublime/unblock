# unblock-mcp — File-level Plan

- **Status:** DRAFT — for review. Conforms to `docs/plans/01-design-spine.md` §5 (normative MCP contract), `docs/plans/00-roadmap.md` (versions), `docs/PRD.md` (FR-20/FR-11/FR-12/FR-15, NFR-18, D2/D7/D13).
- **Date:** 2026-06-19
- **One-line purpose:** The **PRIMARY** product surface — an `rmcp 1.0` stdio MCP server (`unblock serve`) that exposes the engine as a consolidated **7-tool** taxonomy + resources + prompts, schemars-validated under quotas (NFR-18), discoverable via a `contract_version`-stamped capabilities/schema bundle (FR-12), with every error mapped to the structured `code`/`message`/`hint`/`retryable` boundary (FR-11). It is a **thin adapter** over `unblock-engine::Session` — no domain logic, no write orchestration (RK-2/FR-9/D14).

## Layer & dependencies

- **Layer:** L7 (`mcp`), the top of the acyclic graph alongside `unblock-cli`. Must stay acyclic — nothing depends on `unblock-mcp`.
- **Depends-on (per spine §0 / PRD §8.1):** `unblock-engine` (L5, the single mutation home — all tools call `Session`), `unblock-render` (L6, structured output shaping / stdout-stderr discipline NFR-14), `unblock-policy` (L1, versioned contract ids/descriptors surfaced in capabilities, e.g. `unblock.scheduler.v1` in v1.1), plus the L0 leaves transitively: `unblock-model` (domain types **and** the query/result contract types in tool I/O — per spine §1.10 these include `DiagnosticKind`/`DiagnosticReport`/`DiagnosticFinding`, the display/result DTOs `CountBucket`/`DepTree`/`CloseOutcome`/`ExportReport`/`ImportReport`, and the filter inputs `ListFilters`/`CountGroupBy`; this crate **sources them from `unblock-model` and never redefines them**, CF-A/CF-B/CF-C), `unblock-error` (`ErrorCode`/`StructuredError`/exit-code table). **External:** `rmcp` (features `server`, `transport-io`; pinned, RK-2), `schemars`, `serde`/`serde_json`, `chrono`, `tokio`, `tracing`, `snafu`.
- **Conformance invariants:** `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`, clippy pedantic. **No libsql/backend type ever appears** (spine §6.2). **Tool count ≤ 8**, extended by discriminator before adding tools (spine §6.6 / RK-3). No `git`/`Command::new` (NFR-6). Network/TLS only via the transitive `remote` feature of storage, never enabled here on the default path (D15).

## Public API summary (what other crates import)

`unblock-mcp` is consumed by **exactly one** caller: `unblock-cli`'s `serve` subcommand (D3 — the CLI owns binary lifecycle; the MCP server is the feature surface). The surface is deliberately tiny:

| Item | Version | Consumed by | Notes |
|---|---|---|---|
| `pub async fn serve(session: Arc<Session>, opts: ServeOptions) -> Result<(), McpServerError>` | v1 | `unblock-cli` | Builds the rmcp server, binds the stdio transport, runs until cancellation. |
| `pub struct ServeOptions { pub instructions: Option<String>, pub quotas: Quotas, pub cancel: CancellationToken }` | v1 | `unblock-cli` | Server-name/version, instruction string, request quotas (NFR-18), cooperative-shutdown handle (FR-17). |
| `pub struct Quotas { max_request_bytes, max_array_len, max_string_len, max_batch, max_concurrent_requests }` | v1 | `unblock-cli`, tests | Untrusted-input limits enforced before any tool body (NFR-18). |
| `pub const CONTRACT_VERSION: &str` | v1 | tests, capabilities builder | Bumped on any tool/resource/prompt schema change (FR-12). |
| `pub fn capabilities() -> Capabilities` / `pub fn schema_bundle() -> SchemaBundle` | v1 | `unblock-cli` (offline `version`/`schema` dump), conformance tests | Pure (no `Session`); lets CLI emit the contract without a running server. |
| `pub enum McpServerError` (snafu) | v1 | `unblock-cli` | Server lifecycle/transport errors only; per-tool domain errors are returned in-band as `ToolOutput::Error`. |

Everything else (tool routers, input/output DTOs, resource/prompt handlers, the engine-error→`StructuredError` mapper) is **`pub(crate)`** — not part of the cross-crate contract. The spine §5 input/output **shapes** are normative but live behind this crate's boundary; only `serve`/`capabilities`/`schema_bundle`/`CONTRACT_VERSION`/`ServeOptions`/`Quotas`/`McpServerError` are exported.

---

## FILE BREAKDOWN

> Module layout follows the original's `mcp/{mod,tools,resources,prompts}.rs` split (grounding: `temp/beads_rust-main/src/mcp/`) but de-monolithed: `tools.rs` was 5,130 LOC there; here each tool family is its own file under `src/tools/`. Versions: **v1** (M2 ship), **v1.1** (surface growth — labels/comments/scheduler/coordination/gates/saved-queries), **v1.2** (sync-status resources), **v1.3** (batch/streaming/subscription surface). Per roadmap §6/§7, `unblock-mcp` participates in all four; no v2+ work is planned here beyond the "new transports" direction note.

### Core wiring

| Path | Responsibility | Key items | Version | Tests |
|---|---|---|---|---|
| `src/lib.rs` | Crate root: lints (`forbid(unsafe_code)`, `warn(missing_docs)`), module tree, public re-exports (`serve`, `ServeOptions`, `Quotas`, `CONTRACT_VERSION`, `capabilities`, `schema_bundle`, `McpServerError`). | re-exports only; no logic. | v1 | doctest on the `serve` example (compile-only, mocked `Session`); `tests/public_api.rs` asserts the exported symbol set is exactly the contract above. |
| `src/server.rs` | `serve()` entry point + rmcp `ServerHandler` impl. Builds the server (name `"unblock"`, `env!("CARGO_PKG_VERSION")`, instructions), registers the 7 tool routers + resources + prompts, binds `transport-io` stdio, runs to completion, integrates `CancellationToken` for FR-17 cooperative shutdown. Mirrors original `run_serve` (`mod.rs:720`) but over `rmcp` not `fastmcp_rust`, and holding `Arc<Session>` (engine owns the write Semaphore — no `with_mutation`/cross-process lock here, D14). | `pub async fn serve`; `struct UnblockServer { session: Arc<Session>, quotas: Quotas }`; `impl ServerHandler`; `#[tool_router]` aggregation. | v1 | unit: server builds with all 7 tools + 5 resources + 3 prompts registered (assert counts ≤8 tools); shutdown: `cancel.cancel()` drains in-flight then returns `Ok(())`; integration via `tests/lifecycle.rs`. |
| `src/options.rs` | `ServeOptions`, `Quotas` (defaults: `max_request_bytes=256KiB`, `max_array_len=10_000`, `max_string_len=64KiB`, `max_batch=100`, `max_concurrent_requests=64`), `CONTRACT_VERSION`. | `ServeOptions`, `Quotas` (+`Default`), `CONTRACT_VERSION`. | v1 | unit: `Quotas::default()` snapshot (insta); proptest that a request exceeding any limit is classified over-quota. v1.3: extends defaults for batch/stream. |
| `src/error.rs` | Crate's snafu enum **for server lifecycle only** (transport bind failure, stdio I/O, server already running). Domain errors are NOT here — they flow in-band. Plus the **boundary mapper** `engine_error_to_structured(&EngineError) -> StructuredError` (spine §2.4 / §5.6): composes the engine's union error → exactly one `ErrorCode` → `code/message/hint/retryable/context`, terminal-sanitized message. | `enum McpServerError` (snafu, `#[snafu(visibility(pub(crate)))]` except the public enum); `pub(crate) fn engine_error_to_structured`; `pub(crate) fn to_rmcp_error_data(&StructuredError) -> rmcp::model::ErrorData`. | v1 | unit: every `EngineError` variant maps to a stable `ErrorCode`; **golden insta snapshot** of the full `code→{exit_code,retryable,hint-shape}` map (parity with spine §2.3 exit table — FR-11); message sanitization strips control chars. |

### Tools (`src/tools/`) — 7 consolidated tools (spine §5.1)

| Path | Responsibility | Key items (spine §5.2/§5.3 shapes — normative) | Version | Tests |
|---|---|---|---|---|
| `src/tools/mod.rs` | Tool-router aggregation; shared `validate(&Quotas, &T)` preflight (size/array/string limits, NFR-18) run **before** any `Session` call; `Attribution` flatten helper; common `Result<ToolOutput, _>` → rmcp `CallToolResult` adapter (always-valid-JSON, FR-11). | `pub(crate) fn tool_router`; `fn enforce_quota`; `fn ok_json(ToolOutput)`; `fn err_json(StructuredError)`. | v1 | unit: oversized arg rejected pre-engine (no `Session` touch — assert via spy `Session`); malformed-action enum → `ValidationFailed`; output is valid JSON on both ok and err paths. |
| `src/tools/issue.rs` | Tool **#1 `issue`**: `IssueInput` (`action: create\|show\|update\|close\|reopen\|delete`) → `Session::{create,get,update,close_with_suggestions,delete}`. quick-create returns `ToolOutput::Id`; `close{suggest_next}` returns `ToolOutput::Close` w/ `newly_unblocked` (FR-11). Reopen via `update` patch. Delete carries `DeleteModeInput`→`DeleteMode` (tombstone/cascade/hard/dry-run). [FR-1a/1b/1c] | `enum IssueInput`, `struct PatchInput`, `enum DeleteModeInput`, `#[tool] async fn issue`. | v1 | unit per action against a mock `Session`; insta snapshot of each `JsonSchema`; proptest: round-trip create→show field fidelity; dry-run mutates nothing (mock asserts no write call); reparent-cycle surfaces `CycleDetected`. |
| `src/tools/claim.rs` | Tool **#2 `claim`**: `ClaimInput{id,assignee,attribution}` → `Session::claim`. Loser path → `ErrorCode::AlreadyClaimed` (retryable). [FR-2] | `struct ClaimInput`, `#[tool] async fn claim`. | v1 | unit: success returns claimed `Issue`; mock contention → `AlreadyClaimed`; schema snapshot. |
| `src/tools/defer.rs` | Tool **#3 `defer`**: `DeferInput{action: defer\|undefer}` → `Session::{defer,undefer}`. [FR-3] | `enum DeferInput`, `#[tool] async fn defer`. | v1 | unit: defer sets `defer_until`; undefer clears; schema snapshot. |
| `src/tools/query.rs` | Tool **#4 `query`**: `QueryInput{kind: list\|ready\|blocked\|search\|count\|stale}` + `FilterInput`→`ListFilters` → `Session::{list,ready,blocked,search,count,stale}`. `ready` default-complete (no limit unless set); `search` default cap 50. [FR-4] | `enum QueryInput`, `struct FilterInput`, `fn to_list_filters`, `#[tool] async fn query`. | v1 | unit per kind; proptest: `FilterInput`→`ListFilters` total mapping; insta snapshot of `ready` output shape; cap-50 default + override; quota caps `limit`. |
| `src/tools/dep.rs` | Tool **#5 `dep`**: `DepToolInput{action: add\|remove\|list\|tree\|cycles\|graph}` → `Session::{add_dep,remove_dep,list_dependencies(via get),dependency_tree,dependency_graph,detect_cycles}` (`graph` action backed by `Session::dependency_graph(roots)`). Cycle-rejecting add surfaces the path. [FR-5] | `enum DepToolInput` (was `DepInput2`; spine §5.2), `struct DepInput` (edge), `#[tool] async fn dep`. | v1 | unit per action; `add` blocks-cycle → `CycleDetected` w/ path; `tree`/`graph` snapshot; schema snapshot. |
| `src/tools/sync.rs` | Tool **#6 `sync`**: `SyncInput{action: export\|import\|import_bd}` → `Session::{export_jsonl,import_jsonl,import_bd}`. Path defaults to `.unblock/issues.jsonl`; path-confinement + conflict-marker rejection are enforced **in `unblock-sync`/engine**, surfaced here as `PathTraversal`/`ConflictMarkers`/`JsonlParseError`. `import{dry_run}`. [FR-7/8/26] | `enum SyncInput`, `#[tool] async fn sync`. | v1 | unit: export returns `ExportReport`; import dry-run reports plan, no writes; conflict-marker file → `ConflictMarkers` (mock engine); `import_bd` idempotency surfaced; schema snapshot. |
| `src/tools/diagnostics.rs` | Tool **#7 `diagnostics`** (7-tool taxonomy unchanged): `DiagnosticsInput{kind: stats\|info\|where\|version\|lint\|changelog\|orphans}` (mirrors `DiagnosticKind` from spine §1.10) → `Session::diagnostics(DiagnosticKind)` returning `DiagnosticReport`. `DiagnosticKind`/`DiagnosticReport`/`DiagnosticFinding` are **sourced from `unblock-model`** (spine §1.10, CF-A/CF-B) — no local redefinition; this file only declares the wire-facing `DiagnosticsInput` discriminator and maps it to the model `DiagnosticKind`. `version` embeds `CONTRACT_VERSION`. Pure-DB; **no git** (FR-15/NFR-6). | `enum DiagnosticsInput`, `fn to_diagnostic_kind`, `#[tool] async fn diagnostics`. | v1 | unit per kind; `DiagnosticsInput`→`DiagnosticKind` total mapping; `version` includes contract version; static assert no `Command`/git symbol (covered by workspace NFR-6 gate); schema snapshot. |
| `src/tools/dto.rs` | Shared input DTOs that flatten across tools: `Attribution` (capture-only, never enforced — spine §5.2), `DepInput`, and `From`/`Into` glue mapping input DTOs → model query types (`ListFilters`/`CountGroupBy`, spine §1.10/CF-C). **Result/display DTOs returned through `ToolOutput`** (`CountBucket`, `DepTree`, `CloseOutcome`, `ExportReport`, `ImportReport`, `DiagnosticReport`) are the spine §1.10 types **sourced from `unblock-model`** (CF-A/CF-B) and serialized as-is — not redefined here. Keeps tool files thin. | `struct Attribution`, conversions. | v1 | unit: `Attribution` default + flatten; proptest: DTO→model conversions lossless for supported fields, drop-list reported for unmapped. |

**v1.1 tool extensions (by discriminator, NOT new tools — RK-3 / spine §6.6):**

| Path | Change | Version | Tests |
|---|---|---|---|
| `src/tools/issue.rs` | Add `comment` sub-action(s) for threaded comments add/list (FR-6); labels add/remove/set already in `update` patch — extend with `rename`/`list_all` via a label discriminator on `query` or `issue` (decided in OQ-3). | v1.1 | new-action unit + schema-diff test asserting `CONTRACT_VERSION` bumped. |
| `src/tools/query.rs` | Add `kind: scheduler` (ranked, `unblock.scheduler.v1`, FR-18) and `kind: coordination` (read-only stale-claim diagnosis `unblock.coordination.v1`, FR-18); add `kind: saved` for saved queries (FR-21). | v1.1 | unit: scheduler output carries the versioned contract id; coordination is read-only (no write call); insta snapshots. |
| `src/tools/dep.rs` | (epic rollups surfaced via `query`/resource, not new tool). | v1.1 | rollup snapshot. |
| `src/tools/gate.rs` *(new file, still under tool #1/#4 discriminators — evaluated in OQ-4 whether gates ride `issue.update` transitions or a `query kind: gates`)* | Workflow-gate verdicts (`ci_green`/`min_reviewers`/`security_sign_off`, FR-19) surfaced where transitions happen; policy from `unblock-policy`/`.unblock/policy.toml`. | v1.1 | unit: a blocked transition returns `PolicyViolation` with the failing gate; schema snapshot. |

**v1.3 tool extensions:**

| Path | Change | Version | Tests |
|---|---|---|---|
| `src/tools/batch.rs` *(only if a new tool is justified vs. discriminator — RK-3 budget check)* | Batch tool calls; bounded by `Quotas.max_batch`. Richer MCP surface (roadmap §4). Preference per §4 is **resources over new tools** — this file exists only if batching cannot ride existing discriminators. | v1.3 | proptest: batch result order = input order; over-`max_batch` rejected; partial-failure reporting. |

### Resources (`src/resources/`) — spine §5.4

| Path | Responsibility | Key items | Version | Tests |
|---|---|---|---|---|
| `src/resources/mod.rs` | Resource registration + URI routing (`unblock://...`); read-only, never acquires the write permit (FR-10). | `pub(crate) fn register_resources`; URI parse/dispatch. | v1 | unit: URI parse table (valid/invalid/unknown → structured error); reads bypass write path (assert via mock). |
| `src/resources/issues.rs` | `unblock://issues/{id}` → `Issue`; `unblock://issues/ready` → `Vec<Issue>` (default-complete, agent entrypoint); `unblock://issues/blocked` → `Vec<Issue>`. Calls `Session::{get,ready,blocked}`. Ground: original `IssueResource`/`ReadyIssuesResource`/`BlockedIssuesResource` (`resources.rs`). | `ReadyResource`, `BlockedResource`, `IssueByIdResource`. | v1 | unit: `{id}` not-found → structured `IssueNotFound` w/ fuzzy `similar_ids` (mirror original `issue_not_found_resource`); ready/blocked snapshot. |
| `src/resources/capabilities.rs` | `unblock://capabilities` → `Capabilities { contract_version, tools, resources, prompts, error_codes }` (FR-12, spine §5.4). Pure builder (no `Session`). | `Capabilities`, `ToolDescriptor`, `ResourceDescriptor`, `PromptDescriptor`, `ErrorCodeDescriptor`, `pub fn capabilities()`. | v1 | **golden insta snapshot** of the whole capabilities doc; test: `contract_version == CONTRACT_VERSION`; every `ErrorCode` present with correct exit_code/retryable (FR-11 parity). |
| `src/resources/schema.rs` | `unblock://schema` → `SchemaBundle` (JsonSchema per tool I/O, FR-12). Pure builder via `schemars`. | `SchemaBundle`, `pub fn schema_bundle()`. | v1 | **golden insta snapshot** of all 7 tool schemas; **drift test:** changing any input DTO without bumping `CONTRACT_VERSION` fails (FR-12 AC). |
| `src/resources/coordination.rs` | `unblock://coordination/status` (read-only stale-claim diagnostics, `unblock.coordination.v1`, FR-18). | `CoordinationStatusResource`. | v1.1 | snapshot; read-only assertion. |
| `src/resources/sync_status.rs` | `unblock://sync/status` — replica lag / sync conflicts when the `remote` feature is active (roadmap §3, FR-16-sync). **Feature-gated** so default builds never link it. | `SyncStatusResource` behind `#[cfg(feature = "remote-status")]`. | v1.2 | feature-on snapshot; feature-off compile-absence test. |
| `src/resources/changes.rs` | Subscription-style change-notification resource / large-result streaming (roadmap §4, prefer resources over tools). | `ChangesResource`. | v1.3 | streaming chunk ordering; large-result paging snapshot. |

### Prompts (`src/prompts/`) — spine §5.5

| Path | Responsibility | Key items | Version | Tests |
|---|---|---|---|---|
| `src/prompts/mod.rs` | Prompt registration; shared arg-validation helper (mirror original `validate_prompt_arg`). | `register_prompts`, `validate_prompt_arg`. | v1 | unit: unknown arg defaults with warning. |
| `src/prompts/triage.rs` | `triage` guided workflow (blocked/unassigned/deferred context). Ground: original `TriagePrompt`. | `TriagePrompt`. | v1 | insta snapshot of generated messages against a fixed mock `Session`. |
| `src/prompts/plan_next_work.rs` | `plan_next_work` → drives `ready` → `claim` selection (FR-20). | `PlanNextWorkPrompt`. | v1 | snapshot. |
| `src/prompts/close_with_suggestions.rs` | `close_with_suggestions` → close + surface newly-unblocked (FR-11). | `ClosePrompt`. | v1 | snapshot of close+suggestions message. |

---

## Crate-level test & bench plan

**`tests/` (integration, run with `cargo test`; engine-backed via a real in-memory `Session` over a temp libsql):**

- `tests/public_api.rs` — v1 — asserts the exact exported symbol set (the public-API table). Compile-fail guards (`trybuild`) that internal DTOs are NOT exported.
- `tests/lifecycle.rs` — v1 — **the M2 exit-gate e2e:** an in-process MCP client drives `query{ready}` → `claim` → `issue{close, suggest_next}` end-to-end over the stdio transport, asserting newly-unblocked surfaced (FR-20 AC). Includes cooperative-shutdown: `cancel` mid-session returns cleanly (FR-17).
- `tests/contract_suite.rs` — v1 — **golden conformance:** capabilities + schema bundle snapshots; `CONTRACT_VERSION` stamped; full `ErrorCode`→exit-code/retryable parity vs. `unblock-error` spine §2.3 (FR-11/FR-12). Re-run as the `contract_version` drift gate.
- `tests/quotas.rs` — v1 — **NFR-18 untrusted-input boundary:** oversized request bytes, over-length arrays/strings, over-batch all rejected **before** the engine is touched (spy `Session` records zero calls); a malicious path arg is refused at preflight (surfaced as `PathTraversal`); blast radius confined to the workspace.
- `tests/error_mapping.rs` — v1 — every engine error → in-band `ToolOutput::Error` that is valid JSON; rmcp tool-error `data` carries `code/message/hint/retryable/context` (spine §5.6).
- `tests/cli_parity.rs` — v1 — same op via MCP tool vs. via `Session` directly yields identical results (FR-9 — behaviour cannot drift; the spine §4.2 property at the L7 boundary).
- v1.1: `tests/surface_growth.rs` — new discriminators (comments/scheduler/coordination/gates/saved) each bump `CONTRACT_VERSION`; tool count still ≤ 8 (RK-3 assertion).
- v1.2: `tests/sync_resources.rs` *(feature-gated)* — sync-status resource over a `wiremock`'d remote.
- v1.3: `tests/batch_stream.rs` — batch ordering, streaming/subscription resources, `max_batch` enforcement.

**proptest (NFR-16):** DTO↔model conversions are total and lossless on supported fields (`tools/dto.rs`, `tools/query.rs`); quota classification is monotonic.

**insta snapshots (NFR-14, CI `--check` gate):** every tool `JsonSchema`, the capabilities doc, the schema bundle, the error-code map, and each prompt's rendered messages. These ARE the `contract_version` drift detectors (FR-12).

**criterion benches (`benches/`, v1):** `bench_query_ready` (tool-dispatch + schemars-validate + serialize overhead **above** the engine — must be a thin slice of NFR-1's `ready` budget); `bench_serialize_large_list` (10k-issue `ToolOutput::Issues` serialization). Purpose: prove the MCP adapter adds negligible latency over `Session` (RK-2 thin-adapter claim). v1.3 adds `bench_batch`.

**fuzz (via `unblock-fuzz`, not a member here):** a fuzz target feeds arbitrary JSON tool-call payloads through schemars validation + DTO deserialization (the untrusted boundary, NFR-18) asserting no panic and that out-of-quota/invalid inputs are rejected before any `Session` call. Listed here for traceability; the target lives in `unblock-fuzz`.

---

## Open questions (specific to this crate)

- **OQ-1 (rmcp 1.0 API surface, RK-2):** the spine assumes `#[tool_router]`/`#[tool]` macro ergonomics and an `unblock://` resource-template API. Confirm rmcp 1.0's exact attribute-macro names, the `ServerHandler` trait shape, and how resource templates (`{id}`) are declared — pin the version and isolate any churn here. Does rmcp 1.0 give a built-in request-size limit, or must `Quotas` wrap the transport read?
- **OQ-2 (in-band vs. protocol error):** spine §5.6 says a failed tool call returns **valid JSON** (`ToolOutput::Error`) AND attaches rmcp tool-error `data`. Confirm rmcp 1.0 lets a tool both return content and signal `isError` — or do we choose one channel? (Default plan: return `ToolOutput::Error` content with `isError=true` + structured `data`.)
- **OQ-3 (label rename/list-all placement, v1.1):** do label `rename`/`list_all` ride the `issue` tool (an `action`) or `query` (a `kind`)? Affects which file grows and the v1.1 `contract_version` bump. Lean: `query kind: labels` for reads, `issue action: relabel` for the rename mutation — needs a decision before v1.1 build.
- **OQ-4 (gates surface, v1.1):** are workflow-gate verdicts (FR-19) enforced inside `issue.update` status transitions (returning `PolicyViolation`), or exposed as a separate `query kind: gates` read? The former keeps the tool count flat (RK-3) and matches "gates block transitions"; confirm with policy owner.
- **OQ-5 (read-snapshot cache):** the original (`mod.rs` `McpReadSnapshotCache`) cached read JSON keyed by a db/wal/jsonl mtime witness. With libsql WAL + the engine read fast path (FR-10), is an L7 read cache still worth it, or does it belong in the engine? Default: **omit at L7 in v1**; revisit if NFR-1 `ready` budgets need it.
- **OQ-6 (new transports, v2+):** roadmap §5 lists HTTP/SSE as a later direction under the same isolation discipline (D2 locks stdio primary). Reserve a `src/transport/` seam now, or defer entirely? Default: defer; `serve()` signature is already transport-agnostic enough to extend.
