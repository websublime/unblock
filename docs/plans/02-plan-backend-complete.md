# PLAN: P02 — Backend Complete (providers + memory tools + Layer-1 pipeline enforcement)

**Status:** APPROVED *(2026-06-16 — review-driven drift/gap reconciliation applied 2026-06-16 (23 confirmed findings from an adversarial plan×PRD×SPEC×code sweep). User decisions Q1–Q6 resolved §6; the SPEC §11 P02/P05 traceability patch is applied on main, not pending. Previously DRAFT 2026-06-16.)*
**Author:** Ada (architect)
**Date:** 2026-06-16
**Source PRD:** [docs/PRD.md](../PRD.md) (APPROVED, 2026-05-07)
**Source SPEC:** [docs/SPEC.md](../SPEC.md) (APPROVED 2026-05-07, round-6 cascade-symmetry sync applied 2026-05-12)
**Companion:** [docs/MANIFESTO.md](../MANIFESTO.md) (APPROVED, 2026-05-07)
**Predecessor plan:** [docs/plans/01-plan-backend-mvp.md](./01-plan-backend-mvp.md) (APPROVED)

> Stage 2 deliverable. This is a **phase plan**, not an implementation
> spec. It defines what is in/out of P02, the high-level work breakdown,
> internal and external dependencies, the external APIs and assumptions
> that must be validated by research, and the acceptance criteria.
> Implementation contracts (exact JSON schemas per MCP tool, exact RPC
> signatures, exact error envelopes, exact migration files, the
> `catalogue.json` BLOCK-conditions section, the GitHub normalisation
> field map) land in the phase **spec** (`docs/specs/02-spec-backend-complete.md`),
> authored under `/spec` once this plan is APPROVED **and** `/research`
> closes the open assumptions in §5.

---

## 1. Phase Goal

P02 completes the agent-facing backend. It takes the headless core stood
up in P01 — work-item CRUD, the dependency graph, atomic claim, the
comment trail, structured state columns, and the 23-tool Streamable HTTP
MCP transport — and adds the three pillars that make it *complete* per
PRD §8:

1. **Provider integration (`providers` service, FR-11).** A GitHub
   repository can be linked; the public `POST /webhooks/github` endpoint
   ingests HMAC-verified webhook events, deduplicates them, and
   normalises them into canonical `workitems.items`; bidirectional sync
   propagates `://unblock` changes back to GitHub Issues; a scheduled
   reconciler closes the gap when webhooks are missed (Law 3).
2. **Memory service completion (FR-13).** The four memory MCP tools
   (`remember`, `recall`, `memories`, `forget`) ship, backed by the
   always-on secret sanitiser (NFR-7), raising the agent-facing MCP tool
   surface from **23 (P01) to 27**.
3. **Pipeline enforcement Layer 1 (FR-14, Manifesto Law 8).** The MCP
   state-transition validator goes live: every state-mutating tool call
   is gated by the explicit preconditions in PRD §6.7, encoded in
   `catalogue.json` BLOCK conditions, codegen-compiled into the Go
   validator, and exposed for re-validation via `verify_can_transition`.

P02 exit criterion (verbatim from PRD §8 + SPEC §11): *a GitHub
repository can be linked, webhooks normalise events into canonical work
items, and an attempt to mark a work item `done` without the required
comment trail is rejected at the MCP boundary.* SPEC §11 extends this:
*`mcp.meta_catalogue` returns the live catalogue.json; `verify_can_transition`
validates a candidate transition against the same Layer-1 validator.*

**Explicitly NOT in P02:** the Reactive Agent Environment /
`unblock-agentic` work described in
`docs/research/agentic-research/RFC-REACTIVE-AGENT-ENVIRONMENT.md`. That
RFC tags itself as **P02+/additive**, not P02. It is out of scope for
this phase (see §3.7) and neither this plan nor the P02 spec depend on
it.

---

## 2. Scope — What is IN P02

### 2.1 Encore Go services to complete

Per SPEC §5.2 (8 services × 8 schemas, 1:1 mapping locked). P01 stood up
`auth`, `org`, `workitems`, `deps`, and `mcp` with live code, and laid
down **all eight schemas' DDL** (P01 plan §2.1 + Q2 CONFIRMED). P02 fills
in the two services left as schema-only stubs that the P02 exit criterion
requires:

| Service | Surface delivered in P02 | Notes |
|---|---|---|
| `providers` | Full service code: `LinkRepo`, `Sync`, `Reconcile` private RPCs; the public `POST /webhooks/github` handler (HMAC-verified); the webhook normaliser (GitHub issue/PR event → canonical `workitems.items` via `providers.mappings`); the bidirectional sync writer (`://unblock` mutation → GitHub REST/GraphQL); the reconciliation cron (Law 3); the per-row payload sanitiser + 90-day digest cron (§9.4.5 retention policy). | Schema (`providers.installations`, `providers.events`, `providers.mappings`) already migrated in P01. P02 adds **service code only**, plus any *additive* migration the digest/retention job needs (see §2.5). GitLab is explicitly v1.1 (§3.6). |
| `memory` | Full service code: `Remember`, `Recall`, `List`, `Forget` private RPCs; the four MCP tool facades (`remember`/`recall`/`memories`/`forget`); the always-on secret sanitiser (NFR-7); the `pgcrypto` encrypt-at-rest path (`value_enc`); the `ts_doc` tsvector build (sanitise-before-tokenise per AR-10); tag + FTS query surface (the `entries_key_trgm_idx` trigram index, SPEC §9.4.8, is a forward-looking DDL affordance with no v1.0 tool consumer). | Schema (`memory.entries`, `memory.entry_refs`) already migrated in P01. P02 adds **service code** plus the **`memory.sanitiser_events` audit table** (AR-14 — explicitly "added in P02"; new additive migration). |

Services that change in P02 without being net-new:

| Service | What changes in P02 |
|---|---|
| `mcp` | (a) **+4 memory tools** (24–27), facades over `memory.*` RPCs. (b) **Layer-1 state-transition validator** goes live: `set_state`, `claim`, `close` gain the comment-trail-driven BLOCK conditions deferred from P01 (P01 plan §3.4 / Q1). (c) **`catalogue.json` BLOCK-conditions section** authored (§5.7 / §7.5); `go generate` now emits a **load-bearing** `catalogue.gen.go` validator (P01 shipped a placeholder section). (d) **`mcp.meta_catalogue` v1** and **`verify_can_transition` v1** operational primitives go live (SPEC §5.2.2 — both were deferred to P02). |
| `workitems` / `deps` | No net-new surface, but P02's Layer-1 validator reads the comment trail (`workitems.comments`) and the four state columns — confirm the P01 `GetTrail` / `get_state` RPCs expose everything the validator needs without a new private RPC. (Validated in research, see §5.) The webhook normaliser writes through the existing `workitems.Create` / `Update` RPCs — no direct cross-schema writes (Law 6 / RBAC §5.6). |

### 2.2 The MCP tool surface: 23 → 27 (SPEC §5.2.2)

P02 adds the four memory tools. The P01 23-tool inventory is unchanged.

| # | Tool | P02 contract notes |
|---|---|---|
| 24 | `remember` | Write a scoped memory entry (`org`/`project`/`user`). Secret sanitiser runs **before** encryption (NFR-7); 8 KB `value_size` cap (DDL CHECK, §9.4.8); per-(scope, key) uniqueness enforced by the partial unique indexes. Records a `memory.sanitiser_events` row on every sanitiser hit (AR-14). |
| 25 | `recall` | Read entries by scope + key; supports tag and full-text (`ts_doc` GIN) filters. Decrypt-on-read; never decrypt-on-search (AR-10). RBAC-scoped. Whether expired rows (`memory.entries.expires_at`) are filtered at read time is resolved by R-P02-13 / B-6. |
| 26 | `memories` | List entries by scope with pagination; cheap dashboard read (powers `prime`'s `memory_hints` field, which was empty in P01 — P01 plan §3.2). Whether expired rows (`expires_at`) are filtered is resolved by R-P02-13 / B-6. |
| 27 | `forget` | Soft-delete an entry (audit-trail preserved; the exact soft-delete column lands in the spec — DDL today has no `deleted_at`, so this is an additive-column decision, see §5 / R-P02-10). |

> The provider sync tooling promised by PRD FR-8 ("plus the providers/sync
> tooling needed for bidirectional GitHub sync") lives **inside** the
> `providers` private RPC surface, **not** as MCP tools (SPEC §5.2.2). There
> is no agent-facing "sync now" tool at v1.0; sync is automatic on webhook
> receipt and reconciliation is scheduled. The 27-tool count is therefore
> exact and final for v1.0.

### 2.3 Layer-1 pipeline enforcement (FR-14, Law 8 layer 1)

This is the **highest-leverage** P02 deliverable and the one the exit
criterion names directly. The work:

- **Author the `catalogue.json` BLOCK-conditions section** per the typed
  schema in SPEC §7.5.1, one or more `block_conditions` objects per
  transition at JSON path `.transitions[].block_conditions`. The
  transition set is the PRD §6.7 state machine table (every row becomes a
  transition object). Ada keeps the PRD §6.7 human table and the JSON in
  sync; the CI drift test (SPEC §7.2) enforces equality mechanically.
- **Codegen the Go validator** — `go generate` reads
  `apps/api/mcp/catalogue.json` and emits `apps/api/mcp/catalogue.gen.go`
  (committed; CI fails on a `go generate` diff). P01 scaffolded this with
  a placeholder section; P02 makes it load-bearing.
- **Gate `set_state` / `claim` / `close`** with the generated validator.
  This *replaces* the P01 structural-only relaxation (P01 plan §3.4):
  - `close` now requires `qa_state=passed` (or the override path), not
    just `claimed_by_id IS NOT NULL`.
  - `set_state` transitions now check the comment trail (e.g.
    `qa_state→passed` requires a `(kind=qa, status=success)` comment;
    `review_state→approved` requires `(kind=review, status=success)`).
  - The exit-criterion case — `done` (via `close` / `set_state(qa_state=passed)`)
    without the required review-then-QA comment trail (`kind=review,
    status=success` then `kind=qa, status=success`, per PRD §6.7) — is
    rejected with a structured `PIPELINE_PRECONDITION_NOT_MET` error citing
    the missing precondition.
- **Ship `verify_can_transition` v1** — a read-only hook-facing primitive
  (not a top-level agent tool; SPEC §5.2.2) that re-validates a candidate
  transition against the **same** generated validator. This is the MCP
  machinery Layer 2 (P04) will call from the `verify-state` hook.
- **Ship `mcp.meta_catalogue` v1** — the read-only catalogue endpoint that
  serves the live `catalogue.json` over the MCP channel for the P04
  build-time renderer to verify against the checked-in copy (AR-4 third
  corner).

> **The five PRD §6.2 column-value invariants already shipped in P01**
> (P01 plan §2.2 Tool 13, round-2 D2): writing `qa_state=failed` requires
> `review_state=approved`; writing `review_state=needs_rework` resets
> `qa_state=pending`; etc. Those are pure column-value rules with no
> comment-trail dependency. **P02 adds the comment-trail-driven gates on
> top.** The spec must reconcile the two layers so a transition is not
> double-validated with conflicting messages.

### 2.4 Provider integration (FR-11, Law 3)

The `providers` service is the largest net-new code surface. Sub-areas:

- **Webhook ingestion** — `POST /webhooks/github` (public, FR-12). Verify
  the per-installation HMAC signature (`webhook_secret_enc`, §9.4.5);
  insert into `providers.events` with the `(provider, delivery_id)`
  dedup constraint (AR-12); return `200 OK` on a recognised duplicate so
  GitHub stops retrying; the per-row payload sanitiser redacts emails /
  credential patterns **on insert** (§9.4.5 retention policy, first
  layer).
- **Normalisation** — map a GitHub `issues.*` / `pull_request.*` event
  into a canonical `workitems.items` create/update via the existing
  `workitems` RPCs, recording the mapping in `providers.mappings`
  (`provider_kind ∈ {issue, pull_request}`, external-id uniqueness). The
  exact field map (GitHub issue → canonical item: title, body, labels,
  state, assignee, milestone) is a **spec-level contract**; the plan only
  asserts it exists and is research-validated (§5).
- **Bidirectional sync** — opt-in per installation. A `://unblock`
  mutation on a mapped item propagates back to the GitHub Issue via REST /
  GraphQL. The loop-prevention strategy (avoid webhook→sync→webhook
  storms) is a research item (§5 / R-P02-3).
- **Reconciliation cron** (Law 3) — a scheduled job that detects drift
  (`providers.mappings.drift_detected_at`) and reconciles when webhooks
  were missed or GitHub was offline. Cadence + scope is a spec contract.
- **Payload retention** — the 90-day digest cron (§9.4.5) that replaces
  raw `providers.events.payload` with a metadata-only digest. The exact
  digest schema + redactor pattern set land in the P02 spec (§9.4.5 says
  so verbatim).

### 2.5 Cross-cutting machinery in P02

- **Additive migrations only.** P02 does **not** re-run §9.4.5 / §9.4.7 /
  §9.4.8 (all eight schemas already migrated in P01 — Q2 CONFIRMED). P02
  adds *forward* migrations for: (a) `memory.sanitiser_events` (AR-14);
  (b) any `memory.entries` soft-delete column `forget` needs (R-P02-10 / B-4);
  (c) anything the providers digest/retention job requires beyond the
  shipped §9.4.5 DDL. Per `feedback_migration_edit_drift`: **never edit an
  applied P01 migration in place** — P02 migrations are new, higher-numbered
  files.
- **Encore Pub/Sub + Cron.** P02 introduces the providers reconciliation
  cron, the payload-digest cron, and (per AR-14) the memory sanitiser
  periodic re-scan job. Confirm whether the `provider.events` Pub/Sub
  topic named in CLAUDE.md is needed, or whether webhook→normalise is
  synchronous within the handler (research / spec decision).
- **Encore Cloud deploy + secrets (Olive).** P02 is the first phase that
  deploys to Encore Cloud staging (P01 plan Q6: "Encore Cloud staging
  deploy is a P02 ops task owned by Olive"). This brings: the GitHub
  webhook secret + the provider write/webhook credential (shape pinned by
  R-P02-4 — App ID + private-key PEM under the GitHub-App working
  assumption, OAuth-app client-id/secret/redirect-uri under the fallback)
  into Encore secrets; the
  `pgcrypto` DEK (`MEMORY_DEK`) provisioning; the AR-13 free-tier ceiling
  measurement (Pub/Sub rate, connection cap, cold-start) and the AR-16
  synthetic warmer (`mcp-warmer` cron hitting `meta_catalogue`).
- **RBAC regression suite extension (NFR-2).** P01 shipped the suite for
  its surfaces; P02 **extends it to `providers` and `memory`** (P01 plan
  §2.3 says so explicitly). Memory scope isolation (org/project/user) and
  provider installation org-scoping are new cross-tenant write surfaces.
- **Catalogue drift CI (AR-4) becomes load-bearing.** P01 scaffolded it as
  a no-op; in P02 the BLOCK-conditions section is real, so the Go codegen
  corner and the `mcp.meta_catalogue` live corner must agree. (The Rust
  `include_str!` corner stays inert until P04, but the Go↔live pair is
  active in P02.)
- **Quality gates (NFR-10).** Greta's Go gate set (`encore test ./...`,
  `go vet`, `go fmt`, `encore check`, JSON-tag lint per CLAUDE.md §Coding
  Standards) green on every PR; the JSON wire-tag rule
  (`grep -rnE 'json:"[A-Z]' apps/api/` returns zero) extended to the new
  providers + memory + sanitiser structs.

### 2.6 Repository / infra deliverables

- `apps/api/providers/` service code + the `POST /webhooks/github` wiring
  in `apps/api/public/` (the second of the two FR-12 v1.0 public
  endpoints; P01 wired only `/mcp`).
- `apps/api/memory/` service code + the four MCP tool registrations in
  `apps/api/mcp/`.
- `apps/api/mcp/catalogue.json` BLOCK-conditions section authored;
  `catalogue.gen.go` regenerated and committed.
- New P02 migrations under `apps/api/db/migrations/` (the dedicated
  zero-API `db` migration-owner service per CLAUDE.md Coding Standards;
  the sole `sqldb.NewDatabase("unblock", ...)` owner is `apps/api/db/db.go`)
  — additive only, higher-numbered than the P01 set (which ends at
  `0140_deps_cascade_events_kind_chk_fix`).
- **Repo hygiene (P02-impl obligation).** The `providers/.gitkeep` and
  `memory/.gitkeep` stubs currently cite the stale
  `apps/api/auth/migrations/...` path; they MUST be deleted (once the
  service `.go` files land) or corrected to `apps/api/db/migrations/`
  during P02 implementation.
- `infra/` (Olive): Encore Cloud staging deploy config, GitHub OAuth-app +
  webhook secrets, cron schedules, the AR-13 capacity-measurement gate, the
  AR-16 warmer cron.

---

## 3. Scope — What is OUT of P02

### 3.1 `boards` service code (deferred to P05) — RESOLVED (Q1)

This plan **excludes** boards service code from P02, aligning with: (a) the
user's stated P02 scope (providers + memory tools + Layer-1 only); (b)
the P01 plan §2.1, which defers boards code to **P05**; (c) the fact that
saved-view persistence has no consumer until the Astro web client (P05).
The `boards` schema already migrated in P01; only its *service code* is at
issue.

**RESOLVED 2026-06-16 (Q1): boards service code → P05.** The earlier SPEC
§11 P02 row listed `apps/api/boards` as a P02 component; that was a stale
traceability entry. The SPEC §11 patch is **applied on main** (see §8.4 /
§9): the P02 row drops `apps/api/boards`, the P05 row gains it (service
code only — the §9.4.7 schema already landed in P01). SPEC §11 is once
again the authoritative traceability source.

### 3.2 Reactive Agent Environment / `unblock-agentic` (P02+/additive, NOT P02)

The RFC at
`docs/research/agentic-research/RFC-REACTIVE-AGENT-ENVIRONMENT.md` and the
companion `UNBLOCK-AGENTIC-RUST.md` / `MOZAIK-ARCHITECTURE-REFERENCE.md`
describe a reactive agent environment. The RFC explicitly tags this as
**P02+/additive**, i.e. *after and on top of* P02, not part of it. P02
ships none of it. The P02 backend exposes the MCP + webhook surface that
such an environment would later consume, but no agentic-runtime code,
no reactive-environment types, and no new MCP tools for it ship in P02.

### 3.3 GitLab provider (deferred to v1.1)

- No `POST /webhooks/gitlab` endpoint, no GitLab normaliser, no GitLab
  bidirectional sync. The `providers` schema already supports
  `provider='gitlab'` (the CHECK enum), and the service code should be
  written so GitLab slots in at v1.1 without re-architecture, but **no
  GitLab code ships in P02** (PRD §9.2, SPEC §12.2).

### 3.4 Layer 2 + Layer 3 pipeline enforcement (deferred to P04)

- The `verify-state` plugin hook (Layer 2) and the agent-prompt BLOCK
  conditions (Layer 3) are rendered by `unblock-plugin` in P04. P02 ships
  only the **MCP machinery** they depend on: the Layer-1 validator,
  `verify_can_transition`, and `mcp.meta_catalogue`. The Rust
  `include_str!` catalogue corner (AR-4) stays inert until P04.

### 3.5 Astro web client (deferred to P05, v1.1)

- No Astro app, no Astro Actions, no BFF cookie. `prime`'s `memory_hints`
  field becomes populated in P02 (memory tools ship), but it is consumed
  by agents over MCP, not by a browser.

### 3.6 AST CLI and plugin renderer (P03 / P04)

- No Rust code under `crates/` ships with P02 (NFR-9 — decoupled
  deliverables). P02 is entirely `apps/api/` Go + infra.

### 3.7 New persistent stores / scope inflation

- No Redis, no service-local SQLite, no second database (FR-1, SPEC §12.1).
  All P02 state lives in the existing eight Postgres schemas plus the
  additive P02 migrations of §2.5.

---

## 4. High-Level Task Breakdown

Coarse-grained tracks, not bd beads. The phase **spec** turns each track
into the JSON-schema-locked task list that `/tasks` writes into bd. All
Go work is owned by **Greta** (go-supervisor); all deploy/secrets/CI work
by **Olive** (infra-supervisor).

### 4.1 Track A — Layer-1 enforcement (depends on P01 `mcp` + catalogue scaffold)

| ID | Task | Owner |
|---|---|---|
| A-1 | Author `catalogue.json` `.transitions[].block_conditions` for every PRD §6.7 transition (typed schema SPEC §7.5.1) | go-supervisor (Greta) |
| A-2 | `go generate` → load-bearing `catalogue.gen.go` validator; commit; CI fails on diff | go-supervisor (Greta) |
| A-3 | Gate `set_state` / `claim` / `close` with the generated validator; replace the P01 structural-only relaxation; reconcile with the five P01 column-value invariants (§2.3) | go-supervisor (Greta) |
| A-4 | `verify_can_transition` v1 (hook-facing primitive, same validator) | go-supervisor (Greta) |
| A-5 | `mcp.meta_catalogue` v1 (serve live catalogue.json over the MCP channel) | go-supervisor (Greta) |
| A-6 | Catalogue-drift CI (AR-4) made load-bearing for the Go-codegen ↔ live-`meta_catalogue` pair | infra-supervisor (Olive) |

### 4.2 Track B — Memory service + 4 tools (independent of Track A)

| ID | Task | Owner |
|---|---|---|
| B-1 | `memory` service: `Remember`/`Recall`/`List`/`Forget` private RPCs; `pgcrypto` `value_enc` encrypt-at-rest; `ts_doc` build (sanitise-before-tokenise, AR-10) | go-supervisor (Greta) |
| B-2 | Always-on secret sanitiser (NFR-7) + `memory.sanitiser_events` audit table (AR-14; additive migration) + the periodic re-scan job | go-supervisor (Greta) |
| B-3 | MCP tools 24–27 (`remember`/`recall`/`memories`/`forget`) as facades over `memory.*`; wire `prime.memory_hints` | go-supervisor (Greta) |
| B-4 | `forget` soft-delete column decision + additive migration (R-P02-10) | go-supervisor (Greta) |
| B-5 | RBAC regression suite extended to `memory` (org/project/user scope isolation) | go-supervisor (Greta) |
| B-6 | Resolve `memory.entries.expires_at` semantics (R-P02-13) — either wire it into `remember` (write) + `recall`/`memories` (read-time filter), or document it as inert/reserved for v1.0 (no DDL change) | go-supervisor (Greta) |

### 4.3 Track C — Providers: ingestion + normalisation (depends on P01 `workitems`)

| ID | Task | Owner |
|---|---|---|
| C-1 | `POST /webhooks/github` public handler: HMAC verify, `providers.events` insert + `(provider, delivery_id)` dedup (AR-12), on-insert payload sanitiser, 200-on-duplicate + failure-status contract per R-P02-4b | go-supervisor (Greta) |
| C-2 | `providers.LinkRepo` RPC: create `providers.installations`, store encrypted installation id + webhook secret | go-supervisor (Greta) |
| C-3 | Webhook normaliser: GitHub issue/PR event → canonical `workitems.items` via `workitems` RPCs; record `providers.mappings` | go-supervisor (Greta) |
| C-4 | Payload 90-day digest cron + redactor pattern set (§9.4.5 retention) | go-supervisor (Greta) |

### 4.4 Track D — Providers: bidirectional sync + reconciliation (depends on C-1..C-3)

| ID | Task | Owner |
|---|---|---|
| D-1 | Bidirectional sync writer: `://unblock` mutation → GitHub REST/GraphQL on mapped items; loop-prevention (R-P02-3). Built on the Go GitHub client library pinned by R-P02-3b. | go-supervisor (Greta) |
| D-2 | Reconciliation cron (Law 3): drift detection (`drift_detected_at`) + reconcile on missed webhooks / provider outage | go-supervisor (Greta) |
| D-3 | RBAC regression suite extended to `providers` (installation org-scoping) | go-supervisor (Greta) |

### 4.5 Track E — Infra: Encore Cloud staging + secrets + capacity (Olive)

| ID | Task | Owner |
|---|---|---|
| E-1 | Encore Cloud staging deploy (P01 Q6 deferred this to P02) | infra-supervisor (Olive) |
| E-2 | Secrets: provision the R-P02-4-decided provider credential (App path ⇒ App ID + private-key PEM + per-install installation-id; fallback ⇒ OAuth-app client-id/secret/redirect-uri) + per-install webhook secret + `MEMORY_DEK`. **Note:** under the App path, `apps/api/SECRETS.md`'s table and `apps/api/secrets.nonprod.cue` must gain `GitHubApp*` placeholders (App ID, private-key PEM) — neither exists today (the registry holds OAuth-app secrets only). R-P02-4 is the gate that selects the final set. | infra-supervisor (Olive) |
| E-3 | AR-13 free-tier ceiling measurement (Pub/Sub rate, connection cap, cold-start) + AR-16 `mcp-warmer` cron | infra-supervisor (Olive) |
| E-4 | Cron schedules wired (reconcile, payload-digest, sanitiser re-scan, warmer) | infra-supervisor (Olive) |

### 4.6 Track F — Exit-criterion harness

| ID | Task | Owner |
|---|---|---|
| F-1 | E2E: link a GitHub repo, deliver a synthetic HMAC-signed `issues.opened` webhook, assert a canonical work item is created + mapped; deliver a tampered-signature webhook and assert a 4xx-final response with no `providers.events` insert and no normalisation; deliver a replayed `X-GitHub-Delivery` and assert 200 with no double-create | go-supervisor (Greta) |
| F-2 | E2E: attempt `done` (close / `qa_state→passed`) without the required `kind=review` / `kind=qa` comment trail; assert `PIPELINE_PRECONDITION_NOT_MET` rejection at the MCP boundary | go-supervisor (Greta) |
| F-3 | E2E: `remember → recall` round-trip with a credential-shaped value; assert sanitiser fired + `sanitiser_events` row written; assert `recall` returns the sanitised form | go-supervisor (Greta) |
| F-4 | E2E: `meta_catalogue` returns the live catalogue; `verify_can_transition` agrees with the Layer-1 validator on a candidate transition | go-supervisor (Greta) |
| F-5 | E2E bidirectional-sync round-trip: mutate a mapped `://unblock` item, assert the GitHub Issue is updated via REST/GraphQL, assert the echo webhook does NOT re-trigger a normalise→re-write storm (loop-prevention, R-P02-3). Covers §8.1. | go-supervisor (Greta) |
| F-6 | E2E reconciliation / Law 3: simulate a missed webhook (or GitHub-offline) so a mapped item drifts; assert `drift_detected_at` is set and the reconciliation cron repairs it; assert MCP is still served while the provider is offline. Covers §8.1 + NFR-3. | go-supervisor (Greta) |

### 4.7 Inter-track dependencies (summary)

```
P01 (mcp + workitems + deps + catalogue scaffold)
  ├─► A (Layer-1 enforcement)  ───────────────────────┐
  │     └─► A-6 (catalogue-drift CI, Olive)            │
  │         gated by A-2 + A-5                         │
  ├─► B (memory + 4 tools)     ───────────────────────┤
  └─► C (providers ingestion)  ───────────────────────┤─► F (exit-criterion harness)
        └─► D (providers sync + reconcile) ───────────┘
  E (infra staging/secrets/cron) runs alongside C/D, gates D's live sync
```

Tracks A, B, and C are mutually independent and can run in parallel after
P01 closes. D depends on C. **A-6** (catalogue-drift CI) is the one Track-A
task owned by **Olive** (not Greta), gated by A-2 (`catalogue.gen.go`) +
A-5 (`meta_catalogue`) — see §4.1 Greta=Go / Olive=CI ownership. E (infra)
gates D's bidirectional-sync live test and F's webhook E2E (which needs
secrets). F depends on A + B + **C directly** (C feeds F-1's webhook-ingestion
E2E, which needs C and **not** D) — and additionally on **D** for the full
sync round-trip: **F-5** (bidirectional-sync round-trip) consumes **D-1**, and
**F-6** (reconciliation / Law 3) consumes **D-2**.

---

## 5. External Dependencies and Research Required

These items must be validated by **Smith (research)** under `/research`
before the P02 spec is written. Each is a P02 assumption that internal
docs alone cannot confirm. They become **R-P02-1 …** in
`docs/research/02-research-backend-complete.md` (Smith's output, gating
the phase spec).

### 5.1 GitHub provider APIs

| ID | Question | Why it matters |
|---|---|---|
| R-P02-1 | **GitHub webhook event schema + HMAC verification.** Exact shape of `issues.*` and `pull_request.*` payloads at the API version we pin; the `X-Hub-Signature-256` HMAC scheme; which events we subscribe to for v1.0; `X-GitHub-Delivery` as the dedup key (AR-12). | The normaliser's field map (R-P02-2) and the `providers.events` dedup both depend on this. |
| R-P02-2 | **GitHub Issue ↔ canonical `workitems.items` field map.** How GitHub issue fields (title, body, state, labels, assignees, milestone, locked) map onto our canonical model — and the reverse map for bidirectional sync. What has no clean counterpart (e.g. GitHub has no dependency graph; we have no GitHub "project column"). | This is the core normalisation contract; the spec pins it but research must confirm GitHub's actual field availability + the GraphQL vs REST choice. |
| R-P02-3 | **Bidirectional sync loop prevention.** When `://unblock` writes to GitHub, GitHub fires a webhook back. How do we distinguish our-own-write echoes from genuine external edits (e.g. a sync marker, `last_synced_at` comparison, an actor allowlist)? GitHub rate limits (REST 5000/hr, GraphQL points) and how the reconciler stays under them. | Without this, sync storms or rate-limit exhaustion are real. Drives the `providers.mappings.last_synced_at` / `drift_detected_at` usage. |
| R-P02-3b | **Go GitHub client library for the outbound writer/reconciler.** Pin the Go GitHub client surface — `github.com/google/go-github` (REST) + `shurcooL/githubv4` (GraphQL) vs hand-rolled `net/http` — tied to rate-limit header parsing, secondary-rate-limit backoff, pagination, and GraphQL points accounting that D-1/D-2 need. | Without a pinned client, D-1/D-2 re-implement rate-limit + pagination + points accounting ad hoc. The chosen library is the one D-1 builds on (see §4.4). |
| R-P02-4 | **GitHub OAuth app vs GitHub App for webhooks + write.** OAuth app (user token) vs GitHub App (installation token) — which gives us per-repo webhook subscription + issue-write at v1.0 with the cleanest secret model? Encore secret storage for the chosen credential. | Determines `providers.installations.installation_id_enc` semantics and the secrets work in Track E. PRD §10.1 says "GitHub — OAuth identity, webhooks, REST/GraphQL"; the *webhook subscription* mechanism must be pinned. |
| R-P02-4b | **Webhook failure-response status contract.** Pin the HTTP status per non-happy class on `POST /webhooks/github` — bad/absent HMAC signature, unknown/unregistered installation, malformed JSON, oversized payload — classifying each as 4xx-final (GitHub must NOT retry) vs 5xx-retryable (transient our-side). GitHub redelivers all non-2xx; a 5xx on a permanent signature mismatch storms retries. | Feeds C-1 and F-1. A mis-classified status either drops legitimate retryable failures or storms retries on permanent failures. |

### 5.2 MCP SDK / Layer-1 validator mechanics

| ID | Question | Why it matters |
|---|---|---|
| R-P02-5 | **`modelcontextprotocol/go-sdk` structured-error contract for BLOCK rejections.** How to surface `PIPELINE_PRECONDITION_NOT_MET` (SPEC §7.5.1 `error_code` / `error_message` / `rejection_reason`) as a JSON-RPC error the agent clients (Claude Code, Copilot, Cursor) render usefully — error vs `isError` tool-result content. | The exit criterion is a *rejection*; its wire shape must be agent-legible. Builds on P01 R-P01-3 / R-P01-8. |
| R-P02-6 | **`verify_can_transition` exposure over Streamable HTTP.** SPEC §5.2.2 says it is "not a separate top-level MCP tool — a read-only sub-call exposed via the same SSE channel for the Layer-2 hook". Confirm the go-sdk mechanism for a hook-facing-but-not-listed primitive (is it an un-advertised tool, a resource, or a custom method?). | Affects how P04's `verify-state` hook calls it. If the SDK has no clean "hidden tool" concept, the spec needs an alternative shape. |
| R-P02-7 | **Comment-trail read for the validator.** Does the P01 `get_state` / `GetTrail` surface return enough (most-recent `(kind, status)` *and* any-comment-of-kind existence) for the §7.5.1 `last_comment_*` and `any_comment_*` predicates without an N+1 per transition check? | If not, P02 adds a `workitems` read RPC. The plan assumes the P01 surface suffices; research confirms or flags. |

### 5.3 Memory: encryption + sanitiser

| ID | Question | Why it matters |
|---|---|---|
| R-P02-8 | **`pgcrypto` `pgp_sym_encrypt` / DEK supply via Encore secret.** Confirm the §9.4.10 local-secrets format (`apps/api/.secrets.local.cue`, `MEMORY_DEK`→Go `MemoryDEK`) and the encrypt/decrypt path performance against the NFR-1-adjacent budget for `recall`. DEK rotation (`MEMORY_DEK_NEXT`, AR-7) is documented but not exercised in P02 — confirm. | Memory is encrypt-at-rest; the path must be correct and not breach latency. |
| R-P02-9 | **Secret-sanitiser pattern set + `ts_doc` ordering (AR-10/AR-14).** What credential-shape regex set ships at v1.0; confirm sanitise-runs-before-tokenise so `ts_doc` never carries a secret; the `memory.sanitiser_events` audit shape. | NFR-7 is always-on, no opt-out. The pattern set is a spec contract; research pins the v1.0 baseline + false-negative posture (PRD R-6). |
| R-P02-10 | **`forget` soft-delete vs hard-delete.** SPEC §5.2.2 says `forget` is a "soft-delete (audit-trail preserved via `deleted_at`-equivalent)" but the §9.4.8 DDL has **no `deleted_at` column**. Confirm the additive column + whether the per-scope unique indexes must become partial-on-not-deleted. | Genuine DDL gap. Drives B-4. |
| R-P02-13 | **`memory.entries.expires_at` read+write semantics.** The column ships in the P01 DDL but no tool/task addresses expiry. Decide: does `remember` set it; do `recall`/`memories` filter `WHERE expires_at IS NULL OR expires_at > now()` at read time; or is it inert/reserved for v1.0 (no DDL change)? | Same standard the plan applies to `forget`/`deleted_at`: a shipped column with no defined tool behaviour is a latent gap. Drives B-6 and the §2.2 `recall`/`memories` contract rows. |

### 5.4 Infra / Encore Cloud

| ID | Question | Why it matters |
|---|---|---|
| R-P02-11 | **Encore Cron + Pub/Sub for the providers reconciler, payload-digest, and sanitiser re-scan.** Encore cron declaration semantics; whether webhook→normalise should be synchronous-in-handler or via the `provider.events` Pub/Sub topic (CLAUDE.md names the topic — confirm it is real and needed). **If async is selected, the spec MUST declare the new `provider.events` subscriber's publisher-generated ULID `EventID` idempotency key carried as a typed payload field (SPEC AR-11) — at-least-once replay dedup is distinct from the handler-side `(provider, delivery_id)` constraint (AR-12).** | Determines Track C/D/E structure and AR-13 Pub/Sub-rate budget. |
| R-P02-12 | **Encore Cloud free-tier ceilings under P02 load (AR-13/AR-16).** Concrete Pub/Sub rate, connection cap, cold-start numbers measured on staging; the `mcp-warmer` cron viability; whether the webhook ingestion + cron jobs fit the free tier. | First real deploy; the M-1 latency target and the cron jobs must coexist on the free tier. |

---

## 6. Resolved Decisions (Q1–Q6)

All six questions were **resolved by the user on 2026-06-16**. The
decisions below are binding inputs to `/research` and `/spec`.

### Q1. `boards` service code — P02 or P05? — RESOLVED: **P05**

**Decision (user, 2026-06-16): boards service code → P05** (NOT P02).
Saved-view persistence has no consumer until the Astro web client (P05),
and this aligns with P01 plan §6 Q2 ("grow code in … P05 (boards)"). The
`boards` *schema* (§9.4.7) already migrated in P01; only the service code
placement was in question. **Applied on main:** the SPEC §11 P02 row drops
`apps/api/boards`, the §11 P05 row adds it (service code only), and the
01-plan line that read "Saved-view CRUD — P02" is corrected to P05 so the
predecessor plan is internally consistent (see §8.4 / §9). SPEC §11 is
once again the authoritative traceability source.

### Q2. P01 Layer-1 deferral lands in full here — RESOLVED: **CONFIRMED**

**Decision (user, 2026-06-16): CONFIRMED.** P02 **tightens** `close` to
require the full comment trail / `qa_state=passed`. P01 plan Q1 shipped
`close` with a relaxed `claimed_by_id IS NOT NULL`-only precondition; that
relaxation is replaced by the Layer-1 validator in P02. Any P01-relaxed
flow that called `close` on a non-QA-passed item now fails **by design**
(pre-production, no users — `feedback_pre_production`). The P02
exit-criterion harness flows that relied on the relaxation are updated in
**F-2** to drive the full review/QA comment trail first.

### Q3. Webhook → normalise: synchronous or via Pub/Sub? — RESOLVED: **research-led (R-P02-11)**

**Decision (user, 2026-06-16): research-led (R-P02-11).** The **working
assumption** is *ack-fast / normalise-async via the `provider.events`
topic* — the handler verifies HMAC → dedups → inserts `providers.events`
→ returns 200, and a Pub/Sub subscriber normalises asynchronously, so a
normaliser bug never makes GitHub retry-storm (Law 3 spirit). This is
**not committed** until R-P02-11 confirms the Encore Cron + Pub/Sub
mechanics and the topic's reality. The spec commits the final shape after
research. Per the global no-unilateral-simplification rule, the async path
is the assumption, not a simplification away from it.

### Q4. GitHub App vs OAuth app for webhooks + write — RESOLVED: **research-led (R-P02-4)**

**Decision (user, 2026-06-16): research-led (R-P02-4).** The **working
assumption** is a **GitHub App (installation tokens)** — the idiomatic
per-repo webhook-subscription + issue-write path that maps cleanly onto
`providers.installations.installation_id_enc`. This is **not committed**
until R-P02-4 confirms it against GitHub's actual capabilities and the
Encore secret model. The spec pins the final credential shape after
research; Track E secrets follow that decision.

### Q5. Encore Cloud staging at P02 close — required gate or best-effort? — RESOLVED: **ships, but not the functional gate**

**Decision (user, 2026-06-16):** the staging deploy **ships** (Track E),
but the **functional acceptance bar (§8) stays local-emulator + CI
green**. The AR-13 (free-tier ceiling) and AR-16 (`mcp-warmer`) capacity
numbers are a **published report, not a hard gate** (mirrors the P01 NFR-1
cold-start carve-out and PRD R-1). The staging deploy and capacity
measurement are required *work*; they are not a *pass/fail release gate*
on the P02 functional exit criterion.

### Q6. Memory tool latency vs NFR-1 budget — RESOLVED: **CONFIRMED no PRD gate**

**Decision (user, 2026-06-16): CONFIRMED.** Memory tools carry **no PRD
latency gate.** `recall` decrypts `value_enc` per row, but memory reads
are **not** on the `prime → ready → claim` hot path (NFR-1 / M-1 is the
only PRD latency north-star and it excludes memory). Any memory latency
budget is a spec-level NFR, not a release gate.

---

## 7. Risks Specific to P02

P02-level risks (SPEC §13 AR-* covers architecture-wide risks; PRD §12
covers product-wide risks).

| # | Risk | Mitigation |
|---|---|---|
| RP02-1 | **Bidirectional sync loop / GitHub rate-limit exhaustion.** A naive sync writes to GitHub, the echo webhook re-triggers normalisation, which re-writes — a storm that burns the 5000/hr REST budget. | R-P02-3 closes the loop-prevention design before D-1; `last_synced_at` echo-suppression + actor allowlist; reconciler respects rate-limit headers. Sync is opt-in per installation, so blast radius is bounded. |
| RP02-2 | **Layer-1 validator double-validates against the P01 column-value invariants with conflicting errors.** P01 already enforces the five §6.2 column rules in `set_state`; P02 adds comment-trail gates. A clumsy merge yields two rejection codes for one bad transition. | A-3 reconciles the two layers into one validator pass; the spec pins which check runs first and which error wins. F-2 asserts a single, correct rejection. |
| RP02-3 | **Catalogue drift between PRD §6.7, `catalogue.json`, and `catalogue.gen.go`.** Three representations of the same state machine; hand-authoring the JSON from the PRD table invites transcription error. | AR-4 CI drift test (now load-bearing, A-6) diffs the Go-codegen corner against the live `meta_catalogue`; Ada keeps PRD §6.7 ↔ JSON in sync; the spec includes a per-transition test matrix. |
| RP02-4 | **Webhook HMAC / dedup edge cases.** Replayed `X-GitHub-Delivery`, signature-mismatch, oversized payloads, or a payload-sanitiser false-positive that mangles a legit field. | AR-12 unique constraint + 200-on-duplicate; HMAC verified before any processing; sanitiser is redact-not-reject; F-1 covers the happy path + a replay + a bad-signature case. |
| RP02-5 | **Secret sanitiser false negative leaks a credential into `ts_doc` (unencrypted, GIN-indexed).** AR-10/AR-14: `ts_doc` is plaintext-derived and unencrypted by necessity. | Sanitise-before-tokenise ordering is structural (B-1); `sanitiser_events` audit + periodic re-scan (B-2, AR-14) make a missed pattern recoverable; `ts_doc` is SELECT-locked to the `memory` connection user (AR-10). |
| RP02-6 | **`forget` DDL gap (no `deleted_at`).** SPEC §5.2.2 promises soft-delete but §9.4.8 has no column for it. | R-P02-10 / B-4 resolve the additive column + whether the unique indexes go partial-on-not-deleted before B-3 ships `forget`. |
| RP02-7 | **Encore Cloud free-tier ceiling binds at first real deploy (AR-13).** Webhook ingestion + three cron jobs + the warmer + MCP traffic may exceed the free-tier Pub/Sub rate or connection cap. | E-3 measures on staging before D goes live; the AR-1 exit path (NATS + standard Postgres) is the documented escape; capacity is a report (Q5), not a launch blocker, but the numbers gate scale-out planning. |
| RP02-8 | **Migration drift: P02 edits a P01 migration in place.** Per `feedback_migration_edit_drift`, editing an applied migration silently drifts the long-lived staging + local DBs even while CI stays green on a fresh run. | All P02 migrations are new forward files, higher-numbered than `0140`; CI runs migrations fresh AND a staging-replay check confirms forward-only. |

---

## 8. Acceptance Criteria for P02

This phase is **DONE** when all of the following are demonstrably true.

### 8.1 Functional acceptance (PRD §8 + SPEC §11 P02 exit criterion)

- [ ] A GitHub repository can be linked (`providers.LinkRepo` creates an
      installation with an encrypted webhook secret).
- [ ] A synthetic HMAC-signed `issues.opened` webhook delivered to
      `POST /webhooks/github` is signature-verified, deduplicated, and
      normalised into a canonical `workitems.items` row mapped via
      `providers.mappings`. A duplicate `X-GitHub-Delivery` returns 200
      and does not double-create.
- [ ] A `://unblock` mutation on a mapped item propagates back to the
      GitHub Issue (bidirectional sync), without triggering a sync loop.
- [ ] The reconciliation cron detects and repairs a missed-webhook drift
      case (Law 3).
- [ ] The four memory tools work end-to-end: `remember` (with sanitiser +
      encrypt-at-rest), `recall`, `memories`, `forget`. The MCP tool
      surface is **27**.
- [ ] An attempt to mark a work item `done` (via `close` or
      `set_state(qa_state=passed)`) **without** the required comment trail
      (`kind=review, status=success` then `kind=qa, status=success`) is
      rejected at the MCP boundary with a structured
      `PIPELINE_PRECONDITION_NOT_MET` error citing the missing precondition.
- [ ] `mcp.meta_catalogue` returns the live `catalogue.json`;
      `verify_can_transition` validates a candidate transition against the
      same Layer-1 validator and agrees with `set_state`'s gate.

### 8.2 Non-functional acceptance

- [ ] **NFR-2.** RBAC regression suite extended to `providers` and
      `memory`; zero cross-tenant leaks; memory scope isolation
      (org/project/user) and provider installation org-scoping covered.
      Release-blocking.
- [ ] **NFR-3 (Law 3).** Provider outage / missed webhook does not stop
      the product; the reconciler repairs drift on schedule. A test
      simulates GitHub-offline and asserts the product still serves MCP.
- [ ] **NFR-6 (P02 portion — Law 8 Layer 1 only).** The Layer-1 MCP
      validator is implemented as the *first* of the three enforcement
      layers (FR-14); an integration test asserts every PRD §6.7
      transition's precondition is enforced (per-transition matrix). Full
      NFR-6 — the three-layer simultaneous-bypass property of
      FR-14/15/16 — closes at P04 when Layers 2/3 ship (this plan §3.4).
- [ ] **NFR-7.** Secret sanitiser is always-on (no opt-out); a
      credential-shaped value is sanitised before encryption and before
      `ts_doc` tokenisation; `sanitiser_events` row written.
- [ ] **NFR-10.** Greta's Go gate set green (`encore test ./...`,
      `go vet`, `go fmt`, `encore check`); JSON wire-tag lint
      (`grep -rnE 'json:"[A-Z]' apps/api/` → zero) green on the new
      providers + memory + sanitiser structs.
- [ ] **NFR-12.** JSON-Lines logs on STDERR; MCP envelopes on STDOUT;
      the DEK / webhook secret / OAuth token never logged. Encore `trace_id`
      (SPEC §10.5) propagates from `POST /webhooks/github` through the
      (async) normaliser subscriber into the downstream `workitems`
      `Create`/`Update` RPCs and into the four P02 cron jobs; ≥1 test
      asserts a single webhook delivery and its async normalisation share
      one trace tree (so a normaliser failure is correlatable to its
      `X-GitHub-Delivery`). The subscriber clause is conditional on
      Q3/R-P02-11 — if synchronous-in-handler wins, it becomes the
      in-handler RPC chain.

### 8.3 Architectural invariants

- [ ] **Additive migrations only.** P02 ships no edit to a P01 migration;
      all P02 migrations are new, higher-numbered than `0140`; a
      forward-only replay check is green.
- [ ] **No new persistent store.** All P02 state in the existing eight
      Postgres schemas + additive migrations; no Redis, no SQLite, no
      second DB (FR-1).
- [ ] **Catalogue is single-source.** `catalogue.json` →
      `catalogue.gen.go` (Go) ↔ `mcp.meta_catalogue` (live) agree; CI
      drift test (AR-4) green for the Go↔live pair (Rust corner inert
      until P04).
- [ ] **Webhook dedup is structural.** `(provider, delivery_id)` unique
      constraint (AR-12) rejects replays at the DB; the normaliser is
      never invoked twice for one delivery id.
- [ ] **No `crates/` code ships with P02** (NFR-9 — decoupled
      deliverables).
- [ ] **Manifesto Laws covered in P02** (L3 provider events, L8 layer 1)
      are structurally present, each backed by ≥ 1 regression test.

### 8.4 Documentation

- [ ] `docs/specs/02-spec-backend-complete.md` is APPROVED before
      implementation starts.
- [ ] `docs/research/02-research-backend-complete.md` closes R-P02-1
      through R-P02-13 (incl. the R-P02-3b / R-P02-4b siblings) with
      verified findings before the spec is approved.
- [x] **Q1 resolved "boards → P05": SPEC §11 P02/P05 rows patched
      (applied 2026-06-16, changelog entry dated 2026-06-16, status
      remains APPROVED).** The §11 P02 row also gained `apps/api/memory`
      and had its stale "§9.4.5 + §9.4.7" migration reference replaced
      with the additive-only reality; the 01-plan boards "P02" line is
      corrected to P05. §11 is once again the authoritative traceability
      source. The spec is written against the patched §11.
- [ ] README.md updated with the P02 surface (27 MCP tools incl. the four
      memory tools; the `POST /webhooks/github` public endpoint; the
      pipeline-enforcement behaviour).
- [ ] CLAUDE.md / AGENTS.md updated to reflect P02 reality (providers +
      memory live; Layer-1 enforcement active; Encore Cloud staging if Q5
      lands that way).

### 8.5 Ops (non-gating per Q5)

- [ ] **Track E (non-gating per Q5).** Encore Cloud staging deploy live
      (E-1) + provider + per-install webhook secret + `MEMORY_DEK`
      provisioned (E-2); AR-13 free-tier ceiling report + AR-16
      `mcp-warmer` viability published (E-3). Confirms the required work
      landed; **not a pass/fail release gate** on the P02 functional exit
      criterion (mirrors Q5 — staging + capacity are required *work*, not a
      functional gate).

---

## 9. Sequencing Notes

- **`/research` runs first** to close R-P02-1 through R-P02-13 (incl. the
  R-P02-3b / R-P02-4b siblings). Findings
  update or contradict §2 / §5 of this plan; the plan is re-approved if
  anything load-bearing flips (esp. R-P02-2 field map, R-P02-3 loop
  prevention, R-P02-4 App-vs-OAuth, R-P02-10 `forget` DDL gap).
- **SPEC §11 patch is applied (2026-06-16).** Q1 resolved boards→P05; the
  §11 P02/P05 traceability reconciliation (boards P02→P05, add
  `apps/api/memory` to P02, replace the stale §9.4.5/§9.4.7 base-schema
  migration reference with the additive-only reality) landed on main
  ahead of the spec, plus the 01-plan boards-line correction — spec-first
  on drift, per `feedback_spec_first_on_drift` / `feedback_spec_commit_on_main`.
  No further §11 edit is pending for this phase.
- **`/spec` runs after research approval** and produces
  `docs/specs/02-spec-backend-complete.md`: the GitHub field map, the
  webhook + sync + reconcile contracts (including — if R-P02-11 selects
  the async path — the `provider.events` subscriber's publisher-generated
  ULID `EventID` idempotency key carried as a typed payload field per SPEC
  AR-11, distinct from the `(provider, delivery_id)` AR-12 constraint), the
  four memory tool JSON schemas, the sanitiser pattern set, the
  `catalogue.json` BLOCK-conditions section, the additive migration files,
  and the per-transition test matrix.
- **`/tasks` runs after spec approval** and emits bd beads in the
  dependency order of §4.7. Every implementation bead must require the
  worker to read the spec/plan (per `feedback_bead_description_not_spec`).
- **`/do` per supervisor** runs the work, gated by the per-track
  dependency edges; Greta owns all Go, Olive owns infra. Branches base
  off main (`feedback_branch_base_main`).

---

## 10. Reference Anchors

- **PRD** §5.1 (FR-8 27 tools, FR-9 state-transition validation, FR-10
  comment trail, FR-11 GitHub integration, FR-12 public endpoints, FR-13
  memory), §5.2 (FR-14 Layer 1), §6.2 (state invariants), §6.5 (comment
  trail), §6.7 (pipeline state machine — the BLOCK-conditions source
  table), §8 (P02 exit criterion), §9.2 (GitLab v1.1 deferral), §12
  (R-2/R-6/R-7 risks).
- **SPEC** §5.2 (8-service decomposition), §5.2.2 (27-tool inventory; the
  four memory tools; `verify_can_transition` + `meta_catalogue` as
  operational primitives), §5.3 (public surface — `POST /webhooks/github`),
  §5.6 (RBAC), §5.7 / §5.7.1 (state machine + `pipeline_stage` derivation),
  §7.4 (three layers of Law 8), §7.5 (BLOCK condition schema — the typed
  catalogue contract), §9.4.5 (`providers` DDL + retention policy), §9.4.8
  (`memory` DDL), §9.4.10 (DEK / local-secrets), §11 (P02 traceability —
  reconciled 2026-06-16: boards→P05, `apps/api/memory` added to P02,
  additive-only migration reality; Q1 resolved), §13 (AR-1, AR-4, AR-7, AR-10, AR-12,
  AR-13, AR-14, AR-16).
- **Manifesto** Laws L3 (Postgres source of truth / provider events), L8
  (pipeline gates enforced architecturally — Layer 1 in P02).
- **Predecessor:** P01 plan §2.1 (schema-only stubs for providers/memory),
  §3.4 (Layer-1 deferral to P02), Q2 (all 8 schemas migrate in P01), Q6
  (Encore Cloud staging is a P02 ops task).
- **Explicitly excluded:**
  `docs/research/agentic-research/RFC-REACTIVE-AGENT-ENVIRONMENT.md`
  (P02+/additive, not P02 — §3.2).
