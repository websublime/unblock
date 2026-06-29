# unblock — Version Roadmap

- **Status:** v1 + v1.1 **LOCKED** (derived from APPROVED PRD); v1.2 / v1.3 / v2-plus **PROPOSED** (for Miguel review)
- **Date:** 2026-06-19
- **Owner:** Miguel Ramos
- **Sources of truth:** `docs/PRD.md` (PRD APPROVED v1.1, §5 tiers / §11 scope / §13 phasing), `docs/plans/01-design-spine.md` (cross-crate interfaces — wins on any cross-crate type/signature disagreement), `docs/plans/implementation-plan.md` (v1 walking skeleton). Hierarchy: PRD > spine > crate plans. Grounding for deferred/later items: original `temp/beads_rust-main` feature inventory.
- **Stage:** Pre-1.0, no external users — breaking changes welcome, no migration / backward-compat burden (PRD header).

> This roadmap allocates the natural product evolution across releases. **v1 and v1.1 are locked** — they
> restate the PRD verbatim in intent and exist here only for a single horizon view. **v1.2 onward is a
> proposal**: a defensible allocation of PRD-deferred items + original-product capabilities, sequenced by
> dependency and value. Per-crate planners should treat v1/v1.1 as authoritative and v1.2+ as direction.

---

## 0. How to read this document

- **Theme/Goal** — the one-line reason the release exists.
- **Lands** — the FR/NFR ids and features delivered (FR ids trace to PRD §5; new proposed capabilities are tagged `[NEW]` and not yet PRD-blessed).
- **Crates touched** — which of the 12 workspace crates (PRD §8.1) take work.
- **Status** — `LOCKED` (PRD-approved) or `PROPOSED` (review candidate).

Acyclic layering is invariant across all releases (PRD §8.1 / NFR-15):
`model`/`error` → `policy` → `storage` → `sync`/`health` → `config` → `engine` → `render` → `mcp`/`cli`.

---

## 1. v1 — Walking skeleton (the agent-first thin slice)  **[LOCKED]**

**Theme:** Ship the defensible wedge — a local-first, dependency-aware issue store with atomic multi-agent
claim, a ready-work query, and MCP-over-stdio as the primary surface — proven non-spinning at swarm scale.

**Goal (PRD §14.1 ship-gates):** every v1-tier FR meets its AC; NFR-1 perf budgets + NFR-2 (250k in CI)
pass as hard gates; unblock dogfoods its own repo (issues imported from `bd` via FR-26).

### Lands (FRs)
| FR | Capability |
|---|---|
| FR-1a/1b/1c | Issue create / quick-create; show/update (multi-id, labels, reparent w/ cycle reject); tombstone delete (cascade/hard/dry-run); dedicated restore/un-tombstone (D20) |
| FR-2 | **Atomic claim** (assignee + `in_progress`, no race window) — the wedge |
| FR-3 | Scheduling: `defer` / `undefer` |
| FR-4 | Query surface: `list` / `ready` / `blocked` / `search` / `count` / `stale` (`ready` = canonical agent entrypoint) |
| FR-5 | Typed dependency edges + graph (`petgraph` traversal, `blocks`-cycle rejection) |
| FR-7 / FR-8 | Optional JSONL export/import (atomic write; conflict-marker + malformed-JSON rejection; path confinement) |
| FR-26 | One-shot best-effort `bd` → unblock import (D16) |
| FR-9 / FR-10 | Single shared engine; in-process write `Semaphore` (D14); read fast path |
| FR-11 / FR-12 | Agent contract: structured errors (`code`/`message`/`hint`/`retryable`), 0–8 exit codes + MCP error parity; self-describing `capabilities`/`schema` versioned by `contract_version` |
| FR-13 (subset) | Layered config: CLI > env (`UNBLOCK_*`) > project `config.toml` > defaults |
| FR-14 | Workspace bootstrap: `init [--prefix]`, `agents` (AGENTS.md) |
| FR-15 | Pure-DB diagnostics: stats/info/where/version/lint; `changelog` (closed-issue metadata) + `orphans` (`external_ref` pattern) — **no git** |
| FR-16 (lite) | `doctor` + libsql `integrity_check` + basic diagnostics |
| FR-17 | Cooperative shutdown (SIGINT/SIGTERM/SIGHUP → atomic flag; clean libsql flush/close) |
| FR-20 | **MCP stdio server (PRIMARY)** on rmcp 1.7: ≤8 consolidated tools, resources, prompts |
| FR-25 | **Self-update** (`unblock update`) via `axoupdater`; verified by GitHub artifact attestations (NFR-17, D17) |

### Key NFR gates
NFR-1 (perf budgets), NFR-2 (250k CI / 1M manual under single-serve topology), **NFR-3 (no hot-spin —
contention lab in M0 before any crate depends on storage)**, NFR-4/5 (atomic export + reliability gates),
NFR-6 (zero git), NFR-9 (`forbid(unsafe_code)`, pinned actions), NFR-14 (stdout/stderr discipline),
NFR-15 (acyclic layering), NFR-16 (Storage contract suite), NFR-18 (MCP untrusted-input boundary).

### Crates touched (all 12)
`unblock-model`, `unblock-error`, `unblock-policy`, `unblock-storage` (libsql, local default — `features = ["core"]`;
remote feature **off**, D15), `unblock-sync` (light), `unblock-health` (lite), `unblock-config` (subset),
`unblock-engine`, `unblock-render` (reduced), `unblock-mcp` (primary), `unblock-cli` (lifecycle: serve/
migrate/doctor/version/update), `unblock-fuzz`.

### Milestones (PRD §13 / plan §2–5)
M0 Foundation → M1 Engine + core domain → M2 MCP surface → M3 Reliability + ops.

---

## 2. v1.1 — Organization, coordination & ergonomics  **[LOCKED]**

**Theme:** Layer the human/swarm-orchestration ergonomics on top of the proven core — the features the PRD
deliberately deferred out of the thin slice but committed to (PRD §5 `[v1.1]` items, §11, §13 row "v1.1+").

**Goal:** close the explicitly-deferred backlog without changing the storage topology or the agent contract's
shape (only additive `contract_version` bumps).

### Lands (FRs)
| FR | Capability | Crates |
|---|---|---|
| FR-6 | **Organization:** labels (rename/list-all), threaded comments (add/list), epic rollups + auto-close-eligibility | model, storage, engine, mcp |
| FR-1c (D20 seams) | **Restore extensions:** cascade-restore (needs a delete-batch identity to avoid over-reviving independently-tombstoned children) + TTL-refusal of expired tombstones (`deletions_retention_days`, reserved/unenforced in v1) | model, storage, engine, mcp |
| FR-18 | **Swarm coordination diagnostics:** `scheduler` (ranked, explainable `unblock.scheduler.v1`); `coordination status` (`unblock.coordination.v1`, read-only stale-claim diagnosis). Purely DB-state-derived (Agent Mail dropped, PRD §12) | policy, engine, mcp |
| FR-19 | **Workflow gates:** policy-driven (`.unblock/policy.toml`) transition gates (ci_green / min_reviewers / security_sign_off) | policy, config, engine, mcp |
| FR-13 (full) | DB config-table + user-config layers; full startup/runtime partitioning | config, storage, engine |
| FR-16 (full) | Full Healthy/Drifted/Recoverable/Unsafe taxonomy redefined for a libsql-authoritative world; evidence under `.unblock/.recovery/` | health, engine, cli |
| FR-21 | Saved queries (named reusable `list` filter sets) | policy, storage, engine, mcp |
| FR-22 | Audit / flight recorder: append-only `interactions.jsonl`, Tier-1 attribution (capture-only) | engine, sync, mcp |
| FR-23 | Shell completions (bash/zsh/fish/powershell/elvish) | cli |
| — | **TOON output** (feature-gated) in render | render |

### Crates touched
Primarily `unblock-policy`, `unblock-health`, `unblock-config`, `unblock-engine`, `unblock-mcp`,
`unblock-render`, `unblock-cli`. MCP surface grows: tools/resources for
labels/comments, scheduler, coordination, gates, saved-queries (plan §6).

---

## 3. v1.2 — Shared state: libsql remote / replica sync  **[PROPOSED]**

**Theme:** Turn the locked-in "credible shared-state path" (PRD value prop, D1/D15) from a feature flag into a
real product capability — the path from single-workspace local to a shared service, without ever becoming a
bespoke hosted server.

**Why now:** D15 deliberately ships the remote/replica feature **off by default** in v1 ("v1.2 territory" per
the project brief). The seam exists; v1.2 lights it up. This is the single largest deferred capability and the
natural next product step once the local core is hardened.

**Goal:** multiple agents/humans across machines share one logical issue store via libsql sync, with
offline-first still intact and credentials handled safely — and the non-spin guarantee extended to the remote
path (NFR-3's secondary jittered-backoff fallback finally exercised in anger).

### Lands (features)
| Item | Capability | Trace |
|---|---|---|
| `[NEW]` Remote/replica feature GA | Promote the non-default libsql remote/embedded-replica feature to a supported, documented build; embedded-replica local-read + remote-write-back | D1/D15, NFR-10 |
| `[NEW]` Credential handling | libsql auth tokens via `UNBLOCK_*` env **or** OS keychain only — never `config.toml` (NFR-18 already mandates this; v1.2 implements the keychain path) | NFR-18 |
| FR-13 sync layers | Config precedence extended for remote endpoints / sync intervals (startup-only keys) | FR-13 |
| `[NEW]` Sync-mode health | `doctor` + health taxonomy extended: replica lag, sync conflicts, WAL-on-remote integrity; "Drifted" gains a remote meaning | FR-16 (full) |
| `[NEW]` Multi-workspace discovery | Limited multi-workspace handling for the shared case (one operator, several synced workspaces) — **explicitly NOT** the dropped town/mayor routing (FR-24/D11); scoped to remote-sync addressing only | distinct from D11 |
| `[NEW]` Resilience GA | The remote-only jittered backoff (`backon`/`tokio-retry`, never archived `backoff 0.4`) + `failsafe` circuit-breaker validated under a remote contention lab; `wiremock` coverage promoted to a remote contract suite | NFR-3, NFR-16 |
| `[NEW]` Concurrency model extension | Revisit D14: define the supported topology when writes can originate from multiple synced serve processes (per-workspace single-writer still holds locally; remote reconciliation rules defined) | D14 follow-up |

### Crates touched
`unblock-storage` (remote/replica impl + sync semantics — the heart of this release), `unblock-sync`
(reconciliation seams if any), `unblock-health` (sync diagnostics), `unblock-config` (remote endpoints +
keychain credential resolution), `unblock-engine` (write topology under sync), `unblock-mcp` (sync-status
resources). `unblock-model`/`unblock-error` only if a sync-state type or error variant is needed.

### Risks / open questions for review
- Does the remote path keep the offline-first promise intact (queue-and-reconcile vs hard-online)?
- Multi-writer reconciliation: last-write-wins on `content_hash` vs operation log — needs a decision before build.
- Keychain portability across Linux/macOS/Windows (NFR-11) — may need per-OS backends.
- TLS/HTTP transitive surface only enters builds that opt into remote (NFR-10 must stay green on default build).

---

## 4. v1.3 — Scale, swarm coordination depth & MCP surface richness  **[PROPOSED]**

**Theme:** Push the wedge from "correct at 250k" to "fast and rich at 1M issues / 10k agents", and deepen the
swarm-orchestration story beyond v1.1's read-only diagnostics.

**Why now:** v1 validates 1M only as a *manual* corpus (NFR-2); v1.1 ships coordination as read-only
*diagnostics*. v1.3 hardens both into supported, performant, actively-helpful capabilities once shared state
(v1.2) is real.

**Goal:** 1M-issue performance is a CI hard gate, not a manual exercise; the scheduler/coordination contracts
gain active assistance (not just observation); the MCP surface gets richer without bloating the tool list.

### Lands (features)
| Item | Capability | Trace |
|---|---|---|
| `[NEW]` 1M-issue perf as CI gate | Promote NFR-2's manual 1M / 10k-agent corpus to an automated regression gate; index/query tuning (the original's `workitems_ready_index` lesson) | NFR-1/NFR-2 |
| `[NEW]` Active coordination | Beyond `coordination status`: stale-claim **reclaim** policy, claim TTLs/heartbeats, deterministic re-assignment evidence — still DB-derived, still no Agent Mail | FR-18 extension |
| `[NEW]` Scheduler v2 | Richer ranking signals (cost/estimate-aware, critical-path-aware via `petgraph`), still a pure versioned `unblock.scheduler.v2` contract | FR-18 / policy |
| `[NEW]` Richer MCP surface | Streaming/large-result resources, batch tools, subscription-style change notifications — measured against the tool-count budget (RK-3); resources preferred over new tools | FR-20 / PRD §9 |
| `[NEW]` Compaction / archival | Activate the model's compaction fields (kept for JSONL fidelity, D12) as a real archival path for very large stores; restore-from-snapshot | D12, domain model |
| `[NEW]` Performance observability | `tracing`-based perf spans + a `criterion` dashboard; contention-lab generalized to a continuous load harness | NFR-13 |

### Crates touched
`unblock-storage` (index/query tuning, archival), `unblock-policy` (scheduler v2, reclaim contracts),
`unblock-engine` (claim TTL/heartbeat, compaction orchestration), `unblock-mcp` (richer surface, batch/
streaming), `unblock-health` (scale diagnostics), `unblock-render` (large-result formatting). Bench-only
touches to `unblock-fuzz`/harnesses.

### Risks / open questions for review
- Claim TTL/heartbeat changes the contract — must be additive (`contract_version` bump, not breaking).
- 1M as a *required* CI gate may be slow/expensive — may need a sampled or scheduled (nightly) gate.
- Compaction interacts with JSONL round-trip fidelity (D12) — round-trip property tests must extend to compacted issues.

---

## 5. v2-plus / later horizon  **[PROPOSED — direction only]**

**Theme:** Capabilities that are credible long-term but deliberately *not* committed — they either reverse a
locked decision, need a concrete external demand, or imply a materially larger product surface.

| Candidate | Notes / why later |
|---|---|
| Cross-project / multi-repo routing (the original's town/mayor) | **Explicitly dropped in v1 (FR-24/D11)**; reintroduce *only* on a concrete multi-repo demand, and likely in a shape informed by v1.2 multi-workspace sync rather than the original's elaborate mayor design |
| Human-facing surface | PRD makes the human developer **secondary/future** (persona table); D3 forbids a domain CLI. A richer human surface (TUI / web client over MCP, or a relaxation of D3) would need an explicit decision — it contradicts a locked principle today |
| DB-only mode (drop JSONL entirely) | D5 keeps JSONL as optional and notes the design is "reversible toward DB-only later" — a candidate once sync (v1.2) makes JSONL redundant for the shared case |
| Hosted / managed shared service | PRD §11 keeps this out of scope (collaboration is via libsql sync, not a bespoke server). Only revisit if v1.2 sync proves insufficient for real teams |
| Pluggable alternative storage backends | The `Storage` trait + contract suite (NFR-16) make this *possible*; a second backend would only ship on concrete demand (the trait exists precisely so this is cheap when needed) |
| Additional MCP transports (beyond stdio) | D2 locks stdio as primary; an HTTP/SSE transport would follow the same isolation discipline if a non-CLI client demands it |

These are intentionally unscheduled. Each requires a product decision (and several reverse a locked §4
decision) before it can leave this list.

---

## 6. Feature-to-version matrix

Legend: ● lands · ◐ extended/hardened · `[NEW]` not yet in PRD FR set · L=LOCKED, P=PROPOSED

| Feature / FR | v1 (L) | v1.1 (L) | v1.2 (P) | v1.3 (P) | v2+ (P) |
|---|:--:|:--:|:--:|:--:|:--:|
| Issue CRUD + tombstone delete (FR-1) | ● | | | | |
| Atomic claim (FR-2) | ● | | | ◐ TTL/heartbeat | |
| Defer/undefer (FR-3) | ● | | | | |
| Query: list/ready/blocked/search/count/stale (FR-4) | ● | | | | |
| Typed deps + graph (FR-5) | ● | | | | |
| Labels / comments / epic rollups (FR-6) | | ● | | | |
| JSONL export/import (FR-7/8) | ● | | | | ◐ DB-only option |
| `bd` one-shot import (FR-26) | ● | | | | |
| Shared engine + write Semaphore + read fast path (FR-9/10) | ● | | ◐ sync topology | ◐ TTL | |
| Agent contract + exit codes + capabilities/schema (FR-11/12) | ● | | | ◐ richer | |
| Layered config (FR-13) | ● subset | ● full | ◐ remote keys | | |
| Workspace bootstrap (FR-14) | ● | | ◐ multi-ws | | |
| Pure-DB diagnostics (FR-15) | ● | | | | |
| Workspace health (FR-16) | ● lite | ● full | ◐ sync health | ◐ scale | |
| Cooperative shutdown (FR-17) | ● | | | | |
| Swarm coordination / scheduler (FR-18) | | ● diagnostics | | ◐ active + v2 | |
| Workflow gates (FR-19) | | ● | | | |
| MCP stdio server (FR-20) | ● | ◐ surface | ◐ sync resources | ◐ batch/stream | ◐ new transports |
| Saved queries (FR-21) | | ● | | | |
| Audit / flight recorder (FR-22) | | ● | | | |
| Shell completions (FR-23) | | ● | | | |
| Cross-project routing (FR-24) | ✗ dropped | | | | ◐ reconsider |
| Self-update (FR-25) | ● | | | | |
| TOON output | | ● | | | |
| **libsql remote/replica sync** `[NEW]` | (off, D15) | | ● GA | | |
| **Credential / keychain handling** `[NEW]` | | | ● | | |
| **Multi-workspace (sync-scoped)** `[NEW]` | | | ● | | |
| **1M-issue perf as CI gate** `[NEW]` | (manual) | | | ● | |
| **Compaction / archival activation** `[NEW]` | (fields only) | | | ● | |
| **Pluggable backends / hosted service / human surface** `[NEW]` | | | | | ◐ |

---

## 7. Crate-impact summary across releases

| Crate | v1 | v1.1 | v1.2 | v1.3 | v2+ |
|---|:--:|:--:|:--:|:--:|:--:|
| `unblock-model` | ● | ● | ◐ | ◐ | |
| `unblock-error` | ● | ● | ◐ | ◐ | |
| `unblock-policy` | ● | ● | | ● | |
| `unblock-storage` | ● | ◐ | ● | ● | ◐ |
| `unblock-sync` | ● | ◐ | ◐ | ◐ | ◐ |
| `unblock-health` | ● lite | ● full | ● | ● | |
| `unblock-config` | ● subset | ● full | ● | | |
| `unblock-engine` | ● | ● | ● | ● | |
| `unblock-render` | ● | ● | | ◐ | |
| `unblock-mcp` | ● | ● | ◐ | ● | ◐ |
| `unblock-cli` | ● | ● | | | |

---

## 8. Sequencing rationale (one paragraph)

v1 proves the wedge is *correct* (atomic claim + no hot-spin at 250k); v1.1 makes it *ergonomic* for swarms and
humans-via-clients (coordination diagnostics, gates, organization); v1.2 makes it *shared* (the libsql remote/
replica path the architecture was built to enable, D1/D15); v1.3 makes it *fast and actively helpful at the top
of the scale curve* (1M as a CI gate, active coordination, richer MCP). Everything in v2-plus either reverses a
locked decision or awaits concrete external demand and is therefore deliberately unscheduled. The acyclic
layering and the `Storage` trait/contract suite are the two invariants that make this sequence cheap: remote
storage (v1.2) and alternative backends (v2+) slot in behind the trait without touching callers.
