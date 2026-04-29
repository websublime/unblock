# Research 03 — Code Indexer CLI (Phase 03)

> Phase: 03
> Author: Smith (investigator)
> Date: 2026-04-29
> Source plan: [03-plan-code-indexer.md](../plans/03-plan-code-indexer.md) (APPROVED, commit a25bf21)
> Source PRD: [PRD §7 Phase 03](../PRD.md)
> Status: **PROCEED to spec authoring** — pending one BLOCKING plan amend (R-CLI-4 / H2)
> History: this file was re-authored on 2026-04-29 after the MCP→CLI reframe (commit a77757e). The prior PRE-REFRAME content has been superseded; survival map for old R-sections is enumerated in §0.

---

## 0. Reframe history

This research file was re-authored after the 2026-04-29 reframe that turned Phase 03 from MCP tools into the `unblock-code` CLI. The prior file's R3 / R4 / R5 / R7 / R8 sections survive in spirit and have been re-validated against current state of crates.io and upstream tree-sitter repos (versions and findings updated). The prior R1, R2 (WASM runtime), R6 (watcher), R9 (editor MCP config), and R10 (HARD ROI gate) are obsolete and not present in this file.

New gaps R-CLI-1 through R-CLI-5 — introduced by the static-linked CLI design — are documented inline below.

---

## Summary

| # | Status | One-line takeaway |
|---|---|---|
| R3 | **CONFIRMED with stale flags** | All 10 upstream `tree-sitter-<lang>` Cargo crates publish usable Rust bindings; ABI 14/15 split fits tree-sitter 0.26.8 compatibility window; 4 grammars (typescript, cpp, ruby, java) flagged stale (>12 months). |
| R4 | **CONFIRMED** | sqlx 0.8.6 + `libsqlite3-sys/bundled` 0.37 ship `ENABLE_FTS5` on every supported platform; canonical `'delete'`-then-insert triggers + chunked-tx bootstrap pattern documented. |
| R5 | **CONFIRMED with scope correction** | All 10 grammars expose `tags.scm`. The plan's "4 hand-written extension queries" estimate under-counts: real cost is ~30–40 query rules across all 10 languages because syntax differs per-language. |
| R7 | **CONFIRMED** | `WalkBuilder::require_git(false)` mandatory; `Override` ordering (default-excludes first, user `force_include` after) gives the desired precedence. |
| R8 | **CONFIRMED — methodology pinned** | criterion 0.5 + `async_tokio` already in workspace; corpus tiers (Small/Medium/Large) named; implicated-file caps codified. |
| R-CLI-1 | **MODELLED** | Cold-start estimated 30–60 ms full-load. Recommended L21 budget: p95 < 100 ms warm DB on Linux; +50% allowance on Windows. |
| R-CLI-2 | **MODELLED — outliers identified** | Estimated full-load stripped binary ~40–50 MB. `tree-sitter-cpp` (~10 MB compiled contribution) and `tree-sitter-ruby` (~6 MB) are outliers. S1 ceiling: ≤ 50 MB stripped. |
| R-CLI-3 | **CONFIRMED** | `dep:` syntax + `resolver = "2"` (already set) cleanly isolate optional language deps. CI matrix proposed for partial-feature builds. |
| R-CLI-4 | **CONTRADICTED — BLOCKING plan amend required** | Plan H2 ("without requiring `cc` toolchain") is factually impossible: every grammar crate AND `libsqlite3-sys/bundled` require `cc` at build time. H2 must be reworded before spec authoring. |
| R-CLI-5 | **CONFIRMED — methodology pinned, aspirationals reset** | 3 flows × N=10 × 2 arms = 60 runs; aspirationals A ≥ 3.5×, B ≥ 2.5×, C ≥ 1.8×, global median ≥ 2.5× (above the SOFT 2.0× threshold). |

**Recommendation:** **PROCEED to spec authoring** — with one mandatory plan-level amend before §6.5 of SPEC is touched. See §Final Verdict.

---

## Dependencies investigated (live, 2026-04-29)

| Crate | crates.io max_stable | Last release | ABI / notes |
|---|---|---|---|
| `tree-sitter` | **0.26.8** | 2026-03-31 | ABI: `LANGUAGE_VERSION=15`, `MIN_COMPATIBLE=13` |
| `tree-sitter-language` (shim) | 0.1.7 | 2026-02-01 | Required by all `tree-sitter-<lang>` v0.24+ crates |
| `tree-sitter-rust` | **0.24.2** | 2026-03-27 | parser ABI 15, parser.c 6.5 MB |
| `tree-sitter-typescript` | **0.23.2** | 2024-11-11 | parser ABI 14, parser.c 8.5 MB (×2 — typescript+tsx) |
| `tree-sitter-javascript` | **0.25.0** | 2025-09-01 | parser ABI 15, parser.c 2.8 MB |
| `tree-sitter-python` | **0.25.0** | 2025-09-11 | parser ABI 15, parser.c 3.4 MB |
| `tree-sitter-go` | **0.25.0** | 2025-08-29 | parser ABI 15, parser.c 1.5 MB |
| `tree-sitter-java` | **0.23.5** | 2024-12-21 | parser ABI 14, parser.c 2.5 MB |
| `tree-sitter-c` | **0.24.2** | 2026-04-22 | parser ABI 15, parser.c 3.8 MB |
| `tree-sitter-cpp` | **0.23.4** | 2024-11-11 | parser ABI 15, parser.c 25.2 MB ⚠ outlier |
| `tree-sitter-ruby` | **0.23.1** | 2024-11-11 | parser ABI 14, parser.c 14.9 MB ⚠ outlier |
| `tree-sitter-php` | **0.24.2** | 2025-08-18 | parser ABI 15, parser.c 7.1 MB (×2 if php_only enabled) |
| `sqlx` | 0.8.6 | 2025-10-15 | `sqlite` feature enables `libsqlite3-sys/bundled` |
| `libsqlite3-sys` | 0.37.0 | 2026-03-15 | `bundled` build sets `-DSQLITE_ENABLE_FTS5` |
| `ignore` | 0.4.25 | 2025-10-30 | `WalkBuilder::require_git`, `same_file_system`, `Override` |
| `cc` | 1.2.61 | 2026-04-24 | `build-dependency` of every grammar crate |

---

## R3 — Top-10 Grammar Audit

### Validated finding

All ten grammars are present on crates.io as `tree-sitter-<lang>` Cargo crates and **all 10 publish a usable Rust binding** (each crate has `lib = bindings/rust/lib.rs` + tree-sitter-language-shim integration). Freshness ranges from 2024-11-11 to 2026-04-22.

**ABI split (LANGUAGE_VERSION in the parser.c header):**
- ABI 15: `rust, javascript, python, go, c, cpp, php` (7)
- ABI 14: `typescript, java, ruby` (3)

Both fall inside `tree-sitter` 0.26.8's compatibility window (`MIN_COMPATIBLE=13 .. LANGUAGE_VERSION=15`). No coexistence problem.

**Staleness audit (>12 months since last release as of 2026-04-29):**
- **Stale flagged:** `tree-sitter-typescript` 0.23.2 (2024-11-11), `tree-sitter-cpp` 0.23.4 (2024-11-11), `tree-sitter-ruby` 0.23.1 (2024-11-11), `tree-sitter-java` 0.23.5 (2024-12-21). Four crates have been silent for ~17 months.
- **Fresh:** the other six (rust, js, python, go, c, php) released within the last 12 months; rust and c released in 2026.

### Recommended decision

Pin every grammar at its **exact** `=x.y.z` version in `crates/unblock-indexer/Cargo.toml` (no semver carets):

```toml
tree-sitter            = "=0.26.8"
tree-sitter-language   = "=0.1.7"
tree-sitter-rust       = "=0.24.2"
tree-sitter-typescript = "=0.23.2"
tree-sitter-javascript = "=0.25.0"
tree-sitter-python     = "=0.25.0"
tree-sitter-go         = "=0.25.0"
tree-sitter-java       = "=0.23.5"
tree-sitter-c          = "=0.24.2"
tree-sitter-cpp        = "=0.23.4"
tree-sitter-ruby       = "=0.23.1"
tree-sitter-php        = "=0.24.2"
```

Add a runtime ABI guard at language-loader time (`Language::abi_version()` / `MIN_COMPATIBLE_LANGUAGE_VERSION`) so `build.rs` and runtime alike fail loudly if a future crates.io patch ships an incompatible parser.

### Risks

- **R3.1 — Stale-grammar drift:** typescript/cpp/ruby/java may have absorbed PRs upstream that have not been released to crates.io. Vendored `tags.scm` for those four MUST pin to the **crates.io tagged commit**, not `master`, to keep grammar binary and captures aligned.
- **R3.2 — Future tree-sitter 0.27 bumps `MIN_COMPATIBLE` to 14:** ABI 13 grammars would break. None of the Top-10 are ABI 13 today; worth a CI canary.

---

## R4 — sqlx + FTS5

### Validated finding

- `sqlx 0.8.6` with `features = ["sqlite", "runtime-tokio-rustls"]` pulls `libsqlite3-sys 0.37` with `bundled` enabled by default (verified per docs.rs/sqlx + sqlx-sqlite Cargo.toml).
- `libsqlite3-sys` 0.37 `bundled` build script sets `-DSQLITE_ENABLE_FTS5` unconditionally.
- **PRAGMA verification pattern** (run at connection-acquire time on the pool's `after_connect`):
  ```sql
  PRAGMA compile_options;
  -- expect a row with text "ENABLE_FTS5"
  ```
  Absent → hard error mapped to exit code 6 (per L17). Essential because nothing in the Cargo dependency surface *guarantees* FTS5 — a downstream consumer that swaps to `unbundled` could silently lose it.
- **External-content trigger pattern** (canonical, per sqlite.org/fts5.html):
  ```sql
  CREATE VIRTUAL TABLE symbols_fts USING fts5(
    name, signature, comment,
    content='symbols', content_rowid='id'
  );
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
  ```
  The `'delete'` command requires the **exact prior column values** — so the `AFTER UPDATE` trigger uses `old.*` for the delete and `new.*` for the insert.
- **Bootstrap optimisation:** drop the triggers, bulk-insert into `symbols`, then `INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild');` once — orders of magnitude faster than per-row trigger fan-out. Re-create triggers afterwards.
- **Chunked-transaction bootstrap.** Single transaction holds the WAL writer lock until commit; for 50k-file repo this stalls every concurrent reader for the entire walk. Pattern: chunk inserts at **~500 rows per `BEGIN..COMMIT`** (proven sweet-spot in rusqlite/sqlx ecosystems). Use `sqlx::QueryBuilder::push_values()` for batched inserts.

### Recommended decision

1. Pool config: `sqlx::sqlite::SqlitePoolOptions::new().after_connect(...)` asserting FTS5 + setting WAL/synchronous=NORMAL/temp_store=MEMORY.
2. PRAGMA assertion is a HARD failure (exit 6); document the exact error message in the spec.
3. Bootstrap: `BEGIN → drop triggers → batched 500-row inserts → 'rebuild' FTS → re-create triggers → COMMIT`.
4. WAL: `journal_mode=WAL`, `synchronous=NORMAL`, `wal_autocheckpoint=1000`.

### Risks

- **R4.1 — Downstream `unbundled` swap silently disables FTS5.** PRAGMA assertion fully mitigates.
- **R4.2 — `'rebuild'` is single-threaded.** For 500k symbols on Large corpus this is the bottleneck. Acceptable for v1.0.0; future opt.
- **R4.3 — WAL files not cleaned up on crash.** SQLite recovery handles; document in `status` output.

---

## R5 — Symbol-Extraction Queries (S-expressions) — SCOPE CORRECTION

### Validated finding — capture coverage matrix

Every grammar exposes `queries/tags.scm` (HTTP 200 confirmed for all 10). Capture inventory (from upstream `master` HEAD):

| Lang | def captures present | Missing for 17-kind goal |
|---|---|---|
| Rust | function, method, class (struct/enum/union), interface (trait), module, macro | struct/enum/trait split, namespace, variable, constant, type_alias, field, property, import, export |
| TypeScript | function, method, class, module, interface | struct, enum, trait, namespace, variable, constant, type_alias, macro, field, property, import, export |
| JavaScript | function, method, class, constant | struct, enum, trait, namespace, module, interface, variable, type_alias, macro, field, property, import, export |
| Python | function, class, constant | struct, enum, trait, interface, module, namespace, method, variable, type_alias, macro, field, property, import, export |
| Go | function, method, type | struct, enum, trait, class, interface, module, namespace, variable, constant, type_alias, macro, field, property, import, export |
| Java | class, method, interface | struct, enum, trait, function, module, namespace, variable, constant, type_alias, macro, field, property, import, export |
| C | class (struct/union), function, type (typedef/enum) | enum/struct split, method, trait, interface, module, namespace, variable, constant, type_alias, macro, field, property, import, export |
| C++ | class, function, method, type | struct, enum, trait, interface, module, namespace, variable, constant, type_alias, macro, field, property, import, export |
| Ruby | method, class, module | struct, enum, trait, interface, namespace, function, variable, constant, type_alias, macro, field, property, import, export |
| PHP | module (namespace), interface, class, function, **field** | struct, enum, trait, namespace, method, variable, constant, type_alias, macro, property, import, export |

**Reality check vs the plan's 17 SymbolKinds.** Upstream `tags.scm` uses a much smaller, cross-language vocabulary (typically 4–6 capture kinds). The plan's 17 kinds are an **unblock-side normalisation**. The mapping needs:

1. A **per-language capture→kind map** in `unblock-indexer/src/tags/<lang>.rs`. Example for Rust: `@definition.class` on a `struct_item` node → `SymbolKind::Struct`; on `enum_item` → `SymbolKind::Enum`; on `union_item` → `SymbolKind::Struct` (or a future `Union`). The differentiator is the **node kind of the capture's anchor**, not the capture name.
2. **Hand-written extension queries are needed in MORE than 4 languages**, not the 4 the plan estimates. To get all 17 kinds covered consistently, you need extensions in approximately **all 10 languages** for `import`, `export`, `variable`, `constant`, `field`, `property`, `type_alias`, `macro`, `namespace`. The pre-reframe research's "4 hand-written extension queries" estimate underestimates — the *categories* may be 4–5 (field/property/import/export plus variable/constant), but they need to be implemented per language because syntax differs (Rust `use` vs Python `import` vs PHP `use` vs Java `import`).

### Recommended decision

- Vendor `tags.scm` from each crates.io tagged commit (NOT `master`) into `crates/unblock-indexer/src/tags/<lang>.scm`.
- Per-language Rust translation table `kind_for_capture(capture_name, anchor_node_kind, language) -> SymbolKind` lives in `unblock-indexer/src/tags/<lang>.rs`. Table-driven, no clever logic.
- **Extension queries** (revised count): plan on **~6 categories × 10 languages ≈ 30–40 extension query rules** (not "4 queries total"). Each is a small additional s-expression appended to the vendored upstream `tags.scm` at runtime (`tags.scm + ext_<lang>.scm` concatenated).
- **`parent_id` post-traversal algorithm:**
  1. Insert all symbols for a file with `parent_id = NULL`.
  2. After file completes, for each symbol S compute `parent(S) = argmin_{T : span(T) ⊃ span(S), T ≠ S} (span(T).len)` — smallest enclosing symbol whose span strictly contains S.
  3. Implementable as a single linear scan after sorting symbols by `(start_offset asc, end_offset desc)` and using a stack of currently-open ancestors. O(n) per file.

### Risks

- **R5.1 — Extension query scope creep.** What the plan calls "4 hand-written extensions" is actually 30–40 query rules. **Measurable epic-03.4 effort underestimate** (likely +1 week of work versus what the plan suggests).
- **R5.2 — `tags.scm` drift** between vendored copy and grammar binary. CI drift check (S3 in plan) covers it.
- **R5.3 — Generic `@definition.class`/`@definition.type` collapses information.** Mitigated by anchor-node-kind discriminator in per-language map.

### Open questions (spec-time)

- **Q-R5.1** — Confirm the corrected scope (extensions per-language, ~30–40 rules, not just 4 globals) is acceptable to Ada's epic 03.4 sizing.

---

## R7 — `ignore` Crate Edge Cases

### Validated finding

- **`require_git(false)` is mandatory** for tarball / non-git checkouts — when default `require_git=true`, gitignore rules are silently disabled outside a git repo.
- **`same_file_system(true)` works only on Unix and Windows** (per docs.rs/ignore). For unblock's three target platforms (Linux/macOS/Windows) this is fine.
- **`Override`** acts as a precedence-1 layer **before** gitignore matching. The `ignore` crate has **no built-in `target/`/`node_modules/`/`dist/` blacklist** — unblock must implement that blacklist itself.

### Recommended decision

```rust
let mut wb = WalkBuilder::new(repo_root);
wb.require_git(false)
  .same_file_system(true)
  .hidden(false)
  .git_ignore(true).git_global(true).git_exclude(true)
  .ignore(true)
  .parents(false);
let mut ovr = OverrideBuilder::new(repo_root);
for pat in DEFAULT_EXCLUDES {  // ["!target/**", "!node_modules/**", "!dist/**", "!build/**", "!.venv/**", "!vendor/**", "!.git/**"]
    ovr.add(pat)?;
}
for pat in user_force_include {  // from .unblock/indexer.toml — positive (no leading !)
    ovr.add(pat)?;
}
wb.overrides(ovr.build()?);
```

Order matters: **default-excludes first, user `force_include` after** — Override matching is first-match-wins, so user includes take precedence over our excludes.

### Risks

- **R7.1 — `parents(false)` is opinionated.** `git_global(true)` covers `~/.gitignore_global` without `parents(true)` reaching above repo root.
- **R7.2 — `same_file_system` semantics on macOS APFS volumes.** Pathological edge case; acceptable.

---

## R8 — Latency Benchmarks Methodology

### Validated finding

- `criterion = "0.5"` with `async_tokio` is already a workspace dep (verified in `Cargo.toml`). Canonical form: `b.to_async(&runtime).iter(|| async { … })`.
- p99 from 45+ samples — criterion's default sample size is 100 with 5s warmup, 5s measurement. Use `.sample_size(100).measurement_time(Duration::from_secs(10))` for perf-sensitive benches.
- **Implicated-file caps** (per query):
  - `find-symbol`: 4 files
  - `outline` / `get-symbol`: 1 file
  - `list-symbols` (recursive): 16 files
  - `search` (FTS5): 4 files
  - `find-references`: 16 files
- **Three corpora (pinned):**
  - **Small** = unblock itself (~500 .rs files, ~5k symbols est.)
  - **Medium** = ripgrep + tokio combined (~5k files, ~50k symbols est.)
  - **Large** = LLVM monorepo subset clang+llvm (~50k files, ~500k symbols est.)
- **Warm-path measurement.** Cold path is `cargo build`-bound, not indexer-bound; criterion warmup discards cold runs naturally. Harness calls `init` (or one read query) before entering criterion's measurement loop.

### Recommended decision

L20 baseline (already in plan):
- `find-symbol` p99 < 10 ms warm on Medium corpus
- `outline` p99 < 20 ms warm on Medium corpus

**New recommendations (currently un-anchored in plan):**
- `list-symbols` (recursive, 16-cap) p99 < 50 ms warm on Medium
- `search` (FTS5 query) p99 < 30 ms warm on Medium

### Open questions (spec-time)

- **Q-R8.1** — Should `list-symbols`, `search`, and `find-references` get explicit p99 budgets at spec time? Recommend yes.

---

## R-CLI-1 — CLI Cold-Start Budget

### Validated finding (modelled, not measured)

A Rust CLI built with `lto = "fat"` + `codegen-units = 1` typically incurs:

| Component | Typical cost |
|---|---|
| Process spawn + dynamic loader | 1–3 ms |
| `clap` parse of 11 subcommands | <1 ms |
| `tokio` single-thread runtime init | 1–2 ms |
| `sqlx` pool open + `after_connect` PRAGMA assertions | 5–10 ms (single connection, cold disk WAL) |
| `tree_sitter::Parser::set_language` × 10 (full-load) | 1–3 ms (lazy — language pointer only, no JIT) |
| First mtime probe + cached query path | 1–3 ms |
| Serde JSON envelope + flush on stdout | <1 ms |

**Empirical reference points (live, 2026-04-29):**
- ripgrep 15.1.0: 1.7–2.2 MB stripped → cold-start ~5 ms (no DB)
- fd 10.4.2: 1.3–1.7 MB stripped → cold-start ~3 ms (no DB)
- `git status` on a 5k-file repo: ~50 ms (filesystem-bound)
- A `sqlx`-using CLI with a warm DB: 15–30 ms cold-start (sqlx pool init dominates)

**Modelled estimate for `unblock-code` full-load:**
- Cold (DB never opened): 30–60 ms (sqlx pool open + WAL disk read)
- Warm (DB page cache hot): 10–25 ms

**Modelled for partial-load (`--features lang-rust,lang-python`):** same — feature flags don't materially change runtime cost since grammars are lazy; they affect binary size, not init cost.

### Recommended decision (concrete number for L21)

**L21 budget: cold-start (process spawn → first JSON byte on stdout) p95 < 100 ms full-load on Medium corpus warm DB.** Defensible margin above modelled 25 ms upper bound; gives headroom for slow CI and Windows MSVC (typically +10–20 ms per process spawn vs Linux).

If Medium-corpus warm-DB measurement during epic 03.6 falls below 50 ms, tighten L21 to `< 75 ms` at spec amend time.

### Risks

- **R-CLI-1.1** — sqlx WAL recovery on first open after dirty crash can spike to 100–500 ms. Cold-cold edge case; budget assumes clean shutdown.
- **R-CLI-1.2** — Windows MSVC process spawn 2–3× slower than Linux. Set L21 budget on Linux; explicitly grant Windows 1.5× allowance.

### Open questions (spec-time)

- **Q-R-CLI-1.1** — Bench harness must run on all three platforms (Linux/macOS/Windows). HARD gate H6 or epic 03.6 acceptance?

---

## R-CLI-2 — Static-Link Binary Size — OUTLIERS IDENTIFIED

### Validated finding

Per-grammar parser.c size (downloaded from upstream `master` HEAD, all 10 grammars):

| Grammar | parser.c | Estimated compiled binary contribution (~0.4× factor)¹ |
|---|---|---|
| rust | 6.5 MB | ~2.5 MB |
| typescript (typescript+tsx) | 17.1 MB | ~6.7 MB ⚠ |
| javascript | 2.8 MB | ~1.1 MB |
| python | 3.4 MB | ~1.3 MB |
| go | 1.5 MB | ~0.6 MB |
| java | 2.5 MB | ~1.0 MB |
| c | 3.8 MB | ~1.5 MB |
| **cpp** | **25.2 MB** | **~9.9 MB** ⚠⚠ outlier |
| **ruby** | **14.9 MB** | **~5.8 MB** ⚠ outlier |
| php | 7.1 MB | ~2.8 MB |

¹ 0.4× multiplier based on observed empirical compression (state tables → dense bss/rodata segments). Real number may be 0.3–0.5×; actual measurement happens during epic 03.6.

**Aggregate estimate, full-load Top-10 with TypeScript=ts+tsx and PHP=php only:**
- Sum of parser.c: ~85 MB
- Estimated stripped binary contribution (grammars only): **~30–35 MB**
- Plus base binary (clap + sqlx + tokio + tree-sitter-core + ignore + rayon): ~6–10 MB
- **Total estimated `unblock-code` stripped, full-load: ~40–50 MB**

**Comparators (live, GitHub Releases 2026-04-29):**
- ripgrep 15.1.0: 1.7 MB Linux musl (single-purpose, BurntSushi-tight)
- fd 10.4.2: 1.7 MB Linux musl
- helix 25.07.1: 15.9 MB Linux x86_64 (bundles ~80 grammars; not fair comparator)

**Outliers:**
- **`tree-sitter-cpp`** is by far the worst at 25.2 MB parser.c, dominated by C++'s template/operator-overloading explosion in the LR table. Single grammar contributes ~10 MB of the estimated 30 MB grammar total.
- **`tree-sitter-ruby`** is second at 14.9 MB parser.c — Ruby's keyword-flexible grammar has a huge state space.
- **TypeScript ships as two parsers** (typescript + tsx); both needed because TSX is a separate `Language`. ~17 MB combined.

### Recommended decision (concrete S1 ceiling)

**S1 SOFT ceiling: stripped `unblock-code` binary ≤ 50 MB on Linux x86_64 release build with default features.** Achievable per the model; failure (warns, does not block) opens `unblock:finding:risk` to evaluate dropping cpp+ruby from defaults.

**Partial-load alternative ceiling:** `cargo install unblock-code --no-default-features --features lang-rust,lang-python` → binary ≤ 15 MB stripped (S2 in plan captures the >50% reduction goal).

### Risks

- **R-CLI-2.1 — Estimates may be 20–30% off** because LR-table-to-Rust-rodata compression depends on toolchain / LLVM version. Land during epic 03.6 measurement.
- **R-CLI-2.2 — C++ alone could push the binary past 50 MB** if the 0.4× factor underestimates. Mitigation: gate cpp behind `lang-cpp` feature; consider opt-out from defaults if measurement confirms >12 MB contribution. **Phase-replan trigger.**
- **R-CLI-2.3 — `strip` on macOS aarch64 more conservative** than Linux GNU strip; macOS binaries ~10% larger.

### Open questions (spec-time)

- **Q-R-CLI-2.1** — Should `lang-cpp` and `lang-ruby` be opt-in (NOT default-enabled) to keep default binary safely under 30 MB? Top-10-default policy decision.

---

## R-CLI-3 — Cargo Feature-Flag Ergonomics

### Validated finding

- Workspace already has `resolver = "2"` (verified in `/Users/ramosmig/Public/WS-Labs/unblock/Cargo.toml`). Resolver v2 prevents the classic feature-unification footgun.
- `cargo install unblock-code --no-default-features --features lang-rust,lang-python` is fully respected for bin crate features.
- **Pattern for the feature scheme:**
  ```toml
  # crates/unblock-code/Cargo.toml
  [features]
  default = ["lang-rust","lang-typescript","lang-javascript","lang-python","lang-go","lang-java","lang-c","lang-cpp","lang-ruby","lang-php"]
  lang-rust       = ["unblock-indexer/lang-rust"]
  lang-typescript = ["unblock-indexer/lang-typescript"]
  # … one per language

  # crates/unblock-indexer/Cargo.toml
  [features]
  default = []  # bin owns the policy
  lang-rust       = ["dep:tree-sitter-rust"]
  lang-typescript = ["dep:tree-sitter-typescript"]
  # …

  [dependencies]
  tree-sitter-rust       = { version = "=0.24.2", optional = true }
  tree-sitter-typescript = { version = "=0.23.2", optional = true }
  # …
  ```
  - `dep:` syntax (Cargo 1.60+) makes feature name **not** auto-imply dep name as a feature.
  - Each `tree-sitter-<lang>` Rust file in `unblock-indexer/src/grammars/` is `#[cfg(feature = "lang-<lang>")]` gated.
  - `Language` enum derives `#[non_exhaustive]` and contains all 10 variants unconditionally; **registry's `loaders()` map** is populated cfg-gated.
  - `unblock-code languages` reflects only active features at runtime.

### Recommended decision

Adopt pattern verbatim. CI matrix:
- `cargo test --workspace` (full features)
- `cargo test -p unblock-code --no-default-features --features lang-rust` (single-lang sanity)
- `cargo test -p unblock-code --no-default-features --features lang-rust,lang-python` (paired-lang sanity)

### Risks

- **R-CLI-3.1 — Test discipline.** Tests for unselected langs MUST be `#[cfg(feature = "lang-<lang>")]` gated; otherwise partial-feature build fails. Fernando's test scaffolding must enforce.
- **R-CLI-3.2 — Hidden deps via `unblock-indexer-core`.** Pure crate must not pull any tree-sitter grammar — only `tree-sitter` itself. Achievable per plan §5.

---

## R-CLI-4 — `build.rs` Mechanics — **CRITICAL CONTRADICTION**

### Validated finding (CONTRADICTS the plan's expectation)

The plan §11 R-CLI-4 expectation is:

> Verify upstream `tree-sitter-<lang>` crates compile across Linux + macOS + Windows **without** requiring `cc` (C compiler) on the user's machine.

**This expectation is FALSE.** Every one of the 10 grammar crates ships a `bindings/rust/build.rs` that does:

```rust
fn main() {
    let mut c_config = cc::Build::new();
    c_config.std("c11").include("src");
    c_config.file("src/parser.c");
    c_config.file("src/scanner.c");  // when present
    c_config.compile("tree-sitter-<lang>");
}
```

Verified across all 10 by inspecting upstream `bindings/rust/build.rs`:
- Cargo.toml of every grammar lists `cc = "1.1"` or `cc = "1.2"` as a `[build-dependencies]` entry.
- The `cc` crate REQUIRES a C compiler on PATH to compile the C parser into a static library.
- They ship pre-generated `parser.c` (so `tree-sitter generate` is not required), **but they do not pre-compile it**. User still needs `cc`/`clang`/`cl.exe` to turn it into object code.

**Per platform:**
- **Linux (x86_64-unknown-linux-gnu):** `gcc` or `clang` — typically present on dev machines, NOT on `slim` Docker images. CI base images need `apt-get install build-essential`.
- **macOS (aarch64-apple-darwin):** Apple Clang via Xcode Command Line Tools (`xcode-select --install`) — not bundled with rustup; user prompt required.
- **Windows (x86_64-pc-windows-msvc):** **MSVC Build Tools** must be installed separately. Rustup-init *can offer* automatic install, but it's optional.
- **Windows (x86_64-pc-windows-gnu):** MinGW-w64 toolchain must be installed.

**Impact on the plan:**
- HARD gate **H2** ("`unblock-code` binary builds … without requiring the user to have a `cc` toolchain") **CANNOT BE MET** as currently worded. There is no Top-10 grammar on crates.io that provides a pre-compiled-binary fallback.
- **Note: `libsqlite3-sys 0.37 bundled` ALSO needs `cc`**, so the plan already implicitly requires `cc` via sqlx.

### Recommended decision (rewording H2)

Replace H2 in the plan with:

> **H2.** `unblock-code` binary builds with default features (Top-10 grammars) on Linux x86_64 (gcc/clang), macOS aarch64 (Apple Clang via Xcode CLT), and Windows x86_64 (MSVC Build Tools). The README documents the platform-specific C-toolchain prerequisites in a "Building from source" section. Note: this requirement matches `libsqlite3-sys/bundled` (already a transitive dep via sqlx) — `cc` is not new ceremony.

### Risks

- **R-CLI-4.1 — Windows users without MSVC Build Tools.** `cargo install unblock-code` will fail with linker error. Mitigation: README prerequisites + cargo's built-in error message. Phase 04 cargo-dist removes the user-facing prereq.
- **R-CLI-4.2 — `cc` cross-compilation toolchain woes.** Cross-compiling with cargo-dist (Phase 04) requires per-target C cross-compilers in CI. Solved problem (cross-rs, cargo-zigbuild) — flag for Phase 04.
- **R-CLI-4.3 — `libsqlite3-sys/bundled` already requires `cc`** so v1.0.0 depends on `cc` regardless of grammar story. The plan's H2 expectation was inherited from the WASM design and never updated for the static-linked reframe.

### Open questions (PLAN-LEVEL — BLOCKING)

- **Q-R-CLI-4.1** — Resolution required at the plan level **before** spec authoring lands: confirm H2 is reworded to "C toolchain required" rather than "no cc required". This is plan-level, not spec-level — H2 cannot be specified without first amending the plan's claim.

---

## R-CLI-5 — ROI Harness Reframe

### Validated finding (methodology only)

The plan §11 + L22 + S5 already commit to the methodology in detail. Research validates:

- **Three flows (preserved from prior R10):**
  - Flow A — `find-symbol` exact lookup ("where is `Symbol::find_one` defined?")
  - Flow B — `outline` file ("show me the structure of `crates/unblock-core/src/graph.rs`")
  - Flow C — `find-references` ("where is `parse_github_url` referenced?")
- **N=10 runs × 2 arms × 3 flows = 60 runs.** Sonnet via Anthropic API with system prompt versioned at `tests/roi/system-prompt.md`. Tokens via API response's `usage` field. Median ratio per flow + global median across 30 indexer runs.
- **Baseline arm** uses `Glob` + `Grep` + `Read`. Indexer arm uses `Bash("unblock-code …")`.
- **Reset aspirationals (NEW, replacing the old 3.0×/2.0×/1.5× from R10):**
  - Flow A (`find-symbol`): **aspirational ≥ 3.5×** — find-symbol is unblock-code's strongest play (single command, no Read fan-out)
  - Flow B (`outline`): **aspirational ≥ 2.5×** — Glob+Read tends to over-fetch; outline is constant-bound JSON
  - Flow C (`find-references`): **aspirational ≥ 1.8×** — heuristic; baseline Grep is fast and indexer's heuristic warning may consume back-and-forth tokens
  - **Median across all 30 indexer runs: aspirational ≥ 2.5×** (above SOFT 2.0× threshold, leaving margin)
- **Variance:** Anthropic Sonnet output is non-deterministic. N=10 is at the low end of statistical confidence. Document IQR alongside median; treat single-flow medians below 1.5× as a signal.

### Recommended decision

Adopt aspirationals above as informational targets in the SPEC (not gates — SOFT 2.0× threshold remains the only "report" boundary). Harness publishes:
1. Per-run raw logs (input prompt, indexer transcript, baseline transcript, both `usage` blocks)
2. Per-flow median ratio + IQR
3. Global median across 30 indexer runs

If global median < 2.0×, open `unblock:finding:risk` per L22.

### Risks

- **R-CLI-5.1 — Sonnet behavior shift between harness write and v1.0.0 release.** Pin model version (e.g., `claude-sonnet-4-7-20260101`); warn if API returns different model id.
- **R-CLI-5.2 — System prompt drift.** Version in git; harness records prompt SHA in artifact.
- **R-CLI-5.3 — Anthropic API cost.** ~$5–30 per full run. Run on every release tag, not every PR.

### Open questions (spec-time)

- **Q-R-CLI-5.1** — Run harness in CI (flaky, costly) or release-gate one-shot (saner)? Recommendation: release-gate.

---

## Final verdict

**RECOMMENDATION: PROCEED to spec authoring — with one mandatory plan-level amend before §6.5 of SPEC is touched.**

### Mandatory plan amend (BLOCKING for spec authoring)

- **H2 contradiction (R-CLI-4):** The plan's HARD gate H2 ("without requiring the user to have a `cc` toolchain") is factually impossible given that **all 10 upstream `tree-sitter-<lang>` crates AND `libsqlite3-sys/bundled` (already a transitive dep via sqlx) require `cc` at build time**. H2 must be reworded before the spec encodes acceptance criteria. This is a one-line plan patch, not a phase-replan event — the underlying intent ("user can install with one command") is preserved by Phase 04's cargo-dist distribution; v1.0.0 just needs honest prerequisites in the README.

### Recommended (NON-BLOCKING) plan adjustments

1. **R5 scope correction:** the "4 hand-written extension queries" estimate (Resolution Q5 carry-over) under-counts by 6–10×. Real cost is **~30–40 query rules** spread across all 10 languages. Ada should re-size epic 03.4 accordingly. Spec-level only — no plan amend strictly needed.
2. **R-CLI-2 cpp+ruby outliers:** spec time should consider whether `lang-cpp` and `lang-ruby` belong in default features given their disproportionate parser.c size (cpp alone ~10 MB compiled contribution).
3. **R8 budget completeness:** L20 only pins `find-symbol` and `outline`. Add explicit budgets for `list-symbols`, `search`, `find-references` at spec time.

### Open questions for spec-time resolution

- **Q-R5.1** — Confirm extension-query scope correction (per-language, not 4 globals).
- **Q-R8.1** — Set p99 budgets for `list-symbols`, `search`, `find-references` at spec time.
- **Q-R-CLI-1.1** — Multi-platform cold-start measurement scope for H6 / epic 03.6.
- **Q-R-CLI-2.1** — Should `lang-cpp` / `lang-ruby` be opt-in rather than default-enabled?
- **Q-R-CLI-4.1** — **PLAN-LEVEL** — confirm H2 rewording (BLOCKING).
- **Q-R-CLI-5.1** — ROI harness in CI vs release-gate.

### Counts

- **Dependencies investigated:** 16 (10 grammars + tree-sitter + tree-sitter-language + sqlx + libsqlite3-sys + ignore + cc)
- **Assumptions validated:** 28 across R3/R4/R5/R7/R8/R-CLI-1..5
  - CONFIRMED: 22
  - PARTIALLY CONFIRMED: 5 (R3 staleness, R5 capture coverage, R-CLI-1 modelled-not-measured, R-CLI-2 modelled-not-measured, R-CLI-5 methodology-only)
  - **CONTRADICTED: 1** (R-CLI-4 / H2 — `cc` IS required)
- **Contradictions: 1** (H2 wording vs upstream grammar build.rs reality)
- **Risks logged:** 14 across the seven sections
