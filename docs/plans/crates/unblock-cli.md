# unblock-cli — File-level Plan

**Source of truth:** PRD APPROVED v1.1 (per spine §0 hierarchy: PRD > spine > crate plans; on any cross-crate type/signature disagreement, the spine wins and this plan is the bug).

**One-line purpose:** the reduced `unblock` binary — lifecycle/ops only (`serve`, `migrate`, `doctor`, `version`, plus `init`/`agents` bootstrap, `update` self-update (v1, D17), and `completions` (v1.1)), with thin routing into `unblock-engine`/`unblock-mcp`, the canonical 0–8 exit-code policy, and cooperative shutdown. **No domain features** (D3): create/list/close/dep/etc. exist only over MCP.

**Layer:** L7 (`mcp | cli`). It is a sibling of `unblock-mcp`, not a dependency of it.

**Depends on** (must match PRD §8.1 / spine §0 layering — acyclic, no back-edge):
- `unblock-engine` (L5) — the single session/mutation home (FR-9); CLI is a thin adapter.
- `unblock-render` (L6) — output formatting for `doctor`/`version`/`migrate` reports (json/robot/plain) under NFR-14 stdout/stderr discipline.
- `unblock-policy` (L1) — only for any contract-version/diagnostic surface it must echo (read-only); no policy *decisions* are made here.
- `unblock-error` (L0) — `StructuredError`, `ErrorCode`, exit-code table; the **only** place the 0–8 mapping is applied (spine §2, conformance rule 5).
- `unblock-config` (L4, transitively via engine) — for `SessionConfig` assembly from layered config (CLI > env `UNBLOCK_*` > `.unblock/config.toml` > defaults).
- `unblock-mcp` (L7, sibling) — `serve` calls `unblock_mcp::serve(session, transport, shutdown)`; this is an L7↔L7 edge. **SETTLED (spine §0.1, NORMATIVE):** cli owns the binary and depends on mcp; mcp is a library exposing `serve(...)`. Direction is fixed **cli → mcp, never mcp → cli** (keeps the graph acyclic). See resolved Q1 below.
- (v1) `axoupdater` (library dep) for the `unblock update` self-update command (FR-25/NFR-17/**D17**) — no separate crate.

**Crates that import from `unblock-cli`:** none. It is a binary-bearing leaf; its only public surface is `unblock_cli::run()` (so `main.rs` stays trivial and the routing is unit/integration-testable). Nothing depends on it.

---

## Public API summary (per version)

`unblock-cli` is a binary crate with a thin library facade so the routing layer is testable without spawning a process.

| Item | Signature | Version | Notes |
|---|---|---|---|
| `pub async fn run() -> ExitCode` | parses args, dispatches, maps errors → exit code | v1 | called by `main`; never panics on a domain error |
| `pub async fn run_with(args: impl IntoIterator<Item = OsString>) -> ExitCode` | testable entrypoint (inject argv) | v1 | used by integration tests + `assert_cmd` |
| `pub struct Cli` (clap `Parser`) | top-level command, global flags | v1 | stable clap only (D9) |
| `pub enum Command` | `Serve \| Migrate \| Doctor \| Version \| Init \| Agents` (+ `Update` in v1; `Completions` in v1.1) | v1/v1.1 | the *only* subcommands (D3) |
| `pub enum CliExit` | typed exit outcome → `std::process::ExitCode` | v1 | wraps `unblock_error::ErrorCode::exit_code()` |

No domain types are re-exported; consumers needing them go through MCP. The library facade exists purely for testability (conformance rule: routing must be exercisable without a real process so the exit-code golden suite is hermetic).

---

## FILE BREAKDOWN

### Crate root & manifest

| Path | Responsibility | Key items | Version | Tests |
|---|---|---|---|---|
| `Cargo.toml` | Crate manifest. Bin `unblock`; lib facade `unblock_cli`. Deps: `clap` (stable features only — `derive`, `env`, **no** unstable/dynamic, D9), `tokio` (full, D6), `unblock-engine`, `unblock-render`, `unblock-error`, `unblock-policy`, `unblock-mcp`, `tracing`, `tracing-subscriber`. v1 adds `axoupdater` for self-update (D17); v1.1 adds `clap_complete` (static). `[lints] workspace = true`. The default-on Cargo feature is named **`self-update`** and it **enables the `unblock update` command** — the feature name (`self-update`) deliberately differs from the command token (`unblock update`); this mismatch is by design (CF-K, G-18). `--no-default-features` drops both the feature and the `unblock update` command (the only network surface). | `[[bin]] name = "unblock"`; `[features] default = ["self-update"]`, `self-update = ["dep:axoupdater"]` (v1) | v1 | `cargo-deny` layering check (no back-edge); feature-matrix build in CI |
| `build.rs` | Capture build-time metadata for `version` (git sha **via env only**, never a git crate / `Command::new("git")` — NFR-6): version, build profile, rustc, target, enabled features. Reads `VERGEN_*`/`CARGO_*` env; no network, no git invocation. | emits `cargo:rustc-env=UNBLOCK_BUILD_*` | v1 | compile-time only; asserted indirectly by `version` snapshot |
| `src/main.rs` | Trivial process entry. `#![forbid(unsafe_code)]`. Builds tokio runtime, calls `unblock_cli::run().await`, returns its `ExitCode`. | `#[tokio::main] async fn main() -> ExitCode` | v1 | none (logic lives in lib) |
| `src/lib.rs` | `#![forbid(unsafe_code)] #![warn(missing_docs)]`. Public facade: `run`, `run_with`, module wiring (`cli`, `dispatch`, `exit`, `commands`, `logging`, `shutdown`, `output`). | re-exports `Cli`, `Command`, `run`, `run_with` | v1 | doctest on `run_with` smoke path |

### Argument model & routing

| Path | Responsibility | Key items | Version | Tests |
|---|---|---|---|---|
| `src/cli.rs` | clap `Parser` definitions. Global flags: `--dir` (`UNBLOCK_DIR`), `--actor` (`UNBLOCK_ACTOR`), `--output`/`-o` (`UNBLOCK_OUTPUT_FORMAT`: `json\|robot\|plain`), `-v/--verbose`, `-q/--quiet`. Subcommands enumerate **only** lifecycle/ops (D3). Per-command arg structs. | `struct Cli { global: GlobalArgs, command: Command }`; `struct GlobalArgs`; `enum Command`; `struct ServeArgs`, `MigrateArgs`, `DoctorArgs`, `VersionArgs`, `InitArgs`, `AgentsArgs`; `UpdateArgs` (v1); v1.1: `CompletionsArgs`; `enum OutputFormat`; `enum ShellType` (v1.1) | v1 (+v1.1 additions) | unit: clap `debug_assert` (`Cli::command().debug_assert()`); env-var binding parses; `--help` snapshot (insta); reject unknown/domain subcommand (e.g. `unblock create` → usage error exit 1/clap exit 2) |
| `src/dispatch.rs` | Pure routing: `Command` → the matching `commands::*::run(...)`. Assembles `SessionConfig` from `GlobalArgs` + config layering (via `unblock-config` through engine facade). Decides which commands open a `Session` (`migrate`/`doctor`/`serve`) vs which don't (`version`, `init` pre-DB, `agents`). | `async fn dispatch(cli: Cli) -> Result<(), CliError>`; `fn session_config(global: &GlobalArgs, cmd: &Command) -> SessionConfig` | v1 | unit: each variant routes to the right handler (handlers stubbed via trait seam); `version`/`init` do **not** open storage |
| `src/exit.rs` | The 0–8 exit-code boundary (spine §2.3, conformance rule 5). Converts any `CliError`/`EngineError` → `StructuredError` → `ExitCode`. Emits the structured error: **JSON on stdout** in json/robot mode (always valid JSON even on error, FR-11), human on stderr in plain mode. Diagnostics strictly stderr (NFR-14). | `enum CliError` (snafu, `#[snafu(transparent)]` wrapping `EngineError`/`McpError`/local I/O); `impl CliError { fn code(&self) -> ErrorCode }`; `fn into_exit(err: CliError, fmt: OutputFormat) -> ExitCode`; `fn ok_exit() -> ExitCode` | v1 | **golden exit-code snapshot** (insta): every `ErrorCode` → its 0–8 exit (mirrors `unblock-error` table, FR-11); error JSON validity on every code path; stdout-vs-stderr placement asserted |
| `src/output.rs` | Thin wrapper over `unblock-render` for the few lifecycle outputs (`version` info, `migrate` report, `doctor` report). Resolves `OutputFormat` from global flags/config. Enforces NFR-14: structured → stdout, diagnostics → stderr. | `struct CliRenderer`; `fn render<T: Serialize>(&self, value: &T, fmt: OutputFormat)`; `fn diag(msg: &str)` (→ stderr) | v1 | unit: format selection precedence (flag > env > config > default); snapshot of each lifecycle payload shape (insta) |

### Cross-cutting infrastructure

| Path | Responsibility | Key items | Version | Tests |
|---|---|---|---|---|
| `src/logging.rs` | `tracing-subscriber` setup. `-v/-q` map to levels; structured `tracing` on the `unblock.reliability` target (NFR-13); **logs to stderr only** (NFR-14). No log line ever pollutes stdout. | `fn init_logging(verbose: u8, quiet: bool) -> Result<(), CliError>` | v1 | unit: level mapping; assert subscriber writes to stderr; idempotent re-init guarded |
| `src/shutdown.rs` | Cooperative shutdown (FR-17). Installs SIGINT/SIGTERM/SIGHUP handlers → an atomic "shutdown requested" flag + a `tokio::sync::Notify`/`CancellationToken` consumed by `serve`. Second signal → async-signal-safe `_exit(128+signo)`. **Windows: no-op** (cfg-guarded, NFR-11). Exit code carries `128 + signo`. Pure `signal-hook`/tokio signal — **no unsafe** (forbid). | `fn install() -> ShutdownToken`; `struct ShutdownToken { flag: Arc<AtomicBool>, notify: Arc<Notify> }`; `fn is_requested() -> bool`; `fn signal_exit_code() -> Option<u8>` | v1 | unit (unix-cfg): first signal sets flag + correct `128+signo`; idempotent install; Windows path compiles to no-op; integration: SIGTERM during `serve` triggers clean `Session::shutdown()` |

### Command handlers (one file per lifecycle command)

| Path | Responsibility | Key items | Version | Tests |
|---|---|---|---|---|
| `src/commands/mod.rs` | Command-module wiring + a shared `CommandHandler` seam (so `dispatch` is unit-testable with stubs). | `pub mod {serve, migrate, doctor, version, init, agents, update}` (+ `completions` v1.1) | v1 | none (aggregator) |
| `src/commands/serve.rs` | **PRIMARY runtime command** (FR-20). Opens `Session` (engine), installs shutdown token, builds the rmcp stdio server via `unblock_mcp::serve(session, ShutdownToken)`, blocks until EOF/shutdown, then `session.shutdown()` (flush+close libsql cleanly, FR-17). Single-serve-per-workspace topology (D14) — engine owns the write `Semaphore`; CLI does not duplicate it. | `async fn run(args: &ServeArgs, cfg: SessionConfig, token: ShutdownToken) -> Result<(), CliError>` | v1 | integration (`assert_cmd` + piped stdio): `initialize` handshake succeeds; SIGTERM → clean shutdown, exit `128+15`, no WAL corruption (failure-injection, NFR-5); stdout carries only MCP framing |
| `src/commands/migrate.rs` | Run `Session`/`Storage::migrate()` to apply schema. Reports applied/up-to-date as a structured report. Expected to run when `serve` is inactive (D14, best-effort otherwise). | `async fn run(args: &MigrateArgs, cfg: SessionConfig) -> Result<MigrateReport, CliError>`; `struct MigrateReport { applied: Vec<String>, already_current: bool }` | v1 | integration: fresh dir migrates then is idempotent (second run = `already_current`); snapshot of report (insta); exit 0 on success, exit 2 on `SchemaMismatch` |
| `src/commands/doctor.rs` | FR-16 (lite): libsql `integrity_check` + basic diagnostics via `unblock-health` (through engine). Renders a health report; **no git** (NFR-6). Non-zero exit on detected corruption. (Full Healthy/Drifted/Recoverable/Unsafe taxonomy = v1.1, FR-16 full — this file grows a `--repair` seam then.) | `async fn run(args: &DoctorArgs, cfg: SessionConfig) -> Result<DoctorReport, CliError>`; `struct DoctorReport`; v1.1: `args.repair`, evidence under `.unblock/.recovery/` | v1 (grows v1.1) | integration: clean DB → healthy, exit 0; corrupted fixture → non-zero + structured findings; snapshot report (insta); NFR-6 static-gate friendly (no git symbols) |
| `src/commands/version.rs` | Emit version/build/rustc/target/features from `build.rs` env. **No network** on the normal path (D13/NFR-17) — the original's GitHub update-check is dropped from `version`; any update check lives only in the `update` command. | `async fn run(args: &VersionArgs, fmt: OutputFormat) -> Result<VersionReport, CliError>`; `struct VersionReport { version, build, commit: Option<_>, rustc, target, features: Vec<_> }` | v1 | unit/snapshot: stable JSON & plain shapes (insta); assert no network call (no reqwest symbol on default build) |
| `src/commands/init.rs` | FR-14 bootstrap: create `.unblock/` (config.toml scaffold, empty libsql via migrate), `--prefix` for id prefix, `--force` to overwrite. Idempotent; refuses to clobber a non-empty `.unblock/` without `--force`. | `async fn run(args: &InitArgs) -> Result<InitReport, CliError>`; `struct InitArgs { prefix: Option<String>, force: bool }` | v1 | integration: `init` idempotent; refuses non-empty dir w/o `--force` → exit 2 (`AlreadyInitialized`); `--prefix` honored; created config round-trips |
| `src/commands/agents.rs` | FR-14: inject/maintain `AGENTS.md` (how an agent connects to `unblock serve` / the MCP contract). Idempotent merge of a managed block. | `async fn run(args: &AgentsArgs, cfg: SessionConfig) -> Result<(), CliError>` | v1 | integration: creates `AGENTS.md`; re-run updates only the managed block; snapshot of generated block |
| `src/commands/completions.rs` | FR-23 (**v1.1**): static shell completions for bash/zsh/fish/powershell/elvish via `clap_complete::generate` from `Cli::command()`. Static only (no runtime/dynamic completer — D9). To stdout or `-o <file>`. | `fn run(args: &CompletionsArgs) -> Result<(), CliError>`; `enum ShellType` | v1.1 | unit: each shell generates non-empty script; snapshot per shell (insta); `-o` writes file |
| `src/commands/update.rs` | FR-25 (**v1**, D17): the `unblock update` command — self-update via the `axoupdater` library. Updates verified against GitHub artifact attestations before execution (NFR-17); **never on a normal path**; behind the default-on `self-update` feature (feature name ≠ command token, CF-K). | `async fn run(args: &UpdateArgs) -> Result<(), CliError>` | v1 | integration (mocked release source via `wiremock`): rejects unverifiable/tampered artifact; happy path swaps binary atomically; `--no-default-features` drops it |

### Tests (integration / contract)

| Path | Responsibility | Key items | Version | Tests/cases |
|---|---|---|---|---|
| `tests/exit_codes.rs` | **Golden exit-code contract** (FR-11, conformance rule 5). Drives `run_with` (hermetic) for each error class and asserts the 0–8 exit + valid-JSON error payload. | uses `assert_cmd`/`run_with` + `insta` | v1 | one case per `ErrorCode` category (2 DB, 3 issue, 4 validation, 5 dep, 6 sync, 7 config, 8 io, 1 internal, 0 success); JSON parses on every error |
| `tests/help_snapshots.rs` | `--help` / `-h` for top-level + each subcommand snapshot-pinned (drift detection). | `insta` | v1 | top-level + serve/migrate/doctor/version/init/agents/update; v1.1: completions |
| `tests/serve_lifecycle.rs` | End-to-end `serve` over piped stdio: MCP `initialize` + a `ready` call (proves CLI↔MCP wiring, FR-9/FR-20); SIGTERM → clean shutdown (FR-17/NFR-5). | `assert_cmd`, child process, signal | v1 | initialize handshake; `ready→claim→close` smoke via MCP; SIGTERM mid-write commits-or-rolls-back, exit `128+15`, no WAL corruption |
| `tests/migrate_doctor.rs` | `migrate` idempotency + `doctor` on clean and corrupted fixtures. | `assert_cmd`, temp workspace, corrupt-db fixture | v1 | migrate fresh→idempotent; doctor healthy exit 0; doctor corrupted non-zero with structured findings |
| `tests/init_agents.rs` | `init`/`agents` bootstrap (FR-14). | `assert_cmd`, temp dir | v1 | init idempotent + `--force` clobber guard + `--prefix`; agents managed-block merge |
| `tests/no_git_gate.rs` | NFR-6 static assertion specific to the binary: no `Command::new("git")`, no git crate symbol, no network symbol on default build. | source scan / symbol check | v1 | fails if any git/network symbol leaks into the default-feature binary |
| `tests/cli_engine_parity.rs` | FR-9: a lifecycle op routed through CLI matches the same op via the engine API directly (no drift). | direct `Session` vs CLI dispatch | v1 | `migrate`, `doctor` produce identical engine-level outcomes |
| `tests/completions.rs` | FR-23 static completion generation. | `insta` | v1.1 | one snapshot per shell |
| `tests/update_verify.rs` | FR-25 attestation-verification gate (axoupdater). | `wiremock` mock release | v1 | reject unverifiable/tampered artifact; happy path |

---

## Crate-level test & bench plan

- **Unit tests** (`#[cfg(test)]` in each `src/*.rs`): clap parsing/env binding, format precedence, level mapping, shutdown flag/exit-code math, dispatch routing with stubbed handlers.
- **Contract suite — exit codes (FR-11):** `tests/exit_codes.rs` is the authoritative CLI half of the golden 0–8 pin; it must stay in lock-step with `unblock-error`'s table (a deliberate dual-pin so a divergence fails CI in both crates).
- **Snapshot tests (`insta`, NFR-14):** all `--help`, every lifecycle report shape (version/migrate/doctor), completion scripts (v1.1). CI runs `cargo insta test --check`.
- **Integration tests (`assert_cmd` + `tempfile`):** real binary, real temp workspace, piped stdio for `serve`. The serve↔MCP and SIGTERM failure-injection cases are the reliability gate (NFR-5).
- **proptest:** light — property over the exit-code mapping (every `ErrorCode` round-trips to a value in `0..=8`) and over `OutputFormat` precedence resolution (CLI > env > config > default holds for arbitrary layer combinations).
- **No `criterion` benches in this crate.** CLI is not on a hot path; performance budgets (NFR-1/NFR-2) are owned by storage/engine. (If `serve` startup latency becomes a metric per §14, add a single startup-time bench in v1 — flagged as an open question, Q4.)
- **fuzz:** none here; ingestion fuzzing lives in `unblock-fuzz` over model/sync/storage. The CLI's only untrusted input is argv (clap-validated) and stdin (handled by `unblock-mcp`'s schemars boundary, NFR-18).
- **No-git static gate:** `tests/no_git_gate.rs` + workspace `cargo-deny` (NFR-6).

---

## Open questions specific to this crate

1. **Q1 — cli → mcp edge direction. RESOLVED (spine §0.1, NORMATIVE).** `unblock-cli` depends on `unblock-mcp`: cli owns the binary and calls `unblock_mcp::serve(session, transport, shutdown)`; mcp is a library, never owns the binary. Direction is fixed **cli → mcp, never mcp → cli** (acyclic). This keeps the single binary `unblock` (NFR-11) and one arg parser. The edge is settled (see "Depends on" line above); no further confirmation needed before T2.2/T3.1.
2. **Q2 — `init`/`agents` placement.** FR-14 bootstrap is arguably "lifecycle/ops" (so it belongs to the CLI) but is borderline against D3's "no domain features." **Assumed:** `init`/`agents` are lifecycle (workspace bootstrap, not issue domain) and live here. If Miguel deems them domain, they move behind an MCP tool and the CLI shrinks to serve/migrate/doctor/version only. **Milestone of record (G-12):** FR-14 (init/agents) and the CLI flag-forwarding half of FR-13 deliver at **M3** via **T3.1** (these files build in `unblock-cli`, which is constructed at M3); PRD §13 was realigned from M1 to M3 to match this plan.
3. **Q3 — `doctor --repair` timing.** v1 `doctor` is read-only (lite). The original surfaced a `ConcurrencyLost` exit on a contended `--repair`. Under D14 (single-serve, migrate/doctor run when serve inactive), do we ship a v1 `--repair` at all, or defer all repair to v1.1 (FR-16 full) with the `.unblock/.recovery/` evidence path? **Assumed:** v1 = diagnose-only; `--repair` lands in v1.1.
4. **Q4 — startup-latency metric.** §14.1 records agent round-trip latency as a success metric (TBD on M2). Does `serve` startup time get its own bench/gate in this crate, or is it measured end-to-end in the engine/mcp benches? **Assumed:** measured at the mcp/engine layer; no criterion bench in cli unless this becomes a named gate.
5. **Q5 — global `--dir`/`--actor` vs config precedence ownership.** The layered precedence (CLI > env > config > defaults, FR-13) is resolved in `unblock-config`. The CLI just forwards flags. Confirm the CLI must **not** re-implement precedence and only passes raw override candidates into `SessionConfig` assembly (avoids a second source of truth).
