# unblock — Version Roadmap

- **Status:** v1 + v1.1 **LOCKED** (derived from APPROVED PRD); v1.2–v1.5 / v2-plus **PROPOSED** (for Miguel review)
- **Date:** 2026-06-19 · **v1.2–v1.5 resequence:** 2026-07-07 (session ratified by Miguel; audience decision minted as PRD §4 D28) · **post-GA v1.2+ resequence:** 2026-07-20 (Miguel-ratified; the PROPOSED v1.2–v1.5 order is reworked and a v1.0.1 maintenance-patch slot added — full mapping in roadmap §8, rationale + guardrail in PRD §4 D41)
- **Owner:** Miguel Ramos
- **Sources of truth:** `docs/PRD.md` (PRD APPROVED v1.1, §5 tiers / §11 scope / §13 phasing), `docs/plans/01-design-spine.md` (cross-crate interfaces — wins on any cross-crate type/signature disagreement), `docs/plans/implementation-plan.md` (v1 walking skeleton). Hierarchy: PRD > spine > crate plans. Grounding for deferred/later items: original `temp/beads_rust-main` feature inventory; UX grounding for the v1.4 TUI: the TUI-adopted subset of the 14 mockups under `temp/tentative-v2/docs/designs/` (**reference-only**, same status as `temp/beads_rust-main`; the graph / burnup screens there inform the v2+ PRO web instead — roadmap §7).
- **Stage:** GA 1.0.0 — semver stability applies from GA (D35); breaking changes → 2.0.0 (supersedes the pre-1.0 posture; PRD header).

> This roadmap allocates the natural product evolution across releases. **v1 and v1.1 are locked** — they
> restate the PRD verbatim in intent and exist here only for a single horizon view. **v1.2 onward is a
> proposal**: a defensible allocation of PRD-deferred items + original-product capabilities + the 2026-07-07
> audience decision (PRD §4 D28 — mixed human+agent company teams over shared state; humans via MCP clients),
> sequenced by dependency and value. Per-crate planners should treat v1/v1.1 as authoritative and v1.2+ as direction.

---

## 0. How to read this document

- **Theme/Goal** — the one-line reason the release exists.
- **Lands** — the FR/NFR ids and features delivered (FR ids trace to PRD §5; new proposed capabilities are tagged `[NEW]` and not yet PRD-blessed).
- **Crates touched** — which of the 12 workspace crates (PRD §8.1) take work. The proposed v1.4 `unblock-tui` crate (roadmap §5/§9) would be a 13th; it is minted only at v1.4 lock (PRD §8.1 is unchanged until then).
- **Status** — `LOCKED` (PRD-approved) or `PROPOSED` (review candidate). PROPOSED versions are **direction, locked just-in-time** as each nears its build window; every per-version tech/scope call below is **re-confirmed at that version's lock** with fresh research and real learnings.

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
| FR-26 | One-shot best-effort `bd` → unblock import (D16/D24) |
| FR-9 / FR-10 | Single shared engine; in-process write `Semaphore` (D14) + cross-process advisory `.unblock/.write.lock` (D31); read fast path |
| FR-11 / FR-12 | Agent contract: structured errors (`code`/`message`/`hint`/`retryable`), 0–8 exit codes + MCP error parity; self-describing `capabilities`/`schema` versioned by `contract_version` |
| FR-13 (subset) | Layered config: CLI > env (`UNBLOCK_*`) > project `config.toml` > defaults |
| FR-14 | Workspace bootstrap: `init [--prefix]`, `agents` (AGENTS.md) |
| FR-15 | Pure-DB diagnostics: stats/info/where/version/lint; `changelog` (closed-issue metadata) + `orphans` (`external_ref` pattern) — **no git** |
| FR-16 (lite) | `doctor` + libsql `integrity_check` + basic diagnostics |
| FR-17 | Cooperative shutdown (SIGINT/SIGTERM/SIGHUP → atomic flag; clean libsql flush/close) |
| FR-20 | **MCP stdio server (PRIMARY)** on rmcp 1.7: ≤8 consolidated tools, resources, prompts |
| FR-25 | **Self-update** (`unblock update`) via `axoupdater`; verified by the dist-installer SHA256 checksum before swap; attestations = publish-side provenance (NFR-17, D17/D35) |

### Key NFR gates
NFR-1 (perf budgets — hybrid gate, D34), NFR-2 (250k CI / 1M manual under the child-per-client topology, D14+D31), **NFR-3 (no hot-spin —
contention lab in M0 before any crate depends on storage)**, NFR-4/5 (atomic export + reliability gates),
NFR-6 (zero git), NFR-9 (`forbid(unsafe_code)`, pinned actions), NFR-14 (stdout/stderr discipline),
NFR-15 (acyclic layering), NFR-16 (Storage contract suite), NFR-18 (MCP untrusted-input boundary).

### Crates touched (all 12)
`unblock-model`, `unblock-error`, `unblock-policy`, `unblock-storage` (libsql, local default — `features = ["core"]`;
remote feature **off**, D15), `unblock-sync` (light), `unblock-health` (lite), `unblock-config` (subset),
`unblock-engine`, `unblock-render` (reduced), `unblock-mcp` (primary), `unblock-cli` (lifecycle: mcp/
migrate/doctor/version/update), `unblock-fuzz`.

### Milestones (PRD §13 / plan §2–5)
M0 Foundation → M1 Engine + core domain → M2 MCP surface → M3 Reliability + ops.

---

### v1.0.1 — maintenance patch  **[PLANNED]**

A small, orthogonal-to-the-resequence patch slot (recorded 2026-07-20; PRD §4 D41): a bugfix/maintenance
release cut on top of GA **before** v1.1, carrying the concrete dogfood defects found running unblock on its
own repo plus the one release-pipeline gap never exercised end-to-end:
- **`comment add` silent no-op** (P0) — a dogfood-found defect where an add returns success but persists nothing
  (the `add` payload field mismatch surfaced while recording outcomes-as-comments); fix so a comment is durably stored.
- **Missing forward-migration for the comments schema** (P1) — the D37 comments tables need a forward migration
  so long-lived DBs (not just fresh-created ones) pick up the schema (the D-feedback "editing an applied
  migration drifts long-lived DBs" lesson — ship a *forward* migration, never an in-place edit).
- **`unblock update` end-to-end smoke** — the self-update path (FR-25, axoupdater → dist installer → SHA256
  check-before-swap) has never been run end-to-end against a real published release; add the smoke so the GA
  self-update promise is exercised, not just unit-asserted.

This slot is **maintenance only** — no FR is added or re-tiered; it does not touch the v1.2+ resequence below.

---

## 2. v1.1 — Organization, coordination & ergonomics  **[LOCKED]**

**Theme:** Layer the human/swarm-orchestration ergonomics on top of the proven core — the features the PRD
deliberately deferred out of the thin slice but committed to (PRD §5 `[v1.1]` items, §11, §13 row "v1.1+").

**Goal:** close the explicitly-deferred backlog without changing the storage topology or the agent contract's
shape (only additive `contract_version` bumps).

### Lands (FRs)
| FR | Capability | Crates |
|---|---|---|
| FR-6 | **Organization (v1.1 remainder):** labels (rename/list-all), epic rollups + auto-close-eligibility. *(**Comments graduated to v1 — D37**: FLAT full-CRUD comments — add/list/update/delete — over the dedicated `comment` tool, landing at T3.9 and HOLDING the `v1.0.0` GA tag; NOT the "threaded (add/list)" v1.1 sketch.)* | model, storage, engine, mcp |
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

## 3. v1.2 — Planning layer: goals + milestones  **[PROPOSED]**

**Theme:** Give the store a first-class planning layer with a clean **semantic triad** (ratified
2026-07-07): **Goal = why** (outcome; success ≠ completion; cross-cutting) · **Epic = what** (exists, FR-6) ·
**Milestone = when** (time-boxed delivery bucket). No overlap between the three.

**Why now (planning-first — resequenced 2026-07-20; PRD §4 D41):** two real drivers pull planning ahead of
shared state. **(a) Felt dogfood demand** — the team already **hand-builds epics, milestones and priorities**
to run unblock's own roadmap today; a first-class planning layer is the capability most missed in daily use, so
it earns the next committed slot. **(b) Schema-before-distribution** — the planning layer is an **additive
schema change** (`Issue.milestone_id` + the Milestone and Goal entities); settling it on **one cheap local file
BEFORE replication** is far cheaper than migrating a schema across a primary + version-skewed embedded replicas
once shared state (v1.3) is live. Land the schema while there is exactly one writer, then distribute it.
Sequencing still holds downstream: the v1.5 scheduler v2 consumes milestone due dates / critical path — the
planning layer lands first precisely so those ranking signals exist (roadmap §6).

**Goal:** first-class milestones + slim first-class goals + ONE consolidated MCP planning surface — a
**pure-local release** (no network dependency).

### Lands (features)
| Item | Capability | Trace |
|---|---|---|
| `[NEW]` Milestone entity (first-class) | An entity, **NOT an issue**: id, title, optional description, optional due_date, state open/closed, created_at/closed_at. `Issue` gains an optional `milestone_id` — **exactly one milestone per issue** (GitHub-style; ratified). Derived rollups per milestone (the D26 `epic_child_rollup` precedent: SQL-ordered aggregate) | domain model; D26 precedent |
| `[NEW]` Milestone-scoped queries | `ready`/`list` gain a milestone filter — ready-work per release is the flagship agent feature; `changelog` gains a milestone filter (release notes from closed issues); stats gain per-milestone counters; FR-19 gate **candidate** on milestone close (no open issues / move them) | FR-4, FR-15, FR-19 |
| `[NEW]` Goal entity (first-class, slim) | An entity: id, title, **success_criteria required**, state open/achieved/missed/abandoned, optional outcome_note on close. **Many-to-many links** to issues/epics/milestones via its own link table (the dependencies table stays strictly issue↔issue). **NO metric automation** in the first cut — success is assessed by a human/agent at close. Value for agents: steering context (the *why*) attached to work | D28 (steering context) |
| `[NEW]` Consolidated `planning` MCP tool | ONE tool (verbs: create/update/close/assign/link…) + read resources, respecting the RK-3 tool-count budget; **additive `CONTRACT_VERSION` bump** | FR-20, FR-12 |
| `[NEW]` JSONL export fidelity | The milestones/goals layout in the export (own files vs sections) is an explicit design point **resolved at v1.2 lock** | D5, D12 |

Notes:
- **bd import (FR-26):** no mapping needed — bd has neither concept.
- **Dogfood:** once landed, unblock models its own roadmap/milestones natively (the PRD §13 dogfood-tracking
  note extends naturally).

### Lock-time forks *(recorded 2026-07-07 from the UX mockups; NOT decided now — resolved at v1.2 lock)*
- **Recursive milestones** (quarter ⊃ sprint nesting + derived status, as the mockup roadmap screen shows)
  vs flat-with-`parent_id`-seam vs flat. (The v1.4 roadmap screen inherits this fork — roadmap §5.)
- **Typed/structured comments** — an optional kind/status on FR-6 comments (trail narrative:
  investigation/decision/deviation/completed) — additive; a candidate for v1.2 scope.

### Crates touched
`unblock-model` (Milestone/Goal types), `unblock-storage` (tables + rollups), `unblock-engine` (mutations +
queries), `unblock-mcp` (the `planning` tool + resources), `unblock-render` (formatting), `unblock-policy`
(gates/scheduler seams). Pure-local: no network dependency. (Feature-to-version matrix roadmap §8 + crate
table roadmap §9 updated accordingly.)

---

## 4. v1.3 — Shared state: one primary, many machines (mixed human+agent teams)  **[PROPOSED]**

**Theme:** Shared state for **mixed human+agent teams** (PRD §4 D28): ONE logical issue store shared with many
machines — dev laptops, CI runners, cloud agents — via libsql **embedded replicas**. Not merely "turn on the
libsql feature": v1.3 is the release where unblock becomes a team product over shared state. The local-only
single-workspace deployment is the initial test phase, not the product's end state.

**Why now:** D15 deliberately ships the remote/replica feature **off by default** in v1 (deferred as later
"shared-state territory" per the project brief). The seam exists; v1.3 lights it up — and D28 makes shared state
the product's committed direction, not an optional add-on. This is the single largest deferred capability, taken
up **once the local core is hardened AND the planning schema (v1.2) is settled** — schema-before-distribution
(PRD §4 D41): the additive planning tables are far cheaper to land on one local primary than to migrate across
version-skewed replicas afterward. **A mandatory "Turso Sync vs embedded replicas — fresh Rust-SDK/engine
maturity check" gate is folded into the v1.3 lock** (embedded replicas are now the vendor-*legacy* path; Turso
Sync is the vendor-recommended-but-*beta* path — the lock re-runs this call with fresh research). **A SECOND
axis — research this FIRST (Miguel, 2026-07-20; tracked as issue `ub-w3a`):** WHERE the primary lives —
**self-hosted libsql/sqld** (run the libsql server yourself per its USER_GUIDE —
`github.com/tursodatabase/libsql/blob/main/docs/USER_GUIDE.md` — Docker-local to experiment NOW, self-deploy
later; company data stays self-governed) **vs the managed Turso Cloud**. Miguel's steer: prefer self-hosted
(kick off a Docker-local spike early to de-risk the largest lift); Turso Cloud is the fallback if self-hosting
isn't viable.

**Tech default (decided 2026-07-07; re-confirmed at v1.3 lock):** build v1.3 on **libsql embedded replicas** —
production-supported today, and the vendor's own "battle-tested foundation" recommendation for mission-critical
use. Honest dated note (as of 2026-07): the vendor now recommends the newer **Turso Sync** (built on the beta
Turso Database engine) for NEW sync projects — i.e. embedded replicas are the vendor-legacy path. The **Turso
Sync migration is an explicit v2+ candidate** (roadmap §7), kept cheap behind the `Storage` trait + the NFR-16
contract suite. The embedded-replicas choice is **re-confirmed at v1.3 lock with fresh research** (libsql crate
status, Turso Sync maturity). Sources (as of 2026-07): docs.turso.tech/libsql, github.com/tursodatabase/turso,
github.com/tursodatabase/libsql, turso.tech blog (sync-benchmark, offline-writes beta, local-first).

**Goal:** multiple humans and agents across machines share one logical issue store with equal stakeholder
footing (PRD §4 D28): reads stay local (embedded replica), writes serialize at the primary, credentials are
handled safely — and the non-spin guarantee extends to the remote path (NFR-3's secondary jittered-backoff
fallback finally exercised in anger).

**Offline stance (decided 2026-07-07):** in a remote workspace, **writes require network** — a failed remote
write is a clean structured error (`retryable=true`), never a silent queue; **reads stay local** via the
embedded replica, so **offline = read-only**. **NO queue-and-reconcile in v1.3** (integrity-first: correctness
over convenience). Offline write reconciliation is revisited only if/when Turso Sync is adopted (roadmap §7 —
it designs for that natively).

**Concurrency (D14 extension, decided 2026-07-07; the D14 amendment lands at v1.3 lock):** per-replica single-writer stays; **global serialization
at the primary** — the atomic claim (FR-2) resolves cross-machine at the primary. Explicit performance
contract, stated so nobody expects otherwise: **all writes serialize at the primary, reads scale via replicas,
no multi-master semantics.**

**Distribution pattern:** remote stays a **non-default Cargo feature**; the **`dist` release artifacts enable
it** (dev `cargo build` stays slim — NFR-10 —; shipped binaries are full). Final call at v1.3 lock. (The v1.4
local TUI does **not** reuse this pattern — it is 100% Rust with no Cargo feature and no web assets, roadmap §5.)

### Lands (features)
| Item | Capability | Trace |
|---|---|---|
| `[NEW]` Remote/replica feature GA (embedded replicas) | Promote the non-default libsql remote/embedded-replica feature to a supported, documented build; embedded-replica local-read + remote-write-at-primary | D1/D15/D28, NFR-10 |
| `[NEW]` Join-existing-workspace onboarding | A teammate clones the repo and connects to the existing shared store (e.g. an `init --remote` flow / committed-config detection) | D28, FR-14 |
| `[NEW]` Config split | Committed/shareable project config (remote URL, sync interval — FR-13 startup-only keys) **vs** per-user secrets (auth token ONLY via `UNBLOCK_*` env or OS keychain, NFR-18 — never `config.toml`) **vs** local non-committed state (`unblock.db`) | FR-13, NFR-18 |
| `[NEW]` Credential handling | libsql auth tokens via `UNBLOCK_*` env **or** OS keychain only — never `config.toml` (NFR-18 already mandates this; v1.3 implements the keychain path) | NFR-18 |
| `[NEW]` Self-hosted sqld path | Self-hosted sqld **documented AND tested** as the data-governance path (company data need not go to Turso Cloud); the remote contract suite (`wiremock`) covers it — same protocol | D28, NFR-16 |
| `[NEW]` Actor-attribution conventions | Distinguish humans from agents in `UNBLOCK_ACTOR` values; feeds FR-22 audit and FR-18 coordination status ("is this claim held by a person or a dead agent?") | FR-22, FR-18, D28 |
| `[NEW]` Documented no-ACL limitation | Whoever holds the token has full write within the team trust domain; fine-grained auth/ACL is **explicitly v2+** (roadmap §7) — do not promise it | NFR-18 |
| FR-13 sync layers | Config precedence extended for remote endpoints / sync intervals (startup-only keys) | FR-13 |
| `[NEW]` Sync-mode health | `doctor` + health taxonomy extended: replica lag, sync conflicts, WAL-on-remote integrity; "Drifted" gains a remote meaning | FR-16 (full) |
| `[NEW]` Multi-workspace discovery | Limited multi-workspace handling for the shared case (one operator, several synced workspaces) — **explicitly NOT** the dropped town/mayor routing (FR-24/D11); scoped to remote-sync addressing only | distinct from D11 |
| `[NEW]` Resilience GA | The remote-only jittered backoff (`backon`/`tokio-retry`, never archived `backoff 0.4`) + `failsafe` circuit-breaker validated under a remote contention lab; `wiremock` coverage promoted to a remote contract suite | NFR-3, NFR-16 |
| `[NEW]` Concurrency contract (D14 extension) | Per-replica single-writer; global serialization at the primary; the atomic claim (FR-2) resolves cross-machine at the primary; **no multi-master semantics** | D14, FR-2 |
| `[NEW]` Mixed-actor remote contention lab | Extend the NFR-3 lab: agent swarms + sporadic human writes against one primary | NFR-3, D28 |

### Crates touched
`unblock-storage` (embedded-replica impl + sync semantics — the heart of this release), `unblock-sync`
(reconciliation seams if any), `unblock-health` (sync diagnostics), `unblock-config` (remote endpoints,
config split + keychain credential resolution), `unblock-engine` (write topology at the primary),
`unblock-mcp` (sync-status resources), `unblock-cli` (join-existing-workspace onboarding flow).
`unblock-model`/`unblock-error` only if a sync-state type or error variant is needed.

### Risks / open questions for review *(updated 2026-07-07)*
- Keychain portability across Linux/macOS/Windows (NFR-11) — may need per-OS backends.
- TLS/HTTP transitive surface only enters builds that opt into remote (NFR-10 must stay green on default build).
- Lock-time confirmations: the embedded-replicas-vs-Turso-Sync default (fresh research at v1.3 lock) and the
  "dist artifacts enable `remote`" distribution call.
- The D14 **"single-MCP-server per workspace"** wording: the **local** (single-machine) half is **RESOLVED by D31**
  (2026-07-09) — child-per-client is the supported topology, cross-process serialization restored via the
  `.unblock/.write.lock` advisory lock — so the v1.3 topology review now covers only the **cross-machine
  primary-serialization** half; the **local two-writer / co-tenancy** case surfaced at v1.4 (a TUI and an agent
  run on one machine — roadmap §5) is likewise covered by D31, not deferred.
- *Answered 2026-07-07 (dropped):* the offline-first question — decided above (remote writes require network;
  reads stay local; no queue-and-reconcile in v1.3).
- *Deferred 2026-07-07 (dropped as a v1.3 question):* multi-writer reconciliation (LWW-vs-oplog) — moot under
  the primary-serialized write contract; deferred with the Turso Sync v2+ candidate (roadmap §7).

---

## 5. v1.4 — Human surface: local TUI  **[PROPOSED]**

**Theme / purpose (Miguel's framing):** an **offline, local, terminal-native** window for the team's
**developers** to **visualize the state of the project/workspace** — a rich-DX point of visibility *inside* the
dev workflow (no browser, no context-switch). The same "visualize the state" purpose as the retired web-local
proposal, now terminal-native. Reads are always local (the local DB, or the local replica in remote
workspaces); in remote workspaces writes follow the v1.3 stance (network required — roadmap §4). **Phase 1 is
read-only visualization.**

**Architecture (ratified 2026-07-08):** the TUI is an **MCP client over stdio** — it spawns `unblock mcp` as
a child process and speaks MCP over **stdio, exactly as Claude Code / agents do today**. There is **no second
domain surface** (FR-9 single mutation home; D2/D3 preserved, not relaxed — PRD §4 D28): only the concrete
client changes (web → terminal); D28's principle (humans work through an MCP client) is unchanged. A new
`tui` **lifecycle command** on the D3 surface (lifecycle/ops only, no domain CLI; the canonical D3 verb set —
and the doc-lint command-token class that pins it — gains `tui` at v1.4 lock). **NO loopback HTTP server, NO
embedded web assets, NO dependency on any streamable-HTTP transport** (an unscheduled v2+ item — roadmap §7) — stdio ships in v1.

**Security (NFR-18):** the untrusted-input surface is **~unchanged from v1** — a child process over stdio opens
**no new socket**, so there is **no session token, no Origin/Host validation, and no DNS-rebinding/CSRF
concern** (all of which the retired loopback-web design required). A stated advantage of the terminal-native
design over the web-local one.

**Dependency / sequencing (ratified 2026-07-08; resequenced 2026-07-20):** the TUI **transport** has **no hard
dependency beyond the v1 MCP contract** (stdio is shipped in v1), so it **MAY be pulled earlier
opportunistically**. Its phase-1 **screens**, however, couple to domain *data* that the resequence now lands
**before** it: **swarm observability** needs FR-18 + FR-22 (both **[v1.1]**) and the **milestone board /
roadmap / burnup** need the **v1.2** planning layer (roadmap §3) — both of which land ahead of the TUI, so its
screens arrive **full, not thin**. It takes the **v1.4** slot — landing after the planning layer (roadmap §3)
and the shared-state release (roadmap §4) that the resequence puts ahead of it. The critical path keeps
planning first, then shared-state, then the TUI, then scale (v1.2 → v1.3 → v1.4 → v1.5); it is set by the
2026-07-20 resequence (PRD §4 D41).

**Phasing (ratified 2026-07-08):**
- **Phase 1 — read-only:** ready queue, board by status/milestone, issue detail (trail / dependencies /
  labels), activity, and **swarm observability** (live claims by actor via FR-22 audit + FR-18 coordination),
  plus a text roadmap/burnup per milestone.
- **Phase 2 — writes:** claim / close / create / edit + milestone/goal management via the **same MCP tools**
  with the human as actor.
- **Live updates:** phase 1 may poll; the v1.5 subscription-style server→client notifications (roadmap §6)
  upgrade it **over stdio** (notifications are protocol-level — no HTTP needed).

**Screens — the KEY difference from the retired web proposal: graphs are EXCLUDED.** The
force/hierarchical/radial dependency graph does **not** translate to a terminal at mockup quality
(braille-canvas approximations only — no real hover/zoom/drag), so it is **deliberately out of scope for the
TUI** and belongs to the v2+ commercial PRO web (roadmap §7). Adopted for the TUI: **ready / board / detail /
activity + a text roadmap/burnup per milestone.**

**Stack (ratified 2026-07-08):** **ratatui** (the mature core — a modular workspace since 0.30) is the base;
**`ratatui-kit` is too young to carry the product's human face** (recorded the same way line-ui was "leading
candidate, not locked"), and **tui-realm** (an Elm/React-like, stateful model) is the alternative for a
component/state architecture. Final framework call at v1.4 lock. **100% Rust: NO npm, NO Node, NO `dist` Node
build stage, NO `rust-embed` of web assets, NO `ui` Cargo feature** — `cargo-deny` covers the whole tree and
the binary gains no npm supply-chain surface. (This **removes** the single biggest cost the retired web-local
proposal carried.)

**UX reference (ratified 2026-07-08):** the same 14 mockups at `temp/tentative-v2/docs/designs/*.png` stay the
UX reference — **reference-only** (the tentative-v2 tree has the same status as `temp/beads_rust-main`); the
terminal-noir aesthetic translates *even more* literally to a real terminal.
- **Adopted screens:** tasks / ready / board, issue detail (trail / dependencies / labels / claim), activity,
  roadmap (subject to the milestone-nesting fork recorded at roadmap §3).
- **NOT adopted for the TUI:** the graph screens (force/hierarchical/radial) — they move to the v2+ commercial
  PRO web (roadmap §7).
- **Reinterpreted:** findings = filtered issue views (labels / saved queries FR-21) — findings are ordinary
  issues, not a new concept.
- **Still DISCARDED (Miguel, 2026-07-07):** the pipeline screen (a tri-state impl/review/qa pipeline is not in
  unblock's model; FR-19 gates are the domain concept) and the memory screen (no product memory concept
  exists; if ever wanted it is a separate future product discussion — deliberately NOT scoped here).

### Crates touched
**`unblock-tui`** *(proposed L7 crate — a ratatui terminal app that is itself the MCP client: it spawns
`unblock mcp` as a child and speaks MCP over stdio, with no loopback server / no embedded web assets / no new
transport; minted at v1.4 lock)*, `unblock-cli` (the `tui` lifecycle command), `unblock-mcp` (consumed over
stdio — no new transport). **No Node build stage, no npm gate** (removed with the web proposal).

### Risks / open questions for v1.4 lock
- Framework final call: **ratatui** vs **tui-realm** (and `ratatui-kit` maturity) — not yet locked.
- Which screens make the phase-1 read-only cut.
- Phase-2 write-scope boundaries (which MCP tools the TUI exposes to the human actor first).
- The milestone-nesting fork (roadmap §3) shapes the text roadmap/burnup screen.
- Multi-process against one **local** `unblock.db`: because the TUI spawns its **own** `unblock mcp` child, a
  TUI process **and** an agent on the same machine are **two independent local writers** — the in-process write
  `Semaphore` (D14) serializes within one MCP server, **not across two**, so cross-process serialization is the
  **restored `.unblock/.write.lock` advisory lock (D31)**: this within-machine co-tenancy is now the **SUPPORTED**
  topology (child-per-client, D31), correct-by-construction, with `BEGIN IMMEDIATE` + native `busy_timeout`
  (NFR-3) as the WAL-level backstop and NFS/SMB the documented residual. This is **distinct** from the
  cross-machine v1.3 case (one local MCP server per machine; all writes serialize at the shared primary — roadmap §4,
  PRD §8.2). The v1.3 topology review the roadmap already promises (roadmap §4) now covers only the cross-machine
  primary-serialization half; the local two-writer case is **RESOLVED by D31**.

---

## 6. v1.5 — Scale, swarm coordination depth & MCP surface richness  **[PROPOSED]**

*(Resequenced 2026-07-20 to v1.5: the former scale section, carried over in full **minus** the streamable-HTTP
transport — that transport is REMOVED from this version and relocated to the v2+ unscheduled bucket, roadmap §7.)*

**Theme:** Push the wedge from "correct at 250k" to "fast and rich at 1M issues / 10k agents", and deepen the
swarm-orchestration story beyond v1.1's read-only diagnostics.

**Why now:** v1 validates 1M only as a *manual* corpus (NFR-2); v1.1 ships coordination as read-only
*diagnostics*. v1.5 hardens both into supported, performant, actively-helpful capabilities once shared state (v1.3)
is real. Synergy: scheduler v2 consumes milestone due dates and critical path from the **v1.2 planning layer**
(roadmap §3) — the planning layer is settled first precisely so these ranking signals already exist.

**Goal:** 1M-issue performance is a CI hard gate, not a manual exercise; the scheduler/coordination contracts
gain active assistance (not just observation); the MCP surface gets richer — without bloating the tool list.

### Lands (features)
| Item | Capability | Trace |
|---|---|---|
| `[NEW]` 1M-issue perf as CI gate | Promote NFR-2's manual 1M / 10k-agent corpus to an automated regression gate; index/query tuning (the original's `workitems_ready_index` lesson) | NFR-1/NFR-2 |
| `[NEW]` Active coordination | Beyond `coordination status`: stale-claim **reclaim** policy, claim TTLs/heartbeats, deterministic re-assignment evidence — still DB-derived, still no Agent Mail | FR-18 extension |
| `[NEW]` Scheduler v2 | Richer ranking signals (cost/estimate-aware, critical-path-aware via `petgraph`, milestone-due-date-aware via the v1.2 planning layer), still a pure versioned `unblock.scheduler.v2` contract | FR-18 / policy |
| `[NEW]` Richer MCP surface | Streaming/large-result resources, batch tools, subscription-style change notifications — measured against the tool-count budget (RK-3); resources preferred over new tools | FR-20 / PRD §9 |
| `[NEW]` Compaction / archival | Activate the model's compaction fields (kept for JSONL fidelity, D12) as a real archival path for very large stores; restore-from-snapshot | D12, domain model |
| `[NEW]` Performance observability | `tracing`-based perf spans + a `criterion` dashboard; contention-lab generalized to a continuous load harness | NFR-13 |

### Crates touched
`unblock-storage` (index/query tuning, archival), `unblock-policy` (scheduler v2, reclaim contracts),
`unblock-engine` (claim TTL/heartbeat, compaction orchestration), `unblock-mcp` (richer surface, batch/
streaming), `unblock-health` (scale diagnostics), `unblock-render`
(large-result formatting), plus incidental touches to `unblock-model` (compaction-field activation, D12),
`unblock-error` (archival error variants) and `unblock-sync` (snapshot / archival sync). Bench-only
touches to `unblock-fuzz`/harnesses.

### Risks / open questions for review
- Claim TTL/heartbeat changes the contract — must be additive (`contract_version` bump, not breaking).
- 1M as a *required* CI gate may be slow/expensive — may need a sampled or scheduled (nightly) gate.
- Compaction interacts with JSONL round-trip fidelity (D12) — round-trip property tests must extend to compacted issues.

---

## 7. v2-plus / later horizon  **[PROPOSED — direction only]**

**Theme:** Capabilities that are credible long-term but deliberately *not* committed — they either reverse a
locked decision, need a concrete external demand, or imply a materially larger product surface.

| Candidate | Notes / why later |
|---|---|
| Cross-project / multi-repo routing (the original's town/mayor) | **Explicitly dropped in v1 (FR-24/D11)**; reintroduce *only* on a concrete multi-repo demand, and likely in a shape informed by v1.3 multi-workspace sync rather than the original's elaborate mayor design |
| **Turso Sync / Turso Database backend migration** | Storage backend evolution behind the `Storage` trait + the NFR-16 contract suite. The vendor's recommended path for NEW sync projects (as of 2026-07), but its engine is beta — revisit when the engine leaves beta. Also the **only path to offline-write reconciliation** (the v1.3 stance — roadmap §4 — defers queue-and-reconcile to this candidate) |
| **Fine-grained auth/ACL for shared stores** | The future answer to the v1.3 documented no-ACL limitation (roadmap §4: token = full write within the team trust domain); needs concrete team-scale demand and likely server-side enforcement |
| **Commercial web dashboard (PRO)** | A **separate commercial product** (NOT the OSS binary): a **static SPA** + a **client-minted read-only Turso token** (`data_read` fine-grained permission — minted by the **team's own Turso account / control plane** (the same trust domain that already holds write in v1.3 — self-hosted `sqld` deployments mint the equivalent `sqld` JWT/Hrana read grant) as a short-lived read grant, *not* by any credential-custodying PRO backend; `data_read` is a Turso-*platform* token scope, not unblock's deferred v2+ ACL, whose future per-user scoping is the fine-grained-ACL row above — held in the browser via `@libsql/client/web` over Hrana/HTTP, so **we never custody credentials, never proxy data**) + **`unblock-model` + `unblock-policy` compiled to WASM** (viable precisely because NFR-15 keeps L0/L1 pure — no tokio/I/O/petgraph-in-policy), so the domain logic that actually lives in those crates — the ready **sort** (the hybrid re-rank comparator, `unblock-policy`) and the scheduler **explanations** — comes from the **same crates**, not reimplemented. The **storage-layer** derivations do NOT: `blocked` (a live 3-pass SQL computation incl. a fixpoint blocked-parent propagation), the ready **filter** (`id NOT IN <blocked set>`) and the epic **rollups** are `unblock-storage` (L2) SQL, not model/policy (PRD §8.1 / `01-design-spine.md` §3.2), so the browser **re-issues that storage SQL directly against the read-only Turso replica** (a future `unblock-storage`-compiled-to-WASM path could later move it in-process). (**FR-9 preserved — the PRO offers no *mutation* surface at all; it is structurally read-only.**) Renders the "wow" that the terminal cannot: the dependency graph, burnup, live swarm observability (the graph / burnup mockups under `temp/tentative-v2/docs/designs/`, TUI-excluded per roadmap §5, inform this). **Zero hosted engine, zero credential custody; no drift on the WASM'd domain logic (sort + explanations) — the storage SQL is the one thing the browser re-executes rather than forks.** This is where **Astro + line-ui migrate** (freed from the binary + D13/NFR-6 — SSR / Astro-Actions become legal again in a separate repo/product; the npm/Node ecosystem lives there, not the OSS tree). **Structurally read-only** — browser writes via raw Hrana would bypass the engine (content_hash / events / claims), so writes are NOT offered; a real-engine-in-browser (a WASM `Storage` backend running the actual engine over Hrana) is a **v2+-dreaming** note only — build nothing on it now. **Gates:** v1.3 shipped (no client Turso remote ⇒ nothing to point at) **AND** a go-to-market decision (external customers = a pivot from D28's "internal company teams" framing — needs its own future D-id and a PRO PRD-lite when/if it locks). **Moat honesty:** the MCP contract is public / self-describing, so a free viewer is buildable by anyone — the moat is the WASM domain core + schema ownership (pre-1.0) + pace + the agent-swarm-native angle, not the UI itself. **Direction only — no spec, no promise, no timeline** (like the rest of this table) |
| DB-only mode (drop JSONL entirely) | D5 keeps JSONL as optional and notes the design is "reversible toward DB-only later" — a candidate once sync (v1.3) makes JSONL redundant for the shared case |
| Hosted / managed shared service | PRD §11 keeps this out of scope (collaboration is via libsql sync, not a bespoke server). Only revisit if v1.3 sync proves insufficient for real teams |
| Pluggable alternative storage backends | The `Storage` trait + contract suite (NFR-16) make this *possible*; a second backend would only ship on concrete demand (the trait exists precisely so this is cheap when needed — the Turso Sync row above is its first concrete instance) |

**Moved out of the committed versions (2026-07-07 resequence, updated by the 2026-07-20 resequence — PRD §4 D41):**
- **Human-facing surface** → scheduled as **v1.4** (roadmap §5; pulled forward from v1.5 by the 2026-07-20
  resequence). The local **TUI** is an MCP *client*, so D3 is preserved rather than relaxed — the audience shift
  is PRD §4 D28. (The web dashboard's "wow" — graphs, burnup, swarm — returns above as the separate v2+
  commercial PRO product.)
- **MCP streamable-HTTP transport** → **unscheduled (v2+), no committed slot** (removed from the scale version by
  the 2026-07-20 resequence). A D2 extension — **stdio stays primary** — whose "UI enabler" justification is
  **gone**: the v1.4 local TUI speaks MCP over stdio (roadmap §5) and the v2+ PRO web reads Turso directly
  (above), so neither needs it. Its only remaining rationale is thin — an MCP server exposed to clients that
  **cannot spawn a local stdio child** (a shared team MCP-server endpoint); under the v1.3 embedded-replica model
  each machine already runs its own local MCP server over stdio, so there is no committed demand. **Any other
  transports likewise stay unscheduled** and would follow the same isolation discipline.

These are intentionally unscheduled. Each requires a product decision (and several reverse a locked PRD §4
decision) before it can leave this list.

---

## 8. Feature-to-version matrix

Legend: ● lands · ◐ extended/hardened · ✗ = dropped · blank = not landing · `[NEW]` not yet in PRD FR set · L=LOCKED, P=PROPOSED

| Feature / FR | v1 (L) | v1.1 (L) | v1.2 (P) | v1.3 (P) | v1.4 (P) | v1.5 (P) | v2+ (P) |
|---|:--:|:--:|:--:|:--:|:--:|:--:|:--:|
| Issue CRUD + tombstone delete (FR-1) | ● | | ◐ `milestone_id` | | | | |
| Atomic claim (FR-2) | ● | | | ◐ cross-machine at primary | | ◐ TTL/heartbeat | |
| Defer/undefer (FR-3) | ● | | | | | | |
| Query: list/ready/blocked/search/count/stale (FR-4) | ● | | ◐ milestone filter | | | | |
| Typed deps + graph (FR-5) | ● | | | | | | |
| Comments — full CRUD add/list/update/delete (FR-6, D37) | ● | | | | | | |
| Labels (rename/list-all) / epic rollups (FR-6) | | ● | | | | | |
| JSONL export/import (FR-7/8) | ● | | ◐ milestones/goals layout (lock design point) | | | | ◐ DB-only option |
| `bd` one-shot import (FR-26) | ● | | | | | | |
| Shared engine + write Semaphore + read fast path (FR-9/10) | ● | | | ◐ primary-serialized topology | | ◐ TTL | |
| Agent contract + exit codes + capabilities/schema (FR-11/12) | ● | | ◐ planning tool (additive bump) | | | ◐ richer | |
| Layered config (FR-13) | ● subset | ● full | | ◐ remote keys + config split | | | |
| Workspace bootstrap (FR-14) | ● | | | ◐ multi-ws + join-remote onboarding | | | |
| Pure-DB diagnostics (FR-15) | ● | | ◐ milestone filters/counters | | | | |
| Workspace health (FR-16) | ● lite | ● full | | ◐ sync health | | ◐ scale | |
| Cooperative shutdown (FR-17) | ● | | | | | | |
| Swarm coordination / scheduler (FR-18) | | ● diagnostics | | ◐ actor attribution | | ◐ active + v2 | |
| Workflow gates (FR-19) | | ● | ◐ milestone-close gate (candidate) | | | | |
| MCP stdio server (FR-20) | ● | ◐ surface | ◐ planning tool | ◐ sync resources | | ◐ batch/stream | ◐ other transports (unscheduled) |
| Saved queries (FR-21) | | ● | | | | | |
| Audit / flight recorder (FR-22) | | ● | | ◐ actor conventions | | | |
| Shell completions (FR-23) | | ● | | | | | |
| Cross-project routing (FR-24) | ✗ dropped | | | | | | ◐ reconsider |
| Self-update (FR-25) | ● | | | | | | |
| TOON output | | ● | | | | | |
| **libsql remote/replica sync (embedded replicas)** `[NEW]` | (off, D15) | | | ● GA | | | ◐ Turso Sync candidate |
| **Credential / keychain handling** `[NEW]` | | | | ● | | | |
| **Join-existing-workspace onboarding** `[NEW]` | | | | ● | | | |
| **Self-hosted sqld (documented + tested)** `[NEW]` | | | | ● | | | |
| **Multi-workspace (sync-scoped)** `[NEW]` | | | | ● | | | |
| **Milestones (first-class) + milestone-scoped queries** `[NEW]` | | | ● | | | | |
| **Goals (first-class, slim)** `[NEW]` | | | ● | | | | |
| **MCP streamable-HTTP transport** `[NEW]` | | | | | | | ◐ unscheduled |
| **1M-issue perf as CI gate** `[NEW]` | (manual) | | | | | ● | |
| **Compaction / archival activation** `[NEW]` | (fields only) | | | | | ● | |
| **Local TUI — MCP client (stdio)** `[NEW]` | | | | | ● P1 read-only / P2 writes | | |
| **Pluggable backends / hosted service** `[NEW]` | | | | | | | ◐ |
| **Fine-grained auth/ACL (shared stores)** `[NEW]` | | | | | | | ◐ |

---

## 9. Crate-impact summary across releases

Legend (distinct from the roadmap §8 feature-matrix legend — here the glyphs track **crate work per release**,
not feature-landing): ● substantial work in that release · ◐ incidental / hardening touch · blank = untouched.
(So a crate can be ● here in a release where the roadmap §8 feature row is ◐ or blank — e.g. `unblock-health` is ● at v1.3 (substantial sync-diagnostics work) while its FR-16 feature-matrix row is only ◐ sync health.)

| Crate | v1 | v1.1 | v1.2 | v1.3 | v1.4 | v1.5 | v2+ |
|---|:--:|:--:|:--:|:--:|:--:|:--:|:--:|
| `unblock-model` | ● | ● | ● | ◐ | | ◐ | |
| `unblock-error` | ● | ● | | ◐ | | ◐ | |
| `unblock-policy` | ● | ● | ◐ | | | ● | |
| `unblock-storage` | ● | ◐ | ● | ● | | ● | ◐ |
| `unblock-sync` | ● | ◐ | | ◐ | | ◐ | ◐ |
| `unblock-health` | ● lite | ● full | | ● | | ● | |
| `unblock-config` | ● subset | ● full | | ● | | | |
| `unblock-engine` | ● | ● | ● | ● | | ● | |
| `unblock-render` | ● | ● | ◐ | | | ◐ | |
| `unblock-mcp` | ● | ● | ● | ◐ | | ● | ◐ |
| `unblock-cli` | ● | ● | | ◐ | ◐ | | |
| `unblock-fuzz` *(ingestion + bench harness)* | ● | | | | | ◐ | |
| `unblock-tui` *(proposed — minted at v1.4 lock)* | | | | | ● | | |

Notes:
- **`unblock-tui`** is the proposed 13th workspace crate (L7 — a ratatui terminal app that is itself the **MCP
  client**: it spawns `unblock mcp` as a child and speaks MCP over **stdio** (exactly as Claude Code / agents
  do today), roadmap §5; **no loopback server, no embedded web assets, no new transport**). At **runtime** it
  links nothing of the server — it drives a child process over stdio — so its only compile-time edge to
  `unblock-mcp` is for the **contract DTO types** it deserializes from the wire (the D25 per-tool output
  shapes); `unblock-mcp` itself takes **no new v1.4 work** (hence it is blank at v1.4 in the table above — the
  web-era ◐ was the retired HTTP-serving touch). It is minted only at v1.4 lock, when PRD §8.1 grows; until
  then the 12-crate set is unchanged.
- **100% Rust, no Node:** the TUI adds **no npm/Node build stage** to `dist` and **no `ui` Cargo feature** —
  `cargo-deny` covers the whole tree and the binary gains no npm supply-chain surface. (The web dashboard's
  npm/Node ecosystem lives in the separate v2+ commercial PRO product — roadmap §7 — not the OSS tree.)

---

## 10. Sequencing rationale (one paragraph)

v1 proves the wedge is *correct* (atomic claim + no hot-spin at 250k); v1.1 makes it *ergonomic* for swarms and
humans-via-clients (coordination diagnostics, gates, organization); **v1.2 gives the store a *planning layer***
(goals = why, milestones = when) — settled on **one cheap local file BEFORE replication**, so the additive
planning schema (`Issue.milestone_id`, Milestone/Goal) is proven against a single primary rather than migrated
across version-skewed replicas later (schema-before-distribution, PRD §4 D41 — and it answers the felt dogfood
demand, the team hand-building epics/milestones today); **v1.3 makes it *shared*** for mixed human+agent teams
(PRD §4 D28 — libsql embedded replicas: reads local, all writes serialized at the primary, no multi-master; a
mandatory Turso-Sync-vs-embedded-replicas maturity check is folded into its lock); **v1.4 opens the *human
window*** (an offline, local, terminal-native, read-first **TUI** that is itself an MCP client over **stdio** —
no new transport needed, FR-9's single surface preserved, its screens now fed by the v1.2 planning + v1.1 swarm
data that land first); **v1.5 makes it *fast and actively helpful at the top of the scale curve*** (1M as a CI
gate, active coordination, richer MCP) — its scheduler v2 consumes v1.2's milestone signals, so planning still
lands well ahead of that consumer. The streamable-HTTP transport is **unscheduled (v2+)** — its "UI enabler"
rationale is gone (the TUI speaks stdio; the PRO web reads Turso directly). Everything else in v2-plus either
reverses a locked decision or awaits concrete external demand and is therefore deliberately unscheduled. The
acyclic layering and the `Storage` trait/contract suite are the two invariants that make this sequence cheap:
remote storage (v1.3), backend evolution (the v2+ Turso Sync candidate) and alternative backends slot in
behind the trait without touching callers, and the TUI rides the existing MCP contract instead of minting a
second domain surface.
