# CLAUDE.md — unblock

Operational contract for anyone (human or agent) working in this repo. Keep it lean; it is always in context.
**Pointers over prose** — the authoritative detail lives in `docs/`.

## What this is

**unblock** — a ground-up, idiomatic, multi-crate **Rust** rewrite of the deprecated `beads_rust` "agent-first
issue tracker". **MCP-first**: every domain feature is an MCP tool/resource/prompt over stdio; the CLI is
lifecycle/ops only. GA 1.0.0 — semver stability applies from GA (D35): the MCP contract, CLI surface, and 0–8 exit
codes are stable, a breaking change → 2.0.0. (Original source for reference only: `temp/beads_rust-main`.)

## Document map (read before working)

| Doc | Role |
|---|---|
| `docs/PRD.md` | Product truth — decisions (§4 D1..D37), FR/NFR, domain model, milestones. **APPROVED v1.1.** |
| `docs/plans/01-design-spine.md` | **Authoritative interface contract** (types, `Storage` trait, `Session` API, MCP schemas, errors). |
| `docs/plans/implementation-plan.md` | Task DAG M0–M3 (T-ids) + acceptance criteria. |
| `docs/plans/STATUS.md` | **Live task registry** — what's done / in-progress / to-do (the system of record). |
| `docs/plans/crates/unblock-*.md` | Per-file plan for each crate. |
| `docs/plans/00-roadmap.md` | v1/v1.1 LOCKED; v1.2–v1.5/v2+ PROPOSED. |
| `docs/plans/ci-cd-and-distribution.md` | CI gates + `dist` release + the doc-lint. |

**Rule:** the spine is the **reference** on interface disagreements — but a plan↔spine drift is reconciled **with
Miguel** (review → iterate → adjust): usually the plan is fixed to match the spine, but the drift may expose a
*spine* bug. **Never overwrite either side silently.** Surface every drift/gap and **resolve it in the same
session** by default (template: `docs/plans/templates/drift-gap-report.md`).

## Hard rules

- **Never decide to simplify the solution.** If you reach a point where simplifying seems necessary, **stop and ask**.
- **Ask before anything hard to reverse** or outward-facing (publishing, deleting, network sends).
- **A task/plan description is never authoritative** — always read the referenced spec (spine/PRD §) before implementing.
- **Semantic search (`ccc`/cocoindex) is discovery, not authority** — it locates `file:line` ranges; it **never** replaces a full read of the authoritative spec (spine/PRD) or original source (`temp/beads_rust-main`) when exact fidelity matters (byte layouts, complete rule sets, contract field sets). A returned chunk is a pointer, not the whole answer.
- **Converse in Portuguese; write all artifacts (code, docs, comments) in English.**

## Architecture & layering (NFR-15 — enforced, acyclic)

`model`/`error` (L0) → `policy` (L1) → `storage` (L2) → `sync`/`health` (L3) → `config` (L4) → `engine` (L5) →
`render` (L6) → `mcp`/`cli` (L7). Edges point downward only; **no back-edges, no cycles** (CI checks this).

- `unblock-storage` and `unblock-render` and `unblock-policy` depend on **model + error only**.
- `unblock-engine` is the **single mutation home** (FR-9): MCP and CLI are thin adapters over `Session`, so they
  cannot drift. Writes serialize through an **in-process tokio `Semaphore`** (D14) that serializes within one MCP
  server; cross-process writes are serialized by the D31 `.write.lock` advisory lock — the supported topology is
  **child-per-client** (D31), not single-MCP-server-per-workspace.
- `unblock-cli` → `unblock-mcp` (cli owns the binary; mcp is a library exposing `run_mcp_server()`); **never** mcp → cli.
- Cross-crate types are **defined once in `unblock-model`** and re-exported (never redefined) — see spine §1.10.

## Idiomatic Rust

- **Edition 2024**, **stable toolchain `1.96.0`** (no nightly without a documented reason).
- `#![forbid(unsafe_code)]` in every crate. Clippy **pedantic** clean (deny on CI).
- **Errors: `snafu`, per-crate** enums with context selectors. No god-enum; no backend (libsql) error leakage.
  Map to MCP error data / the 0–8 exit-code table **only at the L7 boundary** (`unblock-error` owns the taxonomy).
- **Async throughout (tokio).** `Storage` is an `async_trait`. No blocking calls on async paths.
- No `unwrap`/`expect`/`panic!` in library code paths (tests excepted); return typed errors.
- Dependency graph for issue deps via `petgraph` (don't hand-roll cycle detection).

## Conventions

- Names: binary `unblock`; crates `unblock-*`; workspace dir `.unblock/`; DB `unblock.db`; optional export
  `issues.jsonl`; config `config.toml`; contract ids `unblock.*.v1`; env prefix `UNBLOCK_` (`UNBLOCK_DIR`,
  `UNBLOCK_ACTOR`, `UNBLOCK_JSONL`, `UNBLOCK_OUTPUT_FORMAT`).
- Crates `unblock-*` are **workspace-internal (not published to crates.io)**; only the `unblock` binary ships (via `dist`).
- Config is **TOML**. Output: **structured to stdout, diagnostics to stderr** (NFR-14); output shapes are
  snapshot-pinned (`insta`).
- **No git operations, no git library linked, no network on any normal command path** (D13/NFR-6). Network only
  on explicit `unblock update` (axoupdater runs the dist installer; SHA256-checksum-verified before swap). libsql `remote` feature is **off by default**.
- libsql is the source of truth; JSONL is an **optional** export/import (no 3-way merge, no locks) — model B (D5).

## Testing (`cargo test`; no Go/Encore tooling here)

Per crate: unit + `proptest` (invariants) + `insta` snapshots (`cargo insta test --check` on CI) +
`cargo-fuzz` (ingestion) + `criterion` (perf-sensitive). The **Storage contract suite** (NFR-16) and the
**contention lab** (NFR-3, M0 gate — must prove no CPU hot-spin) are hard gates. `wiremock` for any remote path.

## Process

How we work — lifecycle, decisions, multi-agent orchestration, review & tracking discipline — lives in the
process guide, imported so every session loads it:

@docs/PROCESS.md

Essentials: every lifecycle phase runs as a **hand-picked team** (PROCESS.md §4 — specialist mates +
`multi-agent-coordinator`, spawned as a Workflow). **The main session is the *orchestrator*, not an implementer**:
it assigns teams (incl. **Implement**), awaits outcomes, and decides/acts — it hand-writes only conversational or
one-line/trivial edits. Substantive, multi-file, and multi-crate work (including scaffolding) is **always** a spawned
team, which writes in an **isolated worktree** (never in the shared tree). Take the next **ready** task from
`docs/plans/STATUS.md`. Branch off `main`. A change ships only after the **design Review** and the **Verify quality gate** (each ≥3
agents) pass → **Conventional, atomic commits** (`git-workflow-manager`) → **Claude opens the PR** (`gh pr
create`); a human merges → on merge, flip the `STATUS.md` task to ☑ done.

## Build / CI / distribution

Stable `cargo`; `cargo-audit` + `cargo-deny` (also bans any git crate, enforces the transitive budget); GitHub
Actions pinned to 40-char SHAs. Release via **`dist`** (5 target triples, shell+powershell installers, GitHub
artifact attestations, `axoupdater`-backed `unblock update`). See `docs/plans/ci-cd-and-distribution.md`.
