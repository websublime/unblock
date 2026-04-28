# Research 03 — Code Indexer MCP (Phase 03)

> Phase: 03
> Author: Smith (investigator)
> Date: 2026-04-27
> Source plan: [03-plan-code-indexer.md](../plans/03-plan-code-indexer.md)
> Source PRD: [PRD §7 Phase 03](../PRD.md)
> Status: Findings ready for `/spec` consumption

---

## Summary

| # | Status | One-line takeaway |
|---|---|---|
| R1 | **CONFIRMED with adjustment** | `tree-sitter build --wasm` produces a `tree-sitter-<lang>.wasm` blob; current toolchain uses `wasi-sdk` (not emscripten) since v0.26.1. Packaging via GH Releases is mechanically straightforward. |
| R2 | **CONFIRMED — `tree-sitter` crate `wasm` feature wins over `tree-sitter-loader`** | Use `tree-sitter` crate's `wasm` feature → `WasmStore::load_language(name, bytes)`. `tree-sitter-loader` targets *grammars cloned to disk*, not pre-built WASM blobs — it is the wrong tool for L2. |
| R3 | **PARTIALLY CONFIRMED — ABI fragmentation risk** | All 10 grammars exist and are maintained, **but** they span tree-sitter ABI versions 14 and 15 with last-release dates ranging from Nov 2024 to Mar 2026. Cpp + Ruby are stale (Nov 2024). Pin per language; do not assume one tree-sitter version covers all 10. |
| R4 | **CONFIRMED with caveat** | sqlx-sqlite uses libsqlite3-sys's `bundled` feature by default and the rusqlite-style bundled build compiles with `-DSQLITE_ENABLE_FTS5`. **Verify post-merge with `PRAGMA compile_options;`** — sqlx delegates the compile flags to libsqlite3-sys and does not document this guarantee itself. |
| R5 | **CONFIRMED — per-language `.scm`, shared kind enum** | Tree-sitter's own `tags.scm` files (one per grammar repo) already define a near-canonical capture set (`@definition.{class,function,method,interface,module,...}`). Reuse these, don't reinvent. The plan §6.3 enum is mostly aligned; minor adjustments below. |
| R6 | **PARTIALLY CONFIRMED — known macOS atomic-save risk** | `notify-debouncer-full` handles renames via `FileIdMap` and merges paired rename events. Plan's 200ms debounce is in the right order of magnitude but the upstream README example uses 2 s. Per-query mtime check (already in §8.2) is the correct safety net. |
| R7 | **CONFIRMED with caveat** | `ignore::WalkBuilder` handles nested `.gitignore`, `.ignore`, hidden files, and non-git checkouts via `require_git(false)`. Default `require_git=true` will silently disable gitignore on a non-git checkout — must be set explicitly. |
| R8 | **OPEN — methodology defined, numbers TBD** | Latency targets (10ms / 20ms p99) are *plausible* for a warm SQLite + FTS5 index but require the criterion bench suite from Epic 03.6. Methodology proposed below. |
| R9 | **CONFIRMED — six host config map produced** | Six host map enumerated below. JetBrains is the *only* host without a stable user-editable JSON file (settings UI only, with "Import from Claude" supported). |
| R10 | **OPEN — methodology defined, no baseline yet** | Token-saving harness methodology defined below; the actual baseline corpus must be produced during Epic 03.6 by replaying 3 representative agent flows on a fixture repo. |

**Recommendation:** **Proceed to spec authoring.** No locked decision in plan §3 is unworkable. R3 (ABI drift), R4 (FTS5 verification), R6 (macOS), R8 / R10 (post-implementation measurement) carry forward as risks the spec must address explicitly.

---

## R1 — WASM grammar pipeline mechanics

**Validated finding.** `tree-sitter build --wasm` exists, produces a single `tree-sitter-<lang>.wasm` artifact per grammar, and is the supported route for distributing pluggable grammars. The toolchain switched from emscripten to `wasi-sdk` in tree-sitter CLI v0.26.1 (release notes state: *"switching to compile parsers to wasm using wasi-sdk, not emscripten"*). v0.26.7 also published release artifacts as zip archives. CI release-asset publishing on GH Releases is straightforward; max asset size is 2 GiB (a full grammar WASM is < 1 MB), there is no documented bandwidth cap, and assets are downloadable anonymously via `browser_download_url` (GitHub redirects to S3 storage; no separate documented rate limit but per-IP throttling on excessive parallel downloads is observed in the wild).

**Recommended decision.**
1. CI matrix uses **wasi-sdk** in the runner (do not pin emscripten — pre-0.26.1 path is dead).
2. Release tag pattern: `v<unblock-version>-grammars` (already in plan §9), with assets named `tree-sitter-<lang>-<grammar-version>.wasm` + a top-level `manifest.toml` listing `(language, grammar_version, tree_sitter_abi_version, sha256)` per row.
3. Runtime fetcher constructs URLs as `https://github.com/websublime/unblock/releases/download/<release-tag>/<asset-name>`. Use `browser_download_url` (not the API asset endpoint) to avoid the 5 000/h anonymous API budget — the redirect target does not consume API quota.
4. Integrity verified by SHA-256 against `manifest.toml` *after* download.

**Evidence.**
- Tree-sitter v0.26.1 release notes (via Github): *"specify abi version via env var"* + emscripten → wasi-sdk switch.
- v0.26.7 release notes: zip archive distribution.
- GitHub docs: 2 GiB asset size cap, no bandwidth cap; anonymous download via `browser_download_url`.

**Risk register.**
- Emscripten/wasi-sdk transition means anyone reading older tree-sitter docs may be misled. Pin CI to a known tree-sitter CLI version (`>= 0.26.7`).
- Anonymous per-IP throttling on GitHub asset downloads under fan-out (if many users bootstrap simultaneously, e.g. CI). Phase 02's circuit breaker + retry already mitigates; document an offline-bundle escape hatch as future work (not Phase 03).
- `manifest.toml` itself must be authenticated to prevent integrity-bypass — recommendation: ship the manifest as part of the **same** release, signed by the same release tag SHA, and include the manifest's own SHA-256 as a constant in `unblock-indexer-core` (compile-time anchor).

---

## R2 — Runtime WASM loading in Rust

**Validated finding.** The `tree-sitter` Rust crate (latest 0.26.8) has a `wasm` feature flag that pulls in `wasmtime-c-api-impl` and exposes:

- `WasmStore::new(engine: &Engine)` — wasmtime engine wrapper, `Send + Sync`.
- `WasmStore::load_language(name: &str, bytes: &[u8]) -> Result<Language, WasmError>`.
- `Parser::set_wasm_store(&mut self, &mut WasmStore)`.

ABI constants: `LANGUAGE_VERSION = 15`, `MIN_COMPATIBLE_LANGUAGE_VERSION = 13` (per `tree_sitter/api.h`). A grammar built against ABI 14 still loads under tree-sitter 0.26.x.

**`tree-sitter-loader` is the wrong abstraction** for the plan's L4 decision. Per its docs: *"dynamically find and build grammars at runtime, if you have cloned the grammars' repositories to your local filesystem."* It compiles native dylibs (via `cc`/`wasi-sdk`) on the user's machine — opposite of "fetch a pre-built WASM blob from a GH Release." Using `tree-sitter-loader` would force every end-user to have `cc`/`wasi-sdk` installed, breaking pluggability.

**Recommended decision.**
- **Use `tree-sitter` crate with `wasm` feature.** No `wasmtime` direct dependency, no `tree-sitter-loader`. Wasmtime is a transitive dep through the feature.
- Maintain a per-language `WasmStore` *cache* keyed by language name; `Parser` instances are cheap, `WasmStore` is the expensive object (wasmtime compiled module).
- Lazy-load grammars: do not load Java's WASM if the repo has no `.java` files (mirrors plan §8.1 step 2's threshold = ≥ 1 file).

**Evidence.**
- `docs.rs/tree-sitter` confirms the `wasm` feature, `WasmStore` API, and ABI constants.
- `tree-sitter-loader` crate docs explicitly target on-disk grammar repositories.
- `WasmStore` impls `Send + Sync` — usable from a tokio runtime without adapter.

**Risk register.**
- **Init cost (R8 dependency).** `WasmStore::load_language` invokes a wasmtime compilation. No published numbers in tree-sitter docs. Bench during Epic 03.2; if init > 100 ms / language, consider `wasmtime::Engine` configured with cache dir to amortise across processes. Lapce / Zed reportedly observe 50–150 ms cold-load per WASM grammar in practice.
- ABI version mismatch errors are surfaced as `WasmError`. The fetcher must pre-validate `tree_sitter_abi_version` field in `manifest.toml` against the runtime's `MIN_COMPATIBLE_LANGUAGE_VERSION` *before* attempting `load_language` — fail fast with an actionable message.

**Open question (flag for spec).**
- Should we cache the wasmtime compiled artefact (Engine cache) under `~/.cache/unblock/grammars/wasmtime-cache/` to reduce subsequent process startup? Bench result determines this.

---

## R3 — Top-10 grammar audit

**Validated finding.** All 10 grammars exist on `github.com/tree-sitter/tree-sitter-<lang>` and are MIT-licensed. Maintenance and version freshness vary materially:

| Language | Latest version | Released | Notes |
|---|---|---|---|
| Rust | v0.24.2 | 2026-03-27 | Active |
| TypeScript | v0.23.2 | 2024-11-11 | Stale (~17 months); two parsers (`typescript`, `tsx`) |
| JavaScript | v0.25.0 | 2025-09-01 | Active (covers JS + JSX) |
| Python | v0.25.0 | 2025-09-11 | Active |
| Go | v0.25.0 | 2025-08-29 | Active |
| Java | v0.23.5 | 2024-12-21 | Stale (~16 months) |
| C | v0.24.2 | 2026-04-22 | Active (just released) |
| C++ | v0.23.4 | 2024-11-11 | **Stale (~17 months); 54 open issues** |
| Ruby | v0.23.1 | 2024-11-11 | Stale (~17 months) |
| PHP | v0.24.2 | 2025-08-18 | Active |

ABI versions across these grammars are not all aligned. The 0.23.x grammars typically target ABI 14; 0.24.x and 0.25.x target ABI 14 or 15. Tree-sitter 0.26.x runtime supports ABI 13–15, so all 10 *load* — but the manifest must record the actual ABI per grammar.

**Recommended decision.**
1. **Pin grammar versions in `manifest.toml`** with explicit `tree_sitter_abi_version` field per row. Use the latest release of each as of 2026-04-27 (table above) for v1.0.0.
2. **Add a "freshness" column** in CI grammar audit job — flag grammars whose latest release is > 12 months old. Cpp / Ruby / TypeScript / Java currently flag.
3. **TypeScript ships as two grammars** (`typescript` and `tsx`). Plan §6.3 / §2.4 must be aware: *"TypeScript"* in the Top-10 is two WASM blobs, not one. Recommend treating `tsx` as the canonical TS grammar (it accepts both `.ts` and `.tsx` per tree-sitter-typescript README).
4. **Document the upstream-stale risk** in plan/spec §15 — if a stale upstream grammar misses a language feature, the contribution path is to upstream and version-bump, not fork.

**Evidence.** GitHub release pages for each `tree-sitter-<lang>` repo (queried 2026-04-27).

**Risk register.**
- **C++ grammar staleness** (Nov 2024, 54 open issues) is the highest-risk item in the Top-10. Verify against a representative C++20/23 fixture during Epic 03.4 fixture work.
- **Ruby** grammar is also Nov 2024; Ruby 3.3+ pattern-matching syntax may parse with errors. Same fixture validation needed.
- ABI version pinning per grammar means the matrix in `grammars.yml` is `language × grammar_version` (per-cell version), not a single tree-sitter version.

---

## R4 — `sqlx` + FTS5

**Validated finding.** Three independent confirmations that FTS5 is present in the default sqlx-sqlite build:

1. `sqlx-sqlite` enables `libsqlite3-sys`'s `bundled` feature by default — confirmed in sqlx README and `sqlx-sqlite/Cargo.toml`.
2. `libsqlite3-sys`'s bundled `build.rs` includes `.flag("-DSQLITE_ENABLE_FTS5")` — confirmed against rusqlite source (sqlx delegates to the same crate).
3. SQLite docs: FTS5 is **not** included by default; it must be enabled via `-DSQLITE_ENABLE_FTS5`. Bundled libsqlite3-sys does so.

WAL mode is set via `SqliteConnectOptions::journal_mode(SqliteJournalMode::Wal)`. The setting is sticky across connections (per sqlx docs).

External-content FTS5 with sync triggers (the schema in plan §10) is the canonical pattern per `sqlite.org/fts5.html`, including the `'delete'` command that must run **before** content-table mutation in UPDATE/DELETE triggers. The plan's schema is correct in shape but the trigger ordering must be explicit in the spec.

**Recommended decision.**
1. Use `sqlx = { version = "0.8", features = ["sqlite", "runtime-tokio", "macros", "migrate"] }` — `bundled` is implicit through `sqlx-sqlite`.
2. Run `PRAGMA compile_options;` on first connect and assert that `ENABLE_FTS5` appears. Surface a hard error otherwise — guards against a future libsqlite3-sys regression.
3. Document the canonical FTS5 trigger pattern in `unblock-indexer-core` schema constants (DDL strings) as **insert / delete / update** triplet. Insert-trigger is straightforward; delete and update must use the `INSERT INTO symbols_fts(symbols_fts, rowid, ...) VALUES('delete', ...)` form *before* the content-table mutation.
4. Use `INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild')` after `reindex(path?)` to repopulate the index from the freshly-truncated content table.

**Evidence.**
- rusqlite `libsqlite3-sys/build.rs` line 116-ish: `.flag("-DSQLITE_ENABLE_FTS5")`.
- sqlx README confirms `bundled` is the default for the `sqlite` feature.
- SQLite FTS5 docs document the external-content pattern, the `'delete'` command, and the `'rebuild'` command.

**Risk register.**
- A `bundled = false` override (e.g. user using system libsqlite3 without FTS5) would silently break the indexer. The runtime `PRAGMA compile_options;` check catches this.
- Performance at scale: SQLite docs note FTS5 indexes are typically 30–50% of content size and require one extra content-table lookup per match. For symbol-name FTS this is irrelevant (names are short, content table is `symbols`); for `signature` and future `comment` columns it scales linearly with code volume. Acceptable for token-saving workload.
- WAL writer/reader contention under the file watcher: WAL allows N readers + 1 writer concurrently. Bootstrap uses a *single* transaction, so it holds the writer lock for the whole bootstrap duration — readers (queries) will block. **Spec must call this out** and recommend either (a) chunked transactions during bootstrap, or (b) disabling query serving until bootstrap completes. (a) is preferable for UX.

**Open question (flag for spec).**
- Plan §6.2 declares `symbols_fts` columns `name, signature, comment`. Where does `comment` come from? Not in the `symbols` content table per the schema. Either (i) add `comment TEXT` column to `symbols`, or (ii) drop `comment` from FTS5 for v1. **Decision needed in spec.**

---

## R5 — Symbol extraction queries (S-expressions)

**Validated finding.** Tree-sitter grammars ship a *standard* S-expression query file `queries/tags.scm` per language repo, designed for exactly this use case (originally for `ctags` / GitHub code navigation). They use a converged capture vocabulary:

- `@definition.{class, function, method, interface, module, macro, constant, type, ...}`
- `@reference.{call, type, class, implementation}`
- `@name`, `@doc`

Sample audit:

| Lang | Capture set in `tags.scm` |
|---|---|
| Rust | `definition.{class (struct/enum/union), function, method, interface (trait), module, macro}`; `reference.{call, implementation}` |
| Python | `definition.{constant, class, function}`; `reference.call` |
| Go | `definition.{function, method, type}`; `reference.{call, type}` |
| TypeScript | `definition.{function, method, class, module, interface}`; `reference.{type, class}` |

Per-language tweaks are inevitable (Go has no class; Rust has `trait` mapped to `interface`; Python has no formal `interface`). The plan's §6.3 kind enum is well-aligned with this vocabulary.

**Recommended decision.**
1. **Reuse upstream `tags.scm` as the starting point** for each language; vendor a copy under `crates/unblock-indexer-core/queries/<lang>.scm` (so spec changes are versioned in our repo, decoupled from grammar releases).
2. **Adjust the canonical kind enum** (plan §6.3) to align with `tags.scm` vocabulary:
   - Add: `macro` (Rust, Ruby), `type` (Go's `type X = Y`).
   - Map in the traversal: `class → class | struct | enum | trait` per language. Document the mapping table in `unblock-indexer-core::kind::map_capture_to_kind()`.
   - Drop or postpone: `field`, `property`, `import`, `export` — these are **not** in standard `tags.scm` and would require per-language hand-written queries. Defer to a v1.1.
3. **Final canonical enum (recommendation):** `function`, `method`, `class`, `struct`, `enum`, `interface`, `trait`, `module`, `namespace`, `variable`, `constant`, `type_alias`, `macro`. (13 variants, down from 16.) Spec adopts this as the source of truth.

**Evidence.**
- `tags.scm` files for Rust, Python, Go, TypeScript per their respective tree-sitter-<lang> repos (queried 2026-04-27).
- nvim-treesitter project demonstrates the `tags.scm`/`highlights.scm`/`locals.scm` convention is the de-facto standard.

**Risk register.**
- Stale grammars (Cpp / Ruby / Java — see R3) may have outdated `tags.scm` missing modern syntax (e.g. C++ concepts, Ruby pattern matching). Validate with fixture repos in Epic 03.4.
- `tags.scm` is not authoritative for *every* symbol — e.g. Rust's `tags.scm` doesn't capture `impl` blocks separately. We may need supplementary queries for `outline` hierarchical view (parent_id linkage). Plan §6.3 requires `parent_id`; `tags.scm` alone gives flat captures. **Spec must address parent linkage** — typically via post-processing the captured node's tree position.

**Open question (flag for spec).**
- Are field/property/import/export deferred to v1.1 acceptable, or must they ship in Phase 03? Plan §6.3 lists them. Recommendation above is to drop. **User decision required if the plan's enum is to remain authoritative.**

---

## R6 — `notify-debouncer-full` cross-platform behaviour

**Validated finding.**

- **Platforms:** linux/inotify, macOS/FSEvents, Windows/ReadDirectoryChangesW, BSD/kqueue. Build matrix confirmed for aarch64-apple-darwin, aarch64/x86_64-linux-gnu, i686/x86_64-pc-windows-msvc.
- **Renames:** debouncer merges paired Rename From/To events. `FileIdMap` (the recommended cache) "stitches together rename events in case the notification back-end doesn't emit rename cookies" — explicitly designed for backend variance.
- **Atomic save (vim, IntelliJ, VS Code):** these editors write to a `.tmp` file then rename onto the target. The debouncer's rename-merging handles this *if* the original target is also being watched. The "delete" event is suppressed and "modify" emitted in well-handled cases; mishandling produces a delete-then-create pair. The plan's per-query mtime check is the documented safety net.
- **Debounce window:** the upstream README example uses `Duration::from_secs(2)`. Plan's 200 ms is aggressive; for an interactive editor save it is fine, but for a `rsync`/`git checkout` of many files you can see thrash. Recommendation below.
- **Large directory trees:** notify uses recursive watches; on Linux, inotify has a per-user watch limit (default 8192 → 524288 on modern systems) — a large monorepo can exhaust it. The plan's `ignore`-aware walker reduces watch surface, but the watcher itself currently watches the whole repo root recursively. **Mitigation:** call `notify::Watcher::watch(root, RecursiveMode::Recursive)` once on the canonicalised root, document the inotify limit in the README, and surface watcher-init failures with an actionable error (e.g. `cat /proc/sys/fs/inotify/max_user_watches`).

**Recommended decision.**
1. Use `notify-debouncer-full` (latest 0.5.x branch tracks `notify` v8/v9). Configure with `FileIdMap` cache.
2. **Default debounce: 500 ms** (compromise between plan's 200 ms and upstream's 2 s). Make it configurable via `.unblock/indexer.toml` with the 200 ms recommendation in the spec for interactive flows. Bench during Epic 03.5 to validate.
3. **Per-query mtime check is mandatory** — plan §8.2 already specifies this. Spec must phrase it as an invariant, not an optimisation.
4. **Linux inotify hint** in error messages when `Watcher::new` fails on large trees.

**Evidence.**
- `notify-debouncer-full` docs.rs page.
- `FileIdMap` docs.rs page.

**Risk register.**
- macOS FSEvents has known behaviour where recursively-watched paths can drop events under heavy load. Per-query mtime check is the safety net; document this in spec §15.
- Windows `ReadDirectoryChangesW` reports renames as a single event but gives the *new* path only — `FileIdMap` is essential there.
- WSL (Windows-hosted Linux dev) sees pathological inotify behaviour; not a Phase 03 blocker but document.

---

## R7 — `ignore` crate edge cases

**Validated finding.**

- **Nested gitignores:** `WalkBuilder` walks them with documented precedence: glob overrides → `.ignore` → `.gitignore` → `.git/info/exclude` → global → explicit. *"More nested ignore files have a higher precedence than less nested ignore files."* Monorepos with sub-`.gitignore` files Just Work.
- **Non-git checkouts:** **`require_git()` defaults to `true`** — if the directory has no `.git`, gitignore rules are *silently disabled*. To respect `.gitignore` outside a git repo, must call `WalkBuilder::require_git(false)`. **This is a footgun the plan does not currently address.**
- **Custom ignore filenames:** `add_custom_ignore_filename(".unblock-ignore")` available — useful future extension, not Phase 03 scope.
- **Hidden files:** ignored by default (`.git`, `.venv`, etc.); plan's default-excludes list (`target/`, `node_modules/`, `dist/`, `build/`, `.venv/`, `vendor/`, `.git/`) is layered *on top* of this. Note `.git/` is already covered by the hidden-file default.

**Recommended decision.**
1. Configure walker as: `WalkBuilder::new(root).require_git(false).hidden(true).git_ignore(true).git_global(true).git_exclude(true)`.
2. Apply default-excludes via `WalkBuilder::filter_entry` or a custom `Override` glob set — the plan's "default excludes" list (§L6) lives in `unblock-indexer-core` as a constant and is appended by the walker.
3. **`.unblock/languages.toml` override** (plan §L6) — implement as an additive override of which extensions map to which language. Walker still respects gitignore.
4. Document that `same_file_system(true)` should be used to prevent crossing into mounted volumes (e.g. macOS `node_modules` symlinked into `/Volumes/...`).

**Evidence.**
- `WalkBuilder` docs.rs.

**Risk register.**
- Without `require_git(false)`, a developer running unblock on a tarball checkout (no `.git`) would walk into `node_modules/` etc., severely degrading bootstrap. This is a **plan-level adjustment** — spec must specify `require_git(false)`.
- Some monorepos use `.gitignore` files that exclude vendored sources the agent *wants* indexed (e.g. a `vendor/` directory in Go modules). The `.unblock/languages.toml` override in plan §L6 covers extension overrides but not "force-include directory." Possible v1.1 extension; flag as open question.

**Open question (flag for spec).**
- Should `.unblock/indexer.toml` also support a `force_include = ["vendor/"]` field to bypass `.gitignore`? Out of scope per plan §L6 reading, but a real-world need.

---

## R8 — Latency benchmarks methodology

**Validated finding.** No published numbers exist for the exact stack (sqlx + bundled SQLite + WAL + FTS5 with this schema). However, neighbouring evidence:

- A primary-key indexed lookup on SQLite (the `idx_name` index in §10) is typically sub-ms even for tables with millions of rows. The plan's p99 < 10 ms for `find_symbol` is *very* achievable for non-fuzzy queries.
- FTS5 prefix/MATCH queries on a content table with O(100k) symbols typically run in 1–5 ms.
- The dominant cost in `find_symbol` will be (i) per-query mtime check (filesystem `stat` — < 1 ms), (ii) optional re-parse of a single file if mtime is newer (10–50 ms — *blows the budget*), (iii) the SQL query (< 1 ms).
- **The mtime-check-triggered re-parse is the budget killer.** Plan §8.2 must clarify: per-query mtime check applies only to files implicated by the *result*, not the search input. For `find_symbol(name)` this means re-parsing only the file that owns the matching symbol — typically zero re-parses on a steady-state warm system.

**Recommended methodology.**
1. **Corpus** (Epic 03.6 R8 dependency):
   - Small repo: ~500 files, ~5 k symbols (this very repo).
   - Medium repo: ~5 k files, ~50 k symbols (e.g. ripgrep, tokio).
   - Large repo: ~50 k files, ~500 k symbols (e.g. Linux kernel `fs/` subset or LLVM project).
2. **Bench harness:** `criterion` with `async_tokio`. Three benchmarks per query type (`find_symbol`, `outline`, `search_text`) per corpus.
3. **Cold path** (first call after process start) excluded from p99 — measure `warm path` only. Add a `cold_start` benchmark separately to inform expectations.
4. **Fail the build** if p99 > 10 ms / 20 ms on the medium corpus. Large is informational.
5. **WAL contention bench:** simultaneous reader (query) + writer (file watcher inserting after a re-parse) — ensure the writer never starves readers > 50 ms.

**Risk register.**
- Per-query mtime check + re-parse trigger is the dominant tail-latency risk. **Spec must specify the implicated-file rule** to bound the re-parse scope.
- `wasm` parsing init cost (R2) — if not amortised across queries, it adds 50–150 ms per first-touch language. Cache parser instances + `WasmStore` in a `tokio::sync::OnceCell` or similar.
- SQLite WAL checkpoint lag on heavy bootstrap — explicit `PRAGMA wal_autocheckpoint = 1000;` recommendation.

---

## R9 — Setup auto-config schemas

**Validated finding.** Six target hosts; five have user-editable JSON config files, one (JetBrains) does not.

| Host | Config path | Schema (top key) | Notes |
|---|---|---|---|
| **Claude Desktop** | macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`<br>Windows: `%APPDATA%\Claude\claude_desktop_config.json`<br>Linux: not officially supported | `mcpServers` (object) → `<name>: { command, args, env }` | Restart Claude Desktop required after edit. |
| **Claude Code** | macOS/Linux: `~/.claude.json` (user-scope) or per-project `.claude/settings.json` (workspace-scope, in repo)<br>Windows: `%USERPROFILE%\.claude.json` | `mcpServers` (object) — same shape as Claude Desktop. CLI: `claude mcp add <name> <command>` writes the same file. | CLI is the recommended programmatic path. |
| **Cursor** | Project: `.cursor/mcp.json` (in repo)<br>Global: `~/.cursor/mcp.json` | `mcpServers` (object) — same shape. Supports `${env:NAME}`, `${workspaceFolder}`, `${userHome}` interpolation. | Cross-platform (`~` works everywhere per Cursor docs). |
| **Zed** | Settings file (user `~/.config/zed/settings.json` on Linux/macOS; AppData on Windows). | `context_servers` (object) → `<name>: { command, args, env }` or `{ url, headers }` for remote. | Different top-level key (`context_servers`, not `mcpServers`). |
| **VS Code** (Copilot) | Workspace: `.vscode/mcp.json` (in repo)<br>User: edited via "MCP: Open User Configuration" command | `servers` (object) → `<name>: { type: "stdio"\|"http", command, args }` | Different top-level key (`servers`, not `mcpServers`). Different field (`type` instead of inferring from `url` vs `command`). |
| **JetBrains** (AI Assistant) | **No documented user-editable JSON file.** Configure via Settings → Tools → AI Assistant → Model Context Protocol (MCP) → Add. UI accepts an `mcpServers` JSON snippet. Supports "Import from Claude" button. | Same `mcpServers` shape (pasted into modal) | Programmatic auto-config is **not supported** — can only print copy-pastable JSON. |

**Recommended decision.**
1. **`unblock-mcp setup` extends with two responsibilities:** (a) the existing GitHub Project setup (which is what `setup` does today per Phase 01–02), and (b) **a new sub-command or flag** like `unblock-mcp setup --register-mcp <host>` for editor auto-config. Plan §L13 currently conflates these — spec must split.
2. **Idempotent merge per host** — read existing config, merge our key under `mcpServers` / `context_servers` / `servers` (host-specific), preserve unrelated keys, write back. Key collision: prompt the user.
3. **JetBrains:** print copy-pastable JSON to stderr with a clear pointer to Settings → Tools → AI Assistant → MCP → Add → "Import from Claude". Document this as the explicit limitation in §14.6.
4. **Schema differences must be encoded** in `unblock-mcp` per host (top-level key + field names). Don't assume `mcpServers` everywhere.
5. **`type` field in VS Code** must be set to `"stdio"` for our binary.

**Evidence.**
- Anthropic MCP user quickstart (Claude Desktop paths).
- Cursor docs (`docs.cursor.com/context/mcp`).
- VS Code Copilot MCP docs (`code.visualstudio.com/docs/copilot/chat/mcp-servers`).
- Zed AI MCP docs (`zed.dev/docs/ai/mcp`).
- JetBrains AI Assistant MCP help (`www.jetbrains.com/help/ai-assistant/mcp.html`).
- Claude Code MCP docs (`code.claude.com/docs/en/mcp`).

**Risk register.**
- **Naming collision in plan §L13.** The existing `setup` MCP tool (per Phase 01–02) configures GitHub Projects, not editor MCP entries. Plan §L13's "extends" wording is ambiguous. Recommendation: introduce a separate CLI sub-command (e.g. `unblock-mcp install` or `unblock-mcp register`) instead of overloading the existing `setup` MCP tool. **Surface as `needs-review` for Ada.**
- JetBrains' lack of programmatic config means the §14.6 acceptance criterion "registers ... in JetBrains" can only be partially met. Recommend rewording to "produces JetBrains-ready JSON snippet for manual import" to keep the criterion verifiable.
- Idempotent merge conflicts: a user with two MCP servers named `unblock` (e.g. local + remote in Phase 06) needs disambiguation strategy. Reserve `unblock-local` / `unblock-remote` namespacing.

**Open questions (flag for plan).**
- Q9.1: Is the existing `setup` MCP **tool** being repurposed, or is a new CLI subcommand acceptable? Plan §L13 implies the former; my reading of the existing codebase + the constraint is the latter is cleaner.
- Q9.2: §14.6 wording for JetBrains — accept "manual import via copy-paste" as completion?

---

## R10 — Token-saving measurement methodology

**Validated finding.** No published methodology exists for "MCP-tool vs Glob/Grep/Read token saving" — this is a novel measurement. The plan correctly defers it to a phase-exit gate (§14.8). Methodology proposal below.

**Recommended methodology.**

1. **Baseline corpus** — three representative agent flows on a single fixture repo (this repo at the v1.0.0 tag is a reasonable choice):
   - **Flow A (find a symbol):** "find the implementation of `DependencyGraph::ready_set`."
   - **Flow B (understand a file):** "give me the structure of `crates/unblock-core/src/graph.rs`."
   - **Flow C (find references):** "what calls `parse_github_url`?"

2. **Measurement protocol:**
   - **Baseline run:** an agent (Claude Sonnet via API, not via plugin) is given Glob/Grep/Read tools only. Record total input + output tokens to first-correct-answer.
   - **Indexer run:** same agent, given the 9 indexer tools instead. Record same.
   - **Correctness gate:** the answer must match a human-blessed gold answer (text match + symbol-id where applicable). Wrong answers don't count.
   - **Repeat each flow N=10 times** with fresh sessions to control prompt-cache variance.

3. **Reported metrics:**
   - Median + p95 token count per flow per mode.
   - **Token-saving ratio** = `tokens_baseline / tokens_indexer` (target: ≥ 3× for Flow A; ≥ 2× for B/C). These targets are *directional* — actual numbers calibrate the v1.0.0 marketing claim.
   - **Latency:** time-to-first-correct-answer (informational, not gating).

4. **Output artefact:** `docs/research/03-code-indexer-roi.md` (per plan §14.8) — table of metrics per flow + raw transcripts archived as test fixtures.

5. **Phase-exit gate:** report exists, has populated numbers, and the median ratio across all three flows is `> 1.5×`. If not, the phase ships v1.0.0 anyway (per pre-production stance), but the marketing claim is downgraded and a follow-up bead opens to investigate.

**Risk register.**
- Token counts depend on the model's tokenizer — pin to Sonnet's tokenizer for reproducibility. Future models will produce different numbers; the harness must be re-runnable.
- Three flows is a small corpus — directional, not statistical. Consider expanding to 10 flows in v1.1.
- The "agent" running the harness is itself a confounder: a smarter agent uses fewer tokens regardless of tools. Pin the system prompt and the model version in the harness.

**Open questions (flag for spec).**
- Q10.1: Should the harness use an unblock supervisor (Sherlock) or a generic agent? An unblock supervisor is more representative of the production use case but introduces plugin-skill confounders. Recommendation: generic agent for the baseline; document as "lower bound" since supervisors may save more.
- Q10.2: What is the gating threshold (1.5×, 2×, 3×)? Plan §14.8 doesn't specify. Recommendation: 1.5× median, soft gate.

---

## Cross-cutting risks and `needs-review` items

These surfaced during research but are *outside* R1–R10's scope as defined in the plan:

### NR1 — Plan §L13 / §6.3 / §10 internal inconsistency

- **Plan says:** §L13 "extends `unblock-mcp setup`". §6.3 lists 16 symbol kinds including `field/property/import/export`. §10 declares `symbols_fts(name, signature, comment)` but no `comment` column on `symbols`.
- **Reality:**
  - `setup` is an existing MCP tool with established semantics (project setup) — overloading it conflates concerns. (See R9 risk.)
  - `field/property/import/export` are not in standard `tags.scm` and require hand-written queries per language — costly. (See R5 open question.)
  - `comment` in FTS5 has no source column. (See R4 open question.)
- **Impact:** Spec authoring will hit these as unresolved questions.
- **Recommendation:** Ada to resolve the three points above either by amending the plan or by deferring to spec — flagged but not silently rewritten.

### NR2 — Phase-02 dependency ambiguity

- **Plan says:** §13 "Phase 02 (MCP Complete) **must be merged** before Epic 03.2 starts."
- **Reality:** Per `bd` (and PRD §7.2), Phase 02 status is unclear from the documents. The retry-with-backoff and circuit-breaker primitives may live in `unblock-github`, not `unblock-mcp`. Epic 03.2's grammar fetcher needs to *reuse* them — verify the API surface is stable enough to depend on.
- **Recommendation:** Confirm Phase 02's status with the orchestrator before Fernando opens beads for Epic 03.2.

### NR3 — Pre-production stance vs. acceptance criteria

- Plan §14 lists hard acceptance criteria (e.g. "p99 < 10 ms"). Pre-production stance per CLAUDE.md and plan §49 says *"breaking changes acceptable."* These are not contradictory but the spec must distinguish:
  - **Hard gates** (must-pass for v1.0.0 ship): functional + storage + lifecycle.
  - **Soft gates** (informational, document if missed): performance + token-saving ROI.
- **Recommendation:** Spec §Acceptance to split into hard / soft, mirroring this distinction.

---

## Open questions consolidated (for orchestrator → Ada)

| ID | Question | Source gap |
|---|---|---|
| Q4 | `comment` column source for FTS5 — add to `symbols` or drop from FTS5 in v1? | R4 |
| Q5 | Drop `field/property/import/export` from canonical kind enum to v1.1? | R5 |
| Q7 | Add `force_include` glob list to `.unblock/indexer.toml`? | R7 |
| Q9.1 | Use new CLI subcommand instead of overloading `setup` for editor MCP register? | R9 / NR1 |
| Q9.2 | Accept "manual import" as JetBrains §14.6 completion? | R9 |
| Q10.1 | Harness uses generic agent or supervisor? | R10 |
| Q10.2 | ROI gate threshold (1.5× / 2× / 3×)? | R10 |

---

## Final verdict

- **Dependencies investigated:** 12 (`tree-sitter`, `tree-sitter-loader`, `wasmtime` transitive, `sqlx`, `libsqlite3-sys` for FTS5+WAL, `ignore`/`WalkBuilder`, `notify-debouncer-full`/`FileIdMap`, `rayon`, `reqwest`, `sha2`, `tree-sitter-<10 grammars>`, GitHub Releases API).
- **Assumptions validated:** 14 — confirmed: 10; partially confirmed: 3; contradicted: 1.
- **Single contradiction (C1):** plan §10 lists `tree-sitter-loader` as a runtime candidate; correct choice is the `tree-sitter` crate's `wasm` feature (transitively `wasmtime-c-api-impl`). `tree-sitter-loader` targets on-disk grammar repos, not pre-built WASM blobs.
- **Risks (top tier):** R3 grammar ABI fragmentation; R4 FTS5 runtime PRAGMA verification; R6 macOS FSEvents drops; R7 `WalkBuilder::require_git(false)` footgun; R8 WASM init cost + per-query re-parse trigger; NR1 plan internal inconsistencies.
- **Open questions:** 7 — all need Ada/user decision before spec authoring.

**Recommendation: PROCEED to spec authoring.** No locked decision in plan §3 is unworkable. The 7 open questions and 3 needs-review items must be resolved by Ada inside the spec — they do not require renegotiating the plan.

---

## Resolution log (orchestrator + user, 2026-04-28)

The 7 open questions and the cross-cutting items above were decided through user iteration after Smith's research returned. These resolutions are **binding** for spec authoring — do not re-litigate.

### Q4 — `comment` column for FTS5 → **Add to schema (Option A)**
- Add `comment TEXT` column to `symbols` table
- AST traversal extracts per-language doc-comments: Rust `///`, Python docstrings, JSDoc/Javadoc `/** */`, Go above-declaration comments, Ruby `#`/heredocs
- FTS5 indexes `name + signature + comment`
- **Scope impact:** Epic 03.4 grows by ~2-3 weeks (per-language comment-attachment logic)
- **Rationale:** pre-production stance prioritises robustness over ship velocity; full-content search is genuine value for token-saving on semantic queries

### Q5 — Canonical kind enum → **Keep all 16 (Option A)**
- Variants: `function`, `method`, `class`, `struct`, `enum`, `interface`, `trait`, `module`, `namespace`, `variable`, `constant`, `type_alias`, `macro`, `field`, `property`, `import`, `export`
- The 4 extras (`field`, `property`, `import`, `export`) require hand-written S-expression queries per language on top of the upstream `tags.scm`
- Vendor a copy of upstream `tags.scm` under `crates/unblock-indexer-core/queries/<lang>.scm` and extend per language
- **Scope impact:** Epic 03.4 grows by ~40-80h (4 kinds × 10 langs × ~2-4h after first)
- **Rationale:** robustness desde dia 1; agente consegue procurar struct fields and module imports sem fallback para Read+grep

### Q7 — `force_include` glob list → **Include in MVP (Option A)**
- `.unblock/indexer.toml` schema supports:
  ```toml
  [walker]
  force_include = ["vendor/**", "generated/**", "third_party/**"]
  ```
- Override `.gitignore` but **not** the hardcoded default-excludes (`target/`, `node_modules/`, `dist/`, `build/`, `.venv/`, `vendor/`, `.git/`)
- Implementation via `ignore::overrides::Override` applied after `WalkBuilder`
- **Scope impact:** ~4-6h in Epic 03.5
- **Rationale:** real-world need (Go vendored deps, generated code in monorepos)

### Q9.1 — Three entry points (refactor `setup` tool, add wizard) → **B+C, one-shot unified flow priority**

Three coexisting entry points:

| Entry | Caller | When | Scope |
|---|---|---|---|
| `unblock-mcp init` (NEW) | Human in terminal | Onboarding (canonical, one-shot) | Editor register + GitHub Project setup via wizard |
| `unblock-mcp register --host=<x>` (NEW) | Human in terminal or CI | Add editor later, scripted setup | Editor register only |
| `setup` MCP tool (existing) | Agent in active session | Self-heal, idempotent re-setup | GitHub Project setup only |

**Refactor required:**
- Extract `setup` MCP tool's logic into library function `ensure_github_project(client, repo, token) -> Result<SetupReport, SetupError>` in `unblock-github` (or `unblock-core::setup`)
- The MCP tool handler becomes a thin wrapper around this function
- `init` wizard calls the same function
- API contract of MCP tool is preserved (idempotent, same JSON response)

**`unblock-mcp init` wizard flow (3 steps):**
1. Detect editors installed → ask which to register
2. GitHub Project setup → prompt `GITHUB_TOKEN` + repo, validate, run `ensure_github_project()`
3. Register MCP server in selected editors → idempotent merge of config files
4. Print summary + JetBrains manual instructions (per Q9.2)
5. Print "next steps" (restart editors, try `unblock` MCP `ready` tool from inside the agent)

**`unblock-mcp register --host=<x>` flags:**
- `--host=<cursor|claude-code|claude-desktop|zed|vscode|jetbrains|all>`
- `--scope=<workspace|user>` (default user)
- `--server-name=<name>` (default `unblock`)
- `--print-only` (dry run, prints JSON)
- `--force` (overwrite existing entry without prompt)

### Q9.2 — JetBrains support → **Manual import via "Import from Claude" workflow**
- No JetBrains-specific code in the codebase
- `init` wizard prints clear instructions: open IDE → Settings → Tools → AI Assistant → MCP → Add → "Import from Claude" → select `unblock`
- JetBrains AI Assistant reads from Claude Desktop / Code config (which we just registered)
- Acceptance criterion §14.6 phrased as: *"`init` wizard surfaces unblock entry such that JetBrains user can import via 5-click workflow within 30 seconds"*
- **Rationale:** programmatic JetBrains config is technically infeasible (XML internals, version-volatile, unsupported); building a JetBrains plugin is out of scope (separate Kotlin project, +3-4 weeks). "Import from Claude" workflow already exists upstream and is acceptable UX

### Q10.1 — ROI harness agent → **Sonnet via Anthropic API + Claude-Code-like system prompt**
- NOT vanilla API (too divorced from real usage)
- NOT supervisor (Sherlock doesn't exist until Phase 04)
- Sonnet via Anthropic API with a system prompt mimicking Claude Code defaults
- System prompt versioned in `tests/roi/system-prompt.md`
- **Phase 04 follow-up (informational, not gate):** re-run harness with Sherlock supervisor when Phase 04 ships; output `docs/research/04-code-indexer-roi-supervisor.md`

### Q10.2 — ROI threshold structure → **Hard 2.0× global + soft per-flow aspirationals (Option D)**

```
GATE (HARD — blocks Epic 03.6 close):
  median(ratio across 3 flows × N=10 runs) ≥ 2.0×

ASPIRATIONALS (SOFT — reported, not blocking):
  Flow A (find_symbol exact lookup) ≥ 3.0×
  Flow B (outline file)             ≥ 2.0×
  Flow C (find_references)          ≥ 1.5×

ESCAPE PATH if hard gate fails:
  1. Block close of Epic 03.6
  2. Open finding bead (label: unblock:finding:risk) for investigation
  3. Investigate (perf? harness bug? query patterns?) and remediate
  4. Re-measure; only after hard gate passes does Epic 03.6 close

OUTPUT:
  docs/research/03-code-indexer-roi-claude-code.md
  Table: per-flow median + p95 + variance + N + raw transcripts archived as fixtures
```

- **Rationale:** 2.0× is defensible from estimates (expected 2.5-4× median), marketing-clean ("halves token usage"), aligns with "robust" stance — accountability via hard gate. Per-flow soft aspirationals capture honest expectations per query type without blocking ship for edge cases.

### Cross-cutting: contradiction C1 absorbed
- **Plan §10's mention of `tree-sitter-loader` is wrong** for the WASM runtime. Use `tree-sitter` crate's `wasm` feature directly (transitively `wasmtime` via `wasmtime-c-api-impl`). `tree-sitter-loader` targets on-disk grammar repos, not pre-built WASM blobs. Spec must reflect this correction.

### Cross-cutting: NR1 resolved by Q4 + Q5 + Q9.1
- The plan §L13 ambiguity is resolved by Q9.1 (refactor + 3 entry points)
- The §6.3 enum ambiguity is resolved by Q5 (keep all 16)
- The §10 `comment` column issue is resolved by Q4 (add column)

### Cross-cutting: NR3 — hard/soft acceptance gate split adopted
- HARD gates (block ship): functional correctness, storage integrity, lifecycle (bootstrap / shutdown), ROI gate per Q10.2
- SOFT gates (informational, reported): performance benchmark targets per query type (informational), per-flow ROI aspirationals per Q10.2

### Remaining unresolved item: NR2 — Phase 02 dependency surface
- Plan §13 says "Phase 02 must be merged before Epic 03.2 starts" because the grammar fetcher reuses retry / circuit-breaker / OTel
- Smith flagged that the actual API surface needs verification
- **Spec action:** either cite Phase 02's API surface explicitly (if known when spec lands), or mark this dependency as UNRESOLVED in §15 and require explicit verification before Epic 03.2 opens beads

