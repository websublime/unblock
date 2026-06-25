# unblock-config — File-level Plan

- **Status:** DRAFT — conforms to `docs/plans/00-roadmap.md`, `docs/plans/01-design-spine.md`, and `docs/PRD.md` (PRD APPROVED v1.1). Any API here that would drift from the spine must amend the spine first (spine §6.1).
- **Date:** 2026-06-19
- **Grounding:** original `temp/beads_rust-main/src/config/mod.rs` (discovery, `ConfigPaths`, `Metadata`, layered precedence) — reshaped by locked decisions D8/D10/D11/D14/D15 (rename to `.unblock/`, TOML + single `UNBLOCK_` prefix, no town/mayor routing, single-serve topology, remote off by default).

---

## 1. Purpose / Layer / Depends-on

**One-line purpose:** Resolve layered TOML configuration, discover the `.unblock/` workspace, **open/migrate libsql and build the `Arc<dyn Storage>`**, and expose the storage-bearing `WorkspaceContext` the engine consumes (CF-D: config owns workspace-open; engine consumes the context, it does not construct storage). A separate resolve-only `ResolvedContext` (no storage) is returned by the no-DB facade for `init`/no-workspace paths (spine §4, G-5 option b).

> **Type ownership (spine §4, NORMATIVE):** `WorkspaceContext` and `ResolvedContext` are **DEFINED in `unblock-config`** (G-9a). `WorkspaceContext` is **storage-bearing** with `storage: Arc<dyn Storage>` **NON-OPTIONAL** (spine §4.1, G-5 option b); `Session::open` consumes it and never unwraps an `Option`. The resolve-only path returns `ResolvedContext` (no storage handle).

**Layer:** L4 (`config`). Acyclic position per spine §0: `… → L3 sync | health → L4 config → L5 engine → …`. Nothing below L4 may import this crate.

**Depends-on (PRD §8.1, exact):** `unblock-storage`, `unblock-sync`, `unblock-health`, `unblock-model`, `unblock-error`. `unblock-model` is a **DIRECT** dependency: config is a value-producer that re-exports the domain knob type `unblock_model::OutputFormat` (G-7/CF-J — single home in the model) inside `ResolvedConfig`, so it imports the model directly rather than transitively. The edge is L0→L4 (a strict forward edge, acyclic — `unblock-model` is the deepest domain leaf below config). **MUST NOT** depend on `unblock-engine`, `unblock-render`, `unblock-mcp`, `unblock-cli`, or `unblock-policy` (policy is composed at L5 by the engine, not at L4 — CF-11). The layering back-edge CI check (plan §1) fails on violation.

> **Acyclicity note:** the spine lists L3 `health` as depending on `sync` and L4 `config` as depending on `storage + sync + health + model + error` (the `model` edge is L0→L4, the direct `OutputFormat` re-export). That is a strict forward edge set; `unblock-config` introduces no back-edge. The crate does **not** open the engine — it discovers `.unblock/`, builds the `Arc<dyn Storage>` (spine §4 intro + §4.1, CF-D), and returns a fully-resolved storage-bearing `WorkspaceContext { storage: Arc<dyn Storage> /* NON-OPTIONAL */, workspace_dir, actor, config: ResolvedConfig, paths: ConfigPaths }` that the engine (L5) consumes via `Session::open(ctx, cfg)` to turn into a `Session`. The resolve-only facade instead returns a `ResolvedContext` (config + paths, no storage) for `init`/no-DB paths.

> **v1 build split — T1.3a (minimal) lands BEFORE T1.2; T1.3 (full) is additive (spine §4 intro, NORMATIVE).**
> This crate is built in two tasks so the engine (L5) has the context types it consumes:
> - **T1.3a — minimal subset (the interface the engine depends on):** delivers `WorkspaceContext` +
>   `ResolvedContext` + `ConfigError` + `ResolvedConfig` + `ConfigPaths` (the config-owned value/path types, the
>   last two with **hardcoded/defaulted values**), plus `open_with_storage` / `open_workspace`. The facades perform:
>   `.unblock/` **upward discovery from `start`**, **path resolution into `ConfigPaths`** (`unblock_dir =
>   workspace_dir.join(".unblock")`; `db_path`/`jsonl_path` derived from `unblock_dir` + the `ResolvedConfig`
>   filenames — **config OWNS path resolution from T1.3a**), then **libsql open via
>   `unblock_storage::LibsqlStorage::open_local(db_path)`, THEN migrate via `Storage::migrate()`** (two explicit
>   calls — `open_local` does **NOT** run migrations: `DbOpenFailed` wraps `open_local`, `MigrationFailed` wraps
>   `migrate()`), `Arc<dyn Storage>` construction, and **actor resolution precedence** `UNBLOCK_ACTOR` env →
>   `$USER` → `"unblock"`. There is **NO** layered TOML/env/CLI precedence engine yet — `ResolvedConfig` is built
>   from defaults. This sequences **before T1.2** because the engine *consumes* `WorkspaceContext`, and config is
>   **L4** so it **cannot** depend on the engine at **L5** (`cargo xtask check-layering` rejects that back-edge; the
>   build dep edge is engine→config).
> - **T1.3 — full layered resolver (additive, no public-type/signature change):** adds the precedence engine
>   (**CLI > env `UNBLOCK_*` > project `.unblock/config.toml` > defaults**) ADDITIVELY — it **replaces the
>   defaulting internals** behind `ResolvedConfig` (same public shape, now resolved for real) and may **enrich the
>   facade input** (see the facade-signature note). It touches **no public type pinned at T1.3a**.
>
> **`ResolvedConfig` / `ConfigPaths` (config-owned, spine §4 CF-D) — the resolved config VALUES + resolved PATHS
> embedded by value in both contexts.** Both are **DEFINED in this crate** (the spine references them as
> config-owned). Pin a **MINIMAL v1 shape** — only the fields v1 actually reads (grounded in this plan's
> `WorkspaceConfig`/`ProjectConfig` key set and what the engine/`Session` consumes), DEFAULTED at T1.3a and resolved
> for real at T1.3. **`actor` is NOT inside `ResolvedConfig`** — it is the authoritative top-level context field
> (spine §4.1); **paths are NOT inside `ResolvedConfig`** — they live in `ConfigPaths`:
>
> ```rust
> // unblock-config — the resolved, validated config VALUES the engine/Session reads (config-owned, spine §4 CF-D).
> // NOT paths (see ConfigPaths), NOT actor (top-level context field, spine §4.1).
> #[derive(Debug, Clone)]
> pub struct ResolvedConfig {
>     pub output_format: OutputFormat, // re-exported from unblock-model (G-7/CF-J); default per model (Json)
>     pub jsonl_export: bool,      // auto-export JSONL after mutating ops (FR-7); T1.3a default = false
>     pub search_cap: usize,       // search result cap (FR-4); T1.3a default = 50
>     pub db_filename: String,     // T1.3a default = "unblock.db"   (locked name, PRD §12.5)
>     pub jsonl_filename: String,  // T1.3a default = "issues.jsonl" (locked name, PRD §12.5)
> }
>
> // unblock-config — config OWNS path resolution from T1.3a (single source of truth, spine §4 CF-D).
> // Derived from the discovered workspace + the ResolvedConfig filenames.
> #[derive(Debug, Clone)]
> pub struct ConfigPaths {
>     pub unblock_dir: PathBuf,   // the discovered/created `.unblock/` dir (= workspace_dir.join(".unblock"))
>     pub db_path: PathBuf,       // unblock_dir.join(ResolvedConfig.db_filename)    (T1.3a default "unblock.db")
>     pub jsonl_path: PathBuf,    // unblock_dir.join(ResolvedConfig.jsonl_filename) (T1.3a default "issues.jsonl")
> }
> ```
>
> No keys are invented beyond this plan's locked constants and `WorkspaceConfig`/`ProjectConfig` set: `output_format`,
> `jsonl_export`, `search_cap`, `db_filename`, `jsonl_filename` all map to existing config keys — `output_format`,
> `jsonl_export`, `search_cap`, and `db_filename` match the merged `WorkspaceConfig` fields (§3 `config.rs`); the
> filename `jsonl_filename` matches the merged `WorkspaceConfig.jsonl_filename`, which in the raw `ProjectConfig`
> (§3 `schema.rs`) is the key `jsonl_export_filename` (it projects to the merged `jsonl_filename`). `actor` is NOT a
> `ResolvedConfig` key — it is resolved (`UNBLOCK_ACTOR → $USER → "unblock"`) into the **top-level context `actor`
> field** only. The `ConfigPaths` fields (`unblock_dir`/`db_path`/`jsonl_path`) match the §3 `paths.rs`
> `ConfigPaths`. At T1.3a these are **defaulted/derived**; at T1.3 the layered resolver fills the values — the
> shapes do not change. (Relationship to the full resolver's internals: `ResolvedConfig` is the spine-named,
> engine-facing projection embedded in the contexts; the richer internal `WorkspaceConfig` of the full resolver is
> the **T1.3 producer** that *fills* `ResolvedConfig`/`ConfigPaths` — it is not part of the minimal T1.3a public
> surface. The `ConfigPaths` here is the same type the §3 `paths.rs` row owns, surfaced by value in the contexts
> from T1.3a.)
>
> **Facade-signature reconciliation (spine §4 `&Path` ↔ this plan's `&CliOverrides`).** The `&CliOverrides` form the
> §2/§3 tables below show is the **T1.3-ADDITIVE** shape (the CLI threads `--dir`/`--db`/`--actor`/`--output-format`
> down through resolution). The **T1.3a minimal** facade instead takes `start: &Path`:
> `open_with_storage(start: &Path)` / `open_workspace(start: &Path)` (spine §4, the normative v1 signatures). This is
> a pure **sequencing** of the input parameter — the **result** types (`WorkspaceContext` / `ResolvedContext`) are
> identical across T1.3a and T1.3, so the engine (which binds to the result, never the facade signature) is
> unaffected when the input is enriched at T1.3. **No public type or signature pinned by the spine changes.**
>
> The spine wins on any interface disagreement (spine §6.1); this split is an authorized spine-pinned sequencing,
> not a divergence.

---

## 2. Public API summary (what other crates import), per version

### v1 (subset — FR-13 subset; split across T1.3a minimal + T1.3 full — see the build-split note above)

**[T1.3a — minimal subset]** the public surface the engine consumes (facades take `start: &Path`):
- `ResolvedConfig` — the resolved, validated config VALUES the engine/`Session` reads (config-owned, spine §4 CF-D); minimal v1 shape pinned in the build-split note (NOT paths, NOT actor). DEFAULTED at T1.3a, resolved at T1.3.
- `ConfigPaths` — the resolved `.unblock/` + db/jsonl paths (config-owned, spine §4 CF-D): `{ unblock_dir, db_path, jsonl_path }`. **Config OWNS path resolution from T1.3a** (single source of truth — derived from the discovered workspace + the `ResolvedConfig` filenames). DEFAULTED/derived at T1.3a, resolved at T1.3.
- `ResolvedContext` — the resolve-only context (no storage): `{ workspace_dir, actor, config: ResolvedConfig, paths: ConfigPaths }` (spine §4). Returned by `open_workspace` for `init`/no-DB paths (G-5 option b).
- `WorkspaceContext` — the **storage-bearing** context (CF-D): `{ storage: Arc<dyn Storage> /* NON-OPTIONAL, spine §4.1 */, workspace_dir, actor, config: ResolvedConfig, paths: ConfigPaths }`. Consumed by `Session::open`.
- `open_workspace(start: &Path) -> Result<ResolvedContext, ConfigError>` — resolve-only facade (no DB): `.unblock/` upward discovery → defaulted `ResolvedConfig` → path resolution into `ConfigPaths` → returns a `ResolvedContext` with **no storage handle** (for `init`/no-DB paths). **Does NOT open the DB.**
- `open_with_storage(start: &Path) -> Result<WorkspaceContext, ConfigError>` — the **workspace-open facade** (CF-D): discover → resolve `ConfigPaths` → **open libsql via `unblock_storage::LibsqlStorage::open_local(db_path)`, THEN migrate via `Storage::migrate()`** (two explicit calls — `open_local` does **NOT** run migrations; `DbOpenFailed` wraps `open_local`, `MigrationFailed` wraps `migrate()`) → build the `Arc<dyn Storage>` → actor resolution (`UNBLOCK_ACTOR` → `$USER` → `"unblock"`) → returns a storage-bearing `WorkspaceContext` (spine §4.1). The engine consumes this via `Session::open(ctx, cfg)`; **config builds storage, engine never does.**
- `ConfigError` (snafu, per-crate; implements `unblock_error::CodedError`). T1.3a minimal variant set (spine §2.1): `WorkspaceNotFound → NotInitialized`; `DbOpenFailed → source.code()` (wraps `open_local`; typically `Backend → DatabaseError`); `MigrationFailed → source.code()` (wraps `migrate()`; `Migration`/`SchemaMismatch → SchemaMismatch`, `Backend → DatabaseError` — forwarded, not hardcoded); `ActorUnresolved → RequiredField`. The exit-7 config-file variants (`ConfigError`/`ConfigNotFound`/`ConfigParseError`) + I/O are added **additively at T1.3** with the layered resolver.

**[T1.3 — full layered resolver, additive over T1.3a]** (replaces the defaulting internals; facades may take `&CliOverrides`; no public-type/spine-signature change):
- `WorkspaceConfig` — the merged, validated config value (startup + runtime keys); the full-resolver internal that *produces* `ResolvedConfig` (and `ConfigPaths`).
- `ConfigPaths::resolve(dir, cfg, cli)` + accessors (`policy_path`/`recovery_dir`) — the full-resolver path-resolution machinery (§3 `src/paths.rs`). **The `ConfigPaths` TYPE itself is a T1.3a public type** (see the T1.3a list above — config owns path resolution from T1.3a); T1.3 only enriches *how* it is filled (custom filenames, `--db` override) without changing the shape.
- `CliOverrides` — the typed top layer the CLI passes down (highest precedence); the additive facade input shape.
- `EnvOverrides` — parsed `UNBLOCK_*` layer (second precedence).
- `discover_unblock_dir(start: Option<&Path>, cli: &CliOverrides) -> Result<PathBuf, ConfigError>` — walk-up `.unblock/` discovery honoring `UNBLOCK_DIR`/`--dir`.
- `discover_optional_unblock_dir(...) -> Result<Option<PathBuf>, ConfigError>` — for `init`/no-workspace commands.
- `WorkspaceConfig::resolve(cli, env, project_toml, defaults) -> Result<WorkspaceConfig, ConfigError>` — the precedence engine.
- `OutputFormat` (**re-exported** from `unblock-model`, not defined here — G-7/CF-J; `pub use unblock_model::OutputFormat`), `StartupKey`/`RuntimeKey` partition markers (FR-13 startup-vs-runtime).

#### T1.3 acceptance items (carried from the T1.3a Verify gate)

These are **T1.3-scoped** acceptance items deliberately deferred from the T1.3a minimal subset — recorded here so they are not lost (they land with the full layered resolver, when the surface they guard exists):

- **insta error golden:** add an `insta` golden snapshot of the `ConfigError` `(variant → code → exit)` table (`src/error.rs`) once the full T1.3 variant set lands (parse / unknown-key / invalid-value / I/O / credential paths). The L0 `unblock-error` exit-code golden (`unblock-error/tests/exit_code_table.rs`) already pins the `ErrorCode → exit` mapping for the minimal set; T1.3a asserts each variant's mapping with per-variant `code()` / `code().exit_code()` unit asserts, so a golden over the 4-variant T1.3a set would only re-pin what those asserts already cover — it becomes load-bearing when the variant set grows.
- **Security seam — actor bounding:** route the resolved actor through the model's bounds (`ACTOR_MAX_CHARS = 200` + NUL/control-char rejection, per `crates/unblock-model/src/validation.rs`) when the env/CLI actor input surfaces (`UNBLOCK_ACTOR` / `--actor`) are fully wired at T1.3. T1.3a only trims whitespace on the resolved actor; an unbounded/NUL-bearing actor cannot reach storage until those input surfaces exist.
- **Security seam — path injection:** when `db_filename` / `jsonl_filename` become **config-resolved** (no longer locked constants) at T1.3, guard `ConfigPaths::derive` against path-separator / `..` injection so the resolved db/jsonl path cannot escape `unblock_dir`. At T1.3a both filenames are locked constants (`"unblock.db"` / `"issues.jsonl"`), so no untrusted segment reaches the join.
- **Security seam — symlink `.unblock`:** `discovery::is_workspace_dir` uses `Path::is_dir`, which **follows symlinks**; decide at T1.3 whether a symlinked `.unblock` is allowed or must be rejected/resolved (canonicalize + confinement) so the DB cannot be opened outside the discovered subtree.

### v1.1 (full — FR-13 full)
- `DbConfigLayer` — config read from the libsql `config` table (between project-TOML and defaults in precedence).
- `UserConfig` — `~/.config/unblock/config.toml` (XDG) user layer (between project and DB layers).
- `ConfigLayer` enum + `merge_layers(&[ConfigLayer])` made fully general (all 6 layers).
- `WorkspaceConfig::reload_runtime(...)` — re-resolve only runtime keys without reopening storage.
- `policy_path()` accessor for `.unblock/policy.toml` (FR-19 gates; the policy *content* lives in `unblock-policy`, this crate only resolves the path + existence).

### v1.2 (remote — proposed)
- `RemoteConfig` (feature `remote`): endpoint URL, sync interval (startup-only keys).
- `CredentialSource` — resolves libsql auth tokens from `UNBLOCK_*` env **or** OS keychain **only**, never `config.toml` (NFR-18 hard rule, enforced by a deny-test).

> The crate does **not** participate in v1.3 (roadmap §7 crate-impact matrix shows no `unblock-config` cell for v1.3).

---

## 3. FILE BREAKDOWN

> Layout: a flat module crate. `src/lib.rs` re-exports; each concern is one module file. Tests split: in-file `#[cfg(test)]` units + `tests/` integration (precedence/discovery contract suites) + `proptest`/`insta` where shapes are pinned. No `unsafe` (`#![forbid(unsafe_code)]`), `#![warn(missing_docs)]`.

| File | Version | Responsibility | Key items | Tests |
|---|---|---|---|---|
| `Cargo.toml` | v1 (edit v1.1/v1.2) | Crate manifest. Deps: `unblock-storage`, `unblock-sync`, `unblock-health`, `unblock-model` (**DIRECT** L0→L4 — the `unblock_model::OutputFormat` re-export, G-7/CF-J), `unblock-error`, `serde`, `toml`, `snafu`, `tracing`, `tokio` (the workspace-open facades are `async fn`). `unblock-sync`/`unblock-health`/`serde`/`toml`/`tracing` are **forward-declared at T1.3a** (the PRD §8.1 dependency contract + the T1.3 layered-resolver feed) — present by design, exercised by the full resolver. `directories` (XDG user layer) lands **v1.1** with `user_config.rs`, not v1. `[features] remote` (v1.2) gates `keyring`/remote types — off by default (D15/NFR-10). dev-deps: `tempfile`, `tokio`, `chrono` (T1.3a); `proptest`, `insta` land with the v1 layered resolver (T1.3). | `[features] default=[]`, `remote=["dep:keyring"]`; workspace lints inherited. | `cargo-deny` confirms no TLS/keychain on default tree (NFR-10). |
| `src/lib.rs` | v1 | Crate root: lints, module decls, public re-exports (the §2 surface), crate-level docs with a usage doctest. | `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`; `pub use` of `WorkspaceConfig`, `ConfigPaths`, `CliOverrides`, `EnvOverrides`, `ResolvedContext`, `WorkspaceContext`, `ConfigError`, `open_workspace`, `open_with_storage`, `discover_unblock_dir`; and `pub use unblock_model::OutputFormat` (re-export, G-7/CF-J — not defined here). | doctest: `open_workspace` against a `tempfile` `.unblock/`. Compile-fail test (in `tests/`) that importing `unblock_engine` here is impossible (layering doc note, not a real test — see §4). |
| `src/error.rs` | **T1.3a** (extend v1.1/v1.2) | Per-crate snafu enum + `CodedError` impl (spine §2.1 pattern). **T1.3a-minimal variant set (spine §2.1):** `WorkspaceNotFound{start:PathBuf}`, `DbOpenFailed{source:StorageError}`, `MigrationFailed{source:StorageError}`, `ActorUnresolved` — GROWING ADDITIVELY to the T1.3-full set. | **T1.3a:** `#[derive(Snafu)] pub enum ConfigError { WorkspaceNotFound{start}, DbOpenFailed{source:StorageError}, MigrationFailed{source:StorageError}, ActorUnresolved }`; `impl unblock_error::CodedError for ConfigError { fn code(&self)->ErrorCode; }` (the trait impl, NOT an inherent `code()` — matches the landed `StorageError` convention so the L7 blanket `From<&E: CodedError>` bridges it) → `WorkspaceNotFound→NotInitialized`, `DbOpenFailed→source.code()` (typically `Backend→DatabaseError`), `MigrationFailed→source.code()` (`Migration`/`SchemaMismatch→SchemaMismatch`, `Backend→DatabaseError` — forwarded, not hardcoded), `ActorUnresolved→RequiredField` (all exit 2/4 per §2.3). **T1.3-additive (file-config layer):** the layered resolver adds `Parse{source, path}`, `Io{source, path}`, `InvalidValue{key, value, reason}`, `DiscoveryFailed{start}`, `UnknownKey{key} /*warn-only*/` (+ v1.2 `CredentialMissing{}`, `RemotePathNotConfined{}`) → `ConfigError`/`ConfigNotFound`/`ConfigParseError` (exit 7), I/O→`IoError` (exit 8), `RemotePathNotConfined`→`PathTraversal` (exit 6, v1.2). No T1.3a variant is removed or renumbered. | unit: every variant maps to the expected `ErrorCode` + `exit_code()`; `DbOpenFailed`/`MigrationFailed` forward the inner `StorageError` code (a `Backend` cause → `DatabaseError`, a `Migration`/`SchemaMismatch` cause → `SchemaMismatch`); `insta` golden snapshot of `(variant → code → exit)` table (parallels spine §2.3, FR-11). |
| `src/keys.rs` | v1 | The startup-vs-runtime key partition (FR-13 "startup-vs-runtime key partitioning"). Declares which keys are read once at open (DB path, jsonl filename, backend, remote endpoint) vs re-readable at runtime (actor default, output format, jsonl-export toggle, search cap). | `pub enum StartupKey {...}`, `pub enum RuntimeKey {...}`; `pub const STARTUP_KEYS: &[&str]`, `pub const RUNTIME_KEYS: &[&str]`; `fn classify(key:&str)->Option<KeyClass>`. | unit: every field of `WorkspaceConfig` is classified exactly once (no key in both/neither — exhaustiveness assert); `insta` snapshot of the two key lists (drift detector for FR-13 contract). |
| `src/schema.rs` | v1 (fields added v1.1/v1.2) | The serde-deserializable `ProjectConfig` (raw `.unblock/config.toml` shape) + `Defaults`. Distinct from the merged `WorkspaceConfig` (raw layer = all-`Option`; merged = resolved). | `#[derive(Deserialize, Default)] pub struct ProjectConfig { actor: Option<String>, db_filename: Option<String>, jsonl_export_filename: Option<String>, jsonl_export: Option<bool>, output_format: Option<OutputFormat>, search_cap: Option<usize>, deletions_retention_days: Option<u64> /*, v1.1: [user]/[db] never here; v1.2: [remote] table*/ }`; `OutputFormat` is **re-exported from `unblock-model`** (`pub use unblock_model::OutputFormat` — NOT redefined here, G-7/CF-J; the model owns the canonical `Json|Robot|Plain|Csv|Markdown`, `Toon` feature-gated, with the full derive set incl. serde snake_case + JsonSchema); `fn defaults() -> ProjectConfig`. **Forbids** a `[remote] auth_token` key — deny at parse (NFR-18). | unit: parse a representative `config.toml`; unknown-key handling (warn, not fail — startup resilience); `proptest`: any subset of fields round-trips raw→merged without panic; `insta`: default `WorkspaceConfig` snapshot. **deny-test:** a `config.toml` containing an auth token is rejected with `ConfigError` (NFR-18). |
| `src/env.rs` | v1 (keys added v1.2) | Parse the `UNBLOCK_*` layer (single prefix, D10). Maps `UNBLOCK_ACTOR`, `UNBLOCK_DIR`, `UNBLOCK_JSONL`, `UNBLOCK_OUTPUT_FORMAT` (+ v1.2 `UNBLOCK_REMOTE_URL`, `UNBLOCK_AUTH_TOKEN`). **`UNBLOCK_JSONL` is env-only in v1** (D10/G-24a): there is **no `--jsonl` CLI flag**; the jsonl-export toggle is set via env or `config.toml`, and `CliOverrides` carries `jsonl_export` only for programmatic callers, not a clap flag. Injectable env source (a `Fn(&str)->Option<String>` or `&dyn EnvSource`) so tests don't touch process env. | `pub struct EnvOverrides { actor, dir, jsonl_export, output_format, /*v1.2*/ remote_url, auth_token }`; `pub trait EnvSource { fn get(&self,key:&str)->Option<String>; }`; `impl EnvOverrides { pub fn from_source(src:&dyn EnvSource)->Result<Self,ConfigError>; pub fn from_process_env()->Result<Self,ConfigError>; }`. | unit: each var parses; malformed `UNBLOCK_OUTPUT_FORMAT`→`InvalidValue`; empty var treated as unset (matches original `filter(!is_empty)`); **no** legacy `BD_`/`BR_`/`BEADS_` keys recognized (assert they are ignored, D10). |
| `src/cli.rs` | v1 | The typed top layer (`CliOverrides`) the CLI binary fills and passes to `open_workspace`. Highest precedence. Decoupled from `clap` (clap lives in `unblock-cli`; this is a plain struct so the engine/tests don't pull clap). | `pub struct CliOverrides { dir: Option<PathBuf>, db: Option<PathBuf>, actor: Option<String>, output_format: Option<OutputFormat>, jsonl_export: Option<bool>, no_db: bool /*v1.1 doctor path*/ }` with a `Default` + builder-ish setters. | unit: `Default` is all-none; setters compose; a `--db` pointing under `.unblock/` derives `unblock_dir` (port of original `discover_*_with_cli` derivation). |
| `src/discovery.rs` | v1 (multi-ws v1.2-scoped) | Discover the active `.unblock/` dir. Walk up ancestors from `start` (or CWD); honor `--dir`/`UNBLOCK_DIR` first; derive dir from an explicit `--db` that lives under `.unblock/`. **Single-workspace only** (D11 — no town/mayor; no routing.rs port). Path canonicalization for confinement. | `pub fn discover_unblock_dir(start:Option<&Path>, cli:&CliOverrides)->Result<PathBuf,ConfigError>`; `pub fn discover_optional_unblock_dir(...)->Result<Option<PathBuf>,ConfigError>`; `fn is_unblock_dir_name(name:&str)->bool` (only `.unblock`, **no** `_beads` monorepo alias unless re-decided — see Open Q1); `fn derive_dir_from_db(db:&Path)->Result<PathBuf,ConfigError>`. | unit: walk-up finds nearest `.unblock/` across N parents; `UNBLOCK_DIR` overrides walk-up; explicit `--dir` overrides env; `--db` under `.unblock/` derives the dir; not-found yields `ConfigError::NotFound` with hint; symlink/`..` escape rejected. `tests/` integration uses real `tempfile` trees. |
| `src/paths.rs` | v1 (remote v1.2) | Resolve concrete artifact paths from the discovered dir + metadata + overrides. Port of original `ConfigPaths::resolve` minus YAML/legacy-filename sprawl. | `pub struct ConfigPaths { unblock_dir: PathBuf, db_path: PathBuf, jsonl_path: PathBuf }`; `impl ConfigPaths { pub fn resolve(dir:&Path, cfg:&WorkspaceConfig, cli:&CliOverrides)->Result<Self,ConfigError>; pub fn policy_path(&self)->PathBuf /*v1.1*/; pub fn recovery_dir(&self)->PathBuf /*.unblock/.recovery, v1.1 FR-16*/ }`; constants `DB_FILENAME="unblock.db"`, `JSONL_FILENAME="issues.jsonl"`, `CONFIG_FILENAME="config.toml"` (locked names, PRD §12.5). | unit: default paths = `dir/unblock.db` & `dir/issues.jsonl`; `--db` override wins; custom filename via config; `jsonl_path` is jsonl-export-aware; `insta` snapshot of resolved paths for a fixture dir. |
| `src/merge.rs` | v1 (layers added v1.1/v1.2) | The precedence engine. v1 order (highest→lowest): **CLI > env(`UNBLOCK_*`) > project `config.toml` > defaults**. Field-wise override (first non-`None` from highest layer wins), producing the merged `WorkspaceConfig`. Structured to slot in user/DB layers (v1.1) and remote (v1.2) without reshaping callers. | `pub(crate) enum ConfigLayer { Cli(..), Env(..), Project(ProjectConfig), /*v1.1*/ Db(ProjectConfig), User(ProjectConfig), Defaults }`; `pub(crate) fn merge_layers(layers:&[ConfigLayer])->WorkspaceConfig` (ordered highest-first); per-field `fn pick<T>(...)`. | **proptest (FR-13 AC):** for any tuple of partial layers, the merged value of each field equals the highest-precedence layer that set it (precedence is total + deterministic). unit: each adjacent-layer override (cli>env, env>project, project>defaults). |
| `src/config.rs` | v1 (extend v1.1/v1.2) | The merged `WorkspaceConfig` value + its `resolve` constructor + validation + accessors split by startup/runtime. | `#[derive(Debug,Clone)] pub struct WorkspaceConfig { actor:String, output_format:OutputFormat, jsonl_export:bool, search_cap:usize, db_filename:String, jsonl_filename:String, deletions_retention_days:Option<u64> /*v1.1: user/db sourced fields; v1.2: remote:Option<RemoteConfig>*/ }`; `impl WorkspaceConfig { pub fn resolve(cli,env,project,defaults)->Result<Self,ConfigError>; pub fn validate(&self)->Result<(),ConfigError>; pub fn actor(&self)->&str; pub fn output_format(&self)->OutputFormat; /*v1.1*/ pub fn reload_runtime(&mut self, ...)->Result<(),ConfigError>; }`. | unit: `resolve` validation (empty actor→default `"unblock"`? or required — Open Q3); `search_cap` default 50 (FR-4); runtime-only `reload_runtime` does not change startup keys (v1.1). `insta`: a fully-merged `WorkspaceConfig` golden. |
| `src/context.rs` | **T1.3a** (storage open) | `ResolvedContext` (resolve-only, **no storage**) + `WorkspaceContext` (storage-bearing, CF-D). The two-facade split (G-5 option b / G-9): both bundle `config:ResolvedConfig` (config-owned VALUES) + `paths:ConfigPaths` (config-owned PATHS) + `workspace_dir` (project root) + `actor` (authoritative, spine §4.1); `WorkspaceContext` adds the **`Arc<dyn Storage>` this crate builds** as a **NON-OPTIONAL** field (spine §4.1 shape). Holds the `open_workspace`/`open_with_storage` entrypoints. Bridges to `unblock-storage` (opens libsql via `LibsqlStorage::open_local`, **then** migrates via `Storage::migrate()` — two explicit calls, `open_local` does NOT migrate) and to `unblock-health`/`unblock-sync` only for path/preflight, never engine. | `pub struct ResolvedContext { config:ResolvedConfig, paths:ConfigPaths, workspace_dir:PathBuf, actor:String }`; `pub struct WorkspaceContext { config:ResolvedConfig, paths:ConfigPaths, workspace_dir:PathBuf, actor:String, storage:Arc<dyn Storage> /* NON-OPTIONAL — Session::open never unwraps an Option (G-5) */ }` (the **spine §4 CF-D shape exactly**: `config:ResolvedConfig`, NOT `WorkspaceConfig`); `pub fn open_workspace(start:&Path)->Result<ResolvedContext,ConfigError>` (resolve only, no DB; T1.3a `&Path`, T1.3-additive `&CliOverrides`); `pub async fn open_with_storage(start:&Path)->Result<WorkspaceContext,ConfigError>` (open_local **then** migrate, **constructs the `Arc<dyn Storage>`**; `DbOpenFailed` wraps `open_local`, `MigrationFailed` wraps `migrate()`); accessors `config()`, `paths()`, `workspace_dir()`, `actor()` on both, plus `storage()` on `WorkspaceContext`. **Relationship:** at T1.3a the contexts embed `ResolvedConfig`/`ConfigPaths` built from defaults; the richer internal `WorkspaceConfig` of the full resolver (§3 `config.rs`) is the **T1.3 producer** that *fills* `ResolvedConfig`/`ConfigPaths` — it is NOT a context field. **Engine consumes the storage-bearing `WorkspaceContext` via `Session::open(ctx, cfg)`; config builds storage, engine never does (CF-D).** | integration (`tests/open_workspace.rs`): full discover→resolve `ResolvedConfig`/`ConfigPaths` on a `tempfile` `.unblock/` (returns `ResolvedContext`); `open_with_storage` opens a real libsql file (`open_local` then `migrate`) + `integrity_check` ok and yields a `WorkspaceContext` with a live `storage`; error path when dir missing. |
| `src/db_layer.rs` | **v1.1** | Read the libsql `config` table as a `ConfigLayer::Db` (FR-13 full). Reads via the **`Storage::read_config()` seam reserved in spine §3.2 (CF-E, `[v1.1]`-additive)** — never raw libsql here (NFR-15). v1 storage impls may return empty; this layer treats that as an empty `ConfigLayer::Db`. | `pub(crate) async fn load_db_layer(storage:&dyn Storage)->Result<ProjectConfig,ConfigError>`. | unit (mock `Storage`): DB rows map to `ProjectConfig`; precedence: DB sits **below** project-TOML, **above** defaults; absent table → empty layer (no error). |
| `src/user_config.rs` | **v1.1** | Resolve + load `~/.config/unblock/config.toml` (XDG via `directories`). The user layer (between project and DB). | `pub(crate) fn user_config_path()->Option<PathBuf>`; `pub(crate) fn load_user_config()->Result<ProjectConfig,ConfigError>`. | unit (injected HOME/XDG): present file parses; absent → empty layer; malformed → `ConfigParseError`. |
| `src/remote.rs` | **v1.2** (feature `remote`) | Remote/replica config (endpoint, sync interval — startup-only) + the credential resolver that reads tokens from env/keychain **only**. | `#[cfg(feature="remote")] pub struct RemoteConfig { url:String, sync_interval:Duration }`; `pub enum CredentialSource { Env, Keychain }`; `fn resolve_credential(...)->Result<Option<String>,ConfigError>`; path-confinement for remote-derived local replica file. | unit: token from `UNBLOCK_AUTH_TOKEN`; token from keychain mock; a token in `config.toml` is rejected (NFR-18 deny-test, also asserted in `schema.rs`). `wiremock`-adjacent left to storage; here only resolution. feature-gated so default build excludes it. |
| `tests/precedence.rs` | v1 (extend v1.1/v1.2) | Integration contract suite for FR-13 layer precedence across **all** active layers, using injected env + tempfile project TOML (no process-global state). | full-matrix cases: cli>env>project>defaults; per-version adds user/db (v1.1), remote keys (v1.2). | the FR-13 AC gate ("precedence unit-tested across all v1 layers"); golden `insta` of a representative resolution. |
| `tests/discovery.rs` | v1 | Integration discovery suite on real `tempfile` directory trees. | nearest-`.unblock/` walk-up; env/cli override; `--db` derivation; not-found; symlink-escape rejection. | discovery contract; deterministic across platforms (path normalization, NFR-11). |
| `tests/open_workspace.rs` | v1 | End-to-end facade test: `open_workspace` + `open_with_storage` (libsql) round-trip. | resolve-only vs open-with-storage; migrate-if-needed; integrity ok; missing-dir error. | facade smoke gate consumed by engine M1. |
| `tests/no_legacy_prefix.rs` | v1 | Guard: legacy `BD_`/`BR_`/`BEADS_` env and YAML config are **not** recognized (D8/D10 regression guard). | set legacy vars + drop a `config.yaml`; assert ignored / not loaded. | locks the rename + single-prefix decision. |

---

## 4. Crate-level test & bench plan

- **Unit (per module):** as tabled above; every public fn has at least one happy + one error case; `missing_docs` warn forces doc coverage.
- **proptest (NFR-16):**
  - `merge.rs`: precedence totality — for any random layer stack, each merged field equals the highest layer that set it (FR-13 invariant).
  - `schema.rs`: raw→merged never panics for any partial `ProjectConfig`.
  - `keys.rs`: every config field classified exactly once (exhaustiveness).
- **insta snapshots (NFR-14, CI `--check`):** error-code table (`error.rs`), default `WorkspaceConfig`, resolved `ConfigPaths` fixture, startup/runtime key lists. Drift in any pins a contract change (parallels FR-12 `contract_version` discipline at the config boundary).
- **Injected-source discipline:** env + filesystem + (v1.1) DB + (v1.2) keychain are all behind injectable traits (`EnvSource`, tempfile dirs, mock `Storage`, mock keychain) so the suite is deterministic and parallel-safe (no `std::env::set_var` races — NFR-16/CI).
- **Layering enforcement:** the workspace-level back-edge check (plan §1, e.g. `cargo-deny`/graph assertion) is the real guard that `unblock-config` imports no L5+ crate; this crate adds no extra compile-fail test beyond a doc note.
- **No criterion bench:** config resolution is one-shot at open and not on a perf budget (NFR-1 targets are storage ops). If `open_with_storage` ever shows up in startup-latency budgets, add a `benches/open.rs` then; not in scope v1.

---

## 5. Open questions specific to this crate

1. **`_unblock` monorepo alias?** Original accepted `.beads` **and** `_beads` (monorepo). Spine/PRD lock only `.unblock/`. Default plan: single name `.unblock` (no alias). Confirm whether the monorepo `_unblock` alias must survive the rename — flag to Miguel rather than silently dropping (per global "never simplify" rule).
2. **`metadata.json` vs `config.toml` for startup paths.** Original split startup path/filename data into `metadata.json` (DB/jsonl filenames, backend, retention). PRD §7 lists `metadata` as an on-disk artifact but §12.5 locks only `config.toml`. Default plan: fold the few startup-path keys (`db_filename`, `jsonl_export_filename`, `deletions_retention_days`, `backend`) into `config.toml` (single file). Confirm whether a separate `metadata.json` is still wanted.
3. **Required vs defaulted `actor`.** Is `actor` mandatory (error if absent across all layers) or defaulted (e.g. `"unblock"` / `$USER`)? FR-13 AC only pins precedence, not requiredness. Engine needs a non-empty `actor` (spine `SessionConfig.actor: String`). Default plan: default to `$USER` then `"unblock"`; confirm.
4. **Where does the libsql open live?** **RESOLVED (CF-D).** Spine §4 intro + §4.1 pin `unblock-config` as the owner of workspace-open: this crate performs `.unblock/` discovery + path resolution, opens libsql via `LibsqlStorage::open_local` **then** migrates via `Storage::migrate()`, and **builds the `Arc<dyn Storage>`**, exposing a `WorkspaceContext { storage, workspace_dir, actor, config: ResolvedConfig, paths: ConfigPaths }`. The engine **consumes** it via `Session::open(ctx, cfg)` and does not construct storage. `open_with_storage` (§3 `context.rs`) is the canonical entrypoint.
5. **DB config-table read seam (v1.1).** **RESOLVED (CF-E).** Spine §3.2 now reserves `Storage::read_config(&self) -> Result<Vec<(String, String)>, StorageError>` as a `[v1.1]`-additive seam (commented stub; v1 impls may return empty). `db_layer.rs` (v1.1) consumes this seam — no further `Storage` amendment needed.
