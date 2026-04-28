# Spec 03 — Code Indexer MCP (v1.0.0)

> Phase: 03
> Status: **APPROVED**
> Author: Ada (architect)
> Date: 2026-04-27
> Crates (new): `unblock-indexer-core`, `unblock-indexer`
> Crates (modified): `unblock-mcp`
> Source PRD: [docs/PRD.md](../PRD.md) (§7 Phase 03)
> Source Plan: [docs/plans/03-plan-code-indexer.md](../plans/03-plan-code-indexer.md) (APPROVED)
> Source Research: [docs/research/03-research-code-indexer.md](../research/03-research-code-indexer.md)
> Companion: [MANIFESTO](../MANIFESTO.md) · [SPEC](../SPEC.md)

---

## Table of Contents

1. [Scope & Conventions](#1-scope--conventions)
2. [Research Alignment & Resolved Decisions](#2-research-alignment--resolved-decisions)
3. [Crate Architecture](#3-crate-architecture)
4. [Domain Types (`unblock-indexer-core`)](#4-domain-types-unblock-indexer-core)
5. [Storage Layer (sqlx + FTS5)](#5-storage-layer-sqlx--fts5)
6. [Cache Layout](#6-cache-layout)
7. [Grammar Pipeline (CI)](#7-grammar-pipeline-ci)
8. [Grammar Runtime (Fetcher + WASM Loader)](#8-grammar-runtime-fetcher--wasm-loader)
9. [File Walker & Language Detection](#9-file-walker--language-detection)
10. [AST Traversal & Symbol Extraction](#10-ast-traversal--symbol-extraction)
11. [Lifecycle: Bootstrap, Watch, Steady-State](#11-lifecycle-bootstrap-watch-steady-state)
12. [MCP Tool Surface](#12-mcp-tool-surface)
13. [Setup & Editor Registration (`init` / `register`)](#13-setup--editor-registration-init--register)
14. [Error Model](#14-error-model)
15. [Configuration](#15-configuration)
16. [Performance Methodology & Gates](#16-performance-methodology--gates)
17. [Token-Saving ROI Harness](#17-token-saving-roi-harness)
18. [Testing Strategy](#18-testing-strategy)
19. [Invariants](#19-invariants)
20. [Open Items & Forward References](#20-open-items--forward-references)

---

## 1. Scope & Conventions

### 1.1 What this spec covers

Everything required to implement Phase 03 (v1.0.0): two new crates (`unblock-indexer-core`, `unblock-indexer`), 9 new MCP tool handlers wired into `unblock-mcp`, the grammar pipeline workflow + manifest format, the runtime grammar fetcher and WASM loader, the SQLite + FTS5 schema with sync triggers, the file walker / watcher / bootstrap lifecycle, the `init` / `register` setup CLI surface, the performance and ROI gates, and the testing strategy. This document is the single source of truth for implementation supervisors working on Phase 03.

### 1.2 What this spec does NOT cover

- Phase 02 functionality (retry, circuit breaker, in-memory `ServerMetrics`) — *consumed* here, not redefined. The resilience surface lives in `unblock-resilience` (per Plan 02 §6); OpenTelemetry export is deferred to Phase 06.
- Phase 04 distribution (cargo-dist, Homebrew, GitHub App auth).
- Plugin pipeline (Phase 05) — supervisors will consume the indexer tools but do not affect this spec.
- Remote MCP transport (Phase 06).
- Cross-file semantic resolution / type inference (out of scope per plan §3).
- Dead-code analysis, refactor suggestions, similarity detection (out of scope per plan §3).
- Issue/code correlation queries (former `unblock-c3g` epic; abandoned).

### 1.3 Pseudocode conventions

- Algorithms use numbered steps in plain English; type definitions use Rust syntax (implementation contract).
- `→` means "returns"; indentation indicates nesting; `IF`, `FOR`, `MATCH`, `RETURN` are control-flow keywords.
- DDL strings are *exact* — they live as `pub const &str` in `unblock-indexer-core::schema`.

### 1.4 References

When this spec says "Plan §N" it refers to [03-plan-code-indexer.md](../plans/03-plan-code-indexer.md). "Research §RN" refers to [03-research-code-indexer.md](../research/03-research-code-indexer.md). "Resolution Q-N" refers to the resolution log in the research document.

---

## 2. Research Alignment & Resolved Decisions

This section is **binding**. Each row enumerates either a research finding that the spec adopts verbatim or a Q-decision from the resolution log that the spec implements. Implementation supervisors must not re-litigate these.

| ID | Source | Decision adopted by this spec |
|---|---|---|
| L1–L14 | Plan §4 | Locked decisions stand unchanged. |
| C1 | Research §R2 / Resolution C1 | **WASM runtime is the `tree-sitter` crate `wasm` feature** (transitive `wasmtime-c-api-impl`). `tree-sitter-loader` is rejected. Plan §10 line for `tree-sitter-loader` is corrected here. |
| R1 | Research §R1 | Pipeline uses `wasi-sdk` (not emscripten), tree-sitter CLI ≥ 0.26.7. Asset naming: `tree-sitter-<lang>-<grammar-version>.wasm`. Manifest: `manifest.toml` shipped in same release. Runtime fetch via `browser_download_url` (avoids API quota). SHA-256 verified post-download. Manifest's own SHA-256 is a compile-time constant in `unblock-indexer-core` (integrity anchor). |
| R3 | Research §R3 / Resolution Q5 | Per-grammar version pin in `manifest.toml` with explicit `tree_sitter_abi_version`. **TypeScript ships as two grammars** (`typescript` and `tsx`); `tsx` is canonical (accepts `.ts` + `.tsx`). Stale-grammar audit annotation in CI (>12 months flagged). |
| R4 | Research §R4 / Resolution Q4 | sqlx `bundled` libsqlite3-sys with FTS5; runtime `PRAGMA compile_options;` assertion on first connect. **Add `comment TEXT` column to `symbols`** (Q4 Option A); FTS5 indexes `name + signature + comment`. External-content sync triggers use the canonical `'delete'` form before content mutation. Bootstrap uses *chunked transactions* to avoid starving readers. |
| R5 | Research §R5 / Resolution Q5 | **Keep all 16 kinds** (Q5 Option A). Vendor upstream `tags.scm` per language under `crates/unblock-indexer-core/queries/<lang>.scm` and extend with hand-written queries for `field`, `property`, `import`, `export`. `parent_id` linkage derived from tree position post-traversal (S-expression captures alone are flat). |
| R6 | Research §R6 | `notify-debouncer-full` with `FileIdMap`. **Default debounce: 500 ms** (configurable; 200 ms recommendation for interactive flows). Per-query mtime check is an **invariant** (§19). Linux inotify limit hint surfaced in error messages. macOS FSEvents drop risk documented and mitigated by mtime safety net. |
| R7 | Research §R7 / Resolution Q7 | `WalkBuilder::require_git(false)` set explicitly (footgun fix). `same_file_system(true)` to prevent crossing into mounted volumes. **`force_include` glob list supported** (Q7 Option A) — overrides `.gitignore` but never the hardcoded default-excludes. |
| R8 | Research §R8 | Performance methodology in §16. Implicated-file rule for per-query mtime check codified. `criterion` + tokio harness. |
| R9 | Research §R9 / Resolution Q9.1 + Q9.2 | **Three entry points** (B+C): existing `setup` MCP tool *refactored* to call `ensure_github_project()`, plus new `unblock-mcp init` (one-shot wizard) and `unblock-mcp register --host=<x>` CLI. JetBrains support is "manual import via Import-from-Claude" — no JetBrains-specific code. |
| R10 | Research §R10 / Resolution Q10.1 + Q10.2 | ROI harness uses **Sonnet via Anthropic API with a Claude-Code-like system prompt** (versioned at `tests/roi/system-prompt.md`). **Hard gate: 2.0× global median across 3 flows × N=10 runs.** Soft per-flow aspirationals: A ≥ 3.0×, B ≥ 2.0×, C ≥ 1.5×. Output: `docs/research/03-code-indexer-roi-claude-code.md`. |
| NR1 | Research §NR1 | Resolved by Q4 + Q5 + Q9.1 above. |
| NR2 | Research §NR2 | **Open**: Phase 02 API surface for retry / circuit breaker / OpenTelemetry must be cited explicitly in §8.2 by Epic 03.2 kickoff. Spec marks this UNRESOLVED — see §20. |
| NR3 | Research §NR3 | Acceptance criteria split into HARD / SOFT gates (§14.x in plan, mirrored in §16 + §17 of this spec). |

---

## 3. Crate Architecture

### 3.1 Crates

```
unblock-indexer-core (lib, MIT, pure Rust, no IO, no async)
  ├─ src/lib.rs                      // re-exports
  ├─ src/types.rs                    // Symbol, SymbolKind, Span, Language, FileRecord, ...
  ├─ src/kind.rs                     // SymbolKind enum + capture → kind mapping
  ├─ src/schema.rs                   // DDL constants (CREATE TABLE / TRIGGER / INDEX)
  ├─ src/manifest.rs                 // Manifest, ManifestEntry; embedded anchor SHA-256
  ├─ src/queries.rs                  // S-expression query string constants per language (compile-time include_str!)
  ├─ src/traversal.rs                // pure AST → Symbol[] given (&Tree, &[u8], &Language, &Query)
  ├─ src/comment.rs                  // doc-comment attachment per language family
  ├─ src/errors.rs                   // IndexerError enum (snafu) + Result<T>
  └─ queries/
        ├─ rust.scm
        ├─ typescript.scm  (covers tsx + ts)
        ├─ javascript.scm
        ├─ python.scm
        ├─ go.scm
        ├─ java.scm
        ├─ c.scm
        ├─ cpp.scm
        ├─ ruby.scm
        └─ php.scm

unblock-indexer (lib, MIT, async, sqlx + tokio)
  ├─ src/lib.rs                      // public façade: Indexer { open, query, reindex, status }
  ├─ src/db.rs                       // sqlx pool, migrations, PRAGMA, FTS5 sync invariants
  ├─ src/grammar/
  │     ├─ fetcher.rs                // reqwest + Phase 02 retry/circuit-breaker; SHA-256 verify
  │     ├─ store.rs                  // WasmStore cache (one per (engine, language))
  │     └─ loader.rs                 // tree_sitter::WasmStore wrapper, Engine config
  ├─ src/walker.rs                   // ignore::WalkBuilder + force_include + default excludes
  ├─ src/watcher.rs                  // notify-debouncer-full + FileIdMap; rename / atomic-save
  ├─ src/bootstrap.rs                // rayon fan-out + chunked transactions
  ├─ src/parse.rs                    // tree-sitter Parser pool (per-language) + traversal driver
  ├─ src/query.rs                    // find_symbol/list_symbols/outline/search_text/get_symbol/find_references
  ├─ src/reindex.rs                  // reindex(path?)
  ├─ src/cache.rs                    // XDG path resolution, repo-hash
  ├─ src/config.rs                   // .unblock/indexer.toml + .unblock/languages.toml loaders
  └─ src/errors.rs                   // crate-scoped errors composing IndexerError

unblock-mcp (bin, modified)
  └─ src/tools/indexer/              // 9 new tool handlers, schemars-backed
        ├─ find_symbol.rs
        ├─ list_symbols.rs
        ├─ outline.rs
        ├─ get_symbol.rs
        ├─ search_text.rs
        ├─ find_references.rs        // description MUST contain "HEURISTIC"
        ├─ list_languages.rs
        ├─ index_status.rs
        └─ reindex.rs
  └─ src/cli/                        // new: init, register subcommands
        ├─ init.rs                   // one-shot wizard
        ├─ register.rs               // `register --host=<x>`
        └─ host/                     // per-host config writers
              ├─ claude_desktop.rs
              ├─ claude_code.rs
              ├─ cursor.rs
              ├─ zed.rs
              ├─ vscode.rs
              └─ jetbrains.rs        // print-only; "Import from Claude" instructions
```

### 3.2 Boundary rule

`unblock-indexer-core` MUST compile without `tokio`, `sqlx`, `reqwest`, `notify`, `ignore`, or `rayon`. Permitted dependencies: `serde`, `serde_json`, `snafu`, `tree-sitter` (parsing only), `toml` (manifest), `sha2` (manifest verification helpers used in tests). Any introduction of an IO/async dependency must be flagged as a spec deviation.

### 3.3 Workspace conventions

- `#![deny(unsafe_code)]` on both crates.
- `snafu` exclusively for errors. Crate-scoped `Result<T>` aliases.
- No `unwrap()` / `expect()` outside test modules.
- Module-level `//!` docs on every module; `///` docs on every `pub` item.
- `tracing` for logs; structured JSON to **stderr** only (stdout is reserved for MCP stdio).

---

## 4. Domain Types (`unblock-indexer-core`)

> Crate: `unblock-indexer-core/src/types.rs` and friends.

### 4.1 `Language`

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,   // canonical grammar: tree-sitter-typescript "tsx" parser (handles .ts + .tsx)
    JavaScript,   // covers .js + .jsx
    Python,
    Go,
    Java,
    C,
    Cpp,          // serialised as "cpp"
    Ruby,
    Php,
}
```

Methods:

- `pub fn from_extension(ext: &str) -> Option<Language>` — extension → language map (see §9.2).
- `pub fn as_str(&self) -> &'static str` — wire form (`"rust"`, `"typescript"`, ...).
- `pub fn grammar_asset_name(&self, version: &str) -> String` — `"tree-sitter-<lang>-<version>.wasm"`.

Per Resolution Q5/R3: TypeScript files (`.ts`, `.tsx`) all map to `Language::TypeScript`; the fetched WASM is `tree-sitter-tsx-<version>.wasm` (not `tree-sitter-typescript`). `.js` and `.jsx` map to `Language::JavaScript`.

### 4.2 `SymbolKind` (Resolution Q5 — all 16)

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Module,
    Namespace,
    Variable,
    Constant,
    TypeAlias,
    Macro,
    Field,
    Property,
    Import,
    Export,
}
```

Wire form is the snake_case identifier. The `kind` column in `symbols` stores the wire form. The mapping `(Language, capture_name) → SymbolKind` lives in `kind.rs::map_capture_to_kind()` and is exhaustive per language; an unknown capture in a vendored `.scm` is a hard error at traversal time (catches grammar drift early).

### 4.3 `Span`

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start_line: u32,   // 1-based
    pub start_col:  u32,   // 1-based, byte offset within line
    pub end_line:   u32,
    pub end_col:    u32,
}
```

Bytes-based columns. UTF-8 decoding is the caller's concern at display time.

### 4.4 `Symbol`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id:         SymbolId,           // newtype around i64; opaque to MCP clients
    pub name:       String,
    pub kind:       SymbolKind,
    pub language:   Language,
    pub file:       String,             // repo-relative, forward slashes
    pub span:       Span,
    pub signature:  Option<String>,     // head line(s) only — see §10.4
    pub comment:    Option<String>,     // attached doc-comment per §10.5
    pub parent_id:  Option<SymbolId>,
}
```

`SymbolId` is `pub struct SymbolId(pub i64);` with `Display`/`FromStr` for opaque transport. MCP tool clients MUST NOT parse it.

### 4.5 `FileRecord`

```rust
pub struct FileRecord {
    pub path:         String,
    pub language:     Language,
    pub mtime:        i64,             // unix seconds
    pub content_hash: String,          // hex SHA-256 of file bytes
    pub parsed_at:    i64,
}
```

### 4.6 `Manifest`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub release_tag:    String,
    pub generated_at:   String,
    pub entries:        Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub language:                Language,
    pub grammar_repo:            String,        // e.g. "tree-sitter/tree-sitter-rust"
    pub grammar_version:         String,        // upstream grammar tag, e.g. "v0.24.2"
    pub tree_sitter_abi_version: u16,           // 14 or 15
    pub asset_name:              String,        // tree-sitter-<lang>-<version>.wasm
    pub sha256:                  String,        // hex
    pub size_bytes:              u64,
}
```

Persisted as TOML. The constant `pub const TRUSTED_MANIFEST_SHA256: &str = "..."` lives in `manifest.rs` and is updated each release via the CI release job (the value commits with the tag).

---

## 5. Storage Layer (sqlx + FTS5)

> Crate: `unblock-indexer/src/db.rs`. Schema constants live in `unblock-indexer-core/src/schema.rs`.

### 5.1 Connection & PRAGMA invariants

On first connect to `<repo-cache>/index.db`:

1. `SqliteConnectOptions::new().filename(...).create_if_missing(true).journal_mode(SqliteJournalMode::Wal).foreign_keys(true).pragma("temp_store", "MEMORY").pragma("synchronous", "NORMAL").pragma("wal_autocheckpoint", "1000")`.
2. Run the migration set (see §5.4).
3. **Assert FTS5 availability** by executing `PRAGMA compile_options;` and confirming the row `ENABLE_FTS5` is present. If absent, emit `IndexerError::Fts5Unavailable { compile_options: Vec<String> }` and refuse to serve queries. Per Research §R4.

Connection pool: `SqlitePool` configured with `max_connections = 4`. WAL allows N readers concurrently with 1 writer; bootstrap holds the writer in chunked transactions (§11.1).

### 5.2 Schema (DDL constants)

The DDL strings below live verbatim as `pub const SCHEMA_VN: &str` in `schema.rs`.

```sql
-- 5.2.1 symbols (content table)
CREATE TABLE symbols (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,
  kind        TEXT NOT NULL,
  language    TEXT NOT NULL,
  file        TEXT NOT NULL,
  start_line  INTEGER NOT NULL,
  start_col   INTEGER NOT NULL,
  end_line    INTEGER NOT NULL,
  end_col     INTEGER NOT NULL,
  signature   TEXT,
  comment     TEXT,                              -- Resolution Q4: column added
  parent_id   INTEGER REFERENCES symbols(id) ON DELETE SET NULL
);
CREATE INDEX idx_symbols_name     ON symbols(name);
CREATE INDEX idx_symbols_file     ON symbols(file);
CREATE INDEX idx_symbols_kind     ON symbols(kind);
CREATE INDEX idx_symbols_lang     ON symbols(language);
CREATE INDEX idx_symbols_parent   ON symbols(parent_id);

-- 5.2.2 symbols_fts (external-content FTS5)
CREATE VIRTUAL TABLE symbols_fts USING fts5(
  name, signature, comment,
  content='symbols',
  content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);

-- 5.2.3 sync triggers (insert / delete / update — canonical FTS5 external-content pattern)
CREATE TRIGGER symbols_ai AFTER INSERT ON symbols BEGIN
  INSERT INTO symbols_fts(rowid, name, signature, comment)
  VALUES (new.id, new.name, new.signature, new.comment);
END;

CREATE TRIGGER symbols_ad AFTER DELETE ON symbols BEGIN
  INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, comment)
  VALUES ('delete', old.id, old.name, old.signature, old.comment);
END;

CREATE TRIGGER symbols_au AFTER UPDATE ON symbols BEGIN
  INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, comment)
  VALUES ('delete', old.id, old.name, old.signature, old.comment);
  INSERT INTO symbols_fts(rowid, name, signature, comment)
  VALUES (new.id, new.name, new.signature, new.comment);
END;

-- 5.2.4 files
CREATE TABLE files (
  path          TEXT PRIMARY KEY,
  language      TEXT NOT NULL,
  mtime         INTEGER NOT NULL,
  content_hash  TEXT NOT NULL,
  parsed_at     INTEGER NOT NULL
);

-- 5.2.5 meta (key/value strings)
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT
);
-- Required keys at bootstrap completion:
--   indexer_version  = env!("CARGO_PKG_VERSION") of unblock-indexer
--   schema_version   = "1"
--   repo_root        = canonicalised absolute path
--   last_full_index  = unix seconds
--   last_incremental = unix seconds
```

### 5.3 FTS5 rebuild after `reindex`

`reindex(path?)` truncates `symbols` for the affected path subtree (or all rows when `path = None`) and bulk-reparses. After truncation the spec mandates:

```sql
INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');
```

…to ensure the FTS5 index is consistent with the freshly populated content table (§11.3).

### 5.4 Migrations

Migrations live under `unblock-indexer/migrations/` as numbered SQL files (`0001_init.sql`, ...). Initial schema is `0001_init.sql` containing §5.2.1–§5.2.5 verbatim. `sqlx::migrate!()` runs them on connect.

Pre-production stance (Plan §49): no backfill / data migration logic for v1.0.0. A schema bump invalidates the cache — the bootstrap phase rebuilds from the filesystem.

### 5.5 WAL contention rule (Research §R4 risk)

Bootstrap MUST NOT hold a single transaction over the whole repo. Instead it MUST chunk by **2 048 inserts** (or 256 files, whichever is smaller) per transaction. Between chunks the writer commits, releasing the WAL writer-lock so reads can proceed. This bounds reader-starvation latency.

---

## 6. Cache Layout

> Crate: `unblock-indexer/src/cache.rs`.

### 6.1 Path resolution

```
$XDG_CACHE_HOME (if set, validated absolute)  →  default = $HOME/.cache
└─ unblock/
   ├─ grammars/
   │   ├─ manifest.toml
   │   └─ tree-sitter-<lang>-<version>.wasm   (one per language pin)
   └─ repos/
       └─ <repo-hash>/
           ├─ index.db
           ├─ index.db-wal
           ├─ index.db-shm
           └─ meta.toml
```

Windows: `%LOCALAPPDATA%\unblock\` per `dirs::cache_dir()`; same sub-tree.

### 6.2 `<repo-hash>`

```
<repo-hash> = hex(SHA-256(canonicalised_absolute_repo_root_path))[..16]
```

16 hex chars = 64 bits — collision probability negligible at single-user scale and avoids long path issues on Windows.

### 6.3 `meta.toml`

```toml
schema_version  = 1
indexer_version = "1.0.0"
repo_root       = "/Users/.../unblock"
last_bootstrap  = 1761600000
```

If `schema_version` or `indexer_version` differs from the running binary, the indexer treats the cache as cold and triggers a full re-bootstrap.

---

## 7. Grammar Pipeline (CI)

> File: `.github/workflows/grammars.yml` (new). Owner: infra-supervisor.

### 7.1 Workflow shape

Matrix: 10 entries (one per `Language`). Each entry pins:

| Field | Source |
|---|---|
| `language` | `Language` enum value (lowercase wire form) |
| `repo` | `tree-sitter/tree-sitter-<lang>` |
| `tag` | upstream grammar tag (R3 audit table) |
| `tree_sitter_cli_version` | `>= 0.26.7` (Research §R1 — wasi-sdk path) |

Per cell:

1. Check out the upstream grammar repo at `tag`.
2. Install tree-sitter CLI ≥ 0.26.7 and `wasi-sdk`.
3. Run `tree-sitter generate`.
4. Run `tree-sitter build --wasm`.
5. Compute SHA-256 of `tree-sitter-<lang>.wasm`.
6. Rename to `tree-sitter-<lang>-<tag>.wasm`.
7. Read `LANGUAGE_VERSION` from generated parser.c (or via `tree-sitter dump-language-version`).
8. Emit one TOML row to a workflow artefact (`manifest-<lang>.toml`).
9. Upload the WASM blob as a workflow artefact.

**Special-case TypeScript:** the cell for TypeScript builds the **`tsx`** parser only (not the `typescript` sub-parser). Asset name is `tree-sitter-tsx-<tag>.wasm`. The `Language::TypeScript` variant resolves to this asset at runtime.

### 7.2 Release job

After all matrix cells pass:

1. Concatenate the per-language manifest fragments into `manifest.toml` ordered by `Language` discriminant.
2. Compute `manifest.toml`'s SHA-256.
3. **Verify** that the `TRUSTED_MANIFEST_SHA256` constant in `unblock-indexer-core/src/manifest.rs` on `main` matches. If not, fail the job with an actionable message ("commit the new constant, then re-run").
4. Create a release tagged `v<unblock-version>-grammars` with assets: all 10 WASMs + `manifest.toml`.

### 7.3 Stale-grammar audit (Research §R3)

A separate job in the same workflow lists each pinned grammar's last-release date and emits a warning (does not fail) if `now - last_release > 365 days`. Initial flagged set per Research §R3: TypeScript, Java, C++, Ruby. The warning surfaces in the release notes.

### 7.4 Asset URL pattern

Runtime fetcher constructs:

```
https://github.com/websublime/unblock/releases/download/<release-tag>/<asset-name>
```

This is `browser_download_url` and does **not** consume the GitHub API rate budget (Research §R1).

---

## 8. Grammar Runtime (Fetcher + WASM Loader)

> Crate: `unblock-indexer/src/grammar/`.

### 8.1 Fetcher (`fetcher.rs`)

```rust
pub async fn ensure_grammar(
    lang: Language,
    cache_root: &Path,
    http: &Phase02ResilientClient,
) -> Result<PathBuf, IndexerError>;
```

Algorithm:

1. **Locate manifest.** If `<cache_root>/grammars/manifest.toml` is missing or its SHA-256 does not match `TRUSTED_MANIFEST_SHA256`, fetch `manifest.toml` from §7.4 URL pattern, verify SHA-256, atomically rename into place.
2. **Look up entry** for `lang` in the manifest. If absent → `IndexerError::LanguageNotInManifest { lang, pr_pointer }`.
3. **ABI pre-check.** Compare `entry.tree_sitter_abi_version` against `tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION ..= tree_sitter::LANGUAGE_VERSION`. Out of range → `IndexerError::AbiMismatch { lang, abi: entry.abi, supported: range }` (Research §R2).
4. **Cache hit?** If `<cache_root>/grammars/<entry.asset_name>` exists and its SHA-256 matches `entry.sha256`, return its path.
5. **Fetch.** GET the §7.4 URL via `Phase02ResilientClient` (reuses retry-with-backoff + circuit-breaker — see §8.2). Streamed to a temp file in the same directory.
6. **Verify SHA-256.** Mismatch → delete tempfile + `IndexerError::IntegrityFailed`.
7. **Atomic rename** into place; return path.

Concurrency: a `tokio::sync::Mutex<HashMap<Language, Arc<OnceCell<PathBuf>>>>` deduplicates concurrent first-touch fetches.

### 8.2 Phase 02 dependency surface (UNRESOLVED — see §20)

This spec assumes `Phase02ResilientClient` exposes:

- `async fn get_bytes(&self, url: &str) -> Result<Bytes, Phase02Error>` with retry + circuit breaker.
- A constructor that accepts a `reqwest::Client` and the existing OpenTelemetry meter.

The exact symbol name and crate location must be confirmed against Phase 02's merged surface before Epic 03.2 opens beads. **Action**: Epic 03.2's first bead is "verify Phase 02 surface and update §8.2 in this spec."

### 8.3 WASM loader (`store.rs`, `loader.rs`)

```rust
// One process-wide engine; cheap to clone (Arc internally).
static ENGINE: OnceCell<wasmtime::Engine> = OnceCell::new();

pub struct GrammarStore {
    inner: tokio::sync::Mutex<tree_sitter::WasmStore>,
    languages: dashmap::DashMap<Language, tree_sitter::Language>,
}

impl GrammarStore {
    pub async fn new() -> Result<Self, IndexerError>;
    pub async fn load(&self, lang: Language, wasm_path: &Path) -> Result<tree_sitter::Language, IndexerError>;
    pub async fn parser_for(&self, lang: Language) -> Result<tree_sitter::Parser, IndexerError>;
}
```

- `WasmStore` is `Send + Sync` (Research §R2) but `Parser::set_wasm_store` is per-parser; the spec keeps a parser pool keyed by `Language` so `WasmStore` compilation cost (50–150 ms cold) amortises across queries.
- Lazy-load: a language's WASM is loaded only on first encounter (Plan §8.1 step 2 threshold = ≥ 1 file).
- The `wasmtime::Engine` is created once with default config for v1.0.0. The on-disk wasmtime artefact cache is **not** enabled in v1.0.0 (Research §R2 open question deferred — see §20).

---

## 9. File Walker & Language Detection

> Crate: `unblock-indexer/src/walker.rs`.

### 9.1 `WalkBuilder` configuration (Research §R7)

```rust
WalkBuilder::new(repo_root)
    .require_git(false)        // FOOTGUN FIX — gitignore even on tarball checkouts
    .hidden(true)              // ignore .git, .venv, etc.
    .git_ignore(true)
    .git_global(true)
    .git_exclude(true)
    .same_file_system(true)    // do not cross volume boundaries
    .add_custom_ignore_filename(".unblock-ignore")  // future-friendly
    .build()
```

After construction, the walker layers in two filters:

1. **Default-excludes glob set** (Plan §L6): `target/`, `node_modules/`, `dist/`, `build/`, `.venv/`, `vendor/`, `.git/`. Implemented via `ignore::overrides::OverrideBuilder`.
2. **`force_include`** (Resolution Q7): user-provided globs from `.unblock/indexer.toml` `[walker].force_include` override `.gitignore` but **not** the default-excludes above. Implemented as a second-pass override applied after `WalkBuilder` produces an entry — entries excluded only by `.gitignore` are re-admitted; entries excluded by default-excludes stay excluded.

### 9.2 Extension → language map

```
.rs                 → Rust
.ts, .tsx           → TypeScript    (parser: tsx)
.js, .jsx, .mjs, .cjs → JavaScript
.py, .pyi           → Python
.go                 → Go
.java               → Java
.c, .h              → C
.cc, .cpp, .cxx, .hpp, .hh, .hxx → Cpp
.rb, .rake          → Ruby
.php, .phtml        → Php
```

Overrides via `.unblock/languages.toml` (Plan §L6) — additive, may map an unknown extension onto an existing `Language` only. Adding *new* languages still requires the CI grammar pipeline (Plan §14.2).

### 9.3 Language detection threshold

A language is "active" for the repo if **≥ 1** file matches its extension set after walking. `list_languages` reports the active set with the per-language file count.

---

## 10. AST Traversal & Symbol Extraction

> Crate: `unblock-indexer-core/src/traversal.rs` (pure) + `unblock-indexer/src/parse.rs` (driver).

### 10.1 Per-language query files

Each `crates/unblock-indexer-core/queries/<lang>.scm` is an extension of upstream `tags.scm` with hand-written captures for the four extras (`field`, `property`, `import`, `export`) per Resolution Q5. Query files are loaded at compile time via `include_str!` into `queries.rs`:

```rust
pub const RUST_QUERY:       &str = include_str!("../queries/rust.scm");
pub const TYPESCRIPT_QUERY: &str = include_str!("../queries/typescript.scm");
// ... 8 more
pub fn query_for(lang: Language) -> &'static str;
```

A property test (§18) compiles every query string against the corresponding loaded grammar at test time — guarantees the vendored `.scm` parses.

### 10.2 Capture vocabulary

Per-language queries MUST emit one `@definition.<kind>` capture per symbol where `<kind>` is from §4.2's wire vocabulary, plus a `@name` capture inside it. Comments (§10.5) are matched separately via `@doc.<kind>`.

Mapping `(Language, capture_name) → SymbolKind` is `kind::map_capture_to_kind()`. The mapping is exhaustive per language; a query that emits a capture not in the mapping triggers `IndexerError::UnknownCapture { lang, capture }`. This catches grammar drift early.

### 10.3 Traversal contract (pure)

```rust
pub fn extract_symbols(
    tree:      &tree_sitter::Tree,
    source:    &[u8],
    language:  Language,
    query:     &tree_sitter::Query,
    file_path: &str,
) -> Result<Vec<RawSymbol>, IndexerError>;
```

`RawSymbol` is `Symbol` *without* `parent_id` and *without* a `SymbolId` — those are assigned by the storage driver after insert.

Algorithm:

1. Walk `tree_sitter::QueryCursor::matches(query, tree.root_node(), source)`.
2. For each match, locate the `@definition.<kind>` parent capture and its inner `@name` child.
3. Produce a `RawSymbol`:
   - `name` = UTF-8 slice of `@name` (bytes-strict; replace lone surrogates with `U+FFFD`).
   - `kind` via §10.2 mapping.
   - `span` from the `@definition.<kind>` node.
   - `signature` per §10.4.
   - `comment` per §10.5.
4. Order results by `(span.start_line, span.start_col)` for deterministic output.

Determinism is a property test invariant: extracting twice from the same `(tree, source)` yields byte-identical `Vec<RawSymbol>`.

### 10.4 Signature extraction

`signature` is the substring of `source` from the symbol's start byte to **the byte before the first opening brace, equals sign, colon, or newline**, whichever comes first, capped at **256 bytes**. For grammars with multi-line declaration heads (Rust `where` clauses, Java method signatures with annotations), the cap is sufficient; truncated signatures end with `…`. Bodies are never stored — the FS is canonical (Plan §L10).

### 10.5 Comment attachment (Resolution Q4)

Per-language doc-comment heuristics:

| Language(s) | Rule |
|---|---|
| Rust | Concatenate consecutive `///` and `//!` lines immediately preceding the symbol's start line. |
| TypeScript / JavaScript / Java / C / C++ / PHP | The `/** ... */` block ending on the line before the symbol's start line, if any. |
| Python | The first string literal statement inside the `def`/`class` body (the docstring). |
| Go | Consecutive `//`-prefixed lines immediately preceding the symbol's start line, no blank line between. |
| Ruby | Consecutive `#`-prefixed lines or a `=begin`/`=end` block immediately preceding the symbol. |

Implementation: each language has a `comment.rs::attach_<lang>()` function. The heuristic operates on `source` bytes and the symbol's start position — pure, deterministic, no IO. Comments are capped at **2 048 bytes** (truncated with `…`) to bound FTS5 index size.

### 10.6 Parent linkage

`parent_id` is computed post-traversal in the storage driver:

1. After symbols are emitted in source order, walk them and for each symbol find the smallest enclosing symbol whose span strictly contains it; that symbol's id is the parent.
2. Top-level symbols have `parent_id = NULL`.
3. The DB insert pass is two-phase: first pass inserts with `parent_id = NULL` and records `(temp_index → row_id)`; second pass updates `parent_id` for non-top-level rows.

This is correct for nested classes/methods/structs, the only hierarchies tree-sitter `tags.scm` exposes flatly. Cross-file relationships (e.g. Rust `impl` blocks) are not modelled in v1.0.0 (Plan §3 — out of scope).

---

## 11. Lifecycle: Bootstrap, Watch, Steady-State

> Crate: `unblock-indexer/src/{bootstrap,watcher,reindex}.rs`.

### 11.1 Cold bootstrap

1. **Open or create cache.** Resolve `<repo-hash>`; load `meta.toml` if present; if `schema_version` or `indexer_version` differs → cold path.
2. **Walk repo** (§9). Group entries by `Language`.
3. **Ensure grammars** in parallel (§8.1): one `tokio::spawn` per active language.
4. **Open SQLite pool** (§5.1). Run migrations.
5. **Parallel parse + chunked insert.** Use `rayon::scope` for the parse step (CPU-bound). Insert from a single async task that drains a channel; commit every 2 048 rows or 256 files.
   - For each file: read bytes, hash, parse with the language's `Parser` from §8.3, run `extract_symbols` (§10.3), enqueue `(FileRecord, Vec<RawSymbol>)`.
6. **Compute parent linkage** (§10.6) per file, immediately after the file's symbols land.
7. **Update `meta.toml`** with `last_bootstrap = now`, `last_full_index = now`.
8. **Spawn watcher** (§11.2).
9. Emit a single tracing event `indexer.bootstrap.complete { files, symbols, duration_ms }`.

Progress logging: every 1 000 files or every 5 seconds (whichever first), emit `indexer.bootstrap.progress { files_done, files_total }` at INFO.

### 11.2 Watcher

`notify-debouncer-full` configured with:

```rust
let cache  = FileIdMap::new();
let debouncer = new_debouncer_with_cache(
    Duration::from_millis(config.watcher.debounce_ms.unwrap_or(500)),  // R6 default
    None,                       // tick_rate = default
    cache,
    move |events| { tx.blocking_send(events).ok(); },
)?;
debouncer.watcher().watch(repo_root, RecursiveMode::Recursive)?;
```

Per Research §R6: default 500 ms debounce, configurable down to 200 ms via `.unblock/indexer.toml`. Linux inotify-init failures emit `IndexerError::WatcherInit { hint: "Increase fs.inotify.max_user_watches (current: ...)" }`.

Event handling:

| Event | Action |
|---|---|
| `Create(path)` | If `path` matches a known extension and passes the walker filter, parse + insert. |
| `Modify(path)` | Re-parse + delete-old-symbols + insert-new in one transaction. |
| `Remove(path)` | `DELETE FROM files WHERE path = ?`; `DELETE FROM symbols WHERE file = ?` (triggers cascade FTS5 delete). |
| `Rename(from, to)` | Treated as `Remove(from) + Create(to)` in v1.0.0 (Plan §8.2). |

### 11.3 Per-query mtime check (INVARIANT — Research §R6, §R8)

For every read tool call:

1. Determine the **implicated files** (§16.2): files whose mtime drift could change the result.
2. For each implicated path: `stat` the file. If `mtime > files.mtime` (or content hash mismatch on a sample), trigger a **synchronous** re-parse for that single file before serving the query.
3. The re-parse uses the same parser pool as steady-state.

**Implicated-file rule per tool:**

| Tool | Implicated set |
|---|---|
| `find_symbol(name, ...)` | The file owning the matched row, on a per-row basis (post-query). Matches whose owning file has changed are re-parsed and the search re-runs against the updated rows. Cap: re-parse at most 4 files per call; remaining matches served as-is with a `stale` flag. |
| `list_symbols(path)` | All files under `path` (recursively if `recursive=true`). |
| `outline(path)` | The single file `path`. |
| `get_symbol(symbol_id)` | The file owning the row. |
| `search_text(query, ...)` | None (FTS5 results are best-effort across the full index — re-parsing every match is unbounded). Stale results explicitly accepted; users requiring freshness must call `reindex`. |
| `find_references(...)` | None (heuristic; freshness not promised). |
| `list_languages` / `index_status` | None. |

### 11.4 Forced reindex

`reindex(path?)` semantics:

1. If `path = None`: `DELETE FROM symbols; DELETE FROM files; INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');` then run §11.1 step 5–7.
2. If `path = Some(p)`: delete rows under the path subtree; reparse only that subtree; rebuild FTS5 only for affected rows (the per-row `'delete'` + reinsert via triggers handles it without a global rebuild).
3. Returns `{ files_reparsed, symbols_emitted, duration_ms }`.

`reindex` runs synchronously; the caller blocks until completion. Watcher events received during a reindex are queued and processed after.

---

## 12. MCP Tool Surface

> Crate: `unblock-mcp/src/tools/indexer/`.

### 12.1 Common conventions

- All tools are registered through the existing `rmcp` server alongside the issue-graph tools.
- Input/output schemas via `schemars` derived structs.
- All errors map to `IndexerError → unblock_mcp::McpError` per the existing convention; see §14.
- All output JSON uses `snake_case`.
- `symbol_id` is rendered as a string (the decimal form of `SymbolId.0`) on the wire to discourage parsing.

### 12.2 Tool reference

#### 12.2.1 `find_symbol`

```rust
struct FindSymbolInput {
    name:     String,
    kind:     Option<SymbolKind>,
    language: Option<Language>,
    limit:    Option<u32>,    // default 20, max 100
    fuzzy:    Option<bool>,   // default false
}

struct FindSymbolOutput {
    matches: Vec<FindSymbolMatch>,
    stale:   bool,            // true if any implicated re-parse was capped (§11.3)
}
struct FindSymbolMatch {
    symbol_id: String,
    name:      String,
    kind:      SymbolKind,
    language:  Language,
    file:      String,
    span:      Span,
    signature: Option<String>,
}
```

SQL (non-fuzzy): `SELECT ... FROM symbols WHERE name = ? AND (kind IS ? OR ?) AND (language IS ? OR ?) ORDER BY file, start_line LIMIT ?`. With `idx_symbols_name` this is sub-ms.

SQL (fuzzy): `SELECT ... FROM symbols_fts WHERE name MATCH ? || '*'` joined to `symbols`. Prefix MATCH gives ~1–5 ms.

#### 12.2.2 `list_symbols`

```rust
struct ListSymbolsInput {
    path:      String,           // repo-relative
    kinds:     Option<Vec<SymbolKind>>,
    recursive: Option<bool>,     // default false
}

struct ListSymbolsOutput { symbols: Vec<ListSymbolsRow> }
struct ListSymbolsRow {
    symbol_id: String,
    name:      String,
    kind:      SymbolKind,
    span:      Span,
    parent_id: Option<String>,
}
```

`recursive=true` uses `WHERE file LIKE ? || '/%' OR file = ?` against `idx_symbols_file`.

#### 12.2.3 `outline`

```rust
struct OutlineInput { path: String }
struct OutlineOutput {
    file:     String,
    language: Language,
    tree:     Vec<OutlineNode>,
}
struct OutlineNode {
    symbol_id: String,
    name:      String,
    kind:      SymbolKind,
    span:      Span,
    children:  Vec<OutlineNode>,
}
```

Tree assembled in-process from a single `SELECT ... WHERE file = ? ORDER BY parent_id NULLS FIRST, start_line`.

#### 12.2.4 `get_symbol`

```rust
struct GetSymbolInput  { symbol_id: String }
struct GetSymbolOutput {
    symbol_id: String,
    name:      String,
    kind:      SymbolKind,
    language:  Language,
    file:      String,
    span:      Span,
    signature: Option<String>,
    comment:   Option<String>,
    parent_id: Option<String>,
    body:      String,           // read from FS at query time, bounded by span
}
```

`body` is read from the filesystem using the symbol's span, capped at **64 KiB** (truncated with a trailing `\n…\n` marker if exceeded). Reads outside the repo root are rejected (path traversal guard).

#### 12.2.5 `search_text`

```rust
struct SearchTextInput {
    query:    String,         // FTS5 MATCH expression — sanitised per §12.3
    scope:    Option<String>, // path prefix filter
    language: Option<Language>,
    limit:    Option<u32>,    // default 20, max 100
}

struct SearchTextOutput { matches: Vec<SearchTextMatch> }
struct SearchTextMatch {
    symbol_id: String,
    name:      String,
    kind:      SymbolKind,
    file:      String,
    span:      Span,
    snippet:   String,        // FTS5 snippet() of the matched column
}
```

Uses SQLite's built-in `snippet(symbols_fts, -1, '<<', '>>', '…', 32)` for snippet rendering.

#### 12.2.6 `find_references` — **HEURISTIC**

```rust
struct FindReferencesInput {
    name:      Option<String>,
    symbol_id: Option<String>,     // exactly one of name | symbol_id is required
}

struct FindReferencesOutput {
    references: Vec<Reference>,
    heuristic:  bool,              // always true
}
struct Reference {
    file:               String,
    span:               Span,
    surrounding_symbol: Option<String>,    // symbol_id whose span contains this reference
}
```

Implementation: tree-sitter `@reference.*` captures from the per-language query, plus a `LIKE '%name%'` fallback on file content for languages whose query lacks references. **Schema description MUST contain the literal substring `HEURISTIC` and the substring `syntactic only, no type resolution`.** A workspace lint (custom build-time check in `unblock-mcp/build.rs`) enforces both substrings — failure aborts compilation.

#### 12.2.7 `list_languages`

```rust
struct ListLanguagesOutput { languages: Vec<LanguageStatus> }
struct LanguageStatus {
    language:        Language,
    grammar_version: String,        // from manifest entry
    abi_version:     u16,
    file_count:      u64,
}
```

#### 12.2.8 `index_status`

```rust
struct IndexStatusOutput {
    repo_root:        String,
    schema_version:   u32,
    indexer_version:  String,
    last_full_index:  i64,    // unix seconds
    last_incremental: i64,
    total_files:      u64,
    total_symbols:    u64,
    watcher_active:   bool,
    db_size_bytes:    u64,
}
```

#### 12.2.9 `reindex`

```rust
struct ReindexInput  { path: Option<String> }
struct ReindexOutput {
    files_reparsed:  u64,
    symbols_emitted: u64,
    duration_ms:     u64,
}
```

### 12.3 FTS5 query sanitisation

`search_text.query` is passed to FTS5 MATCH. To prevent injection of FTS5 control syntax that could surface internal columns or commands, the input is sanitised by:

1. Trimming.
2. Stripping any leading `'` or `"`.
3. Escaping every double-quote as `""`.
4. Wrapping the entire string in `"..."` (FTS5 phrase quoting), unless the input begins with `prefix:` or `column:` (reserved future syntax — rejected with `IndexerError::InvalidFtsQuery` for v1.0.0).

### 12.4 `find_references` schema-string lint

A test in `unblock-mcp/tests/lints.rs` reads the registered tool descriptions and asserts both substrings (`HEURISTIC`, `syntactic only, no type resolution`) appear in `find_references.description`. Failure breaks `cargo test --workspace`.

---

## 13. Setup & Editor Registration (`init` / `register`)

Per Resolution Q9.1, three coexisting entry points:

| Entry | Caller | When | Scope |
|---|---|---|---|
| `unblock-mcp init` (NEW CLI) | Human in terminal | Onboarding (canonical, one-shot) | Editor register + GitHub Project setup via wizard |
| `unblock-mcp register --host=<x>` (NEW CLI) | Human in terminal or CI | Add editor later, scripted | Editor register only |
| `setup` MCP tool (existing, refactored) | Agent in active session | Self-heal, idempotent re-setup | GitHub Project setup only |

### 13.1 Refactor of `setup` MCP tool

The existing `setup` MCP tool's logic is extracted into a library function:

```rust
// In `unblock-github` (preferred home — it owns the GitHub Projects logic).
pub async fn ensure_github_project(
    client: &GitHubClient,
    repo:   &RepoCoords,
    token:  &SecretString,
) -> Result<SetupReport, SetupError>;
```

The `setup` MCP tool handler becomes a thin wrapper around this function. **Public API unchanged** for MCP clients (idempotent JSON contract preserved). Per workspace convention, this is an additive `API:` line in the commit message (no breaking change to tool callers).

### 13.2 `unblock-mcp init` wizard

CLI flow:

1. **Detect editors installed** by probing the per-host config file existence (Cursor, Zed, VS Code, Claude Code, Claude Desktop). Print findings; ask which to register (multi-select).
2. **GitHub Project setup**: prompt `GITHUB_TOKEN` (read from env if `UNBLOCK_TOKEN`/`GITHUB_TOKEN` set) + `owner/repo`; validate via `client.viewer()`; call `ensure_github_project()` (§13.1).
3. **Register MCP server** in selected editors via §13.3 idempotent merge.
4. **Print JetBrains instructions** (§13.4) if user mentioned JetBrains in their toolchain.
5. **Print summary** + "next steps" (restart editors, then invoke the unblock MCP `ready` tool from inside the agent).

Errors abort cleanly without partial writes; all file edits are atomic (write to `<file>.tmp`, fsync, rename).

### 13.3 `unblock-mcp register --host=<x>`

CLI flags:

```
--host=<cursor | claude-code | claude-desktop | zed | vscode | jetbrains | all>
--scope=<workspace | user>      # default: user
--server-name=<name>            # default: unblock
--print-only                    # dry run (writes nothing; prints JSON to stdout)
--force                         # overwrite existing entry without prompting
```

Per-host writers (`unblock-mcp/src/cli/host/*.rs`) implement a shared trait:

```rust
trait HostRegistrar {
    fn config_paths(&self, scope: Scope) -> Vec<PathBuf>;     // OS-aware
    fn server_entry(&self, name: &str) -> Value;              // host-specific JSON
    fn merge(&self, existing: &mut Value, name: &str, force: bool) -> Result<MergeAction, RegisterError>;
}
```

Per-host top-level keys and entry shapes:

| Host | Top-level key | Entry shape |
|---|---|---|
| Claude Desktop | `mcpServers` | `{ command, args, env }` |
| Claude Code | `mcpServers` | `{ command, args, env }` |
| Cursor | `mcpServers` | `{ command, args, env }` (supports `${env:NAME}`) |
| Zed | **`context_servers`** | `{ command, args, env }` |
| VS Code | **`servers`** | `{ type: "stdio", command, args }` |

Idempotent merge:

1. Read existing config (or initialise empty); preserve all unrelated keys.
2. If `<top-key>.<server-name>` exists and `--force` is unset → prompt (interactive) or fail with `MergeAction::Conflict` (non-interactive); user can rerun with `--force`.
3. Write atomically (`.tmp` + rename).

`--print-only` short-circuits step 3; the resulting JSON is printed to stdout. This is also the path used for JetBrains.

### 13.4 JetBrains (Resolution Q9.2)

No JetBrains-specific code. The `register --host=jetbrains` command:

1. Emits the canonical `mcpServers` JSON (Claude-shaped).
2. Prints the 5-click instructions to stderr:

```
JetBrains AI Assistant supports MCP via "Import from Claude":
  1. Open IDE → Settings → Tools → AI Assistant
  2. Section: Model Context Protocol (MCP)
  3. Click "Add"
  4. Select "Import from Claude"
  5. Pick the `unblock` server entry

Alternative: paste the JSON below into the same dialog.
```

Acceptance criterion (mirrors plan §14.6 reworded per Q9.2): *`init` wizard surfaces unblock entry such that JetBrains user can import via 5-click workflow within 30 seconds.*

### 13.5 Path resolution per host

| Host | OS | Path |
|---|---|---|
| Claude Desktop | macOS | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Claude Desktop | Windows | `%APPDATA%\Claude\claude_desktop_config.json` |
| Claude Desktop | Linux | not officially supported — `register` emits a warning + falls back to `~/.config/Claude/claude_desktop_config.json` (best effort) |
| Claude Code | all | user: `~/.claude.json`; workspace: `.claude/settings.json` |
| Cursor | all | user: `~/.cursor/mcp.json`; workspace: `.cursor/mcp.json` |
| Zed | macOS/Linux | `~/.config/zed/settings.json` |
| Zed | Windows | `%APPDATA%\Zed\settings.json` |
| VS Code | all | user: via "MCP: Open User Configuration" — registrar writes to the documented path; workspace: `.vscode/mcp.json` |

---

## 14. Error Model

> Crate: `unblock-indexer-core/src/errors.rs`.

```rust
#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub))]
pub enum IndexerError {
    #[snafu(display("language not supported: {lang}; contribute via {pr_pointer}"))]
    LanguageNotSupported  { lang: String, pr_pointer: String },

    #[snafu(display("language {lang:?} missing from manifest; contribute via {pr_pointer}"))]
    LanguageNotInManifest { lang: Language, pr_pointer: String },

    #[snafu(display("grammar fetch failed for {lang:?}: {source}"))]
    GrammarFetch          { lang: Language, source: BoxedError },

    #[snafu(display("integrity check failed for {asset}: expected {expected}, got {actual}"))]
    IntegrityFailed       { asset: String, expected: String, actual: String },

    #[snafu(display("ABI mismatch for {lang:?}: grammar abi={abi}, supported={supported_min}..={supported_max}"))]
    AbiMismatch           { lang: Language, abi: u16, supported_min: u16, supported_max: u16 },

    #[snafu(display("FTS5 unavailable: SQLite was not built with ENABLE_FTS5; compile_options={compile_options:?}"))]
    Fts5Unavailable       { compile_options: Vec<String> },

    #[snafu(display("watcher init failed: {hint}"))]
    WatcherInit           { hint: String },

    #[snafu(display("parse failed for {file}: {source}"))]
    ParseFailed           { file: String, source: BoxedError },

    #[snafu(display("file not found: {path}"))]
    FileNotFound          { path: String },

    #[snafu(display("symbol not found: {symbol_id}"))]
    SymbolNotFound        { symbol_id: String },

    #[snafu(display("invalid FTS5 query: {reason}"))]
    InvalidFtsQuery       { reason: String },

    #[snafu(display("unknown capture in {lang:?} query: {capture}"))]
    UnknownCapture        { lang: Language, capture: String },

    #[snafu(display("db locked: {source}"))]
    DbLocked              { source: BoxedError },

    #[snafu(display("io: {source}"))]
    Io                    { source: std::io::Error },

    #[snafu(display("config: {reason}"))]
    Config                { reason: String },
}

pub type Result<T, E = IndexerError> = std::result::Result<T, E>;
```

`pr_pointer` is a stable URL constant: `https://github.com/websublime/unblock/blob/main/CONTRIBUTING.md#adding-a-language`.

### 14.1 MCP error mapping

`unblock-mcp` converts `IndexerError` into `rmcp::Error` with structured codes:

| `IndexerError` variant | MCP code |
|---|---|
| `LanguageNotSupported`, `LanguageNotInManifest` | `unsupported_language` |
| `GrammarFetch` | `grammar_fetch_failed` |
| `IntegrityFailed`, `AbiMismatch` | `grammar_integrity` |
| `Fts5Unavailable` | `fts5_unavailable` |
| `WatcherInit` | `watcher_init_failed` |
| `ParseFailed` | `parse_failed` |
| `FileNotFound`, `SymbolNotFound` | `not_found` |
| `InvalidFtsQuery` | `invalid_input` |
| `DbLocked` | `db_locked` |
| `UnknownCapture`, `Io`, `Config` | `internal` |

---

## 15. Configuration

Two TOML files under `<repo-root>/.unblock/`. Both are optional.

### 15.1 `.unblock/indexer.toml`

```toml
[watcher]
debounce_ms     = 500          # default 500; recommended 200 for interactive
                               # (Research §R6)

[walker]
force_include   = []           # globs that override .gitignore (Resolution Q7)
                               # NEVER overrides the hardcoded default-excludes

[languages]
                               # Optional: disable an auto-detected language
disabled        = []           # e.g. ["ruby"] — skip parsing .rb files
```

### 15.2 `.unblock/languages.toml`

```toml
[extensions]
                               # additive: extend an existing Language's extension set
".mts" = "typescript"
```

A given extension may map to **at most one** Language; collisions abort `init`/bootstrap with `IndexerError::Config`.

### 15.3 Environment

| Variable | Effect |
|---|---|
| `XDG_CACHE_HOME` | Overrides `$HOME/.cache` for the cache root (§6.1). |
| `UNBLOCK_INDEXER_LOG` | tracing filter override; default `info`. |
| `UNBLOCK_GRAMMAR_RELEASE_TAG` | Override the manifest release tag (CI testing only; not user-facing). |

---

## 16. Performance Methodology & Gates

### 16.1 Corpora (Research §R8)

| Tier | Repo | Files | Symbols | Gate |
|---|---|---|---|---|
| Small | this repo (unblock @ v1.0.0) | ~500 | ~5 000 | informational |
| Medium | ripgrep + tokio combined | ~5 000 | ~50 000 | **HARD gate** |
| Large | LLVM project subset | ~50 000 | ~500 000 | informational; bootstrap budget set per measurement |

### 16.2 Implicated-file rule (codified)

The per-query mtime check (§11.3) MUST follow the implicated-file table in §11.3. This bounds the worst-case re-parse to ≤ 4 files for `find_symbol` and exactly 1 file for `outline` / `get_symbol` / `list_symbols (non-recursive)`. Recursive `list_symbols` paths SHOULD cap re-parses at 16 files per call; remaining stale files are reported via the response's `stale: true` flag (extension already present in `FindSymbolOutput` — adopt for `ListSymbolsOutput` as well; spec §12.2.2 amended accordingly).

### 16.3 `criterion` harness

`crates/unblock-indexer/benches/queries.rs` using `criterion` + `async_tokio`:

- One bench function per tool × corpus tier.
- Warm-path measurement: each bench iteration runs the query 50 times; the first 5 are discarded (cold), p99 is computed from the remaining 45 × N samples.
- `cold_start` separate bench (informational): measures first-call latency including grammar load + parser init.

### 16.4 HARD gates (Plan §14.4)

- `find_symbol` p99 < 10 ms on Medium.
- `outline` p99 < 20 ms on Medium.
- WAL contention: under simultaneous reader (random `find_symbol`) + writer (single watcher-driven re-parse), readers' p99 must not exceed 50 ms.

### 16.5 SOFT gates (Plan §14.4 — Research NR3)

- `search_text` p99: report only.
- Bootstrap on Large: report a measured budget that becomes the v1.0.0 expectation; not blocking.

---

## 17. Token-Saving ROI Harness

> Output: `docs/research/03-code-indexer-roi-claude-code.md`. Resolution Q10.1 + Q10.2.

### 17.1 Harness location

```
tests/roi/
  ├─ system-prompt.md          # versioned Claude-Code-like system prompt
  ├─ flows/
  │   ├─ flow_a_find_symbol.json
  │   ├─ flow_b_outline.json
  │   └─ flow_c_find_references.json
  ├─ harness.rs                # Rust binary; calls Anthropic API + this MCP server
  └─ fixtures/
      └─ unblock-v1.0.0/       # frozen checkout for reproducibility
```

### 17.2 Three flows

| Flow | Question | Gold answer |
|---|---|---|
| A | "Find the implementation of `DependencyGraph::ready_set`." | exact symbol (`file:line` + `symbol_id`) |
| B | "Give me the structure of `crates/unblock-core/src/graph.rs`." | outline node ids in order |
| C | "What calls `parse_github_url`?" | reference list |

### 17.3 Protocol

1. **Baseline run**: agent has only `Glob`, `Grep`, `Read`. Run flow until first correct answer (validated against gold). Record total input + output tokens.
2. **Indexer run**: agent has only the 9 indexer tools. Same.
3. **N = 10** runs per flow per mode, fresh sessions to control prompt-cache variance.
4. Pin Claude Sonnet model id and the system prompt; record both in the report.

### 17.4 Reported metrics

- Median + p95 token count per flow per mode.
- Token-saving ratio = `tokens_baseline / tokens_indexer` per run.
- Time-to-first-correct-answer (informational).

### 17.5 Gates (Resolution Q10.2)

```
HARD (blocks Epic 03.6 close):
  median(ratio across all 30 runs) ≥ 2.0×

SOFT (informational, reported):
  Flow A median ≥ 3.0×
  Flow B median ≥ 2.0×
  Flow C median ≥ 1.5×

ESCAPE PATH if HARD fails:
  1. Block Epic 03.6 close.
  2. Open `unblock:finding:risk` finding bead under the Epic 03.6 parent epic.
  3. Investigate (perf? harness bug? query patterns?) and remediate.
  4. Re-measure; only after HARD passes does Epic 03.6 close.
```

### 17.6 Future follow-up

Phase 04 reruns the harness with a Sherlock supervisor (does not exist in Phase 03) and emits `docs/research/04-code-indexer-roi-supervisor.md`. Informational, not gating.

---

## 18. Testing Strategy

### 18.1 Unit tests (`unblock-indexer-core`)

- `kind::map_capture_to_kind()` exhaustiveness per language.
- `manifest::parse()` round-trips.
- `traversal::extract_symbols()` determinism (proptest).
- `comment::attach_<lang>()` boundary conditions (no comment, multi-line, edge of file).
- `Span` ordering invariants.

### 18.2 Integration tests (`unblock-indexer`)

- **Schema migration**: open a fresh DB, assert FTS5 triggers fire (insert / update / delete a row, query `symbols_fts`).
- **PRAGMA assertion**: assert `Fts5Unavailable` is returned when FTS5 is absent (built-feature guard test gated behind a custom cfg).
- **Walker**: fixture tree with nested `.gitignore`, `force_include`, default-excludes, `same_file_system` boundary; assert exact entry set.
- **Watcher**: synthetic create/modify/delete/rename events; assert DB state converges within 1 s of debounce window.
- **Bootstrap on small fixture**: parses a 50-file mixed-language fixture without panic; symbol counts match a checked-in baseline.
- **Parent linkage**: assert hierarchical structure for a Rust file with nested `mod` + `impl` + `fn`.

### 18.3 MCP-level tests (`unblock-mcp`)

- One test per tool against a mixed-language fixture repo.
- `find_references` description lint (§12.4).
- `init` and `register` smoke tests using `--print-only` (no FS writes).
- Idempotent merge: write existing config, re-run register, assert no duplicate keys.

### 18.4 Property tests

- Symbol extraction determinism: `extract_symbols(t, s) == extract_symbols(t, s)` byte-for-byte.
- FTS5 round-trip: every inserted symbol's `name` is recoverable via prefix match.
- Walker idempotence: re-running the walker on a quiescent tree yields the same entry set.

### 18.5 Grammar pipeline tests

- A CI smoke test downloads the freshly published manifest + WASMs and runs `extract_symbols` against a per-language micro-fixture (`tests/grammar-smoke/<lang>/sample.<ext>`).

---

## 19. Invariants

These are the **non-negotiable** properties of the indexer subsystem. Implementation MUST preserve them; tests SHOULD assert them.

1. **No body text in the database.** Only span. Bodies are read from the FS at query time.
2. **GitHub is not the indexer's source of truth.** The indexer's source of truth is the local filesystem. Law 1 of the MANIFESTO is preserved.
3. **`unblock-indexer-core` has zero IO and zero async dependencies.** Enforced by Cargo.toml.
4. **Per-query mtime check is mandatory** for tools listed in §11.3. It is not an optimisation.
5. **Manifest integrity is anchored at compile time.** The manifest's own SHA-256 is a constant in the binary; runtime trust derives from it.
6. **WAL writer chunking.** No bootstrap or reindex transaction may exceed 2 048 inserts or 256 files before commit.
7. **FTS5 must be present.** First connect asserts `ENABLE_FTS5` via `PRAGMA compile_options;` or the indexer refuses to start.
8. **Stdout is reserved for MCP.** Logs (`tracing`) go to stderr exclusively. Wizard prompts go to stderr; CLI output (e.g. `register --print-only`) goes to stdout.
9. **`find_references` description always contains `HEURISTIC` and `syntactic only, no type resolution`.** Lint-enforced (§12.4).
10. **Opaque `symbol_id`.** Wire form is a string; clients MUST NOT parse it. Internal representation may change.
11. **`force_include` cannot bypass default-excludes.** Always — even with explicit user config.
12. **Walker uses `require_git(false)`.** Footgun fix per Research §R7.

---

## 20. Open Items & Forward References

These items are **not blocking** spec approval. They are tracked here so Fernando's bead breakdown captures them as work items.

### 20.1 RESOLVED — Phase 02 dependency surface

**Resolution (2026-04-28, Plan 02 APPROVED).** Phase 02 plan §6 pinned the contract: the resilience layer ships as a stand-alone crate `unblock-resilience` (extracted in Epic 02.A, no transitive dep on `unblock-github`). `unblock-indexer` depends directly on `unblock-resilience`. OpenTelemetry is **deferred to Phase 06** — Phase 03 instruments via the in-memory `ServerMetrics` introduced in Phase 02 (no external collector required).

**Pinned import surface for `unblock-indexer`** (per [02-plan-mcp-complete §6.3](../plans/02-plan-mcp-complete.md#63-pinned-api-for-phase-03-consumption)):

```rust
use unblock_resilience::{ResiliencePolicy, IsRetryable, BreakerSnapshot, RetrySnapshot, BreakerState};
```

**Action remaining for Epic 03.2 first bead:**

1. Verify the merged Phase 02 surface matches §6.3 of Plan 02 byte-for-byte.
2. Update §8.2 of this spec with the concrete `unblock_resilience::*` symbol names (replace any `unblock_github::resilience::*` placeholder text).
3. Implement `IsRetryable` on the grammar-fetch error type.

If Phase 02 has not merged when Epic 03.2 opens, Epic 03.1 (workspace setup) may proceed in parallel; Epic 03.2 blocks on the surface confirmation. The crate-extraction question is closed.

### 20.2 DEFERRED — wasmtime Engine cache (Research §R2 open question)

The wasmtime on-disk artefact cache (`Engine::config().cache_config_load_default()`) is **not** enabled in v1.0.0. If the bench suite (§16) shows cold-start `WasmStore::load_language` cost > 100 ms × number of languages dominates the cold path, Epic 03.6 reopens this decision and ships a v1.0.x patch.

### 20.3 DEFERRED — offline grammar bundle (Research §R1 risk)

Air-gapped environments cannot fetch grammars. v1.0.0 does **not** ship an offline bundle. Surfaced via `IndexerError::GrammarFetch` with a clear message pointing at a future work item. Tracked under `unblock:finding:risk` post-ship if user demand materialises.

### 20.4 DEFERRED — `comment` attachment heuristics edge cases

The per-language attachment heuristics in §10.5 are intentionally simple. Edge cases (Rust attribute macros between `///` and the symbol; Java annotations between Javadoc and the symbol) MAY produce a missing `comment`. Acceptable for v1.0.0; tracked as quality bead under Epic 03.4 if the integration tests reveal high miss rate (>5 % on the medium corpus).

### 20.5 FORWARD — Phase 04 Sherlock ROI rerun (§17.6)

Informational only; no Phase 03 work item.

---

## Sign-off checklist (Ada → User)

- [ ] All 7 Q-resolutions integrated (Q4, Q5, Q7, Q9.1, Q9.2, Q10.1, Q10.2).
- [ ] Contradiction C1 (`tree-sitter-loader` rejection) explicit in §2 and §8.
- [ ] R3 TypeScript-as-`tsx` decision explicit in §4.1, §7.1, §9.2.
- [ ] R4 FTS5 PRAGMA assertion + chunked transactions explicit (§5.1, §5.5, §11.1).
- [ ] R6 default debounce 500 ms (configurable) explicit (§11.2, §15.1).
- [ ] R7 `require_git(false)` + `force_include` semantics explicit (§9.1, §15.1, §19.11–12).
- [ ] All 16 SymbolKinds enumerated and the 4 extras' query strategy documented (§4.2, §10.1).
- [ ] HARD/SOFT gate split per NR3 (§16.4–§16.5, §17.5).
- [ ] §20 captures the one remaining UNRESOLVED item (Phase 02 surface).
- [ ] No standalone design doc; this spec + `docs/plans/03-plan-code-indexer.md` are the only artefacts.

*This spec is the single source of truth for Phase 03 implementation. Bead descriptions reference this document and the plan; they never duplicate authoritative content.*
