# Phase 03 Plan — Code Indexer CLI

> Phase: 03
> Author: Ada (architect)
> Date: 2026-04-29
> Status: **APPROVED** (amended 2026-04-29 post-research)
> Source PRD: [PRD §7 Phase 03](../PRD.md#phase-03--code-indexer-cli-v100--03-plan-code-indexermd-re-authoring-after-2026-04-29-reframe)
> Source SPEC: [SPEC §6.5 Code Indexer CLI](../SPEC.md#65-code-indexer-cli-phase-03)
> Companion research: [docs/research/03-research-code-indexer.md](../research/03-research-code-indexer.md) (PRE-REFRAME — partial obsolescence; survival map in §11)
> Companion spec: `docs/specs/03-spec-code-indexer.md` (re-authoring after 2026-04-29 reframe)
>
> **Amended 2026-04-29 post-research:** H2 reworded (research R-CLI-4 confirmed `cc` toolchain is unavoidable — every upstream tree-sitter-`<lang>` crate AND `libsqlite3-sys/bundled` require `cc` at build time); `lang-cpp` and `lang-ruby` moved from default features to opt-in (research R-CLI-2 + Q-R-CLI-2.1 — defaults reduce to 8 languages, target stripped binary ≤ 30 MB); L20 expanded with `list-symbols` p99 < 50 ms and `search` p99 < 30 ms warm budgets (Q-R8.1); ROI harness clarified as release-gate one-shot, not per-PR CI (Q-R-CLI-5.1); Epic 03.4 query-rule scope corrected to ~30–40 rules across 10 languages, +~1 week vs initial estimate (per R5).

---

## Note on the 2026-04-29 reframe

This plan is a **re-authoring**. The original 03-plan-code-indexer.md targeted **MCP tools** for code analysis and was deleted on 2026-04-29 (commit `a77757e`) when the phase was reframed to a separate **CLI binary** (`unblock-code`) with statically-linked tree-sitter grammars. The PRD §7 Phase 03 stub and SPEC §6.5 already encode the new product surface verbatim; this plan layers epic structure, research gaps, and acceptance criteria on top.

The reframe also rolled back the `unblock-resilience` consumer in `unblock-indexer` (originally introduced in Phase 02 to host the future grammar fetcher). With WASM deferred, there is no HTTP fetcher in v1.0.0; `unblock-indexer` does not depend on `unblock-resilience`. See [02-plan-mcp-complete §11.6](./02-plan-mcp-complete.md) and [02-spec-mcp-complete §17.1](../specs/02-spec-mcp-complete.md) for the rollback record.

---

## 1. Purpose

Save tokens for AI agents. Instead of an agent burning context on `Glob` + `Grep` + `Read` to locate symbols, definitions, and code structure, a sibling CLI binary `unblock-code` answers structured questions ("where is X / what does Y export / show me Z") via fast `Bash("unblock-code …")` invocations backed by a local SQLite + FTS5 index.

The deliverable is **distinct from the issue-tracker MCP**: stateless one-shot, local-only, universal — any Bash-capable agent works without MCP. v1.0.0 ships **two binaries**: `unblock-mcp` (issue tracker, unchanged from Phase 02) and `unblock-code` (code analysis, new).

## 2. Scope (in)

- Two new lib crates and one new bin crate, added to the workspace as described in PRD §6.1 Phase 03.
- 11 CLI commands (catalogue in §6) — read-only against a local SQLite + FTS5 index plus two write commands (`reindex`, `init`) that mutate only the local cache, never the working tree.
- Top-10 statically-linked tree-sitter grammars: Rust, TypeScript, JavaScript, Python, Go, Java, C, C++, Ruby, PHP — exposed via Cargo feature flags. **Defaults: 8 languages** (Rust, TypeScript, JavaScript, Python, Go, Java, C, PHP); `lang-cpp` and `lang-ruby` are opt-in (per L4 / L10, research R-CLI-2 + Q-R-CLI-2.1).
- 17 canonical `SymbolKind` variants persisted in SQLite with FTS5 over `name`, `signature`, `comment`.
- Per-query mtime check as the **sole** sync mechanism between one-shot CLI invocations (invariant, not optimisation).
- ROI harness measuring `Bash("unblock-code …")` against a `Glob/Grep/Read` baseline, gating release on the 2.0× hard threshold.

## 3. Scope (out — explicit non-goals for v1.0.0)

The following are **deferred** or **rejected** for v1.0.0. Each is a hard line; flipping any of them is a phase-replan event, not a bead-level decision.

- **Cross-file semantic resolution**, type inference, real call graph. `find-references` is **HEURISTIC** syntactic-only; the JSON envelope marks it explicitly.
- **Analytics**: dead-code, cyclomatic complexity, similarity, redundancy / 102-style checks. Out of charter — `unblock-code` is a query surface, not a linter.
- **WASM grammar runtime**, runtime fetcher, integrity manifest. Static-linked is the v1.0.0 model. Revisit only if (a) binary size grows beyond practicality, or (b) demand for runtime pluggability materialises.
- **Daemon mode** and **file watcher**. v1.0.0 is one-shot; per-query mtime check is sufficient.
- **Editor MCP registration**. The CLI does not register with editors. No MCP transport, no editor config schema.
- **Network grammar fetcher**. No HTTP at runtime. `unblock-indexer` does not depend on `unblock-resilience`.
- **Issue/code correlation queries** between `unblock-mcp` (issue tracker) and `unblock-code` (code indexer). The two binaries do not share runtime state.
- **C# / Swift / Dart**. Deferred to v1.0.x via a follow-up bead once Top-10 stabilises.

## 4. Locked Architectural Decisions

The following 22 decisions are **locked** at plan-approval time. They are reflected verbatim from the PRD/SPEC patches and from prior decision logs. Re-litigation requires a phase-replan event.

### Product / architecture
- **L1.** CLI binary `unblock-code`, one-shot stateless. NO MCP, NO editor registration, NO watcher, NO daemon mode in v1.0.0.
- **L2.** Three new crates: `unblock-indexer-core` (pure lib), `unblock-indexer` (impure lib), `unblock-code` (bin, clap-based).
- **L3.** Seven crates in the workspace post-Phase 03; `unblock-mcp` is untouched.

### Parser / grammars
- **L4.** `tree-sitter` Rust crate + `tree-sitter-<lang>` upstream crates **statically linked** via Cargo feature flags. **8 grammars default-enabled** (Rust, TypeScript, JavaScript, Python, Go, Java, C, PHP); `lang-cpp` and `lang-ruby` are **opt-in** via `cargo install unblock-code --features lang-cpp,lang-ruby` (research R-CLI-2 + Q-R-CLI-2.1: dropping these two trims ~16 MB stripped, landing default ≤ 30 MB instead of 40–50 MB).
- **L5.** **Fresh implementation (Option C).** Architectural inspiration from external prior art is allowed; **zero code copied**; **no attribution** in NOTICE/THIRD_PARTY. Vendor each language's `tags.scm` directly from the upstream tree-sitter repository.
- **L6.** `build.rs` compiles the statically-linked grammars at build time (mechanics validated in research R-CLI-4).

### Storage
- **L7.** `sqlx` + SQLite + FTS5 + WAL. Schema: 17 canonical kinds + Span + `parent_id` + `comment` column for FTS5 (preserves prior Resolution Q4). The 17 kinds are: `function, method, class, struct, enum, interface, trait, module, namespace, variable, constant, type_alias, macro, field, property, import, export` — matching PRD §7 verbatim. Earlier "16 kinds" wording in the input brief was a typo; corrected here at plan APPROVED time.
- **L8.** Cache at `~/.cache/unblock/repos/<repo-hash>/index.db`. **Span-only** — no body text in DB.
- **L9.** Per-query mtime check is the **sole** sync mechanism between CLI invocations (invariant). Implicated-file rule from prior spec §16.2 carries over.

### Languages
- **L10.** v1.0.0 Top-10 supported: Rust, TypeScript, JavaScript, Python, Go, Java, C, C++, Ruby, PHP. **Default install indexes 8** (Rust, TypeScript, JavaScript, Python, Go, Java, C, PHP); `lang-cpp` and `lang-ruby` are **opt-in** Cargo features (per L4, research R-CLI-2 + Q-R-CLI-2.1). The exhaustive 10-language test matrix in CI explicitly enables both opt-in features. C# / Swift / Dart deferred to v1.0.x via follow-up bead.

### CLI surface (11 commands)
- **L11.** `find-symbol`, `list-symbols`, `outline`, `get-symbol`, `search`, `find-references` (HEURISTIC), `reindex`, `status`, `languages`, `init`, `parse`.

### Out-of-scope (re-stated as locked nons)
- **L12.** Analytics: dead-code, cyclomatic complexity, similarity, redundancy / 102-checks.
- **L13.** Cross-file semantic resolution / type inference / real call graph. `find-references` is HEURISTIC syntactic-only.
- **L14.** WASM runtime, daemon mode, file watcher, editor MCP registration, MCP tools, network grammar fetcher.

### Conventions
- **L15.** `snafu` errors + crate-scoped `Result<T>`; no `unwrap()` / `expect()` outside tests; `#[non_exhaustive]` on growable public enums per CLAUDE.md "Coding Standards".
- **L16.** `tracing` JSON Lines on **STDERR**; **STDOUT** reserved for JSON output (invariant — never mix progress and results).
- **L17.** Exit codes by error family: `0` success, `2` invalid input, `3` unsupported language, `4` grammar/network, `5` db, `6` fts5, `7` parse, `99` internal.
- **L18.** `#![deny(unsafe_code)]` workspace-wide.
- **L19.** MIT in all three new crates. NO NOTICE / THIRD_PARTY file referring to external prior art.

### Performance gates v1.0.0
- **L20.** Warm-path p99 budgets on the Medium representative corpus (per research R8 / Q-R8.1):
    - `find-symbol` p99 < **10 ms**.
    - `outline` p99 < **20 ms**.
    - `list-symbols` (recursive, 16-implicated-file cap) p99 < **50 ms**.
    - `search` (FTS5 query) p99 < **30 ms**.
    - `find-references` — **no hard budget** (HEURISTIC, informational only).
  All four pinned budgets are HARD acceptance criteria via H6 (§14.1).
- **L21.** CLI cold-start budget (process spawn → first JSON byte on stdout); concrete number set by research R-CLI-1.
- **L22.** ROI **SOFT** gate — report only. Harness measures ≥ 2.0× median vs Glob/Grep/Read across 3 representative agent flows × N=10 runs each, indexer arm uses `Bash("unblock-code …")`, Sonnet via the Anthropic API with a Claude-Code-like system prompt. If median < 2.0× across the 3 flows × N=10 runs, open a `unblock:finding:risk` follow-up bead but do **not** block phase close. Rationale: harness implementation cost (1–2 weeks Rust dev), Anthropic API cost ($5–30/run), risk of false PASS via harness bug, risk of phase ship-block on harness defects rather than indexer correctness. Numbers are still measured and published; gating is non-blocking.

## 5. Crate Architecture

```
crates/unblock-indexer-core/       ← NEW lib (pure)
├── src/
│   ├── lib.rs                      ← public surface, crate-scoped Result
│   ├── errors.rs                   ← snafu, #[non_exhaustive]
│   ├── kinds.rs                    ← 16 canonical SymbolKind
│   ├── span.rs                     ← Span (1-based byte offsets)
│   ├── symbol.rs                   ← Symbol (DTO)
│   ├── ast/                        ← AST traversal over tree_sitter::Tree
│   │   ├── mod.rs
│   │   └── visitor.rs
│   ├── queries.rs                  ← compiled S-expression query handles
│   └── schema.rs                   ← DDL constants (CREATE TABLE / VIRTUAL TABLE / triggers)

crates/unblock-indexer/             ← NEW lib (impure)
├── build.rs                        ← compiles statically-linked grammars (R-CLI-4)
├── src/
│   ├── lib.rs
│   ├── errors.rs
│   ├── grammars/                   ← language registry, feature-gated
│   │   ├── mod.rs                  ← Language enum, registry::loaders()
│   │   ├── rust.rs    (cfg=lang-rust)
│   │   ├── typescript.rs (cfg=lang-typescript)
│   │   └── …                       ← one file per Top-10
│   ├── tags/                       ← vendored upstream tags.scm + 4 hand-written extensions
│   ├── walker.rs                   ← `ignore` crate walk, mtime probe
│   ├── parse.rs                    ← tree-sitter parse + AST traversal driver
│   ├── store.rs                    ← sqlx pool, schema migrations, FTS5 PRAGMA assertion
│   ├── bootstrap.rs                ← rayon-driven full reindex, chunked transactions
│   └── query.rs                    ← read-side queries (find-symbol, list-symbols, outline…)

crates/unblock-code/                ← NEW bin (clap-based)
├── src/
│   ├── main.rs
│   ├── errors.rs                   ← exit-code mapping (L17)
│   ├── cli.rs                      ← clap definitions for 11 subcommands
│   ├── envelope.rs                 ← JSON envelope serialisation (D1–D4)
│   ├── tracing.rs                  ← JSON Lines on stderr (L16)
│   └── commands/                   ← one module per command in L11
│       ├── find_symbol.rs
│       ├── list_symbols.rs
│       ├── outline.rs
│       ├── get_symbol.rs
│       ├── search.rs
│       ├── find_references.rs
│       ├── reindex.rs
│       ├── status.rs
│       ├── languages.rs
│       ├── init.rs
│       └── parse.rs
```

`unblock-indexer-core` has zero IO, zero async, zero `tokio`. It owns the schema constants and the AST visitor that converts a `tree_sitter::Tree` into a stream of `Symbol` rows. `unblock-indexer` is the only crate that touches the filesystem, sqlite, or grammar registries. `unblock-code` is the only crate that touches stdout/stderr or `clap`.

## 6. Storage

| Aspect | Decision |
|---|---|
| Engine | SQLite via `sqlx` (sqlite feature) |
| Concurrency | WAL mode (`PRAGMA journal_mode=WAL`) |
| FTS5 | External-content virtual table over (`name`, `signature`, `comment`); `'rebuild'` after full reindex |
| FTS5 verification | At connection open — `PRAGMA compile_options;` must report `ENABLE_FTS5`; otherwise hard error (exit code 6) |
| Schema | 16 `SymbolKind` enum (text), Span (4 ints), `parent_id` (nullable rowid), `comment` (text) |
| Body text | **Not stored.** Span-only. `get-symbol --body` reads from filesystem on demand |
| Cache root | `~/.cache/unblock/repos/<repo-hash>/index.db` |
| Repo hash | SHA-256(absolute repo root path) — local-only, no git remote dependency |
| Sync mechanism | Per-query mtime check (sole; invariant) — implicated-file rule carries from prior spec §16.2 |
| Schema versioning | `schema_version` constant in `unblock-indexer-core`; on mismatch the index is wiped and rebuilt (pre-prod stance — no migrations) |

## 7. CLI Surface

### 7.1 JSON envelope conventions (D1–D4)

- **D1.** One JSON envelope per invocation on **stdout**. No NDJSON in v1.0.0.
- **D2.** Errors emitted on stdout as `{"error":{"code":"…","message":"…","details":{…}}}`. Exit code by family (L17) signals additionally. Stderr carries `tracing` JSON Lines + human progress.
- **D3.** Minified JSON by default; `--pretty` flag for human reading.
- **D4.** `symbol_id` is an **opaque string** (SQLite rowid encoded as text internally). Clients MUST NOT parse — documented in `--help`.
- **Field conventions.** snake_case fields. `span = {start_line, start_col, end_line, end_col}` 1-based. `stale: true` when per-query mtime check capped re-parse. `truncated: true` when `--limit` cut results. `heuristic: true` + `warning` only on `find-references`.

### 7.2 Per-command envelope schemas (sketch — locked)

| Command | Envelope |
|---|---|
| `find-symbol` | `{tool, matches:[{symbol_id, name, kind, language, file, span, signature}], total_matches, stale, truncated, duration_ms}` |
| `list-symbols` | `{tool, file, symbols:[{symbol_id, name, kind, span, parent_id}], stale, duration_ms}` |
| `outline` | `{tool, file, language, tree:[OutlineNode{symbol_id, name, kind, span, children:[…]}], duration_ms}` |
| `get-symbol` | `{tool, symbol_id, name, kind, language, file, span, signature, comment, parent_id, body}` |
| `search` | `{tool, query, matches:[{symbol_id, name, kind, file, span, snippet}], total_matches, duration_ms}` |
| `find-references` | `{tool, heuristic:true, warning:"syntactic only, no type resolution", references:[{file, span, surrounding_symbol}]}` |
| `languages` | `{tool, languages:[{language, file_count, symbol_count}]}` — only languages whose Cargo feature is enabled in the running binary appear. Default install lists 8 (no `cpp`, no `ruby`); installs with `--features lang-cpp,lang-ruby` list 10. |
| `status` | `{tool, repo_root, schema_version, indexer_version, last_full_index, last_incremental, total_files, total_symbols, db_size_bytes}` |
| `reindex` | `{tool, files_reparsed, symbols_emitted, duration_ms}` |
| `init` | `{tool, repo_root, languages_detected:[…], files_indexed, symbols_emitted, duration_ms}` |
| `parse` | `{tool, file, language, tree}` — S-expression string by default; `--json-tree` flag emits a verbose tree-as-JSON |

`tool` is always the literal command name (`"find-symbol"`, etc.). The exact field types and required-vs-optional matrix are pinned in the spec.

## 8. Lifecycle (no watcher)

```
$ unblock-code <command> [args]
   ├─ open ~/.cache/unblock/repos/<repo-hash>/index.db (sqlx, WAL)
   │     └─ if missing: lazy bootstrap (walk + parse + insert) per implicated-file rule
   ├─ per-query mtime check on each implicated file
   │     └─ if mtime newer than indexed_at: re-parse + replace symbols for that file
   ├─ run query
   ├─ emit JSON envelope on stdout
   └─ exit (process gone)
```

Forced re-sync is `unblock-code reindex [path]`. Bootstrap on a fresh repo is `unblock-code init` (explicit alternative to lazy first-query bootstrap). There is no daemon, no shared memory, no state between invocations beyond the SQLite file.

## 9. External Dependencies

| Crate | Purpose | New / existing |
|---|---|---|
| `tree-sitter` | Parser core | New |
| `tree-sitter-rust`, `…-typescript`, `…-javascript`, `…-python`, `…-go`, `…-java`, `…-c`, `…-php` | Static-linked grammars — **8 default** (`lang-rust`, `lang-typescript`, `lang-javascript`, `lang-python`, `lang-go`, `lang-java`, `lang-c`, `lang-php`) | New (8 crates default) |
| `tree-sitter-cpp`, `tree-sitter-ruby` | Static-linked grammars — **opt-in only** (`lang-cpp`, `lang-ruby`); ~16 MB stripped combined per R-CLI-2 | New (2 crates, opt-in) |
| `sqlx` (sqlite feature, runtime-tokio-rustls) | SQLite + FTS5 client | New |
| `ignore` | gitignore-aware walker | New |
| `rayon` | Parallel bootstrap | New |
| `clap` (derive) | CLI parser | New |
| `serde` / `serde_json` | Envelope (de)serialisation | Existing |
| `snafu` | Error types | Existing |
| `tracing` / `tracing-subscriber` | JSON Lines on stderr | Existing |
| `tokio` | Runtime for sqlx | Existing |

`unblock-indexer` deliberately does **not** depend on `unblock-resilience` — there is no HTTP fetcher in v1.0.0.

## 10. Lifecycle dependencies on previous phases

- **Phase 01 / 02 invariants are not consumed.** `unblock-mcp` is untouched. `unblock-core`, `unblock-github`, `unblock-resilience` see zero changes.
- The `DriftKind::StaleStatus` and `#[non_exhaustive]` discipline established in Phase 02 (per CLAUDE.md "Coding Standards") **carry forward**: every growable public enum in the new crates (e.g. `SymbolKind`, error enums, `Language`) carries `#[non_exhaustive]`.
- The pre-production stance from Phase 02 (no migrations, breaking changes OK) **applies**: schema mismatches wipe and rebuild the local cache.

## 11. Research Gaps for `/research 03`

This plan **must not be promoted to a spec** until research has validated or contradicted each gap below. The plan deliberately leaves quantitative thresholds open where research is required.

### 11.1 Surviving from PRE-REFRAME research (auto-import)

The 2026-04-29 reframe note in `docs/research/03-research-code-indexer.md` already maps which sections survive. They are imported verbatim into the new research file:

- **R3 — Top-10 grammar audit.** Re-validate freshness as of 2026-04-29; pin per-grammar version + tree-sitter ABI version per language. ABI fragmentation across 14/15 was flagged.
- **R4 — sqlx + FTS5.** PRAGMA `compile_options` assertion at connection open; external-content `'delete'`-then-insert triggers; chunked-transaction bootstrap.
- **R5 — Symbol-extraction queries.** Vendor upstream `tags.scm` per language; 4 hand-written extension queries (field / property / import / export — preserves Resolution Q5 from prior research). `parent_id` resolved post-traversal.
- **R7 — `ignore` crate edge cases.** `require_git(false)` footgun; `same_file_system`; `force_include` from `.unblock/indexer.toml` (preserves Resolution Q7).
- **R8 — Latency methodology.** Corpus tiers (small / medium / large), criterion + tokio harness, implicated-file rule. Targets in L20 carry over.

### 11.2 New investigation gaps (CLI-specific)

- **R-CLI-1 — CLI cold-start budget.** Bench process spawn + sqlx open + first JSON byte on stdout. Measure full-load (10 langs static-linked) and partial-load (e.g. `--features lang-rust,lang-python` only). Fix L21 to a concrete millisecond budget.
- **R-CLI-2 — Static-link binary size.** Per-language contribution (each `tree-sitter-<lang>` crate) and aggregate size. Comparators: ripgrep (~10 MB), fd (~5 MB). Define an acceptability threshold.
- **R-CLI-3 — Cargo feature-flag ergonomics.** Validate that `cargo install unblock-code --no-default-features --features lang-rust,lang-python` produces a smaller binary that still passes the test suite for the selected languages. Document the `Cargo.toml` shape for downstream packagers.
- **R-CLI-4 — `build.rs` mechanics.** **CONFIRMED post-research:** every upstream `tree-sitter-<lang>` crate ships a generated `parser.c` but compiles it via the `cc` build dependency at consumer build time. A host C toolchain (gcc/clang on Linux, Apple Clang via Xcode CLT on macOS, MSVC Build Tools on Windows) is therefore mandatory. This matches `libsqlite3-sys/bundled` (already transitive via sqlx), so the requirement adds zero new ceremony. The README's "Building from source" section documents the per-platform prerequisites; H2 (§14.1) reflects the corrected gate.
- **R-CLI-5 — R10 reframe (ROI harness).** Re-baseline the 3 aspirational flows (A/B/C from prior R10) under the new transport: indexer arm uses `Bash("unblock-code …")`. Sonnet via the Anthropic API with a Claude-Code-like system prompt. The 2.0× ratio is now a **SOFT** target (per L22 / S5) — measured and published, follow-up bead if missed, no ship-block. Absolute numbers are reset. **Q-R-CLI-5.1 (locked):** the ROI harness runs as a **release-gate one-shot** (executed manually before tagging v1.0.0), NOT in CI per PR. Per-PR runs are ruled out by Anthropic API cost ($5–30/run) and Sonnet output non-determinism.

### 11.3 Why this matters

Several **L-decisions** are deliberately quantitative-open: L20 (perf gates) is anchored but the corpus needs research re-run; L21 (cold-start budget) is fully open pending R-CLI-1; the binary-size ceiling is open pending R-CLI-2. Promoting to spec without these numbers risks shipping a binary the agent ecosystem rejects on cold-start or size grounds, or one whose ROI report (now SOFT per L22) lands embarrassingly below 2.0× because the corpus and flows were never grounded.

## 12. Epic Breakdown

The phase decomposes into six epics. Each epic produces beads whose `description` references this plan + the future spec; bead descriptions are never authoritative (per `feedback_bead_description_not_spec`).

### Epic 03.1 — Workspace + crate skeletons + error model
- Add three new crate manifests (`unblock-indexer-core`, `unblock-indexer`, `unblock-code`) to the workspace `Cargo.toml`.
- Wire `#![deny(unsafe_code)]` and the standard lint group across all three.
- Introduce crate-scoped `Result<T>` aliases via `snafu`.
- Skeleton `lib.rs` / `main.rs` with module declarations matching §5.
- CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc --no-deps`, `cargo test` all pass on the empty crates.

### Epic 03.2 — Grammars + `languages` command
- `build.rs` in `unblock-indexer` compiles the statically-linked grammars per L4 / L6.
- `Language` enum + Cargo feature flags (`lang-rust`, `lang-typescript`, …, `lang-cpp`, `lang-ruby`) — **8 default-enabled** (`lang-rust`, `lang-typescript`, `lang-javascript`, `lang-python`, `lang-go`, `lang-java`, `lang-c`, `lang-php`); **`lang-cpp` and `lang-ruby` are opt-in** (per L4 / L10, research R-CLI-2 + Q-R-CLI-2.1).
- `Cargo.toml` `[features]` table:
  - `default = ["lang-rust","lang-typescript","lang-javascript","lang-python","lang-go","lang-java","lang-c","lang-php"]`
  - `lang-cpp = ["dep:tree-sitter-cpp"]` and `lang-ruby = ["dep:tree-sitter-ruby"]` declared but excluded from `default`.
- Loader registry exposes a `tree_sitter::Language` per active language at runtime; cpp/ruby loaders compile only under their feature flags.
- `unblock-code languages` returns `{tool, languages: [{language, file_count: 0, symbol_count: 0}]}` (counts populated once Epic 03.5 lands) — default install lists 8 entries; `--features lang-cpp,lang-ruby` install lists 10.
- Acceptance: `cargo install unblock-code --no-default-features --features lang-rust` produces a working binary that exposes only Rust. Additionally, `cargo install unblock-code --features lang-cpp,lang-ruby` produces the exhaustive 10-language binary used in the CI test matrix.

### Epic 03.3 — Storage layer
- `sqlx` connection pool + WAL pragma + FTS5 `compile_options` assertion at open.
- DDL constants in `unblock-indexer-core::schema` — `CREATE TABLE symbols`, `CREATE VIRTUAL TABLE symbols_fts USING fts5(...)`, triggers for external-content sync.
- Schema-version constant; on mismatch, wipe and recreate (no migrations).
- Internal CLI subcommand `unblock-code status` returns repo metadata, schema version, totals.

### Epic 03.4 — AST traversal + symbol extraction
- 17 canonical `SymbolKind` variants in `unblock-indexer-core::kinds`: `function, method, class, struct, enum, interface, trait, module, namespace, variable, constant, type_alias, macro, field, property, import, export`. Matches PRD §7 Phase 03 verbatim. Locked at plan APPROVED time — no spec-level Q5 flag remains.
- Vendor upstream `tags.scm` per Top-10 language into `crates/unblock-indexer/src/tags/<lang>.scm`.
- Add hand-written extension queries (field / property / import / export categories) per prior Resolution Q5.

> **Sizing note (Q-R5.1 acknowledgment).** Per research R5, the "hand-written extension queries" scope is **per-language**, not 4 globals. Estimated **~30–40 query rules total** (4–6 categories × 10 languages because syntax differs: Rust `use` vs Python `import` vs Java `import` vs PHP `use` vs Ruby `require`, etc.). Epic 03.4 is sized accordingly (**+~1 week** vs the initial estimate that assumed 4 shared queries).
- AST visitor in `unblock-indexer-core::ast::visitor` consumes a `tree_sitter::Tree` + a query handle and yields `Symbol` rows.
- `parent_id` resolved post-traversal via a deterministic rowid scheme.
- `unblock-code parse <file>` returns the S-expression tree (default) or verbose JSON tree (`--json-tree`).

### Epic 03.5 — Walker + bootstrap + per-query mtime check
- `walker.rs` wraps `ignore::WalkBuilder` with `require_git(false)`, `same_file_system(true)`, and the `force_include` list from `.unblock/indexer.toml` (Resolution Q7).
- `bootstrap.rs` runs the walker, parses each file in parallel via `rayon`, emits symbols in chunked transactions per the prior plan §5.5.
- Per-query mtime check is implemented as the **sole** sync mechanism — no watcher, no debouncer.
- `unblock-code init` and `unblock-code reindex` expose the bootstrap path.

### Epic 03.6 — CLI surface (read-side commands) + JSON envelope + ROI harness
- Implement the 7 read-side commands per L11: `find-symbol`, `list-symbols`, `outline`, `get-symbol`, `search`, `find-references`, `status`.
- Wire envelope serialisation per §7 (D1–D4).
- `--pretty` flag honoured across all commands.
- `find-references` emits `heuristic: true` + the explicit warning string.
- ROI harness per L22 (SOFT gate S5): 3 representative agent flows × N=10 runs each, baseline = `Glob/Grep/Read`, indexer = `Bash("unblock-code …")`. Sonnet via Anthropic API. Numbers are measured and published as a release artifact; if median < 2.0× on any flow, open a `unblock:finding:risk` follow-up bead — do **not** block phase close. The ROI harness runs as a **release-gate one-shot** (executed manually before tagging v1.0.0), NOT in CI per PR. Per-PR runs are ruled out by Anthropic API cost ($5–30/run) and Sonnet output non-determinism (Q-R-CLI-5.1). Harness code must still land **before** the perf-tuning beads in 03.6 so the ratio is observable on demand throughout the epic, even though it no longer gates release and does not run in PR CI.

## 13. Task Dependencies

```
03.1 (workspace + skeletons)
  ├─→ 03.2 (grammars + languages)
  └─→ 03.3 (storage)

03.2 + 03.3
  └─→ 03.4 (AST traversal + symbol extraction)

03.4
  └─→ 03.5 (walker + bootstrap + mtime check)

03.5
  └─→ 03.6 (CLI surface + ROI harness)
```

Epic 03.6 cannot start before 03.5 lands the bootstrap path. Epic 03.4 must wait for both 03.2 (grammars exposed) and 03.3 (DDL constants stable).

Beads inside an epic decide their internal questions and close them inline (per `feedback_epic_decision_closure`); review findings attach to the parent epic, not a separate "Review Findings" epic (per `feedback_findings_epic_parent`).

## 14. Acceptance Criteria

Acceptance splits into **9 HARD** gates (release-blocking) and **5 SOFT** gates (warnings logged / follow-up beads, do not block release).

### 14.1 HARD gates (release-blocking) — 9 total

- **H1.** `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc --no-deps`, `cargo test` pass across the workspace.
- **H2.** `unblock-code` binary builds with default features on Linux x86_64 (gcc/clang), macOS aarch64 (Apple Clang via Xcode CLT), and Windows x86_64 (MSVC Build Tools). The README documents the platform-specific C-toolchain prerequisites in a "Building from source" section. Note: this requirement matches `libsqlite3-sys/bundled` (already a transitive dep via sqlx) — `cc` is not new ceremony. Research R-CLI-4 confirmed every upstream `tree-sitter-<lang>` crate compiles `parser.c` via the `cc` build dependency, so a host C toolchain is unavoidable. (Default-feature scope is 8 languages per AMEND 2 — `lang-cpp` and `lang-ruby` are opt-in.)
- **H3.** Top-10 languages parse a mixed-language fixture repo without panic — measured on a CI build with **all 10 feature flags explicitly enabled** (`--features lang-cpp,lang-ruby` in addition to the 8 defaults). Default install (8 langs) is exercised by a separate fixture restricted to defaulted languages.
- **H4.** All 11 CLI commands return JSON envelopes that validate against the spec's schema fixtures.
- **H5.** Warm-path p99 budgets met on the Medium representative repo (corpus per research R8 / Q-R8.1):
    - `find-symbol` p99 < **10 ms**.
    - `outline` p99 < **20 ms**.
    - `list-symbols` (recursive, 16-implicated-file cap) p99 < **50 ms**.
    - `search` (FTS5 query) p99 < **30 ms**.
    - `find-references` informational only — no budget.
  All four pinned budgets are sub-bullets under this single perf gate; H-count remains 9 HARD.
- **H6.** CLI cold-start budget met (number set by research R-CLI-1).
- **H7.** FTS5 PRAGMA assertion fires on connection open; binary refuses to operate on a SQLite without FTS5 (exit code 6).
- **H8.** Per-query mtime check is verified by an integration test that mutates a tracked file and asserts the next query reflects the new symbol set without an explicit `reindex`.
- **H9.** `find-references` JSON envelope always carries `heuristic: true` and the documented warning string.

### 14.2 SOFT gates (warning / follow-up bead, non-blocking) — 5 total

- **S1.** Static-linked binary size within the threshold defined by research R-CLI-2 (warns if exceeded; does not block).
- **S2.** `cargo install unblock-code --no-default-features --features lang-rust,lang-python` succeeds and the resulting binary passes the test suite for the selected languages (warns if size reduction is below 50% vs full build).
- **S3.** Vendored `tags.scm` files match upstream `HEAD` of each grammar repo at the pin recorded in research R3 (warns if drift detected at CI time; does not block).
- **S4.** `parse --json-tree` output is byte-stable across runs on identical input (deterministic ordering — warns if non-determinism observed).
- **S5.** ROI report (per L22): indexer arm ≥ 2.0× median throughput vs Glob/Grep/Read baseline across 3 flows × N=10 runs. The harness still runs and publishes raw run logs + computed median ratio per flow as a release artifact (per §16). If the median is below 2.0× on any flow, open a `unblock:finding:risk` follow-up bead linked to this phase; phase close is **not** blocked.

## 15. Risks

- **R1 — ABI fragmentation across grammars.** Research R3 already flagged tree-sitter ABI 14/15 split with stale grammars (e.g. C++ + Ruby last released Nov 2024). Pinning per-grammar version is mandatory; `build.rs` must fail loudly on ABI mismatch.
- **R2 — Static-linked binary size exceeds R-CLI-2 threshold.** Mitigation path is the feature-flag escape hatch (R-CLI-3); fall-back is dropping a grammar from the default set (would require phase-replan).
- **R3 — Cold-start budget unmet.** If the budget set by R-CLI-1 cannot be met with full Top-10 grammars loaded, the daemon-mode deferral (§3) must be revisited. Out-of-scope today; flagged as a phase-replan trigger.
- **R4 — ROI report below 2.0×.** Per L22 / S5 the gate is SOFT: the phase ships, numbers are published, and a `unblock:finding:risk` follow-up bead captures the disappointing result for post-v1.0.0 work. The harness must still land **before** the perf-tuning beads in 03.6 so the ratio is observable throughout the epic, but it no longer ship-blocks. Risk is now "we ship a binary whose ROI is documented as marginal", mitigated by keeping the harness honest (raw logs + per-flow medians) so reviewers can assess severity.
- **R5 — `find-references` heuristic produces low-quality results that erode trust.** Mitigation: every envelope carries `heuristic: true` + warning; the spec must pin the exact warning string verbatim.
- **R6 — Schema-mismatch wipe surprises a user mid-flight.** Mitigation: the wipe path emits an explicit `tracing` event on stderr; documented in `--help` and `status`.
- **R7 — Vendored `tags.scm` drift over time.** Mitigation: research R3 records the pinned commit per grammar; CI can periodically verify upstream drift (S3).

## 16. Definition of Done

This plan is **DONE** when:

- All HARD gates in §14.1 pass on a fresh clone.
- The companion spec (`docs/specs/03-spec-code-indexer.md`) lands at status APPROVED with each L-decision and Q-resolution pinned to a specific section.
- The companion research file (`docs/research/03-research-code-indexer.md`) is re-authored with R3 / R4 / R5 / R7 / R8 surviving content + R-CLI-1 through R-CLI-5 closed at status `CONFIRMED` or `OPEN-WITH-FALLBACK`.
- `cargo install unblock-code` from a release tag produces a working Top-10 binary.
- The ROI harness report is published as a release artifact with raw run logs + computed median ratio per flow.
- Phase 02's `unblock-resilience` has zero consumers from `unblock-indexer` (rollback verified — see [02-spec-mcp-complete §17.1](../specs/02-spec-mcp-complete.md)).

---

**Status: APPROVED (2026-04-29) after user review of Q1 (17 SymbolKinds locked) and Q2 (ROI gate downgraded HARD→SOFT, L22 / S5).** This plan is docs-only; no code change. Stay on `main` per the pre-production stance and the `feedback_branch_base_main` constraint.
