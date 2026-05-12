# PLAN: P01 — Backend MVP (Encore Go + Postgres + 14 MCP Tools)

**Status:** APPROVED
**Author:** Ada (architect)
**Date:** 2026-05-07 (resolutions applied 2026-05-08)
**Source PRD:** [docs/PRD.md](../PRD.md) (APPROVED, 2026-05-07)
**Source SPEC:** [docs/SPEC.md](../SPEC.md) (APPROVED 2026-05-07, round-2 review applied)
**Companion:** [docs/MANIFESTO.md](../MANIFESTO.md) (APPROVED, 2026-05-07)

> Stage 2 deliverable. This is a **phase plan**, not an implementation
> spec. It defines what is in/out of P01, the high-level work breakdown,
> internal and external dependencies, and the acceptance criteria.
> Implementation contracts (exact JSON schemas per MCP tool, exact RPC
> signatures, exact error envelopes, exact migration files) land in the
> phase **spec** (`docs/specs/01-spec-backend-mvp.md`) authored under
> `/spec` once this plan is APPROVED and research validates the open
> assumptions in §6.

---

## 1. Phase Goal

Ship the foundational backend that makes Manifesto Law 7 real: an AI
agent calls `prime → ready → claim → close` and the platform answers in
under two seconds on a warm cache, with cascade firing structurally and
cycle detection rejecting offending edges at write time.

P01 is the **agent-facing core** — work-item CRUD, dependency graph,
ready-set materialisation, atomic claim, comment trail, structured state
columns, and the SSE MCP transport with 14 tools. Provider integration,
the four memory tools, and Layer-1 state-transition validation are
deferred to P02 by design (PRD §8). P01 lays down the schemas that P02
fills in — including `providers`, `boards`, `mcp`, and `memory` — but
exposes only the surfaces required for the P01 exit criterion.

P01 exit criterion (verbatim from PRD §8 + SPEC §11): *an agent can
authenticate via Bearer API key and complete `prime → ready → claim →
close` against a manually-seeded graph; cascade fires; cycle detection
rejects offending edges.*

---

## 2. Scope — What is IN P01

### 2.1 Encore Go services to stand up

Per SPEC §5.2 (8 services × 8 schemas, 1:1 mapping locked).

| Service | Surface delivered in P01 | Notes |
|---|---|---|
| `auth` | OAuth2+PKCE flow (private `auth.ExchangeOAuthCode`), session validation (private `auth.Validate`), API key issuance + validation (MCP Bearer auth), `Identity` resolution | OAuth callback **lives on the Astro origin** in P05; in P01 the callback is exercised by integration tests only. API key issuance ships **as a private RPC** since there is no web UI to drive it; tests and operators use it via Encore's local dashboard or a one-shot CLI seeder (§4.4). |
| `org` | Org / project CRUD, RBAC role bindings, `org.Authorize(identity, resource, action)` | Required because every `workitems`, `deps`, and `mcp` call is org-scoped (NFR-2). |
| `workitems` | Items, comments, labels, milestones (recursive), findings; private RPCs `Create / Update / GetTrail / ListByMilestone / AppendComment / SetStateColumns` plus the four milestone RPCs `CreateMilestone / UpdateMilestone / AssignItem / MilestoneTree` (round-2 D1 — see SPEC §4.4.1). | Comments append-only; milestone tree depth ≤ 4 (M-INV-6); findings are first-class child items. **Milestone CRUD is reachable only via private RPC in P01** — agent-facing milestone MCP tools defer to P02 to preserve PRD FR-8 "18 tools at v1.0" (option (c) in round-2 D1). The seeder CLI and the future Astro client (P05) drive milestone creation through the private RPCs. |
| `deps` | Edges, cycle detection at write time, ready-set materialisation, cascade subscriber, `deps.cascade_events` audit table; private RPCs `AddEdge / RemoveEdge / IsReady / Closure` | The cascade subscriber **also maintains `pipeline_stage`** (SPEC §5.7.1). |
| `mcp` | Streamable HTTP transport (`POST /mcp` + `GET /mcp` per MCP spec 2025-06-18), Go SDK `github.com/modelcontextprotocol/go-sdk`, tool registry, the 14 P01 tools, Bearer API key auth via `auth.Validate`, structured error envelope | **No state-machine BLOCK conditions** in P01 — see §3.4 below for the explicit deferral. |
| **public** | FR-12 v1.0 endpoints' wiring (`POST /mcp` + `GET /mcp` Streamable HTTP only — `POST /webhooks/github` is P02) | Only the MCP endpoint in P01. |

Services whose **schema** ships in P01 but whose **runtime surface is empty**:

| Service | What ships in P01 | What is deferred |
|---|---|---|
| `providers` | DDL only (§9.4.5); no service code, no public webhook endpoint | Webhook handler, normaliser, sync — P02 |
| `boards` | DDL only (§9.4.7); no service code | Saved-view CRUD — P02 |
| `memory` | DDL only (§9.4.8); no service code, no MCP tools, no sanitiser | Sanitiser, the 4 MCP tools, secret-event audit — P02 |

Rationale for "schema-only" services in P01: SPEC §11 traceability lists
P01 migrations as §9.4.1–§9.4.4 + §9.4.6 + §9.4.8 (so `memory` schema is
in P01 by SPEC), and we extend that to **all eight schemas in P01** so
the database has its final canonical shape on day one. **This is an
intentional deviation from SPEC §11 — see §6 OPEN QUESTION 2.** The
alternative (gradual migrations) was considered and rejected because
splitting DDL across phases couples the migration order to the phase
sequence, which the §9.4.0 ordering rules already pin independently.

### 2.2 The 14 MCP tools (SPEC §5.2.2)

| # | Tool | P01 contract notes |
|---|---|---|
| 1 | `prime` | Returns ready set summary, claimed-by-me items, recent cascade events. **Memory hints field is empty in P01** (memory tools ship in P02). |
| 2 | `ready` | Reads `workitems.items.is_ready=true`; honours `--limit`; org/project-scoped; deterministic ordering by `(priority, created_at, id)`. |
| 3 | `claim` | Atomic `SELECT FOR UPDATE` per SPEC §5.5. Loser receives structured "already claimed" error citing winner's identifier and timestamp. |
| 4 | `create` | Creates `type=task | epic | finding`. P01 supports parent (epic), milestone, labels, and dependencies on creation. |
| 5 | `update` | Mutates title, body, priority, milestone, labels. **Does not** touch state dimensions (use `set_state`). |
| 6 | `close` | Sets `closed_at`, sets `Status=Done`. **In P01** there is no Layer-1 precondition gate — see §3.4. Emits cascade. |
| 7 | `show` | One item with full comment trail, dependencies (in/out), and finding children. |
| 8 | `list` | Filters: status, pipeline_stage, claimed-by, milestone, labels. RBAC-scoped. |
| 9 | `search` | FTS over titles + bodies + comment bodies. RBAC-scoped. |
| 10 | `comment` | Append `(kind, status, body)`; both axes per FR-10. Append-only. |
| 11 | `add_dependency` | `from blocks to`. Cycle-checked at write time using the depth-counter recursive CTE in SPEC §9.4.9 (depth ≤ 256, AR-8). Acquires per-project `pg_advisory_xact_lock` to serialise racing edge writes (research AF5). |
| 12 | `remove_dependency` | Removes edge; emits `deps.cascade.requested`; the `to` side may flip `is_ready=true`. |
| 13 | `set_state` | Writes one or more of `impl_state`, `review_state`, `qa_state`, `pipeline_state`. **In P01, this enforces structural invariants AND the five PRD §6.2 state-machine invariants (round-2 D2)** — pure column-value rules with no comment-trail dependency (e.g. writing `qa_state=failed` requires `review_state=approved`; writing `review_state=needs_rework` resets `qa_state=pending` atomically). The **comment-trail-driven Layer-1 BLOCK conditions are P02** — see §3.4. |
| 14 | `get_state` | Returns the four state columns + materialised `pipeline_stage` + most recent `(kind, status)` per kind from the comment trail. |

### 2.3 Cross-cutting machinery in P01

- **Cascade subsystem (Law 1)** — `deps.cascade.requested` topic, the
  cascade subscriber that maintains `is_ready` AND `pipeline_stage`,
  the `deps.cascade_events` audit row written once per `(delivery_id,
  triggered_by_item_id)` per AR-11.
- **Atomic claim (Law 5)** — exactly the SQL in SPEC §5.5; no advisory
  locks, no compare-and-swap.
- **Cycle detection (NFR-5)** — at write time inside `add_dependency`
  and inside `create` when dependencies are inlined; recursive CTE per
  SPEC §9.4.9 with depth-counter cap (≤ 256); structured error pointing at the
  offending chain.
- **RBAC (NFR-2)** — `pkg/rbac` typed query helper + per-service DB
  binding so cross-service direct reads are impossible. The exhaustive
  RBAC regression suite under `apps/api/shared/rbactest` ships in P01
  for the surfaces P01 exposes; P02 extends it for `providers` and
  `memory`.
- **Tracing & logging (NFR-12)** — Encore's distributed tracing
  carries `trace_id` from `mcp/sse` ingress through every private RPC
  and the cascade subscriber; persisted in `mcp.tool_calls.trace_id`.
- **Quality gates (NFR-10)** — `go test ./... -race`, `go vet`,
  `golangci-lint run` (zero warnings), Encore-generated client diff is
  zero. Plus the NFR-1 latency check (`prime → ready → claim` p99 < 2 s)
  on the warm-cache integration harness.
- **Catalogue source file** — `apps/api/mcp/catalogue.json` is **created
  in P01** with the 14 P01 tools' tool definitions, but the
  state-machine section is left as a placeholder (the BLOCK conditions
  catalogue is finalised in P02). `go generate` runs in P01 and emits
  `apps/api/mcp/catalogue.gen.go`. SPEC §7.2 / AR-4 drift CI is
  scaffolded in P01; it goes red on a real semantic drift starting in
  P02 when `unblock-plugin` consumes the catalogue. **See §6 OPEN
  QUESTION 4** for the interplay with P02 / P04.

### 2.4 Repository / infra deliverables

- `apps/api/encore.app` initialised; one Postgres database;
  Encore Pub/Sub topics `deps.cascade.requested` and
  `deps.cascade.completed` declared.
- `apps/api/migrations/` ships the canonical DDL for **all eight
  schemas** per §2.1 above, ordered per SPEC §9.4.0.
- `infra/github/` CI workflow runs Greta's gate set (Go) plus the
  cross-cutting NFR-1 + NFR-2 gates. (Infra supervisor scope.)
- A one-shot Go CLI seeder under `apps/api/cmd/unblock-seed/` capable
  of (a) bootstrapping an `auth.users` + `org.organizations` + project
  + API key fixture and (b) loading a manually-seeded dependency graph
  from a YAML fixture for the exit-criterion harness. **See §6 OPEN
  QUESTION 3** — the user may prefer Encore's local dashboard for this.

---

## 3. Scope — What is OUT of P01

These are **deferred to a later phase**. They are explicitly out of P01;
attempting to add any of them widens the phase and breaks the exit
criterion focus.

### 3.1 Provider integration (deferred to P02)

- No `POST /webhooks/github` handler.
- No GitHub REST/GraphQL bidirectional sync.
- No reconciliation scheduler.
- No `providers` service code (schema only — see §2.1).
- All work items in P01 are created via MCP `create`. There is no
  external event source.

### 3.2 Memory service (deferred to P02)

- No `remember` / `recall` / `memories` / `forget` MCP tools.
- No secret sanitiser, no `memory.sanitiser_events` audit table
  population (the table itself, if specified, lands in P02 alongside
  the service code).
- `prime` returns an empty `memory_hints` field in P01.

### 3.3 Astro web client (deferred to P05, v1.1)

- No Astro app directory shipping; no Astro Actions; no BFF cookie.
- The OAuth callback is exercised by integration tests in P01, not by
  a browser.
- No `X-Unblock-BFF-Origin` enforcement in middleware **as a runtime
  concern** in P01 — the middleware ships with the rule active but no
  BFF caller exists yet, so the surface is exercised only by tests.

### 3.4 Layer-1 state-transition BLOCK conditions (deferred to P02)

This is the **largest deliberate deferral** and it requires explicit
acknowledgement: PRD §8 P01 exit criterion does **not** require Layer-1
enforcement; PRD §8 P02 exit criterion does. The SPEC §5.2.2 P01 tool
table calls `set_state` "gated by Layer-1 BLOCK conditions (§7.5)" —
this plan **interprets that as forward-pointing** (the tool that will be
gated, when Layer 1 ships in P02), not as a P01 deliverable.

**In P01:**
- `set_state` writes the requested columns subject only to **structural
  invariants** that already live in the DDL — e.g. `impl_state=done`
  requires `claimed_by_id IS NOT NULL` (DB-level), the `(impl_state,
  review_state, qa_state, pipeline_state)` CHECK constraints from §9.4.3
  reject malformed combinations.
- `close` does **not** require `qa_state=passed` in P01; the precondition
  is `claimed_by_id IS NOT NULL` only. **This is a deliberate P01
  relaxation** so the exit criterion's `prime → ready → claim → close`
  flow can succeed without a full review/QA dance. P02 tightens this by
  replacing the structural-only check with the full Layer-1 validator.
- `mcp.verify_can_transition` does not exist as an RPC in P01.
- `mcp.meta_catalogue` does not exist as an RPC in P01 (the JSON file
  exists per §2.3, but no MCP tool exposes it).

**See §6 OPEN QUESTION 1** — this deferral must be confirmed by the user
because it is a literal reading of PRD §8 that conflicts with the SPEC
§5.2.2 table footnote.

### 3.5 Plugin renderer integration (deferred to P04)

- No `crates/unblock-plugin/data/catalogue.json` consumption in P01.
- The CI catalogue-drift test (SPEC §7.2 / AR-4) is **scaffolded** in
  P01 but is a no-op because the consumer side does not exist yet. It
  becomes load-bearing in P04.

### 3.6 GitLab provider (deferred to v1.1)

- No `POST /webhooks/gitlab` endpoint.
- GitLab as an OAuth provider can be wired in P01 (one of two values for
  `auth.users.primary_provider`), but no GitLab event ingestion ships.

### 3.7 AST CLI and plugin renderer

- P03 (`unblock-code`) and P04 (`unblock-plugin`) are entirely separate
  phases. P01 produces no Rust code under `crates/`.

---

## 4. High-Level Task Breakdown

These are coarse-grained tracks, not bd beads. The phase **spec** turns
each track into the JSON-schema-locked task list that `/tasks` writes
into bd.

### 4.1 Track A — Foundations (no internal dependencies)

| ID | Task | Owner |
|---|---|---|
| A-1 | Initialise Encore app at `apps/api/encore.app` (Go module, build, dev-loop sanity) | go-supervisor (Greta) |
| A-2 | Bootstrap migration: `pgcrypto`, `pg_trgm`, eight schemas declared (per SPEC §9.4.0) | go-supervisor (Greta) |
| A-3 | Migrations §9.4.1–§9.4.8 in canonical order | go-supervisor (Greta) |
| A-4 | `pkg/rbac` typed query helper + per-service DB binding | go-supervisor (Greta) |
| A-5 | Tracing + JSON-Lines logging scaffold (NFR-12) — ULID `trace_id` minted at MCP entry, propagated via `context.Context` per SPEC §10.2 Option B (no `X-Unblock-Trace-Id` header) | go-supervisor (Greta) |
| A-6 | CI workflow + lint gates + NFR-1 latency harness scaffold + NFR-2 RBAC suite scaffold (per-language gates wired into `.github/workflows/`) | infra-supervisor (Olive) |

### 4.2 Track B — auth + org (depends on A-1, A-3, A-4)

| ID | Task | Owner |
|---|---|---|
| B-1 | `auth` service: `Validate`, `ExchangeOAuthCode`, API key issuance + validation, session lifecycle | go-supervisor (Greta) |
| B-2 | `org` service: orgs/projects CRUD, role bindings, `Authorize` | go-supervisor (Greta) |
| B-3 | RBAC regression suite for `auth` + `org` surfaces | go-supervisor (Greta) |

### 4.3 Track C — workitems + deps (depends on B-1, B-2)

| ID | Task | Owner |
|---|---|---|
| C-1 | `workitems` service: items + comments + labels + milestones; CRUD private RPCs | go-supervisor (Greta) |
| C-2 | `deps` service: edges, cycle detection (depth-counter CTE ≤ 256 + per-project advisory lock) at write time | go-supervisor (Greta) |
| C-3 | Cascade subsystem: `deps.cascade.requested` topic, subscriber that maintains `is_ready` + `pipeline_stage`, `deps.cascade_events` idempotent insert (AR-11) | go-supervisor (Greta) |
| C-4 | Atomic claim transaction (SPEC §5.5) | go-supervisor (Greta) |
| C-5 | `pipeline_stage` derivation table integration tests (SPEC §5.7.1) | go-supervisor (Greta) |
| C-6 | RBAC regression suite extended to `workitems` + `deps` | go-supervisor (Greta) |

### 4.4 Track D — MCP server + 14 tools (depends on Track C)

| ID | Task | Owner |
|---|---|---|
| D-1 | `mcp` service skeleton: Streamable HTTP transport (`POST /mcp` + `GET /mcp` per MCP 2025-06-18 spec) using `github.com/modelcontextprotocol/go-sdk`, Bearer API key auth via `auth.Validate` (HMAC-SHA256 lookup per §9.4.6), tool registry, structured JSON-RPC error envelope, `mcp.tool_calls` audit row per call | go-supervisor (Greta) |
| D-2 | Tools 1–4: `prime`, `ready`, `claim`, `create` | go-supervisor (Greta) |
| D-3 | Tools 5–8: `update`, `close`, `show`, `list` | go-supervisor (Greta) |
| D-4 | Tools 9–10: `search`, `comment` | go-supervisor (Greta) |
| D-5 | Tools 11–12: `add_dependency`, `remove_dependency` | go-supervisor (Greta) |
| D-6 | Tools 13–14: `set_state` (structural invariants only — §3.4), `get_state` | go-supervisor (Greta) |
| D-7 | `apps/api/mcp/catalogue.json` v0 (P01 tools, no BLOCK conditions yet) + `go generate` + `catalogue.gen.go` committed | go-supervisor (Greta) |

### 4.5 Track E — Exit-criterion harness + ops

| ID | Task | Owner |
|---|---|---|
| E-1 | Seeder CLI under `apps/api/cmd/unblock-seed/` (per §6 Q3) | go-supervisor (Greta) |
| E-2 | NFR-1 latency harness: warm-cache `prime → ready → claim` p99 < 2 s integration test | go-supervisor (Greta) |
| E-3 | NFR-2 RBAC regression suite — release-blocking gate | go-supervisor (Greta) |
| E-4 | End-to-end exit-criterion test: agent authenticates, completes `prime → ready → claim → close`, cascade fires, second agent observes the new ready set, cycle attempt is rejected | go-supervisor (Greta) |

### 4.6 Inter-track dependencies (summary)

```
A (foundations)
  ├─► B (auth + org)
  │     └─► C (workitems + deps + cascade + claim)
  │           └─► D (mcp + 14 tools)
  │                 └─► E (exit-criterion harness + gates)
  └─► CI/lint gates (run on every track's PRs)
```

A and B can start in parallel after A-2 lands (the bootstrap migration).
C blocks until B exposes `Identity` resolution. D blocks until C
exposes the workitems + deps RPCs. E blocks until D ships D-1 + D-2 at
minimum.

---

## 5. External Dependencies and Research Required

These items must be validated by **Smith (research)** under `/research`
before P01 implementation begins. Each one is a P01 assumption that
cannot be verified by reading internal docs alone.

### 5.1 Encore Go capabilities (R-P01-1 through R-P01-4)

| ID | Question | Why it matters |
|---|---|---|
| R-P01-1 | Encore Go `Pub/Sub` typed-topic API at the version we will pin: does `at-least-once delivery` give us a `delivery_id` we can use for AR-11 idempotency, or do we need to hash the payload? | The cascade subscriber's idempotent insert depends on this. SPEC §5.4 / AR-11 references "the delivery id is propagated from the Pub/Sub envelope". Confirm the SDK exposes it. |
| R-P01-2 | Encore Go `sqldb.Database` migration runner: can it run cross-schema migrations in our ordering (§9.4.0), and how does it handle `pgcrypto` + `pg_trgm` extension declarations? | Migration order must match SPEC §9.4.0. If Encore's runner reorders or batches, we must adapt. |
| R-P01-3 | MCP transport stack: `modelcontextprotocol/go-sdk` (Go SDK, separate from Rust `rmcp`) over Streamable HTTP per MCP spec 2025-06-18 — do we use the SDK directly, or roll the JSON-RPC framing ourselves? | The MCP transport is the only public agent-facing endpoint. **CONTRADICTED in research C3+C6**: rmcp is Rust-only; SSE is the deprecated transport. Architectural fix in Round 2 (Ada). |
| R-P01-4 | Encore Cloud free-tier ceilings (per AR-13): connection cap, Pub/Sub rate, cold-start behaviour. Are the documented ceilings compatible with the NFR-1 budget on a synthetic warmer? | If the free-tier cold-start outliers are seconds, NFR-1 measurement methodology must explicitly carve them out. |

### 5.2 Postgres mechanics (R-P01-5 through R-P01-7)

| ID | Question |
|---|---|
| R-P01-5 | Cycle-detection CTE depth bound. **Research C5 CONTRADICTED the original `LIMIT 256` proposal** — `LIMIT` inside a recursive term has undocumented PG semantics ("not recommended" per PG manual). Replaced with explicit depth counter (`WHERE depth < 256`). SPEC §9.4.9 + AR-8 updated. |
| R-P01-6 | `SELECT FOR UPDATE` behaviour under Encore's connection pooler: does it preserve transactional locking semantics, or does the pooler ever break the transaction? (Pgbouncer transaction pooling vs. session pooling matters here.) |
| R-P01-7 | `tsvector` GIN performance over `workitems.items.title + body` plus `workitems.comments.body` for the `search` tool at the v1 fixture size — is there a multi-table FTS pattern Encore prefers (materialised view vs. per-table indices joined at query time)? |

### 5.3 MCP wire protocol (R-P01-8)

| ID | Question |
|---|---|
| R-P01-8 | MCP wire protocol version compatibility for Claude Code, GitHub Copilot CLI, Cursor, and a vanilla Anthropic SDK harness — confirm the JSON-RPC error envelope shape we need to emit, and the Streamable HTTP keep-alive / reconnect semantics on Encore Cloud's edge proxy. **Research C6 confirmed Streamable HTTP per 2025-06-18 spec** — SSE is the deprecated transport. Round 2 (Ada) updates the transport choice. |

### 5.4 OAuth + API key (R-P01-9, R-P01-10)

| ID | Question |
|---|---|
| R-P01-9 | GitHub OAuth2+PKCE: required scopes for v1.0 (read user, read repo metadata for the future P02 webhook subscription, no write at v1.0)? Is there a recommended PKCE flow library in Go we should pin? |
| R-P01-10 | API key format and storage. **Research C7 CONTRADICTED the original argon2id choice** — argon2id is the wrong primitive for 256-bit-entropy random keys (brute force is mathematically infeasible regardless of hash speed; the ~50ms per-call cost would breach NFR-1). Replaced with **HMAC-SHA256(server_secret, key)** stored as `bytea` raw 32 bytes. Server secret rotates via Encore secret swap; lookup by `key_prefix` first, then HMAC compare. Rotation: no auto-rotation in v1.0 — manual revoke (`revoked_at`) + new key issuance. SPEC §9.4.6 updated. |

These ten research items become **R-P01-1 through R-P01-10** in
`docs/research/01-research-backend-mvp.md` (Smith's output, gating the
phase **spec**).

---

## 6. Open Questions for the User

Each must be resolved before the spec is written. Defaults are documented
but not assumed.

### Q1. Layer-1 state-transition validator scope in P01

**Question.** PRD §8 P02 says "attempts to mark `done` without the
required comment trail are rejected at the MCP boundary". SPEC §5.2.2
table calls `set_state` "gated by Layer-1 BLOCK conditions". This plan
interprets the latter as forward-pointing — i.e. P01 ships `set_state`
and `close` with **structural-only** invariants (no comment-trail
checks); the comment-trail-driven Layer-1 BLOCK conditions ship in P02.

Do you confirm this deferral? Alternative: ship Layer-1 BLOCK conditions
in P01 (widens the phase substantially — adds the BLOCK condition
schema build-out from SPEC §7.5, the catalogue authoring, the codegen
target, and the unit test matrix per transition).

**Default if you say nothing:** defer per this plan (matches PRD §8
verbatim).

**Resolved 2026-05-08:** **CONFIRMED** — Layer-1 BLOCK conditions defer
to P02. P01 ships `set_state` + `close` with structural-only invariants
(FK presence, enum bounds). P02 adds the comment-trail-driven validation
+ `catalogue.json` BLOCK-conditions section + `catalogue.gen.go` codegen.

### Q2. All 8 schemas in P01 vs. SPEC §11 traceability

**Question.** SPEC §11 says P01 migrations are §9.4.1–§9.4.4 + §9.4.6
+ §9.4.8 (six of eight schemas). This plan proposes laying down **all
eight** in P01 — the two extras being `providers` (§9.4.5) and `boards`
(§9.4.7) — because partial migrations couple to phase order while
§9.4.0's migration order is independent. The services for `providers`,
`boards`, and `memory` remain empty in P01 per §2.1.

Confirm? Alternative: stick to SPEC §11 verbatim and add `providers` +
`boards` migrations in P02. (If we do that, we need to confirm no P01
table has an FK pointing into those schemas — which appears to be true,
but should be re-checked against §9.4.)

**Default if you say nothing:** all 8 schemas in P01 per this plan.

**Resolved 2026-05-08:** **CONFIRMED** — all 8 schemas migrate in P01.
Services for `providers`, `boards`, `memory` remain empty stubs in P01
and grow code in P02 (providers + memory) and P05 (boards).

### Q3. API key issuance UX in P01 (no web UI)

**Question.** P01 has no Astro frontend, so there is no human-driven UX
for issuing the first API key. Three options, in order of effort:

1. **Encore local dashboard only** — operators issue keys via Encore's
   built-in admin UI on dev/staging; production ops opens an Encore
   shell. Lowest effort.
2. **Tiny Go CLI seeder under `apps/api/cmd/unblock-seed/`** — issues a
   bootstrap user + org + project + API key from a config file. Useful
   for the exit-criterion harness anyway. **Plan default.**
3. **A hidden public endpoint guarded by a one-time bootstrap secret** —
   rejected as it widens the FR-12 public surface (only two endpoints
   allowed at v1.0).

**Default if you say nothing:** option 2.

**Resolved 2026-05-08:** **CONFIRMED** — option 2: tiny Go CLI seeder at
`apps/api/cmd/unblock-seed/`. Reusable by the exit-criterion harness;
ops-repeatable; no public-surface widening.

### Q4. `mcp.meta_catalogue` and `verify_can_transition` in P01 vs. P02

**Question.** SPEC §5.2.2 / §11 places `mcp.meta_catalogue` v1 and
`verify_can_transition` v1 in **P02** as "operational primitives". This
plan respects that placement. However, the `apps/api/mcp/catalogue.json`
**source file** ships in P01 (with the 14 P01 tools' tool definitions
and a placeholder for the BLOCK-conditions section) so that the codegen
target and the CI scaffold exist from day one.

Confirm the file ships in P01 but the `mcp.meta_catalogue` MCP-level
endpoint waits until P02? Alternative: ship `mcp.meta_catalogue` in P01
too (cheap; just exposes the file via the SSE channel) — at the cost of
locking the wire shape before P02 has a chance to add the BLOCK
conditions section.

**Default if you say nothing:** file in P01, MCP endpoint in P02.

**Resolved 2026-05-08:** **CONFIRMED** — `apps/api/mcp/catalogue.json`
v0 (the 14 P01 tools, no BLOCK conditions section) ships in P01. The
`mcp.meta_catalogue` SSE endpoint and the `apps/api/mcp/catalogue.gen.go`
codegen target wait until P02, after the BLOCK-conditions section is
authored.

### Q5. Supervisor mapping for P01

**Question.** This project's CLAUDE.md lists `rust-supervisor` and
`infra-supervisor`. The Go backend at `apps/api/` does not match
`rust-supervisor`. SPEC §6.8 mentions **Greta** (Go / Encore) as a
dynamic supervisor. Is `infra-supervisor` the right owner for **all**
Go work in P01, or is there a `go-supervisor` / `Greta` to be
provisioned via `/add-supervisor` before tasks are dispatched?

This plan tags every P01 task as `infra-supervisor` as a placeholder
because Greta is not yet provisioned in this repo.

**Default if you say nothing:** infra-supervisor owns all Go work in
P01; provisioning Greta is itself a P00 ops task we surface separately.

**Resolved 2026-05-08:** **PUSH BACK ON DEFAULT — provision now, not
deferred.** Run `/setup` (Daphne) immediately to provision all four
dynamic supervisors named in `CLAUDE.md` § Supervisors and SPEC §6.8:

  - **Greta** — Go (Encore Go services in `apps/api/`). Owns all Go
    application code in P01: A-1, A-2, A-3, A-4, A-5, B-1, B-2, B-3,
    C-1 through C-6, D-1 through D-7, E-1, E-2, E-3, E-4.
  - **Aria** — TypeScript / Astro / line-ui (`apps/web/`). Idle in P01;
    becomes active in P05.
  - **Neo** — Rust (`crates/`, both `unblock-code` and `unblock-plugin`).
    Idle in P01; becomes active in P03 (AST CLI) and P04 (plugin
    renderer).
  - **Olive** — Infrastructure / CI-CD. Owns the CI gate scaffolding
    in P01: A-6 (CI workflow + lint gates + NFR-1 latency harness
    scaffold + NFR-2 RBAC suite scaffold). Becomes more active in P02
    (Encore deploy, secrets management).

After `/setup` provisions Daphne's render of the four supervisors into
`.claude/agents/`, all P01 task tags in §4 above are authoritative.
`infra-supervisor` is dropped from this plan.

### Q6. Production hosting target for P01

**Question.** PRD §10.1 says Encore Cloud free tier. Is there a
**staging** environment we want at P01 close, or is "the local Encore
emulator + the exit-criterion harness pass" the bar for P01 acceptance?
A staging environment changes the work breakdown (adds infra/encore
deploy config, secrets, DNS).

**Default if you say nothing:** P01 acceptance is on the local Encore
emulator + CI; first deploy to Encore Cloud is a P02 ops task.

**Resolved 2026-05-08:** **CONFIRMED** — P01 acceptance is on the local
Encore emulator + CI green. Encore Cloud staging deploy is a P02 ops
task owned by Olive.

### Q7. `prime`'s "recent cascade events" surface in P01

**Question.** `prime` returns "recent cascade events" per SPEC §5.2.2.
P01 has the `deps.cascade_events` table populated by the cascade
subscriber, so the data is there. What is "recent"? Last 50 events for
the agent's org/project, ordered by `triggered_at desc`? Or
"events since the agent's last `prime` call" (which requires tracking
per-agent last-seen timestamps)?

**Default if you say nothing:** last 50 events per agent's org/project.
Per-agent last-seen tracking is a P02 enhancement.

**Resolved 2026-05-08:** **CONFIRMED** — `prime` returns the last 50
`deps.cascade_events` rows scoped to the agent's org/project, ordered
by `triggered_at desc`. Per-agent last-seen tracking is deferred to P02.

---

## 7. Risks Specific to P01

These are P01-level risks (SPEC §13 covers architecture-wide risks; PRD
§12 covers product-wide risks).

| # | Risk | Mitigation |
|---|---|---|
| RP01-1 | **NFR-1 (`prime → ready → claim` < 2 s p99) misses on the local emulator harness.** The materialised `is_ready` plus the `claimed_by_id IS NULL` filter are O(index lookup); the budget is reachable, but only if `prime` does not fan out to N+1 RPCs. | E-2 wires the latency harness early so `prime` is profiled while D-2 is being written. If we miss, we batch the RPCs out of `prime` into one `workitems` private RPC that returns the bundle. |
| RP01-2 | **Cascade subscriber idempotency regression.** AR-11 promises the subscriber is idempotent; a sloppy implementation that flips `is_ready` twice or double-counts `cascade_events` rows is a Law 1 violation. | Property test: re-deliver every test event twice; assert post-state is byte-identical. The `(delivery_id, triggered_by_item_id)` uniqueness check is asserted at the DB level. |
| RP01-3 | **Cycle detection CTE depth-counter cap (256) refuses a legitimate non-cyclic 257+ node chain at v1 scale.** AR-8 sets the cap; a 200-node legitimate chain plus a new edge-write would error misleadingly. | Document the cap as a v1 product constraint; surface it in the `add_dependency` error envelope; revisit based on the first-month data. |
| RP01-4 | **Encore SSE long-lived connection drops under Encore Cloud edge proxy.** Free-tier proxies often kill idle connections. Agent reconnect logic must handle this gracefully. | R-P01-3 / R-P01-8 close this in research before D-1 starts. Heartbeat ping every 15s; client reconnect on close. |
| RP01-5 | **API key entropy / storage chosen wrong.** Once the key format is locked, rotating to a different format is a public-surface migration. | R-P01-10 closes this in research; the spec pins the format before D-1 ships. |
| RP01-6 | **`pkg/rbac` typed query helper is bypassable by a future supervisor.** If a contributor hand-rolls a SQL query against another service's schema, RBAC silently regresses. | Encore's per-service DB binding makes cross-schema reads compile-fail; CI lint enforces no `WithDB(otherSchema)` calls outside the owner. |
| RP01-7 | **Migration ordering drift between P01 and P02.** If P02 needs to add a column to a P01 schema, naive migration ordering can break the `providers` → `workitems` FK direction. | P01 spec pins the migration filename convention; P02 plan extends it without renumbering. |

---

## 8. Acceptance Criteria for P01

This phase is **DONE** when all of the following are demonstrably true.

### 8.1 Functional acceptance (PRD §8 P01 exit criterion)

- [ ] An agent authenticates via `Bearer <api-key>` against the Streamable HTTP MCP endpoint (`POST /mcp` for tool calls; `GET /mcp` for server-initiated SSE).
- [ ] The agent calls `prime` and receives a non-empty ready set summary,
      claimed-by-me list (initially empty), and recent cascade events
      list (initially populated by the seeder).
- [ ] The agent calls `ready --limit 1` and receives one item ordered
      deterministically.
- [ ] The agent calls `claim` on that item and is granted the claim;
      a second concurrent agent receives a structured "already claimed"
      error.
- [ ] The agent calls `close` on the claimed item; cascade fires; a
      newly-unblocked dependent flips `is_ready=true`; the next `prime`
      reflects it.
- [ ] An attempt to add a cycle-creating edge via `add_dependency` is
      rejected at write time with a structured error pointing at the
      offending chain.

### 8.2 Non-functional acceptance

- [ ] **NFR-1.** `prime → ready → claim` p99 < 2 s on the warm-cache
      harness against the seeded fixture.
- [ ] **NFR-2.** RBAC regression suite green; zero cross-tenant leaks
      across every P01 read and write surface.
- [ ] **NFR-5.** Cycle creation is rejected at write time, structurally;
      no read-time cycle detection path is reachable.
- [ ] **NFR-9.** Decoupled deliverables share no runtime state — there
      is no Rust code under `crates/` shipping with P01.
- [ ] **NFR-10.** Greta gate set (`go test ./... -race`, `go vet`,
      `golangci-lint run --max-warnings 0`, Encore client diff zero)
      green on every PR; release CI green on the P01 close commit.
- [ ] **NFR-12.** All logs are JSON Lines on STDERR; STDOUT carries only
      MCP envelopes; manual inspection on the exit-criterion harness
      confirms the separation.

### 8.3 Architectural invariants

- [ ] All eight Postgres schemas exist with the canonical DDL from SPEC
      §9.4.1–§9.4.8 (subject to §6 Q2).
- [ ] `is_ready` and `pipeline_stage` are materialised by exactly one
      writer (the cascade subscriber); integration test asserts no other
      code path UPDATEs either column.
- [ ] `deps.cascade_events` insert is idempotent on re-delivery;
      property test green.
- [ ] Atomic claim is a single transaction with `SELECT FOR UPDATE`;
      property test runs N=100 concurrent claims and asserts exactly one
      winner.
- [ ] Manifesto Laws covered in P01 (L1, L2, L3-foundations, L5, L7) are
      structurally present, not discipline-only — each invariant is
      backed by at least one regression test.

### 8.4 Documentation

- [ ] `docs/specs/01-spec-backend-mvp.md` is APPROVED before
      implementation starts.
- [ ] `docs/research/01-research-backend-mvp.md` closes R-P01-1 through
      R-P01-10 with verified findings before the spec is approved.
- [ ] README.md updated with P01 user surface (MCP Bearer auth, the 14
      tools' one-liners, the seeder CLI invocation).
- [ ] AGENTS.md / CLAUDE.md updated to reflect the Go backend reality
      (Greta provisioning if §6 Q5 lands that way; Rust v1 archive note).

---

## 9. Sequencing Notes

- **`/research` runs first** to close R-P01-1 through R-P01-10.
  Findings update or contradict §2 / §3 of this plan; the plan is
  re-approved if anything load-bearing flips.
- **`/spec` runs after research approval** and produces
  `docs/specs/01-spec-backend-mvp.md` with the JSON-locked tool
  signatures, RPC signatures, error envelopes, migration files, and the
  `apps/api/mcp/catalogue.json` v0 content.
- **`/tasks` runs after spec approval** and emits bd beads onto the
  graph in the dependency order of §4.6.
- **`/do` per supervisor** runs the work, gated by the per-track
  dependency edges.

---

## 10. Reference Anchors

- PRD §1, §4 (US-1 through US-9), §5.1 (FR-1 through FR-13), §8 (P01
  exit criterion), §10 (deps + constraints), §11 (M-1 through M-5).
- SPEC §3.1 (component diagram), §5.2 (8-service decomposition), §5.2.2
  (the 14 P01 tools), §5.3 (public surface), §5.4 (cascade), §5.5
  (atomic claim), §5.6 (RBAC), §5.7 (state machine, P02 deferral),
  §5.7.1 (`pipeline_stage` derivation), §9.4.0–§9.4.10 (canonical DDL),
  §11 (P01 traceability), §13 AR-8 / AR-11 / AR-13.
- Manifesto Laws L1, L2, L3, L5, L7 (covered in P01); L4 (foundations
  laid, full enforcement at P05); L6 (no Rust in P01); L8 (Layer 1 P02,
  Layers 2 + 3 P04).
