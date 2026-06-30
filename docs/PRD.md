# unblock — Product Requirements Document

- **Status:** APPROVED (v1.1) — §12 closed; CI/CD + distribution via `dist` added (D17); plans in `docs/plans/`
- **Date:** 2026-06-19
- **Owner:** Miguel Ramos
- **Repo:** `websublime/unblock`
- **Stage:** Pre-1.0, no external users — breaking changes welcome, no migration or backward-compat burden.

> **unblock** is a ground-up, idiomatic, multi-crate Rust rewrite of the deprecated `beads_rust` tool
> (binary `br`, an "agent-first issue tracker"; source under `temp/beads_rust-main`). It is grounded in a
> code-confirmed discovery of the original (25 functional / 17 non-functional requirements, 11 domain
> entities) and reshaped by explicit, locked product decisions (§4).
>
> **Value proposition:** *unblock is the only local, offline-capable, dependency-aware issue store with
> atomic multi-agent claim, a versioned dependency-aware scheduler, and contention-safe swarm coordination
> at 250k+ issues — no accounts, no internet, with a credible shared-state path via libsql sync.*

---

## 1. Overview & Vision

unblock is a **local-first, agent-first issue tracker** purpose-built so AI coding agents — and the humans
orchestrating swarms of them — can keep **dependency-aware, machine-readable issue state next to the code**,
with no accounts and no external service required.

It inverts the original product's interface: the **Model Context Protocol (MCP) is the primary surface**.
Every domain feature is exposed as MCP tools/resources/prompts over stdio; the command-line binary exists
only for **lifecycle/operations** (serve, migrate, doctor, version, init, agents, update — D3), not as the feature surface.

Persistence is a **libsql** (Turso SQLite fork) database — the **source of truth** — accessed behind a
`Storage` trait, local-file by default with a native path to remote/replicated sync later. A line-oriented
JSONL export/import is retained as an **optional** portability/audit feature (git-diffable snapshots), **not**
as a synchronization mechanism.

### 1.1 Competitive context

| Alternative | Why unblock instead |
|---|---|
| **GitHub MCP server** (issues as MCP tools) | Requires internet + account; no dependency graph; no atomic claim; no offline. |
| **saga-mcp** (TS, SQLite, hierarchy + deps + audit) | No atomic claim; single-agent assumption; no swarm-coordination diagnostics; no scheduler; no performance validation; TS runtime overhead. |
| **Raw SQLite MCP server** | No domain model; no exit-code/error contract; no typed dependency edges. |

**Defensible wedge:** swarm-scale correctness at 250k+ issues — atomic multi-agent claim, contention-safe
coordination, a versioned agent error contract, and a dependency-aware scheduler. Everything else is table stakes.

## 2. Problem Statement

External trackers (GitHub Issues, Jira, Linear) require internet and accounts, fragment context away from the
code, cost money, and expose weak machine APIs; bare TODO comments carry no status, dependencies, or
queryability. AI coding agents working in swarms need **deterministic, non-interactive, dependency-aware**
issue state that lives with the repo, is offline-capable, and is machine-readable end to end.

The **trust anchor** is the local libsql database (durable, transactional, with a real sync protocol for the
shared case). Issues no longer need git as a transport; a git-diffable JSONL snapshot remains available as an
**optional, secondary** convenience for audit and review, not as the source of truth.

The original `beads_rust` delivered the agent value but accreted into an unmaintainable shape: a single
~50k-LOC binary crate with by-convention-only layering, monster files (`storage/sqlite.rs` ~22.6k LOC,
`sync/mod.rs` ~9.4k, `doctor.rs`), write-orchestration logic duplicated between the CLI and the MCP server,
and a deep dependency on niche single-author sibling crates — most critically the pre-1.0 pure-Rust
**fsqlite** SQLite engine (15 crates) whose error type leaked into the public API and which **hot-spins at
100% CPU on lock contention** (defect 243), forcing `busy_timeout=0` plus a hand-rolled backoff.

unblock delivers the same agent value as a **coherent, acyclic multi-crate workspace** with an embeddable
core, a mainstream storage backend behind a trait, MCP as the first-class interface, and the agent contract
preserved.

## 3. Personas

| Persona | Tier | Needs |
|---|---|---|
| **AI coding agent** | primary | Deterministic, non-interactive, always-valid structured output; structured errors (`code`/`message`/`hint`/`retryable`); the 0–8 exit-code taxonomy (CLI) / matching MCP error codes; atomic claim; a "what should I work on" query (`ready`/`scheduler`); discovery of contracts (`capabilities`/`schema`) versioned by `contract_version`; `discovered-from` dependency for the work flywheel; stale-claim coordination diagnostics; MCP stdio transport. |
| **Swarm orchestrator** | primary | Ranked, explainable ready-work scheduling (deterministic evidence `unblock.scheduler.v1`); read-only coordination status to diagnose hidden/stale `in_progress` claims (`unblock.coordination.v1`); contention resilience (no CPU hot-spin) at 250k–1M issues and thousands of agents. |
| **CI / external system** | primary | Scriptable structured output and exit-code categories; changelog from closed issues; orphan detection; workflow **gate** verdicts (`ci_green`/`min_reviewers`/`security_sign_off`) that block status transitions. |
| **Maintainer / rewrite engineer** | primary | Embeddable, testable core library API; enforced acyclic crate layering; mainstream swappable dependencies behind traits; a `Storage`-trait contract suite. |
| **Human developer** | **secondary / future** | Issue lifecycle and dependency planning. **Note:** under D3 (CLI has no domain features) and D7 (no rich rendering), daily domain work requires a **running MCP client**; a human in a bare terminal cannot create/list/close/dep directly. This persona is explicitly secondary in v1; a domain-CLI shim is out of scope (would contradict D3). |

## 4. Product Principles & Key Decisions

These decisions are **locked** (confirmed with Miguel) and shape the rest of this document.

| ID | Decision | Rationale |
|---|---|---|
| **D1** | **Storage = libsql** behind an async `Storage` trait. Local file now; remote/embedded-replica/synced available later. | Mainstream, maintained; gives local **and** remote with one async client → no separate "shared service". Replaces the niche fsqlite stack and its hot-spin defect. |
| **D2** | **MCP = rmcp over stdio, and MCP is the PRIMARY interface.** | rmcp is the official async SDK (`rmcp 1.7`, `server`+`transport-io`). Agent-first product → the agent protocol is the product. |
| **D3** | **CLI is reduced to lifecycle/ops only** (`serve`, `migrate`, `doctor`, `version`, `init`, `agents`, `update` — all lifecycle/ops, no domain features). All **domain features via MCP**. | "All features via MCP." The binary still needs a lifecycle surface; that is not the feature surface. |
| **D4** | **Errors = snafu**, **per-crate** error enums (no god-enum); mapped to MCP error data / exit codes at the boundary. | Idiomatic typed errors with context selectors; no backend error leakage. |
| **D5** | **libsql is source of truth; JSONL is an OPTIONAL light export/import feature.** Drop the heavy git-merge coordination (3-way merge, 4-phase collision detection, distributed locks, data-loss guards). | libsql provides real sharing; git-as-transport is redundant. Keep JSONL only for portable, diffable snapshots. Reversible toward DB-only later. |
| **D6** | **Whole stack is async (tokio).** | Consequence of rmcp + libsql; matches the scaffold (`tokio = full`). |
| **D7** | **MCP-only sheds the rich-rendering stack** (rich_rust, crossterm, indicatif); rendering is the MCP client's job. | Structured content over MCP; large supply-chain reduction. |
| **D8** | **Rename everything to `unblock`** — binary `unblock`, crates `unblock-*`, config dir `.unblock/` (**monorepo alias `_unblock/` also accepted on discovery** — the original beads `.beads`+`_beads` affordance for dot-dir-hostile environments; amended 2026-06-26), resource URIs `unblock://…`, contract ids `unblock.*.v1`. | New product identity. |
| **D9** | **Target stable Rust** (pinned `1.96.0`); drop clap dynamic/unstable features. | Removes the only reason the original needed nightly. |
| **D10** | **Config = TOML** (`.unblock/config.toml`); **single env prefix `UNBLOCK_`** (`UNBLOCK_ACTOR`/`UNBLOCK_DIR`/`UNBLOCK_JSONL`/`UNBLOCK_OUTPUT_FORMAT`). The resolved actor (precedence `--actor` > `UNBLOCK_ACTOR` > `config.toml [actor]` > `$USER` > `"unblock"`) is **bounded via `unblock_model::validation::validate_actor`** (≤ 200 chars + NUL/control-char rejection) before it can reach storage. | Idiomatic; avoids the unmaintained serde_yaml fork on an untrusted-input path; unifies BD_/BEADS_/BR_. |
| **D11** | **Drop** the town/mayor **cross-project routing**; keep single-workspace `.unblock/` discovery. | Elaborate and niche; reintroduce only on a concrete multi-repo need. |
| **D12** | **Keep** TOON output (feature-gated; **v1.1**), **keep** the compaction fields in the model — **rationale: JSONL round-trip fidelity, not Go-bd conformance.** *(In-binary self-update superseded by D17 — now `axoupdater` in `unblock-cli` at v1, no isolated crate; see FR-25.)* | Conservative; not dropped without need, but scoped out of v1 (see D13/§13). |
| **D13** | **No daemon self-install, no automatic git operations, no git library linked, no network on normal command paths.** | Preserves the non-invasive, offline-first stance. (The MCP stdio server is a foreground process launched by the client, not a self-installed daemon.) |
| **D14** | **v1 concurrency = single `unblock serve` per workspace owns the DB.** The engine single-writer discipline is **in-process only** (a tokio `Semaphore` in `unblock-engine`); agents scale by connecting as MCP clients to that one serve process. Concurrent external writers (CLI while serve runs, or multiple serve) are **best-effort** via SQLite WAL + `busy_timeout`, **not** the supported path; `migrate`/`doctor` run when serve is inactive. | Simplest correct model; makes NFR-2/NFR-3 testable; matches libsql's local SQLite backend. |
| **D15** | **libsql ships local-file-only / bundled by default; the remote/replica feature is OFF by default**, behind an explicit non-default Cargo feature, with a warning that enabling it may make network calls. | Preserves D13/NFR-17 ("no network on normal path") and keeps the TLS/HTTP transitive surface out of the default build (NFR-10/NFR-11). |
| **D16** | **v1 ships a one-shot, best-effort `bd`→unblock import** (bd-export → `issues.jsonl` → `import`). | A general bd→unblock migration capability for anyone with an existing `bd` repo. (unblock's own dev tracking is NOT in bd — it lives in `docs/plans/STATUS.md`.) Reuses the FR-7/FR-8 import path. |
| **D17** | **CI/CD & distribution via `dist`** (cargo-dist): 6 target triples (mac/linux/win × x86_64/aarch64), shell+powershell installers, **self-update via `axoupdater` in v1**, GitHub artifact attestations; CI quality gate from M0. | Mainstream release tooling; replaces the hand-rolled `self_update` stack; satisfies NFR-9/NFR-11/NFR-17. See `docs/plans/ci-cd-and-distribution.md`. |
| **D18** | **`blocked` composes the `list` narrowing facets (FR-4 "filters compose").** `blocked_issues` applies the same narrowing facet set as `list` (status-OR, `issue_type`-OR, inclusive `priority` range, `assignee`, `labels_all` AND / `labels_any` OR, `text_contains` title-only) to candidate rows **before** the live blocked-set membership test, making `blocked` conform to the FR-4 "filters compose" AC. Facets only NARROW — the 3-pass blocked detection and the `ORDER BY` are unchanged. `blocked`'s baseline stays **deferred-inclusive** (`status NOT IN ('closed','tombstone')`); it does NOT inherit `list`'s default visibility, so `include_closed`/`include_deferred` are no-ops on `blocked` (spine §3.2.1). | `blocked` was facet-agnostic only by omission (OQ-B), inconsistent with `list`/`search`/`count`/`stale`; honoring facets is agent-ergonomic + spine-consistent. No interface change (`Session::blocked(&filters)` already takes filters); no migration. |
| **D19** | **`detect_cycles` exposes a `blocking_only` parameter (gating-only vs all-dependency-type cycle detection).** The single `detect_cycles(blocking_only: bool)` (storage trait + `Session` forward) unifies the original's two methods: `blocking_only=true` restricts the cycle graph to the 4 gating types (`affects_ready_work`: `blocks`/`parent-child`/`conditional-blocks`/`waits-for`) — the ready-work view (= original `detect_blocking_cycles`); `blocking_only=false` detects cycles over **all** dependency types — the integrity/lint view (= original `detect_all_cycles`). The `dep cycles` MCP action surfaces it (default TRUE = gating-only); results are ordered cycle-path witnesses (shape pinned by spine §3.2.1/§5.3). | Agents need both the gating view (what blocks ready-work, the FR-5 AC) and the all-types view (dependency integrity/lint); the original `bd` split this across `detect_blocking_cycles(true)`/`detect_all_cycles(false)`, unblock unifies it via one param. Faithful to the original; no interface drift beyond the added param; no migration. |
| **D20** | **Live issue restore (un-tombstone) — net-new (no `bd` precedent).** A **dedicated** `Session::restore(id)` / `Storage::restore_issue(id, actor)` (NOT an `update` patch) reverses a SOFT delete (FR-1c "recoverable"). **(1) Target status = best-effort via `closed_at`:** restore lands `Closed` iff `closed_at.is_some()`, else `Open` — the pre-delete status is not preserved (only `original_type` survives), so `closed_at` is the only was-Closed signal; this also self-satisfies the issues-table CHECK constraint on both branches. Open and Closed round-trip; InProgress/Blocked/Deferred collapse to Open (lost, acceptable). **(2) Type fields:** `issue_type` is **UNTOUCHED** and `original_type` is **cleared → `None`** — explicitly **CORRECTING** the earlier "restore `original_type`→`issue_type`" framing in STATUS/impl-plan (that write is a no-op for local deletes and CORRUPTS imported rows whose serde-carried `original_type` diverges from `issue_type`; `into_tombstone` never mutates `issue_type`, so the live value on a tombstone is already correct). **(3) Audit:** emits a single `Event(Restored)` — NEVER `StatusChanged`/`Reopened` (a dedicated path; it does not reuse the generic update terminal→non-terminal rule). **(4) Scalar/non-cascading in v1:** restores ONE issue — no `--cascade`, no ancestor guard (the tombstone records no delete-batch provenance); cascade-restore is a **v1.1 seam**. **Idempotent error model:** already-active → no-op `Ok(issue)` (no event, no `updated_at`); missing/hard-deleted → `IssueNotFound` (no new ErrorCode minted — the unblock-error golden/exit-code/retryable set stays frozen). **TTL-agnostic in v1** (`deletions_retention_days` reserved/unenforced; TTL-refusal is a v1.1 seam). **ORTHOGONAL to the import no-resurrection invariant** (NFR-8): that guarantee is import-path-scoped; restore is the sanctioned, audited LIVE recovery path. | FR-1c's "recoverable" AC needs a real recovery op, but a tombstone cannot be reopened via `update` (the storage tombstone-patch guard fires first) — so restore is structurally a separate, audited engine path. `closed_at`-driven status is the only signal that survives a soft delete; clearing `original_type` (not writing it to `issue_type`) is the only correct inverse of `into_tombstone` and avoids corrupting imported rows. Scalar-only v1 avoids over-reviving independently-tombstoned children; the cascade/TTL seams shape the v1.1 delete-batch identity. |
| **D21** | **Issue-id generation scheme + atomic minting in the engine (FR-1a).** A faithful port of the classic `bd` adaptive-base36 id scheme, minted by the **engine** under the **D14 write permit**. **Prefix is CONFIG-DERIVED (default `ub`):** the issue-id prefix is **not** a hard-coded constant — it is resolved from `unblock-config` (an ADDITIVE config key, default `ub`, faithful to the original `IdConfig::with_prefix`/`issue_prefix`), normalized via the model `normalize_prefix`, and the engine allocator reads it from the `WorkspaceContext`/`ResolvedConfig` it already holds at mint time (so `ub-` in `ub-<hash>`/`ub-<slug>-<hash>` is the *default* rendering, overridable per workspace). **Creator seed:** the engine resolves the actor → `creator` and passes it as the seed's `creator` field at mint (`generate_id_seed(..., creator, ...)`, faithful to the original `IdGenerationInput.creator`). **(1) Root id = `ub-<hash>`:** SHA-256 over a length-prefixed seed (`len:title`, `len:description`, `len:creator/actor`, `len:created_at` nanos, `len:nonce`), first 8 bytes → `u64` → base36-lowercase, truncated to an **adaptive length** (`min=3..max=8`) chosen by the birthday-problem heuristic `P(collision) ≈ 1 − e^(−n²/2·36^len) < 0.25` over the current issue count; the candidate loop tries nonces `0..10` at a length, then **grows the length**, then the original's saturated fallback (12-char hash, then `…{nonce}` desperation), probing each candidate against storage via `get_issue(id).await?.is_some()` (the existence probe reuses the `get_issue` read — there is **no** `Storage::exists` method) — all **under the write permit** so the probe→insert is atomic. **(2) Slug variant `ub-<slug>-<hash>`** (optional user slug): the slug is normalized to lowercase ASCII-alphanumeric + single hyphens (cap 48 chars, trailing-hyphen-stripped) via the model's `normalize_slug`, then made **prefix-budget-aware** via `normalize_slug_for_prefix(slug, prefix)` — which fits `<prefix>-<slug>` within `MAX_ID_PREFIX_LEN` (=64), truncating the slug to the remaining budget (and re-trimming a trailing hyphen) or **dropping the slug entirely** (empty drop-signal) when the prefix alone exhausts the budget; the **hash suffix is always appended** (uniqueness); an empty-after-normalization slug (or a budget-exhausted drop) falls back to the hash-only `ub-<hash>` ladder. **(3) Hierarchical child id = `parent.N`** when a `parent` is supplied: `N` = storage's existing `next_child_number(parent)` (its `child_counters` high-water mark), then `create_issue` bumps the counter in-tx. **(4) Placement = engine, NOT L7:** the mint+probe+insert (and the child-counter read→bump) MUST be serialized under the single write permit so two concurrent creates under the same parent cannot mint the same `parent.N` — this is **why** minting cannot live in the CLI/MCP adapters; FR-9's single-mutation-home keeps MCP and CLI from drifting. The pure, deterministic candidate compute (hash/seed/adaptive-length/slug-normalize) lives in **`unblock-model`** (`id.rs`, alongside the parser); the **stateful** collision-retry loop + the storage existence probe (`get_issue(id).await?.is_some()`, NOT a `Storage::exists`) and the `next_child_number` read live in the **engine** id-allocator. **(5) `Session::create(&Issue)` (caller-supplied id) STAYS** — it is the **import/internal** path (it preserves caller ids so the `content_hash`-keyed import idempotency, FR-26, is byte-stable; it **never mints**). A **new** engine minting path (`create_issue(NewIssue)`) is added for **interactive** create (MCP/CLI), resolving `parent`→`parent.N`, adding `deps` as edges in/after the same tx, and returning the created `Issue`. | FR-1a needs a real id to mint, and the child-counter `next_child_number`→`+1` read-modify-write is a **race** unless serialized — so minting MUST run atomically under the D14 permit (an L7 adapter cannot guarantee that), and FR-9's single mutation home keeps MCP and CLI on one path. The `bd` adaptive scheme keeps ids short while collision-safe as the DB grows; the slug variant keeps ids human-recognizable. Keeping `create(&Issue)` separate preserves the import path's id-preservation (FR-26 idempotency), which a minting path would break. Splitting pure-compute (model) from the stateful probe (engine) keeps `unblock-model` I/O-free while reusing the existing `id.rs` parser home. |

> The locked §4 set was reviewed by three independent lenses; none is a blocking technical flaw. NFR-3 framing
> (CF-3) and FR-9 scope (CF-1) were clarified — see §6 and §8.2 — without reversing any decision.

## 5. Functional Requirements

> **Delivery (D2/D3):** FR-1…FR-23 are surfaced as **MCP tools/resources/prompts** (§9). CLI command names
> are the canonical vocabulary for tool/action naming and for the few lifecycle commands.
> **v1 tier** = the walking-skeleton thin slice (D14/§13). Items marked **[v1.1]** are deferred.
> Acceptance criteria (AC) below are the objective "done" gate for each v1 must-FR.

### Core issue management
> **FR-1 umbrella:** "FR-1" denotes the core CRUD requirement as a whole and is realized by the three
> sub-requirements **FR-1a** (Create), **FR-1b** (Read/Update), and **FR-1c** (Delete). Other docs may cite
> either the umbrella "FR-1" or a specific sub-id; both resolve here.
- **FR-1a [must] — Create.** create (type, priority, labels, parent, deps, due/defer, estimate, slug, bulk markdown import, `ephemeral`, attribution fields) and quick-create (returns id only). **The id is MINTED by the engine under the D14 write permit (D21):** a root `ub-<hash>` (faithful `bd` adaptive-base36 scheme) or, with a slug, `ub-<slug>-<hash>`; with a `parent`, the hierarchical `parent.N` via storage's `next_child_number`. The minting create path (`Session::create_issue(NewIssue)`) is **distinct** from the id-preserving `Session::create(&Issue)`, which the import/internal path uses (it never mints — preserves caller ids for FR-26 idempotency). *AC: every option round-trips through libsql and a `show` of the new id; quick-create returns only the id; invalid enum/priority yields a structured error with `code` + `hint`.*
- **FR-1b [must] — Read/Update.** show; update (multi-id; label add/remove/set; reparent). *AC: multi-id update is atomic per id; reparent rejects cycles; `updated_at` advances; no-op update is detectable.*
- **FR-1c [must] — Delete + restore semantics.** tombstone-based delete with cascade / hard / dry-run, plus a dedicated **restore** (un-tombstone, D20). *AC: default delete tombstones (recoverable); `--cascade` tombstones children; `--hard` removes rows; `--dry-run` mutates nothing and reports the plan; tombstones never resurrect **on import**.* **"Recoverable" AC (D20):** a dedicated, **idempotent** `restore` round-trips a SOFT-deleted issue back to active — status best-effort via `closed_at` (`Closed` iff `closed_at` set, else `Open`), `issue_type` untouched + `original_type` cleared, a single `Restored` audit event — and **re-enters the issue into the dependency graph with its surviving edges** (it may be immediately blocked). *Restore of an already-active issue is a no-op `Ok`; restore of a **hard-deleted** target is rejected (`IssueNotFound`). The live `restore` is the sanctioned recovery path and is DISJOINT from the import "tombstones never resurrect" clause (which is import-scoped).*
- **FR-2 [must] — Atomic claim.** A single mutation sets assignee **and** `in_progress` with no race window. *AC: under the contention lab, N agents claiming the same issue yield exactly one winner; losers get a deterministic "already claimed" structured error.*
- **FR-3 [must] — Scheduling / defer.** `defer`/`undefer` move issues out of and back into the ready set until a date. *AC: a deferred issue never appears in `ready` before `defer_until`; `undefer` restores it immediately.*

### Querying
- **FR-4 [must] — Query surface.** `list` (deep filters: status/type/assignee/label AND+OR/priority ranges/text-contains; structured + CSV), `ready` (unblocked + undeferred + hybrid-sorted; the agent entrypoint; **default-complete/unlimited**), `blocked`, `search` (full-text, default cap 50), `count` (group-by), `stale`. *AC: `ready` excludes blocked/deferred/closed; filters compose (on `list`, `count`, `search`, `stale`, **and `blocked`** — D18); `search` cap is overridable; outputs are snapshot-stable (NFR-14).* *(v1 "full-text" = case-insensitive `instr()` substring over title + description + id; ranked FTS5 deferred to v1.3 per spine §3.2.1.)*
- **FR-5 [must] — Dependencies & graph.** Typed edges (`blocks`, `parent-child`, `conditional-blocks`, `waits-for`, `related`, `discovered-from`, `replies-to`, `relates-to`, `duplicates`, `supersedes`, `caused-by`, `Custom`); dep add/remove/list/tree/cycles + graph view. Only `blocks`/`parent-child`/`conditional-blocks`/`waits-for` affect ready-work. Graph traversal/cycle detection via `petgraph`. The cycles view reports either gating-only or all-dependency-type cycles (`blocking_only`, D19). *AC: adding an edge that creates a `blocks` cycle is rejected with the actual ordered cycle path (naming every node in the cycle, e.g. `a -> b -> c -> a`, not endpoints-only); `ready` reflects edge changes immediately (an `add_dep` blocks an issue out of `ready`; a `remove_dep` re-admits it).*
- **FR-6 [v1.1] — Organization.** Labels (add/remove/list/list-all/rename), threaded comments (add/list), epic rollups with auto-close-eligibility.
- **FR-21 [v1.1] — Saved queries.** Named, reusable `list`-style filter sets. *(Demoted; re-evaluate whether agents have a use case post-D2/D3.)*

### Persistence & interchange (model B)
- **FR-7 [must] — Persistence + optional JSONL interchange.** libsql is the source of truth. Provide **optional** `export`/`import` to/from a line-oriented `issues.jsonl` for portability/audit/git-diffable snapshots. Export is atomic (temp in same dir → flush + `sync_all` → atomic rename; on error remove temp, leave original intact). **No** automatic three-way merge, **no** git operations. *AC: export is byte-deterministic for a fixed DB state; a killed export never corrupts the existing file (NFR-4).*
- **FR-8 [must] — Safe import.** Import validates each line, **rejects git conflict markers and malformed JSON before any DB mutation**, and confines the JSONL path (canonicalize; reject symlink escapes / `.git` / `..` / disallowed extensions). Tombstones never resurrected. *AC: a file with a conflict marker is rejected with zero DB writes; a symlink-escaping path is refused at preflight.*
- **FR-26 [must] — One-shot `bd` import (D16).** Best-effort import of any existing `bd`/beads repo's data via `bd-export` → `issues.jsonl` → `import`, mapping bd fields to the unblock domain model. *AC: a representative `bd` repo imports with a reported count of issues/deps/comments migrated and a list of any dropped/unmapped fields; rerunning is idempotent (dedup by `content_hash`).*

### Engine, contract & lifecycle
- **FR-9 [must] — Single shared engine/session.** One implementation of `open → (optional import) → mutate → (optional export) → recover`, consumed by **both** the MCP server and the CLI, so behaviour cannot drift. **Mutation serialization is in-process** (a tokio `Semaphore`), per D14; reads use a fast path. *AC: a property test shows interleaved mutations through the engine are linearizable; MCP and CLI produce identical results for the same operation.*
- **FR-10 [must] — Read fast path.** Read operations bypass the write semaphore. *AC: reads proceed while a write holds the semaphore (WAL readers).* 
- **FR-11 [must] — Agent contract surface.** Always-valid structured output; structured errors carrying `code`/`message`/`hint`/`retryable`; the 0–8 exit-code taxonomy (CLI) and matching MCP error codes; `close --suggest-next` returns newly unblocked issues; `ready` is the canonical discovery query. *AC: a golden snapshot pins every exit code and MCP error code; output is always valid JSON even on error.*
- **FR-12 [must] — Self-describing contracts.** `capabilities` and `schema` emit a versioned (`contract_version`) machine-readable description of tools, payload shapes, and error/exit codes. *AC: bumping a tool's schema bumps `contract_version`; a client can detect drift.*
- **FR-13 [must (subset) / v1.1 (full)] — Layered configuration.** v1 ships **CLI > env (`UNBLOCK_*`) > project `.unblock/config.toml` > defaults**; the DB config table and user-config layers are **[v1.1]**. Startup-vs-runtime key partitioning. *AC: precedence is unit-tested across all v1 layers.*
- **FR-14 [must] — Workspace bootstrap.** `init [--prefix]` creates `.unblock/`; `agents` injects/maintains `AGENTS.md`. *AC: `init` is idempotent; refuses to clobber an existing non-empty `.unblock/` without `--force`.*

### Health, reliability & shutdown
- **FR-16 [must (lite) / v1.1 (full taxonomy)] — Workspace health.** v1: `doctor` + libsql `integrity_check` + basic diagnostics. **[v1.1]:** the full Healthy/Drifted/Recoverable/Unsafe taxonomy with composite severity, redefined for a libsql-authoritative world (Recoverable → libsql integrity/WAL recovery, not JSONL; Drifted meaningful only when JSONL export is enabled). Recovery preserves evidence under `.unblock/.recovery/`.
- **FR-17 [must] — Cooperative shutdown.** Translate SIGINT/SIGTERM/SIGHUP into an atomic shutdown flag so the serve process unwinds cleanly and flushes/closes libsql; a second signal escalates to async-signal-safe exit; Windows no-op. *AC: SIGTERM mid-write commits-or-rolls-back cleanly and closes the DB with no WAL corruption (failure-injection test).*

### Coordination, gates, audit
- **FR-18 [v1.1] — Swarm coordination diagnostics.** `scheduler` ranks ready work with explainable deterministic evidence (`unblock.scheduler.v1`); `coordination status` (`unblock.coordination.v1`) read-only diagnoses hidden/stale `in_progress` claims. Pure versioned contracts. **Coordination is purely DB-state-derived; the upstream "Agent Mail" dependency is dropped** (§12).
- **FR-19 [v1.1] — Workflow gates.** Policy-driven (`.unblock/policy.toml`) transition gates (CI/reviewers report pass/fail; transition blocked until required gates pass). Project-local, not exported.
- **FR-22 [v1.1] — Audit / flight recorder.** Append-only `interactions.jsonl` with capture-only Tier-1 attribution (`agent_name`/`harness`/`model`), never enforced.

### MCP surface (primary) & diagnostics
- **FR-20 [must] — MCP stdio server (PRIMARY).** `unblock serve` exposes **tools** (issue lifecycle, claim, defer, query, dependencies, sync export/import; v1.1 adds labels/comments, scheduler, coordination, gates), **resources** (`unblock://issues/ready`, `unblock://issues/{id}`, `unblock://capabilities`, `unblock://schema`; v1.1 adds coordination/status), and **prompts** (triage, plan_next_work). Built on **rmcp** in the isolated `unblock-mcp` crate. *AC: an MCP client can complete ready → claim → close end to end; tool input is schema-validated (`schemars`) and rejects oversized/invalid args.*
- **FR-15 [must] — Diagnostics & info (pure-DB).** stats/status, info, where, version, lint; **changelog and orphans derive purely from DB state** — `changelog` from closed-issue metadata, `orphans` from issues whose `external_ref` matches a commit pattern. **No git is read or linked** (resolves the NFR-6 contradiction). *AC: the NFR-6 static gate passes with FR-15 present.*
- **FR-23 [v1.1] — Shell completions** (static, bash/zsh/fish/powershell/elvish).

### Distribution & self-update (D17)
- **FR-25 [must] — Self-update via `dist`/`axoupdater` (v1).** `unblock update` (a lifecycle command, D3) embeds `axoupdater`; updates are verified against GitHub artifact attestations before execution (NFR-17); never invoked on normal command paths. Distribution: shell + powershell installers across 6 target triples. The command lands in `unblock-cli` behind the `self-update` Cargo feature (the feature name enables the `unblock update` command; default-on, dropped under `--no-default-features` per CF-K). *(See `docs/plans/ci-cd-and-distribution.md`.)*

### Dropped (D11)
- **FR-24 [wont] — Cross-project town/mayor routing.** Single-workspace discovery only.

## 6. Non-Functional Requirements

- **NFR-1 [performance]** Storage targets with `criterion` baselines and a 10% CI regression gate: create <1ms; list 1k <10ms / 10k <100ms; ready 1k <5ms / 10k <50ms; export 10k <500ms; import 10k <1s. *(Re-baseline on libsql.)*
- **NFR-2 [performance]** Swarm scale **under the D14 topology** (one serve per workspace in v1; cross-machine/multi-dev sharing is the v1.2 libsql-remote path, writes serialized at the primary), with a read-only fast path. **v1 commits to 250k issues validated in CI** (ci-cd `scale` job, storage/engine; owner impl-plan T3.5). The **1M-issue / 10k-agent corpus is a v1.3 CI gate** (not a v1 acceptance gate); it is not part of the v1 ship-gate set.
- **NFR-3 [reliability/perf]** **The primary non-spin guarantee is libsql/SQLite WAL + the native `busy_timeout` (>0) handler + the in-process serialized writer — this resolves the fsqlite-243 hot-spin by construction.** App-level jittered backoff is a **secondary** fallback for the remote/replica path only; the chosen retry crate MUST pass `cargo-deny` (do **not** use the archived `backoff 0.4` — use `backon` or `tokio-retry`). A contention-replay lab proves no 100% CPU hot-spin. *(Validate libsql busy/lock semantics EARLY — see Risk Register.)*
- **NFR-4 [reliability]** Atomic JSONL export (temp + `sync_all` + atomic rename; remove temp, leave original intact on error), verified by failure-injection e2e tests.
- **NFR-5 [reliability]** Release reliability gates must pass: failure-replay, e2e export/import failure-injection, long-lived single-workspace stress, interleaved concurrent command-family integrity; emergency override requires a written reason.
- **NFR-6 [security]** unblock runs **zero git operations** and links **no git library**, enforced by a static gate (no `Command::new("git")`, no git crate). FR-15 is pure-DB and compatible with this gate.
- **NFR-7 [security]** JSONL writes confined to `.unblock/` by default; external paths require explicit opt-in with canonicalization and rejection of symlink escapes / `.git` / `..` / disallowed extensions; preflight before opening/parsing.
- **NFR-8 [security]** Import rejects git conflict markers and malformed JSON before any DB mutation; tombstones never resurrected; force flags mutually exclusive and never bypass syntax/conflict-marker validation. **The no-resurrection guarantee is import-path-scoped** — the live, audited `restore` op (D20) is the sanctioned exception (it un-tombstones a soft-deleted issue through the engine, not through import).
- **NFR-9 [supply-chain]** **`forbid(unsafe_code)`** in every crate (stronger than `deny`; update the scaffold); clippy pedantic; commit `Cargo.lock`; `cargo-audit`/`cargo-deny` in CI; pin every GitHub Action to a 40-char SHA (incl. the `dist`-generated release workflow). Release/distribution via `dist` (D17, `docs/plans/ci-cd-and-distribution.md`).
- **NFR-10 [supply-chain]** Minimize transitive surface: prefer mainstream multi-maintainer crates; eliminate the 15-crate fsqlite stack; keep network/TLS (`reqwest` is **transitive-only** behind libsql's `remote` feature + the cli `self-update`/`axoupdater` surface — never a direct dep; libsql remote) **behind non-default features** so it never appears on the normal path.
- **NFR-11 [portability]** Single self-contained `unblock` binary, no runtime system deps on Linux/macOS/Windows; path normalization; cfg-guarded unix signal handling.
- **NFR-12 [portability]** Build on **stable Rust** (`1.96.0`); nightly only for an explicit, documented reason (none currently). **Gate:** a CI job pins `rust-toolchain` to `1.96.0` stable and builds `--locked`; a green stable build is the NFR-12 acceptance gate (see ci-cd §2).
- **NFR-13 [observability]** Structured `tracing` on an `unblock.reliability` target (operation/path/result/reason): INFO for guard activations, force overrides, external path use, conflict markers; DEBUG for per-file/per-issue events.
- **NFR-14 [observability]** Clean structured output strictly on stdout; diagnostics strictly on stderr; snapshot-guarded stable output shapes (`insta`).
- **NFR-15 [architecture]** Enforced **acyclic** crate layering: pure leaf domain/policy crates depend only on model+error; storage hides its backend behind a trait with a backend-agnostic error; **no crate reaches into another's internals** (`unblock-storage` depends only on model+error — see §8.1).
- **NFR-16 [testability]** Per-crate unit tests; `proptest` over lifecycle/content-hash/import round-trip; `insta` snapshots with a CI check gate; `cargo-fuzz` over the ingestion surface; a `Storage`-trait **contract suite** validating each backend independently; `wiremock` for any remote/network path.
- **NFR-17 [reliability/security]** Distribution/self-update via `dist`/`axoupdater` (v1): artifacts carry **GitHub artifact attestations** and are verified before execution; **no network calls on any normal command path** — only on explicit `unblock update` (offline-first; libsql remote behind a non-default feature, D15). See `docs/plans/ci-cd-and-distribution.md`.
- **NFR-18 [security — threat model]** The MCP tool surface is an untrusted-input boundary: all tool args are `schemars`-validated with size/rate limits; a malicious/buggy agent is bounded to its own workspace's data (no path escape, no host command execution); libsql remote credentials (when used) are **never** stored in `config.toml` — only via `UNBLOCK_*` env or OS keychain. Tie safe drift to `contract_version` (FR-12).

## 7. Domain Model

| Entity | Key fields | Notes |
|---|---|---|
| **Issue** | id (prefix + optional slug + hash), content_hash (canonical dedup, not serialized), title, description, design, acceptance_criteria, notes, status, priority (0–4), issue_type, assignee, owner, estimated_minutes, created_at/by, updated_at, closed_at, close_reason, closed_by_session, due_at, defer_until, external_ref, source_system/repo/repo_path, agent_context, tombstone fields, compaction fields, sender, ephemeral, pinned, is_template; relations: labels, dependencies, comments | Core work item. `content_hash` + `sync_equals` drive dedup/equality. Compaction fields **kept for JSONL round-trip fidelity** (D12), not Go-bd conformance. |
| **Status** | open, in_progress, blocked, deferred, draft, closed, tombstone, pinned, `Custom(String)` | Open enum; only some dep types gate ready-work. |
| **Priority** | newtype i32 in 0..=4 (0=Critical … 4=Backlog); parses `P0`/`0` | Surfaced in hybrid ready sorting. |
| **IssueType** | task, bug, feature, epic, chore, docs, question, `Custom(String)` | Open enum; epic participates in rollups. |
| **Dependency** | issue_id → depends_on_id, dep_type, created_at/by, metadata (JSON), thread_id | `discovered-from` is central to the agent flywheel. |
| **Comment** | id, issue_id, author, body, created_at | Threaded. [v1.1] |
| **Event** | id, issue_id, event_type, actor, old_value, new_value, comment, created_at; Tier-1 attribution (agent_name/harness/model) | Append-only audit; attribution capture-only; written transactionally inside mutate(). |
| **EpicStatus** | epic id, total_children, closed_children, eligible_for_close | Derived rollup. [v1.1] |
| **WorkspaceHealth / AnomalyClass** | classification, anomaly code+severity, composite severity = max | Full taxonomy [v1.1]; v1 ships libsql integrity + doctor. |
| **GateResult / PolicyDocument** | gate name, provider, status, required gates per transition | Project-local (`.unblock/policy.toml`), not exported. [v1.1] |
| **On-disk artifacts** (`.unblock/`) | `unblock.db` (libsql, source of truth), `config.toml`, `policy.toml` [v1.1], `interactions.jsonl` [v1.1], `issues.jsonl` (optional export) | libsql is authoritative; `issues.jsonl` is an optional snapshot (D5). **No separate `metadata.json`** — startup-path keys (db/jsonl filenames, retention, backend) fold into `config.toml` (Q2, 2026-06-26; names §12.5). |

## 8. Architecture

### 8.1 Crate decomposition (acyclic, bottom-up)

| Crate | Layer | Responsibility | Depends on |
|---|---|---|---|
| `unblock-model` | L0 | Pure domain types; content-hash / sync-equality / tombstone logic; validation; shared contract types (e.g. cache-key). No I/O. | error (single sanctioned L0 edge — CF-G: `FromStr::Err` / `IssueValidator` return `unblock_error::ModelError`; `error` has no in-workspace deps so L0 stays acyclic) |
| `unblock-error` | L0 | **snafu** error taxonomy; structured error payloads; 0–8 exit-code table. Backend-agnostic. | — |
| `unblock-policy` | L1 | Pure versioned decision contracts: scheduler, coordination, gates/close-policy, inheritance, cache. Side-effect-free. | model, error |
| `unblock-storage` | L2 | `Storage` trait + **libsql** implementation (schema/migrations, queries, transactions, WAL + `busy_timeout`). Only crate aware of the backend. **Depends on model+error only** (CF-11 fix). | model, error |
| `unblock-sync` | L3 | **Light** JSONL export/import + atomic write + path-confinement + conflict-marker scan. *(Shrunk per D5.)* | storage, model, error |
| `unblock-health` | L3 | v1: libsql integrity + diagnostics. [v1.1]: full Workspace Health Contract. | model, error, sync |
| `unblock-config` | L4 | Layered TOML config resolution, `.unblock/` discovery, open-a-workspace facade. | storage, sync, health, model, error |
| `unblock-engine` | L5 | Shared session API (open → import? → mutate → export? → recover); **in-process write Semaphore (D14)**; shutdown/logging. Composes storage **+ policy**. Embeddable surface for MCP/CLI. | config, sync, storage, policy, health, model, error |
| `unblock-render` | L6 | Output/format (json/robot/plain/csv/markdown; **TOON feature-gated, v1.1**) behind a trait. Reduced under D7. | model, error |
| `unblock-mcp` | L7 | **Primary** rmcp stdio server (tools/resources/prompts) over the engine. Feature-isolated. | engine, render, policy, model, error |
| `unblock-cli` | L7 | Reduced binary: lifecycle/ops (serve/migrate/doctor/version) + thin routing; owns cooperative-shutdown signal install (FR-17, OQ-4). | engine, render, policy, mcp, error |
| `unblock-fuzz` | — | Unpublished member; `cargo-fuzz` targets over model/sync/storage. | model, sync, storage, error |

> CF-11 fix: `unblock-storage` no longer depends on `unblock-policy`; the engine (L5) composes storage + policy.
> Any shared contract type policy/storage both need lives in `unblock-model`.
>
> **"Depends on" convention:** L0 crates (`model`, `error`) are listed **explicitly** on every row that depends on
> them directly (not left transitive), so each row is a complete dependency declaration. The only sanctioned
> intra-L0 edge is `model → error` (CF-G).

### 8.2 Concurrency, async & data flow

- **Async everywhere** (tokio). The `Storage` trait is async (`async-trait`); libsql is an async client.
- **Supported v1 topology (D14):** each MCP client spawns its **own** `unblock serve` (stdio = one server per
  client). v1 is **local single-workspace**: for one client, the engine serializes writes with an **in-process
  tokio `Semaphore`** and reads use a fast path (WAL readers). Multiple local serve processes on the same
  `unblock.db` (two editor windows, or a CLI `migrate`/`doctor` while serve runs) fall back to SQLite WAL +
  `busy_timeout` — correct but **best-effort**; `migrate`/`doctor` are expected to run when serve is inactive.
- **Multi-dev / cross-machine sharing is v1.2, not v1 (D15):** there, each dev keeps their own local `unblock
  serve` pointed at a **shared libsql/Turso primary** (embedded replica: local reads, **writes delegated to the
  primary** → atomic ops like `claim` serialize there; reads are eventually-consistent per sync interval). That is
  how the swarm/multi-dev persona scales — **not** thousands of clients on one local serve. See `00-roadmap.md` §3.
- **Non-spin guarantee (NFR-3):** WAL + native `busy_timeout` (>0) + the in-process serialized writer — no
  app-level spin; defect-243 cannot recur. `failsafe`/retry only guard the optional remote/replica path.
- **Write flow (model B):** acquire engine write permit → mutate libsql (issue rows + transactional audit
  events) → optionally export to `issues.jsonl` (atomic temp+fsync+rename) if export is enabled. No git, no merge.
- **Read flow:** fast-path query against libsql (in-memory graph traversal via `petgraph` where needed) → render.
- **Shutdown (FR-17):** a cooperative flag flips on signal; the engine flushes/closes libsql cleanly.

## 9. MCP Surface Design (primary interface)

Designed deliberately rather than one-tool-per-CLI-command (to keep the client's tool list small — token cost
+ selection accuracy):

- **Tools** — actions, consolidated where natural (e.g. an `issue` tool with an `action` enum for
  create/update/close/reopen/delete; dedicated `claim`, `ready`, `dep`, `search`, `export`/`import`). Schemas via `schemars`.
- **Resources** — queryable state: `unblock://issues/{id}`, `unblock://issues/ready`, `unblock://issues/blocked`, `unblock://capabilities`, `unblock://schema`.
- **Prompts** — guided workflows: `triage`, `plan_next_work`, `close_with_suggestions`.
- **Errors** — domain errors map to MCP error data carrying `code`/`message`/`hint`/`retryable`, parallel to the CLI 0–8 exit codes.
- **Discovery** — `capabilities`/`schema` versioned by `contract_version`.
- **Target:** keep v1 tool count small (favour consolidated tools + resources over a per-command explosion); record the count as a success metric (§14).

## 10. Technology Stack

Confirmed against the workspace scaffold (`Cargo.toml`):

- **Runtime/async:** `tokio` (full). **Storage:** **libsql** *(in `workspace.dependencies` as `default-features = false, features = ["core"]` — local; SQLite is statically bundled by `core` (there is **no** separate `bundled` Cargo feature); remote/replica behind the non-default `remote` feature — D15)* behind `async-trait` `Storage`. **Graph:** `petgraph`. **MCP:** `rmcp 1.7` (`server`, `transport-io`). **Schemas:** `schemars`. **Errors:** `snafu`. **Time:** `chrono`. **Serialization:** `serde`/`serde_json`; **config:** TOML. **Resilience (remote path only):** `failsafe` + a maintained retry crate (`backon`/`tokio-retry`, **not** archived `backoff 0.4`). **HTTP (non-default, transitive-only):** `reqwest` — pulled **only** via libsql's `remote` feature and the cli `self-update`/`axoupdater` surface; **not** a direct workspace dependency; **mocked by** `wiremock`. **Logging:** `tracing`(+subscriber). **CLI (lifecycle):** a lightweight `clap` (stable features only) *(add)*. **Testing:** `proptest`, `criterion` (async_tokio), `insta`, `cargo-fuzz`. **Toolchain:** stable `1.96.0`; lints **`unsafe_code = forbid`**, `missing_docs = warn`, clippy pedantic.

**Scaffold (done at T0.1+T0.2):** added `libsql` (`default-features = false, features = ["core"]`), `clap` (`derive`/`env`), `axoupdater` (cli `self-update`), `backon` (remote retry); swapped archived `backoff` → `backon`; **dropped direct `reqwest`** (transitive-only); `unsafe_code` `deny` → `forbid`; `default-members` excludes `unblock-fuzz`; `rmcp` pinned to `1.7` via the committed `Cargo.lock`. *(This list is the dependency-stack SSOT alongside the per-crate plans — there is no separate spine dependency section.)*

**Removed vs original:** fsqlite (15 crates) → libsql; fastmcp-rust → rmcp; rich_rust/crossterm/indicatif → dropped (D7); serde_yml → TOML; anyhow/thiserror → snafu; clap unstable/nightly → stable; **`self_update` → `dist`/`axoupdater`** (D17). **Release tooling:** `dist` (cargo-dist) for the CI release pipeline + installers + attestations.

**Crate publishing:** the `unblock-*` library crates are **workspace-internal — not published to crates.io**; only the `unblock` binary is distributed (via `dist`).

## 11. Out of Scope (v1)

- Town/mayor cross-project routing (D11) — `wont`.
- A hosted/networked multi-user service (collaboration via libsql sync or optional JSONL snapshot, not a bespoke server).
- Automatic git operations, hook installation, git-history reading, or any self-installed daemon (D13/NFR-6).
- Backward compatibility with classic Go-bd's Dolt architecture or with existing on-disk DBs. *(Note: a one-shot best-effort `bd` data import IS in scope — FR-26/D16 — distinct from maintaining compatibility.)*
- v1.1-deferred features: FR-6, FR-13(full), FR-16(full taxonomy), FR-18, FR-19, FR-21, FR-22, FR-23, TOON. *(FR-25 self-update moved to v1 via `dist`/`axoupdater` — D17.)*

## 12. Resolved Items

### 12.1 Agent Mail — DROPPED
It belonged to the upstream beads author's ecosystem. unblock has no "Agent Mail" dependency; swarm coordination (FR-18) is **purely DB-state-derived**.

### 12.2 MCP tool taxonomy — DEFINED
The concrete v1 consolidated tool/resource/prompt set and tool-count target are specified in the implementation plan (`docs/plans/implementation-plan.md`, MCP surface section).

### 12.3 Self-update (FR-25) — LANDS IN v1
Self-update **lands in v1 via `axoupdater`** (D17/CF-K); signing = **GitHub artifact attestations** verified before execution (NFR-17); **no isolated crate** — the command lives in `unblock-cli`. The earlier minisign-vs-embedded-key question is closed (attestations chosen). Command name is canonical `unblock update` (see §12.6).

### 12.4 Health taxonomy (FR-16) — DEFERRED to v1.1
v1 ships `doctor` + libsql `integrity_check`; the Recoverable/Drifted/Unsafe redefinition for a libsql-authoritative world is a v1.1 design item.

### 12.5 Names — LOCKED
Config dir `.unblock/` (monorepo alias `_unblock/` also accepted on discovery — D8, FORK-2/2026-06-26); DB `unblock.db`; optional export `issues.jsonl`; config `config.toml`; v1.1 artifacts `policy.toml`, `interactions.jsonl`.

### 12.6 Self-update command name — `unblock update`
The single v1 self-update command is spelled **`unblock update`** everywhere (PRD FR-25/NFR-17, ci-cd, roadmap, README, and `unblock-cli` — `Command::Update`/`UpdateArgs`/`commands/update.rs`/help snapshots). The Cargo feature stays named **`self-update`**; the feature enables the `unblock update` command (G-2/G-18).

## 13. Phasing & Milestones

A vertical walking skeleton (D14, thin-slice scope). Each milestone is independently shippable/testable.

| Milestone | Crates | FRs | Gate |
|---|---|---|---|
| **M0 — Foundation** | unblock-model, unblock-error, unblock-storage (Storage trait + libsql impl) | — | `Storage` contract suite (NFR-16) green; **contention lab confirms no hot-spin (NFR-3) BEFORE other crates depend on storage** |
| **M1 — Engine + core domain** | unblock-engine (Semaphore, lifecycle), unblock-policy, unblock-config (subset) | FR-1a/1b/1c, FR-2, FR-3, FR-4 (incl. ready), FR-5, FR-9, FR-10, FR-13 (config-resolution subset; engine/env/file precedence) | Property test: engine mutations linearizable; CRUD/ready/dep via internal API |
| **M2 — MCP surface** | unblock-mcp, unblock-render (reduced), unblock-sync | FR-20, FR-11, FR-12, FR-7, FR-8, FR-26 (bd import), FR-15 | MCP client completes ready → claim → close; a representative bd repo imports |
| **M3 — Reliability + ops + GA** | unblock-cli, unblock-health (lite) | FR-14 (init/agents bootstrap), FR-13 (CLI flag-forwarding half), FR-16 (lite), FR-17, FR-25 (dist/axoupdater) | Shutdown/failure-injection (NFR-4/5) + perf budgets (NFR-1/2) green; `dist` release pipeline + attestations |
| **v1.1+** | — | FR-6, FR-13(full), FR-16(full), FR-18, FR-19, FR-21, FR-22, FR-23, TOON | per-feature |

## 14. Success Metrics & Risk Register

### 14.1 Success metrics (ship-gates)
- **Functional parity (v1 slice):** every v1-tier FR meets its AC.
- **Performance:** NFR-1 budgets and NFR-2 (250k in CI) pass as hard gates.
- **Agent experience:** end-to-end `ready → claim → close` round-trip latency under a target (TBD on M2); MCP tool count ≤ target (§9).
- **Dev tracking:** unblock's v1 development is tracked in `docs/plans/STATUS.md` (Markdown registry); once v1 works, unblock can take over tracking its own v1.1+ work (the dogfood milestone).

### 14.2 Risk register
| ID | Risk | L | I | Mitigation | Trigger/owner |
|---|---|---|---|---|---|
| RK-1 | libsql busy/lock semantics don't actually avoid hot-spin under load | M | H | **Contention lab in M0, before any crate depends on storage** (NFR-3); **fallback = `rusqlite` behind the same `Storage` trait** if it fails (a swap, not a rewrite) | M0 / storage owner |
| RK-2 | rmcp 1.7 API churn | M | M | Pin version; isolate in `unblock-mcp`; thin adapter over engine | M2 / mcp owner |
| RK-3 | MCP tool count hurts client selection accuracy | M | M | Consolidate tools + resources (§9); measure count (§14.1) | M2 |
| RK-4 | libsql remote feature leaks TLS/HTTP into default build | L | M | Remote behind non-default feature (D15); cargo-deny on tree | M0 |
| RK-5 | Health-contract scope creep | M | M | v1 ships lite; full taxonomy deferred to v1.1 (FR-16) | v1.1 |
| RK-6 | Self-update supply-chain (unsigned) | L | H | v1: verify GitHub artifact attestations before execution (FR-25/NFR-17); `axoupdater` in `unblock-cli`, no isolated crate (D17/CF-K) | M3 / cli owner |

## Appendix A — Traceability

Derived from a 3-analyst + coordinator discovery workflow over `temp/beads_rust-main` and a subsequent
3-lens + coordinator review (technical / product / PM). FR/NFR ids map to the original feature inventory;
deviations are annotated inline and consolidated in §4 (Key Decisions). Review verdict that produced v0.2:
"excellent foundation; needs a concurrency decision, a delivery wrapper, a tightened v1 thin-slice, and
contradiction fixes" — all addressed in this revision.
