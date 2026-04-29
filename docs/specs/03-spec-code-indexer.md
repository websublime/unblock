# Spec 03 — Code Indexer CLI (`unblock-code` v1.0.0)

**Status:** APPROVED
**Author:** Ada (architect)
**Date:** 2026-04-29
**Crates (new):** `unblock-indexer-core`, `unblock-indexer`, `unblock-code`
**Crates (modified):** none (workspace `Cargo.toml` only)
**Source PRD:** [docs/PRD.md](../PRD.md) (§7 Phase 03)
**Source SPEC:** [docs/SPEC.md](../SPEC.md) (§6.5 Code Indexer CLI)
**Source Plan:** [docs/plans/03-plan-code-indexer.md](../plans/03-plan-code-indexer.md) (APPROVED, commit `42b1f62`)
**Source Research:** [docs/research/03-research-code-indexer.md](../research/03-research-code-indexer.md) (PROCEED verdict, commit `42b1f62`)
**Companion:** [MANIFESTO](../MANIFESTO.md) · [02-spec-mcp-complete](./02-spec-mcp-complete.md)

> **Amended 2026-04-29 post-review:** SO-2 added `walker.max_file_bytes` config knob (default 2 MiB); SO-3 comment markers stripped per-language family in `comment` column (§9.7 + invariant in §17); SO-4 dropped `meta.toml` — SQLite `meta` table is sole metadata source.

---

## Table of Contents

1.  [Scope & Conventions](#1-scope--conventions)
2.  [Research Alignment & Locked Resolutions](#2-research-alignment--locked-resolutions)
3.  [Crate Architecture](#3-crate-architecture)
4.  [Domain Types — `unblock-indexer-core`](#4-domain-types--unblock-indexer-core)
5.  [Storage Layer (sqlx + FTS5)](#5-storage-layer-sqlx--fts5)
6.  [Cache Layout](#6-cache-layout)
7.  [Grammar Loading & Cargo Features](#7-grammar-loading--cargo-features)
8.  [File Walker & Language Detection](#8-file-walker--language-detection)
9.  [AST Traversal & Symbol Extraction](#9-ast-traversal--symbol-extraction)
10. [Lifecycle: Bootstrap & Steady-State (NO watcher)](#10-lifecycle-bootstrap--steady-state-no-watcher)
11. [CLI Surface](#11-cli-surface)
12. [Error Model](#12-error-model)
13. [Configuration](#13-configuration)
14. [Performance Methodology & Gates](#14-performance-methodology--gates)
15. [ROI Harness](#15-roi-harness)
16. [Testing Strategy](#16-testing-strategy)
17. [Invariants](#17-invariants)
18. [Open Items & Forward References](#18-open-items--forward-references)

---

## 1. Scope & Conventions

### 1.1 What this spec authoritatively defines

This document is the **authoritative technical contract** for Phase 03. It pins:

- Exact public API of three new crates (`unblock-indexer-core`, `unblock-indexer`, `unblock-code`).
- Exact 17-variant `SymbolKind` enum, `Span`, `Symbol`, `Language`, `SymbolId`, and error types.
- DDL constants for the SQLite schema (`symbols`, `files`, `meta`, `symbols_fts` virtual table, three triggers).
- FTS5 trigger semantics (`AFTER INSERT` / `AFTER DELETE` / `AFTER UPDATE`) and the bootstrap `'rebuild'` optimisation.
- Walker configuration (`WalkBuilder` flags, default-excludes glob list, force-include precedence).
- Per-language capture-to-`SymbolKind` translation contract (table-driven in `src/tags/<lang>.rs`).
- `parent_id` post-traversal algorithm (deterministic O(n) per file).
- Bootstrap algorithm with chunked transactions (~500 rows / `BEGIN..COMMIT`) and `INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');`.
- Per-query mtime check semantics and the implicated-file caps per command.
- Per-command JSON envelope schemas (locked from plan §7.2).
- Exit-code mapping (L17) and complete `IndexerError` variant inventory.
- Performance gates split between HARD (Linux primary) and SOFT (multi-platform informational + size + ROI).
- Acceptance test plan and ROI-harness protocol.

Where the plan committed to a decision, this spec **expands it into a contract**.
Where research validated or contradicted an assumption, this spec **codifies the
research-validated reality** — never the original assumption (per `feedback_pre_production`
+ research-first-design rule from CLAUDE.md).

### 1.2 What is NOT in scope

Mirror of plan §3, restated here for spec-time discipline:

- Cross-file semantic resolution, type inference, real call graph. `find-references` is **HEURISTIC** syntactic-only.
- Analytics: dead-code, cyclomatic complexity, similarity, redundancy / 102-style checks.
- WASM grammar runtime, runtime fetcher, integrity manifest.
- Daemon mode and file watcher.
- Editor MCP registration. The CLI does not register with editors.
- Network grammar fetcher. No HTTP at runtime. `unblock-indexer` does **not** depend on `unblock-resilience`.
- Issue/code correlation queries between `unblock-mcp` and `unblock-code`.
- C# / Swift / Dart language support (deferred to v1.0.x).
- MCP tools, NDJSON streaming output, multi-envelope responses.

### 1.3 Conventions used in this spec

- **MUST / MUST NOT / SHOULD / MAY** follow RFC 2119.
- Code blocks tagged ```rust``` are **normative signatures** unless explicitly marked `// illustrative`.
- DDL fragments tagged ```sql``` are **wire-format normative**. Variable names in DDL constants are stable identifiers and MUST NOT change between v1.0.0 and v1.0.x.
- Schema fragments tagged ```json``` are **wire-format normative** for the CLI envelope contract.
- File paths are absolute or workspace-relative (`crates/...`); module-internal paths are crate-relative.
- Algorithms are presented as numbered plain-English steps; types remain in Rust syntax.
- Cross-references use `Plan §N`, `Research §RN`, `Resolution Q-N` notation.
- Pre-production stance applies (per `feedback_pre_production`): no migrations, no backward-compat shims, breaking changes acceptable across all unblock crates.

### 1.4 Decision provenance

Every locked decision in this spec carries provenance to:

- `Plan §4 / L1..L22` — locked architectural decisions.
- `Plan §7.1 / D1..D4` — locked envelope conventions.
- `Research §RN` — research-validated facts (R3, R4, R5, R7, R8, R-CLI-1..5).
- `Q-S-X.Y` — spec-time questions resolved in §2.
- **SPEC-ORIGINAL** — a decision introduced by this spec (no plan/research provenance) and surfaced for user review.

---

## 2. Research Alignment & Locked Resolutions

This section records how research findings, locked plan decisions, and spec-time
open questions translate into spec-level constraints.

### 2.1 Research-validated bindings

| Plan / research item                          | Status                       | Spec-level resolution                                                                                            |
| --------------------------------------------- | ---------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| **R3 — Top-10 Grammar Audit**                 | CONFIRMED                    | §7.2 pins all 12 dependency versions verbatim with `=x.y.z`. ABI 14/15 split tolerated by tree-sitter 0.26.8.    |
| **R3.1 — Stale grammars (typescript/cpp/ruby/java)** | CONFIRMED            | §9.2 mandates vendoring `tags.scm` from each grammar's **crates.io tagged commit**, never `master`.              |
| **R4 — sqlx + FTS5 PRAGMA + triggers**        | CONFIRMED                    | §5.2 codifies pool `after_connect`, FTS5 PRAGMA assertion (exit 6), `'delete'`-then-insert triggers.             |
| **R4 — Bootstrap optimisation**               | CONFIRMED                    | §5.4 + §10.1 codify drop-triggers → bulk-insert → `'rebuild'` → re-create-triggers, with ~500-row chunked tx.    |
| **R5 — Symbol-extraction queries**            | CONFIRMED with scope correction | §9 codifies per-language `tags.scm + ext_<lang>.scm` concatenation, ~30–40 query rules across 10 languages (Q-R5.1). |
| **R5 — `parent_id` post-traversal**           | CONFIRMED                    | §9.5 codifies the O(n) linear-scan algorithm (sort + ancestor stack).                                            |
| **R7 — `ignore` crate edge cases**            | CONFIRMED                    | §8.1 codifies `WalkBuilder::require_git(false)`, `same_file_system(true)`, `Override` precedence (excludes-first). |
| **R8 — Latency methodology**                  | CONFIRMED                    | §14 codifies criterion harness, three corpus tiers, implicated-file caps per command.                            |
| **R-CLI-1 — Cold-start budget**               | MODELLED                     | §14.4 sets L21: cold-start p95 < 100 ms full-load on Linux x86_64 warm-DB Medium corpus (HARD); macOS+Windows informational (Q-S-2). |
| **R-CLI-2 — Binary size outliers (cpp+ruby)** | CONFIRMED                    | §7.3 codifies `lang-cpp` + `lang-ruby` as opt-in, NOT default. S1 SOFT ceiling: ≤ 30 MB stripped Linux x86_64 default-feature build. |
| **R-CLI-3 — Cargo feature ergonomics**        | CONFIRMED                    | §7.3 codifies the `dep:`-based feature scheme; resolver = "2" already set workspace-wide.                        |
| **R-CLI-4 — `cc` toolchain required**         | CONTRADICTED → REWORDED       | H2 (§14.5) reflects "C toolchain required"; README ships a "Building from source" section. Plan was amended at 42b1f62. |
| **R-CLI-5 — ROI methodology**                 | CONFIRMED                    | §15 codifies 3 flows × N=10 × 2 arms = 60 runs; aspirationals A ≥ 3.5×, B ≥ 2.5×, C ≥ 1.8×, global median ≥ 2.5×; SOFT 2.0× threshold. Release-gate one-shot, not per-PR CI. |

### 2.2 Plan locked decisions (provenance map)

| Plan ID | Subject                                            | Spec section |
| ------- | -------------------------------------------------- | ------------ |
| L1      | One-shot CLI; no MCP / no daemon / no watcher      | §10          |
| L2 / L3 | 3 new crates, 7 workspace crates post-Phase 03     | §3           |
| L4      | Static-linked grammars; 8 default + 2 opt-in       | §7.1, §7.3   |
| L5      | Fresh implementation; MIT; no third-party attribution | §3.4         |
| L6      | `build.rs` compiles grammars                       | §7.4         |
| L7      | sqlx + SQLite + FTS5 + WAL; 17 SymbolKinds + Span + parent_id + comment | §4, §5 |
| L8      | Cache at `~/.cache/unblock/repos/<repo-hash>/index.db`; span-only | §6 |
| L9      | Per-query mtime check; sole sync mechanism         | §10.2        |
| L10     | Top-10 with cpp+ruby opt-in; C#/Swift/Dart deferred | §7.1        |
| L11     | 11 commands enumerated                             | §11          |
| L12–L14 | Out-of-scope items                                 | §1.2         |
| L15     | snafu errors; `Result<T>` per crate; `#[non_exhaustive]` | §4, §12 |
| L16     | tracing JSON Lines on STDERR; STDOUT reserved for envelope | §11.1, §17 |
| L17     | Exit-code families (`0/2/3/4/5/6/7/99`)            | §12.2        |
| L18     | `#![deny(unsafe_code)]` workspace-wide             | §3.3         |
| L19     | MIT, no third-party attribution                    | §3.4         |
| L20     | Warm-path p99 budgets (find-symbol < 10 ms; outline < 20 ms; list-symbols < 50 ms; search < 30 ms; find-references no budget) | §14.3 |
| L21     | Cold-start p95 < 100 ms full-load on Linux        | §14.4        |
| L22     | ROI SOFT gate; release-gate one-shot              | §15          |

| Plan ID | Subject                                            | Spec section |
| ------- | -------------------------------------------------- | ------------ |
| D1      | One JSON envelope per invocation on stdout         | §11.1        |
| D2      | Errors as JSON envelope on stdout + non-zero exit  | §11.1, §12.2 |
| D3      | Minified default; `--pretty` flag                  | §11.1        |
| D4      | Opaque `symbol_id` (string)                        | §4.6, §11.1  |

### 2.3 Spec-time open questions — resolutions

These questions are introduced by this spec and **resolved here** for the user's
review. Each is tagged `Q-S-N` and appears in the relevant section as anchor.

| ID       | Question                                                    | Resolution                                                                                                                                                                         |
| -------- | ----------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Q-S-1**| `language` string values in JSON envelope                   | Lowercase ASCII matching the Cargo feature suffix exactly: `rust`, `typescript`, `javascript`, `python`, `go`, `java`, `c`, `cpp`, `ruby`, `php`. NOT `c++`, NOT `rb`. (§11.1)     |
| **Q-S-2**| Multi-platform cold-start scope for L21 / H6                | Linux x86_64 is the **primary HARD gate** (`p95 < 100 ms` warm-DB Medium). macOS aarch64 + Windows x86_64 measured & reported as **informational**; Windows allowance +50% per R-CLI-1.2. (§14.4) |
| **Q-S-3**| `reindex(path?)` semantics                                  | `path = None` → full re-bootstrap (drop triggers → bulk insert → `'rebuild'` → re-create triggers). `path = Some(p)` → delete affected rows + re-parse subtree + per-row trigger sync. (§10.4) |
| **Q-S-4**| `get-symbol --body` field bounds                            | Read filesystem at query time using span; cap at **64 KiB** (truncated with trailing `\n…\n` if exceeded); reject reads outside repo root (path-traversal guard). (§11.5)        |
| **Q-S-5**| `search` FTS5 query sanitisation                            | Trim, strip leading single/double quotes, escape inner `"` as `""`, wrap in `"..."` for phrase quoting; reject `prefix:` / `column:` syntax with `InvalidFtsQuery`. (§11.6)       |
| **Q-S-6**| `find-references` schema lint                               | Workspace lint via `unblock-code/build.rs` asserts the literal substrings `HEURISTIC` and `syntactic only, no type resolution` appear in `--help` output AND in JSON `warning`. Failure aborts compilation. (§11.7) |
| **Q-S-7**| Opaque `symbol_id` wire format                              | String serialisation of SQLite rowid (`i64` → decimal string). Internal `pub struct SymbolId(pub i64)` with `Display` + `FromStr`. Clients MUST NOT parse. (§4.6)                |

**Spec invariant:** any text below that contradicts §2.1, §2.2, or §2.3 is a defect.

---

## 3. Crate Architecture

Phase 03 introduces three new crates and modifies only the workspace `Cargo.toml`
(adds the three new members). After Phase 03 the workspace contains seven crates:
`unblock-core`, `unblock-github`, `unblock-resilience`, `unblock-mcp`,
`unblock-indexer-core`, `unblock-indexer`, `unblock-code`.

### 3.1 Dependency graph

```
unblock-indexer-core ──── (pure; zero IO; zero async; depends only on workspace types + serde + snafu)
                          │
                          ▼
                    unblock-indexer ──── tokio · sqlx · ignore · rayon · tree-sitter · 8–10 grammars
                          │
                          ▼
                     unblock-code (bin) ── clap · serde_json · tracing-subscriber
```

`unblock-indexer-core` MUST NOT depend on `tokio`, `sqlx`, `ignore`, `rayon`, or
any `tree-sitter-<lang>` crate. It MAY depend on the bare `tree-sitter` crate to
re-export `Tree` and `Node` types for the AST visitor contract.

`unblock-indexer` is the only crate that touches the filesystem, sqlite, or
grammar registries. `unblock-code` is the only crate that touches stdout/stderr,
clap, or the process-exit code surface. (Plan §5; L1, L2, L3.)

### 3.2 Module layout (authoritative)

```
crates/unblock-indexer-core/
├── Cargo.toml              # license = "MIT"
└── src/
    ├── lib.rs              # crate-scoped Result, re-exports
    ├── errors.rs           # CoreError (snafu, #[non_exhaustive])
    ├── kinds.rs            # SymbolKind (17 variants, #[non_exhaustive])
    ├── language.rs         # Language enum (10 variants, #[non_exhaustive], cfg-gated registry)
    ├── span.rs             # Span (1-based byte offsets + line/col)
    ├── symbol.rs           # Symbol DTO + SymbolId newtype
    ├── ast/
    │   ├── mod.rs          # AST visitor contract (pure)
    │   └── visitor.rs      # Traversal driver: tree_sitter::Tree → Vec<Symbol>
    ├── tags.rs             # CaptureName, AnchorKind, kind_for_capture trait
    ├── schema.rs           # DDL constants: SCHEMA_VERSION, CREATE_*, TRIGGERS_*
    └── parent.rs           # parent_id post-traversal algorithm

crates/unblock-indexer/
├── Cargo.toml              # license = "MIT"
├── build.rs                # workspace-lint harness (no grammar compilation here — handled by upstream crates)
└── src/
    ├── lib.rs
    ├── errors.rs           # IndexerError (snafu, #[non_exhaustive])
    ├── grammars/
    │   ├── mod.rs          # registry::loaders() -> HashMap<Language, fn() -> tree_sitter::Language>
    │   ├── rust.rs         #[cfg(feature = "lang-rust")]
    │   ├── typescript.rs   #[cfg(feature = "lang-typescript")]
    │   ├── javascript.rs   #[cfg(feature = "lang-javascript")]
    │   ├── python.rs       #[cfg(feature = "lang-python")]
    │   ├── go.rs           #[cfg(feature = "lang-go")]
    │   ├── java.rs         #[cfg(feature = "lang-java")]
    │   ├── c.rs            #[cfg(feature = "lang-c")]
    │   ├── cpp.rs          #[cfg(feature = "lang-cpp")]    -- opt-in
    │   ├── ruby.rs         #[cfg(feature = "lang-ruby")]   -- opt-in
    │   └── php.rs          #[cfg(feature = "lang-php")]
    ├── tags/               # vendored tags.scm + extensions
    │   ├── rust.scm        # vendored from tagged commit
    │   ├── rust.rs         # capture→kind translation table
    │   ├── ext_rust.scm    # hand-written extensions (imports, fields, constants…)
    │   ├── typescript.scm
    │   ├── typescript.rs
    │   ├── ext_typescript.scm
    │   └── …               # (one tags + tags.rs + ext_<lang>.scm trio per active language)
    ├── walker.rs           # WalkBuilder wrapper, mtime probe, force_include
    ├── parse.rs            # parse driver: file → tree_sitter::Tree → Vec<Symbol>
    ├── store.rs            # sqlx pool, schema migrations, FTS5 PRAGMA assertion
    ├── bootstrap.rs        # rayon-driven full reindex, chunked transactions
    ├── reindex.rs          # subtree reindex (Q-S-3)
    ├── mtime.rs            # per-query mtime check (sole sync mechanism)
    ├── repo.rs             # repo_root discovery + repo_hash
    ├── config.rs           # .unblock/indexer.toml + .unblock/languages.toml loader
    └── query.rs            # read-side queries (find-symbol, list-symbols, outline, get-symbol, search, find-references)

crates/unblock-code/
├── Cargo.toml              # license = "MIT"; bin = unblock-code; default-features = 8 langs
├── build.rs                # workspace lint: assert HEURISTIC strings present in find-references --help + envelope warning (Q-S-6)
└── src/
    ├── main.rs             # tokio::main; clap parse; dispatch
    ├── errors.rs           # CliError + exit-code mapping (L17)
    ├── cli.rs              # clap derive definitions for 11 subcommands
    ├── envelope.rs         # JSON envelope serde structs (per §11.x)
    ├── tracing_init.rs     # tracing-subscriber JSON Lines on stderr
    └── commands/
        ├── mod.rs
        ├── find_symbol.rs
        ├── list_symbols.rs
        ├── outline.rs
        ├── get_symbol.rs
        ├── search.rs
        ├── find_references.rs
        ├── reindex.rs
        ├── status.rs
        ├── languages.rs
        ├── init.rs
        └── parse.rs
```

### 3.3 Workspace conventions

All three new crates inherit:

- `edition = "2024"`.
- `[lints]` from workspace: `unsafe_code = "deny"` (L18), `clippy::pedantic = "warn"`, `missing_docs = "warn"`.
- `snafu` for errors (L15) — no `unwrap()` / `expect()` outside `#[cfg(test)]` modules.
- `///` doc comments on every `pub fn` and `pub struct`; `//!` module-level docs on every module (CLAUDE.md "Coding Standards").
- `#[non_exhaustive]` on every growable public enum (CLAUDE.md "Coding Standards"; Plan §10): `SymbolKind`, `Language`, `CoreError`, `IndexerError`, `CliError`.
- Crate-scoped `pub type Result<T, E = X> = core::result::Result<T, E>;` aliases.

### 3.4 Licensing

All three crates carry `license = "MIT"` (L19). No `NOTICE`, no `THIRD_PARTY` file.
The vendored `tags.scm` files originate from upstream tree-sitter grammars whose
licenses are MIT/Apache-2.0; their license headers MUST be preserved verbatim
inside each `.scm` file (top-of-file comment), but no aggregated attribution file
is added to the repo.

---

## 4. Domain Types — `unblock-indexer-core`

`unblock-indexer-core` is **pure**: no IO, no async, no tokio, no sqlx, no ignore,
no `tree-sitter-<lang>` grammar deps. It MAY depend on the bare `tree-sitter`
crate (for `Tree` / `Node` re-exports) and on workspace crates `serde`, `snafu`,
`chrono`.

### 4.1 `Language` enum

Defined in `crates/unblock-indexer-core/src/language.rs`.

```rust
/// A source language supported by `unblock-code`.
///
/// All ten variants are unconditionally present so that consumers compiling with
/// any feature subset can match exhaustively (with `#[non_exhaustive]` discipline).
/// The grammar **loader** (`unblock-indexer::grammars::registry::loaders`) is the
/// only surface that gates by Cargo feature.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    C,
    Cpp,
    Ruby,
    Php,
}

impl Language {
    /// Lowercase ASCII identifier matching the Cargo feature suffix exactly
    /// (Q-S-1). Stable wire format on the JSON envelope.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Go => "go",
            Self::Java => "java",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Ruby => "ruby",
            Self::Php => "php",
        }
    }

    /// Parse from the wire-format string. Unknown strings → `None`.
    pub fn from_wire(s: &str) -> Option<Self> { /* …match-all 10… */ }

    /// Iterate over every variant — useful for `languages` command + tests.
    pub fn all() -> &'static [Self] { /* slice of 10 */ }
}
```

**Wire format invariant (Q-S-1):** the JSON `language` field on every envelope is
the lowercase ASCII string from `Language::as_str`. NEVER `"c++"`, NEVER `"rb"`.

### 4.2 `SymbolKind` enum (17 variants, locked)

Defined in `crates/unblock-indexer-core/src/kinds.rs`. Matches PRD §7 verbatim
(L7). Locked at plan APPROVED time.

```rust
/// One of the seventeen canonical symbol kinds emitted by the indexer.
///
/// The mapping from a per-language tree-sitter capture to a `SymbolKind` is
/// per-language and table-driven (see `unblock-indexer::tags::<lang>`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

impl SymbolKind {
    pub fn as_str(self) -> &'static str { /* … snake_case strings … */ }
    pub fn from_wire(s: &str) -> Option<Self> { /* … */ }
    pub fn all() -> &'static [Self] { /* slice of 17 */ }
}
```

**Storage representation:** `SymbolKind` is persisted as the `as_str` value in
SQLite (`TEXT NOT NULL`). The schema migration logic does NOT translate variants;
on a `from_wire` failure during a query result decode, `IndexerError::SchemaDecode`
is raised (exit 5).

### 4.3 `Span`

Defined in `crates/unblock-indexer-core/src/span.rs`. Stored as four `INTEGER`
columns + two byte-offset `INTEGER` columns (six total) for ordering and
parent-id computation.

```rust
/// 1-based line/column span with absolute byte offsets.
///
/// Line/column are derived from the byte offsets at extraction time. Byte
/// offsets are the canonical sort key for `parent_id` resolution (§9.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub start_offset: u32,
    pub end_offset: u32,
}

impl Span {
    pub fn from_node(node: tree_sitter::Node<'_>, src: &[u8]) -> Self { /* … */ }
    pub fn len(&self) -> u32 { self.end_offset - self.start_offset }
    pub fn contains(&self, other: &Span) -> bool {
        self.start_offset <= other.start_offset && other.end_offset <= self.end_offset && self != other
    }
}
```

**Invariant:** `start_line >= 1`, `start_col >= 1`, `end_line >= start_line`,
`end_offset >= start_offset`. Wire format on JSON envelopes uses the four
line/col fields **only** (see §11.1); byte offsets are SQLite-internal.

### 4.4 `Symbol` DTO

Defined in `crates/unblock-indexer-core/src/symbol.rs`.

```rust
/// A single symbol record produced by the AST visitor.
///
/// `id` and `parent_id` are populated only after the symbol has been persisted
/// (post-traversal `parent_id` assignment per §9.5); fresh-from-AST symbols
/// have `id = None` and `parent_id = None`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub id: Option<SymbolId>,
    pub name: String,
    pub kind: SymbolKind,
    pub language: Language,
    pub file: String,                // workspace-relative POSIX path; never absolute
    pub span: Span,
    pub signature: Option<String>,   // single-line signature; None if grammar provides none
    pub comment: Option<String>,     // attached doc-comment; None if not present
    pub parent_id: Option<SymbolId>,
}
```

**Path conventions:** `file` is always **forward-slash** POSIX form, relative to
the repo root, even on Windows. The walker normalises during emission.

### 4.5 `FileRecord`

Defined in `crates/unblock-indexer-core/src/symbol.rs`. Used by `mtime.rs` and the
`status` command.

```rust
/// One row in the `files` table — tracks indexed-at mtime per file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileRecord {
    pub file: String,                // POSIX, repo-relative
    pub language: Language,
    pub size_bytes: u64,
    pub mtime_unix_ms: i64,          // last filesystem mtime at index time
    pub indexed_at_unix_ms: i64,     // when this row was written
    pub symbol_count: u32,
}
```

### 4.6 `SymbolId` newtype (Q-S-7)

Defined in `crates/unblock-indexer-core/src/symbol.rs`.

```rust
/// Opaque, monotonic identifier assigned by SQLite (`rowid`).
///
/// Wire format: decimal string. Clients MUST NOT parse the value or assume
/// numeric ordering; SQLite may re-use rowids after deletion (in practice
/// `INTEGER PRIMARY KEY AUTOINCREMENT` prevents reuse — we use AUTOINCREMENT
/// to give clients a stable monotonic guarantee).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub i64);

impl core::fmt::Display for SymbolId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl core::str::FromStr for SymbolId {
    type Err = core::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> { Ok(Self(s.parse()?)) }
}

impl serde::Serialize for SymbolId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for SymbolId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s: String = serde::Deserialize::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
```

**SPEC-ORIGINAL invariant:** The `--help` text for `find-symbol`, `get-symbol`,
`list-symbols`, `outline`, and `search` MUST contain the literal string
`Note: symbol_id is opaque; do not parse.` The `unblock-code/build.rs` lint
(Q-S-6 sibling) verifies this string is present in the rendered `--help`
output.

---

## 5. Storage Layer (sqlx + FTS5)

### 5.1 Engine & dependencies

| Component       | Crate / version           | Provenance |
| --------------- | ------------------------- | ---------- |
| SQL client      | `sqlx = "0.8.6"`          | R4         |
| Bundled SQLite  | `libsqlite3-sys = "0.37"` (transitive via `sqlx`'s `sqlite` feature) | R4 |
| Runtime         | `tokio = "1"` (workspace) | existing   |
| Features        | `["sqlite", "runtime-tokio-rustls"]` | R4 |

**Pinned versions:** every dep listed in research §"Dependencies investigated"
is pinned in `crates/unblock-indexer/Cargo.toml` with `=x.y.z` (no semver
carets). This applies to **both grammars and storage deps**.

### 5.2 Connection pool & FTS5 PRAGMA invariant

```rust
// crates/unblock-indexer/src/store.rs
pub async fn open_pool(db_path: &Path) -> Result<sqlx::SqlitePool, IndexerError> {
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .pragma("temp_store", "MEMORY")
        .pragma("wal_autocheckpoint", "1000");

    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(/* see §5.7 */ 4)
        .after_connect(|conn, _meta| Box::pin(async move {
            // FTS5 PRAGMA assertion (R4, L17 exit 6)
            let mut has_fts5 = false;
            let mut rows = sqlx::query_scalar::<_, String>("PRAGMA compile_options")
                .fetch(&mut *conn);
            while let Some(opt) = rows.try_next().await? {
                if opt.eq_ignore_ascii_case("ENABLE_FTS5") { has_fts5 = true; break; }
            }
            if !has_fts5 {
                return Err(sqlx::Error::Configuration(
                    "SQLite was built without ENABLE_FTS5 — exit 6"
                    .into()
                ));
            }
            Ok(())
        }))
        .connect_with(opts)
        .await
        .map_err(IndexerError::open)?
}
```

**Invariant:** PRAGMA assertion is **HARD failure** (exit 6 / `IndexerError::Fts5Missing`).
The exact wire-message is the literal `SQLite was built without ENABLE_FTS5 — exit 6`.
Documented in `--help`.

### 5.3 Schema DDL constants

DDL is owned by `unblock-indexer-core::schema` so that test fixtures and the
storage crate share identical strings.

```rust
// crates/unblock-indexer-core/src/schema.rs

pub const SCHEMA_VERSION: u32 = 1;

pub const CREATE_META: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;
"#;

pub const CREATE_FILES: &str = r#"
CREATE TABLE IF NOT EXISTS files (
    file              TEXT    PRIMARY KEY NOT NULL,
    language          TEXT    NOT NULL,
    size_bytes        INTEGER NOT NULL,
    mtime_unix_ms     INTEGER NOT NULL,
    indexed_at_unix_ms INTEGER NOT NULL,
    symbol_count      INTEGER NOT NULL
) STRICT;
"#;

pub const CREATE_SYMBOLS: &str = r#"
CREATE TABLE IF NOT EXISTS symbols (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT    NOT NULL,
    kind          TEXT    NOT NULL,
    language      TEXT    NOT NULL,
    file          TEXT    NOT NULL,
    start_line    INTEGER NOT NULL,
    start_col     INTEGER NOT NULL,
    end_line      INTEGER NOT NULL,
    end_col       INTEGER NOT NULL,
    start_offset  INTEGER NOT NULL,
    end_offset    INTEGER NOT NULL,
    signature     TEXT,
    comment       TEXT,
    parent_id     INTEGER,
    FOREIGN KEY (file)      REFERENCES files(file) ON DELETE CASCADE,
    FOREIGN KEY (parent_id) REFERENCES symbols(id) ON DELETE SET NULL
) STRICT;
"#;

pub const CREATE_INDEX_SYMBOLS_NAME: &str =
    "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);";
pub const CREATE_INDEX_SYMBOLS_FILE: &str =
    "CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file);";
pub const CREATE_INDEX_SYMBOLS_PARENT: &str =
    "CREATE INDEX IF NOT EXISTS idx_symbols_parent ON symbols(parent_id);";
pub const CREATE_INDEX_SYMBOLS_KIND: &str =
    "CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);";

pub const CREATE_FTS: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
    name, signature, comment,
    content='symbols', content_rowid='id'
);
"#;

pub const CREATE_TRIG_AI: &str = r#"
CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
    INSERT INTO symbols_fts(rowid, name, signature, comment)
        VALUES (new.id, new.name, new.signature, new.comment);
END;
"#;

pub const CREATE_TRIG_AD: &str = r#"
CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, comment)
        VALUES ('delete', old.id, old.name, old.signature, old.comment);
END;
"#;

pub const CREATE_TRIG_AU: &str = r#"
CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, comment)
        VALUES ('delete', old.id, old.name, old.signature, old.comment);
    INSERT INTO symbols_fts(rowid, name, signature, comment)
        VALUES (new.id, new.name, new.signature, new.comment);
END;
"#;

pub const DROP_TRIGGERS: &[&str] = &[
    "DROP TRIGGER IF EXISTS symbols_ai;",
    "DROP TRIGGER IF EXISTS symbols_ad;",
    "DROP TRIGGER IF EXISTS symbols_au;",
];

pub const FTS_REBUILD: &str =
    "INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');";
```

**Notes on trigger semantics (R4):**
- The `'delete'` command requires the **exact prior column values**. Hence the
  `AFTER UPDATE` trigger uses `old.*` for the synthetic delete row and `new.*`
  for the insert row.
- During bootstrap, triggers are dropped; bulk inserts complete; `FTS_REBUILD`
  is run **once**; triggers are re-created. This is O(N) instead of O(N×K) for
  K trigger fan-out (R4.2).

### 5.4 FTS5 rebuild semantics

| Path                 | Sequence                                                                                                           |
| -------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Cold bootstrap       | `BEGIN` → `DROP_TRIGGERS` → chunked bulk inserts (~500 rows / `BEGIN..COMMIT`) → `FTS_REBUILD` → re-create triggers |
| Subtree reindex      | Use existing triggers (`'delete'`-then-insert path); no bulk rebuild                                               |
| Forced full reindex  | Same as cold bootstrap (drops triggers, batches, rebuilds)                                                         |
| Per-file mtime sweep | Use existing triggers (one file at a time)                                                                         |

### 5.5 Schema migration (no migrations — pre-prod stance)

Per Plan §6, `feedback_pre_production`, and L7: **no SQL migrations**. On schema
mismatch the indexer **wipes** the database file and rebuilds.

```rust
// crates/unblock-indexer/src/store.rs
pub async fn ensure_schema(pool: &sqlx::SqlitePool, db_path: &Path)
    -> Result<MigrationOutcome, IndexerError>
{
    // 1. Read meta.schema_version (default 0 if table missing).
    // 2. If meta.schema_version == SCHEMA_VERSION → no-op.
    // 3. Else: close pool, delete db_path + db_path.with_extension("db-wal") + db_path.with_extension("db-shm").
    // 4. Re-open pool, execute all CREATE_* DDL constants in declaration order.
    // 5. INSERT INTO meta (key,value) VALUES ('schema_version', SCHEMA_VERSION).
    // 6. Emit tracing::warn!(target = "indexer.schema",
    //        "schema_mismatch_wipe", old = old_v, new = SCHEMA_VERSION).
}
```

`MigrationOutcome` is `enum { Fresh, Wiped { old: u32 }, NoOp }` and is reported
on `status` (§11.10) so users see the wipe trail.

### 5.6 WAL contention rule

The bootstrap holds the WAL writer lock during each `BEGIN..COMMIT`. To avoid
starving concurrent readers (e.g. when the user runs `unblock-code find-symbol`
while a long-running `init` is in progress), bootstrap chunks at **~500 rows
per transaction**. This is the proven sweet-spot in the rusqlite/sqlx ecosystem
(R4) and balances throughput vs reader-fairness.

**Implementation:** `sqlx::QueryBuilder::push_values(...).push_chunked(...)`
batched across files; commit between chunks; never hold a transaction across
file boundaries during bootstrap.

### 5.7 Pool sizing

| Mode                 | `max_connections`              | Rationale                                                        |
| -------------------- | ------------------------------ | ---------------------------------------------------------------- |
| Read-side query path | 1                              | One-shot CLI; single command per process; no concurrency.        |
| Bootstrap / reindex  | 4                              | Rayon parses files in parallel; writes funnel through one writer + 3 readers for status. |
| Tests                | 1                              | Determinism.                                                     |

The CLI selects pool size at startup based on the dispatched command.

---

## 6. Cache Layout

### 6.1 XDG resolution

```rust
// crates/unblock-indexer/src/repo.rs
pub fn cache_root() -> Result<PathBuf, IndexerError> {
    let xdg = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            // Linux/macOS fallback: $HOME/.cache
            // Windows: dirs::cache_dir() or %LOCALAPPDATA%
            ...
        });
    Ok(xdg.join("unblock"))
}

pub fn repo_cache_dir(repo_root: &Path) -> Result<PathBuf, IndexerError> {
    let hash = repo_hash(repo_root);
    Ok(cache_root()?.join("repos").join(hash))
}

pub fn repo_hash(repo_root: &Path) -> String {
    let canon = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let mut h = sha2::Sha256::new();
    h.update(canon.as_os_str().as_encoded_bytes());
    hex::encode(h.finalize())[..32].to_string()  // first 32 hex chars
}
```

### 6.2 Cache directory layout

```
$XDG_CACHE_HOME/unblock/
└── repos/
    └── <repo-hash>/
        ├── index.db            # SQLite WAL primary
        ├── index.db-wal        # WAL log
        └── index.db-shm        # WAL shared-memory
```

**No auxiliary metadata files.** The SQLite `meta` table inside `index.db` is
the **sole** source of cache metadata (Invariant §17). There is no `meta.toml`,
no JSON sidecar, no lockfile.

### 6.3 SQLite `meta` table contents

The `meta` table (DDL in §5.3) stores all cache-level metadata as
`(key TEXT PRIMARY KEY, value TEXT)` rows. The keys written by `init` and
maintained by `reindex` / lifecycle code are:

| Key                 | Value semantics                                                  |
| ------------------- | ---------------------------------------------------------------- |
| `schema_version`    | Decimal string of `SCHEMA_VERSION` (currently `"1"`).            |
| `indexer_version`   | Cargo package version of `unblock-indexer` at write time (e.g. `"1.0.0"`). |
| `repo_root`         | Absolute, canonicalised repo-root path at index time.            |
| `created_at`        | RFC 3339 / ISO 8601 UTC timestamp of first `init`.               |
| `last_full_index`   | RFC 3339 / ISO 8601 UTC timestamp of the most recent full bootstrap. |
| `last_incremental`  | RFC 3339 / ISO 8601 UTC timestamp of the most recent subtree reindex or per-query mtime sync that wrote rows. |
| `last_wipe_old`     | (Optional) Old `schema_version` before the last schema-mismatch wipe; absent if no wipe has occurred. |

All `meta` writes go through a single helper
`unblock-indexer::store::meta::write(pool, key, value)` that issues
`INSERT INTO meta(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value;`.

### 6.4 Repo-root discovery

```rust
pub fn discover_repo_root(cwd: &Path) -> Result<PathBuf, IndexerError> {
    // 1. Walk upward from cwd looking for `.git` (file or directory).
    // 2. If none found: walk upward looking for `.unblock/`.
    // 3. If still none: error IndexerError::RepoRootNotFound (exit 2).
}
```

The CLI's `--repo-root <path>` flag overrides discovery (used in CI / tests).
Path-traversal guard: every cache write resolves under `cache_root()` and
rejects symlinks that escape.

---

## 7. Grammar Loading & Cargo Features

### 7.1 Default features

Per L4 / L10 / R-CLI-2:

- **8 grammars default-enabled**: `lang-rust`, `lang-typescript`, `lang-javascript`, `lang-python`, `lang-go`, `lang-java`, `lang-c`, `lang-php`.
- **2 grammars opt-in**: `lang-cpp`, `lang-ruby`.

### 7.2 Pinned versions (R3, verbatim)

```toml
# crates/unblock-indexer/Cargo.toml — [dependencies]
tree-sitter            = "=0.26.8"
tree-sitter-language   = "=0.1.7"
tree-sitter-rust       = { version = "=0.24.2", optional = true }
tree-sitter-typescript = { version = "=0.23.2", optional = true }
tree-sitter-javascript = { version = "=0.25.0", optional = true }
tree-sitter-python     = { version = "=0.25.0", optional = true }
tree-sitter-go         = { version = "=0.25.0", optional = true }
tree-sitter-java       = { version = "=0.23.5", optional = true }
tree-sitter-c          = { version = "=0.24.2", optional = true }
tree-sitter-cpp        = { version = "=0.23.4", optional = true }   # opt-in
tree-sitter-ruby       = { version = "=0.23.1", optional = true }   # opt-in
tree-sitter-php        = { version = "=0.24.2", optional = true }
sqlx                   = { version = "=0.8.6", features = ["sqlite", "runtime-tokio-rustls"] }
ignore                 = "=0.4.25"
rayon                  = "1"
```

ABI 14/15 split is tolerated by `tree-sitter` 0.26.8 (`MIN_COMPATIBLE_LANGUAGE_VERSION = 13`,
`LANGUAGE_VERSION = 15`). The runtime ABI guard fires per language at registry
load.

### 7.3 Feature scheme (`dep:` syntax + resolver = "2")

```toml
# crates/unblock-indexer/Cargo.toml — [features]
[features]
default = []   # bin owns the policy; lib never default-enables
lang-rust       = ["dep:tree-sitter-rust"]
lang-typescript = ["dep:tree-sitter-typescript"]
lang-javascript = ["dep:tree-sitter-javascript"]
lang-python     = ["dep:tree-sitter-python"]
lang-go         = ["dep:tree-sitter-go"]
lang-java       = ["dep:tree-sitter-java"]
lang-c          = ["dep:tree-sitter-c"]
lang-cpp        = ["dep:tree-sitter-cpp"]    # opt-in
lang-ruby       = ["dep:tree-sitter-ruby"]   # opt-in
lang-php        = ["dep:tree-sitter-php"]

# crates/unblock-code/Cargo.toml — [features]
[features]
default = [
    "lang-rust", "lang-typescript", "lang-javascript", "lang-python",
    "lang-go", "lang-java", "lang-c", "lang-php",
]
lang-rust       = ["unblock-indexer/lang-rust"]
lang-typescript = ["unblock-indexer/lang-typescript"]
# … one per language, including the two opt-ins …
lang-cpp        = ["unblock-indexer/lang-cpp"]
lang-ruby       = ["unblock-indexer/lang-ruby"]
```

`resolver = "2"` is already set workspace-wide (verified at `Cargo.toml` line 7).
The `dep:` prefix prevents Cargo from auto-creating a feature with the same name
as an optional dep.

### 7.4 Build prerequisites & `build.rs`

- Each `tree-sitter-<lang>` upstream crate ships its own `build.rs` that
  compiles its `parser.c` via the `cc` build dependency. **Host C toolchain is
  mandatory** (R-CLI-4): gcc/clang on Linux, Apple Clang via Xcode CLT on macOS,
  MSVC Build Tools on Windows.
- `libsqlite3-sys 0.37 (bundled)` ALSO needs `cc` — the requirement is not new.
- `crates/unblock-indexer/build.rs` does **not** compile grammars itself. It
  performs:
  1. **ABI guard** — for each enabled `lang-<x>` feature, `cargo:rustc-cfg=abi_<n>`
     where `n` is the parser ABI version reported by the grammar crate's
     `LANGUAGE` symbol header.
  2. **`tags.scm` drift check (S3, soft)** — `cargo:warning=` if vendored SHA
     does not match the recorded crates.io tagged commit. CI test `--tags-drift`
     converts the warning to a hard fail when run with that flag.
- `crates/unblock-code/build.rs` performs:
  1. **`find-references` HEURISTIC lint (Q-S-6)** — render the `--help` output
     for `find-references` (compiled via `clap`) and `grep` for the literal
     substrings `HEURISTIC` and `syntactic only, no type resolution`. Failure
     → `panic!()` aborts compilation.
  2. **`symbol_id` opaque-note lint (§4.6)** — same approach, asserts the
     `symbol_id is opaque; do not parse.` substring on the affected commands.

The `cc` requirement is documented in `README.md` under a **"Building from
source"** section (H2). cargo-dist (Phase 04) ships pre-compiled binaries that
remove the user-facing prerequisite.

### 7.5 Language registry

```rust
// crates/unblock-indexer/src/grammars/mod.rs
use std::collections::HashMap;
use tree_sitter::Language as TsLanguage;
use unblock_indexer_core::Language;

pub type LoaderFn = fn() -> TsLanguage;

/// Returns a loader map containing **only** languages whose Cargo feature is
/// enabled in the current build. Default-feature build → 8 entries. Build with
/// `--features lang-cpp,lang-ruby` → 10 entries.
pub fn loaders() -> HashMap<Language, LoaderFn> {
    let mut m = HashMap::new();
    #[cfg(feature = "lang-rust")]      m.insert(Language::Rust,       rust::language as LoaderFn);
    #[cfg(feature = "lang-typescript")]m.insert(Language::TypeScript, typescript::language as LoaderFn);
    #[cfg(feature = "lang-javascript")]m.insert(Language::JavaScript, javascript::language as LoaderFn);
    #[cfg(feature = "lang-python")]    m.insert(Language::Python,     python::language as LoaderFn);
    #[cfg(feature = "lang-go")]        m.insert(Language::Go,         go::language as LoaderFn);
    #[cfg(feature = "lang-java")]      m.insert(Language::Java,       java::language as LoaderFn);
    #[cfg(feature = "lang-c")]         m.insert(Language::C,          c::language as LoaderFn);
    #[cfg(feature = "lang-cpp")]       m.insert(Language::Cpp,        cpp::language as LoaderFn);
    #[cfg(feature = "lang-ruby")]      m.insert(Language::Ruby,       ruby::language as LoaderFn);
    #[cfg(feature = "lang-php")]       m.insert(Language::Php,        php::language as LoaderFn);
    m
}

#[cfg(feature = "lang-rust")]
mod rust {
    pub fn language() -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }
}
// … one mod per active feature …
```

**Runtime ABI guard** (called once per process at `loaders()` use):
```rust
pub fn assert_abi_compat(lang: Language, ts_lang: &TsLanguage) -> Result<(), IndexerError> {
    let abi = ts_lang.abi_version();
    if !(tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION).contains(&abi) {
        return Err(IndexerError::AbiMismatch { language: lang, abi });
    }
    Ok(())
}
```

---

## 8. File Walker & Language Detection

### 8.1 `WalkBuilder` configuration (R7, verbatim)

```rust
// crates/unblock-indexer/src/walker.rs
use ignore::{WalkBuilder, overrides::OverrideBuilder};

pub fn walker(repo_root: &Path, force_include: &[String]) -> Result<ignore::Walk, IndexerError> {
    let mut wb = WalkBuilder::new(repo_root);
    wb.require_git(false)        // mandatory for non-git checkouts (R7)
      .same_file_system(true)    // Unix+Windows only — fine for our 3 targets
      .hidden(false)             // do not skip hidden files by default
      .git_ignore(true)
      .git_global(true)
      .git_exclude(true)
      .ignore(true)
      .parents(false);            // do NOT walk upward to parent .gitignore

    let mut ovr = OverrideBuilder::new(repo_root);
    for pat in DEFAULT_EXCLUDES {
        ovr.add(pat).map_err(IndexerError::override_pattern)?;
    }
    for pat in force_include {     // user `force_include` patterns from .unblock/indexer.toml
        ovr.add(pat).map_err(IndexerError::override_pattern)?;
    }
    wb.overrides(ovr.build().map_err(IndexerError::override_pattern)?);

    Ok(wb.build())
}

pub const DEFAULT_EXCLUDES: &[&str] = &[
    "!target/**",
    "!node_modules/**",
    "!dist/**",
    "!build/**",
    "!.venv/**",
    "!vendor/**",
    "!.git/**",
];
```

**Override precedence** (R7): default-excludes are added FIRST; user `force_include`
patterns are added AFTER. `OverrideBuilder` applies first-match-wins, so a user
include pattern (no leading `!`) like `target/dist-public/**` overrides the
default `!target/**` exclude.

### 8.2 Language detection

```rust
// crates/unblock-indexer/src/walker.rs
pub fn detect_language(path: &Path, overrides: &LanguageOverrides) -> Option<Language> {
    if let Some(lang) = overrides.match_extension(path) {
        return Some(lang);
    }
    DEFAULT_EXTENSION_MAP.get(path.extension()?.to_str()?).copied()
}

pub static DEFAULT_EXTENSION_MAP: &[(&str, Language)] = &[
    ("rs",   Language::Rust),
    ("ts",   Language::TypeScript),
    ("tsx",  Language::TypeScript),
    ("mts",  Language::TypeScript),
    ("cts",  Language::TypeScript),
    ("js",   Language::JavaScript),
    ("jsx",  Language::JavaScript),
    ("mjs",  Language::JavaScript),
    ("cjs",  Language::JavaScript),
    ("py",   Language::Python),
    ("pyi",  Language::Python),
    ("go",   Language::Go),
    ("java", Language::Java),
    ("c",    Language::C),
    ("h",    Language::C),               // ambiguous; user override may steer to Cpp
    ("cc",   Language::Cpp),
    ("cpp",  Language::Cpp),
    ("cxx",  Language::Cpp),
    ("hpp",  Language::Cpp),
    ("hh",   Language::Cpp),
    ("rb",   Language::Ruby),
    ("php",  Language::Php),
];
```

**`.h` ambiguity:** by default `.h` → C. Users on C++ projects override via
`.unblock/languages.toml` (§13.2):

```toml
[extensions]
h = "cpp"
```

### 8.3 File-size threshold

Files larger than **`walker.max_file_bytes`** are skipped with a `tracing::debug!`
event carrying `path`, `size`, and `threshold` fields. The walker reads
`walker.max_file_bytes` from `<repo-root>/.unblock/indexer.toml` (§13.1); falls
back to the default `2 * 1024 * 1024` (2 MiB) if the file is absent, the key is
unset, or the value is invalid (negative, zero, or non-integer). An invalid
value emits a `tracing::warn!` event at config-load time and falls back to the
default — it does NOT abort startup.

The default constant lives in `crates/unblock-indexer/src/walker.rs` as
`pub const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;` and is exposed via
the `WalkerConfig` struct loaded by `crates/unblock-indexer/src/config.rs`
(§13.1). Resolved at the start of every walk; not cached across invocations.

### 8.4 Force-include precedence

User `force_include` from `.unblock/indexer.toml` is applied **after** the
default-excludes Override block. First-match-wins → user inclusion overrides
default exclusion. There is no mechanism for the user to **bypass** the
default excludes wholesale; this is by design (Invariant §17).

---

## 9. AST Traversal & Symbol Extraction

### 9.1 Traversal contract (pure)

The traversal lives in `unblock-indexer-core::ast::visitor`. It is a **pure
function**: it consumes a `tree_sitter::Tree`, the source bytes, and a per-language
capture-translation table, and yields `Vec<Symbol>` with `parent_id = None`.

```rust
// crates/unblock-indexer-core/src/ast/visitor.rs
pub trait CaptureTranslator: Send + Sync {
    /// Map a (capture_name, anchor_node_kind) pair to a `SymbolKind` for the
    /// associated language. Returns `None` if the capture does not yield a
    /// symbol in unblock's vocabulary.
    fn kind_for_capture(
        &self,
        capture_name: &str,
        anchor_node_kind: &str,
    ) -> Option<SymbolKind>;

    fn language(&self) -> Language;
    fn query_source(&self) -> &str;  // tags.scm + ext_<lang>.scm concatenated
}

pub fn traverse<T: CaptureTranslator>(
    tree: &tree_sitter::Tree,
    src: &[u8],
    translator: &T,
    file: &str,
) -> Result<Vec<Symbol>, CoreError> { /* … see §9.4 … */ }
```

The traversal MUST NOT touch the filesystem, sqlx, tokio, or `tracing`. It MAY
allocate (`Vec<Symbol>`, `String`).

### 9.2 Vendored `tags.scm` files

Per R5: each language vendors `tags.scm` from the **crates.io tagged commit** of
its grammar (NOT `master`). For the 4 stale grammars (typescript / cpp / ruby /
java) this is essential because the published binary is from Nov–Dec 2024 while
upstream master may have moved.

The vendored `.scm` files live under `crates/unblock-indexer/src/tags/<lang>.scm`
with the upstream license header preserved verbatim at top-of-file.

### 9.3 Extension queries (R5 scope correction)

Per R5, the plan's "4 hand-written extension queries" estimate was per-category,
not per-language. The actual scope is **~30–40 query rules** distributed across
all 10 languages, covering the categories that upstream `tags.scm` does not
emit consistently:

- **`import`** — Rust `use`, Python `import`/`from`, JS `import`/`require`, TS `import`, Go `import`, Java `import`, C `#include`, C++ `#include`/`using`, Ruby `require`/`require_relative`, PHP `use`.
- **`export`** — JS/TS `export`, Rust `pub` modifier (informational), PHP `?>` wraps (no-op), Python `__all__`, Java `public` modifier (informational).
- **`variable`** / **`constant`** — let/const/var/static across all 10 languages.
- **`field`** — struct/class fields where upstream `tags.scm` omits them.
- **`property`** — JS getter/setter, TS getter/setter, PHP property, Python `@property`.
- **`type_alias`** — Rust `type`, TS `type`, Python `TypeAlias`, Java records (informational), Go `type X = Y`.
- **`macro`** — Rust `macro_rules!`, C/C++ `#define`, PHP `define()`.
- **`namespace`** — PHP `namespace`, C++ `namespace`, Java `package`.

The extension query files are stored at `crates/unblock-indexer/src/tags/ext_<lang>.scm`.
At parse time, `query_source` concatenates `tags.scm` + `ext_<lang>.scm` and
compiles the result via `tree_sitter::Query::new`.

### 9.4 Per-language capture-to-`SymbolKind` translation table

Each language ships a Rust translation table:

```rust
// crates/unblock-indexer/src/tags/rust.rs (illustrative)
use unblock_indexer_core::{ast::CaptureTranslator, Language, SymbolKind};

pub struct RustTranslator;

impl CaptureTranslator for RustTranslator {
    fn kind_for_capture(&self, cap: &str, anchor: &str) -> Option<SymbolKind> {
        match (cap, anchor) {
            // upstream tags.scm captures
            ("definition.function", "function_item")  => Some(SymbolKind::Function),
            ("definition.method",   "function_item")  => Some(SymbolKind::Method),
            ("definition.class",    "struct_item")    => Some(SymbolKind::Struct),
            ("definition.class",    "enum_item")      => Some(SymbolKind::Enum),
            ("definition.class",    "union_item")     => Some(SymbolKind::Struct),
            ("definition.interface","trait_item")     => Some(SymbolKind::Trait),
            ("definition.module",   "mod_item")       => Some(SymbolKind::Module),
            ("definition.macro",    "macro_definition")=> Some(SymbolKind::Macro),
            // ext_rust.scm captures
            ("definition.import",   _)                => Some(SymbolKind::Import),
            ("definition.constant", "const_item")     => Some(SymbolKind::Constant),
            ("definition.variable", "let_declaration")=> Some(SymbolKind::Variable),
            ("definition.type_alias","type_item")     => Some(SymbolKind::TypeAlias),
            ("definition.field",    "field_declaration") => Some(SymbolKind::Field),
            _ => None,
        }
    }
    fn language(&self) -> Language { Language::Rust }
    fn query_source(&self) -> &str { TAGS_RUST_FULL }   // const concatenation of tags.scm + ext_rust.scm
}

const TAGS_RUST_FULL: &str = concat!(
    include_str!("rust.scm"),
    include_str!("ext_rust.scm"),
);
```

A similar translator exists for each active language. The mapping discipline
is **table-driven** — no clever logic. **R5.3 mitigation:** the discriminator
is the **anchor-node-kind** (e.g. `struct_item` vs `enum_item`), which prevents
the upstream `@definition.class` capture from collapsing struct/enum/union
into a single bucket.

### 9.5 `parent_id` post-traversal algorithm

Per R5, parent linkage is computed in O(n) per file:

1. After the AST visitor returns `Vec<Symbol>` for a file, sort by
   `(start_offset asc, end_offset desc)`. Ties: outermost first.
2. Initialise an empty stack of `(SymbolId, Span)` representing currently-open
   ancestors.
3. For each symbol `S` in sorted order:
   a. While the top of the stack has `end_offset < S.start_offset`, pop it.
   b. If the stack is non-empty, set `S.parent_id = Some(stack.top().id)`.
      Else `S.parent_id = None`.
   c. Push `S` onto the stack.

The traversal occurs **before** SQLite assigns rowids — so the algorithm
operates on a temporary `Vec<Symbol>` and then issues a second pass once
rowids are known to back-fill `parent_id` columns. The implementation is in
`unblock-indexer-core::parent::resolve_parents`.

### 9.6 Signature extraction

For each symbol kind, the visitor extracts a single-line signature:

| Kind                   | Signature source                                              |
| ---------------------- | ------------------------------------------------------------- |
| function / method      | First line of the function declaration (params + return type) |
| struct / class / enum  | First line of the type declaration                            |
| trait / interface      | First line of the trait/interface declaration                 |
| field / property       | The field declaration line                                    |
| variable / constant    | The declaration line including type + initial value (capped at 200 chars) |
| import / export        | The full import/export statement (capped at 500 chars)        |
| macro / type_alias     | First line                                                    |
| module / namespace     | The declaration line (e.g. `mod foo;`, `namespace App\Bar;`)  |

**Multi-line signature normalisation:** runs of whitespace become single
spaces; signature is at most 500 chars (truncated with trailing `…`).

### 9.7 Comment attachment & stripping per language family

#### 9.7.1 Attachment rules

| Family       | Attachment rule                                                                                |
| ------------ | ---------------------------------------------------------------------------------------------- |
| Rust         | `///` outer doc comments OR `/** … */` immediately preceding the symbol; `//!` excluded.       |
| TypeScript / JavaScript | JSDoc (`/** … */`) immediately preceding the symbol.                                |
| Python       | Triple-quoted string literal (`"""`/`'''`) as the **first statement** of the symbol's body.    |
| Go           | Line comments (`//`) immediately preceding the symbol with no blank line.                      |
| Java         | Javadoc (`/** … */`) immediately preceding the symbol.                                         |
| C / C++      | `/** … */` or `///` immediately preceding the symbol.                                          |
| Ruby         | Line comments (`#`) immediately preceding the symbol with no blank line; or `=begin … =end`.   |
| PHP          | PHPDoc (`/** … */`) immediately preceding the symbol.                                          |

"Immediately preceding" means: the comment block ends at line `S.start_line - 1`
(inclusive), with at most a single blank line between (Python: zero blank lines
since the docstring is inside the body).

#### 9.7.2 Stripping rules (v1.0.0)

The `comment` column in `symbols` stores the **stripped** text — comment-leader
markers MUST NOT appear in the column. FTS5 indexes the stripped text and
`snippet()` results render clean text. Stripping is per language family:

| Family | Rule |
|---|---|
| Rust `///` / `//!` line doc | Strip leading `///` or `//!` (with optional single space after); join consecutive lines with `\n`. |
| Rust `/** … */` block doc | Strip opening `/**`, closing `*/`, and any leading `*` (with optional space) on intermediate lines. |
| TypeScript / JavaScript JSDoc, Java Javadoc, C / C++ / PHP `/** … */` | Same as Rust block doc: strip `/**`, `*/`, and per-line leading `* `. |
| C / C++ `///` line doc (when used) | Same as Rust `///`. |
| Python docstring | Strip surrounding `"""` or `'''` (single or triple). Preserve internal indentation as-is (do NOT reflow). |
| Go `//` line comments | Strip `// ` (or `//` without space) prefix per line; join with `\n`. |
| Ruby `#` line comments | Strip `# ` (or `#` without space) prefix per line; join with `\n`. |
| Ruby `=begin … =end` block | Strip the `=begin\n` opening line and `=end\n` closing line; preserve content between verbatim. |

#### 9.7.3 Implementation

Each language family gets a small `strip_<family>(raw: &str) -> String` helper
in `crates/unblock-indexer-core/src/comment.rs` (pure, no IO, no allocation
beyond the result `String`). Total ~60–80 LOC across the 8 families. The
visitor calls the appropriate helper based on the matched comment-node kind
before storing into `Symbol::comment`.

```rust
// crates/unblock-indexer-core/src/comment.rs
pub fn strip_rust_line(raw: &str) -> String { /* /// + //! */ }
pub fn strip_block_doc(raw: &str) -> String { /* /** … */ — Rust, JS, TS, Java, C, C++, PHP */ }
pub fn strip_python_docstring(raw: &str) -> String { /* triple-quote variants */ }
pub fn strip_go(raw: &str) -> String { /* // line */ }
pub fn strip_ruby_line(raw: &str) -> String { /* # line */ }
pub fn strip_ruby_block(raw: &str) -> String { /* =begin … =end */ }
```

#### 9.7.4 Idempotency invariant (property test)

For every helper:

```
strip_<family>(strip_<family>(s)) == strip_<family>(s)   for all s
```

Verified by proptest fixtures in `crates/unblock-indexer-core/tests/comment.rs`
(§16.4).

### 9.8 Empty-result invariant

If a file parses successfully but yields zero symbols, an entry in `files` is
still written (for mtime tracking) with `symbol_count = 0`. Subsequent queries
on that file return empty result sets without re-parsing.

---

## 10. Lifecycle: Bootstrap & Steady-State (NO watcher)

### 10.1 Cold bootstrap (`init` or first lazy use)

Algorithm in `unblock-indexer::bootstrap::run_full`:

1. **Acquire pool** — open `index.db` with WAL; `after_connect` asserts FTS5 (§5.2).
2. **Schema check** — `ensure_schema` (§5.5); on mismatch, wipe + recreate.
3. **Drop triggers** — execute `DROP_TRIGGERS` (§5.3).
4. **Walk repo** — `walker(repo_root, force_include)` yields `DirEntry`s.
5. **Parallel parse** — rayon distributes file parsing across `num_cpus()`
   threads; each thread parses to a `Vec<Symbol>`.
6. **Chunked insert** — symbols are bucketed into ~500-row chunks; each chunk
   is committed in its own `BEGIN..COMMIT` (§5.6).
7. **`parent_id` back-fill** — after all rowids are known, run
   `UPDATE symbols SET parent_id = ? WHERE id = ?` per file using the algorithm
   in §9.5; chunked at ~500 rows / tx.
8. **FTS rebuild** — execute `FTS_REBUILD` (`INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');`).
9. **Re-create triggers** — execute `CREATE_TRIG_AI`, `CREATE_TRIG_AD`, `CREATE_TRIG_AU`.
10. **Update meta** — write `schema_version`, `indexer_version`, `repo_root`,
    `created_at` (only on first `init`), and `last_full_index = now()` to the
    SQLite `meta` table (§6.3). No auxiliary file is written.
11. **Emit envelope** — `init` reports duration, files indexed, symbols emitted.

### 10.2 Per-query mtime check (sole sync mechanism, INVARIANT)

Per L9: between CLI invocations there is no watcher, no daemon, no in-memory
cache. Every query path runs an mtime check against the implicated files.

```rust
// crates/unblock-indexer/src/mtime.rs
pub async fn ensure_fresh(
    pool: &sqlx::SqlitePool,
    implicated: &[PathBuf],
) -> Result<MtimeReport, IndexerError> {
    // 1. Stat each implicated file (fs::metadata).
    // 2. SELECT files.mtime_unix_ms WHERE file IN (?, ?, …).
    // 3. For each file with fs.mtime > db.mtime: re-parse + replace symbols.
    // 4. Cap implicated set at IMPLICATED_CAP[command] (§10.3).
    // 5. If a file is implicated but not in `files` table, parse it now.
    // 6. Return MtimeReport { reparsed: u32, stale: bool }.
}
```

`stale: true` is set on the JSON envelope when the cap was reached and at least
one un-checked implicated file existed. Clients SHOULD honour this signal by
issuing `unblock-code reindex` if precision matters.

### 10.3 Implicated-file caps per command (R8)

| Command              | Implicated-file cap | How the cap is computed                                                  |
| -------------------- | ------------------: | ------------------------------------------------------------------------ |
| `find-symbol`        | 4                  | First 4 files matching the candidate-name index lookup.                  |
| `outline`            | 1                  | The exact file requested.                                                |
| `get-symbol`         | 1                  | The file containing the requested `symbol_id`.                           |
| `list-symbols`       | 16                 | Recursive case: first 16 files under the requested directory.            |
| `search`             | 4                  | First 4 files whose FTS5 row hits the query.                             |
| `find-references`    | 16                 | First 16 files matching the candidate-identifier grep.                   |
| `parse`              | 1                  | The exact file requested.                                                |
| `status`/`languages` | 0                  | No mtime check.                                                          |
| `init`/`reindex`     | unbounded          | Bootstrap or subtree reindex.                                            |

The caps are **constants** in `unblock-indexer::mtime::IMPLICATED_CAP` and are
NOT user-configurable in v1.0.0.

### 10.4 `reindex(path?)` semantics (Q-S-3)

```rust
// crates/unblock-indexer/src/reindex.rs
pub async fn run_reindex(
    pool: &sqlx::SqlitePool,
    repo_root: &Path,
    path: Option<&Path>,
) -> Result<ReindexReport, IndexerError> {
    match path {
        None => bootstrap::run_full(pool, repo_root).await,  // §10.1
        Some(p) => run_subtree(pool, repo_root, p).await,
    }
}

async fn run_subtree(...) -> Result<ReindexReport, IndexerError> {
    // 1. Resolve `p` relative to repo_root; reject path-traversal escapes.
    // 2. DELETE FROM files WHERE file LIKE 'p/%' — ON DELETE CASCADE removes symbols.
    //    (Triggers fire `'delete'` rows on FTS5; do NOT drop triggers.)
    // 3. Walk the subtree (`walker(repo_root, force_include)` filtered to subtree).
    // 4. Parse + insert per file using existing triggers (live FTS5 sync).
    // 5. Run parent_id back-fill restricted to the affected files.
    // 6. Return ReindexReport { files_reparsed, symbols_emitted, duration_ms }.
}
```

Subtree reindex does **not** use the bulk-rebuild path because the trigger
fan-out is acceptable for small file counts. The threshold for "small" is
**implicit**: subtree reindex is only invoked when the user requests a path,
so the path is typically a small fraction of the repo.

### 10.5 Forced reindex semantics

`unblock-code reindex` (no path) is functionally identical to `unblock-code init`
**except**: `init` errors with `IndexerError::AlreadyInitialised` (exit 2) if
the SQLite `meta` table at `<cache>/index.db` already contains a `created_at`
row, while `reindex` proceeds unconditionally. The "already initialised" probe
reads from SQLite only — there is no auxiliary file to inspect.

---

## 11. CLI Surface

### 11.1 Common conventions (D1–D4)

- **D1.** Exactly one JSON envelope per invocation on **stdout**. No NDJSON.
- **D2.** Errors emitted on **stdout** as `{"error":{"code":"...","message":"...","details":{...}}}`.
  Exit code (per L17) signals additionally. **Stderr** carries `tracing` JSON
  Lines + human progress. **Stdout is reserved for the envelope** (Invariant §17).
- **D3.** Minified JSON by default. `--pretty` flag enables indented output.
- **D4.** `symbol_id` is an **opaque string** (Q-S-7); clients MUST NOT parse.
- **`tool` field** — every envelope carries `"tool": "<command-name>"` as the
  literal CLI subcommand, e.g. `"tool": "find-symbol"`.
- **Field naming** — `snake_case` for all envelope fields.
- **Span field** — `"span": {"start_line":…,"start_col":…,"end_line":…,"end_col":…}`,
  1-based line/col; byte offsets are NOT exposed on the wire.
- **`stale: true`** — set when per-query mtime check capped re-parse.
- **`truncated: true`** — set when `--limit N` cut results.
- **`heuristic: true` + `warning`** — set ONLY on `find-references`.
- **`duration_ms`** — wall-clock from command dispatch to envelope serialisation,
  inclusive.

### 11.2 Global flags

```
unblock-code [GLOBAL FLAGS] <command> [COMMAND ARGS]

GLOBAL FLAGS:
  --repo-root <path>     Override repo-root discovery.
  --pretty               Pretty-print JSON envelope.
  --json-only            Suppress non-error stderr output (still emits tracing
                         JSON Lines if RUST_LOG is set).
  --no-mtime-check       Skip the per-query mtime check (debug-only; SHOULD NOT
                         be used in production agent flows).
  --help                 Print help.
  --version              Print version.
```

### 11.3 `find-symbol`

```
unblock-code find-symbol <NAME> [--kind <KIND>] [--language <LANG>]
                                 [--limit <N>] [--exact|--prefix|--substring]
```

**Default match mode:** `--exact`. Wire format:

```json
{
  "tool": "find-symbol",
  "matches": [
    {
      "symbol_id": "12345",
      "name": "parse_github_url",
      "kind": "function",
      "language": "rust",
      "file": "crates/unblock-github/src/client.rs",
      "span": {"start_line":42,"start_col":1,"end_line":58,"end_col":2},
      "signature": "pub fn parse_github_url(s: &str) -> Result<GitHubUrl, Error>",
      "parent_id": null
    }
  ],
  "total_matches": 1,
  "stale": false,
  "truncated": false,
  "duration_ms": 4
}
```

**Behaviour:**
- Exact-match path uses `idx_symbols_name`; substring-match path falls back to
  FTS5 over the `name` column.
- `--limit` defaults to 50; `truncated: true` set when more matches exist.
- `--language` filters by `Language::as_str` exact match.
- `--kind` accepts comma-separated `SymbolKind` values, e.g.
  `--kind function,method,class`.

### 11.4 `list-symbols`

```
unblock-code list-symbols <PATH> [--recursive] [--kind <KIND>]
```

`PATH` may be a file or directory. With `--recursive`, all files under the dir
are included (capped at 16 implicated files per §10.3). Wire format:

```json
{
  "tool": "list-symbols",
  "file": "crates/unblock-core/src/graph.rs",
  "symbols": [
    {"symbol_id":"7","name":"GraphBuilder","kind":"struct","span":{...},"parent_id":null},
    {"symbol_id":"8","name":"new","kind":"method","span":{...},"parent_id":"7"}
  ],
  "stale": false,
  "duration_ms": 3
}
```

Recursive mode adds an outer `files: [{file, symbols: […]}]` array — schema
documented in `cli.rs` and pinned in fixtures.

### 11.5 `outline`

```
unblock-code outline <FILE>
```

Returns the parent-child symbol tree for the file:

```json
{
  "tool": "outline",
  "file": "crates/unblock-core/src/graph.rs",
  "language": "rust",
  "tree": [
    {
      "symbol_id":"7","name":"GraphBuilder","kind":"struct","span":{...},
      "children": [
        {"symbol_id":"8","name":"new","kind":"method","span":{...},"children":[]}
      ]
    }
  ],
  "duration_ms": 2
}
```

### 11.6 `get-symbol`

```
unblock-code get-symbol <SYMBOL_ID> [--body]
```

Returns full record. With `--body`, reads the source from filesystem at query
time using span:

```json
{
  "tool": "get-symbol",
  "symbol_id": "12345",
  "name": "parse_github_url",
  "kind": "function",
  "language": "rust",
  "file": "crates/unblock-github/src/client.rs",
  "span": {...},
  "signature": "pub fn parse_github_url(...) -> ...",
  "comment": "/// Parses a GitHub URL into…",
  "parent_id": null,
  "body": "pub fn parse_github_url(s: &str) -> Result<…> {\n    …\n}"
}
```

**Body bounds (Q-S-4):**
- Cap at **64 KiB** (`MAX_BODY_BYTES = 64 * 1024`).
- If exceeded: truncate at byte 65,536, append the literal `\n…\n`, and set
  `"body_truncated": true` on the envelope.
- **Path-traversal guard:** the file path is canonicalised under the repo root
  before any read; reads outside the repo root error with
  `IndexerError::PathEscape` (exit 2).
- Non-UTF-8 bytes: replaced with `U+FFFD` (replacement character) — the body is
  ALWAYS valid UTF-8 on the wire.

### 11.7 `search`

```
unblock-code search <QUERY> [--kind <KIND>] [--language <LANG>] [--limit <N>]
```

Wire format:

```json
{
  "tool": "search",
  "query": "github url",
  "matches": [
    {
      "symbol_id": "12345",
      "name": "parse_github_url",
      "kind": "function",
      "file": "crates/unblock-github/src/client.rs",
      "span": {...},
      "snippet": "<b>Parses</b> a <b>GitHub URL</b> into…"
    }
  ],
  "total_matches": 1,
  "duration_ms": 6
}
```

**FTS5 sanitisation (Q-S-5):**
1. `let q = input.trim();`
2. Strip leading single/double quotes (idempotent).
3. Reject queries containing `:` (column qualifier) or `*` (prefix syntax) with
   `IndexerError::InvalidFtsQuery` (exit 2) — these are advanced FTS5 syntax
   and ambiguous in agent prompts.
4. Escape inner `"` as `""`.
5. Wrap in `"..."` for phrase quoting.
6. Apply to `symbols_fts MATCH ?`; `snippet()` aux function highlights `<b>...</b>`.

The literal sanitisation logic lives in `unblock-indexer::query::sanitise_fts_query`
and has its own unit-test fixture set.

### 11.8 `find-references`

```
unblock-code find-references <NAME> [--limit <N>]
```

Wire format (Q-S-6):

```json
{
  "tool": "find-references",
  "heuristic": true,
  "warning": "syntactic only, no type resolution",
  "references": [
    {
      "file": "crates/unblock-core/src/lib.rs",
      "span": {...},
      "surrounding_symbol": {
        "symbol_id": "42",
        "name": "init",
        "kind": "function"
      }
    }
  ],
  "total_references": 1,
  "duration_ms": 9
}
```

**HEURISTIC implementation:**
1. Extract candidate identifiers via tree-sitter `identifier`-like captures.
2. Match candidate identifiers against the `name` parameter (exact string).
3. Cap implicated files at 16 (§10.3).
4. Resolve `surrounding_symbol` via `parent_id` chain at the reference position.

**Lint enforcement (Q-S-6):** `crates/unblock-code/build.rs` asserts:
- The literal substrings `HEURISTIC` and `syntactic only, no type resolution`
  appear in `find-references --help`.
- The literal substring `syntactic only, no type resolution` appears in the
  envelope's `warning` field (verified against a static fixture in `commands/find_references.rs`).
- Failure → `panic!("find-references HEURISTIC lint failed: …");` aborts compilation.

### 11.9 `reindex`

```
unblock-code reindex [PATH]
```

Behaviour per §10.4. Wire format:

```json
{
  "tool": "reindex",
  "path": null,
  "files_reparsed": 487,
  "symbols_emitted": 5912,
  "duration_ms": 1820
}
```

`path` is `null` for full reindex; the resolved repo-relative POSIX path
otherwise.

### 11.10 `status`

```
unblock-code status
```

All metadata fields are read from the SQLite `meta` table (§6.3). No auxiliary
file is consulted. The `--pretty` global flag covers human readability; the
default minified envelope is the contract. Wire format:

```json
{
  "tool": "status",
  "repo_root": "/Users/me/repo",
  "repo_hash": "f3c7…",
  "schema_version": 1,
  "indexer_version": "1.0.0",
  "last_full_index": "2026-04-29T12:00:00Z",
  "last_incremental": "2026-04-29T12:05:00Z",
  "total_files": 487,
  "total_symbols": 5912,
  "db_size_bytes": 1843200,
  "fts5_enabled": true,
  "languages_active": ["rust","typescript","javascript","python","go","java","c","php"]
}
```

### 11.11 `languages`

```
unblock-code languages
```

Wire format:

```json
{
  "tool": "languages",
  "languages": [
    {"language":"rust","file_count":120,"symbol_count":1523},
    {"language":"typescript","file_count":80,"symbol_count":2104}
  ]
}
```

Only languages whose Cargo feature is enabled in the running binary appear.
Default install lists 8; `--features lang-cpp,lang-ruby` install lists 10.

### 11.12 `init`

```
unblock-code init [--force]
```

Wire format:

```json
{
  "tool": "init",
  "repo_root": "/Users/me/repo",
  "languages_detected": ["rust","typescript"],
  "files_indexed": 487,
  "symbols_emitted": 5912,
  "duration_ms": 1820
}
```

Error if the cache `index.db` already contains a `meta.created_at` row and
`--force` is not passed: `IndexerError::AlreadyInitialised` (exit 2). Probe is
SQLite-only — see §10.5.

### 11.13 `parse`

```
unblock-code parse <FILE> [--json-tree]
```

Default emits the S-expression form of the tree-sitter parse tree:

```json
{
  "tool": "parse",
  "file": "crates/unblock-core/src/lib.rs",
  "language": "rust",
  "tree": "(source_file (function_item name: (identifier) parameters: (parameters) body: (block)))"
}
```

With `--json-tree` the tree is emitted as a verbose JSON structure (recursive
`{kind, span, children: […]}`). The JSON form is **deterministic** — node ordering
is stable for identical inputs (S4 SOFT gate).

---

## 12. Error Model

### 12.1 `CoreError` (in `unblock-indexer-core::errors`)

Pure errors raised inside the AST visitor / schema / parent computation:

```rust
#[non_exhaustive]
#[derive(Debug, snafu::Snafu)]
pub enum CoreError {
    #[snafu(display("query compilation failed for {language}: {source}"))]
    QueryCompile { language: Language, source: tree_sitter::QueryError },

    #[snafu(display("schema decode error in column {column}: value {value:?} not a known {kind}"))]
    SchemaDecode { column: &'static str, value: String, kind: &'static str },

    #[snafu(display("invalid span: end before start at offset {offset}"))]
    InvalidSpan { offset: u32 },

    #[snafu(display("parent_id resolution overflow at depth {depth}"))]
    ParentDepth { depth: u32 },
}

pub type CoreResult<T> = core::result::Result<T, CoreError>;
```

### 12.2 `IndexerError` (in `unblock-indexer::errors`)

```rust
#[non_exhaustive]
#[derive(Debug, snafu::Snafu)]
pub enum IndexerError {
    #[snafu(display("invalid input: {message}"))]
    InvalidInput { message: String },                                // exit 2
    #[snafu(display("repo root not found from {cwd}"))]
    RepoRootNotFound { cwd: PathBuf },                                // exit 2
    #[snafu(display("path escapes repo root: {path}"))]
    PathEscape { path: PathBuf },                                     // exit 2
    #[snafu(display("already initialised at {path}; use --force or `reindex`"))]
    AlreadyInitialised { path: PathBuf },                             // exit 2
    #[snafu(display("invalid FTS5 query: {message}"))]
    InvalidFtsQuery { message: String },                              // exit 2

    #[snafu(display("language {language} not enabled in this build"))]
    LanguageDisabled { language: Language },                          // exit 3
    #[snafu(display("no language detected for {path}"))]
    UnknownLanguage { path: PathBuf },                                // exit 3
    #[snafu(display("ABI mismatch for {language}: parser ABI {abi} not in compatibility window"))]
    AbiMismatch { language: Language, abi: usize },                   // exit 3

    #[snafu(display("walker override pattern error: {source}"))]
    OverridePattern { source: ignore::Error },                         // exit 4
    #[snafu(display("io error reading {path}: {source}"))]
    Io { path: PathBuf, source: std::io::Error },                     // exit 4

    #[snafu(display("database error: {source}"))]
    Db { source: sqlx::Error },                                       // exit 5
    #[snafu(display("database open failed at {path}: {source}"))]
    DbOpen { path: PathBuf, source: sqlx::Error },                    // exit 5
    #[snafu(display("schema migration failed: {source}"))]
    Schema { source: sqlx::Error },                                   // exit 5

    #[snafu(display("SQLite was built without ENABLE_FTS5 — exit 6"))]
    Fts5Missing,                                                       // exit 6

    #[snafu(display("parse failure for {path}: {message}"))]
    Parse { path: PathBuf, message: String },                          // exit 7
    #[snafu(display("query execution error for {language}: {source}"))]
    QueryExec { language: Language, source: tree_sitter::QueryError }, // exit 7
    #[snafu(display("AST traversal error: {source}"))]
    Ast { source: CoreError },                                         // exit 7

    #[snafu(display("internal error: {message}"))]
    Internal { message: String },                                      // exit 99
}

pub type IndexerResult<T> = core::result::Result<T, IndexerError>;
```

### 12.3 `CliError` and exit-code mapping (in `unblock-code::errors`)

```rust
#[non_exhaustive]
#[derive(Debug, snafu::Snafu)]
pub enum CliError {
    #[snafu(display("CLI parse error: {source}"))]
    Clap { source: clap::Error },                              // exit 2
    #[snafu(display("envelope serialisation failed: {source}"))]
    Envelope { source: serde_json::Error },                    // exit 99
    #[snafu(transparent)]
    Indexer { source: IndexerError },                          // delegated mapping
    #[snafu(display("tracing initialisation failed: {source}"))]
    TracingInit { source: tracing_subscriber::util::TryInitError },  // exit 99
}
```

**Exit-code table (L17):**

| Code | Family               | `IndexerError` variants                                                          |
| ---: | -------------------- | -------------------------------------------------------------------------------- |
| 0    | success              | (none)                                                                           |
| 2    | invalid input        | `InvalidInput`, `RepoRootNotFound`, `PathEscape`, `AlreadyInitialised`, `InvalidFtsQuery`, `Clap` |
| 3    | unsupported language | `LanguageDisabled`, `UnknownLanguage`, `AbiMismatch`                              |
| 4    | grammar / IO / network | `OverridePattern`, `Io` (network is no-op in v1.0.0)                            |
| 5    | database             | `Db`, `DbOpen`, `Schema`                                                         |
| 6    | FTS5                 | `Fts5Missing`                                                                    |
| 7    | parse                | `Parse`, `QueryExec`, `Ast`                                                      |
| 99   | internal             | `Internal`, `Envelope`, `TracingInit`                                            |

The mapping function lives in `crates/unblock-code/src/errors.rs::exit_code(&CliError) -> i32`
and is exhaustive (`#[non_exhaustive]` notwithstanding — the function takes a
reference and uses `_ => 99` for forward-compat, then panics in tests on the
catch-all).

### 12.4 Wire-format error envelope (D2)

```json
{
  "error": {
    "code": "fts5_missing",
    "message": "SQLite was built without ENABLE_FTS5 — exit 6",
    "details": {}
  }
}
```

`code` is a stable, snake_case identifier matching one of the `IndexerError`
variants (e.g. `invalid_input`, `repo_root_not_found`, `language_disabled`,
`fts5_missing`, `parse_error`). `details` carries variant-specific structured
data (e.g. `{"path": "…"}` for `PathEscape`).

---

## 13. Configuration

### 13.1 `.unblock/indexer.toml`

Located at `<repo-root>/.unblock/indexer.toml`. **Optional**; absence means
defaults.

```toml
[walker]
# Maximum size in bytes for any single file passed to the parser. Files
# exceeding this threshold are skipped with a tracing::debug! event carrying
# path, size, and threshold (§8.3). Default: 2 MiB. Invalid values fall back
# to the default with a tracing::warn! at config-load time.
max_file_bytes = 2097152   # 2 * 1024 * 1024

# Force-include patterns (added AFTER default-excludes, first-match-wins).
# Use to re-include a path the default-excludes blocked.
force_include = [
    "vendor/my-internal-tool/**",
    "target/dist-public/**",
]

[languages]
# Languages disabled at index time (does NOT change Cargo features — disables
# detection at runtime so files are skipped). Useful when a project has a few
# stray .py files but is primarily Rust and you want to skip them.
disabled = []
```

The Rust loader struct in `crates/unblock-indexer/src/config.rs`:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct IndexerConfig {
    pub walker: WalkerConfig,
    pub languages: LanguagesConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct WalkerConfig {
    /// Maximum size in bytes for files the walker will hand to the parser.
    /// Defaults to `DEFAULT_MAX_FILE_BYTES` (2 MiB) when absent or invalid.
    pub max_file_bytes: u64,
    /// Force-include glob patterns layered on top of DEFAULT_EXCLUDES.
    pub force_include: Vec<String>,
}

impl Default for WalkerConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: crate::walker::DEFAULT_MAX_FILE_BYTES,
            force_include: Vec::new(),
        }
    }
}
```

**No `debounce_ms` field.** Per L9, the per-query mtime check is the sole sync
mechanism — there is no debouncer.

### 13.2 `.unblock/languages.toml`

Optional per-extension override:

```toml
[extensions]
# .h files are C++ headers in this project, not C.
h = "cpp"
# .pl files are Perl — we don't index them; map to a non-existent variant
# to skip detection (or use languages.disabled in indexer.toml).
```

The values must be one of the ten `Language::as_str` strings. Unknown values
error at config-load time with `IndexerError::InvalidInput` (exit 2).

### 13.3 Environment variables

| Variable             | Purpose                                                                    |
| -------------------- | -------------------------------------------------------------------------- |
| `XDG_CACHE_HOME`     | Override cache root (§6.1).                                                |
| `UNBLOCK_INDEXER_LOG`| `tracing-subscriber` filter (e.g. `info,unblock_indexer=debug`). Maps to RUST_LOG-style filter. |
| `RUST_LOG`           | Fallback if `UNBLOCK_INDEXER_LOG` is unset.                                |
| `UNBLOCK_NO_COLOR`   | Disable colour in human stderr output (envelope is unaffected).            |

### 13.4 Config precedence

1. CLI flag (e.g. `--repo-root`).
2. `.unblock/indexer.toml` / `.unblock/languages.toml`.
3. Built-in defaults (DEFAULT_EXCLUDES, DEFAULT_EXTENSION_MAP, …).

---

## 14. Performance Methodology & Gates

### 14.1 Corpus tiers (R8)

| Tier   | Repo                                | Files (approx) | Symbols (approx) |
| ------ | ----------------------------------- | -------------: | ---------------: |
| Small  | unblock itself                      | 500            | 5,000            |
| Medium | ripgrep + tokio combined            | 5,000          | 50,000           |
| Large  | LLVM monorepo subset (clang+llvm)   | 50,000         | 500,000          |

**Primary gate corpus:** Medium. Small is for fast iteration during
development; Large is informational (Linux only).

### 14.2 Criterion harness

Located at `crates/unblock-indexer/benches/queries.rs`:

```rust
use criterion::{Criterion, criterion_group, criterion_main};

fn bench_find_symbol(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all().build().unwrap();
    let pool = runtime.block_on(open_pool_for_corpus("medium"));
    let mut group = c.benchmark_group("find-symbol");
    group.sample_size(100).measurement_time(std::time::Duration::from_secs(10));
    group.bench_function("warm", |b| {
        b.to_async(&runtime).iter(|| async {
            unblock_indexer::query::find_symbol(&pool, "Foo", Default::default()).await.unwrap()
        });
    });
}

criterion_group!(benches, bench_find_symbol, /* outline, list-symbols, search */);
criterion_main!(benches);
```

Warm-path measurement: criterion's default 5 s warmup + 10 s measurement;
sample size 100 (R8). Cold-path is `cargo build`-bound and is measured by a
separate harness (§14.4).

### 14.3 Warm-path p99 budgets (HARD — L20)

Pinned on the Medium corpus warm DB on Linux x86_64:

| Command                       | p99 budget |
| ----------------------------- | ---------: |
| `find-symbol`                 | < 10 ms    |
| `outline`                     | < 20 ms    |
| `list-symbols` (recursive, 16-cap) | < 50 ms |
| `search` (FTS5 query)         | < 30 ms    |
| `find-references`             | (no budget; informational) |

These are HARD acceptance criteria via H5. Each command's bench fixture asserts
the criterion-reported p99 against the budget; failure is a CI failure.

### 14.4 Cold-start budget (HARD — L21, Q-S-2)

L21 budget: **process spawn → first JSON byte on stdout** measured end-to-end.

| Platform              | Budget (p95)       | Gate level     |
| --------------------- | ------------------: | -------------- |
| Linux x86_64          | < 100 ms full-load | **HARD (H6)**  |
| macOS aarch64         | < 100 ms full-load | informational  |
| Windows x86_64 (MSVC) | < 150 ms full-load (+50% allowance per R-CLI-1.2) | informational |

**Measurement harness** (in `crates/unblock-code/benches/cold_start.rs`):
1. Spawn `target/release/unblock-code status` 100 times consecutively.
2. Measure wall-clock from `Command::spawn` to first byte on stdout.
3. Report p50 / p95 / p99.
4. Linux: assert p95 < 100 ms; failure → CI failure.
5. macOS / Windows: report only.

### 14.5 HARD/SOFT gate split (final)

**HARD — release-blocking (9 total):**

- **H1.** `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc --no-deps`, `cargo test` pass workspace-wide.
- **H2.** Binary builds with default features on Linux x86_64 (gcc/clang), macOS aarch64 (Apple Clang via Xcode CLT), Windows x86_64 (MSVC Build Tools). README documents prerequisites in "Building from source".
- **H3.** Top-10 languages parse a mixed-language fixture repo without panic on a CI build with `--features lang-cpp,lang-ruby` enabled.
- **H4.** All 11 CLI commands return JSON envelopes that validate against `tests/fixtures/envelopes/<command>.schema.json` (JSON Schema draft-2020-12).
- **H5.** Warm-path p99 budgets met on Medium corpus warm DB (§14.3).
- **H6.** Cold-start p95 < 100 ms full-load on Linux x86_64 warm DB Medium corpus (§14.4).
- **H7.** FTS5 PRAGMA assertion fires on connection open; binary refuses to operate on a SQLite without FTS5 (exit 6).
- **H8.** Per-query mtime check verified by integration test that mutates a tracked file and asserts the next query reflects the new symbol set without explicit `reindex`.
- **H9.** `find-references` envelope always carries `heuristic: true` and the documented warning string; `--help` lint passes.

**SOFT — warning / follow-up bead, non-blocking (5 total):**

- **S1.** Default-feature stripped binary ≤ 30 MB on Linux x86_64 (R-CLI-2; warns if exceeded).
- **S2.** `cargo install unblock-code --no-default-features --features lang-rust,lang-python` succeeds; resulting binary passes the test suite for the selected languages; size reduction ≥ 50% vs full build.
- **S3.** Vendored `tags.scm` files match upstream pinned commits (CI drift check; warns if drift).
- **S4.** `parse --json-tree` output is byte-stable across runs on identical input.
- **S5.** ROI report ≥ 2.0× median across 3 flows × N=10 runs (release-gate one-shot, NOT per-PR CI; §15).

---

## 15. ROI Harness

### 15.1 Location & invocation

Source: `tests/roi/` (NOT under `crates/`). Driver: `tests/roi/run.rs` (a
standalone binary in a `[[bin]]` of `tests/roi/Cargo.toml`).

Invocation:
```
ANTHROPIC_API_KEY=... cargo run -p unblock-roi-harness -- \
    --output target/roi-report.json \
    --runs 10 \
    --model claude-sonnet-4-7-20260101
```

### 15.2 Three flows (R-CLI-5, locked)

| Flow | Question                                                              | Indexer command                                    |
| ---- | --------------------------------------------------------------------- | -------------------------------------------------- |
| A    | "Where is `Symbol::find_one` defined?"                                 | `find-symbol Symbol::find_one`                     |
| B    | "Show me the structure of `crates/unblock-core/src/graph.rs`."        | `outline crates/unblock-core/src/graph.rs`         |
| C    | "Where is `parse_github_url` referenced?"                              | `find-references parse_github_url`                 |

**Gold answers** are pinned under `tests/roi/gold/` as JSON files; the harness
asserts the agent's final answer matches the gold (LLM-as-judge is NOT used —
exact JSON equality on the relevant slice).

### 15.3 Protocol

For each flow, for each run (1..10), for each arm:

1. Send the question to Sonnet with the system prompt at `tests/roi/system-prompt.md`
   (versioned in git; harness records the file's SHA256 in the artifact).
2. **Baseline arm:** Sonnet has tools `Glob`, `Grep`, `Read` (no indexer).
3. **Indexer arm:** Sonnet has tool `Bash("unblock-code …")`.
4. Record total tokens (input + output) from the API `usage` block.
5. Record final answer; assert it matches the gold.
6. Compute per-run token ratio = baseline_tokens / indexer_tokens.

### 15.4 Reported metrics (release artifact)

The release artifact `target/roi-report.json` contains:

```json
{
  "model": "claude-sonnet-4-7-20260101",
  "system_prompt_sha256": "...",
  "harness_version": "1.0.0",
  "indexer_version": "1.0.0",
  "flows": [
    {
      "flow": "A",
      "runs": 10,
      "median_ratio": 3.7,
      "iqr": [3.2, 4.1],
      "raw_runs": [
        {"run":1,"baseline_tokens":1234,"indexer_tokens":340,"ratio":3.63,"answer_match":true},
        …
      ]
    },
    { "flow": "B", … },
    { "flow": "C", … }
  ],
  "global_median_indexer_runs": 2.6
}
```

### 15.5 Aspirationals (informational, not gates)

| Flow                                | Aspirational |
| ----------------------------------- | -----------: |
| A (`find-symbol`)                   | ≥ 3.5×       |
| B (`outline`)                       | ≥ 2.5×       |
| C (`find-references` HEURISTIC)     | ≥ 1.8×       |
| Global median across 30 indexer runs| ≥ 2.5×       |

### 15.6 Release-gate vs CI

- Harness runs as **release-gate one-shot**: executed manually before tagging
  v1.0.0; report attached to the GitHub Release.
- **NOT** in CI per PR (Anthropic API cost $5–30/run + Sonnet non-determinism).
- SOFT 2.0× threshold (S5 / L22): if global median < 2.0× on any flow, open a
  `unblock:finding:risk` follow-up bead linked to this phase; phase close is
  **not** blocked.

---

## 16. Testing Strategy

### 16.1 Unit tests (in `unblock-indexer-core`)

- **`SymbolKind` round-trip** — `from_wire(as_str(*)) == Some(*)` for all 17.
- **`Language` round-trip** — same for all 10.
- **`Span::contains`** — proptest invariants (transitivity, anti-symmetry).
- **`parent_id` algorithm** — proptest: random non-overlapping intervals,
  random nested intervals, deeply nested, empty.
- **DDL constants** — every `CREATE_*` parses successfully under
  `sqlparser-rs` in a smoke test (catches typos at unit-test time).
- **`SymbolId` Display/FromStr/Serde** round-trip.

### 16.2 Integration tests (in `unblock-indexer`)

- **FTS5 PRAGMA assertion** — open a SQLite without FTS5 (mocked via a
  custom `vfs`); assert `IndexerError::Fts5Missing` and exit-mapping yields 6.
- **Bootstrap correctness** — small fixture repo (5 files, 3 langs); assert
  total symbols = expected count; assert FTS5 search returns expected matches.
- **Trigger fan-out** — manual UPDATE on a symbol; assert FTS5 row reflects
  new values (verifies `'delete'`-then-insert path).
- **`parent_id` post-insert** — fixture file with nested fn/struct/method;
  assert `parent_id` chain matches expected tree.
- **Per-query mtime check (H8)** — index a fixture; mutate a file's content
  + bump mtime; run `find-symbol` for a newly-added symbol; assert match
  appears WITHOUT explicit `reindex`.
- **Walker default-excludes** — fixture with `target/`, `node_modules/`,
  `.git/` populated; assert none indexed; with `force_include = ["target/dist/**"]`,
  assert `target/dist/foo.rs` IS indexed.
- **Subtree reindex (Q-S-3)** — bootstrap; modify N files in `crates/foo/`;
  run `reindex crates/foo/`; assert other crates' symbol rowids unchanged
  (subtree reindex is scoped).
- **`walker.max_file_bytes` config knob (§8.3, §13.1)** — fixture with a 3 MiB
  file: (a) default config skips it (debug event captured); (b) `max_file_bytes
  = 5242880` includes it; (c) invalid value (`"abc"` or `0`) emits a
  `tracing::warn!`, falls back to default, walker continues without aborting.
- **No `meta.toml` in cache (§6.2, Invariant §17)** — assert a successful
  `init` produces only `index.db`, `index.db-wal`, `index.db-shm` under the
  repo cache directory; no `*.toml` file is created.

### 16.3 CLI-level tests (in `unblock-code`)

- **Envelope schema validation** — for each of 11 commands, run on a fixture
  and validate the stdout against `tests/fixtures/envelopes/<command>.schema.json`.
- **Exit-code mapping** — for each `IndexerError` family, induce the error
  and assert exit code matches §12.3.
- **`find-references` HEURISTIC lint (Q-S-6)** — `--help` test asserts the
  literal substrings; envelope test asserts the `warning` string verbatim.
- **`symbol_id` opaqueness (Q-S-7)** — `--help` test asserts the
  `symbol_id is opaque; do not parse.` substring on `find-symbol`,
  `get-symbol`, `list-symbols`, `outline`, `search`.
- **`get-symbol --body` truncation (Q-S-4)** — fixture symbol with > 64 KiB
  body; assert truncation marker `\n…\n` present and `body_truncated: true`.
- **`get-symbol --body` path-traversal guard** — synthetic symbol with
  `file = "../etc/passwd"`; assert `IndexerError::PathEscape` (exit 2).
- **`search` sanitisation (Q-S-5)** — table of inputs (`prefix:`, `name:foo`,
  `"unbalanced`, `foo*bar`); assert `InvalidFtsQuery` for forbidden, sanitised
  query for allowed.
- **`stale: true` propagation** — fixture > 16 implicated files for
  `list-symbols --recursive`; assert `stale: true`.
- **`--pretty` flag (D3)** — assert indented output; `--pretty=false`
  (default) is compact.
- **STDOUT/STDERR separation (L16)** — assert stdout contains exactly one JSON
  envelope; `tracing` JSON Lines on stderr only.

### 16.4 Property tests (proptest)

- **`SymbolKind` exhaustiveness** — for any `s: String`, `from_wire(s)` is
  either None or roundtrips.
- **`Span` arithmetic** — proptest invariants on `contains`, `len`.
- **`parent_id` algorithm** — random sets of intervals; assert each symbol's
  `parent` is the smallest enclosing.
- **FTS5 sanitisation** — input → sanitised → SQLite never errors (test via a
  real in-memory FTS5 table).
- **Comment stripping idempotency (§9.7.4)** — for each `strip_<family>` helper
  in `unblock-indexer-core::comment`, proptest asserts
  `strip(strip(s)) == strip(s)` over arbitrary UTF-8 input.
- **Comment stripping marker absence** — for each helper, proptest asserts the
  output contains none of the leader markers the helper is responsible for
  (`///`, `//!`, `/**`, `*/`, `# `, `"""`, `'''`, `=begin`, `=end` as
  applicable).

### 16.5 Grammar smoke tests

For each language with its feature enabled, a `tests/grammars/<lang>.rs`
contains:

1. Parse a 50-line fixture file → assert no `tree.root_node().has_error()`.
2. Run the language's `Query::new(query_source)` → assert no `QueryError`.
3. Run `traverse` → assert at least one symbol of each expected `SymbolKind`
   for the language.

The smoke-test fixtures live at `tests/grammars/fixtures/<lang>.<ext>`.

### 16.6 CI matrix

```
- cargo test --workspace                                            # all features
- cargo test -p unblock-code --no-default-features --features lang-rust  # single-lang
- cargo test -p unblock-code --no-default-features --features lang-rust,lang-python  # paired
- cargo test -p unblock-code --features lang-cpp,lang-ruby           # exhaustive 10-lang (H3)
- cargo build -p unblock-code --release --target x86_64-unknown-linux-gnu  # H2
- cargo build -p unblock-code --release --target aarch64-apple-darwin       # H2
- cargo build -p unblock-code --release --target x86_64-pc-windows-msvc     # H2
```

---

## 17. Invariants

The following properties are non-negotiable; any code change that violates one
is a defect requiring revert.

1. **No body in DB.** `symbols` schema has NO `body` column. `--body` reads from
   filesystem at query time. (L8, §5.3.)
2. **Filesystem is canonical.** The DB index is a CACHE. On any contradiction
   between filesystem and DB, the filesystem wins (per-query mtime check resolves).
3. **`unblock-indexer-core` is pure.** Zero IO, zero async, zero `tokio`, zero
   `sqlx`, zero `ignore`, zero `tree-sitter-<lang>` grammar deps. (§3.1.)
4. **Per-query mtime check is mandatory and the SOLE sync mechanism.** No
   watcher, no daemon, no in-memory cache between invocations. (L9, §10.2.)
5. **WAL chunked transactions.** Bootstrap writes commit at ~500 rows / `BEGIN..COMMIT`.
   Never hold a transaction across the entire bootstrap. (R4, §5.6.)
6. **FTS5 must be present.** PRAGMA assertion fires on every connection acquire;
   absence is exit 6. (R4, §5.2.)
7. **STDOUT is reserved for the JSON envelope.** All `tracing`, all human
   progress, all warnings go to STDERR. (L16, D1, D2.)
8. **`find-references` warning lint is a build-time gate.** The literal strings
   `HEURISTIC` and `syntactic only, no type resolution` MUST appear in `--help`
   AND in the envelope's `warning` field. (Q-S-6, §11.7.)
9. **`symbol_id` is opaque.** Wire format is a decimal string. Clients MUST NOT
   parse. The `--help` text declares this on every relevant command. (D4, Q-S-7.)
10. **`force_include` cannot bypass default-excludes wholesale.** The Override
    mechanism is first-match-wins; user inclusion can override a specific
    exclusion, but there is no "disable defaults" knob. (R7, §8.4.)
11. **Walker uses `require_git(false)`.** Mandatory for non-git checkouts.
    (R7, §8.1.)
12. **No HTTP at runtime.** `unblock-indexer` does NOT depend on
    `unblock-resilience`; no network grammar fetcher; no integrity manifest
    downloads. (Plan §3, §9.)
13. **MIT-only; no third-party attribution file.** `tags.scm` license headers
    are preserved verbatim inside the .scm files; no aggregated NOTICE/THIRD_PARTY.
    (L19, §3.4.)
14. **Path traversal guard.** All filesystem reads (`get-symbol --body`, walker,
    cache writes) canonicalise against the repo root or the cache root and
    reject escapes. (§11.6, §6.4.)
15. **`#[non_exhaustive]` on all growable enums.** `SymbolKind`, `Language`,
    `CoreError`, `IndexerError`, `CliError`. (CLAUDE.md "Coding Standards", L15.)
16. **Schema-mismatch wipe is observable.** A `tracing::warn!` event with target
    `indexer.schema` and message `schema_mismatch_wipe` fires on every wipe.
    `status` reports the most recent wipe. (§5.5, R6 mitigation.)
17. **Comment text is stripped per language family.** `symbols.comment` stores
    the stripped text per §9.7; markers (`///`, `/**`, `# `, `"""`, `=begin`,
    etc.) MUST NOT appear in the column. FTS5 search results show clean text in
    snippets. The `strip_<family>` helpers are pure and idempotent (§9.7.4).
    (§9.7, §16.4.)
18. **Cache metadata lives exclusively in `index.db`'s `meta` table.** No
    auxiliary metadata files (`meta.toml`, JSON sidecar, lockfile) exist in the
    cache directory; the on-disk layout is `index.db` + WAL/SHM only.
    `status`, `init`'s "already initialised" probe, and the schema-mismatch
    comparison all read SQLite. (§6.2, §6.3, §10.5.)

---

## 18. Open Items & Forward References

Items deliberately deferred from v1.0.0; each is **out of scope** for this
spec but listed for traceability so future phases inherit the context.

### 18.1 Deferred to v1.0.x

- **Offline grammar bundle** — pre-compiled grammar archives that remove the
  `cc` build prerequisite. Requires Phase 04 cargo-dist work; tracking in
  Phase 04 plan.
- **C# / Swift / Dart support** — additional grammars; deferred per L10. New
  Cargo features `lang-csharp`, `lang-swift`, `lang-dart` would slot into the
  registry.

### 18.2 Deferred to v1.1.0+

- **Daemon mode** — per L1 / L14 explicitly out of scope. Only revisit if the
  cold-start budget (H6) cannot be met with full Top-10 grammars loaded.
- **File watcher revival** — same trigger as daemon mode.
- **WASM grammar runtime revival** — only if (a) static-linked binary size
  grows beyond practicality (S1 sustained breach), or (b) demand for runtime
  pluggability materialises.
- **Cross-file semantic resolution** — type inference, real call graph. `find-references`
  remains HEURISTIC in v1.0.x; semantic resolution is a separate phase with
  its own PRD.
- **Issue/code correlation queries** between `unblock-mcp` (issue tracker) and
  `unblock-code` (code indexer). Two-binary architecture intentionally
  separates them in v1.0.0.
- **Editor MCP transport for `unblock-code`** — re-evaluate after agent
  ecosystem demand signal.

### 18.3 Spec-time "watch list" (informational)

- **R-CLI-1.1** — sqlx WAL recovery on first open after dirty crash can spike
  100–500 ms. H6 budget assumes clean shutdown. If observed in production,
  open a finding bead.
- **R-CLI-2.1** — Stripped-binary measurements may diverge from the modelled
  0.4× factor by 20–30%. S1 ceiling (30 MB default) is the line; if the
  default build crosses 35 MB, evaluate pruning a default grammar.
- **R-CLI-2.2** — C++ alone could push the binary past 50 MB if the 0.4× factor
  underestimates. `lang-cpp` is opt-in (mitigation), but the exhaustive 10-lang
  CI build could grow uncomfortably; track size on each release.
- **R3.2** — Future tree-sitter 0.27 may bump `MIN_COMPATIBLE_LANGUAGE_VERSION`
  to 14. None of the Top-10 are ABI 13 today. Worth a CI canary that builds
  against an unreleased tree-sitter prerelease.
- **S3 drift** — vendored `tags.scm` may diverge from upstream over time;
  CI drift check is informational. Re-vendor on each release.

---

**Status: APPROVED (2026-04-29).** Amended post-review (SO-2 / SO-3 / SO-4 — see
frontmatter amendment marker). `/tasks` may now consume this spec to generate
the implementation beads against the seven epics in
`docs/plans/03-plan-code-indexer.md` §12.
