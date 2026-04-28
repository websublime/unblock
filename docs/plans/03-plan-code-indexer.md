# Plan 03 — Code Indexer MCP (v1.0.0)

> Phase: 03
> Status: APPROVED
> Author: Ada (architect)
> Date: 2026-04-27
> Crates (new): `unblock-indexer-core`, `unblock-indexer`
> Crates (modified): `unblock-mcp`
> Depends on: Phase 02 (MCP Complete) — reuses OpenTelemetry, circuit breaker, retry policies for HTTP grammar fetch
> Required by: Phase 04 (Plugin) — supervisors and Sherlock will lean on indexer tools instead of Glob/Grep/Read
> Source: [MANIFESTO](../MANIFESTO.md) · [PRD §7 Phase 03](../PRD.md) · [SPEC](../SPEC.md)

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [Scope](#2-scope)
3. [Out of Scope (Non-Goals)](#3-out-of-scope-non-goals)
4. [Locked Architectural Decisions](#4-locked-architectural-decisions)
5. [Crate Architecture](#5-crate-architecture)
6. [Storage & Cache Layout](#6-storage--cache-layout)
7. [MCP Tool Surface](#7-mcp-tool-surface)
8. [Lifecycle: Bootstrap, Update, Watch](#8-lifecycle-bootstrap-update-watch)
9. [Grammar Pipeline (CI)](#9-grammar-pipeline-ci)
10. [External Dependencies & APIs](#10-external-dependencies--apis)
11. [Research Gaps (Smith input)](#11-research-gaps-smith-input)
12. [Epic Breakdown](#12-epic-breakdown)
13. [Task Dependencies](#13-task-dependencies)
14. [Acceptance Criteria](#14-acceptance-criteria)
15. [Risks & Mitigations](#15-risks--mitigations)
16. [Definition of Done](#16-definition-of-done)

---

## 1. Purpose

**Token-saving for AI agents.** Today, agents that drive `unblock-mcp` waste tokens on `Glob`, `Grep`, and `Read` chains to find symbols, definitions, exports, and file structure. Phase 03 embeds a multi-language code indexer behind the same MCP binary so that "where is X / what does Y export / show me Z" becomes a fast, structured tool call (target p99 < 10ms for `find_symbol`, < 20ms for `outline`).

The indexer is multi-language, persisted on disk (SQLite + FTS5), pre-warmed by a file watcher, and fully pluggable from MVP day one — new languages ship as WASM tree-sitter grammars fetched at runtime, not as binary recompilations.

**Outcome:** `v1.0.0` of the unblock MCP surface gains a code-indexer tool set served from the same binary as the issue-graph tool set. A supervisor or Sherlock investigating a bead can ask `find_symbol("DependencyGraph")` and get file/line back in milliseconds, instead of grepping the workspace.

**Phase positioning.** Phase 03 slots **after** Phase 02 (MCP Complete) deliberately. The grammar fetcher is HTTP-bound, so it leans on Phase 02's retry-with-backoff, circuit breaker, and OpenTelemetry plumbing rather than reinventing them. Phase 03 ships the first installable v1.0.0 with both tool sets.

**Governing constraints (from MANIFESTO + global feedback):**

- **No simplifications without user approval.** Every locked decision in §4 was negotiated; deviations require user sign-off.
- **Pre-production stance.** No users, no migrations, no deprecation shims. Breaking changes acceptable across all unblock crates during this phase.
- **Zero custom storage for issue data.** The indexer's SQLite is a *local cache of the local filesystem* — it is not a store of GitHub state. Law 1 of the MANIFESTO is preserved.

---

## 2. Scope

### 2.1 Token-saving MCP tool set (9 tools)

Served from the existing `unblock-mcp` binary alongside the Phase 01–02 issue-graph tools.

| # | Tool | Type | Purpose |
|---|---|---|---|
| 1 | `find_symbol` | Read | Locate symbols by name, optional kind/language/limit/fuzzy |
| 2 | `list_symbols` | Read | All symbols in a file or path (recursive optional) |
| 3 | `outline` | Read | Hierarchical tree of file/module structure |
| 4 | `get_symbol` | Read | Full details for an opaque `symbol_id` (body read from FS) |
| 5 | `search_text` | Read | FTS5 matches across names, signatures, comments |
| 6 | `find_references` | Read | Best-effort syntactic references — **explicitly marked HEURISTIC in tool description** |
| 7 | `list_languages` | Read | Loaded grammars for the current repo |
| 8 | `index_status` | Read | Freshness, last update, totals |
| 9 | `reindex` | Write | Force re-parse for whole repo or path |

Symbol disambiguation is via opaque `symbol_id` returned from queries. Bodies are never stored in the database — span only (file:line:col); the tool reads bytes from the filesystem on demand.

### 2.2 New crates

- `unblock-indexer-core` — **pure Rust**, zero IO. Domain types (symbol kinds, span, query input/output shapes), AST traversal logic over `tree_sitter::Tree`, schema definitions as constants. Mirrors the pattern of `unblock-core`.
- `unblock-indexer` — **impure shell**. SQLite (sqlx + FTS5), tree-sitter WASM runtime, grammar fetcher (reqwest, integrity-verified), file walker (`ignore` crate), file watcher (`notify-debouncer-full`), bootstrap parallelism (`rayon`).

### 2.3 Modifications to existing crates

- `unblock-mcp` — register 9 new tool handlers wrapping `unblock-indexer`. The `setup` subcommand extends to register both tool sets in Claude Desktop / Claude Code / Cursor / Zed / VS Code / JetBrains under the same MCP server entry. **Single `unblock-mcp setup` command, no separate indexer install.**
- Workspace `Cargo.toml` — add the two new crates.

### 2.4 Initial language coverage (Top-10)

Rust, TypeScript, JavaScript, Python, Go, Java, C, C++, Ruby, PHP. PR-driven expansion thereafter via the CI grammar pipeline. Unsupported languages return a clear error pointing at the contribution PR template.

### 2.5 Grammar pipeline (own infrastructure)

GitHub Actions matrix per language → `tree-sitter generate` → `tree-sitter build --wasm` → publish WASM blobs as release assets on the `unblock-mcp` GitHub Releases. Runtime fetches on first encounter of a language and caches under XDG (`~/.cache/unblock/grammars/`). Integrity verified via SHA-256 from a signed `manifest.toml`.

---

## 3. Out of Scope (Non-Goals)

The following are **explicitly rejected** for Phase 03 and any future indexer work unless renegotiated:

| Item | Why it is out |
|---|---|
| Dead-code analysis | Requires whole-program reachability — outside token-saving goal |
| Cyclomatic complexity metrics | Outside token-saving goal |
| Redundancy / similarity detection | Outside token-saving goal |
| Refactor suggestions | Outside token-saving goal |
| Cross-file semantic resolution / type inference | Requires LSP-grade analysis; tree-sitter is syntactic only |
| Issue/code correlation queries (legacy `unblock-c3g` epic) | Abandoned. Out of scope for this phase. |
| Forking AstBasedContext-rs | Architectural divergence too large |
| SCIP indexers | Heavy per-language toolchains |
| Wrapping LSPs | Per-language friction |
| Reusing Zed's WASM artefacts | Supply-chain risk + AGPL contamination + ABI lock-in |
| Bundling grammars into the binary | Not pluggable |
| Static-linked tree-sitter grammar crates | Not pluggable |
| Dynamic native libs for grammars | Cross-platform pain + native-code security |
| `rusqlite` | Sync API |
| `turso` 0.5.3 | BETA, FTS5 not exposed; repeats the c3g/GrafeoDB pre-1.0 failure pattern |
| Body storage in DB | Span only; FS is canonical |
| Backward-compat shims | Pre-production stance |

---

## 4. Locked Architectural Decisions

These are **CONFIRMED**. Re-litigation requires explicit user sign-off.

| # | Decision | Locked Value |
|---|---|---|
| L1 | Build vs integrate | From-scratch with tree-sitter |
| L2 | Grammar mechanism | WASM (sandbox, single cross-platform binary) |
| L3 | Grammar source | Own GitHub Actions pipeline; publish to unblock-mcp GitHub Releases |
| L4 | Pluggability | Pluggable from MVP day 1; runtime fetch + XDG cache; not bundled |
| L5 | Initial languages | Top-10: Rust, TS, JS, Python, Go, Java, C, C++, Ruby, PHP |
| L6 | Language detection | File extensions + threshold (≥1 file) + `.unblock/languages.toml` override; gitignore via `ignore` crate; default excludes: `target/`, `node_modules/`, `dist/`, `build/`, `.venv/`, `vendor/`, `.git/` |
| L7 | Cache location | `~/.cache/unblock/grammars/` and `~/.cache/unblock/repos/<repo-hash>/` |
| L8 | Crate split | `unblock-indexer-core` (pure) + `unblock-indexer` (impure); MCP tools added to existing `unblock-mcp` |
| L9 | Storage | `sqlx` with `sqlite` feature + FTS5; WAL mode |
| L10 | Schema | `symbols` + `symbols_fts` (content table) + `files` + `meta`; span-only, no bodies |
| L11 | Tool surface | 9 tools; opaque `symbol_id` for disambiguation |
| L12 | Update strategy | Startup mtime scan + per-query mtime check + `notify-debouncer-full` watcher; parallel bootstrap via `rayon` in single transaction |
| L13 | Setup auto-config | Extends `unblock-mcp setup` to register tools across CC Desktop / CC Code / Cursor / Zed / VS Code / JetBrains; single command |
| L14 | Pre-production stance | No migrations, no deprecation shims; breaking changes acceptable |

---

## 5. Crate Architecture

```
unblock-mcp (bin)
  ├── unblock-tools (existing — issue-graph)
  └── unblock-indexer (new — code-indexer)
        └── unblock-indexer-core (new — pure)

unblock-indexer (impure shell)
  ├── sqlx (sqlite + fts5 + runtime-tokio)
  ├── tree-sitter
  ├── tree-sitter wasm runtime  ← decision pending (research gap R2)
  ├── reqwest                   ← grammar fetch (reuses Phase 02 retry/circuit-breaker)
  ├── ignore                    ← gitignore-aware walker
  ├── notify-debouncer-full     ← file watcher
  ├── rayon                     ← bootstrap parallelism
  ├── sha2                      ← grammar integrity
  ├── snafu                     ← errors (workspace convention)
  └── tracing                   ← logging (workspace convention)

unblock-indexer-core (pure)
  ├── serde / serde_json
  ├── snafu
  └── (no IO, no async)
```

**Boundary rule.** `unblock-indexer-core` MUST compile without any IO or async dependencies. AST traversal accepts a borrowed `tree_sitter::Tree` + source bytes and emits typed records; sqlx layer is in `unblock-indexer`. This mirrors the existing `unblock-core` / `unblock-github` boundary.

**Workspace error model.** Both new crates use `snafu` exclusively. Each defines `src/errors.rs` with crate-scoped `Result<T>`. No `unwrap()` / `expect()` outside tests. `#![deny(unsafe_code)]` workspace-wide.

**Licensing.** Both new crates: MIT (open-source foundation, consistent with `unblock-core` / `unblock-github` / `unblock-tools`).

---

## 6. Storage & Cache Layout

### 6.1 On-disk layout

```
~/.cache/unblock/
├── grammars/
│   ├── manifest.toml                    # versions + SHA-256 per language
│   ├── rust-<version>.wasm
│   ├── typescript-<version>.wasm
│   ├── javascript-<version>.wasm
│   ├── python-<version>.wasm
│   ├── go-<version>.wasm
│   ├── java-<version>.wasm
│   ├── c-<version>.wasm
│   ├── cpp-<version>.wasm
│   ├── ruby-<version>.wasm
│   └── php-<version>.wasm
└── repos/
    └── <repo-hash>/
        ├── index.db        # SQLite + FTS5 + WAL
        └── meta.toml       # repo path, last bootstrap, indexer version
```

`<repo-hash>` derives from the absolute repo root path (canonicalised). XDG-compliant: respects `XDG_CACHE_HOME` when set.

### 6.2 SQLite schema

```sql
CREATE TABLE symbols (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  file TEXT NOT NULL,
  line INTEGER NOT NULL,
  col INTEGER NOT NULL,
  end_line INTEGER NOT NULL,
  end_col INTEGER NOT NULL,
  signature TEXT,
  parent_id INTEGER REFERENCES symbols(id),
  language TEXT NOT NULL
);
CREATE INDEX idx_name ON symbols(name);
CREATE INDEX idx_file ON symbols(file);
CREATE INDEX idx_kind ON symbols(kind);

CREATE VIRTUAL TABLE symbols_fts USING fts5(
  name, signature, comment,
  content='symbols', content_rowid='id'
);

CREATE TABLE files (
  path TEXT PRIMARY KEY,
  language TEXT NOT NULL,
  mtime INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  parsed_at INTEGER NOT NULL
);

CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT
);
```

**Invariants:**
- WAL mode on all connections.
- No body text stored. `signature` is the head line(s) only (cheap to extract from the tree).
- `symbols_fts` is a content table synchronised via SQLite triggers (insert / update / delete on `symbols`) — populated and maintained by `unblock-indexer`.
- `parent_id` enables hierarchical `outline` queries (struct → method, class → function, module → ...).
- The schema constants live in `unblock-indexer-core` so `unblock-indexer` and tests share the canonical DDL.

### 6.3 Symbol kind set

A canonical kind enum (in `unblock-indexer-core`) generalises across the Top-10 languages. Initial proposal — to be validated by R5:

`function`, `method`, `class`, `struct`, `enum`, `interface`, `trait`, `module`, `namespace`, `variable`, `constant`, `type_alias`, `field`, `property`, `import`, `export`.

Per-language tree-sitter S-expression queries map native node types onto this enum. Languages that lack a kind (e.g. Go has no class) simply do not emit it.

---

## 7. MCP Tool Surface

### 7.1 Tool descriptions (high level — full schemas land in the spec)

| Tool | Input | Output | Notes |
|---|---|---|---|
| `find_symbol` | `name: string, kind?: string, language?: string, limit?: u32, fuzzy?: bool` | `[{symbol_id, name, kind, file, line, col, signature?, language}]` | Default `limit=20`. `fuzzy=true` uses FTS5 prefix/trigram. |
| `list_symbols` | `path: string, kinds?: [string], recursive?: bool` | `[{symbol_id, name, kind, line, col, parent_id?}]` | `path` is repo-relative. |
| `outline` | `path: string` | `{file, language, tree: [TreeNode]}` where `TreeNode` is recursive | Single file. |
| `get_symbol` | `symbol_id: u64` | `{...all columns..., body: string}` | Body is read from filesystem at query time. |
| `search_text` | `query: string, scope?: string, language?: string, limit?: u32` | `[{symbol_id, name, kind, file, line, snippet}]` | FTS5 MATCH against name + signature + comment. |
| `find_references` | `name: string` OR `symbol_id: u64` | `[{file, line, col, surrounding_symbol?: symbol_id}]` | **Description must include "HEURISTIC — syntactic only, no type resolution".** |
| `list_languages` | (none) | `[{language, grammar_version, file_count}]` | |
| `index_status` | (none) | `{repo_root, last_full_index, last_incremental, total_files, total_symbols, watcher_active, db_size_bytes}` | |
| `reindex` | `path?: string` | `{files_reparsed, symbols_emitted, duration_ms}` | Full when `path` omitted. |

### 7.2 Symbol disambiguation

`symbol_id` is opaque — clients must not parse it. Internally it is the SQLite `rowid` of `symbols`. The `get_symbol` round-trip is the canonical disambiguation path.

### 7.3 Error model

A new `IndexerError` enum in `unblock-indexer-core` (snafu) with variants for: language not supported, grammar fetch failed, integrity check failed, parse failed, file not found, db locked, etc. Mapped to MCP errors in `unblock-mcp` per existing convention.

**`LanguageNotSupported`** error includes a `pr_pointer` field with a stable URL to the contribution PR template.

---

## 8. Lifecycle: Bootstrap, Update, Watch

### 8.1 First-run (cold)

1. `unblock-mcp` starts. The indexer subsystem reads `~/.cache/unblock/repos/<repo-hash>/meta.toml`; if missing or version-mismatched, performs a **bootstrap**.
2. **Detect languages.** Walk the repo with `ignore` (respects `.gitignore` + default excludes) collecting file extensions. Apply `.unblock/languages.toml` overrides if present. Threshold: any language with ≥1 file is enabled.
3. **Fetch grammars.** For each detected language, ensure the WASM blob exists in `~/.cache/unblock/grammars/`. Missing → HTTP fetch from the unblock-mcp GitHub Releases (using Phase 02 retry + circuit breaker). Verify SHA-256 against `manifest.toml`.
4. **Parallel parse.** `rayon` parallelises file parsing. All `INSERT`s into `symbols` + triggered FTS5 sync occur in **a single transaction** for write throughput.
5. **Spawn watcher.** `notify-debouncer-full` subscribes to the repo root with a 200ms debounce window. (Threshold to be confirmed by R6.)

### 8.2 Steady-state (warm)

- **Per-query mtime check** on files implicated by the query (e.g. `list_symbols(path)` checks `path`'s mtime). If newer than `files.mtime`, the file is re-parsed before the query proceeds.
- **Watcher events** drive incremental re-parses in the background — additions, modifications, renames, deletes. Deleted files cascade to delete their symbols (FK-less; `unblock-indexer` does the cleanup). Renames are treated as delete + insert in the MVP.

### 8.3 Forced re-index

`reindex(path?)` truncates `files` + `symbols` for the path subtree (or whole repo) and re-parses synchronously. Returns counts and duration.

---

## 9. Grammar Pipeline (CI)

A new GitHub Actions workflow `.github/workflows/grammars.yml` lives in this repo. Matrix dimensions: `language × tree-sitter-version`. Per cell:

1. Check out the upstream `tree-sitter-<lang>` grammar at a pinned tag.
2. `tree-sitter generate`.
3. `tree-sitter build --wasm`.
4. Compute SHA-256.
5. Upload the WASM blob and update a generated `manifest.toml` artefact.

A **release job** publishes the matrix output to a versioned GitHub Release of `unblock-mcp` (e.g. `v1.0.0-grammars`). The runtime knows the release URL pattern; integrity check happens against the manifest.

**Open questions handed to research (R1, R3):** packaging mechanics, ABI stability, and integrity model.

---

## 10. External Dependencies & APIs

| Dependency | Use | Phase | Notes |
|---|---|---|---|
| `tree-sitter` (Rust crate) | Parsing core | 03 | ABI version pinning required — see R3 |
| `tree-sitter-loader` OR `wasmtime` | WASM grammar runtime | 03 | Choice deferred to research — R2 |
| `sqlx` (sqlite + macros + runtime-tokio) | DB layer with FTS5 | 03 | Compile-time query checks |
| `ignore` (BurntSushi) | gitignore-aware walker | 03 | Same as ripgrep |
| `notify-debouncer-full` | File watcher | 03 | macOS / Linux / Windows — see R6 |
| `rayon` | Bootstrap parallelism | 03 | CPU-bound parse fan-out |
| `reqwest` | Grammar fetch | 03 | Reuses Phase 02 retry / circuit-breaker / OTel |
| `sha2` | Integrity | 03 | SHA-256 |
| `dirs` or `xdg` | Cache location | 03 | XDG compliance |
| `serde` / `serde_json` | Schemas | 03 | Workspace convention |
| `snafu` | Errors | 03 | Workspace convention |
| `tracing` | Logs | 03 | JSON to stderr; never stdout (MCP stdio reserved) |

External APIs touched:

- **GitHub Releases (read)** — grammar blobs + manifest. Anonymous, rate-limited per IP. Reuses Phase 02's resilience layer.
- **Editor MCP config files** — Claude Desktop, Claude Code, Cursor, Zed, VS Code, JetBrains. JSON / TOML schemas vary per host (R9).

---

## 11. Research Gaps (Smith input)

These are **not blockers for plan approval** but **must be validated before §12 spec authoring**. Smith is dispatched after this plan reaches APPROVED.

| # | Gap | Why it matters | Expected output |
|---|---|---|---|
| R1 | **`tree-sitter build --wasm` packaging** — artefact format, GH Release packaging, runtime fetch, integrity verification (hash) | Fundamental to the plugin model (L4) | Concrete pipeline + verified artefact format |
| R2 | **Runtime WASM loading in Rust** — `tree-sitter-loader` vs `wasmtime` direct; ABI stability across tree-sitter versions; init cost | Determines `unblock-indexer` runtime stack | Decision + benchmarked init cost |
| R3 | **Top-10 grammar audit** — confirm all 10 grammars exist, are maintained, share a compatible tree-sitter ABI | Locks initial language list (L5) | Per-grammar audit table with version pins |
| R4 | **`sqlx` + FTS5** — content tables, sync triggers, perf at target scale | Determines query latency | Working schema migration + benchmarks |
| R5 | **Symbol-extraction queries (S-expressions)** — generalisable cross-language vs per-language; canonical kind set across the Top-10 | Validates the symbol kind enum (§6.3) | Per-language `.scm` files + kind mapping |
| R6 | **`notify-debouncer-full`** — macOS / Linux / Windows edge cases (atomic save, renames, deletes, large directory trees) | Determines watcher correctness | Platform compatibility matrix + debounce tuning |
| R7 | **`ignore` crate edge cases** — monorepos, nested gitignores, non-git checkouts | Determines walker correctness | Edge-case coverage report |
| R8 | **Latency benchmarks** — p99 < 10ms `find_symbol`, < 20ms `outline`, on small / medium / large representative repos | Validates the token-saving promise | Benchmark suite + measured numbers |
| R9 | **Setup auto-config schemas** — Claude Desktop / Code / Cursor / Zed / VS Code / JetBrains JSON paths | Required for `unblock-mcp setup` extension | Per-host config-file map |
| R10 | **Token-saving measurement methodology** — empirically prove ROI post-merge | Justifies the phase | Measurement plan + baseline corpus |

**Plan invariant:** every research gap above maps to at least one task in §12. Smith's findings either confirm the plan or surface contradictions that loop back to Ada for plan revision.

---

## 12. Epic Breakdown

Six epics. Each epic decomposes into beads during `/tasks` (Fernando) **after** research validates this plan. Bead descriptions will reference this plan + the spec; per workflow rules they will not duplicate authoritative content.

### Epic 03.1 — Workspace, Crates, Error Model

**Owner:** rust-supervisor (Neo)
**Output:** Two new crates compile, lint, and test green inside the workspace.

Tasks:
1. Add `unblock-indexer-core` crate (lib, MIT, snafu, no IO).
2. Add `unblock-indexer` crate (lib, MIT, snafu, async, sqlx + tokio).
3. Wire both into `Cargo.toml` workspace; CI (fmt / clippy / test / doc) green.
4. Define `IndexerError` + per-crate `Result<T>` aliases.
5. Module skeletons + module-level `//!` docs; placeholder integration test.

### Epic 03.2 — Grammar Pipeline & Runtime Loader

**Owner:** rust-supervisor (Neo) + infra-supervisor (Olive)
**Output:** WASM grammars built in CI, published to GH Releases, fetched and loaded at runtime, integrity verified.

Tasks:
1. CI workflow `grammars.yml` — matrix per language, `tree-sitter generate` + `tree-sitter build --wasm`. **Depends on R1, R3.**
2. Manifest format (`manifest.toml`) — language, version, SHA-256.
3. Release publishing job — versioned GH Release with WASM blobs + manifest.
4. Runtime grammar fetcher in `unblock-indexer` — reqwest + Phase 02 retry/circuit-breaker; verifies SHA-256; caches under `~/.cache/unblock/grammars/`. **Depends on R1.**
5. WASM runtime loader — `tree-sitter-loader` vs `wasmtime`. **Depends on R2.**
6. `list_languages` MCP tool wiring.

### Epic 03.3 — Storage Layer (sqlx + FTS5)

**Owner:** rust-supervisor (Neo)
**Output:** `index.db` per repo with WAL, FTS5, schema migrations, query helpers.

Tasks:
1. Schema constants in `unblock-indexer-core` (DDL strings).
2. `sqlx` migrations + WAL pragma at connect.
3. FTS5 sync triggers (insert / update / delete on `symbols`).
4. Query helpers for `find_symbol`, `list_symbols`, `outline`, `get_symbol`, `search_text`. **Depends on R4.**
5. Cache-root resolution (`XDG_CACHE_HOME` + repo-hash directory).
6. Concurrency: connection pool sizing + WAL behaviour under file-watcher writes.

### Epic 03.4 — AST Traversal & Symbol Extraction

**Owner:** rust-supervisor (Neo)
**Output:** Tree-sitter parsing emits the canonical symbol records for all Top-10 languages.

Tasks:
1. Symbol kind enum + canonical mapping in `unblock-indexer-core`.
2. Per-language S-expression query files (10 × `.scm`). **Depends on R5.**
3. Traversal that yields `(name, kind, span, signature, parent_id, language)` tuples. Pure, no IO.
4. Signature extractor (head line(s) only — no bodies stored).
5. Property tests: traversal is deterministic for a given (tree, source).

### Epic 03.5 — File Walker, Watcher, Bootstrap

**Owner:** rust-supervisor (Neo)
**Output:** First-run bootstrap is parallel + transactional; steady-state stays in sync via watcher + per-query mtime check.

Tasks:
1. Walker via `ignore` crate; default-excludes list + `.unblock/languages.toml` override loader. **Depends on R7.**
2. Bootstrap: rayon fan-out, single-transaction inserts, progress logging via `tracing`.
3. `notify-debouncer-full` watcher; rename / delete handling. **Depends on R6.**
4. Per-query mtime check at the query layer (not the storage layer).
5. `reindex(path?)` implementation.

### Epic 03.6 — MCP Tool Handlers + Setup Extension

**Owner:** rust-supervisor (Neo)
**Output:** All 9 tools served from `unblock-mcp`; `unblock-mcp setup` registers indexer tools alongside issue-graph tools.

Tasks:
1. Wire 9 tool handlers in `unblock-mcp` — schemas via `schemars`, errors mapped to MCP convention.
2. `find_references` description: explicit "HEURISTIC — syntactic only" string in the schema description (lint-checked).
3. `unblock-mcp setup` extension — write JSON / TOML config for Claude Desktop / Claude Code / Cursor / Zed / VS Code / JetBrains. **Depends on R9.**
4. Integration tests against a fixture repo (mixed-language).
5. Latency benchmark suite (criterion). **Depends on R8.**
6. Token-saving measurement harness — captures baseline (Glob/Grep/Read) vs indexer tool calls. **Depends on R10.**

---

## 13. Task Dependencies

```
Epic 03.1 (workspace)
   └── Epic 03.2 (grammars)         depends on R1, R2, R3
   └── Epic 03.3 (storage)          depends on R4
   └── Epic 03.4 (AST)              depends on R5, Epic 03.2
         └── Epic 03.5 (walker/watcher) depends on R6, R7
               └── Epic 03.6 (MCP + setup)  depends on R8, R9, R10, all prior
```

External-phase dependency: Phase 02 (MCP Complete) **must be merged** before Epic 03.2 starts — the grammar fetcher reuses Phase 02's HTTP resilience layer. Epic 03.1 (pure workspace setup) can begin in parallel with the tail of Phase 02.

---

## 14. Acceptance Criteria

The phase is complete when **all** of the following hold:

### 14.1 Functional

- [ ] `unblock-mcp` exposes the 9 indexer tools alongside the existing issue-graph tools.
- [ ] All Top-10 languages parse fixture repos without panic; `list_languages` reports the correct set.
- [ ] `find_symbol`, `list_symbols`, `outline`, `get_symbol`, `search_text`, `find_references`, `list_languages`, `index_status`, `reindex` return correct results on a mixed-language fixture repo.
- [ ] `find_references` description in the MCP schema includes the literal substring `HEURISTIC` (lint-enforced).
- [ ] Unsupported language returns `LanguageNotSupported` with a populated `pr_pointer`.

### 14.2 Pluggability

- [ ] Adding a new language requires only a PR to the CI grammar matrix — no recompilation of `unblock-mcp`.
- [ ] Grammars fetched from GH Releases are integrity-verified against a signed manifest.
- [ ] Grammar cache lives under `~/.cache/unblock/grammars/` (XDG-compliant).

### 14.3 Storage

- [ ] `index.db` per repo under `~/.cache/unblock/repos/<repo-hash>/index.db`.
- [ ] WAL mode enabled; FTS5 content table synchronised by triggers.
- [ ] No body text stored in the DB.

### 14.4 Performance

- [ ] `find_symbol` p99 < 10ms on the medium representative repo (corpus defined in R8).
- [ ] `outline` p99 < 20ms on the medium representative repo.
- [ ] Bootstrap on the large representative repo completes within an explicit budget set by R8.

### 14.5 Lifecycle

- [ ] Cold start triggers parallel bootstrap in a single transaction.
- [ ] File watcher (`notify-debouncer-full`) updates the index on file change / rename / delete on macOS, Linux, Windows.
- [ ] Per-query mtime check catches drift between watcher events.
- [ ] `reindex(path?)` truncates and reparses; returns counts + duration.

### 14.6 Setup

- [ ] `unblock-mcp setup` registers the indexer tool set in Claude Desktop, Claude Code, Cursor, Zed, VS Code, and JetBrains under the same MCP server entry as the issue-graph tools.
- [ ] Single command — no separate `unblock-indexer setup`.

### 14.7 Quality gates (workspace)

- [ ] `cargo fmt --check --all` clean.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo doc --no-deps --workspace` passes with zero warnings.
- [ ] Property tests (proptest) cover symbol-extraction determinism.

### 14.8 Token-saving evidence

- [ ] R10's measurement harness produces a comparison report (Glob/Grep/Read baseline vs indexer tools) for at least 3 representative agent flows.
- [ ] Result documented in `docs/research/03-code-indexer-roi.md` (Smith) before phase close.

---

## 15. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Tree-sitter ABI drift across languages | Medium | High | R3 audit pins versions; CI matrix enforces compatibility |
| WASM init cost dominates `find_symbol` p99 | Medium | High | R2 benchmarks; lazy load grammars per language; cache parser instances |
| `notify-debouncer-full` misses events on macOS atomic save | Medium | Medium | R6 platform matrix; per-query mtime check is the safety net |
| FTS5 perf collapses on huge repos | Low | High | R4 benchmarks; consider FTS5 prefix indexes; fall back to LIKE for cold queries |
| Grammar fetch fails offline | High | Medium | Phase 02 circuit breaker; clear error pointing user at offline grammar bundle workflow (deferred) |
| `find_references` heuristic produces false positives that mislead agents | High | Medium | Schema description must include "HEURISTIC"; lint enforced |
| Setup auto-config breaks existing user configs | Low | High | Idempotent merge per host (R9); never overwrite unrelated keys |
| ROI cannot be measured | Medium | High | R10 produces measurement methodology before phase close; phase exit blocked without it |

---

## 16. Definition of Done

The phase is **DONE** when:

1. All §14 acceptance criteria are met.
2. All 10 research gaps in §11 have validated answers in `docs/research/03-*.md` (Smith).
3. The spec `docs/specs/03-spec-code-indexer.md` has been authored (Ada, after research) and approved.
4. Beads have been created for every task in §12 (Fernando), referencing this plan and the spec.
5. Implementation closes all beads through the standard pipeline (investigate → do → review → quality).
6. `unblock-mcp` v1.0.0 ships with both tool sets via `cargo-dist`.
7. The token-saving ROI report (§14.8) is published.

---

*This plan defines what Phase 03 is and how it decomposes. The detailed designs (DB schemas, exact tool I/O shapes, WASM loader integration, traversal algorithms) live in the spec — authored after Smith validates the 10 research gaps in §11.*
