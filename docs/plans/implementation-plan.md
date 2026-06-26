# unblock — Implementation Plan (v1)

- **Status:** APPROVED. (Genuinely-deferred Open Items remain marked inline.)
- **Date:** 2026-06-19
- **Source of truth:** `docs/PRD.md` (PRD APPROVED v1.1). This plan operationalizes PRD §13 (Phasing & Milestones).
- **Scope:** the v1 vertical walking skeleton (thin slice). v1.1-deferred features are out of this plan except where a seam must be left for them.

> Crates are **not** created yet (per Miguel). This plan describes the work; crate scaffolding is the first
> task executed *after* the plan is approved (T0.2). Task ids (T-M.n) are designed to map 1:1 onto epics/beads.

---

## 0. Guiding approach

- **Vertical walking skeleton.** Build the narrowest end-to-end path first (model → storage → engine → MCP),
  then widen. Avoid a horizontal big-bang across 12 crates.
- **Validate the riskiest assumption first (RK-1).** Before any crate depends on storage, prove libsql's
  WAL + native `busy_timeout` does **not** hot-spin under contention (NFR-3). This is the M0 exit gate.
- **One mutation home.** All writes flow through `unblock-engine`'s in-process serialized writer (D14); MCP
  and CLI are thin adapters over it (FR-9) so they cannot drift.
- **Contract-first storage.** The `Storage` trait + its contract test suite (NFR-16) are written alongside the
  libsql impl, so a future backend is swappable without touching callers.
- **Acceptance-driven.** Every task closes against the AC of its PRD FR(s); no task is "done" without its gate green.

## 1. Workspace conventions (apply to every crate)

- **Edition** 2024, **MSRV/toolchain** stable `1.96.0`. **Lints:** `unsafe_code = "forbid"`, `missing_docs = "warn"`, clippy pedantic (workspace-level).
- **Errors:** `snafu`, **per-crate** error enum with context selectors; no cross-crate error leakage; map to MCP error data / 0–8 exit codes only at the L7 boundary (`unblock-error` owns the taxonomy + exit-code table).
- **Async:** tokio throughout; `Storage` is an `async_trait`.
- **Layering (NFR-15, enforced):** L0 `model`/`error` → L1 `policy` → L2 `storage` → L3 `sync`/`health` → L4 `config` → L5 `engine` → L6 `render` → L7 `mcp`/`cli`. `unblock-storage` depends on **model+error only**. Add a CI check (e.g. `cargo-deny`/a graph assertion) that fails on a back-edge.
- **Testing per crate:** unit tests; `proptest` for invariants; `insta` snapshots (CI `--check` gate); `cargo-fuzz` for ingestion; `criterion` for perf-sensitive paths; `wiremock` for any remote path.

## 2. Milestone M0 — Foundation  *(exit gate: Storage contract suite green + contention lab proves no hot-spin)*

- **T0.1 — Workspace scaffold deps.** Update root `Cargo.toml`: add `libsql` (features = `["core"]` — local; SQLite is **statically bundled by `core`** — there is no separate `bundled` Cargo feature; **remote/replica behind the non-default `remote` feature**, D15), `clap` (stable features only), `axoupdater` (behind the cli `self-update` feature), `backon` (remote-retry); swap archived `backoff` → `backon`; **drop direct `reqwest`** (now transitive-only via libsql `remote` + `axoupdater`); change `unsafe_code` `deny` → `forbid`; confirm `resolver = "2"`, workspace lints, `default-members` (excludes `unblock-fuzz`). **AC: `cargo build --workspace --locked` on stable 1.96 with `libsql`+`rmcp`+`clap`+`axoupdater`+`backon` co-resolved; `-p unblock-storage`/`-p unblock-engine --features remote` compile; `-p unblock-cli --no-default-features` drops `axoupdater`; `cargo tree -p unblock-storage` network-free on the default build.** *(**MERGED with T0.2** per Miguel — the 12 crates are created in the same branch so co-resolution runs against a real dependency graph, not an empty manifest.)*
- **T0.2 — Create crates with layering.** The 12 `unblock-*` crates (+ an `xtask` tooling crate) as workspace members with the dependency edges from PRD §8.1; add the layering check. *(**MERGED with T0.1** — same branch/PR.)* `petgraph` is a **private** dep of `unblock-storage` (backs `detect_cycles`/`dependency_tree`; sub-decision — the engine forwards via `Session`, it does not run petgraph — see T1.6). The layering check is `cargo xtask check-layering`, which reads `cargo metadata` (resolved graph, incl. feature-gated `dep:` edges) and asserts the PRD §8.1 + spine §0 allowed-edge matrix; T0.9 wires it into the CI `layering` job. `unblock-fuzz` is a member but **not** a default member (its nested cargo-fuzz package needs nightly, kept off the stable default build).
- **T0.3 — `unblock-model`.** Domain types (Issue + all enums: Status/Priority/IssueType/DependencyType/EventType; Dependency, Comment, Event, EpicStatus); `content_hash` (canonical, not serialized) + `sync_equals` + tombstone logic; `IssueValidator`. Pure, no I/O. *(AC: FR-1 round-trip types; proptest on content_hash stability + priority/enum parsing.)*
- **T0.4 — `unblock-error`.** `snafu` taxonomy, `ErrorCode`, structured JSON error payload (`code`/`message`/`hint`/`retryable`), 0–8 exit-code table. *(AC: golden snapshot of every code → exit code, FR-11.)*
- **T0.5 — `unblock-storage`: `Storage` trait.** Async trait covering the v1 operations (create/get/update/delete; list/ready/blocked/search/count/stale; dep add/remove/list; events). Backend-agnostic error.
- **T0.6 — `unblock-storage`: libsql impl.** Schema + migrations; WAL mode; `busy_timeout > 0` (native, non-spinning); prepared queries; transactional mutate (rows + audit events). Remote feature gated off by default (D15).
- **T0.7 — Storage contract test suite (NFR-16).** Backend-independent suite exercising the trait; runs against the libsql impl. *(AC: all green; reusable for a future backend.)*
- **T0.8 — Contention lab (RK-1 / NFR-3).** Harness that drives N concurrent writers and asserts (a) correctness (no lost writes) and (b) **no 100% CPU hot-spin** with WAL + `busy_timeout`. *(AC: defect-243 cannot recur; this gates M1. If it fails, the fallback is `rusqlite` behind the same `Storage` trait — a swap, not a rewrite — after revisiting D14/D15.)*
- **T0.9 — CI scaffolding (M0).** Author `.github/workflows/ci.yml` with the **11 M0 jobs** (ci-cd §2): `fmt` / `clippy` (workspace + a targeted `-p unblock-storage --features testkit` step) / `test` (`--workspace`, the always-on set) / `storage-testkit` (`--features testkit --test contract` NFR-16 + `--test contention_lab` the M0 contention gate, **≥ 2 vCPU**) / `snapshots` (`insta --check`) / `layering` (`cargo xtask check-layering`) / `audit` (`cargo audit`) / `deny` (`cargo deny check`) / `toolchain` (pin 1.96 + build `--locked`, NFR-12) / **`doc-lint`** (`cargo xtask doc-lint`, ci-cd §2.1). Plus a nightly **`fuzz-smoke.yml`** (`schedule` + `workflow_dispatch`): nightly-`2026-04-01` (= rustc 1.96.0-nightly, >= the stable 1.96 target — edition 2024 + let-chains) libFuzzer over the 8 fuzz targets + a separate stable-1.96 step running the two `#[ignore]`d contention-lab controls (forced-spin, WAL-negative) so the gate stays proven non-vacuous. `deny.toml` (licenses + no-git ban `git2`/`gix`/`libgit2-sys` + transitive budget), `.cargo/config.toml` (already present from T0.2), `rust-toolchain.toml` (1.96, already present). **Four jobs are DEFERRED with their gating task:** `bench-gate` → T3.5, `scale` → T3.5, `no-network` → T3.1/T3.6, `rate-limit` → T3.4/T3.5 (they need a `benches/` suite / the 250k corpus / the cli binary + axoupdater / the rate-limit harness). All `uses:` SHA-pinned to a 40-char commit with a trailing `# vX.Y.Z` (NFR-9). **Targeted features, not `--all-features`** — `--all-features` pulls the libsql `remote` TLS stack (`reqwest`/`hyper`/`rustls`), which D15/NFR-10 keep off the M0 gate (ci-cd §2.2). Depends on T0.2. *(AC: CI green on the current workspace; the doc-lint runs and passes; `cargo deny check` passes with the no-git ban proven non-vacuous.)*

## 3. Milestone M1 — Engine + core domain  *(exit gate: CRUD/ready/dep linearizable via internal engine API)*

- **T1.1 — `unblock-policy`.** Pure decision contracts needed by v1: ready/hybrid-sort ranking, dependency→ready gating rules, validation/inheritance helpers, cache-key contract (cache-key type lives in `unblock-model`). Versioned where applicable. *(Side-effect-free; unit + proptest.)*
- **T1.3a — `unblock-config` (v1-MINIMAL subset) — lands BEFORE T1.2.** Delivers exactly the context interface the engine consumes (spine §4 CF-D): `WorkspaceContext` + `ResolvedContext` + `ConfigError` + `ResolvedConfig` (defaulted VALUES) + `ConfigPaths` (config-owned paths — config OWNS path resolution from T1.3a), plus the two facades `open_with_storage(start: &Path)` / `open_workspace(start: &Path)`. No layered precedence engine yet (`ResolvedConfig` is built from defaults; the full resolver is T1.3). **Sequenced before T1.2** because the engine *consumes* `WorkspaceContext` and config is **L4** — it cannot depend on the engine at **L5** (the build dep edge is engine→config; `cargo xtask check-layering` rejects the back-edge). *(AC: workspace `.unblock/` upward discovery from a `start` path; path resolution into `ConfigPaths` (`unblock_dir = workspace_dir/.unblock`; `db_path`/`jsonl_path` derived from `unblock_dir` + the `ResolvedConfig` filenames); **libsql open via `unblock_storage::LibsqlStorage::open_local(db_path)`, THEN migrate via `Storage::migrate()`** — two explicit calls, `open_local` does **NOT** run migrations (`DbOpenFailed` wraps `open_local`, `MigrationFailed` wraps `migrate()`); `Arc<dyn Storage>` build; actor resolution precedence `UNBLOCK_ACTOR` env → `$USER` → `"unblock"`; the resolve-only `open_workspace` facade does **NOT** open the DB; **network-free**.)*
- **T1.2 — `unblock-engine`: session lifecycle + writer.** `open → (optional import) → mutate → (optional export) → recover`; **in-process tokio `Semaphore`** for writes (D14); read fast path (FR-10); shutdown/logging plumbing seams. *(AC: property test — interleaved mutations linearizable, FR-9.)*
- **T1.3 — `unblock-config` (v1 FULL: layered TOML/env/CLI precedence resolver, additive over T1.3a).** Adds the layered resolution **CLI > env (`UNBLOCK_*`) > project `.unblock/config.toml` > defaults**; startup-vs-runtime partitioning; replaces the T1.3a defaulting internals behind the same public `ResolvedConfig`/context types and **adds the `_with_cli` facade overloads** (the `&Path` facades stay — FORK-1 OVERLOAD model, spine §4) — **no public-type/spine-signature change**. (DB-config-table + user-config layers = v1.1.) *(AC: precedence unit tests, FR-13 subset.)*
- **T1.4 — Issue lifecycle (FR-1a/1b/1c, FR-2, FR-3) via engine.** create/quick-create; show/update (**single-id** engine `Session::update`, labels set/add/remove, reparent w/ cycle reject — the **multi-id** loop is the MCP `issue` adapter at T2.3, spine §4.1:882); delete (tombstone/cascade/hard/dry-run); atomic `claim`; defer/undefer. *(AC: each FR's AC; claim race test via the contention lab.)*
- **T1.5 — Querying (FR-4) via engine.** list (filters), ready (default-complete, hybrid sort), blocked, search (cap 50, overridable), count (group-by), stale. *(AC: ready excludes blocked/deferred/closed; filters compose.)*
- **T1.6 — Dependencies & graph (FR-5).** typed edges; add/remove/list/tree/cycles. **Graph ops live in `unblock-storage`** (the `Storage` trait owns `detect_cycles`/`dependency_tree`/`dependency_graph`, spine §3.2): `petgraph` traversal + cycle detection (reject `blocks` cycles with the cycle path) is a **private** storage concern. The **engine forwards via `Session`** — it does not depend on `petgraph` itself. *(AC: edge change reflected in ready immediately.)*
- **T1.7 — Issue restore/undelete (FR-1c "recoverable").** A dedicated `Session::restore(id)` (+ a storage-trait op + spine §4 method + MCP `issue` `restore` action at T2.3) that un-tombstones a soft-deleted issue: restore `original_type`→`issue_type`, clear `deleted_at`/`deleted_by`/`delete_reason`, emit an audit event. Resolves the FR-1c "recoverable" AC — the tombstone-not-patchable rule (bd-inherited) stays; restore is the explicit recovery path. *(Spawned from T1.4 Gap-2, Miguel decision. Open design Qs: cascade-restore of tombstoned children? restore-to-which-status? expired-tombstone retention handling? Add the PRD §4 D-id + FR-1c AC clarification in T1.7's spec.) Depends on T1.4.*

## 4. Milestone M2 — MCP surface (primary)  *(exit gate: an MCP client completes ready → claim → close; bd dogfood data imports)*

- **T2.1 — `unblock-render` (reduced).** json/robot/plain/csv/markdown behind a trait; structured stdout / diagnostics stderr (NFR-14); snapshot-stable shapes. (TOON feature-gated stub = v1.1.)
- **T2.2 — `unblock-mcp`: rmcp stdio server.** `unblock serve` on `rmcp 1.7` (`server`, `transport-io`); `#[tool_router]`; capabilities (tools/resources/prompts); errors → MCP error data mirroring exit codes (FR-11/FR-20). Thin adapter over `unblock-engine`.
- **T2.3 — MCP tool/resource/prompt taxonomy (closes PRD §12.2).** Implement the consolidated v1 surface in §6 below. *(AC: tool count ≤ target; args `schemars`-validated with size/rate limits, NFR-18; bulk-markdown parse splits into N `Create` calls and a malformed block is rejected with code+hint **pre-mutation** (FR-1a).)*
  Bulk markdown import (PRD FR-1a) lands here as a thin MCP parser that splits the markdown into per-issue
  records and **loops `Session::create`** — NOT an engine primitive (no `Session`/model bulk-create; the
  engine surface is single-issue `create`). CLI ingestion mirrors this at T3.1.
  The `reopen` action maps to `Session::update(status: non-terminal)`; T2.3 must NOT expect a missing
  `Session::reopen` method.
- **T2.4 — `unblock-sync` (light).** Optional JSONL export/import: atomic temp+fsync+rename (NFR-4); import validates lines, rejects conflict markers + malformed JSON pre-mutation, path-confinement (FR-7/FR-8/NFR-7/NFR-8). No git, no merge.
- **T2.5 — `bd` one-shot import (FR-26/D16).** bd-export → `issues.jsonl` → import mapping bd fields → unblock model; report migrated/dropped counts; idempotent via `content_hash`. *(AC: a real dogfood `bd` repo imports.)*
- **T2.6 — Self-describing contracts (FR-12).** `capabilities` + `schema` resources versioned by `contract_version`.
- **T2.7 — Diagnostics pure-DB (FR-15).** stats/info/where/version/lint; **changelog from closed-issue metadata; orphans from `external_ref` commit-pattern match — no git read** (keeps NFR-6 static gate green).

## 5. Milestone M3 — Reliability + ops  *(exit gate: shutdown/failure-injection + perf budgets green)*

- **T3.1 — `unblock-cli` (lifecycle).** `unblock` binary: `serve`, `migrate`, `doctor`, `version`, `init`, `agents`, `update` (D3 widened; see PRD §13 M3 row); thin routing; CLI flag-forwarding into config resolution (FR-13 CLI half); tracing setup; exit-code policy; static completions (v1.1 expands). `clap` stable. *(AC: FR-14 — `init`/`agents` bootstrap a workspace `.unblock/` + agent scaffolding; FR-13 partitioning test — startup-only config (DB path, migrations) is read once at open and never re-read at runtime, while runtime config is resolved per-command. `unblock update` lives behind the `self-update` Cargo feature, per CF-K.)*
- **T3.2 — Cooperative shutdown (FR-17).** SIGINT/SIGTERM/SIGHUP → atomic flag; serve unwinds + flushes/closes libsql; second signal → async-signal-safe exit; Windows no-op. *(AC: SIGTERM mid-write leaves no WAL corruption — failure-injection test, NFR-5.)*
- **T3.3 — `unblock-health` (lite, FR-16).** `doctor` + libsql `integrity_check` + basic diagnostics. (Full taxonomy = v1.1.)
- **T3.4 — Reliability gates (NFR-4/NFR-5/NFR-13).** failure-replay, export/import failure-injection, long-lived single-workspace stress, interleaved concurrent command-family integrity. *(NFR-13 named test: `tracing_capture_reliability` (in `unblock-engine` or `unblock-sync`, via the `tracing-test` subscriber layer) asserts a guard activation emits an INFO event on the `unblock.reliability` target carrying the required `operation`/`path`/`result`/`reason` fields.)*
- **T3.5 — Performance budgets (NFR-1/NFR-2).** `criterion` baselines + 10% regression gate; 250k-issue CI corpus under the single-serve topology; record agent round-trip latency (PRD §14 metric).
- **T3.6 — Release pipeline + `unblock update` (FR-25, D17).** `dist` config in the workspace manifest; generated GitHub release workflow (6 triples, shell+powershell, attestations); embed `axoupdater` for the `unblock update` command (behind the `self-update` Cargo feature; feature name ≠ command name, per CF-K); SHA-pin the generated workflow's actions. *(AC: a tagged release produces verified artifacts + installers; `unblock update` upgrades from a prior release with attestation verification. See `ci-cd-and-distribution.md`.)*
- **T3.7 — Product `README.md` (root).** Write the top-level `README.md`: what unblock is, install (dist installers), how to wire it into an MCP client (`unblock serve --dir`), and the `unblock` lifecycle commands. *(AC: a new user can install unblock and wire it into an MCP client from the README alone.)*

## 6. MCP surface — concrete v1 taxonomy (closes PRD §12.2)

Consolidated to keep the client tool list small (target **≤ 8 tools**); read-heavy state is exposed as resources.

**Tools**
1. `issue` — `action`: create | show | update | close | reopen | delete (+ quick-create via `create` returning id only). [FR-1a/1b/1c]
2. `claim` — atomic assignee + in_progress. [FR-2]
3. `defer` — `action`: defer | undefer. [FR-3]
4. `query` — `kind`: list | ready | blocked | search | count | stale (+ filters). [FR-4]
5. `dep` — `action`: add | remove | list | tree | cycles | graph. [FR-5]
6. `sync` — `action`: export | import | import_bd. [FR-7/FR-8/FR-26]
7. `diagnostics` — `kind`: stats | info | where | version | lint | changelog | orphans. [FR-15]

**Resources**
- `unblock://issues/{id}`, `unblock://issues/ready`, `unblock://issues/blocked`, `unblock://capabilities`, `unblock://schema`. [FR-4/FR-12]

**Prompts**
- `triage`, `plan_next_work`, `close_with_suggestions`. [FR-20]

*(v1.1 adds tools/resources for labels/comments, scheduler, coordination, gates, saved-queries.)*

## 7. Cross-cutting (all milestones)

- **CI/quality:** `cargo fmt --check`, clippy pedantic deny-on-CI, `cargo-audit` + `cargo-deny` (also enforces NFR-10 transitive budget + no-git NFR-6 + maintained-retry-crate NFR-3), `insta --check`, layering back-edge check, GitHub Actions pinned to 40-char SHAs (NFR-9). **From M0.**
- **Standing NFR-9 re-pin (every `dist`/workflow-generator upgrade).** Whenever `dist` (cargo-dist) is bumped and regenerates the release workflow, re-pin every regenerated action reference back to a 40-char SHA before merge (the generator emits floating tags). This is a recurring guard, not a one-time T3.6 step; the SHA-pin lint gates the bump PR.
- **Release/distribution:** `dist` (cargo-dist) at v1 GA — 6 target triples, shell+powershell installers, GitHub artifact attestations, `axoupdater`-backed `unblock update` command (behind the `self-update` Cargo feature, CF-K; FR-25/D17). Full plan in `ci-cd-and-distribution.md`.
- **Human setup (one-time, M3):** repo secrets/permissions for release are configured in GitHub by a human (not Claude) — `AXOUPDATER_GITHUB_TOKEN`, attestation permissions (`id-token: write`, `attestations: write`), and (deferred) a homebrew-tap token.
- **Security (NFR-18):** schemars validation + size/rate limits on every MCP tool arg; no libsql tokens in `config.toml` (env/keychain only); blast-radius confined to the workspace.
- **Observability:** `tracing` on `unblock.reliability` target (NFR-13); stdout/stderr discipline (NFR-14).
- **Fuzz:** `unblock-fuzz` targets over model/sync/storage ingestion (NFR-16).

## 8. Critical path & sequencing

```
T0.1 → T0.2 → {T0.3 model, T0.4 error} → T0.5 trait → T0.6 libsql → T0.7 contract suite
                                                                   → T0.8 contention lab  [M0 GATE / RK-1]
M0 → T1.1 policy → T1.3a config-min → {T1.2 engine → {T1.4 lifecycle → T1.7 restore, T1.5 query, T1.6 deps}, T1.3 config-full}   [M1 GATE]
M1 → T2.1 render → T2.2 mcp → T2.3 taxonomy → {T2.4 sync, T2.5 bd import, T2.6 contracts, T2.7 diagnostics}  [M2 GATE]
M2 → {T3.1 cli, T3.2 shutdown, T3.3 health-lite, T3.4 reliability, T3.5 perf}            [M3 GATE → v1]
```

- **Hard gate:** T0.8 (contention lab) must pass before M1. If libsql fails the non-spin assertion, stop and re-open D14/D15 before building upward.
- **Parallelizable within a milestone:** the brace `{…}` groups can proceed concurrently once their milestone's prerequisite tasks are done.

## 9. Definition of done (v1)

All M0–M3 exit gates green; every v1-tier FR meets its AC (PRD §5); NFR ship-gates (NFR-1/2/3/4/5/6/9/10/11/12/13/14/15/16/17/18) pass; the dogfood gate met (unblock manages its own issues, imported from `bd` via FR-26). v1.1 backlog: FR-6, FR-13(full), FR-16(full), FR-18, FR-19, FR-21, FR-22, FR-23, TOON. (FR-25 self-update moved to v1 via dist/axoupdater, D17.)
