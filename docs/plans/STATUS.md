# unblock — Build STATUS (task registry)

Durable, git-backed system-of-record for **what is done / in progress / to do**. It is the live status
overlay over [`implementation-plan.md`](implementation-plan.md) (the task DAG + acceptance criteria) and
the [roadmap](00-roadmap.md). The harness Task tools (TaskCreate/Update) are the **per-session execution
layer**; THIS file is the **cross-session source of truth** (it survives compaction and restarts).

**Legend:** ☑ done · ◐ in-progress · ⊘ blocked · ☐ todo.  **Ready** = every "Depends on" is ☑.

**Rules**
- Update a row the moment a task changes state; keep this file in the same commit as the work.
- A task is ☑ only when it meets its PRD acceptance criteria **and** passed review (no close-without-review).
- When any decision (D-id), FR tier, or command surface changes, run the decision-change checklist (see
  `ci-cd-and-distribution.md` §2.1 doc-lint + spine = single source of truth).
- The spine (`01-design-spine.md`) wins on any cross-crate interface disagreement.

---

## Phase 0 — Planning & QA  *(DONE)*

| Item | Status |
|---|---|
| Discovery (3-agent + coordinator) → dossier | ☑ |
| PRD v0.1 → 3-lens review → v0.2 → **APPROVED v1.1** | ☑ |
| Per-file plans: spine + 12 crate docs (~190 files) | ☑ |
| 5 spine amendments (CF-A..CF-E) | ☑ |
| CI/CD + distribution — D17 (dist/axoupdater) | ☑ |
| Gap/drift QA: 24 findings applied + verified → **GO** | ☑ |
| Tracking decision: Markdown `STATUS.md` (this file) | ☑ |

## Pre-T0 — before crate creation

| id | Item | Depends on | Status |
|---|---|---|---|
| P.1 | This `STATUS.md` registry | — | ☑ |
| P.2 | Root `CLAUDE.md` (global rules / arch / idiomatic Rust) | — | ☑ |
| P.3 | Roadmap v1.2/v1.3 review — **deferred to v1 GA** (just-in-time; not a pre-T0 gate) | — | — |

## M0 — Foundation  *(gate: Storage contract suite green + contention lab proves no hot-spin)*

| id | Task | Depends on | Status | Notes |
|---|---|---|---|---|
| T0.1 | Workspace scaffold deps (`Cargo.toml`: libsql/clap/axoupdater; backoff→backon; reqwest dropped; deny→forbid) | P.2 | ☑ | **MERGED with T0.2** ([PR #366](https://github.com/websublime/unblock/pull/366)); 3-lens design Review = PASS-with-changes, applied |
| T0.2 | Create 12 `unblock-*` crates + `xtask` layering check | T0.1 | ☑ | **MERGED with T0.1** ([PR #366](https://github.com/websublime/unblock/pull/366)); AC green: build 1.96 + remote/no-default variants + storage/mcp network-free; per-crate `CLAUDE.md` stubs done; **design Review + Verify gates both PASS** (layering rejects an injected back-edge — non-vacuous) |
| T0.3 | `unblock-model` (types, hash, sync-eq, validation) | T0.2 | ☑ | **MERGED** ([PR #368](https://github.com/websublime/unblock/pull/368)); **design Review + Verify gates both PASS_WITH_CHANGES, applied**. Spec-first PART 1 (spine §1.8 frozen 17-field hash padding tail Q4=KEEP; §1.1–§1.4 hand-rolled enum serde, no `untagged`) + crate impl (Issue + open enums, `content_hash` bd-byte-parity, `sync_equals`, tombstone, full `IssueValidator`→aggregate `ValidationFailed{fields}`, `src/id.rs`, 12 §1.10 DTOs). Verify fix: `is_expired_tombstone` panic-free (`checked_add_signed`) + panic-safety proptest. Ingestion cargo-fuzz targets **deferred to T0.7** (see that row); proptest covers panic-safety now. |
| T0.4 | `unblock-error` (snafu taxonomy, exit codes) | T0.2 | ☑ | **MERGED** ([PR #367](https://github.com/websublime/unblock/pull/367)); **design Review + Verify gates both PASS_WITH_CHANGES, applied**. Spec-first PART 1 (spine §2 D-E1 `FieldError`/`ValidationFailed`, 35-variant reconcile, OQ-1/2/4/5 + model Q2 resolved) + crate impl (35 `ErrorCode`, `StructuredError`, `ExitCode`, `CodedError`, `ModelError`). Security hardenings: (1) sanitize `hint` at the L0 chokepoint (with_hint/from_coded/From), spine §2.4 covers message+hint+context-render note; (2) bound `find_similar_ids` input (`MAX_SUGGESTION_INPUT_CHARS=256`) + two-row Levenshtein. `unblock-error` fuzz target (`fuzz_sanitize`) **deferred to T0.7** (see that row); proptest covers the invariants now. |
| T0.5 | `unblock-storage`: `Storage` trait | T0.3, T0.4 | ☑ | **MERGED** ([PR #369](https://github.com/websublime/unblock/pull/369)); **design Review + Verify gates both PASS_WITH_CHANGES, applied**. Spec-first PART 1 (spine §3.1 IssuePatch Option-B full field set + `Default`; spine §3.1 `StorageError` made concrete incl. `Migration{from:i32,to:i32,reason:String}`; spine §2.1 per-crate enums implement `CodedError`; crate plan: `dependency_graph` added to trait + deps.rs, T0.5↔T0.6 manifest/dep boundary, CF-E seams commented, ErrorCode map pinned `Backend`→`DatabaseError`/`Migration`→`SchemaMismatch`/`IntegrityFailed`→`DatabaseError`) + crate impl (4 backend-free files: `lib.rs`/`trait_def.rs` (full §3.2 method set incl. `dependency_graph`, CF-E commented)/`filters.rs` (`DeletePlan`/`DeleteMode`/`IssuePatch`)/`error.rs` (`StorageError`+`CodedError`+`BackendOpaque`); object-safety + `NoopStorage` + golden ErrorCode + retryable + context-holder + sanitization tests; manifest trimmed, network-free). **5 non-blocking doc-precision follow-ups → T0.6 spec-first**: `update_issue` no-op semantics, `search_issues` field set, `blocked_issues` order (DESC-beads vs ASC fork), tombstone delegation to the model helper, `retryable()` coupling. |
| T0.6 | `unblock-storage`: libsql impl (WAL, busy_timeout) | T0.5 | ☑ | **MERGED** ([PR #370](https://github.com/websublime/unblock/pull/370)); **design Review (2 iterations: FAIL→reconcile→PASS) + Verify (PASS_WITH_CHANGES) both applied**. Spine §3.2.1 source-verified method-semantics + EventType oracle; §3.3 (busy_timeout=5000 native, BEGIN IMMEDIATE, wal_autocheckpoint=0+manual, OQ-5 = write conn + separate read conn / shared-cache `:memory:`). Backend: `LibsqlStorage` (two-conn), 38-col schema (CASCADE; `depends_on_id` no-FK; `idx_issues_ready`), `CURRENT_SCHEMA_VERSION=1` forward migrations, CRUD + **atomic assignee-only claim** (no race; same-actor no-op), queries (`instr`+id search; **3-pass live blocked** = direct 3-type ∪ epic-rollup ∪ **transitive parent→children**), deps (petgraph + DFS cycle-path), events, pure-DB diagnostics; `From<libsql::Error>` busy/locked map. 58 tests, network-free (RK-4), no panic in lib paths. Verify fixes: `:memory:` WAL-pragma flake (WAL file-path-only; 32-task stress test, 0 flakes/21+ runs) + transitive 3rd blocked pass (Miguel). **Deferred:** T0.7 contract suite + fuzz targets; T0.8 contention lab (**must use a FILE DB** — `:memory:` shared-cache can't WAL). *(Reconciled at T0.8, spec-first: **NFR-1/NFR-2 perf budgets are T3.5**, not T0.8 — the contention lab asserts only non-spin + correctness, no throughput/latency, per `implementation-plan.md` T0.8 AC + the locked-scope `benches/storage.rs` at T3.5. The earlier "+ NFR-1/2 perf" framing on the T0.8 line was drift.)* |
| T0.7 | Storage contract test suite (NFR-16) | T0.6 | ☑ | **MERGED** ([PR #371](https://github.com/websublime/unblock/pull/371)); **design Review (PASS_WITH_CHANGES) + Verify (PASS, no must-fix) gates both applied**. (A) NFR-16 backend-independent contract suite in `unblock-storage::testkit` (generic over a storage factory; one `contract_*` case per trait method + cross-cutting invariants + 2 gated `StorageTestkit`-seam cases) run against `open_in_memory` + temp-file `open_local` in `tests/contract.rs`; `behaviour.rs` kept 100% intact. (B) 8 cargo-fuzz targets wired in the `unblock-fuzz` MEMBER crate as stable `run_<t>_case` cores + a nested nightly libFuzzer package (`fuzz/`, `exclude`d from the workspace): model+error `{content_hash, issue_ingest, parse_id, enum_deserialize, sanitize}` (**incl. T0.4's deferred `fuzz_sanitize`** + T0.3's deferred model ingestion targets) + storage `{query_filters, cycle_detect, id_alloc}`; `tests/regression.rs` replays the committed corpus + a `proptest!` smoke per target on stable. **Notes:** root `[workspace] exclude = ["crates/unblock-fuzz/fuzz"]` + `tempfile` added to `[workspace.dependencies]`; nested fuzz package pins `nightly-2024-10-31` (scoped `fuzz/rust-toolchain.toml`); gated `StorageTestkit` seams (`testkit_insert_raw_edge` + `testkit_child_high_water`) added (impl in-module in `libsql/testkit.rs`, no crate-root visibility widening); member crate **drops the stub `unblock-sync` dep** (the JSONL/`bd`/sync targets are post-T0.7); spec-first: `unblock-fuzz.md` reconciled (OQ-1/OQ-6 resolved, structural rule + landed targets), `count_issues` trait doc corrected for the Label exception, model `EXTERNAL_REF_MAX_CHARS`/`LABEL_MAX_LEN` promoted to `pub`. **Remaining M0:** T0.8 contention lab (M0 gate; FILE DB) + T0.9 CI scaffolding (CI test job **must** run `--features testkit` so the NFR-16 suite executes). |
| T0.8 | **Contention lab (RK-1 / NFR-3 — M0 GATE)** | T0.6 | ☑ | **MERGED** ([PR #372](https://github.com/websublime/unblock/pull/372)); **design Review (PASS_WITH_CHANGES) + Verify (PASS, `metric_sound=true`, no must-fix) both applied**. **RK-1 RESOLVED — no rusqlite pivot.** **Topology:** `tests/contention_lab.rs` (`#![cfg(feature = "testkit")]`) drives `K` independent `LibsqlStorage` instances (adaptive to `available_parallelism()`, floor 2; hard-fail < 2 vCPU) on **one shared temp FILE DB** (`open_local`, not `:memory:` — shared-cache is non-WAL), so cross-instance writers contend on the real WAL write lock. **Metric:** baseline-relative **CPU-per-write ratio** via the `cpu-time` crate (whole-process CPU, all tokio threads, unsafe-free — no raw libc) — **sequential** own-file baseline (honest single-write cost) vs **parallel** shared-file contended, `R = contended ÷ baseline`. Gate asserts **R ≤ 5.0 (PROVISIONAL, calibration-pending; perf budgets = T3.5)**. **Measured on a multi-core dev machine: `R ≈ 1.0–1.2`** (honest blocking — sleep-based, never spins; the band BOUNDS the independent run-to-run spread observed across the implementer's 3× flake check AND the Verify gate's runs — not a single point), well under the ceiling. **Busy-retry witness** (mandatory, deterministic): a zero-timeout probe (libsql has no busy-handler callback; native timeout blocks silently) → witness > 0 contended / == 0 baseline (else INCONCLUSIVE, never a silent pass). **Correctness:** claim storm → exactly-one durable winner (re-SELECT; losers allowlist `AlreadyClaimed|DatabaseLocked`); disjoint-create reconciliation (no `IdCollision`; allowlist `DatabaseLocked`); update storm (allowlist `DatabaseLocked`); any other variant = corruption. **WAL-bound:** passive `wal_checkpoint(PASSIVE)` every **50** committed mutations on the held write conn (never TRUNCATE in the hot path — resolves spine §3.3 "tuned at T0.8") bounds the `-wal` under a 64 MiB ceiling; **negative control** (`#[ignore]`, checkpoint OFF) breaches it at ~220 MiB. **Forced-spin control** (`#[ignore]`, `busy_timeout=0` → tight non-yielding spin via gated `open_local_with_busy_timeout`) measured **`R ≈ 27`** — proves the metric detects a real hot-spin (non-vacuous). Clean `integrity_check()`; full diagnostic block printed every run. **No throughput/latency asserted (perf = T3.5).** **Manifest:** root + storage `Cargo.toml` add dev-dep `cpu-time` (Cargo.lock pinned `1.0.0`); `mod.rs`/`libsql/testkit.rs`/`src/testkit.rs` add 3 `AtomicU64`/`AtomicBool` counters + checkpoint cadence + busy-witness toggle + gated `StorageTestkit` accessors (`testkit_busy_retry_count`/`testkit_checkpoint_count`/`testkit_mutation_count`/`testkit_set_checkpoint_interval`/`testkit_set_busy_witness`) + the gated forced-spin constructor — **no crate-root visibility widening**; new `mod.rs` unit test proves the passive checkpoint bounds the WAL sidecar. **Fallback if it ever genuinely fails (real contention + high R / lost write / non-empty integrity): rusqlite behind the `Storage` trait, re-open D14/D15.** |
| T0.9 | CI scaffolding (ci.yml, deny.toml, .cargo/config, **doc-lint**) | T0.2 | ◐ | **IMPLEMENTED (branch `t0.9-ci-scaffolding`, awaiting design Review + Verify gates → PR).** **As-built job set (11 M0 jobs, ci-cd §2):** `.github/workflows/ci.yml` = `fmt` / `clippy` (workspace + targeted `-p unblock-storage --features testkit`) / `test` (`--workspace --locked`, the always-on set) / `storage-testkit` (`--features testkit --test contract` NFR-16 + `--test contention_lab` the M0 gate, with an explicit **≥ 2 vCPU** `nproc` precondition step) / `snapshots` (`insta test --check`) / `layering` (`cargo xtask check-layering`) / `audit` (`rustsec/audit-check`) / `deny` (`EmbarkStudios/cargo-deny-action check`) / `toolchain` (`cargo build --workspace --locked`, NFR-12) / **`doc-lint`** (`cargo xtask doc-lint`). Plus `.github/workflows/fuzz-smoke.yml` (`schedule: cron 0 4 * * *` + `workflow_dispatch`): a nightly-`2024-10-31` libFuzzer matrix over the **8** fuzz targets (`-max_total_time=60`) + a **separate stable-1.96** `contention-controls` job running the 2 `#[ignore]`d controls (`-- --ignored`) — nightly never leaks onto the stable storage build. **Every `uses:` SHA-pinned** to a 40-char commit + `# vX.Y.Z`: checkout `08c6903…` v5.0.0, rust-cache `c193711…` v2.9.1, dtolnay/rust-toolchain `29eef33…` (channel via `toolchain:` input), cargo-deny-action `bb137d7…` v2.0.20, audit-check `69366f3…` v2.0.0. **DEFERRED ledger** (listed at the top of `ci.yml`): `bench-gate`→T3.5, `scale`→T3.5, `no-network`→T3.1/T3.6, `rate-limit`→T3.4/T3.5 (each needs an artefact that doesn't exist until its task). **`doc-lint` (NEW `xtask/src/doc_lint.rs`, 6 classes a–f over the fixed 19-file corpus, `regex`-driven — already in the lock via tracing-subscriber, +0 transitive):** GREEN (`doc-lint OK: 19 docs, 6 classes clean`); 6 planted-violation unit tests (one/class) + 7 guard tests + a `tests/doc_lint_corpus.rs` integration test pinning the real corpus GREEN **and** asserting the existence-guard FAILs on a truncated corpus (non-vacuity, mirroring T0.2's back-edge proof); a live-corpus injection probe fired a+c+d+e simultaneously then reverted. **Targeted features, NOT `--all-features`** (ci-cd §2.2): `cargo tree -e features --all-features` resolves the libsql `remote` TLS stack (reqwest/hyper/rustls), so `--all-features` is banned from the M0 gate (D15/NFR-10); `-p unblock-storage --features testkit` verified TLS/network-free. **`deny.toml`:** all four checks pass (`advisories ok, bans ok, licenses ok, sources ok`); license allowlist (MIT/Apache-2.0[+LLVM-exc]/BSD-2/BSD-3/ISC/Zlib/**Unicode-3.0**/**CDLA-Permissive-2.0**/MPL-2.0/OpenSSL, confidence 0.93) covers the unconditional axoupdater/aws-lc/`ring` TLS stack; `[bans].deny` = `git2`/`gix`/`libgit2-sys` (NFR-6) **proven non-vacuous** (temporarily adding `git2` made `cargo deny check bans` FAIL with `error[banned]` for git2 + transitive libgit2-sys; reverted, lock cleaned). **`cargo audit`:** flagged **RUSTSEC-2026-0185** (`quinn-proto 0.11.14`, high, lock-resident but UNREACHABLE by any feature path — behind reqwest QUIC) → resolved by `cargo update -p quinn-proto` to 0.11.15 (semver-compatible, no code change); audit now clean. **Cargo.lock delta = +regex edge +quinn-proto bump.** **`.cargo/config.toml` + `rust-toolchain.toml` already present (T0.2)** — confirmed, not re-created. **Reconciliations (spec-first):** ci-cd §2 (per-job `Lands` column + DEFERRED ledger + new §2.2 targeted-features) + impl-plan T0.9 + spine §6 (conformance rules promoted to addressable `### 6.1`..`§6.7`) + ~20 doc cross-ref qualifications (bare `§N` → `spine §N`/`PRD §N`) that the doc-lint surfaced. **Known issue (pre-existing, NOT T0.9):** `unblock-storage::libsql::tests::open_in_memory_parallel_first_write_stress` is an intermittent `:memory:` parallel-open WAL flake (documented at T0.6) — the broad `test` job could flake on it; flagged for the Verify gate (T0.9 does not touch storage source). |

## M1 — Engine + core domain  *(gate: CRUD/ready/dep linearizable via engine)*

| id | Task | Depends on | Status | Notes |
|---|---|---|---|---|
| T1.1 | `unblock-policy` (ready-sort, gating, cache-key) | M0 | ☐ | |
| T1.2 | `unblock-engine` (session + write Semaphore, D14) | T1.1 | ☐ | |
| T1.3 | `unblock-config` (v1 subset, layered TOML) | T1.2 | ☐ | |
| T1.4 | Issue lifecycle FR-1a/1b/1c, FR-2, FR-3 | T1.2 | ☐ | |
| T1.5 | Querying FR-4 | T1.2 | ☐ | |
| T1.6 | Dependencies & graph FR-5 (petgraph) | T1.2 | ☐ | |

## M2 — MCP surface (primary)  *(gate: MCP client does ready→claim→close; bd import works)*

| id | Task | Depends on | Status | Notes |
|---|---|---|---|---|
| T2.1 | `unblock-render` (reduced) | M1 | ☐ | |
| T2.2 | `unblock-mcp`: rmcp stdio serve | T2.1 | ☐ | |
| T2.3 | MCP tool/resource/prompt taxonomy (7 tools) | T2.2 | ☐ | |
| T2.4 | `unblock-sync` (light JSONL export/import) | T2.2 | ☐ | |
| T2.5 | `bd` one-shot import FR-26 (generic bd→unblock) | T2.4 | ☐ | |
| T2.6 | Self-describing contracts FR-12 | T2.2 | ☐ | |
| T2.7 | Diagnostics FR-15 (pure-DB) | T2.2 | ☐ | |

## M3 — Reliability + ops + GA  *(gate: shutdown/failure-injection + perf budgets green)*

| id | Task | Depends on | Status | Notes |
|---|---|---|---|---|
| T3.1 | `unblock-cli` (lifecycle: serve/migrate/doctor/version/init/agents/update) | M2 | ☐ | |
| T3.2 | Cooperative shutdown FR-17 | T3.1 | ☐ | |
| T3.3 | `unblock-health` (lite) FR-16 | T3.1 | ☐ | |
| T3.4 | Reliability gates NFR-4/5 | T3.1 | ☐ | |
| T3.5 | Perf budgets NFR-1/2 | T3.1 | ☐ | |
| T3.6 | Release pipeline + `unblock update` (dist/axoupdater) FR-25 | T3.1 | ☐ | |
| T3.7 | Product `README.md` (root) | T3.1 | ☐ | install + MCP wiring docs |

## Future versions — deferred *(not scheduled)*

Intentionally **not decomposed into tasks** until v1 is substantially complete. They are reviewed and locked
**just-in-time** (near v1 GA), using real v1 learnings — not speculatively up front. v1 already reserves their
seams (libsql `remote` feature off-by-default D15, backend-agnostic `Storage` trait, NFR-3 remote backoff), so the
v1 design is not at risk. See `00-roadmap.md`.

- **v1.1** (LOCKED scope, backlog): FR-6, FR-13 (full), FR-16 (full taxonomy), FR-18, FR-19, FR-21, FR-22, FR-23, TOON.
- **v1.2 / v1.3 / v2+** (PROPOSED — direction only): see `00-roadmap.md`; reviewed when v1 nears GA.
