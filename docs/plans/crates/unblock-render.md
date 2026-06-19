# unblock-render — File-level Plan

- **Status:** DRAFT — for review. Conforms to `docs/plans/01-design-spine.md` (AUTHORITATIVE) + `docs/PRD.md` (APPROVED) + `docs/plans/00-roadmap.md`. Source of truth: **PRD APPROVED v1.1** (G-19).
- **Date:** 2026-06-19
- **One-line purpose:** Output/format layer — turn engine/domain result types into byte-stable serialized output (`json` / `robot` / `plain` / `csv` / `markdown`; `toon` feature-gated v1.1) behind a single `Renderer` trait. **Reduced under D7** (no rich terminal stack: no `rich_rust`/`crossterm`/`indicatif`).
- **Layer:** L6.
- **Depends on:** `unblock-model` (L0), `unblock-error` (L0) **only** — per PRD §8.1 / spine §0 layering. Plus leaf crates: `serde`/`serde_json` (json/robot), `csv` (RFC-4180 escaping), `chrono` (timestamp formatting). **No** dependency on `engine`/`storage`/`mcp`/`cli` (would be a back-edge — forbidden by NFR-15). The MCP and CLI crates (L7) depend **on** render, not the reverse.

> **Critical scope note (D7 / spine §"render").** MCP is PRIMARY (D2) and rendering is "the MCP client's job" (D7). `unblock-mcp` returns structured `ToolOutput` (spine §5.3) as JSON/`JsonSchema` directly — it does **not** route through this crate's human formats. `unblock-render` therefore exists mainly for **`unblock-cli`** lifecycle/ops output (serve/migrate/doctor/version diagnostics) and for the small set of human-facing/CSV/markdown exports that are not the MCP structured surface. The render crate is deliberately thin in v1; the original's ~3.2k-LOC `format/` + ~2.3k-LOC `output/` rich stack collapses to a small trait + a handful of format backends. **This crate never writes to a file** (atomic JSONL export is `unblock-sync`'s job, FR-7/NFR-4) and never touches stdout/stderr directly — it returns `String`/bytes; the caller (cli) owns the stdout/stderr split (NFR-14).

---

## 1. Public API summary (what other crates import)

### v1
- `Renderer` trait — `render_<kind>(&self, value, opts) -> Result<RenderOutput, RenderError>` family behind one object-safe-ish dispatch (see §3 `src/renderer.rs`).
- `OutputFormat` — **RE-EXPORTED** from `unblock-model` (`pub use unblock_model::OutputFormat`), **not defined here** (G-7 / CF-J, spine §1.10). Variants `Json | Robot | Plain | Csv | Markdown` (`Toon` variant `#[cfg(feature = "toon")]`, behaviourally v1.1) and its full derive set (`Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq`) are owned by the model crate. Render adds the **parse/precedence helpers** locally (`FromStr`/`Display`, env-string parse mapping `UNBLOCK_OUTPUT_FORMAT`, D10) — implemented as inherent/extension helpers in `src/format.rs`, not a second enum.
- `RenderOptions` — color off (D7: always no-color/plain), max-width, CSV field selection, pretty-json toggle, timestamp format. **No TTY detection** (caller decides; render is pure).
- `RenderOutput` — `{ stdout: String, content_type: ContentType }` (pure value; caller routes to stdout, diagnostics to stderr — NFR-14). v1 has no stderr payload (errors are `Err`).
- `RenderError` (snafu, per-crate, D4) + `fn code(&self) -> unblock_error::ErrorCode`.
- `sanitize::{sanitize_inline, sanitize_text}` — terminal-control escaping for untrusted strings (ported from original `format/text.rs`; NFR-18 untrusted-input hygiene even for human output).
- `pick_format(cli, env, config_default) -> OutputFormat` — pure precedence resolver (CLI > env `UNBLOCK_OUTPUT_FORMAT` > config default > `Json` for cli-lifecycle). Mirrors FR-13 precedence shape but is a pure function (config values passed in; render does not read env/files itself — keeps it pure & testable).

### v1.1
- `toon` feature lights up `OutputFormat::Toon` + the `ToonRenderer` backend (D12, roadmap §2 "TOON output").
- Render support for the new v1.1 domain surface that the CLI/MCP may need to display: `Comment`, `EpicStatus`, label lists, `GateResult`, scheduler/coordination report value types (csv/markdown/plain views). Additive only.

### v1.3
- Large-result formatting helpers (streaming/chunked CSV + markdown writer over an iterator, not a `Vec`) for the 1M-issue / batch surface (roadmap §4, `unblock-render` ◐). Additive: `render_stream_csv(iter, …) -> impl` writer-based API alongside the v1 `Vec`-based one.

> **No v1.2 participation** (roadmap §7: render is blank for v1.2 — remote/replica sync touches storage/config/health, not formatting).

---

## 2. Crate-level conformance invariants

1. `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`, clippy pedantic (workspace lints) — spine §0.
2. Depends on **model + error only** (+ pure leaf format libs). No back-edge to L5/L7 (NFR-15; CI layering check).
3. **No I/O, no global state, no stdout/stderr writes** — every entry point returns `Result<RenderOutput, RenderError>` or `Result<String, RenderError>`. Caller owns the stdout/stderr split (NFR-14) and any file write (NFR-4 is sync's, not render's).
4. **Byte-deterministic** for a fixed input + fixed `RenderOptions` (FR-7 export determinism spirit; NFR-14 snapshot-stable). No HashMap iteration order leaks; timestamps formatted via a single fixed helper (RFC-3339 / `SecondsFormat::Secs`, UTC).
5. **Untrusted-string hygiene:** every human/text format passes user-controlled strings (title, labels, assignee, description, comment body) through `sanitize::*` before embedding (NFR-18). JSON/robot rely on `serde_json` escaping; CSV on RFC-4180 quoting.
6. `Robot` == compact single-line JSON to stdout (machine-readable, diagnostics elsewhere); `Json` == pretty JSON. Both reuse the same `serde_json` path; robot is just `to_writer` (no pretty).
7. TOON behind `#[cfg(feature = "toon")]`, default-off; default build has **zero** TOON transitive surface (NFR-10).

---

## 3. FILE BREAKDOWN

Legend for **Version**: `v1` introduced in v1; `v1.1` / `v1.3` introduced or changed in that release.

| Path | Responsibility | Key items (reference spine where noted) | Version | Tests |
|---|---|---|---|---|
| `Cargo.toml` | Crate manifest. Deps: `unblock-model`, `unblock-error`, `serde`, `serde_json`, `csv`, `chrono`. `[features] default=[]`, `toon=["dep:toon"]` (v1.1; feature name pinned **`toon`** per G-24b / spine §1.10). Workspace lints inherited. | feature table; `forbid(unsafe_code)` | v1 (toon feat added v1.1) | n/a (built by all suites) |
| `src/lib.rs` | Crate root: lint attrs, module decls, re-exports (the §1 public API). Crate-level docs with a usage doctest. | `pub use unblock_model::OutputFormat; pub use renderer::*; pub use options::*; pub use format::*; pub use error::*; pub use sanitize::{sanitize_inline, sanitize_text};` (`format::*` re-exports the parse/precedence helpers, **not** an `OutputFormat` enum — that comes from model, G-7) | v1 | doctest: render a 1-issue list as `Json` and assert it parses back; doctest compiles & runs |
| `src/error.rs` | Per-crate snafu error enum (D4) + `ErrorCode` mapping. | `#[derive(Snafu)] pub enum RenderError { Serialize{source}, Csv{source}, UnsupportedFormat{format}, FieldUnknown{field}, ToonEncode{..}(#[cfg(feature="toon")]) }`; `impl RenderError { pub fn code(&self) -> unblock_error::ErrorCode }` (Serialize→`JsonError`, Csv/IO→`IoError`, UnsupportedFormat/FieldUnknown→`ValidationFailed`). | v1 | unit: each variant → correct `ErrorCode` → correct exit code (cross-check spine §2.3 table); golden mapping snapshot |
| `src/format.rs` | `OutputFormat` parsing/precedence **helpers** (the enum itself is re-exported from `unblock-model`, G-7). | `pub use unblock_model::OutputFormat;` then `Display`/`FromStr` (case-insensitive; unknown → `RenderError::UnsupportedFormat`); `parse_env_value(&str)`; `pick_format(cli, env, cfg_default)` precedence resolver (CLI > env `UNBLOCK_OUTPUT_FORMAT` > config > default). Default for cli-lifecycle = `Json`. The `#[cfg(feature="toon")] Toon` variant is gated in the model definition; render only lights up its backend. | v1 (Toon variant cfg-gated; behaviour v1.1) | unit: round-trip `Display`↔`FromStr` for all variants; `parse_env_value` aliases (`text`→`Plain`, `plain`→`Plain`); precedence matrix (CLI overrides env overrides cfg overrides default) — proptest over the 4 layers; unknown string → error |
| `src/options.rs` | Render configuration value type (pure). | `struct RenderOptions { pretty_json: bool, max_width: Option<usize>, csv_fields: Option<Vec<String>>, timestamp_secs_only: bool }`; `Default`; builder-style setters. **No color/TTY field (D7 — always plain).** `struct RenderOutput { stdout: String, content_type: ContentType }`; `enum ContentType { Json, Text, Csv, Markdown, Toon }`. | v1 | unit: defaults are plain/no-color; builders mutate expected field |
| `src/sanitize.rs` | Terminal-control escaping for untrusted strings (port of original `format/text.rs::sanitize_terminal_*`, NFR-18). | `pub fn sanitize_inline(&str) -> Cow<str>` (escape all C0/C1/ESC/OSC; no `\n`/`\t`); `pub fn sanitize_text(&str) -> Cow<str>` (preserve `\n`/`\t`, escape the rest); `Cow`-borrow fast path when unchanged. | v1 | unit: ANSI/ESC/BEL/DEL escaped to `\u{..}`; plain ASCII borrows (no alloc); `\n`/`\t` preserved by `text`, escaped by `inline`; proptest: output never contains a raw control byte except allowed-layout; fuzz target (see §4) |
| `src/renderer.rs` | The `Renderer` trait + format dispatch. The crate's contract. | `trait Renderer { fn format(&self) -> OutputFormat; fn issue(&self, &Issue, &RenderOptions) -> Result<RenderOutput, RenderError>; fn issues(&self, &[Issue], …); fn counts(&self, &[CountBucket], …); fn dep_tree(&self, &DepTree, …); fn cycles(&self, &[Vec<String>], …); fn structured_error(&self, &StructuredError, …); fn diagnostics(&self, &DiagnosticReport, …) }` — all input types from spine §1/§3/§2.4. `fn renderer_for(fmt: OutputFormat, opts: RenderOptions) -> Box<dyn Renderer>` factory. | v1 | unit: factory returns a renderer whose `.format()` matches; trait-object dispatch compiles; per-method "renders without error" smoke for each (Issue/Vec/counts/tree/cycles/error/diag) × each format |
| `src/backend/mod.rs` | Backend submodule wiring. | `mod json; mod plain; mod csv_fmt; mod markdown; #[cfg(feature="toon")] mod toon;` | v1 (toon arm v1.1) | n/a |
| `src/backend/json.rs` | JSON + Robot backend (one impl, two modes). | `struct JsonRenderer { robot: bool, opts }` impl `Renderer`. Robot = `serde_json::to_string` (compact); Json = `to_string_pretty`. **Always-valid-JSON even on error** (FR-11): `structured_error` serializes `StructuredError` (spine §2.4). | v1 | insta snapshot: each result kind → pretty JSON + compact robot JSON; unit: `structured_error` produces valid JSON (parse-back); proptest: any `Issue` round-trips serialize→parse without panic; determinism: same input → identical bytes (no map-order flake) |
| `src/backend/plain.rs` | Plain text (human, no color — D7). Line-oriented issue/list/tree/count/error views. | `struct PlainRenderer { opts }`; helpers `format_issue_line`, `format_issue_long`, status/priority/type labels (ported from original `format/text.rs`, **stripped of color/icons-as-ANSI**; ascii labels only). All user strings via `sanitize_*`. | v1 | insta snapshot: issue line, long view, empty list, count table, dep-tree indent, cycle path, error block; unit: width truncation respects `max_width`; sanitize applied (inject ESC into title → escaped in output) |
| `src/backend/csv_fmt.rs` | CSV backend (RFC-4180). Port of original `format/csv.rs` (field selection + escaping). | `struct CsvRenderer { fields: Vec<&'static str>, opts }`; `DEFAULT_FIELDS`, `ALL_FIELDS`, `parse_fields`, `escape_field`, `get_field_value(&Issue, field)`; header + rows via the `csv` crate. Only `issues`/`issue` produce CSV; other kinds → `RenderError::UnsupportedFormat`. | v1 | insta snapshot: default-fields CSV, all-fields CSV; unit: comma/quote/newline escaping; unknown `--fields` → `FieldUnknown` error; empty list → header-only; proptest: every emitted row re-parses with the `csv` reader to the same cell count |
| `src/backend/markdown.rs` | Markdown backend (issue detail + list table + dep tree). Reduced port of original `format/markdown.rs` (drop syntax highlighting/`syntax.rs` — that was a rich-stack concern, D7). | `struct MarkdownRenderer { opts }`; `escape_markdown`, issue→detail section, issues→GFM table, dep tree→nested list. Sanitize + markdown-escape user strings. | v1 | insta snapshot: issue detail md, list table md, tree md; unit: pipe/backtick/bracket escaping in titles; empty list → "no issues" note |
| `src/backend/toon.rs` | TOON (token-optimized) backend — **feature-gated** (D12, roadmap v1.1). Wraps the `toon` encoder. | `#[cfg(feature="toon")] struct ToonRenderer { opts }` impl `Renderer`; reuses `serde_json::Value` bridge then TOON-encodes. | **v1.1** (file present but `#[cfg]`-empty in v1) | insta snapshot (cfg-gated): list + counts TOON output; unit: feature-off build has no `toon` symbol; determinism snapshot |
| `src/stream.rs` | Large-result streaming writers (CSV/markdown over an iterator + `io::Write`, not a `Vec`). | `pub fn render_stream_csv<W, I>(w: &mut W, rows: I, fields, opts)`; `render_stream_markdown_table(...)`. For 1M-issue/batch surface (roadmap §4). Additive; v1 `Vec` API stays. | **v1.3** | unit: streamed CSV byte-identical to the `Vec`-based `CsvRenderer` for the same input (equivalence test); large-input no-OOM smoke (10k rows into a sink); determinism |
| `tests/contract.rs` | **Render contract suite** — every `OutputFormat` × every result kind renders deterministically and (for json/robot/csv) re-parses. The crate's NFR-16-style contract gate. | parametrized over `OutputFormat::all()`; asserts: no panic, byte-stable across 2 runs, json/robot parse back to equal value, csv re-parses. | v1 (extended v1.1 for new kinds, v1.3 for stream) | the suite itself |
| `tests/snapshots/` (insta) | `insta` golden files for plain/markdown/csv/json/robot/toon outputs (CI `--check` gate, NFR-14/NFR-16). | one `.snap` per (format × representative fixture) | v1 (+toon v1.1) | `cargo insta test` |
| `tests/determinism.rs` | Byte-determinism + no-map-order-flake across all formats and repeated renders. | renders the same fixture N times, asserts identical bytes; shuffles input `Vec` order where order is defined and asserts stable sort where the spine defines one (ready hybrid sort lives in engine/policy, not here — render preserves caller order). | v1 | proptest |
| `tests/sanitize_fuzz_seed.rs` | Seed corpus + property bridge for the fuzz target (control-char escaping never panics / never emits raw control). | proptest over arbitrary strings → `sanitize_*` invariant. | v1 | proptest |
| `benches/render.rs` | `criterion` benches for the hot render paths (NFR-1 budget visibility for the cli/diagnostics path). | bench: render 1k / 10k issues as json, robot, csv, plain, markdown. Async not needed (render is sync/pure). | v1 (extended v1.3 with stream bench) | criterion baseline + 10% regression gate |

> **Fuzz target** (lives in the workspace `unblock-fuzz` crate, not here — PRD §8.1): `fuzz_targets/render_sanitize.rs` over `sanitize::{sanitize_inline, sanitize_text}` + `fuzz_targets/render_csv_escape.rs` over CSV field escaping. Listed here for traceability; the file lives in `unblock-fuzz` (NFR-16). Introduced v1.

---

## 4. Crate-level test & bench plan

- **Unit tests** — colocated `#[cfg(test)]` per backend (escaping edge cases, format parsing, error→code mapping).
- **Contract suite** (`tests/contract.rs`) — the render analogue of the Storage contract suite (NFR-16): for each `OutputFormat` × each renderable kind, assert (a) no panic, (b) byte-determinism across two runs, (c) parse-back equivalence for the structured formats (json/robot via serde, csv via `csv::Reader`). This is the gate that catches a format regressing.
- **insta snapshots** (`tests/snapshots/`) — golden output for plain/markdown/csv/json/robot (+toon when the feature is on); CI runs `insta --check` (NFR-14). Snapshots are the human-output stability gate.
- **proptest** — (1) `sanitize_*` never emits a raw control char (except allowed `\n`/`\t` in `text`) and never panics on arbitrary input; (2) `OutputFormat` `Display`↔`FromStr` round-trip; (3) any `Issue` json-renders→parses→equal; (4) `pick_format` precedence holds for all 4-layer combinations.
- **fuzz** — `cargo-fuzz` targets in `unblock-fuzz` over sanitize + CSV escaping (untrusted-input boundary, NFR-18).
- **criterion** (`benches/render.rs`) — 1k/10k issue render across formats; baseline + 10% regression gate (NFR-1). v1.3 adds the streaming-writer bench (1M-row sink).
- **Feature-matrix CI** — build & test both `--no-default-features` (default, no toon) and `--features toon` (v1.1) to keep the TOON surface off the default build (NFR-10) and prove the cfg-gated file compiles both ways.

---

## 5. Open questions specific to this crate

1. **Does the CLI actually need `plain`/`markdown`/`csv` in v1, or only `json`/`robot`?** D3 reduces the CLI to lifecycle/ops (serve/migrate/doctor/version) and D2/D7 make MCP the structured surface. If the v1 CLI emits only json/robot diagnostics, then `plain`/`csv`/`markdown` backends could themselves slip to v1.1 (export/audit ergonomics) — shrinking the v1 crate to json/robot + sanitize. **Recommendation:** keep `plain` (human-readable doctor/version output is valuable) and `json`/`robot` in v1; consider deferring `csv`/`markdown` to v1.1 unless a v1 consumer is identified. Needs a one-line confirmation from the cli/mcp planners.
2. **~~Where does `DiagnosticReport` live and what is its shape?~~ RESOLVED (spine §1.10, CF-B).** `DiagnosticKind` (Stats/Info/Where/Version/Lint/Changelog/Orphans) and `DiagnosticReport { kind, findings: Vec<DiagnosticFinding> }` plus `DiagnosticFinding { label, detail }` are now defined normatively in `unblock-model` §1.10. Render imports them directly — no back-edge.
3. **~~Where do the render-visible display/result DTOs live — model or engine/storage?~~ RESOLVED (spine §1.10, CF-A/CF-C — the single most important cross-crate decision for this crate).** All display/result DTOs render must format now live in `unblock-model`: `CountBucket` (+`CountGroupBy`), `DepTree` (+`GraphEdge`), `CloseOutcome`, `ExportReport`, `ImportReport`, `DiagnosticReport`. `unblock-storage`/`unblock-engine` **re-export** them; they do not define them. Render therefore stays **model + error only** with no spine conflict and no dependency-list widening.
4. **TOON crate sourcing (v1.1):** the original used `toon_rust`/`toon-rust`; confirm the maintained crate name + that it passes `cargo-deny` before wiring the `toon` feature (NFR-9/NFR-10). Pin it; keep it strictly optional.
5. **Timestamp format canonicalization:** confirm render uses the same RFC-3339 `SecondsFormat` as JSONL export (`unblock-sync`) so human/csv views match the export bytes — avoids two divergent time formats. Assumed `Utc` + `SecondsFormat::Secs`.

---

## 6. Cross-crate dependencies assumed (summary)

- **Display-DTOs in `unblock-model` (CONFIRMED by spine §1.10):** `CountBucket` (+`CountGroupBy`), `DepTree`/`GraphEdge`, `CloseOutcome`, `DiagnosticReport`/`DiagnosticKind`/`DiagnosticFinding`, `ExportReport`/`ImportReport`, and `OutputFormat` are all **defined in `unblock-model` §1.10** and importable from there (plus `unblock-error` for `StructuredError`, spine §2.4). `unblock-storage` (§3.1) and `unblock-engine` (§4.1) **re-export** these — they no longer define them. Render keeps its **model + error only** dependency with no spine conflict (CF-A/CF-B/CF-C/CF-J resolved; Open Questions §5.2/§5.3 closed).
- **Derive contract (G-1 / spine §1.10 derive-policy):** every §1.10 DTO render parses back (json/robot contract suite, §2 invariant 6, §4) derives the **full set** — `Debug, Clone, Serialize, Deserialize, JsonSchema` (+`PartialEq, Eq` for the parse-back equality assertions). This is normatively guaranteed by spine §1.10; render's `tests/contract.rs` serialize→parse→`assert_eq` round-trip therefore compiles. `ExportReport.path: PathBuf` is `JsonSchema`-valid (string) per the same policy. Render itself adds **no** derives to these types (it owns none of them).
