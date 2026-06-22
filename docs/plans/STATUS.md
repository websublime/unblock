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
| T0.5 | `unblock-storage`: `Storage` trait | T0.3, T0.4 | ☐ | |
| T0.6 | `unblock-storage`: libsql impl (WAL, busy_timeout) | T0.5 | ☐ | |
| T0.7 | Storage contract test suite (NFR-16) | T0.6 | ☐ | also wires the nested `unblock-fuzz` cargo-fuzz targets (nightly, `exclude`d from stable workspace — NFR-12), **incl. T0.4's deferred `fuzz_sanitize`** over `unblock_error::sanitize_message` **and T0.3's deferred `unblock-model` ingestion targets**: `serde_json::from_slice::<Issue>`, `parse_id`/`is_valid_id_format`, the hand-rolled open-enum `Deserialize` (Status/IssueType/DependencyType/EventType), and `compute_content_hash`. (T0.3 lands stable-side panic-safety **proptests** over arbitrary bytes/strings now — `tests/proptest_panic_safety.rs` — so the deferral is explicit + tracked, not silently dropped.) |
| T0.8 | **Contention lab (RK-1 / NFR-3 — M0 GATE)** | T0.6 | ☐ | no hot-spin before anything depends on storage; fallback = rusqlite behind the trait |
| T0.9 | CI scaffolding (ci.yml, deny.toml, .cargo/config, **doc-lint**) | T0.2 | ☐ | quality gate + doc-lint live from M0. *(Verify-gate follow-ups: `deny.toml [bans]` must deny `git2`/`gix`/`libgit2-sys` to machine-enforce NFR-6; the `layering` job must check out the committed `Cargo.lock` so `cargo metadata --offline` resolves. `.cargo/config.toml` already exists from T0.2.)* |

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
