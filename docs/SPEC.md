# SPEC: ://unblock — Stage 1 Product Architecture

**Status:** APPROVED *(round-6 cascade-symmetry sync applied 2026-05-12; previously APPROVED 2026-05-07 with round-2 iterations applied 2026-05-07, round-3 research applied 2026-05-08, and §9.4.10 local-secrets format corrected 2026-05-08)*
**Author:** Ada (architect)
**Date:** 2026-05-07 (round-3 research applied 2026-05-08; §9.4.10 format drift fix applied 2026-05-08; round-6 cascade-symmetry sync applied 2026-05-12)
**Changelog:**
- 2026-05-08 — §9.4.10 corrected the local-secrets file path/format from `.encore/local-secrets.toml` (TOML) to `apps/api/.secrets.local.cue` (CUE) per Encore official docs; added parallel mapping note (`MEMORY_DEK` → Go field `MemoryDEK`) referencing P01 spec §3.5 for the full mapping table.
- 2026-05-12 — §9.4.4 `deps.cascade_events.kind` CHECK enum extended to four first-class values (`'close'`, `'edge_added'`, `'edge_removed'`, `'state_change'`); the round-2 framing of two kinds with a "future" `state_change` was retired. The four kinds are P01-active and partition the cascade write path into two propagation regimes (Regime A: writer-inline `is_ready` writes by Tool 12; Regime B: subscriber-only `pipeline_stage` writes plus subscriber-driven `is_ready` writes for `close` / `edge_added` / `state_change`). Canonical model lives in phase spec [§6.3.0 "Propagation regimes"](./specs/01-spec-backend-mvp.md); this document records *what exists* at architectural level, the phase spec records *how it is implemented*. Status flipped to DRAFT pending re-approval.
- 2026-06-04 — §5.2.2 + §1 + §6.2 contracts table + §3.x repo-tree + §11 traceability: the v1.0 MCP tool inventory is reconciled from **18 → 27** in lockstep with the **P01 round-16 amendment** (`docs/specs/01-spec-backend-mvp.md`, beads `unblock-tv8.71` / `.74` / `.75`). P01 grows from 14 → 23 (adds `promote`, four milestone tools, four label tools); v1.0 grows to 27 (23 P01 + 4 memory at P02). Provenance: this edit exists solely to keep the Stage-1 architecture inventory identical to the P01 phase spec — no architectural decision changed, only the tool count and names. The P01-round-16 `0120_mcp_issued_to_user_notnull` migration is noted in the §11 P01 row.
- 2026-06-11 — §9.4.3 + §11 P01 row: P01-round-16 lockstep drift closure (surfaced by `/investigate` on bead `unblock-tv8.75`, the label-registry MCP tools). The `workitems.labels` DDL in §9.4.3 gained an `updated_at timestamptz NOT NULL DEFAULT now()` column to match the phase-spec `Label.UpdatedAt` struct field (which always declared it) — the original `0040_workitems.up.sql` omitted it, a genuine contradiction. Resolution DECIDED by Miguel: ADD the column (the registry is mutable via MCP Tool 22 `update_label`, and `items` / `milestones` / `comments` all carry `updated_at`), via the new up-only migration `0130_workitems_labels_updated_at`, noted in the §11 P01 row alongside `0120`. Provenance-only architectural sync — no architectural decision beyond the column addition. Status remains APPROVED.
- 2026-06-11 — §9.4.6 + §11 P01 row + 2026-06-04 changelog entry: P01-round-16 lockstep drift closure (surfaced by `/investigate` on bead `unblock-tv8.73`). The `mcp.api_keys.issued_to_user` DDL in §9.4.6 was still nullable with `ON DELETE SET NULL` and described as an org-level service key; it is brought into lockstep with the P01 round-16 contract — NOT NULL, FK `ON DELETE CASCADE`, with the audit-survival note (`mcp.tool_calls.api_key_id` is `ON DELETE SET NULL`, so tool-call history survives user deletion). The migration name is also corrected `0110` → `0120` here and in the §11 P01 row (slot `0110` is held by the committed `0110_mcp_warning_codes` migration, bead `unblock-tv8.63`). Provenance-only architectural sync — no architectural decision changed. Status remains APPROVED.
**Source PRD:** [docs/PRD.md](./PRD.md) (APPROVED, 2026-05-07)
**Companion:** [docs/MANIFESTO.md](./MANIFESTO.md) (APPROVED, 2026-05-07)
**Carries forward verbatim:** [docs/code-cli/plan.md](./code-cli/plan.md), [docs/code-cli/spec.md](./code-cli/spec.md), [docs/code-cli/research.md](./code-cli/research.md)

> Stage 1 deliverable. This document defines the high-level system architecture
> — the deliverables, their boundaries, the technology choices, the public
> surface, the cross-cutting invariants, **and the canonical Postgres DDL for
> all eight schemas**. It does not contain phase-level implementation specs
> (exact MCP tool signatures, exact Pub/Sub topic shapes, exact OAuth scope
> sets). Those land in per-phase `docs/specs/0X-spec-*.md` documents authored
> under `/spec` after this architecture is APPROVED.
>
> **DDL exception (added at user request 2026-05-07):** §9.4 contains the full
> canonical DDL for all eight schemas. Per-phase specs may **add** indexes or
> columns later but **must not deviate** from the column types, constraints,
> or relationships defined here.

---

## 1. Overview

`://unblock` is a provider-agnostic work-tracking engine for AI agents. The
Manifesto's eight Principles and eight Laws are non-negotiable constraints on
this architecture; every design decision below is traceable to one or more.

The product is composed of **four independently shippable deliverables** —
three "orthogonal" per Manifesto Principle 4, plus the plugin renderer that
realises Stage 3 of Law 8:

| # | Deliverable | Tech | Phase | Ships at |
|---|---|---|---|---|
| 1 | **Backend (API + remote MCP)** | Go (Encore) on a single Postgres + Pub/Sub | P01 + P02 | v1.0 |
| 2 | **AST CLI (`unblock-code`)** | Rust (edition 2024), tree-sitter, SQLite + FTS5 | P03 | v1.0 |
| 3 | **Plugin renderer (`unblock-plugin`)** | Rust (edition 2024) | P04 | v1.0 |
| 4 | **Web client** | Astro 5 + line-ui on Cloudflare Pages | P05 | v1.1 (line-ui-blocked) |

These four deliverables are **structurally decoupled** (Manifesto Law 6). They
share no runtime state. The backend is the single source of truth for the
graph and project memory. The AST CLI is local-only. The plugin renderer is a
build-time tool. The web client is a BFF over the backend. Coupling exists
only at well-defined contracts: the MCP wire protocol, the GitHub webhook
payload schema, and the Astro-origin OAuth callback that bridges browser →
backend without ever exposing backend credentials to the browser.

### 1.1 What is in the architecture

- The deliverable inventory and what each one is for.
- The technology stack per deliverable (and the rejection rationale for the
  alternatives considered).
- The repository / monorepo layout that holds the deliverables.
- The Postgres schema topology (eight schemas, what each owns) **plus the
  canonical DDL for every table** (§9.4).
- The public network surface (the two public endpoints at v1.0, three at
  v1.1, the BFF boundary, the MCP transport).
- The cross-cutting invariants (RBAC, BFF discipline, cascade, three-layer
  pipeline enforcement, atomic claim, AST CLI decoupling).
- The phase-to-component traceability matrix.

### 1.2 What is *not* in the architecture (deferred to `/spec` per phase)

- Exact MCP tool names and JSON schemas (only the inventory count and shape
  is locked here per FR-8).
- Exact Pub/Sub topic names and event payloads.
- Exact OAuth scope sets per provider.
- Per-tool latency budgets beyond the PRD-level NFRs.
- Exact line-ui component selection per Astro view.
- Exact `tags.scm` query rule inventory (locked in `docs/code-cli/spec.md`).
- Per-phase migration scripts (the canonical schema is in §9.4; phase migrations
  realise it in order).

---

## 2. Architectural Drivers

These are the constraints that shape every choice that follows. Each is sourced
from the Manifesto, the PRD, or the locked code-cli artefacts.

### 2.1 From the Manifesto Laws (non-negotiable invariants)

| Law | Architectural impact |
|---|---|
| L1 — Cascade is structural | Every mutation that closes a work item must emit a Pub/Sub event whose subscriber recomputes the ready set in the same logical operation. The cascade subsystem is part of the canonical write path, not an optional integration. |
| L2 — One graph, one truth | Provider state, client cache, and agent claims reconcile to the Postgres graph. No write path may produce a state where Postgres disagrees with itself. |
| L3 — Postgres is the source of truth | Provider integrations are *event sources* only. The product must operate when GitHub is offline. Reconciliation is scheduled, not synchronous. |
| L4 — BFF is structural | The browser never holds backend credentials. Astro Actions are the sole privileged client. The Encore API is unreachable from the browser except for the documented public endpoints (FR-12). The OAuth callback is on the Astro origin (Astro Action), never on Encore. |
| L5 — Claim semantics are atomic | Claim is a single Postgres transaction with `SELECT FOR UPDATE`. No queue, no advisory lock, no compare-and-swap on a column. |
| L6 — Decoupled deliverables share no runtime state | `unblock-code` does not call the backend. The backend does not call `unblock-code`. The plugin renderer is build-time. The Astro client uses Encore RPC and nothing else. |
| L7 — One command from productive work | `prime → ready → claim` p99 < 2 s warm cache. The hot path must avoid N+1 queries, must not block on provider APIs, and must not require the agent to traverse the graph itself. |
| L8 — Pipeline gates are enforced architecturally | Three independent layers (MCP state-transition validation, post-dispatch state validator, agent prompt structure with BLOCK conditions) must all be bypassed simultaneously for the pipeline to be violated. |

### 2.2 From the PRD (locked product decisions)

- Single Postgres database with **8 schemas**: `auth`, `org`, `workitems`,
  `deps`, `providers`, `mcp`, `boards`, `memory` (FR-1).
- **27 MCP tools** at v1.0 over **Streamable HTTP** (per MCP spec 2025-06-18)
  with `Bearer <api-key>` auth (FR-8; raised from 18 by the P01 round-16
  amendment). 23 ship in P01 (incl. `promote`, four milestone tools, four
  label tools), +4 memory tools in P02, plus the providers/sync tooling
  required for FR-11.
- **Two public Encore endpoints at v1.0** (`POST /webhooks/github`,
  `POST /mcp` + `GET /mcp` — the latter pair is **one logical endpoint at
  one path** per the Streamable HTTP convention) and **+1 at v1.1**
  (`POST /webhooks/gitlab`). The OAuth
  callback is an **Astro Action on the Astro origin**, not an Encore endpoint
  (FR-12, NFR-4).
- **Three orthogonal state dimensions** plus `pipeline_state` exception column
  (PRD §6.2). No derived label columns, no label-based reconciliation.
- **Recursive milestone hierarchy** absorbs the v1 iteration concept (PRD
  §6.3). Max depth 4. `workitems.iterations` is dropped.
- **Findings are first-class child work items**, not label suffixes (PRD §6.6).
- **Comment trail is `(kind, status)` orthogonal**, append-only, NOT NULL on
  status with default `info` (FR-10, PRD §6.5).
- **AST CLI carries forward verbatim** from
  `docs/code-cli/{plan,spec,research}.md`. This architecture references that
  scope, does not re-litigate it.
- **Headless v1.0** — backend + AST CLI + plugin ship at v1.0. Astro web
  ships at v1.1, gated on `line-ui` v1.

### 2.3 From the v1 Rust workspace (carry-forward)

The current repository hosted a Rust v1 workspace (`crates/unblock-core`,
`unblock-github`, `unblock-mcp`, `unblock-resilience`). For the v1.0 product,
that workspace is **archived under `temp/rust-v1/`** (gitignored, local-only
archaeology) — the backend is Go (Encore) per FR-1 and FR-8. The Rust crates
that survive into v1.0 are the new code-cli crates (`unblock-indexer-core`,
`unblock-indexer`, `unblock-code`) plus the plugin renderer (`unblock-plugin`).
**There is no Rust binary called `unblock-mcp` in the new architecture.** The
MCP server is an Encore Go service in `apps/api/mcp/`.

This is a **deliberate replatform**, not a regression. The rationale is in
PRD §14.1: Encore's Pub/Sub + private/public RPC distinction + bundled
Postgres + auth provider scaffolding cuts P01 calendar time without giving up
Manifesto Laws. The Rust v1 GitHub adapter, graph, and MCP server are not
carried forward verbatim; their ideas (typed errors, `petgraph`-style cycle
detection, atomic claim transaction) inform the Go re-implementation.

---

## 3. System Topology

### 3.1 Component diagram (logical)

```
                     ┌─────────────────────────────────────────────┐
                     │                  AI Agent                   │
                     │   (Claude Code / Copilot / Cursor / …)      │
                     └────────────────┬────────────────────────────┘
                                      │ MCP over Streamable HTTP
                                      │ Bearer <api-key>
                                      │ POST /mcp + GET /mcp
                                      │
   ┌────────────────────────┐    ┌────▼─────────────────────┐    ┌────────────────────────┐
   │       Browser          │    │    Encore Go Backend     │◄───┤   GitHub (provider)    │
   │   (Astro web, P05)     │    │  ┌────────────────────┐  │    │   webhooks + REST/GQL  │
   │                        │    │  │   Public surface   │  │    └────────────────────────┘
   │  Astro Actions BFF     │    │  │  POST /mcp         │  │
   │  HttpOnly cookie       │    │  │  GET  /mcp         │  │
   │  on Astro origin       │    │  │  /webhooks/github  │  │
   │  /auth/[provider]/     │    │  │  (gitlab @ v1.1)   │  │
   │                        │    │  └─────────┬──────────┘  │
   │   callback             │    │            │             │
   │  (Astro Action,        │    │  ┌─────────▼──────────┐  │
   │   not Encore)          │◄───┼─►│ Private services   │  │
   └─────────┬──────────────┘    │  │ auth   org         │  │
             │ Encore RPC        │  │ workitems  deps    │  │
             │ (private API,     │  │ providers  mcp     │  │
             │  Astro origin     │  │ boards  memory     │  │
             │  only)            │  └─────────┬──────────┘  │
             │ Authorization:    │            │             │
             │   Bearer          │            │             │
             │   <session_id>    │            │             │
             │ X-Unblock-BFF-    │            │             │
             │   Origin: astro   │            │             │
             └───────────────────┤            │             │
                                 │  ┌─────────▼──────────┐  │
                                 │  │ Postgres + Pub/Sub │  │
                                 │  │ 8 schemas (§9.4)   │  │
                                 │  └────────────────────┘  │
                                 └──────────────────────────┘
                                              ▲
                                              │ build-time embed
                                              │ via include_str!
                                              │ (catalogue.json)
   ┌────────────────────────────────┐    ┌────┴───────────────────────┐
   │      AST CLI (unblock-code)    │    │  Plugin renderer (unblock- │
   │      Rust, local-only          │    │  plugin), Rust, build-time │
   │      tree-sitter + SQLite/FTS5 │    │  Renders Claude Code +     │
   │      ~/.cache/unblock/...      │    │  Copilot prompts/hooks     │
   │                                │    │  Reads mcp.meta_catalogue  │
   │                                │    │  at build (& CI verifies)  │
   └────────────────────────────────┘    └────────────────────────────┘
   (no network at runtime,                (no runtime, no network —
    no shared state with backend)          generates static config)
```

The dashed flow `Browser ─ /auth/[provider]/callback (Astro Action) ─ Encore
private RPC` is the post-OAuth handshake. The browser's redirect target is
the **Astro origin**, not Encore. The Astro Action receives the OAuth code,
calls the private `auth.exchangeOAuthCode` RPC inside the Encore mesh, and
sets the HttpOnly cookie on the Astro origin in the same response. Encore
never sees a public callback URL.

### 3.2 Boundary contracts

The deliverables interact only across these explicit contracts. No other
runtime coupling is permitted (Manifesto Law 6).

| Contract | Producer | Consumer | Surface |
|---|---|---|---|
| **MCP wire protocol** (`modelcontextprotocol/go-sdk`, JSON-RPC over Streamable HTTP) | Encore backend | AI agents | Bearer auth, 27 tools at v1.0 (P01 round-16; was 18); transport per MCP 2025-06-18 spec — see §5.3 |
| **Astro Actions ↔ Encore RPC** (private API) | Encore backend (private services) | Astro Actions BFF | Encore-generated TypeScript clients; not reachable from the browser; auth via forwarded session id (`Authorization: Bearer <session_id>` + `X-Unblock-BFF-Origin: astro`) per §5.3.1 |
| **GitHub webhook payload** | GitHub | Encore backend `providers` service | `POST /webhooks/github`, signature-verified |
| **OAuth callback (Astro origin)** | GitHub / GitLab | Astro Action `auth/[provider]/callback` → private RPC `auth.exchangeOAuthCode` | Browser redirect target on `unblock.websublime.com`; PKCE-validated; HttpOnly cookie set on Astro origin |
| **Plugin catalogue export** | Encore backend `mcp.meta_catalogue` MCP tool **and** checked-in `crates/unblock-plugin/data/catalogue.json` | `unblock-plugin` (build-time `include_str!`); CI drift test (runtime) | Compile-time embed into the Rust plugin renderer; CI compares the embedded JSON to the live MCP `meta.catalogue` response and fails on mismatch |
| **MCP `verify_can_transition`** | Encore backend | Plugin `verify-state` hook in dispatched session | Same MCP transport, called by Claude Code Stop / Copilot agentStop hook |
| **Plugin BLOCK conditions** | `unblock-plugin` (build-time) | Claude Code / Copilot agent prompt | Static markdown / TOML emitted onto the host's plugin directory; consumed at agent start, not at runtime by the backend |

The AST CLI participates in **no** runtime contract with the backend. Its
only outputs are stdout JSON envelopes (consumed by humans/agents via Bash)
and the local `~/.cache/unblock/repos/<hash>/index.db`.

---

## 4. Repository Layout

The v1.0 repository is a polyglot monorepo. The top-level layout reflects the
"three orthogonal deliverables" of Manifesto Principle 4 plus shared docs and
infrastructure:

```
unblock/
├── apps/
│   ├── api/                        # Go (Encore) backend — P01 + P02
│   │   ├── encore.app
│   │   ├── go.mod
│   │   ├── auth/                   # auth service (OAuth2+PKCE, API keys, sessions)
│   │   ├── org/                    # org / project / RBAC service
│   │   ├── workitems/              # work items, comments, labels, milestones
│   │   ├── deps/                   # dependency graph engine, ready/cycle/cascade
│   │   ├── providers/              # GitHub webhook ingestion + bidirectional sync
│   │   ├── mcp/                    # MCP server (Streamable HTTP transport, 27 tools at v1.0 — 23 in P01 per round-16)
│   │   │   └── catalogue.json      # canonical state-machine + tool catalogue export
│   │   ├── boards/                 # kanban / saved-view persistence
│   │   ├── memory/                 # scoped memory service (4 MCP tools)
│   │   ├── public/                 # the public endpoints (FR-12, 2 at v1.0)
│   │   ├── migrations/             # canonical migrations realising §9.4 DDL
│   │   └── shared/                 # cross-service types, errors, RBAC helpers
│   └── web/                        # Astro 5 + line-ui frontend — P05 (v1.1)
│       ├── astro.config.mjs
│       ├── package.json
│       ├── src/
│       │   ├── pages/              # routes
│       │   ├── actions/            # Astro Actions = BFF (Law 4)
│       │   │   └── auth/
│       │   │       └── [provider]/
│       │   │           └── callback.ts  # OAuth callback (Astro origin)
│       │   ├── components/         # line-ui-based composition
│       │   └── lib/                # Encore RPC client wrappers
│       └── public/
├── crates/                         # Rust workspace — P03 + P04
│   ├── Cargo.toml                  # workspace manifest
│   ├── unblock-indexer-core/       # pure lib (parsing, kinds, span, mtime rules)
│   ├── unblock-indexer/            # impure lib (sqlx, tree-sitter, FS walker)
│   ├── unblock-code/               # bin (clap CLI; 11 commands)
│   └── unblock-plugin/             # bin (build-time renderer for Claude/Copilot)
│       └── data/
│           └── catalogue.json      # checked-in copy embedded via include_str!
├── docs/
│   ├── MANIFESTO.md                # APPROVED
│   ├── PRD.md                      # APPROVED
│   ├── SPEC.md                     # this file (Stage 1 architecture)
│   ├── plans/                      # per-phase plans (Stage 2, /plan output)
│   ├── specs/                      # per-phase specs (Stage 2, /spec output)
│   ├── research/                   # Stage 2 research artefacts (/research output)
│   └── code-cli/                   # locked AST CLI plan + spec + research
├── infra/                          # CI/CD, deploy, IaC — Olive's surface
│   ├── github/                     # .github/workflows/*
│   ├── encore/                     # encore deploy configuration
│   └── cloudflare/                 # cloudflare pages config (P05)
├── temp/
│   └── rust-v1/                    # archived legacy Rust workspace (gitignored)
├── README.md
├── AGENTS.md
└── CLAUDE.md
```

`temp/rust-v1/` is **gitignored** (local-only archaeology). It is excluded
from the v1.0 workspace manifest and is not built by CI. The Rust
`unblock-mcp` v1 binary is gone from the active workspace; the new MCP server
is the Encore Go service at `apps/api/mcp/`.

### 4.1 Why monorepo, not multi-repo

- **Atomic cross-cutting changes.** A change to the MCP tool inventory often
  needs a corresponding change to the plugin's BLOCK conditions and to the
  Astro Actions surface. Cross-repo PRs would split this into uncoordinated
  units; a monorepo keeps them atomic.
- **Single CI pipeline per supervisor.** Greta (Go) builds `apps/api/`,
  Aria (Astro) builds `apps/web/`, Neo (Rust) builds `crates/`, Olive
  (CI/CD) wires them. Cross-supervisor work coordinates through the same
  branching model.
- **Shared documentation surface.** `docs/` is the architect's, PM's, and
  reviewers' shared substrate. Splitting it across repos breaks the trace
  from PRD → SPEC → plan → spec → tasks → review.

---

## 5. Backend Architecture (Encore Go)

### 5.1 Why Encore Go (and what was rejected)

**Selected: Encore Go on a single Postgres.** Locked from the strategic
discussion in the Claude.ai chat (see PRD §14.1). AR-1 (lock-in) is
**accepted** with the documented exit path through self-hosted NATS + standard
Postgres if Encore ever blocks us.

| Need | How Encore satisfies it | Alternative considered | Why rejected |
|---|---|---|---|
| Public/private API distinction (Law 4 BFF) | Encore's `//encore:api public` vs `private` decorators are first-class | Vanilla Go with Chi + manual middleware | Loses the structural guarantee; private vs public becomes documentation, not a compile-time check |
| Pub/Sub for cascade (Law 1) | Encore's typed Pub/Sub topics over NATS in the local emulator and managed in Cloud | Postgres `LISTEN/NOTIFY` | Works at the v1 scale, but couples cascade to the same connection pool that serves reads. Encore's Pub/Sub gives an isolation boundary "for free" |
| Postgres provisioning + migrations + secrets | Encore's `sqldb.Database` + bundled migration runner | Go + sqlx + manual migrations + Vault | Re-builds Encore's table stakes; no value over the framework |
| Hosted free tier with a Cloud step-up path | Encore Cloud free tier for v1; managed Postgres + Pub/Sub | Self-hosted Fly.io / Railway | v1 has no SLA (PRD §9.2) and no users; managed infra cost beats engineering cost |

**Rejected wholesale:**

- **Node + tRPC + Postgres.** Greater type-safety per file but worse
  concurrency story for the cascade subsystem; Astro frontend already
  consumes the BFF, no second TS surface needed in the backend.
- **Rust + Axum + sqlx.** Excellent for the AST CLI; a poor fit for an MCP
  server with rapid product evolution, OAuth flows, webhook adapters, and
  Pub/Sub. The Rust v1 implementation taught us that the calendar cost of
  every product change in Rust is ~2× the Go equivalent at this product
  stage.

### 5.2 Service decomposition (8 services, 8 schemas, 1 database)

The eight services map 1:1 to the eight schemas declared in FR-1 (logical
ownership: each service is the only writer to its schema). **This 8:8 logical
mapping is locked** (Manifesto Principle 4: orthogonal deliverables applies
intra-backend too — `boards`, `mcp`, and `memory` stay separate). Cross-service
queries happen via Encore RPC; **no service reads another service's tables
directly** (this is enforced by Encore's per-service DB binding).

**Migration ownership (research C2, CONTRADICTED — corrected).** Encore's
database primitive is **per-service, not per-schema**: per the Encore docs,
"Each database is defined within a service" and the migrations directory
belongs to **one** owning service. Other services consume via
`sqldb.Named("name")` (read/write through Encore's DB binding) but cannot
contribute migration files. Therefore the eight schemas cannot ship as eight
service-local migrations directories — they must live under a single
**migration-owner service**.

We designate **`auth` as the migration-owner service**. Rationale:

- `auth` is the bottom of the dependency graph in §9.4.0 — every other schema
  has FKs into `auth.users` (or transitively via `org`), and no schema has a
  FK pointing **into** `auth`. Migrations starting from `auth` therefore
  satisfy FK ordering naturally.
- `auth` is small and stable; piggy-backing the canonical migrations on it
  does not couple migration cadence to a high-churn service.
- It is the lowest service Encore must deploy first regardless, so the
  deploy-ordering implication (below) costs nothing extra.

**Concrete shape.** All canonical-ordered SQL migration files for **all
eight schemas** in §9.4.0 order live under
`apps/api/auth/migrations/NNNN_<descr>.up.sql`. The `auth` service is the
only caller of `sqldb.NewDatabase("unblock", sqldb.DatabaseConfig{Migrations:
"./migrations"})` — it owns the named handle. The other seven services
(`org`, `workitems`, `deps`, `providers`, `mcp`, `boards`, `memory`)
declare `var db = sqldb.Named("unblock")` to obtain the same DB handle for
their queries. Each service still writes only to its logical schema; Encore's
per-service DB binding plus the `pkg/rbac` typed query helper (§5.6) reject
cross-schema writes at compile time.

**Deploy-ordering implication (new architectural risk AR-15).** Because
`auth` owns migrations and every other service depends on schemas created
by those migrations, **`auth` must reach a healthy state (migrations applied)
before any other service that runs queries against `org`, `workitems`,
`deps`, `providers`, `mcp`, `boards`, or `memory` can serve traffic**.
Encore handles this automatically through its dependency graph: a service
that calls `sqldb.Named("unblock")` implicitly depends on the service that
defined `unblock`, and Encore deploys in dependency order. The architecture
records this as AR-15 in §13 so future contributors do not collapse the
ordering guarantee into a "best effort" assumption.

**Why not the alternative of one DB per service.** Encore's native pattern
is one DB per service; we deliberately reject it because FR-1 mandates "a
single Postgres database with 8 schemas". Cross-schema FKs (every
`workitems.items.org_id → org.organizations(id)` reference) require a
single database — they cannot span Encore-managed databases. The
single-migration-owner pattern is the correct way to honour both FR-1 and
Encore's per-service DB ownership.

| Service | Schema | Owns | Public APIs | Private RPCs (sample) |
|---|---|---|---|---|
| `auth` | `auth` | OAuth2+PKCE flows, session cookies, API keys, identity claims | — *(callback is on Astro origin)* | `auth.Validate(token) → Identity`, `auth.ExchangeOAuthCode(code, pkce) → Session` |
| `org` | `org` | Orgs, projects, RBAC roles + bindings, label scoping rules | — | `org.Authorize(identity, resource, action)` |
| `workitems` | `workitems` | Work items, comments, labels, milestones (recursive), findings | — | `workitems.Create / Update / GetTrail / ListByMilestone` |
| `deps` | `deps` | Dependency edges, ready set materialisation, cycle detection, cascade subscriber | — | `deps.AddEdge / RemoveEdge / IsReady / Closure` |
| `providers` | `providers` | Provider integrations (GitHub at v1.0, +GitLab at v1.1), webhook signature verification, normalisation, bidirectional sync state | `POST /webhooks/github` (v1.0), `POST /webhooks/gitlab` (v1.1) | `providers.LinkRepo / Sync / Reconcile` |
| `mcp` | `mcp` | MCP transport (Streamable HTTP per spec 2025-06-18), tool registry, state-transition validator (Law 8 layer 1), `verify_can_transition`, `meta.catalogue` | `POST /mcp` + `GET /mcp` (single path, two methods) | (the MCP tool handlers themselves are the API; they call other services' private RPCs) |
| `boards` | `boards` | Kanban / saved view persistence, per-user view preferences | — | `boards.Save / Load / List` |
| `memory` | `memory` | Scoped memory entries (`org` / `project` / `user`), tsvector + tag index, secret sanitiser | — | `memory.Remember / Recall / List / Forget` |

#### 5.2.1 Why these eight, not fewer or more (CONFIRMED)

The user reviewed and locked this 8:8 decomposition. Isolation > RPC overhead:

- **`auth` separate from `org`** because identity (who is calling) and
  authorisation (what they can touch) are independently testable concerns;
  collapsing them invites regressions where a fix to OAuth bleeds into RBAC.
- **`workitems` separate from `deps`** because the graph engine has a
  distinct write protocol (cycle check, cascade emit, ready materialisation)
  that is the canonical hot path for Law 7. Mixing it into `workitems`
  would make the cascade subscriber an internal helper instead of a
  first-class component.
- **`mcp` separate from everything** because the state-transition validator
  (Layer 1, FR-9) sits at the MCP boundary and orchestrates RPCs into
  other services. Hosting it inside `workitems` would couple the validator
  to one schema and leak comment-trail awareness into `workitems`'s API.
- **`boards` separate from `workitems`** because saved views are a
  presentation concern with a per-user lifecycle; lumping them into
  `workitems` muddles RBAC (work items are org/project-scoped; boards
  are user-scoped within an org).
- **`memory` separate** because Manifesto Principle 7 elevates it to a
  first-class scoped service with its own MCP tool surface, full-text
  index, and secret sanitiser. It does not share a hot path with
  `workitems`.
- **`providers` separate** because Law 3 ("Postgres is the source of truth")
  demands that provider integrations be event sources whose failure cannot
  stop the rest of the product. A separate service isolates webhook-handler
  failures and reconciliation jobs from the hot path.

#### 5.2.2 27-tool MCP inventory at v1.0 (CONFIRMED — P01 round-16 reconciliation)

> **P01 round-16 reconciliation (2026-06-04).** The original "18-tool"
> inventory was raised to **27** by the P01 round-16 amendment
> (`docs/specs/01-spec-backend-mvp.md` round-16 changelog, beads
> `unblock-tv8.71` / `.74` / `.75`): P01 grows from **14 → 23** by adding
> `promote`, the four milestone management tools, and the four
> label-registry tools; v1.0 grows from 18 → **27** (23 P01 + 4 memory at
> P02). The inventory below is updated to match.

PRD FR-8 promises 27 tools at v1.0. The inventory below pins names and
one-line descriptions. P01 ships **23**, P02 adds the **4** memory tools.
The state-machine accessors (`set_state`, `get_state`, `verify_can_transition`)
and `meta_catalogue` are **operational primitives** that count as part of
the inventory — they are first-class agent-facing tools, not internal helpers.
Reconciliation note: PRD FR-8 lists the categories ("work-item CRUD,
dependencies, ready, claim, close, comment trail, prime, etc.") plus the
round-16 additions; PRD §8 P02 says "+4 memory tools = 27 total". This list
reconciles the two.

##### P01 — 23 tools (work item core, graph, claim, promote, comments, state, prime, milestones, labels)

| # | Tool | One-liner |
|---|---|---|
| 1 | `prime` | Single-call dashboard for a fresh agent session: returns ready set summary, claimed-by-me items, recent cascade events, and scoped memory hints |
| 2 | `ready` | Return the next ready work items for the calling agent's org/project, ordered by priority + creation time, with optional `--limit` |
| 3 | `claim` | Atomic claim (Law 5) — `SELECT FOR UPDATE`, sets `claimed_by_*` and `Status=InProgress`; rejects if already claimed |
| 4 | `create` | Create a work item (`type=task | epic | finding`) with optional parent, milestone, labels, and dependencies |
| 5 | `update` | Update a work item's mutable fields (title, body, priority, milestone, labels) — does **not** mutate state dimensions (use `set_state`) |
| 6 | `close` | Close a work item — fires the cascade subscriber; only allowed when `qa_state=passed` (or override path) |
| 7 | `show` | Return one item with its full comment trail, dependencies, and finding children |
| 8 | `list` | List items by org/project/milestone/label with filters on status, pipeline_stage, claimed-by, etc. |
| 9 | `search` | Full-text search across item titles + bodies + comment bodies; respects RBAC |
| 10 | `comment` | Append a structured `(kind, status, body)` comment to an item; comments are append-only (FR-10) |
| 11 | `add_dependency` | Add a `from blocks to` edge; cycle-checked at write time (NFR-5) |
| 12 | `remove_dependency` | Remove an edge; fires cascade subscriber (the "to" side may flip to ready) |
| 13 | `set_state` | Mutate one or more of `impl_state`, `review_state`, `qa_state`, `pipeline_state` — **gated by Layer-1 BLOCK conditions** (§7.5) |
| 14 | `get_state` | Return the four state columns + the derived `pipeline_stage` + the most recent `(kind, status)` per the comment trail |
| 15 | `promote` | (P01 round-16, bead `unblock-tv8.71`) Transition a Backlog item to Ready; precondition `status='Backlog' AND is_ready=true`. The canonical Ready writer (closes round-12 DRIFT-2) |
| 16 | `create_milestone` | (P01 round-16, bead `unblock-tv8.74`) Create a milestone (org- or project-scoped); facade over `workitems.CreateMilestone` |
| 17 | `update_milestone` | (P01 round-16, bead `unblock-tv8.74`) Update a milestone's name/description/dates/cancellation; reparenting rejected in P01 |
| 18 | `assign_item` | (P01 round-16, bead `unblock-tv8.74`) Assign an item to a milestone (or unassign via empty `milestone_id`); enforces M-INV-7 |
| 19 | `milestone_tree` | (P01 round-16, bead `unblock-tv8.74`) Return the recursive milestone tree (depth bound M-INV-6 = 4) |
| 20 | `create_label` | (P01 round-16, bead `unblock-tv8.75`) Create a user-facing label (org- or project-scoped) over `workitems.labels` |
| 21 | `list_labels` | (P01 round-16, bead `unblock-tv8.75`) List labels in scope (project labels + inherited org labels; project wins on name) |
| 22 | `update_label` | (P01 round-16, bead `unblock-tv8.75`) Rename and/or recolor an existing label (scope immutable) |
| 23 | `delete_label` | (P01 round-16, bead `unblock-tv8.75`) Delete a label; `item_labels` junction rows cascade (items are not deleted) |

##### P02 — adds 4 memory tools (FR-13)

| # | Tool | One-liner |
|---|---|---|
| 24 | `remember` | Write a scoped memory entry (`org` / `project` / `user`); secret sanitiser runs before encryption (NFR-7); 8 KB cap |
| 25 | `recall` | Read memory entries by scope + key; supports tag and full-text filters |
| 26 | `memories` | List memory entries by scope with pagination; cheap dashboard read |
| 27 | `forget` | Soft-delete a memory entry (audit-trail preserved via `deleted_at`-equivalent) |

##### Operational primitives (counted within the 27 above; cross-references)

- `set_state` (#13) and `get_state` (#14) are the canonical state-machine
  accessors. `set_state` is the one tool subject to all of Layer 1's BLOCK
  conditions (§7.5).
- `verify_can_transition` is **not** a separate top-level MCP tool — it is a
  read-only sub-call exposed via the same SSE channel for the Layer-2 hook
  (§7.4). It does not count as one of the 27; it is a thin facade over the
  same Layer-1 validator that backs `set_state`. (PRD FR-8 reconciliation:
  the FR-8 sentence "exposing 27 tools at v1.0" — raised from 18 by the P01
  round-16 amendment — refers to the agent-facing inventory above;
  `verify_can_transition` is a hook-facing primitive shared with the
  Layer-2 enforcement path.)
- `meta_catalogue` is **not** a separate top-level MCP tool either — it is a
  read-only catalogue endpoint served by the `mcp` service over the same SSE
  channel for the Layer-3 build-time renderer to verify against the
  checked-in `catalogue.json` (§7.2). It does not count toward the 27 for
  the same reason: it is an operational read primitive consumed by tooling,
  not by agents in their workflow.

The provider sync tooling promised by PRD FR-8 ("plus the providers/sync
tooling needed for bidirectional GitHub sync") lives **inside** the
`providers` service's private RPC surface, not as MCP tools — bidirectional
sync is automatic on webhook receipt, and reconciliation runs on a schedule.
There is no agent-facing "sync now" tool at v1.0; the architecture treats
provider sync as Law 3 background machinery, not an interactive primitive.

The exact JSON schema (request / response) per tool lands in the P01 / P02
phase specs; this section pins names and intent only.

### 5.3 Public surface (the only public endpoints)

Per FR-12, the following endpoints are reachable from the public Internet:

**v1.0 — two logical endpoints (both transports, same path):**

| Endpoint | Service | Purpose | Auth |
|---|---|---|---|
| `POST /webhooks/github` | `providers` | Provider event sink — webhook signature is verified, payload deduplicated via the `events_delivery_uniq UNIQUE (provider, delivery_id)` constraint (AR-12), then normalised to canonical `WorkItem` | HMAC signature header (no Bearer) |
| `POST /mcp` + `GET /mcp` | `mcp` | Remote MCP for agents over **Streamable HTTP** per the MCP 2025-06-18 spec — single endpoint path supports both `POST` (client → server JSON-RPC requests; may return either a single JSON-RPC response or a server-sent-events stream of incremental responses for long-running tools) and `GET` (server → client SSE stream for server-initiated messages, used for resumable / long-lived sessions). One logical endpoint at one path; two HTTP methods per the spec | `Bearer <api-key>` on every request; `Mcp-Session-Id` response header on `initialize`, echoed by the client on subsequent requests |

**v1.1 — adds one endpoint:**

| Endpoint | Service | Purpose | Auth |
|---|---|---|---|
| `POST /webhooks/gitlab` | `providers` | GitLab event sink (parallel to GitHub) | HMAC signature header |

**MCP transport choice (research C6, CONTRADICTED — corrected).** The
2024-11-05 MCP spec defined an HTTP+SSE transport with **two endpoints** —
a `GET /mcp/sse` SSE stream emitting an `endpoint` event to direct the
client to a separate `POST /mcp/messages` URL. The 2025-06-18 spec
**replaces** that with **Streamable HTTP**: a single endpoint path that
supports both `POST` (client → server) and `GET` (server-initiated
streaming). We ship Streamable HTTP from P01 — the legacy SSE+POST shape
is deprecated and we incur the migration cost now rather than on the
v1.0 → v1.1 boundary.

Streamable HTTP supports two response modes per `POST /mcp` invocation:

- **Classic JSON-RPC response.** The server returns a single
  `application/json` body containing the JSON-RPC response. Used for fast
  tools (e.g. `prime`, `ready`, `claim`, `get_state`).
- **Server-side event stream.** The server returns `text/event-stream` and
  emits incremental JSON-RPC responses, progress notifications, and
  log/diagnostic events as the tool runs. Used for tools whose execution
  may exceed conventional request budgets (none required at v1.0 — but the
  shape is supported because the spec mandates it; future tools that fan
  out into Pub/Sub-driven work can use it without a transport change).

Server-initiated streaming via `GET /mcp` is reserved for the resumable
session model (the server pushes `notifications/*` JSON-RPC frames to the
client when state mutates outside the client's request flow — e.g. a
cascade event reaches an agent's project). It is **not** required for the
P01 exit criterion; the P01 spec pins whether `GET /mcp` is implemented
at v1.0 or deferred to a later phase.

Backwards-compatibility: modern clients (Claude Code, Cursor) support
Streamable HTTP first and fall back to the legacy SSE+POST transport on
4xx. We do **not** ship the legacy fallback — clients pinned to the
2024-11-05 shape must upgrade.

**The OAuth callback is NOT an Encore endpoint.** It is an Astro Action at
`unblock.websublime.com/auth/[provider]/callback` (P05, v1.1). The Astro
Action validates PKCE state, then calls the private Encore RPC
`auth.ExchangeOAuthCode(code, pkce)` to obtain a session token, then sets the
HttpOnly cookie on the Astro origin. Backend credentials never cross the
browser boundary (Law 4 / NFR-4).

All other Encore APIs are private. The Astro Actions BFF (P05, v1.1) is the
sole privileged client for private APIs; until P05 ships, private APIs are
exercised only by integration tests and by the MCP handlers internal to the
backend.

#### 5.3.1 Astro server ↔ Encore private RPC authentication (CONFIRMED)

The Astro server (which hosts Astro Actions, the BFF) authenticates to
Encore's private API by **forwarding the user's session id** rather than by
holding a separate service credential. Concretely:

1. The browser holds an HttpOnly, SameSite=Lax cookie named `unblock_session`
   on the Astro origin (`unblock.websublime.com`). The cookie value is the
   opaque `auth.sessions.id` (ULID) issued by Encore during OAuth (§10.1).
2. Every Astro Action that needs to call a private Encore RPC reads the
   cookie, then calls Encore with two headers:
   - `Authorization: Bearer <session_id>` — the same session id the browser
     would have presented if the BFF discipline were relaxed; Encore validates
     it via `auth.Validate` and resolves it to the original `Identity` record.
   - `X-Unblock-BFF-Origin: astro` — a constant traceability header. Encore
     middleware uses this to differentiate "browser-direct" callers (which
     are forbidden against private RPCs except for the OAuth callback path)
     from "BFF-proxied" callers (which are allowed). The header is **not** an
     auth credential — it is a routing hint for log filtering and for the
     middleware's "this RPC is private; reject if origin is not BFF" rule.
3. The Astro deploy environment carries **no** separate Encore credential.
   There is no service token, no shared secret, no mTLS handshake. The auth
   surface for private RPCs is identical to what `auth.Validate` already
   knows how to do for the public `POST /mcp` + `GET /mcp` endpoint — the
   only difference
   is the `X-Unblock-BFF-Origin: astro` header tells Encore "this request was
   proxied through the BFF, not initiated by the browser".

**Why this mechanism (and what was rejected).**

| Option | Why rejected |
|---|---|
| Static service token in Astro deploy env | Adds a long-lived credential to rotate, complicates Cloudflare Pages secret hygiene, and gains nothing over forwarding the user's session because Encore must still resolve a user `Identity` to enforce RBAC. |
| mTLS between Astro runtime and Encore Cloud | Cloudflare Pages does not give us first-class mTLS originating from the worker; would require a custom proxy. Cost > value. |
| Encore's built-in service-to-service auth | Encore's S2S auth is for service-mesh-internal calls; it does not solve "what is `auth.UserID()` inside the handler", which is the value the BFF must preserve. |
| Forwarding the session (CHOSEN) | Preserves `auth.UserID()` inside Encore handlers (RBAC works unchanged), avoids managing a service credential, makes the BFF a transparent proxy. The BFF discipline is not weakened — the cookie is still HttpOnly, the browser still cannot reach Encore directly because Encore's middleware rejects requests without `X-Unblock-BFF-Origin: astro` *unless* they hit one of the documented public endpoints (FR-12). |

**The structural property.** A direct browser → Encore call to a private RPC
fails because:
- The browser cannot set arbitrary headers on a cross-origin request without
  the server's CORS allowance, and Encore's CORS does not allow
  `X-Unblock-BFF-Origin` from any origin other than `unblock.websublime.com`
  for private routes (and CORS preflight will reject the cross-origin
  preflight before the request body ever ships).
- Even if the header were spoofed (e.g. by a non-browser client), the session
  id is HttpOnly and cannot be read by JavaScript on the browser, so the
  attacker has no `Bearer` to send.

The cookie-on-Astro-origin + header-on-Encore combination preserves Law 4 by
construction; it does not introduce a new credential class.

This auth mechanism is also referenced from §10.1.

### 5.4 Cascade subsystem (Manifesto Law 1)

The cascade is **structural**, not best-effort. Its architecture:

1. Any mutation that writes a "terminal" status (e.g. `Status=Done`,
   `qa_state=passed`, edge removal) emits a `deps.cascade.requested` event
   on a typed Encore Pub/Sub topic.
2. A subscriber inside `deps` consumes the event in the same logical
   operation, recomputes the affected closure, materialises `is_ready=true`
   on `workitems.items` for newly unblocked items, **writes one row to
   `deps.cascade_events`** capturing the trigger item, the affected item set,
   and the cardinality (the `cascaded_count` column powers PRD M-5), and
   emits a `deps.cascade.completed` event carrying the promoted set.
3. The MCP `ready` handler reads the materialised `is_ready` column — it
   does not recompute the graph at read time.

This is what makes `prime → ready → claim` p99 < 2 s feasible (Law 7): reads
are O(filter on `is_ready=true`), not graph traversals. **`is_ready` is
NOT a Postgres `GENERATED` column** — it is a regular boolean column updated
by the cascade subscriber via UPDATE. Postgres `GENERATED` columns cannot
reference other rows, and the readiness predicate is over an item's
incoming edges, which lives in another schema. The Pub/Sub subscriber is the
canonical maintainer of `is_ready` **and `pipeline_stage`** (see §5.7). See
§9.4.4 for the closure CTE pattern and the `deps.cascade_events` audit table.

**Idempotency (AR-11).** Encore Pub/Sub is at-least-once; the subscriber may
see the same `deps.cascade.requested` event twice. The subscriber is
idempotent **by construction**: the closure CTE produces the same target set
for the same input graph, the `UPDATE … SET is_ready = …` is a value-equality
write (idempotent on a stable graph), and `deps.cascade_events` rows are
written once per `(event_id, triggered_by_item_id)` — see AR-11 in §13.

**Event ID provenance (research C1, CONTRADICTED — corrected).** The Encore Go
Pub/Sub SDK's subscriber handler signature is
`func(ctx context.Context, msg T) error` — it does **not** expose any envelope
metadata (no message id, no attempt count, no delivery headers) to the
handler. We therefore **cannot** propagate a provider-supplied delivery id
from the Pub/Sub envelope. Instead, the **publisher** generates a ULID at
emit time and embeds it as a typed field on the message struct:

```go
type CascadeRequested struct {
    EventID            string // ULID, generated at publish time
    TriggeredByItemID  string
    OrgID              string
    ProjectID          string
    TraceID            string
    // ... other fields
}
```

The subscriber reads `msg.EventID` from the typed payload and uses it as the
idempotency key. Encore's at-least-once redelivery resends the same payload
bytes (including `EventID`), so duplicate deliveries collide on the
`(event_id, triggered_by_item_id)` UNIQUE constraint on
`deps.cascade_events` (§9.4.4). The constraint is enforced at the database
level via `INSERT … ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING`,
so a second delivery is a no-op insert plus an idempotent UPDATE pass over
`is_ready` and `pipeline_stage`.

### 5.5 Atomic claim (Manifesto Law 5)

`claim` is a single Postgres transaction:

```sql
BEGIN;
  SELECT id FROM workitems.items
   WHERE id = $1 AND status = 'Ready' AND claimed_by_id IS NULL
   FOR UPDATE;
  UPDATE workitems.items
     SET claimed_by_id = $2, claimed_by_agent = $3,
         claimed_at = now(), status = 'InProgress'
   WHERE id = $1;
COMMIT;
```

Two agents racing the same item: one's `SELECT FOR UPDATE` returns no row
(post-update visibility from the winner's commit), and the loser receives a
structured "already claimed" error citing the winner's identifier and
timestamp. Exact error envelope lands in the P01 spec.

### 5.6 RBAC (NFR-2 — zero cross-tenant leaks)

Org-level RBAC is enforced as **Postgres row-level filtering** (FR-3),
applied uniformly to every read and write path. Two architectural choices
make this a structural property, not a discipline property:

1. Every service's RPC handlers receive an `Identity` token validated by
   `auth.Validate`; the Identity carries `(user_id, org_id, role)`.
2. Every query against an org/project-scoped table includes the org/project
   filter as a non-negotiable clause; Encore's per-service DB binding plus
   a shared `pkg/rbac` helper provide a typed query builder that refuses
   to compile a query missing the scope filter.

The exhaustive security regression suite (NFR-2) lives under
`apps/api/shared/rbactest` and runs against every release candidate.

### 5.7 State machine (Law 8 layer 1)

Per PRD §6.7, the state machine is encoded inside the `mcp` service. Every
status-changing tool call (`set_state`, `claim`, `close`) is gated by
explicit preconditions; invalid transitions are rejected with a structured
error citing the missing precondition. The state machine reads the comment
trail (FR-10) — e.g. `qa_state → passed` requires a `(kind=qa,
status=success)` comment to exist. The exact precondition map and error
shapes land in the P02 spec. The state-machine catalogue is exported via
the `mcp.meta_catalogue` MCP tool **and** checked-in at
`crates/unblock-plugin/data/catalogue.json` for build-time consumption.

**State-machine source-of-truth and codegen (CONFIRMED).** The Go
state-machine in `apps/api/mcp/` is **generated** from `apps/api/mcp/catalogue.json`
via `go generate` — drift between the catalogue and the in-process validator
is impossible by construction. The codegen target is
`apps/api/mcp/catalogue.gen.go` (committed; CI fails if `go generate` produces
a diff against the committed file). This is one of two parallel embeds of the
same JSON: Go embeds it via codegen, Rust embeds it via `include_str!`
(§7.2). The CI catalogue-drift test (§7.2) closes the third corner — the
live `mcp.meta_catalogue` response. All three derive from the same source
file; see AR-4 in §13.

#### 5.7.1 `pipeline_stage` derivation (CONFIRMED)

`workitems.items.pipeline_stage` is **derived** from the three orthogonal
state dimensions (`impl_state`, `review_state`, `qa_state`) plus the
`pipeline_state` exception column. It is **not** a value an agent or human
writes directly — it is a stored, materialised projection maintained by the
**same Pub/Sub subscriber that writes `is_ready`** (§5.4). The subscriber
reads the four state columns post-mutation and writes the derived label in
the same UPDATE, so reads against the column are O(1).

The decision to materialise the column (rather than expose a view) is
deliberate: the Astro web client's kanban view groups items by
`pipeline_stage`, the `boards` filter JSON references it, and the MCP
`list` / `search` tools accept it as a filter argument. A computed view
would force every read path into a join + CASE evaluation; a materialised
column collapses that to a single scalar lookup, preserving the M-1
latency budget (Law 7).

**Derivation table (authoritative).** The subscriber applies these rules in
order; the first match wins. `pipeline_state ≠ running` short-circuits the
three-dimension logic.

| Input (in order of evaluation) | `pipeline_stage` |
|---|---|
| `pipeline_state = needs_human` | `Deferred` |
| `pipeline_state = paused` | `Deferred` |
| `pipeline_state = no_investigation` AND `impl_state = pending` | `Implementation` |
| `status = Done` OR (`qa_state = passed` AND `closed_at IS NOT NULL`) | `Done` |
| `qa_state = passed` (closure pending) | `Quality` |
| `qa_state = failed` | `Quality` |
| `review_state = approved` AND `qa_state = pending` | `Quality` |
| `review_state = needs_rework` | `Implementation` |
| `impl_state = done` AND `review_state = pending` AND a `kind=review` comment exists | `Review` |
| `impl_state = done` AND `review_state = pending` AND no `kind=review` comment yet | `Implementation` |
| `impl_state = pending` AND a `kind=investigation` comment exists | `Implementation` |
| `impl_state = pending` AND no `kind=investigation` comment exists | `Investigation` |

The "comment exists" predicates are evaluated against `workitems.comments`
filtered by `item_id` and `kind`. The subscriber issues one batched query
per cascade pass (`SELECT item_id, max(case when kind='review' then 1 else 0
end), max(case when kind='investigation' then 1 else 0 end) FROM
workitems.comments WHERE item_id = ANY($affected) GROUP BY item_id`) so the
materialisation cost is bounded by the affected set size, not by a per-row
N+1 scan.

The §9.4 DDL retains `pipeline_stage` as a stored `text NOT NULL DEFAULT
'Investigation'` column with the §6.1 CHECK constraint. An explanatory
comment on the column documents that it is "subscriber-maintained;
do not write directly outside the cascade subscriber". Direct writes from
other paths are a class of bug caught by integration tests that assert
post-mutation `pipeline_stage` matches the derivation table for a hand-curated
fixture set.

**Why a materialised column rather than a Postgres `GENERATED` column or a
view?** Same reason as `is_ready`: Postgres `GENERATED` columns cannot
reference other rows or other tables, and the predicate is over the comment
trail. A view would lose the index path the boards/kanban queries depend on.
The subscriber model unifies maintenance of `is_ready` and `pipeline_stage`
into one Pub/Sub-driven write path.

---

## 6. AST CLI Architecture (Phase 03)

The AST CLI carries forward verbatim from
[docs/code-cli/plan.md](./code-cli/plan.md) and
[docs/code-cli/spec.md](./code-cli/spec.md). This section exists only to
declare the boundary, the crate inventory, and the architectural invariants
that bind the CLI to the rest of the product.

### 6.1 Crate inventory (locked in code-cli/plan §4)

| Crate | Kind | Role |
|---|---|---|
| `unblock-indexer-core` | pure lib | Domain types: `SymbolKind` (17 variants), `Span`, `LanguageId`, mtime invariant, no I/O |
| `unblock-indexer` | impure lib | sqlx + SQLite + FTS5 schema and queries, tree-sitter parsing, FS walker (`ignore` crate), per-query mtime check |
| `unblock-code` | bin | clap CLI hosting 11 commands; the only consumer-visible surface |
| `unblock-plugin` | bin | (Phase 04 — see §7) — sibling of `unblock-code`, not a consumer of indexer crates |

### 6.2 Architectural invariants (Manifesto Law 6 enforcement)

- **No backend coupling.** `unblock-code` does not import `apps/api/` types,
  does not call the backend over HTTP, and does not consume MCP. It is a
  one-shot CLI that operates on the local filesystem and a local SQLite
  cache only.
- **No daemon, no watcher.** Per-query mtime check is the **sole** sync
  mechanism between invocations (locked in code-cli/plan L9).
- **No code generation.** The CLI indexes, queries, and reports. It never
  writes to the working tree (Manifesto out-of-scope).
- **JSON envelopes on STDOUT, tracing on STDERR.** Per NFR-12. No mixing.

### 6.3 Distribution

cargo-dist publishes prebuilt artefacts for **Linux x86_64**, **macOS
aarch64**, and **Windows x86_64** (FR-28). Homebrew formula and npm wrapper
redistribute the same artefacts; no per-platform compile-from-source path.

---

## 7. Plugin Renderer Architecture (Phase 04)

`unblock-plugin` is a Rust binary in `crates/unblock-plugin/`. It runs
**at agent setup time**, not at runtime. Its job is to render the typed
catalogue (8 fixed personas, dynamic supervisors, 20 skills, 3 hooks per
PRD §6.8 / §6.9 / §6.12) onto two host plugin systems:

| Host | Format | What is rendered |
|---|---|---|
| Claude Code | `~/.claude/...` markdown + TOML | Persona prompts, slash skill descriptions, session-start / preToolUse / Stop hooks |
| GitHub Copilot (cloud + local) | Copilot custom instructions / chat config | Same persona + skill catalogue, with hook fall-back to dispatch convention `@<persona>: <task>` for Copilot local (no programmable hooks) |

### 7.1 CLI surface (CONFIRMED, carries forward verbatim from v1 §7.5.5)

```
unblock-plugin render --target=<target> --supervisors=<list> --out=<dir> [--apply]
```

| Flag | Values | Effect |
|---|---|---|
| `--target` | `claude-code`, `copilot-cloud`, `copilot-local` | Which host plugin format to emit |
| `--supervisors` | comma-separated stack list (e.g. `greta,aria,neo,olive`) | Which dynamic supervisors to include |
| `--out` | directory path | Where to write the rendered artefacts (or print preview) |
| `--apply` | flag (default off) | When set, writes directly into `.claude/agents/`, `.claude/skills/`, `.claude/hooks/`, `.claude/settings.json`, `.github/agents/`, `.github/copilot-instructions.md` per target. When unset, prints rendered output to stdout for inspection. |

There is **no** subcommand called `install`. `render` is the canonical
operation. Without `--apply`, the renderer is a pure preview; with `--apply`,
it materialises files at their host-specific paths. This makes the renderer
trivially testable (golden-file tests on the preview output) and gives users
a clean "dry run" workflow.

### 7.2 Catalogue source (CONFIRMED)

The state-machine + tool catalogue lives in **two synchronised places**:

1. `crates/unblock-plugin/data/catalogue.json` — checked-in copy, embedded
   into the binary at compile time via Rust's `include_str!`. This makes the
   plugin renderer fully offline — `unblock-plugin render --target=...` runs
   without ever contacting the backend.
2. `apps/api/mcp/catalogue.json` — the canonical source, served live by the
   `mcp.meta_catalogue` MCP tool call.

**No runtime coupling between plugin and backend.** The plugin renderer
runs at **build time only** and reads the **checked-in** JSON via
`include_str!`. It does not call the backend. The backend's `mcp.meta_catalogue`
endpoint is **not** consulted by the renderer in production — its only job
is to give CI a "live" reference to diff against the checked-in copy.

**Drift mitigation:** a CI test (`.github/workflows/catalogue-drift.yml`)
boots the local Encore emulator, calls `mcp.meta_catalogue`, diffs against
`crates/unblock-plugin/data/catalogue.json`, and fails on mismatch. **CI is
the only point at which the two copies are reconciled** — there is no
runtime path that contacts the backend from the renderer. The checked-in
JSON is the build-time source of truth; the live MCP endpoint is the
runtime source of truth for agents (e.g. an agent calling `mcp.meta_catalogue`
to introspect available transitions); they are never allowed to disagree on
`main`.

The dual location is deliberate: build-time embed keeps the renderer
hermetic (no network at install time, no runtime backend coupling); the
live endpoint serves agent introspection and the CI drift test. The
renderer never hits the network.

### 7.3 Why a renderer, not a runtime agent

- **Manifesto Law 6.** A runtime agent inside the host would couple the
  product to the host's process model. A build-time renderer produces
  static config consumed by the host's own machinery.
- **Layer 3 of Law 8** — agent prompt structure with explicit BLOCK
  conditions — is *static text*. Rendering it at build time is exactly
  the right level.
- **Layer 2 of Law 8** — the post-dispatch state validator — is a *hook*
  registered at install time that calls MCP `verify_can_transition` at
  the host's Stop / agentStop event. The renderer emits the hook
  configuration; the actual validation runs against the live MCP server.

### 7.4 Three layers of Law 8 — where each lives architecturally

| Layer | Where it lives | When it runs | What it does |
|---|---|---|---|
| **Layer 1 — MCP state-transition validation** | `apps/api/mcp/` (Encore service) | At every MCP tool call that mutates state | Rejects the call with a structured error if the precondition does not hold |
| **Layer 2 — post-dispatch state validator** | `unblock-plugin` renders the `verify-state` hook into Claude Code Stop / Copilot agentStop event; the hook calls MCP `verify_can_transition` | After every dispatched session ends | Surfaces non-compliance as a `type=finding` work item linked to the parent epic via `parent_id` and `discovered_from_id` |
| **Layer 3 — agent prompt structure** | `unblock-plugin` renders BLOCK conditions into the persona prompt body | Read by the agent at session start; checked at every relevant tool call inside the session | Refuses to issue a violating call before it ever reaches MCP |

All three layers carry the **same** state-machine knowledge by construction:
the state-machine catalogue is owned by the backend; the plugin renderer
reads that catalogue at build time (via `include_str!` of the checked-in JSON)
and emits matching BLOCK conditions and verify hooks. Bypass requires
simultaneously defeating all three.

### 7.5 BLOCK condition schema (CONFIRMED)

Manifesto Principle 8 and PRD FR-16 promise that "all three layers agree" on
the pipeline. That promise is verifiable only if the BLOCK conditions have a
**typed, machine-checkable shape**. The shape below is the canonical schema;
it is the single representation that all three layers consume.

#### 7.5.1 Shape

```jsonc
{
  "tool": "set_state",                // MCP tool name; matches §11 inventory
  "transition": "qa_state.pending->passed",  // human-readable transition name
  "precondition_human": "review_state == approved AND a comment with kind=qa, status=success exists on this item",
  "required_state": {
    // Each field is optional. Presence means "must hold". Absence means "any value permitted".
    "impl_state":           "done",        // enum value or null
    "review_state":         "approved",
    "qa_state":             "pending",     // the state that must hold BEFORE the transition
    "pipeline_state":       "running",
    "last_comment_kind":    "qa",          // most recent comment's kind on the item
    "last_comment_status":  "success",     // most recent comment's status
    "any_comment_kind":     null,          // OR-shape: a comment with this kind must exist on the item
    "any_comment_status":   null,
    "claimed":              true           // claimed_by_id IS NOT NULL
  },
  "error_code":     "PIPELINE_PRECONDITION_NOT_MET",
  "error_message":  "QA can only pass after review approves and a (kind=qa, status=success) comment is recorded.",
  "rejection_reason": "qa_state.passed.requires.review_approved_and_qa_success_comment"
}
```

Field semantics:

- `tool` — the MCP tool name from §11. Pinning the tool keeps the catalogue
  navigable even when one tool guards multiple transitions.
- `transition` — a stable, human-readable label. Used in `mcp.tool_calls.rejection_reason`
  for analytics (e.g. "which transition rejects most often?").
- `precondition_human` — natural-language form, rendered into the agent's
  prompt by Layer 3. Must mirror `required_state` exactly.
- `required_state` — the **machine-checkable** form. Layer 1 (Go) compiles
  this into a state-validator function; Layer 2 (`verify-state` hook) sends
  it back to MCP `verify_can_transition` for re-validation; Layer 3 (Rust
  renderer) emits the same fields into the prompt's BLOCK section. All three
  derive from this single object.
- `error_code` / `error_message` — what the agent sees on rejection. Layer 1
  returns these on the wire; Layer 3 renders them as the BLOCK message.
- `rejection_reason` — short symbolic code recorded in
  `mcp.tool_calls.rejection_reason` for telemetry.

#### 7.5.2 Catalogue location and how each layer consumes it

The full set of BLOCK conditions lives **inside the same `catalogue.json`**
that carries the state-machine transitions (§5.7) — at the JSON path
`.transitions[].block_conditions` (one or more BLOCK condition objects per
transition). The catalogue is consumed three times from the same source file:

| Layer | Consumer | How |
|---|---|---|
| Layer 1 | `apps/api/mcp/catalogue.gen.go` | `go generate` reads `apps/api/mcp/catalogue.json` and emits a typed Go validator per transition. The generated file is committed; CI fails on a `go generate` diff. |
| Layer 2 | `verify-state` hook → MCP `verify_can_transition` | The hook does not parse the catalogue itself — it calls the MCP RPC, which uses the same Layer-1 generated validator. One source, one validator, two call sites. |
| Layer 3 | `crates/unblock-plugin/data/catalogue.json` (compile-time `include_str!`) | The Rust renderer iterates `transitions[].block_conditions` and emits matching BLOCK clauses into the persona-prompt markdown. |

The CI catalogue-drift test (§7.2) compares all three corners. If any layer's
view of `block_conditions` diverges, the build fails. **This is what makes
"all three layers agree" structural** rather than aspirational.

#### 7.5.3 Example — `qa_state` PASS transition

```jsonc
{
  "transition_id": "qa_state.pending->passed",
  "from": { "qa_state": "pending" },
  "to":   { "qa_state": "passed"  },
  "block_conditions": [
    {
      "tool": "set_state",
      "transition": "qa_state.pending->passed",
      "precondition_human": "review_state must be 'approved' and a (kind=qa, status=success) comment must exist on this item",
      "required_state": {
        "review_state":      "approved",
        "qa_state":          "pending",
        "any_comment_kind":  "qa",
        "any_comment_status": "success"
      },
      "error_code":      "PIPELINE_PRECONDITION_NOT_MET",
      "error_message":   "QA cannot pass: either review_state is not approved, or no (kind=qa, status=success) comment is recorded yet.",
      "rejection_reason": "qa_state.passed.requires.review_approved_and_qa_success_comment"
    }
  ]
}
```

The PRD §6.7 transition table is the human-readable counterpart of this JSON.
Ada (architect) keeps both in sync; the CI drift test enforces it
mechanically. The exact list of transition objects is part of `catalogue.json`
and lands in the P02 phase spec (the Layer-1 implementation phase).

---

## 8. Web Architecture (Phase 05, v1.1)

### 8.1 Why Astro 5 + line-ui + Cloudflare Pages

| Need | Astro + line-ui | Alternative considered | Why rejected |
|---|---|---|---|
| BFF discipline (Law 4) | Astro Actions are first-class server functions on the Astro origin; HttpOnly cookie there only; OAuth callback is an Astro Action | Next.js + Server Actions | Comparable; Astro chosen for its smaller runtime and explicit static-by-default posture, plus team familiarity |
| Headless components, framework-agnostic | `line-ui` (websublime, Web Components on Zag.js) — WebComponents portable across hosts | shadcn/ui (React-only) | Locks the UI to React; defeats Astro's framework-island story |
| Edge hosting, no SLA constraints | Cloudflare Pages free tier | Vercel | Vercel's free tier has stricter SLA and bandwidth gates; Cloudflare is more permissive at v1 scale |

### 8.2 BFF discipline (Law 4) in concrete shape

- The browser holds **one** credential: an HttpOnly, SameSite=Lax cookie
  scoped to the Astro origin, set by the `auth/[provider]/callback` Astro
  Action after PKCE-validated OAuth.
- The browser **never** has an Encore API key. It **never** has a GitHub
  OAuth token. It **never** has anything that could authenticate against
  the backend directly.
- Astro Actions are the **sole** privileged client of the Encore private
  API. They take the cookie, exchange it for an Encore-internal session
  via a private RPC, and forward the call.
- The Encore API enforces this on its side: private RPCs reject any caller
  whose Encore-internal session is not present.

### 8.3 Views at v1.1

Per FR-19: kanban board, dependency graph, roadmap (with recursive
milestone tree per PRD §6.3), per-item comment trail. Each view is one
Astro page composed of line-ui primitives plus a small set of bespoke
components.

**Graph rendering library: deferred to P05 spec (CONFIRMED).** The
architectural commitment is "force-directed dependency graph rendered as
canvas / SVG" with no library lock-in. The exact library (Cytoscape.js,
d3-force, sigma.js, Vis.js, etc.) is locked in the P05 phase spec at
Stage 2.

### 8.4 Hard dependency on line-ui v1

P05 cannot start before line-ui (vitamin repo) reaches feature-complete v1
covering forms, dialogs, dropdowns, popovers, tabs, toasts, navigation,
accordions, tooltips, date pickers, comboboxes, and mobile-responsive
defaults (PRD §10.1). This is the architectural reason v1.0 ships headless
and v1.1 carries the web.

---

## 9. Data Architecture

### 9.1 One Postgres, eight schemas (FR-1)

The entire product runs on a **single Postgres database** with **eight
schemas**. No additional persistent stores. No Redis. No SQLite inside the
API service. Postgres is the source of truth (Law 3); everything that is
not Postgres is either a derived view or an event source.

| Schema | Owning service | What it holds |
|---|---|---|
| `auth` | `auth` | identities, sessions, API keys, OAuth state |
| `org` | `org` | orgs, projects, role bindings, label scopes |
| `workitems` | `workitems` | items (with `milestone_id` column), comments, labels, milestones (recursive), findings, item↔label junction |
| `deps` | `deps` | edges, materialised `ready` view, cycle audit, **cascade event audit (M-5 metric source)** |
| `providers` | `providers` | provider links, webhook delivery audit, sync state |
| `mcp` | `mcp` | MCP API keys, tool call audit, state-transition rejections |
| `boards` | `boards` | saved views, kanban column configs, per-user view prefs |
| `memory` | `memory` | scoped memory entries with `tsvector` and tag indices |

### 9.2 Schema-level invariants (high-level)

- **Work item state lives as enum columns**: `status`, `priority`,
  `pipeline_stage`, `agent_kind`, plus the three orthogonal dimensions
  `impl_state`, `review_state`, `qa_state` and the `pipeline_state`
  exception column (PRD §6.1, §6.2). No derived label column. No label
  reconciliation.
- **Comments are append-only** with `(kind, status)` orthogonal axes
  (FR-10). `status` is `NOT NULL DEFAULT 'info'`. The product imposes no
  cross-axis validation matrix; UI and queries filter on `status`.
- **Findings are first-class child work items** with `type=finding`,
  `severity ∈ {critical, major, minor, risk, extra, deviation}`,
  `parent_id` pointing at the originating bead's parent epic, and
  `discovered_from_id` pointing at the originating bead (PRD §6.6).
- **Milestones are recursive** with self-referential `parent_milestone_id`,
  max depth 4, child date-range ⊆ parent range, sibling overlap allowed
  with warning (PRD §6.3 invariants M-INV-1 … M-INV-7). `workitems.iterations`
  is dropped.
- **Dependency edges live in `deps.dependencies`**, with a check on insert
  that rejects cycle creation at write time (NFR-5). The
  `workitems.items.is_ready` boolean is materialised by the cascade subscriber,
  not recomputed at read time.
- **Memory entries** carry `scope ∈ {org, project, user}`, `value_size ≤ 8 KB`,
  GIN `tsvector` over the value, and a tag column with a GIN index (FR-13).

### 9.3 What is *not* in Postgres

- Provider state (it lives in GitHub / GitLab); the local `providers` schema
  holds only links + sync metadata.
- AST CLI index (it lives in `~/.cache/unblock/repos/<hash>/index.db` per
  user machine — Law 6).
- Build-time plugin output (it lives in the host plugin directory, not in
  the database).

### 9.4 Canonical DDL (locked at architecture level)

This subsection is the **canonical schema reference** for the product. Phase
specs may add indexes or columns later but **must not deviate** from the
column types, constraints, or relationships defined here. PRs that change
the schema must update this section.

#### 9.4.0 Conventions and design choices

- **Primary keys are ULIDs as `text PK`.** ULIDs are 26-char Crockford-base32
  strings, lexicographically sortable, generated client-side in Go via
  `github.com/oklog/ulid/v2`. We use `text` rather than `uuid` because ULID
  is not a UUID and we want the index to sort by creation time.
- **All timestamps are `timestamptz` with `DEFAULT now()`.** Postgres stores
  in UTC; clients render in their local TZ.
- **Enums use `text` columns plus `CHECK (col IN (…))` constraints**, not
  Postgres `CREATE TYPE … AS ENUM`. **Tradeoff explained:** Postgres enum
  types require `ALTER TYPE` migrations to add values, which lock the type
  briefly and complicate cross-schema use; `text + CHECK` allows additive
  enum changes via a simple `ALTER TABLE … DROP CONSTRAINT … ADD CONSTRAINT`
  with no type-level lock. Pre-prod (no migrations needed) makes the
  flexibility worth more than the marginal storage savings of native enum
  types. The CHECK constraint is named so it can be rewritten in place.
- **Cross-schema FKs are explicit.** Every FK that crosses a schema boundary
  uses the fully-qualified name (`org.organizations(id)`, etc.) and is
  declared `ON DELETE` with a deliberate policy (`CASCADE`, `RESTRICT`, or
  `SET NULL` per case below).
- **Encrypted columns use the `pgcrypto` `pgp_sym_encrypt` /
  `pgp_sym_decrypt` family** with a symmetric key supplied via Encore secret
  `MEMORY_DEK` (data encryption key). Columns named with the suffix `_enc`
  hold ciphertext; decryption happens at the service layer with policy-driven
  scope checks. **Key rotation strategy:** introduce `MEMORY_DEK_NEXT`,
  re-encrypt rows in batches in a background job, swap the secrets, drop the
  old one. Documented as AR-7 in §13.
- **Migration ordering:** schemas migrate in this order due to cross-schema
  FK direction. **All eight schemas migrate from a single owner directory**
  (`apps/api/auth/migrations/`, see §5.2 for the rationale — Encore's
  database primitive is per-service, so one service owns the migrations
  directory for the whole DB):
  1. `auth` (no incoming FKs from other schemas).
  2. `org` (FKs into `auth` only).
  3. `workitems` (FKs into `org` and `auth`).
  4. `deps` (FKs into `workitems`).
  5. `providers` (FKs into `org`, `workitems`, `auth`).
  6. `mcp` (FKs into `auth`, `org`).
  7. `boards` (FKs into `org`, `auth`).
  8. `memory` (FKs into `auth`, `org`).

  Filename convention: `NNNN_<descr>.up.sql` with `NNNN` strictly increasing.
  Files for different logical schemas are grouped by the §9.4.0 step number
  (e.g. `0010_bootstrap.up.sql` for `CREATE EXTENSION` declarations,
  `0020_auth.up.sql`, `0030_org.up.sql`, …, `0090_memory.up.sql`) and
  share the single `auth/migrations/` directory. Adding a column to (e.g.)
  `workitems` in a later phase ships as a new file `0095_workitems_add_…
  .up.sql` under the same directory.
- **Required Postgres extensions** (declared once, in the bootstrap migration):
  ```sql
  CREATE EXTENSION IF NOT EXISTS pgcrypto;   -- symmetric encryption for *_enc columns
  CREATE EXTENSION IF NOT EXISTS pg_trgm;    -- trigram similarity for label and tag fuzzy search
  ```
  Postgres `tsvector` GIN support is built-in and requires no extension.

#### 9.4.1 Schema `auth`

```sql
CREATE SCHEMA IF NOT EXISTS auth;

-- Identities. Single primary identity per user (PRD FR-2).
CREATE TABLE auth.users (
    id                  text         PRIMARY KEY,                          -- ULID
    primary_provider    text         NOT NULL,                             -- 'github' | 'gitlab'
    primary_provider_id text         NOT NULL,                             -- provider-side user id
    email               text         NOT NULL,
    display_name        text         NOT NULL,
    avatar_url          text,
    created_at          timestamptz  NOT NULL DEFAULT now(),
    updated_at          timestamptz  NOT NULL DEFAULT now(),
    deleted_at          timestamptz,                                       -- soft delete
    CONSTRAINT users_primary_provider_chk
        CHECK (primary_provider IN ('github', 'gitlab')),
    CONSTRAINT users_primary_provider_unique
        UNIQUE (primary_provider, primary_provider_id)
);
CREATE UNIQUE INDEX users_email_active_uniq
    ON auth.users (lower(email))
    WHERE deleted_at IS NULL;

-- OAuth tokens (encrypted at rest). Linked to users; multiple providers per user
-- supported, but only one is primary per FR-2 (others are event-source attachments).
CREATE TABLE auth.oauth_tokens (
    id                text         PRIMARY KEY,                            -- ULID
    user_id           text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    provider          text         NOT NULL,                               -- 'github' | 'gitlab'
    access_token_enc  bytea        NOT NULL,                               -- pgp_sym_encrypt(...)
    refresh_token_enc bytea,                                               -- nullable; not all providers issue refresh tokens
    scopes            text[]       NOT NULL DEFAULT '{}',
    expires_at        timestamptz,
    created_at        timestamptz  NOT NULL DEFAULT now(),
    rotated_at        timestamptz,                                         -- last refresh
    CONSTRAINT oauth_tokens_provider_chk
        CHECK (provider IN ('github', 'gitlab')),
    CONSTRAINT oauth_tokens_user_provider_uniq
        UNIQUE (user_id, provider)
);
CREATE INDEX oauth_tokens_user_idx ON auth.oauth_tokens (user_id);

-- Sessions. HttpOnly cookie payload references the session id; rotation is
-- expressed as a new row + revocation of the previous one.
CREATE TABLE auth.sessions (
    id           text         PRIMARY KEY,                                 -- ULID; opaque session id
    user_id      text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    issued_at    timestamptz  NOT NULL DEFAULT now(),
    last_seen_at timestamptz  NOT NULL DEFAULT now(),
    expires_at   timestamptz  NOT NULL,
    revoked_at   timestamptz,
    user_agent   text,
    ip_inet      inet,
    CONSTRAINT sessions_expiry_chk
        CHECK (expires_at > issued_at)
);
-- Partial index: only active sessions matter for the auth hot path.
CREATE INDEX sessions_active_user_idx
    ON auth.sessions (user_id, last_seen_at DESC)
    WHERE revoked_at IS NULL;
```

#### 9.4.2 Schema `org`

```sql
CREATE SCHEMA IF NOT EXISTS org;

CREATE TABLE org.organizations (
    id          text         PRIMARY KEY,                                  -- ULID
    slug        text         NOT NULL,
    name        text         NOT NULL,
    description text,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),
    deleted_at  timestamptz,
    CONSTRAINT organizations_slug_uniq UNIQUE (slug)
);

-- Roles are encoded as text + CHECK (per §9.4.0). 4 roles at v1.0.
CREATE TABLE org.members (
    id         text         PRIMARY KEY,                                   -- ULID
    org_id     text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    user_id    text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    role       text         NOT NULL,                                      -- 'owner' | 'admin' | 'member' | 'viewer'
    invited_by text         REFERENCES auth.users(id) ON DELETE SET NULL,
    created_at timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT members_role_chk
        CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    CONSTRAINT members_org_user_uniq
        UNIQUE (org_id, user_id)
);
CREATE INDEX members_user_idx ON org.members (user_id);
CREATE INDEX members_org_role_idx ON org.members (org_id, role);

CREATE TABLE org.projects (
    id          text         PRIMARY KEY,                                  -- ULID
    org_id      text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    slug        text         NOT NULL,
    name        text         NOT NULL,
    description text,
    archived_at timestamptz,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT projects_org_slug_uniq UNIQUE (org_id, slug)
);
CREATE INDEX projects_org_active_idx
    ON org.projects (org_id)
    WHERE archived_at IS NULL;

-- Project-level role override. Effective role = max(org role, project role).
CREATE TABLE org.project_members (
    id         text         PRIMARY KEY,                                   -- ULID
    project_id text         NOT NULL REFERENCES org.projects(id) ON DELETE CASCADE,
    user_id    text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    role       text         NOT NULL,                                      -- 'owner' | 'admin' | 'member' | 'viewer'
    created_at timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT project_members_role_chk
        CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    CONSTRAINT project_members_project_user_uniq
        UNIQUE (project_id, user_id)
);
CREATE INDEX project_members_user_idx ON org.project_members (user_id);
```

#### 9.4.3 Schema `workitems`

```sql
CREATE SCHEMA IF NOT EXISTS workitems;

-- Milestones are recursive (PRD §6.3). Self-FK + scope FKs.
CREATE TABLE workitems.milestones (
    id                  text         PRIMARY KEY,                          -- ULID
    parent_milestone_id text         REFERENCES workitems.milestones(id) ON DELETE SET NULL,
    org_id              text         REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id          text         REFERENCES org.projects(id) ON DELETE CASCADE,
    name                text         NOT NULL,
    description         text,
    start_date          date         NOT NULL,
    end_date            date         NOT NULL,
    cancelled_at        timestamptz,
    cancelled_reason    text,
    created_at          timestamptz  NOT NULL DEFAULT now(),
    updated_at          timestamptz  NOT NULL DEFAULT now(),
    -- M-INV-1: no self-loop
    CONSTRAINT milestones_no_self_loop_chk
        CHECK (parent_milestone_id IS NULL OR parent_milestone_id <> id),
    -- Scope is XOR: org-wide OR project-local, never both, never neither.
    CONSTRAINT milestones_scope_xor_chk
        CHECK ((org_id IS NOT NULL AND project_id IS NULL)
            OR (org_id IS NULL AND project_id IS NOT NULL)),
    -- Date sanity
    CONSTRAINT milestones_date_range_chk
        CHECK (end_date >= start_date)
);
CREATE INDEX milestones_parent_idx       ON workitems.milestones (parent_milestone_id);
CREATE INDEX milestones_org_idx          ON workitems.milestones (org_id);
CREATE INDEX milestones_project_idx      ON workitems.milestones (project_id);
CREATE INDEX milestones_active_idx
    ON workitems.milestones (project_id, start_date)
    WHERE cancelled_at IS NULL;
-- Note: M-INV-2 (cycle prevention), M-INV-3 (range containment), M-INV-5 (scope match),
-- M-INV-6 (max depth = 4), M-INV-7 (item-milestone scope reachability) are enforced
-- in app code via recursive CTE checks at insert/update time. See §9.4.9 for the CTE.

-- Work items. The single most-touched table in the product.
CREATE TABLE workitems.items (
    id                   text         PRIMARY KEY,                         -- ULID
    org_id               text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id           text         REFERENCES org.projects(id) ON DELETE CASCADE,
    milestone_id         text         REFERENCES workitems.milestones(id) ON DELETE SET NULL,
    parent_id            text         REFERENCES workitems.items(id) ON DELETE SET NULL,
    discovered_from_id   text         REFERENCES workitems.items(id) ON DELETE SET NULL,
    type                 text         NOT NULL DEFAULT 'task',             -- 'epic' | 'task' | 'finding'
    title                text         NOT NULL,
    body                 text         NOT NULL DEFAULT '',
    -- §6.1 enums
    status               text         NOT NULL DEFAULT 'Backlog',          -- 'Backlog' | 'Ready' | 'InProgress' | 'Blocked' | 'Done'
    priority             text         NOT NULL DEFAULT 'P3',               -- 'P0'..'P4'
    pipeline_stage       text         NOT NULL DEFAULT 'Investigation',    -- §6.1 PipelineStage values; SUBSCRIBER-MAINTAINED — derived from impl/review/qa/pipeline_state per §5.7.1; do not write directly outside the cascade subscriber
    agent_kind           text,                                             -- §6.1 AgentKind values; nullable until claim
    -- §6.2 three orthogonal dimensions
    impl_state           text         NOT NULL DEFAULT 'pending',          -- 'pending' | 'done'
    review_state         text         NOT NULL DEFAULT 'pending',          -- 'pending' | 'approved' | 'needs_rework'
    qa_state             text         NOT NULL DEFAULT 'pending',          -- 'pending' | 'passed' | 'failed'
    pipeline_state       text         NOT NULL DEFAULT 'running',          -- 'running' | 'needs_human' | 'paused' | 'no_investigation'
    -- §6.6 finding fields
    severity             text,                                             -- only meaningful when type='finding'
    kind_of_finding      text,                                             -- 'review' | 'qa'; only when type='finding'
    -- Claim
    claimed_by_id        text         REFERENCES auth.users(id) ON DELETE SET NULL,
    claimed_by_agent     text,                                             -- AgentKind value of the claimer
    claimed_at           timestamptz,
    -- Cascade-materialised readiness (NOT a generated column; updated by deps subscriber)
    is_ready             boolean      NOT NULL DEFAULT false,
    -- Milestone audit (collapsed from the deleted item_milestone junction)
    milestone_assigned_at timestamptz,
    milestone_assigned_by text         REFERENCES auth.users(id) ON DELETE SET NULL,
    -- Lifecycle
    created_at           timestamptz  NOT NULL DEFAULT now(),
    updated_at           timestamptz  NOT NULL DEFAULT now(),
    closed_at            timestamptz,
    -- Constraints
    CONSTRAINT items_no_self_parent_chk
        CHECK (parent_id IS NULL OR parent_id <> id),
    CONSTRAINT items_no_self_discovery_chk
        CHECK (discovered_from_id IS NULL OR discovered_from_id <> id),
    CONSTRAINT items_type_chk
        CHECK (type IN ('epic', 'task', 'finding')),
    CONSTRAINT items_status_chk
        CHECK (status IN ('Backlog', 'Ready', 'InProgress', 'Blocked', 'Done')),
    CONSTRAINT items_priority_chk
        CHECK (priority IN ('P0', 'P1', 'P2', 'P3', 'P4')),
    CONSTRAINT items_pipeline_stage_chk
        CHECK (pipeline_stage IN ('Investigation', 'Implementation', 'Review', 'Quality', 'Deferred', 'Done')),
    CONSTRAINT items_agent_kind_chk
        CHECK (agent_kind IS NULL OR agent_kind IN ('claude-code', 'copilot', 'cursor', 'codex', 'aider', 'custom')),
    CONSTRAINT items_impl_state_chk
        CHECK (impl_state IN ('pending', 'done')),
    CONSTRAINT items_review_state_chk
        CHECK (review_state IN ('pending', 'approved', 'needs_rework')),
    CONSTRAINT items_qa_state_chk
        CHECK (qa_state IN ('pending', 'passed', 'failed')),
    CONSTRAINT items_pipeline_state_chk
        CHECK (pipeline_state IN ('running', 'needs_human', 'paused', 'no_investigation')),
    CONSTRAINT items_severity_chk
        CHECK (severity IS NULL
            OR severity IN ('critical', 'major', 'minor', 'risk', 'extra', 'deviation')),
    CONSTRAINT items_kind_of_finding_chk
        CHECK (kind_of_finding IS NULL OR kind_of_finding IN ('review', 'qa')),
    -- Findings must declare severity + originating bead + a parent epic.
    -- PRD §6.6 promises findings live "under the parent epic" — parent_id
    -- is required, and the service layer asserts the parent's type='epic'
    -- (deferred check; not expressible as a single-row CHECK).
    CONSTRAINT items_finding_required_fields_chk
        CHECK (
            (type <> 'finding')
            OR (severity IS NOT NULL
                AND kind_of_finding IS NOT NULL
                AND discovered_from_id IS NOT NULL
                AND parent_id IS NOT NULL)
        ),
    -- A claim implies status InProgress or Done. The (claimed_by_id NULL,
    -- status Done) combination is also legal: closing an item via the
    -- override path or via cascade promotion may finalise an item that was
    -- never claimed. Conversely, once claimed, an item's claim audit is
    -- preserved through close — `claimed_by_id` and `claimed_at` are NOT
    -- nulled on close. (Research AF3: the asymmetry is intentional; close
    -- preserves the claim history so the audit trail of who completed the
    -- work survives indefinitely.)
    CONSTRAINT items_claim_status_chk
        CHECK (
            (claimed_by_id IS NULL AND claimed_at IS NULL)
            OR (claimed_by_id IS NOT NULL AND claimed_at IS NOT NULL AND status IN ('InProgress', 'Done'))
        )
);
-- Hot-path indexes
CREATE INDEX items_org_status_idx        ON workitems.items (org_id, status);
CREATE INDEX items_project_status_idx    ON workitems.items (project_id, status);
CREATE INDEX items_milestone_idx         ON workitems.items (milestone_id);
CREATE INDEX items_parent_idx            ON workitems.items (parent_id);
CREATE INDEX items_discovered_from_idx   ON workitems.items (discovered_from_id);
CREATE INDEX items_claimed_by_idx        ON workitems.items (claimed_by_id);
-- Partial index: the `ready` MCP tool's hot path. p99 < 2 s depends on this.
CREATE INDEX items_ready_partial_idx
    ON workitems.items (org_id, project_id, priority)
    WHERE is_ready = true AND status = 'Ready' AND closed_at IS NULL;
-- Full-text search backing the `search` MCP tool (research AF1). Generated
-- column over title + body; GIN-indexed. Multi-table search at query time
-- uses UNION ALL across this index and `comments_fts_idx` (PG GIN cannot
-- span two tables); per-row trigram fallback covers fuzzy match.
ALTER TABLE workitems.items ADD COLUMN fts tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(body,  '')), 'B')
    ) STORED;
CREATE INDEX items_fts_idx ON workitems.items USING GIN (fts);
-- Partial index: in-progress board view.
CREATE INDEX items_in_progress_idx
    ON workitems.items (org_id, project_id, claimed_at DESC)
    WHERE status = 'InProgress';

-- User-facing labels (PRD §6.4). Scope is org XOR project.
CREATE TABLE workitems.labels (
    id          text         PRIMARY KEY,                                  -- ULID
    org_id      text         REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id  text         REFERENCES org.projects(id) ON DELETE CASCADE,
    name        text         NOT NULL,
    color       text         NOT NULL,                                     -- hex, '#rrggbb'
    description text,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),                        -- bumped on every UpdateLabel write
                                                                            -- (P01 round-16, bead unblock-tv8.75; drift closed
                                                                            -- 2026-06-11 — the phase-spec Label struct always
                                                                            -- declared this column but the original
                                                                            -- 0040_workitems.up.sql omitted it; added by migration
                                                                            -- 0130_workitems_labels_updated_at. The registry is
                                                                            -- mutable via MCP Tool 22 update_label, and items /
                                                                            -- milestones / comments all carry updated_at, so the
                                                                            -- column is ADDED (Miguel's decision) rather than
                                                                            -- dropping the struct field.)
    CONSTRAINT labels_scope_xor_chk
        CHECK ((org_id IS NOT NULL AND project_id IS NULL)
            OR (org_id IS NULL AND project_id IS NOT NULL)),
    CONSTRAINT labels_color_chk
        CHECK (color ~ '^#[0-9a-fA-F]{6}$')
);
CREATE UNIQUE INDEX labels_org_name_uniq
    ON workitems.labels (org_id, lower(name))
    WHERE org_id IS NOT NULL;
CREATE UNIQUE INDEX labels_project_name_uniq
    ON workitems.labels (project_id, lower(name))
    WHERE project_id IS NOT NULL;

CREATE TABLE workitems.item_labels (
    item_id    text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    label_id   text         NOT NULL REFERENCES workitems.labels(id) ON DELETE CASCADE,
    applied_at timestamptz  NOT NULL DEFAULT now(),
    applied_by text         REFERENCES auth.users(id) ON DELETE SET NULL,
    PRIMARY KEY (item_id, label_id)
);
CREATE INDEX item_labels_label_idx ON workitems.item_labels (label_id);

-- Item ↔ milestone is 1:1 (per PRD §6.3 "exactly one milestone"). Membership
-- is represented as the `milestone_id` column on `workitems.items` plus the
-- audit fields `milestone_assigned_at` / `milestone_assigned_by` on the
-- same row (added to the items DDL above). No junction table — the
-- earlier draft's parallel `item_milestone` table was redundant with the
-- column and the two paths had asymmetric ON DELETE policies (SET NULL vs
-- CASCADE) that risked drift. Single source of truth on the items row.
-- (`items_milestone_idx` on `workitems.items (milestone_id)` is declared
-- once with the items table indexes above; do not redeclare here.)

-- Comments (PRD §6.5). Append-only, (kind, status) orthogonal axes.
CREATE TABLE workitems.comments (
    id         text         PRIMARY KEY,                                   -- ULID
    item_id    text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    parent_id  text         REFERENCES workitems.comments(id) ON DELETE SET NULL,
    author_id  text         REFERENCES auth.users(id) ON DELETE SET NULL,
    author_agent text,                                                     -- AgentKind value if author is an agent
    kind       text         NOT NULL,                                      -- PRD §6.5 kind list
    status     text         NOT NULL DEFAULT 'info',                       -- 'error' | 'warning' | 'info' | 'success'
    body       text         NOT NULL,
    created_at timestamptz  NOT NULL DEFAULT now(),
    updated_at timestamptz  NOT NULL DEFAULT now(),                            -- bumped on body edit; PRD FR-10
    CONSTRAINT comments_no_self_parent_chk
        CHECK (parent_id IS NULL OR parent_id <> id),
    CONSTRAINT comments_kind_chk
        CHECK (kind IN ('investigation', 'decision', 'deviation', 'completed',
                        'review', 'qa', 'deferred', 'pr', 'needs-human',
                        'override', 'general')),
    CONSTRAINT comments_status_chk
        CHECK (status IN ('error', 'warning', 'info', 'success')),
    CONSTRAINT comments_author_chk
        CHECK (author_id IS NOT NULL OR author_agent IS NOT NULL)
);
CREATE INDEX comments_item_created_idx ON workitems.comments (item_id, created_at);
CREATE INDEX comments_parent_idx       ON workitems.comments (parent_id);
CREATE INDEX comments_status_idx       ON workitems.comments (status);
CREATE INDEX comments_kind_status_idx  ON workitems.comments (kind, status);
-- Full-text search backing the `search` MCP tool (research AF1).
ALTER TABLE workitems.comments ADD COLUMN fts tsvector
    GENERATED ALWAYS AS (to_tsvector('english', coalesce(body, ''))) STORED;
CREATE INDEX comments_fts_idx ON workitems.comments USING GIN (fts);
```

#### 9.4.4 Schema `deps`

```sql
CREATE SCHEMA IF NOT EXISTS deps;

-- Edges between work items. "from blocks to" semantics: from must be Done
-- before to can become Ready.
CREATE TABLE deps.dependencies (
    id          text         PRIMARY KEY,                                  -- ULID
    from_item   text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    to_item     text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    kind        text         NOT NULL DEFAULT 'blocks',                    -- 'blocks' | 'related' (related not enforced for ready)
    created_at  timestamptz  NOT NULL DEFAULT now(),
    created_by  text         REFERENCES auth.users(id) ON DELETE SET NULL,
    CONSTRAINT dependencies_no_self_loop_chk
        CHECK (from_item <> to_item),
    CONSTRAINT dependencies_kind_chk
        CHECK (kind IN ('blocks', 'related')),
    CONSTRAINT dependencies_pair_uniq
        UNIQUE (from_item, to_item, kind)
);
CREATE INDEX dependencies_from_idx ON deps.dependencies (from_item);
CREATE INDEX dependencies_to_idx   ON deps.dependencies (to_item);
-- Partial index: the cycle-detection CTE only walks 'blocks' edges.
CREATE INDEX dependencies_blocks_to_idx
    ON deps.dependencies (to_item, from_item)
    WHERE kind = 'blocks';

-- Cycle audit. When the cycle-prevention CTE rejects a write, we record the
-- attempted edge for forensics. The CTE itself is in §9.4.9.
CREATE TABLE deps.cycles (
    id           text         PRIMARY KEY,                                 -- ULID
    detected_at  timestamptz  NOT NULL DEFAULT now(),
    from_item    text         NOT NULL,                                    -- not FK (the row may not exist)
    to_item      text         NOT NULL,
    cycle_path   text[]       NOT NULL,                                    -- ordered list of item ids forming the cycle
    rejected_by  text         REFERENCES auth.users(id) ON DELETE SET NULL
);
CREATE INDEX cycles_detected_idx ON deps.cycles (detected_at DESC);

-- Cascade events audit (PRD M-5 — "cascade events per day"). Every successful
-- run of the cascade subscriber (Law 1) writes one row here. The M-5 metric
-- query aggregates this table grouped by (org_id, date_trunc('day', triggered_at));
-- this decouples the metric from the observability stack so the number is
-- reproducible from Postgres alone, even after retention windows on traces
-- have rolled over. The table is also used as a forensic record when a
-- cascade affects a surprising set of items.
CREATE TABLE deps.cascade_events (
    id                    text         PRIMARY KEY,                       -- ULID (audit row id)
    -- Publisher-generated event id (ULID) carried as a typed field on the
    -- Pub/Sub message payload. Encore Go's subscriber handler signature does
    -- NOT expose envelope metadata (research C1), so the publisher embeds
    -- this id at emit time and the subscriber reads it from the payload.
    -- The (event_id, triggered_by_item_id) UNIQUE constraint below is the
    -- structural mitigation for at-least-once redelivery (AR-11).
    event_id              text         NOT NULL,
    -- Cascade kind discriminator. Four first-class values, all P01-active.
    -- The cascade subsystem maintains TWO propagation regimes; the canonical
    -- model lives in phase spec §6.3.0 ("Propagation regimes") at
    -- docs/specs/01-spec-backend-mvp.md. Regime A is writer-inline (the
    -- mutation transaction writes both the audit row and the `is_ready`
    -- flip in the same SQL transaction); Regime B is subscriber-only (the
    -- mutation publishes to Pub/Sub and the cascade subscriber materialises
    -- `is_ready` and `pipeline_stage`). Per-kind provenance:
    --   'close'        — Regime B. Written by the cascade subscriber when a
    --                    close event (Tool 6 / workitems.Close) arrives via
    --                    Pub/Sub; walks the forward 'blocks' closure
    --                    (multi-hop, possibly large affected set).
    --   'edge_added'   — Regime B. Written by the cascade subscriber when
    --                    Tool 11 (add_dependency) publishes; a new incoming
    --                    edge may flip the `to_item` out of ready, so the
    --                    subscriber re-evaluates the single-hop predicate.
    --   'edge_removed' — Regime A. Written INLINE by Tool 12
    --                    (remove_dependency) in the same SQL transaction as
    --                    DELETE FROM deps.dependencies; single-hop only (the
    --                    direct to_item is the only candidate to flip ready).
    --                    The subscriber also receives the corresponding
    --                    Pub/Sub event and its INSERT collapses via
    --                    ON CONFLICT (event_id, triggered_by_item_id) DO
    --                    NOTHING — the writer-inline row is authoritative.
    --   'state_change' — Regime B. Written by the cascade subscriber for
    --                    Pub/Sub-driven state cascades; in P01 the only
    --                    emitters are Tool 13 (review/qa state writes that
    --                    can flip `pipeline_stage` for downstream items)
    --                    and `workitems.Claim` on the I-3 reset path (a
    --                    claim that resets `impl_state` from `done` back to
    --                    `in_progress` invalidates derived `pipeline_stage`
    --                    on the same row, and the subscriber rematerialises).
    -- New cascade kinds added in future phases extend this CHECK additively
    -- via their own phase migration and update phase spec §6.3.0.
    kind                  text         NOT NULL,
    org_id                text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id            text         REFERENCES org.projects(id) ON DELETE SET NULL,
    triggered_by_item_id  text         REFERENCES workitems.items(id) ON DELETE SET NULL,
    -- The full set of items whose `is_ready` flipped to true as a result.
    -- Stored as text[] (item ULIDs). For M-5 the cardinality matters more
    -- than the membership; cardinality is denormalised in `cascaded_count`
    -- so the metric query never needs to inspect the array.
    affected_item_ids     text[]       NOT NULL DEFAULT '{}',
    cascaded_count        integer      NOT NULL DEFAULT 0,
    triggered_at          timestamptz  NOT NULL DEFAULT now(),
    trace_id              text,                                            -- correlates with mcp.tool_calls.trace_id
    CONSTRAINT cascade_events_kind_chk
        CHECK (kind IN ('close', 'edge_added', 'edge_removed', 'state_change')),
    CONSTRAINT cascade_events_count_chk
        CHECK (cascaded_count >= 0),
    -- AR-11 idempotency key. A redelivered Pub/Sub message carries the same
    -- payload bytes (including event_id), so the second insert is rejected
    -- by this constraint and the subscriber's UPDATE pass is a no-op on a
    -- stable graph.
    CONSTRAINT cascade_events_event_trigger_uniq
        UNIQUE (event_id, triggered_by_item_id)
);
-- Hot-path index for the M-5 metric query (per-org, by day).
CREATE INDEX cascade_events_org_triggered_idx
    ON deps.cascade_events (org_id, triggered_at DESC);
CREATE INDEX cascade_events_project_idx
    ON deps.cascade_events (project_id, triggered_at DESC)
    WHERE project_id IS NOT NULL;
-- Partial index: cascades that actually moved the graph (cascaded_count > 0).
-- M-5's "non-zero on the median active org" target reads through this index.
CREATE INDEX cascade_events_nonzero_idx
    ON deps.cascade_events (org_id, triggered_at DESC)
    WHERE cascaded_count > 0;
```

#### 9.4.5 Schema `providers`

```sql
CREATE SCHEMA IF NOT EXISTS providers;

CREATE TABLE providers.installations (
    id                  text         PRIMARY KEY,                          -- ULID
    org_id              text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id          text         REFERENCES org.projects(id) ON DELETE CASCADE,
    provider            text         NOT NULL,                             -- 'github' | 'gitlab'
    provider_account    text         NOT NULL,                             -- e.g. 'websublime'
    provider_repo       text,                                              -- nullable for org-level installs
    installation_id_enc bytea        NOT NULL,                             -- pgp_sym_encrypt(provider_installation_id)
    webhook_secret_enc  bytea        NOT NULL,                             -- per-install HMAC secret
    installed_by        text         REFERENCES auth.users(id) ON DELETE SET NULL,
    installed_at        timestamptz  NOT NULL DEFAULT now(),
    revoked_at          timestamptz,
    CONSTRAINT installations_provider_chk
        CHECK (provider IN ('github', 'gitlab')),
    CONSTRAINT installations_target_uniq
        UNIQUE (provider, provider_account, provider_repo)
);
CREATE INDEX installations_org_idx ON providers.installations (org_id);

-- Webhook events. Audit + dedup. Idempotency on (provider, delivery_id).
CREATE TABLE providers.events (
    id              text         PRIMARY KEY,                              -- ULID (our id, not provider's)
    installation_id text         NOT NULL REFERENCES providers.installations(id) ON DELETE CASCADE,
    provider        text         NOT NULL,                                 -- denormalised for hot lookup
    delivery_id     text         NOT NULL,                                 -- e.g. X-GitHub-Delivery
    event_type      text         NOT NULL,                                 -- e.g. 'issues.opened'
    payload         jsonb        NOT NULL,
    signature_ok    boolean      NOT NULL,
    received_at     timestamptz  NOT NULL DEFAULT now(),
    processed_at    timestamptz,
    error           text,
    CONSTRAINT events_provider_chk
        CHECK (provider IN ('github', 'gitlab')),
    CONSTRAINT events_delivery_uniq
        UNIQUE (provider, delivery_id)
);
CREATE INDEX events_installation_received_idx
    ON providers.events (installation_id, received_at DESC);
CREATE INDEX events_unprocessed_idx
    ON providers.events (received_at)
    WHERE processed_at IS NULL;
CREATE INDEX events_payload_gin_idx ON providers.events USING gin (payload);

-- PII retention policy on providers.events.payload (referenced from AR-13/14):
-- raw webhook payloads can carry user emails, repo metadata, and OAuth-related
-- fields. The retention policy is:
--   * Raw payload retained 90 days from `received_at`.
--   * After 90 days a scheduled job (Encore cron, runs daily) replaces the
--     payload with a metadata-only digest:
--         { "event_type": <string>, "actor_login": <hash>, "repo": <hash>,
--           "delivery_id": <string>, "digest_at": <ts> }
--     The digest preserves the audit's debugging value (we still know which
--     event type came from which installation when) without retaining
--     identifying free-text payload fields.
--   * Email addresses and any matched credential patterns are redacted
--     **on insert** by a per-row sanitiser running before the row is
--     committed. The 90-day truncation is the second layer; the sanitiser
--     is the first.
-- The exact digest schema and the redactor pattern set land in the P02 spec.
-- A test asserts that a payload older than 90 days has been digested.

-- Provider ↔ work item mapping. Bidirectional sync key.
CREATE TABLE providers.mappings (
    id              text         PRIMARY KEY,                              -- ULID
    installation_id text         NOT NULL REFERENCES providers.installations(id) ON DELETE CASCADE,
    item_id         text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    provider        text         NOT NULL,
    provider_kind   text         NOT NULL,                                 -- 'issue' | 'pull_request'
    provider_id     text         NOT NULL,                                 -- provider-side id (string for portability)
    provider_url    text         NOT NULL,
    last_synced_at  timestamptz,
    drift_detected_at timestamptz,
    CONSTRAINT mappings_provider_chk
        CHECK (provider IN ('github', 'gitlab')),
    CONSTRAINT mappings_kind_chk
        CHECK (provider_kind IN ('issue', 'pull_request')),
    CONSTRAINT mappings_external_uniq
        UNIQUE (provider, provider_kind, provider_id),
    CONSTRAINT mappings_item_provider_uniq
        UNIQUE (item_id, provider, provider_kind)
);
CREATE INDEX mappings_item_idx          ON providers.mappings (item_id);
CREATE INDEX mappings_installation_idx  ON providers.mappings (installation_id);
CREATE INDEX mappings_drift_idx
    ON providers.mappings (drift_detected_at)
    WHERE drift_detected_at IS NOT NULL;
```

#### 9.4.6 Schema `mcp`

> **Migration / DB-handle ownership note (research C2).** The `mcp` service
> does **not** own the migration files for its schema. Per §5.2, **all eight
> schemas' migrations live under `apps/api/auth/migrations/`** because
> Encore's database primitive is per-service and only one service can own
> the migrations directory for a DB. The `mcp` service obtains its DB
> handle by calling `sqldb.Named("unblock")` (the database name `auth`
> registered in §5.2's migration-owner discussion) and writes only to the
> `mcp.*` tables defined below. Writes to `mcp.api_keys.expires_at` and
> `mcp.api_keys.revoked_at` etc. are mediated by the `pkg/rbac` typed query
> helper (§5.6); cross-schema writes from `mcp` to (e.g.) `workitems.*`
> are compile-time rejected. The same share pattern applies to every other
> service in §9.4.2 — §9.4.5 and §9.4.7 — §9.4.8.

```sql
CREATE SCHEMA IF NOT EXISTS mcp;

-- Per-agent API keys. The hot-path Bearer auth check.
CREATE TABLE mcp.api_keys (
    id              text         PRIMARY KEY,                              -- ULID
    org_id          text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    issued_to_user  text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
                                                                           -- REQUIRED: every MCP API key is owned by exactly one
                                                                           -- user. Deleting that user deletes their keys
                                                                           -- (ON DELETE CASCADE). Tool-call audit history
                                                                           -- survives the deletion because mcp.tool_calls.api_key_id
                                                                           -- is ON DELETE SET NULL, not CASCADE.
                                                                           -- (P01 round-16, bead unblock-tv8.73; drift closed
                                                                           -- 2026-06-11 — was nullable / ON DELETE SET NULL,
                                                                           -- tightened by migration 0120_mcp_issued_to_user_notnull.)
    label           text         NOT NULL,                                 -- e.g. 'claude-code-laptop'
    agent_kind      text         NOT NULL,                                 -- AgentKind value
    key_hash        bytea        NOT NULL,                                 -- HMAC-SHA256(server_secret, key); 32 bytes raw.
                                                                           -- Argon2id is the wrong primitive here: keys are
                                                                           -- 32-byte URL-safe random with 256 bits of entropy,
                                                                           -- so brute force is mathematically infeasible
                                                                           -- regardless of hash speed; argon2id's per-call
                                                                           -- ~50ms cost would directly threaten NFR-1 (the
                                                                           -- p99 < 2s budget is hot-path Bearer auth).
                                                                           -- HMAC with a server-side secret prevents lookup
                                                                           -- table attacks against a leaked key_hash dump
                                                                           -- (research C7).
    key_prefix      text         NOT NULL,                                 -- first 8 chars for hint UI
    scopes          text[]       NOT NULL DEFAULT '{}',                    -- coarse scopes; tool-level RBAC is in mcp service
    created_at      timestamptz  NOT NULL DEFAULT now(),
    last_used_at    timestamptz,
    -- Optional natural expiry. NULL means "never expires by default" — at
    -- v1 we do not auto-rotate API keys. Lifecycle is operator-driven:
    -- (a) issuance happens via `auth.IssueAPIKey` — called from test seeds
    --     via direct INSERT in P01 (round-12 — see
    --     docs/specs/01-spec-backend-mvp.md §11.1.1; the E2E test
    --     `apps/api/exitcriteriontest/` writes the row straight to
    --     mcp.api_keys with `key_hash` computed via
    --     `secrets.APIKeyHMACSecret` per
    --     apps/api/auth/apikey.go:103-111). Operator-facing surfaces
    --     (CLI / web admin) ship in a future phase (P05+).
    -- (b) Rotation is a manual two-step: issue a new key (new prefix), wait
    --     for the agent operator to switch over, then set `revoked_at` on
    --     the old row. Both rows coexist during the rollover window.
    -- (c) There is no auto-refresh, no auto-rotate, and no key-expiry
    --     scheduler in v1. `expires_at` is honoured if set (auth.Validate
    --     refuses keys past `expires_at`) but the column defaults to NULL
    --     and operators rarely set it. See research AF4.
    expires_at      timestamptz,
    revoked_at      timestamptz,
    CONSTRAINT api_keys_agent_chk
        CHECK (agent_kind IN ('claude-code', 'copilot', 'cursor', 'codex', 'aider', 'custom')),
    CONSTRAINT api_keys_prefix_uniq UNIQUE (key_prefix)
);
CREATE INDEX api_keys_org_active_idx
    ON mcp.api_keys (org_id, last_used_at DESC NULLS LAST)
    WHERE revoked_at IS NULL;

-- Tool-call audit. Every MCP call is recorded for forensics + state-machine
-- rejections analysis (Layer 1, FR-9).
CREATE TABLE mcp.tool_calls (
    id           text         PRIMARY KEY,                                 -- ULID
    api_key_id   text         REFERENCES mcp.api_keys(id) ON DELETE SET NULL,
    org_id       text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id   text         REFERENCES org.projects(id) ON DELETE SET NULL,
    item_id      text         REFERENCES workitems.items(id) ON DELETE SET NULL,
    tool_name    text         NOT NULL,
    arguments    jsonb        NOT NULL DEFAULT '{}'::jsonb,
    result_kind  text         NOT NULL,                                    -- 'ok' | 'rejected' | 'error'
    rejection_reason text,                                                 -- precondition name when result_kind='rejected'
    error_code   text,
    duration_ms  integer      NOT NULL,
    trace_id     text,
    called_at    timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT tool_calls_result_chk
        CHECK (result_kind IN ('ok', 'rejected', 'error'))
);
CREATE INDEX tool_calls_org_called_idx     ON mcp.tool_calls (org_id, called_at DESC);
CREATE INDEX tool_calls_item_idx           ON mcp.tool_calls (item_id);
CREATE INDEX tool_calls_rejected_idx
    ON mcp.tool_calls (org_id, called_at DESC)
    WHERE result_kind = 'rejected';
CREATE INDEX tool_calls_arguments_gin_idx  ON mcp.tool_calls USING gin (arguments);
```

#### 9.4.7 Schema `boards`

```sql
CREATE SCHEMA IF NOT EXISTS boards;

CREATE TABLE boards.boards (
    id          text         PRIMARY KEY,                                  -- ULID
    org_id      text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id  text         REFERENCES org.projects(id) ON DELETE CASCADE,
    user_id     text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE, -- saved per user
    name        text         NOT NULL,
    description text,
    filters     jsonb        NOT NULL DEFAULT '{}'::jsonb,                 -- saved filter state (status, label, milestone, etc.)
    layout      text         NOT NULL DEFAULT 'kanban',                    -- 'kanban' | 'list' | 'graph' | 'roadmap'
    is_default  boolean      NOT NULL DEFAULT false,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT boards_layout_chk
        CHECK (layout IN ('kanban', 'list', 'graph', 'roadmap'))
);
CREATE INDEX boards_org_user_idx ON boards.boards (org_id, user_id);
-- Only one default per (user, project) — partial unique.
CREATE UNIQUE INDEX boards_default_per_user_project_uniq
    ON boards.boards (user_id, COALESCE(project_id, ''))
    WHERE is_default = true;

-- Per-board column configuration (kanban). Columns are user-defined groupings;
-- each column has a filter (e.g. by status, by label).
CREATE TABLE boards.columns (
    id           text         PRIMARY KEY,                                 -- ULID
    board_id     text         NOT NULL REFERENCES boards.boards(id) ON DELETE CASCADE,
    name         text         NOT NULL,
    filter       jsonb        NOT NULL DEFAULT '{}'::jsonb,
    position     integer      NOT NULL,
    wip_limit    integer,                                                  -- nullable; null = unlimited
    color        text,
    created_at   timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT columns_position_chk CHECK (position >= 0),
    CONSTRAINT columns_wip_chk      CHECK (wip_limit IS NULL OR wip_limit > 0),
    CONSTRAINT columns_color_chk    CHECK (color IS NULL OR color ~ '^#[0-9a-fA-F]{6}$'),
    CONSTRAINT columns_board_position_uniq UNIQUE (board_id, position)
);
CREATE INDEX columns_board_idx ON boards.columns (board_id, position);
```

#### 9.4.8 Schema `memory`

```sql
CREATE SCHEMA IF NOT EXISTS memory;

-- Scoped memory entries (PRD §6 / FR-13). Three scope kinds.
-- Value is encrypted at rest (NFR-7 sanitisation runs *before* encryption,
-- so the plaintext stored is already sanitised; the *_enc column contains
-- the sanitised plaintext encrypted with pgcrypto).
CREATE TABLE memory.entries (
    id          text         PRIMARY KEY,                                  -- ULID
    scope       text         NOT NULL,                                     -- 'org' | 'project' | 'user'
    org_id      text         REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id  text         REFERENCES org.projects(id) ON DELETE CASCADE,
    user_id     text         REFERENCES auth.users(id) ON DELETE CASCADE,
    author_id   text         REFERENCES auth.users(id) ON DELETE SET NULL,
    author_agent text,                                                     -- AgentKind if written by an agent
    key         text         NOT NULL,                                     -- short label / canonical fact name
    value_enc   bytea        NOT NULL,                                     -- pgp_sym_encrypt(sanitised_plaintext)
    value_size  integer      NOT NULL,                                     -- size of plaintext, bytes; ≤ 8192
    tags        text[]       NOT NULL DEFAULT '{}',
    -- tsvector built over the *plaintext* before encryption, stored alongside
    -- to enable indexed full-text search without decrypt-on-search. The
    -- ts_doc column holds *only* a tokenised, lowercased projection — not the
    -- full plaintext — so its leakage surface is bounded to indexable terms.
    ts_doc      tsvector     NOT NULL,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),
    expires_at  timestamptz,
    CONSTRAINT entries_scope_chk
        CHECK (scope IN ('org', 'project', 'user')),
    -- Scope discriminator: exactly the right scope id is set.
    CONSTRAINT entries_scope_target_chk CHECK (
        (scope = 'org'     AND org_id IS NOT NULL AND project_id IS NULL  AND user_id IS NULL)
     OR (scope = 'project' AND project_id IS NOT NULL AND org_id IS NULL  AND user_id IS NULL)
     OR (scope = 'user'    AND user_id IS NOT NULL AND org_id IS NULL     AND project_id IS NULL)
    ),
    CONSTRAINT entries_size_chk CHECK (value_size > 0 AND value_size <= 8192),
    -- Uniqueness per (scope target, key) — partial unique indexes below
    CONSTRAINT entries_author_chk
        CHECK (author_id IS NOT NULL OR author_agent IS NOT NULL)
);
-- Per-scope uniqueness on key
CREATE UNIQUE INDEX entries_org_key_uniq
    ON memory.entries (org_id, key)
    WHERE scope = 'org';
CREATE UNIQUE INDEX entries_project_key_uniq
    ON memory.entries (project_id, key)
    WHERE scope = 'project';
CREATE UNIQUE INDEX entries_user_key_uniq
    ON memory.entries (user_id, key)
    WHERE scope = 'user';
-- FTS index
CREATE INDEX entries_ts_doc_gin_idx ON memory.entries USING gin (ts_doc);
-- Tag index (GIN on text[])
CREATE INDEX entries_tags_gin_idx   ON memory.entries USING gin (tags);
-- Trigram index on key for fuzzy lookups (uses pg_trgm extension)
CREATE INDEX entries_key_trgm_idx   ON memory.entries USING gin (key gin_trgm_ops);

-- Cross-references: a memory entry can reference work items, comments, PRs,
-- milestones, or be flagged as a general scope-level fact.
-- Modeled as a polymorphic junction with a kind discriminator.
CREATE TABLE memory.entry_refs (
    id         text         PRIMARY KEY,                                   -- ULID
    entry_id   text         NOT NULL REFERENCES memory.entries(id) ON DELETE CASCADE,
    ref_kind   text         NOT NULL,                                      -- 'workitem' | 'comment' | 'pr' | 'milestone' | 'general'
    ref_id     text         NOT NULL,                                      -- target id (no FK due to polymorphism;
                                                                           -- referential integrity by service layer).
                                                                           -- For ref_kind='general': ref_id holds the
                                                                           -- parent entry's scope_id — i.e., the memory
                                                                           -- is a general fact about its org/project/user
                                                                           -- scope, not pinned to any specific entity.
    created_at timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT entry_refs_kind_chk
        CHECK (ref_kind IN ('workitem', 'comment', 'pr', 'milestone', 'general')),
    CONSTRAINT entry_refs_unique UNIQUE (entry_id, ref_kind, ref_id)
);
CREATE INDEX entry_refs_entry_idx ON memory.entry_refs (entry_id);
CREATE INDEX entry_refs_target_idx ON memory.entry_refs (ref_kind, ref_id);
```

#### 9.4.9 Recursive CTE patterns the architecture relies on

These CTEs are referenced by multiple services. They are documented here so
phase specs do not re-derive them.

**Milestone tree walk** (PRD §6.3 — "all items in Q1"):

```sql
WITH RECURSIVE descendants(id) AS (
    SELECT id FROM workitems.milestones WHERE id = $1
    UNION ALL
    SELECT m.id
      FROM workitems.milestones m
      JOIN descendants d ON m.parent_milestone_id = d.id
)
SELECT i.*
  FROM workitems.items i
  JOIN descendants d ON i.milestone_id = d.id;
```

Bound by **M-INV-6 (max depth = 4)**, so the CTE walks at most 4 levels.

**Cycle prevention on dependency insert** (NFR-5 — reject cycles at write
time):

```sql
WITH RECURSIVE reachable(id, depth) AS (
    SELECT $2::text, 0                                            -- the proposed to_item
    UNION ALL
    SELECT d.to_item, r.depth + 1
      FROM deps.dependencies d
      JOIN reachable r ON d.from_item = r.id
      WHERE d.kind = 'blocks'
        AND r.depth < 256                                         -- hard cap on traversal depth (AR-8)
)
SELECT 1 FROM reachable WHERE id = $1                             -- the proposed from_item
LIMIT 1;
```

If this returns a row, inserting `(from=$1, to=$2, blocks)` would create a
cycle.

**Concurrency-safe write protocol for `add_dependency`** (research AF5 —
`SELECT FOR UPDATE` does not block racing INSERTs of brand-new edges, only
touched existing rows; an advisory lock is required to serialise concurrent
edge writes that share a project):

```sql
-- 1. Acquire a per-project advisory lock for the duration of the transaction
SELECT pg_advisory_xact_lock(hashtext('deps.add_dependency:' || $project_id));
-- 2. Run the depth-bounded reachability CTE above
-- 3. If empty result: INSERT INTO deps.dependencies (...)
-- 4. Transaction commits; advisory lock is released automatically
```

The advisory lock scope is the project (deps live inside a project), so
cross-project edge writes do not contend with each other.

**Why a depth counter instead of `LIMIT` in the recursive term:** Postgres
documents `LIMIT` as a parent-query construct; using it inside a recursive
term has undocumented semantics and the PG manual itself notes the trick
"is not recommended" for production (research C5). The depth counter
pattern is the standard approach: terminates the recursive term once
depth exceeds 256, returns a structured error pointing at the offending
edge. See AR-8 in §13.

**Closure for `is_ready` materialisation** (Law 1 cascade):

```sql
-- For a given item id, true iff every 'blocks' predecessor is closed.
SELECT NOT EXISTS (
    SELECT 1
      FROM deps.dependencies d
      JOIN workitems.items i ON i.id = d.from_item
     WHERE d.to_item = $1
       AND d.kind = 'blocks'
       AND i.status <> 'Done'
) AS is_ready;
```

The cascade subscriber recomputes this for every transitively-affected item
and updates `workitems.items.is_ready` accordingly.

**Comment thread reconstruction**:

```sql
WITH RECURSIVE thread(id, parent_id, body, status, kind, created_at, depth) AS (
    SELECT id, parent_id, body, status, kind, created_at, 0
      FROM workitems.comments
     WHERE item_id = $1 AND parent_id IS NULL
    UNION ALL
    SELECT c.id, c.parent_id, c.body, c.status, c.kind, c.created_at, t.depth + 1
      FROM workitems.comments c
      JOIN thread t ON c.parent_id = t.id
)
SELECT * FROM thread
 ORDER BY created_at;
```

A natural soft cap of `depth ≤ 32` is enforced in the service layer (no
practical comment thread is deeper).

#### 9.4.10 pgcrypto encryption pattern (`*_enc` columns)

Encryption uses pgcrypto's `pgp_sym_encrypt`/`pgp_sym_decrypt` with a single
data encryption key (DEK) supplied via the Encore secret `MEMORY_DEK`
(naming kept generic; the same DEK is used for `auth.oauth_tokens.*_enc`,
`providers.installations.*_enc`, and `memory.entries.value_enc`).

**Write path:**
```sql
INSERT INTO memory.entries (..., value_enc, ...)
VALUES (..., pgp_sym_encrypt($plaintext, $dek, 'cipher-algo=aes256, compress-algo=2'), ...);
```

**Read path** (decrypt happens at the service layer **after** authorisation):
```sql
SELECT pgp_sym_decrypt(value_enc, $dek)::text AS value
  FROM memory.entries
 WHERE id = $1;
```

The DEK never leaves the Encore service mesh; the database connection user
does not have shell access; the secret is supplied via Encore Cloud's secret
manager.

**P01 provisioning (research OQ2).** `MEMORY_DEK` must be provisioned in
P01, even though the `memory` service has no runtime code until P02. The
P01 schemas `auth.oauth_tokens.access_token_enc` /
`auth.oauth_tokens.refresh_token_enc` and
`providers.installations.installation_id_enc` /
`providers.installations.webhook_secret_enc` (the latter is a P02 service
but the schema lands in P01 per the plan §2.1 "schema-only services" rule)
all use the same DEK; the bootstrap migration cannot succeed without a
provisioned secret. Olive owns the Encore secret-manager seeding as part
of A-2 in the P01 plan; the local emulator uses a development DEK from
`apps/api/.secrets.local.cue` (CUE format, per Encore official docs at
https://encore.dev/docs/go/primitives/secrets — the file lives at the
Encore app root next to `encore.app` and must be gitignored). The
spec-level logical name `MEMORY_DEK` maps to the Encore Go manifest
field `MemoryDEK`; both the secret-manager key and the CUE-file key
must use the Go field name verbatim. Per-phase specs may pin additional
secrets — see `docs/specs/01-spec-backend-mvp.md` §3.5 for the full P01
mapping table.

**Key rotation strategy:**
1. Provision `MEMORY_DEK_NEXT` alongside `MEMORY_DEK`.
2. Background job re-encrypts batches of rows: read with `MEMORY_DEK`,
   write with `MEMORY_DEK_NEXT`, commit per batch.
3. After all rows are migrated and verified, swap the secrets: rename
   `MEMORY_DEK_NEXT` → `MEMORY_DEK`; the old key is retired.
4. Audit table `memory.key_rotations` (added in the rotation phase spec)
   records the rotation event. Phase-level details land in the rotation spec.

This is AR-7 in §13.

---

## 10. Cross-Cutting Concerns

### 10.1 Authentication and identity

OAuth2+PKCE with GitHub or GitLab as the identity provider; **single
primary identity per user** (FR-2). Secondary providers attach as event
sources only — they do not federate identity. API keys for MCP are issued
per agent and scoped to one org. Exact key lifecycle and rotation policy
land in the P01 spec.

**OAuth callback architecture (CONFIRMED):** The OAuth callback is **not**
an Encore endpoint. It is an Astro Action at
`unblock.websublime.com/auth/[provider]/callback`. Flow:

1. Browser is redirected to GitHub/GitLab OAuth with a PKCE challenge stored
   in an HttpOnly state cookie on the Astro origin.
2. Provider redirects back to the Astro Action with `code` + `state`.
3. The Astro Action validates state + PKCE, then calls the **private** Encore
   RPC `auth.ExchangeOAuthCode(code, pkce_verifier)`.
4. Encore exchanges the code for the OAuth token, encrypts and stores the
   token in `auth.oauth_tokens`, and returns an opaque session id.
5. The Astro Action sets the HttpOnly, SameSite=Lax cookie on the Astro
   origin with the session id and redirects the user to the app.

The browser never holds an Encore API key, never holds an OAuth token, and
never makes a cross-origin request to Encore. Law 4 / NFR-4 is structural.

**BFF → Encore private API auth: see §5.3.1.** The Astro server forwards
the user's session id (`Authorization: Bearer <session_id>` plus
`X-Unblock-BFF-Origin: astro`) — there is no separate service credential to
manage. Astro Actions are a transparent BFF proxy, not a credentialed peer.

### 10.2 Observability

`tracing` JSON Lines on STDERR (NFR-12). STDOUT is reserved for protocol
payloads — MCP envelopes from the backend, JSON envelopes from the AST
CLI. **Never mix progress and results.** Encore's built-in distributed
tracing carries `trace_id` across RPCs; the MCP transport layer propagates
the `trace_id` from the Bearer-key validation step into every tool handler
so a single agent call is one span tree. The `trace_id` is also persisted
in `mcp.tool_calls.trace_id` for after-the-fact correlation.

### 10.3 Quality gates (NFR-10)

| Stack | Gate |
|---|---|
| Go | `go test ./... -race`, `go vet`, `golangci-lint run` (zero warnings), Encore-generated client diff is zero |
| Rust | `cargo fmt --check --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --no-deps --workspace` (zero warnings) |
| Astro | `astro check`, `tsc --noEmit`, `eslint --max-warnings 0`, `vitest run` |
| Cross | NFR-1 latency check (`prime → ready → claim` p99 < 2 s); NFR-2 RBAC regression suite; AST CLI HARD perf gates per code-cli/plan §14.1; **catalogue drift test (§7.2)** |

Every change must pass its stack's full gate set before merge. Every
release candidate must additionally pass the cross gates.

### 10.4 Coding discipline (NFR-11)

- Rust: `#[non_exhaustive]` on growable public enums in library crates;
  `snafu` errors with crate-scoped `Result<T>`; no `unwrap()` / `expect()`
  outside tests; `#![deny(unsafe_code)]` workspace-wide.
- Go: typed errors with `errors.Is` / `errors.As`; never panic in production
  code paths; sentinel errors at package boundaries; `gofmt`-clean.
- Astro: strict TypeScript (`strict: true`, `noUncheckedIndexedAccess: true`);
  Zod (or equivalent) validation at every Astro Action boundary; no `any`
  without a comment.

### 10.5 Logging and tracing

- JSON Lines on STDERR everywhere (NFR-12).
- Per-request correlation: Encore's `trace_id` is propagated from public
  ingress through every private RPC and Pub/Sub subscriber.
- The AST CLI emits its own `trace_id` per invocation (no propagation —
  it is decoupled per Law 6), separate logs.
- The plugin's rendered hooks include the MCP `trace_id` field in their
  reported findings so post-dispatch validations can be correlated to the
  session that produced them.

---

## 11. Phase-to-Component Traceability

| Phase | Components touched | Manifesto coverage | Exit criterion (per PRD §8) |
|---|---|---|---|
| **P01 — Backend MVP** | `apps/api/auth` (incl. **migrations directory for all 8 schemas**, see §5.2), `org`, `workitems`, `deps`, `memory` (schema-only stub), `providers` (schema-only stub), `boards` (schema-only stub), `mcp` (23 tools per P01 round-16 — was 14; adds `promote`, four milestone tools, four label tools; Streamable HTTP transport per MCP spec 2025-06-18), `public/`; migrations §9.4.1–§9.4.8 + round-16 `0120_mcp_issued_to_user_notnull` + round-16 `0130_workitems_labels_updated_at` (all eight schemas land in P01 per the plan resolution; service code for `providers`/`boards`/`memory` is deferred to later phases per plan §2.1) | L1, L2, L3 (foundations), L5, L7 | Agent completes `prime → ready → claim → close` against a manually-seeded graph; cascade fires; cycle detection rejects offending edges |
| **P02 — Backend complete** | `apps/api/providers` (webhook + sync), `apps/api/boards`, `mcp` (+4 memory tools = 27 total; P01 round-16 carried P01 to 23), Layer 1 state-transition validator, **`mcp.meta_catalogue` v1 + `verify_can_transition` v1** (operational primitives, §5.2.2); migrations §9.4.5 + §9.4.7 | L3 (provider events), L8 layer 1 | A GitHub repo can be linked, webhooks normalise into canonical work items, attempts to mark `done` without the required comment trail are rejected at the MCP boundary; `mcp.meta_catalogue` returns the live catalogue.json; `verify_can_transition` validates a candidate transition against the same Layer-1 validator |
| **P03 — AST CLI v1.0.0** | `crates/unblock-indexer-core`, `unblock-indexer`, `unblock-code` | L6 | All 9 HARD gates in code-cli/plan §14.1 pass on a fresh clone; ROI harness publishes raw logs + per-flow medians as a release artefact |
| **P04 — Plugin renderer** | `crates/unblock-plugin` **consumes the P02-shipped `mcp.meta_catalogue` v1** at build time (and embeds `crates/unblock-plugin/data/catalogue.json` via `include_str!`); registers the `verify-state` hook against `mcp.verify_can_transition` (also shipped in P02) | L8 layer 2 + 3 (full Law 8) | Pipeline-bypass attempt is rejected by MCP (Layer 1, P02), flagged by the post-dispatch hook (Layer 2), and refused by the agent prompt's BLOCK condition (Layer 3); all three layers agree; catalogue drift CI test green |
| **P05 — Astro web (v1.1)** | `apps/web/` (Astro Actions BFF including `auth/[provider]/callback`, kanban, graph, roadmap, comments) | L4 | A developer authenticates, sees the same graph the agent sees, and acts on it through the BFF without the browser ever obtaining Encore credentials |

---

## 12. Out-of-Scope for the Architecture

These are deliberately not in this architecture; some are recoverable
post-v1, others are permanent (Manifesto out-of-scope).

### 12.1 Permanent (Manifesto)

- Desktop application (no GPUI, Tauri, Electron).
- Code generation by the AST CLI.
- Custom storage that duplicates Postgres (no Redis, no service-local SQLite
  in `apps/api/`).
- Provider-specific UI (we link to GitHub for native PR review).
- Wiki / CMS replacement (memory is atomic facts, 8 KB cap, no rich text).
- Network-level multi-tenant isolation (RBAC is row-level, not VPC).
- Self-hosting story for v1.
- Real-time collaboration on work-item content.
- Agent decision-making (the platform informs; the agent decides).

### 12.2 v1.0 deferrals (recoverable)

- GitLab provider integration → v1.1.
- Astro web client → v1.1, gated on line-ui v1.
- Import tooling from `bd` / Linear / Jira / GitHub Issues → backlog.
- SLA / uptime guarantees → not offered at v1; depends on Encore Cloud
  step-up.
- Native mobile clients → post-v1 (web is mobile-responsive).
- Linear / Jira provider integration → no committed phase.

---

## 13. Risks This Architecture Carries

These are architecture-level risks (PRD §12 covers product-level risks; this
section calls out the ones the architecture choices specifically introduce).

| # | Risk | Architectural mitigation |
|---|---|---|
| AR-1 | **Encore lock-in.** Replatforming away from Encore is a large undertaking. **ACCEPTED per user decision.** | Encore code is mostly Go + Postgres + Pub/Sub semantics; the abstraction surface is thin. Exit strategy through self-hosted NATS + standard Postgres is feasible at moderate cost if Encore ever blocks us. |
| AR-2 | **Single-Postgres scaling ceiling.** All eight schemas share one DB; one schema's hot writes can affect another. | Acceptable at v1 scale per PRD §11 (the M-1 latency target is met on this topology). Step-up path: read replicas + partitioning per schema, both supported by Encore Cloud's managed Postgres. Re-architecture into multi-DB is a phase-replan event, not a v1 concern. |
| AR-3 | **Polyglot monorepo CI complexity.** Three toolchains (Go, Rust, Astro) in one repo. | Per-stack gate sets isolate the matrices; Olive (CI-CD supervisor) owns orchestration; per-supervisor branches keep concurrent work clean. |
| AR-4 | **Plugin renderer drift from MCP state machine.** Layer 1 is in Go; Layer 3 is rendered at build time by Rust from a checked-in JSON catalogue. They could disagree. | The state-machine catalogue is owned by the backend, exposed live via `mcp.meta_catalogue`, and checked into `crates/unblock-plugin/data/catalogue.json` for build-time embed via `include_str!`. CI (§7.2) diffs the two; mismatch = red build. |
| AR-5 | **AST CLI zero-coupling tempts duplication.** With no shared types, both binaries may re-implement similar utilities. | Acceptable; Manifesto Law 6 is strict precisely because the cost of duplication is lower than the cost of cross-binary coupling. |
| AR-6 | **BFF discipline at v1.0 (no web yet).** Until P05 ships, no BFF exists; all clients of private APIs are internal. | Until v1.1, the public surface is just two FR-12 endpoints; private APIs remain truly private (no browser path at all). When P05 lands, the BFF discipline is enforced from day one of v1.1. |
| AR-7 | **`pgcrypto` symmetric DEK rotation cost.** Three schemas hold `_enc` columns; rotation requires re-encrypting every row of `auth.oauth_tokens`, `providers.installations`, and `memory.entries`. | Rotation is offline-batchable. The rotation strategy (§9.4.10) introduces `MEMORY_DEK_NEXT`, re-encrypts in batches, then swaps. At v1 scale (low row counts) the operation completes in minutes; at scale it remains a background job. The DEK is supplied via Encore secret only — application code never logs it. |
| AR-8 | **Recursive CTE depth on dependency cycle / closure / milestone walks.** Postgres recursive CTEs are bounded by available memory; a deeply chained dependency graph could cost more than the latency budget. | Three mitigations: (a) milestone tree is structurally bounded by M-INV-6 (depth ≤ 4); (b) dependency cycle CTE uses an explicit **depth-counter pattern** (`WHERE depth < 256` inside the recursive term, see §9.4.9) — `LIMIT` inside a recursive term has undocumented PG semantics (research C5) so a counter is the standard approach; (c) `is_ready` is materialised, not computed at read time, so the closure CTE runs only inside the cascade subscriber, asynchronously. The depth cap means a 257-node chain refuses new edges with a structured error pointing at the offending chain. Concurrency: `add_dependency` acquires `pg_advisory_xact_lock(hashtext('deps.add_dependency:' || $project_id))` to serialise concurrent edge writes within a project (research AF5). |
| AR-9 | **GIN index cost on `memory.entries`** (FTS + tags + key trigram). At scale GIN indices have high write amplification. | Memory write rate is low (entries are atomic facts, not chat logs). At v1 scale GIN cost is dominated by read-side fan-out, which is what we want. The 8 KB hard cap on `value_size` keeps `ts_doc` bounded. If write amplification ever becomes an issue, per-scope partitioning of `memory.entries` is the documented step-up. |
| AR-10 | **`tsvector` leakage surface.** `memory.entries.ts_doc` is built from plaintext, stored unencrypted (it must be GIN-indexable). | Tradeoff is explicit: search requires unencrypted indexable terms. Mitigation: secret sanitiser (NFR-7) runs **before** tokenisation, so any sniffable terms in `ts_doc` have already been redacted. Audit: `ts_doc` is only readable by the `memory` service connection user; no other Encore service has SELECT access to that schema. |
| AR-11 | **Pub/Sub at-least-once delivery.** Encore Pub/Sub delivers `deps.cascade.requested` (and any future cascade-driving event) at least once. A subscriber that is not idempotent risks double-counting cascade audit rows or flipping `is_ready` twice with different intermediate observations. | The cascade subscriber is idempotent **by construction** (§5.4): the closure CTE produces a deterministic target set, the UPDATE is a value-equality write on a stable graph, and `deps.cascade_events` rows are written once per `(event_id, triggered_by_item_id)`. **Event id provenance (research C1).** Encore Go's subscriber handler signature does not expose envelope metadata, so the **publisher** generates a ULID `EventID` at emit time and embeds it as a typed field on the message payload. The subscriber reads it from the payload and uses it as the idempotency key; the UNIQUE constraint on `deps.cascade_events (event_id, triggered_by_item_id)` (§9.4.4) is the structural mitigation. Encore's redelivery resends the same payload bytes (including `EventID`), so duplicates collide deterministically. The subscriber maintains both `is_ready` and `pipeline_stage` (§5.7.1) in the same UPDATE, so re-delivery converges to the same row state. New subscribers added in future phases must declare an idempotency key — generated by the publisher and carried as a typed payload field — in their phase spec. |
| AR-12 | **Webhook replay protection.** GitHub may re-deliver the same `X-GitHub-Delivery` id (manual replay, partition retry). Without dedup the `providers.events` audit grows unbounded and the normaliser may produce duplicate work-item updates. | The `events_delivery_uniq UNIQUE (provider, delivery_id)` constraint on `providers.events` (§9.4.5) is the structural mitigation — duplicate inserts fail at the database, the webhook handler returns 200 OK on a recognised duplicate so GitHub stops retrying, and the normaliser is never called twice for the same delivery id. The HMAC signature check (FR-12) gates whether we accept the payload at all; the unique constraint gates whether we process it. Both layers must hold for a webhook to mutate state. |
| AR-13 | **Encore Cloud free-tier ceiling (PRD R-1 cross-reference).** v1.0 launches on the free tier; concrete ceilings affect the M-1 latency target. | Documented ceilings the architecture must respect: **Pub/Sub message rate** (rate limited per project on the free tier — measured ceiling lands in the P02 capacity-planning gate before scale-out); **Postgres connection cap** (free-tier shared-cluster connection limit; `auth.Validate` and the cascade subscriber are the two largest pool consumers — both must use Encore's pooled DB binding rather than a fresh connection per call); **cold-start latency** (free-tier scale-to-zero behaviour can add seconds to a cold MCP `prime` call — mitigated by a synthetic warmer hitting `mcp.meta_catalogue` every N minutes from the same project). The Encore-lock-in exit (AR-1) presupposes that any of these ceilings becomes binding before scale-out is possible. M-1 is measured warm; cold-start is documented as an outlier class for the launch period. |
| AR-14 | **Memory secret-sanitiser false negatives (PRD R-6 architectural mitigation).** The sanitiser is best-effort regex; novel credential shapes will slip through. | Three-part mitigation. (a) **Audit-on-detect:** every sanitiser hit (positive or warning-only) is recorded in a `memory.sanitiser_events` audit table (added in P02 alongside the memory service) so we can review what *was* caught and tune patterns; (b) **periodic re-scan:** a background job re-runs the current pattern set against existing `value_enc` rows on every pattern-set update — re-encrypting any row whose decrypted plaintext now matches a pattern; (c) **scoped re-encryption:** the re-scan operates per-scope (org/project/user) so it can be paused for a tenant if the cost is too high in one project. The sanitiser remains best-effort by design (NFR-7); these mitigations ensure that a missed pattern today is recoverable tomorrow. |
| AR-15 | **Migration-owner service deploy ordering (research C2).** All eight schemas' migrations are owned by the `auth` service (§5.2). Any service whose handlers query a schema other than its own depends on `auth` having reached a healthy state first; if Encore deploys an out-of-order subset (e.g. `workitems` boots before `auth` finishes its bootstrap migration), queries fail with `relation does not exist` errors. | Encore handles this automatically through its dependency graph: a service that calls `sqldb.Named("unblock")` implicitly depends on the service that defined the database (`auth`), and Encore's deployer sequences services in dependency order. The architecture pins `auth` as the migration-owner explicitly (§5.2) so the dependency graph is deterministic. CI gate: a "migrations only" job runs `auth` to completion against an empty database before any service-test job starts (P01 plan A-6). The deploy-ordering invariant is asserted in the exit-criterion harness (E-4) by booting the local Encore emulator from a clean state and confirming `auth` reaches healthy first. |
| AR-16 | **Streamable HTTP cold-start latency on Encore Cloud edge proxy (research C6 + AR-13).** The MCP transport is `POST /mcp` + `GET /mcp` (Streamable HTTP per the 2025-06-18 spec). On Encore Cloud's free tier, the edge proxy may scale to zero between requests; the first MCP `POST /mcp` after an idle period adds cold-start latency (potentially seconds) on top of the warm-cache budget — directly threatening NFR-1 / M-1 (`prime → ready → claim` p99 < 2 s). The legacy SSE transport had the same risk; Streamable HTTP does not introduce a new class of risk but the per-call shape (a fresh `POST` for every tool call rather than a single long-lived SSE stream) means more cold-start opportunities. | Three-part mitigation. (a) **NFR-1 measured warm only.** The latency harness (P01 plan E-2) explicitly carves out cold-start outliers; warm-cache means: pool established, identity validated, no scale-to-zero rehydration in the budget. Documented as part of the M-1 measurement methodology. (b) **Synthetic warmer.** A small Encore cron (`mcp-warmer`, every N minutes) hits `mcp.meta_catalogue` from the same project to keep the MCP service warm during business hours; the same warmer pre-establishes a Postgres pooled connection. (c) **Server-side event-stream response mode.** For long-running tools (none required at v1.0), Streamable HTTP's `text/event-stream` response shape lets a single `POST /mcp` keep the connection open while the tool runs — the cold-start cost is amortised over the tool's full execution. The P01 spec pins which tools (if any) opt into the streaming response mode and documents the keep-alive heartbeat interval for `GET /mcp` long-poll connections. Cold-start under Encore Cloud is a P02 ops measurement task (P01 acceptance is local-emulator only per plan §6 Q6). |

---

## 14. Open Questions for the User

**All questions resolved 2026-05-07.** History preserved for traceability:

| # | Question | Resolution |
|---|---|---|
| 1 | Repository layout (`apps/api/`, `apps/web/`, `crates/`, `temp/rust-v1/` archived) | **CONFIRMED** — see §4. `temp/rust-v1/` gitignored. |
| 2 | 8 services × 8 schemas (1:1) vs collapsing some | **CONFIRMED 8:8** — see §5.2.1. Isolation > RPC overhead (Manifesto Principle 4 applies intra-backend). |
| 3 | Encore Cloud lock-in | **CONFIRMED** — locked from prior strategic discussion. AR-1 accepted with NATS + standard Postgres exit path. |
| 4 | Plugin catalogue export shape | **CONFIRMED** — JSON checked-in at `crates/unblock-plugin/data/catalogue.json` + compile-time embed via `include_str!`. Backend MCP `meta.catalogue` tool exposes the same catalogue live. CI drift test enforces equality. See §7.2. |
| 5 | Public endpoint paths | **CONFIRMED + CORRECTED** — `/auth/callback` was a PRD bug; OAuth callback is on the **Astro origin** as an Astro Action, not Encore. v1.0 Encore public surface = 2 logical endpoints (`/webhooks/github`, `/mcp` over Streamable HTTP per the 2025-06-18 spec — `POST` and `GET` on the same path). v1.1 adds `/webhooks/gitlab`. See §5.3, §10.1. (Round-3 correction: round-2 referenced `GET /mcp/sse`, the deprecated 2024-11-05 transport; round-3 research C6 contradicted that and Streamable HTTP is now the canonical shape.) |
| 6 | Drop Rust `unblock-mcp` crate | **CONFIRMED** — fully archived under `temp/rust-v1/` (gitignored). New MCP server is the Encore Go service in `apps/api/mcp/`. |
| 7 | Plugin renderer install UX | **CONFIRMED** — `unblock-plugin render --target=<target> --supervisors=<list> --out=<dir> [--apply]`. Carries v1 design pattern verbatim. See §7.1. |
| 8 | Web UI graph rendering library | **CONFIRMED** — deferred to P05 spec. Architectural commitment is "force-directed dependency graph rendered as canvas / SVG"; library locked at Stage 2. |

---

## 15. Approval Checklist

This document moves from DRAFT to APPROVED when:

- [x] All eight open questions in §14 are answered.
- [x] The user confirms the four-deliverable shape (backend, AST CLI,
      plugin, web).
- [x] The user confirms the 8-schema / 8-service Encore decomposition.
- [x] The user confirms the polyglot monorepo layout.
- [x] The user confirms the public-endpoint inventory (2 at v1.0, +1 at
      v1.1) and Bearer-key MCP auth model; OAuth callback is on Astro.
- [x] No Manifesto Law is violated by any choice in §3 — §13.
- [x] §9.4 canonical DDL is present for all 8 schemas, with extensions,
      migration ordering, encryption pattern, and recursive CTE patterns
      documented.

**Round-2 review-iteration items (closed 2026-05-07):**

- [x] Cascade audit table `deps.cascade_events` added (§9.4.4); M-5 metric
      query rooted in this table (AR-11 cross-references it).
- [x] `pipeline_stage` ownership pinned (§5.7.1) — derived from the three
      dimensions + `pipeline_state`, materialised by the same Pub/Sub
      subscriber that maintains `is_ready`. Derivation table is
      authoritative.
- [x] Astro server ↔ Encore private RPC auth mechanism pinned (§5.3.1) —
      Astro forwards `Authorization: Bearer <session_id>` plus
      `X-Unblock-BFF-Origin: astro`; no separate service credential. §3.1
      diagram updated. §10.1 references §5.3.1.
- [x] BLOCK condition schema pinned (§7.5) — typed shape, single
      `catalogue.json` source consumed by Layer 1 (Go codegen), Layer 2
      (`verify_can_transition` re-validates), and Layer 3 (Rust renderer).
- [x] 27 MCP tool inventory pinned (§5.2.2; P01 round-16: 18→27) — 23 in
      P01, +4 memory in P02; `verify_can_transition` and `meta_catalogue`
      are operational primitives outside the agent-facing 27 (PRD FR-8
      reconciliation noted inline).
- [x] Diagram + traceability cleanup (§3.1 catalogue arrow added; §11
      P02 ships `meta_catalogue` v1, P04 consumes it; §5.7 codegen route
      named — `apps/api/mcp/catalogue.gen.go` via `go generate`; §7.2
      tightened to clarify CI is the only reconciliation point).
- [x] Pub/Sub idempotency reinforced (§5.4); new architectural risks
      AR-11 (Pub/Sub at-least-once), AR-12 (webhook replay protection),
      AR-13 (Encore Cloud free-tier ceiling), AR-14 (sanitiser false
      negatives mitigation) added to §13.
- [x] Provider-event PII retention policy stated (§9.4.5 — 90 days raw,
      then digest-only).

**Round-2 P01-spec review iterations (closed 2026-05-08):**

- [x] `deps.cascade_events.kind` column added (§9.4.4) with CHECK enum
      `('close', 'edge_removed')`. `'close'` is written by the cascade
      subscriber for Pub/Sub-driven close events; `'edge_removed'` is
      written inline by P01 spec §6.2 Tool 12 (`remove_dependency`).
      Future cascade kinds extend the CHECK additively per phase spec.

**Round-6 cascade-symmetry sync (in progress, opened 2026-05-12):**

- [x] §9.4.4 `deps.cascade_events.kind` CHECK enum extended from two values
      to four first-class values: `('close', 'edge_added', 'edge_removed',
      'state_change')`. All four kinds are P01-active; the round-2 framing
      of `state_change` as "deferred to P02+" is retired.
- [x] §9.4.4 `kind` column doc-block rewritten to describe the symmetric
      writer model and per-kind provenance: `'close'` and `'edge_added'`
      are written by the cascade subscriber (Regime B); `'edge_removed'`
      is written inline by Tool 12 (Regime A) with the subscriber's
      duplicate insert collapsed via `ON CONFLICT … DO NOTHING`;
      `'state_change'` is written by the cascade subscriber, emitted by
      Tool 13 (review/qa state writes) and by `workitems.Claim` on the
      I-3 reset path only.
- [x] Cross-reference pinned: the cascade subsystem maintains two
      propagation regimes (Regime A writer-inline `is_ready`; Regime B
      subscriber-only `pipeline_stage` plus subscriber-driven `is_ready`
      for `close` / `edge_added` / `state_change`). Canonical model and
      implementation contract live in phase spec
      [§6.3.0 "Propagation regimes"](../docs/specs/01-spec-backend-mvp.md).
      This Stage-1 document owns *what exists* at the architecture level;
      the phase spec owns *how it is implemented*.

**Status: APPROVED. Approved 2026-05-07 (Stage-1 canonical); round-2 applied 2026-05-08; round-6 cascade-symmetry sync applied 2026-05-12.**

This document is the input to:

- `/plan` per phase (P01, P02, P03 [carried verbatim], P04, P05) — Stage 2
  phase plans under `docs/plans/`.
- `/spec` per phase — Stage 2 phase specs under `docs/specs/`.
- `/tasks` per phase — Stage 2 task breakdowns into the bd graph.

Phase-level migration scripts realise the canonical DDL in §9.4 in the
order documented in §9.4.0. Phase specs may add columns or indexes; they
must not deviate from the column types, constraints, or relationships
defined in §9.4.
