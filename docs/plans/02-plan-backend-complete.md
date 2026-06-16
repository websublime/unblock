# PLAN: P02 — Backend Complete (providers + memory tools + Layer-1 pipeline enforcement)

**Status:** APPROVED *(Q7=drop / Q8=documented-outlier resolved 2026-06-16. Research-reconciled 2026-06-16 — R-P02-1..13 verdicts recorded (9 CONFIRMED / 3 PARTIAL / 1 CONTRADICTED), C1–C5 resolved (§5.5): C2 SPEC §5.2.2 patched (Resource + custom JSON-RPC method, SSE→Streamable-HTTP); **C1 (Q7) RESOLVED 2026-06-16 = DROP** — the per-install `providers.installations.webhook_secret_enc` column is dropped via a NEW additive forward migration (`0060` unedited; re-added at v1.1 for the OAuth-app/GitLab per-install fallback); HMAC verifies against the app-level Encore secret `GITHUB_APP_WEBHOOK_SECRET` only; SPEC §9.4.5 C1 note + changelog patched; **C5 (Q8) RESOLVED 2026-06-16 = documented cold-start outlier** — no external pinger at v1.0 (SPEC §13 AR-16 option ii; external pinger option i deferred to v1.x); SPEC §13 AR-16 patched; C3 (validator read RPC) + C4 (memory `deleted_at` additive migration) recorded as 02-spec contracts; R1/R2 risks + research OQ4/OQ5 dispositioned. P01 migrations 0060/0090 untouched (the C1 DROP and C4 soft-delete ship as new forward files). **Code-grounded review reconciliation applied 2026-06-16 (23 findings; §5.6 code-grounded spec-constraints added)** — confronted the reconciled docs against live `apps/api/` code: GROUP A drifts fixed (C-2 `installation_id_enc`-only per Q7-DROP; `/webhooks/github` raw endpoint inside `providers` not `apps/api/public/`; §5.2.2 primitives "do NOT count toward the 27" uniform; per-RPC invariant split — `set_state` = I-1/I-2/I-4/I-5 + structural `impl_done_requires_claim`, I-3 = `claim`-only; three-stub `.gitkeep` hygiene); §5.6 added with 13 spec-constraints; AR-11 citation corrected to the executable `cascade_subscriber.go` INSERT. go-sdk version drift DISCONFIRMED — v1.6.0 unchanged. Prior: review-driven drift/gap reconciliation 2026-06-16 (23 findings); user decisions Q1–Q6 resolved §6; SPEC §11 P02/P05 traceability patch applied on main. **§2.3 ordering reconciled to gate-first per 02-spec OQ-A 2026-06-16** — the comment-trail gate runs in the MCP handler before `SetStateColumns`; the pipeline gate's `PIPELINE_PRECONDITION_NOT_MET` wins when both would fail; the old "invariants-first" prose in §2.3 + §5.5 C3 row + §7 RP02-2 was replaced. Plan stays APPROVED.)*
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
`docs/archive/agentic-research/RFC-REACTIVE-AGENT-ENVIRONMENT.md`. That
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
| `workitems` / `deps` | **Net-new validator read surface (research R-P02-7/C3 flipped the original "no new RPC" assumption).** P02's Layer-1 validator reads the comment trail (`workitems.comments`) and the four state columns. The original assumption — "the P01 `GetTrail` / `get_state` RPCs expose everything the validator needs without a new private RPC" — is **WRONG**: `workitems.GetState.recent_kinds` is `DISTINCT ON (kind)` (latest-per-kind), which can serve neither the §7.5.1 global `last_comment_*` predicate (single most-recent comment across all kinds) nor a history-aware `any_comment_*=ever-existed` predicate (a later same-kind comment overwrites an earlier success). **P02 therefore adds a new/extended `workitems` read RPC** for the validator predicates (the exact signature — e.g. `GetCommentTrailPredicates(item_id)` returning the global-latest tuple plus per-`(kind,status)` EXISTS booleans, or a `GetState` extension — is an **02-spec contract**, see §5.5 R-P02-7). The webhook normaliser still writes through the existing `workitems.Create` / `Update` RPCs — no direct cross-schema writes (Law 6 / RBAC §5.6). |

### 2.2 The MCP tool surface: 23 → 27 (SPEC §5.2.2)

P02 adds the four memory tools. The P01 23-tool inventory is unchanged.

| # | Tool | P02 contract notes |
|---|---|---|
| 24 | `remember` | Write a scoped memory entry (`org`/`project`/`user`). Secret sanitiser runs **before** encryption (NFR-7); 8 KB `value_size` cap (DDL CHECK, §9.4.8); per-(scope, key) uniqueness enforced by the partial unique indexes. Records a `memory.sanitiser_events` row on every sanitiser hit (AR-14). |
| 25 | `recall` | Read entries by scope + key; supports tag and full-text (`ts_doc` GIN) filters. Decrypt-on-read; never decrypt-on-search (AR-10). RBAC-scoped. Whether expired rows (`memory.entries.expires_at`) are filtered at read time is resolved by R-P02-13 / B-6. |
| 26 | `memories` | List entries by scope with pagination; cheap dashboard read (powers `prime`'s `memory_hints` field, which was empty in P01 — P01 plan §3.2). Whether expired rows (`expires_at`) are filtered is resolved by R-P02-13 / B-6. |
| 27 | `forget` | Soft-delete an entry (audit-trail preserved). **Research R-P02-10/C4 CONFIRMED the DDL gap is genuine:** `memory.entries` has no `deleted_at`. The additive forward migration (B-4, §2.5) adds `deleted_at timestamptz` AND rewrites the three per-scope unique indexes to partial-on-not-deleted (`… AND deleted_at IS NULL`), else `forget`→re-`remember` of the same `(scope,key)` breaks. `recall`/`memories` MUST filter `deleted_at IS NULL` (couples with the R-P02-13 `expires_at` read filter). No edit to `0090`. |

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
  **Mechanism (research R-P02-6/C2, SPEC §5.2.2 patched 2026-06-16):** the
  go-sdk v1.6.0 has **no "registered-but-unlisted tool"** concept — any
  `AddTool` registration is advertised in `tools/list`. To keep the
  agent-facing `tools/list` at exactly 27, `verify_can_transition` ships as a
  **custom JSON-RPC method** (e.g. `unblock/verifyCanTransition`) intercepted
  via the go-sdk `AddReceivingMiddleware` hook, NOT via `AddTool`. The exact
  method name + arg-decode + §7 envelope path is an 02-spec contract.
- **Ship `mcp.meta_catalogue` v1** — the read-only catalogue endpoint that
  serves the live `catalogue.json` for the P04 build-time renderer to verify
  against the checked-in copy (AR-4 third corner). **Mechanism (research
  R-P02-6/C2, SPEC §5.2.2 patched 2026-06-16):** ships as an **MCP Resource**
  (`AddResource`, stable URI e.g. `unblock://catalogue`, surfaced under
  `resources/list` — never in `tools/list`), over the same Streamable HTTP
  transport (`POST /mcp`). The stale SPEC "same SSE channel" wording is
  corrected to Streamable HTTP in the same patch.

> **The PRD §6.2 column-value invariants already shipped in P01 — but they
> are split across two RPCs, not all in `set_state` (code-grounded review
> D1-4, 2026-06-16).** Per `apps/api/workitems/workitems.go`:
> - **`set_state` (`SetStateColumns`) carries FOUR of the five** — I-1 (auto-reset
>   of `qa_state` when `review_state=needs_rework`; an **auto-reset, NOT a
>   rejection** — `workitems.go:1803-1806`), I-2 (`:1813-1816`), I-5
>   (`:1818-1832`), I-4 (`:1842-1844`) — **plus a separate structural
>   `impl_done_requires_claim` check** (`impl_state=done` requires
>   `claimed_by_id IS NOT NULL` — `:1808-1811`) that is **not** one of I-1..I-5.
> - **I-3 lives in `claim` (`Claim`), not `set_state`** — `qa_state=failed` →
>   `review_state`+`qa_state` reset to pending (`workitems.go:1722` docstring,
>   `:2078`, `:2194`).
>
> Those are pure column-value rules with no comment-trail dependency. **P02
> adds the comment-trail-driven gates on top.** The spec must reconcile the
> layers so a transition is not double-validated with conflicting messages,
> and must pin the invariant set **per-RPC** (do NOT write "I-1..I-5 evaluated
> first in `set_state`").
>
> **Two-error-code reconciliation (research R-P02-7/C3, RP02-2; per-RPC
> ordering per D1-4; gate-first per 02-spec OQ-A RESOLVED 2026-06-16).** P01's
> `set_state` already rejects its column-value invariants with
> `PRECONDITION_NOT_MET` + `data.invariant`. P02's new comment-trail gates
> reject with `PIPELINE_PRECONDITION_NOT_MET` (a **distinct** `envelopeKind*`
> constant — research R-P02-5 confirmed the `errmap.go` path carries it).
> These MUST NOT double-fire. The 02-spec pins the order **per-RPC, gate-first**
> (the comment-trail gate reads pre-existing comments in the MCP handler
> **BEFORE** `SetStateColumns` runs — 02-spec §4.4.5 / OQ-A RESOLVED
> 2026-06-16):
> - **`set_state`:** (1) the **P02 comment-trail gate runs first** in the MCP
>   handler (`PIPELINE_PRECONDITION_NOT_MET`); if it rejects, `SetStateColumns`
>   is never reached. (2) Only if the gate passes does `SetStateColumns` run
>   its column-value invariants — I-1 auto-reset (precedes but is NOT a
>   rejection, so it does not participate in "one error wins"); then the
>   rejection checks **in code order**: structural `impl_done_requires_claim`
>   → I-2 → I-5 → I-4 (all `PRECONDITION_NOT_MET`). **The column invariants are
>   the structural backstop**; when a pipeline gate and a column invariant
>   would both fail, the **pipeline gate's `PIPELINE_PRECONDITION_NOT_MET`
>   wins** because it is checked first.
> - **`claim` (`Claim`):** I-3 is enforced here, not in `set_state`.
>
> So the per-transition test matrix + codegen ordering contract place **I-3
> under `claim`** and **`impl_done_requires_claim` under `set_state`**, and
> exactly one error wins per bad transition (A-3 / RP02-2). F-2 asserts a
> single, correct rejection. Cross-reference 02-spec §4.4.5.

### 2.4 Provider integration (FR-11, Law 3)

The `providers` service is the largest net-new code surface. Sub-areas:

- **Webhook ingestion** — `POST /webhooks/github` (public, FR-12). Verify
  the HMAC signature **before the body is parsed** (constant-time
  `hmac.Equal` over the raw bytes, research R-P02-1). **Secret-model
  reconciliation (research R-P02-4/C1, RESOLVED 2026-06-16 — DROP — see §6 Q7
  / §5.5 C1):** under the **confirmed v1.0 GitHub-App path** the
  webhook secret is **app-level** — one secret for all installs, the
  delivery disambiguated by `installation.id` in the payload — so HMAC
  verifies against an **Encore application secret** (working name
  `GITHUB_APP_WEBHOOK_SECRET`, provisioned in Track E-2) **only**, **NOT** the
  per-install `installations.webhook_secret_enc` column. That per-install
  column is **DROPPED via a new additive forward migration** (a forward
  `ALTER TABLE … DROP COLUMN`; `0060` itself is NOT edited per
  `feedback_migration_edit_drift`; the stub table is empty pre-production so
  the drop is safe — §2.5(d); SPEC §9.4.5 C1 note + changelog patched
  2026-06-16) and is **re-added by a future migration** when the OAuth-app /
  future-GitLab per-install fallback ships (v1.1). Then insert into
  `providers.events` with the
  `(provider, delivery_id)` dedup constraint (AR-12); return `200 OK` on a
  recognised duplicate so GitHub stops retrying; the per-row payload
  sanitiser redacts emails / credential patterns **on insert** (§9.4.5
  retention policy, first layer). Per-class failure statuses (4xx-final vs
  5xx-retryable) per research R-P02-4b are an 02-spec contract (C-1 / F-1).
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
  (b) the **`memory.entries` soft-delete migration (research R-P02-10/C4
  CONFIRMED genuine DDL gap, B-4)** — a NEW additive forward migration that
  (i) adds `deleted_at timestamptz` to `memory.entries`, AND (ii) DROPs +
  recreates the three per-scope unique indexes
  (`entries_org_key_uniq` / `entries_project_key_uniq` /
  `entries_user_key_uniq`) as **partial-on-not-deleted**
  (`… AND deleted_at IS NULL`). Both halves are mandatory: without the
  partial-index rewrite, `forget` (soft-delete) then re-`remember` of the
  same `(scope,key)` violates the live unique index. `0090_memory` is **NOT**
  edited (`feedback_migration_edit_drift`); the migration is a new,
  higher-numbered forward file; the exact file lands in the 02-spec.
  (c) anything the providers digest/retention job requires beyond the
  shipped §9.4.5 DDL.
  (d) the **`providers.installations.webhook_secret_enc` DROP COLUMN
  migration (C1, Q7 RESOLVED 2026-06-16 — DROP)** — a NEW additive forward
  `ALTER TABLE providers.installations DROP COLUMN webhook_secret_enc`. Under
  the confirmed GitHub-App path HMAC verifies against the app-level Encore
  secret `GITHUB_APP_WEBHOOK_SECRET` only, so the per-install column is dead;
  `providers.installations` is an empty stub pre-production so the drop is
  safe. `0060_providers` is **NOT** edited; the column is re-added by a
  future migration when the OAuth-app / GitLab per-install fallback ships
  (v1.1). Exact file authored in 02-spec.
  Per `feedback_migration_edit_drift`: **never edit an
  applied P01 migration in place** — P02 migrations are new, higher-numbered
  files.
- **Encore Pub/Sub + Cron.** P02 introduces the providers reconciliation
  cron, the payload-digest cron, and (per AR-14) the memory sanitiser
  periodic re-scan job. **Research R-P02-11 CONFIRMED** the ack-fast /
  normalise-async path (Q3 working assumption): the `provider.events`
  Pub/Sub topic does **not** yet exist (only `deps-cascade-*` topics do) but
  the pattern is idiomatic — `pubsub.NewTopic[*ProviderEvent](...,
  {DeliveryGuarantee: AtLeastOnce})` + a subscriber, mirroring
  `deps/cascade.go`. **Async path invariant (AR-11):** the subscriber
  payload MUST carry a **publisher-generated ULID `EventID`** as a typed
  field; at-least-once replay dedup uses `ON CONFLICT DO NOTHING` keyed on it
  — **distinct** from the handler-side `(provider, delivery_id)` AR-12 dedup.
  The proven, **executable** template is the
  `INSERT INTO deps.cascade_events … ON CONFLICT (event_id,
  triggered_by_item_id) DO NOTHING` in `insertCascadeEventRow`
  (`apps/api/deps/cascade_subscriber.go:742`, SQL at `:769-779`), with the
  publisher-generated ULID `EventID` field on `CascadeRequested`
  (`deps/cascade.go:53-57`) — **not** the topic-declaration doc comment at
  `cascade.go:158-162` (that is a contract comment, no INSERT). The final
  topic + subscriber + idempotency-key shape is an 02-spec contract.
- **Encore Cloud deploy + secrets (Olive).** P02 is the first phase that
  deploys to Encore Cloud staging (P01 plan Q6: "Encore Cloud staging
  deploy is a P02 ops task owned by Olive"). This brings: the GitHub
  webhook secret + the provider write/webhook credential (shape pinned by
  R-P02-4 — App ID + private-key PEM under the GitHub-App working
  assumption, OAuth-app client-id/secret/redirect-uri under the fallback)
  into Encore secrets; the
  `pgcrypto` DEK (`MEMORY_DEK`) provisioning; the AR-13 free-tier ceiling
  measurement (Pub/Sub rate, connection cap, cold-start) and the AR-16
  synthetic warmer. **Research R-P02-12/C5 (Q8 RESOLVED 2026-06-16 —
  documented outlier — see §6 Q8 / §5.5 C5):** Encore Cloud's **free-tier
  cron minimum interval is once per hour** (and cron does not run in
  local/preview), so an in-Encore `mcp-warmer` cron **cannot run sub-hourly**
  — hourly pings will not keep a scale-to-zero MCP service warm. Two
  non-gating options (Q5: capacity is a report, NFR-1 measured-warm): (a) an
  **external sub-hourly uptime pinger** hitting `POST /mcp`, or (b) accept
  cold-start as the documented launch-period outlier (AR-16(a)). **v1.0 takes
  option (b) — documented outlier, no external pinger at v1.0;** option (a)
  is deferred to v1.x. The SPEC §13 AR-16 wording is patched 2026-06-16 to
  record the hourly bound and mark option (ii)/(b) as the v1.0 disposition;
  E-3 publishes the measured cold-start number on staging.
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

- `apps/api/providers/` service code **including** the `POST /webhooks/github`
  raw public endpoint (`//encore:api public raw`) wired **INSIDE the
  `providers` service** — same pattern as `/mcp`, which is a
  `//encore:api public raw path=/mcp` declared inside the `mcp` service
  (`apps/api/mcp/mcp.go`), **not** an `apps/api/public/` package (no such
  package exists). This is the second of the two FR-12 v1.0 public endpoints;
  P01 wired `/mcp` inside the `mcp` service.
- `apps/api/memory/` service code + the four MCP tool registrations in
  `apps/api/mcp/`.
- `apps/api/mcp/catalogue.json` BLOCK-conditions section authored;
  `catalogue.gen.go` regenerated and committed.
- New P02 migrations under `apps/api/db/migrations/` (the dedicated
  zero-API `db` migration-owner service per CLAUDE.md Coding Standards;
  the sole `sqldb.NewDatabase("unblock", ...)` owner is `apps/api/db/db.go`)
  — additive only, higher-numbered than the P01 set (which ends at
  `0140_deps_cascade_events_kind_chk_fix`).
- **Repo hygiene (P02-impl obligation).** **All THREE** stub `.gitkeep`s cite
  the stale `apps/api/auth/migrations/...` path (line 2 of each; the real
  migration owner is `apps/api/db/migrations/`):
  - `apps/api/providers/.gitkeep` (cites `auth/migrations/0060_providers.up.sql`)
    and `apps/api/memory/.gitkeep` (cites `auth/migrations/0090_memory.up.sql`)
    MUST be **deleted** once their service `.go` files land in P02, **or**
    corrected to `apps/api/db/migrations/`.
  - `apps/api/boards/.gitkeep` (cites `auth/migrations/0080_boards.up.sql`)
    — boards service code is **deferred to P05** (Q1), so the "once the
    service `.go` files land" trigger never fires for it in P02; its stub
    MUST be **corrected in-place** to `apps/api/db/migrations/0080_boards.up.sql`
    during P02 hygiene so the stale path does not silently persist.
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
`docs/archive/agentic-research/RFC-REACTIVE-AGENT-ENVIRONMENT.md` and the
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
| B-4 | `forget` soft-delete additive migration (R-P02-10/C4 CONFIRMED): add `memory.entries.deleted_at` + rewrite the three per-scope unique indexes partial-on-not-deleted (`… AND deleted_at IS NULL`); `recall`/`memories` filter `deleted_at IS NULL`. New forward file, no edit to `0090`. | go-supervisor (Greta) |
| B-5 | RBAC regression suite extended to `memory` (org/project/user scope isolation) | go-supervisor (Greta) |
| B-6 | Resolve `memory.entries.expires_at` semantics (R-P02-13) — either wire it into `remember` (write) + `recall`/`memories` (read-time filter), or document it as inert/reserved for v1.0 (no DDL change) | go-supervisor (Greta) |

### 4.3 Track C — Providers: ingestion + normalisation (depends on P01 `workitems`)

| ID | Task | Owner |
|---|---|---|
| C-1 | `POST /webhooks/github` public handler: HMAC verify, `providers.events` insert + `(provider, delivery_id)` dedup (AR-12), on-insert payload sanitiser, 200-on-duplicate + failure-status contract per R-P02-4b | go-supervisor (Greta) |
| C-2 | `providers.LinkRepo` RPC: create `providers.installations`, store encrypted installation id (`installation_id_enc`) **only** — **NO per-install webhook secret** (Q7 RESOLVED = DROP `webhook_secret_enc`); HMAC verifies against the app-level Encore secret `GITHUB_APP_WEBHOOK_SECRET` (provisioned in Track E-2), not a per-install secret | go-supervisor (Greta) |
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
| E-2 | Secrets (R-P02-4/C1 CONFIRMED App path): provision **App ID + private-key PEM + the app-level webhook secret** (working name `GITHUB_APP_WEBHOOK_SECRET`) + `MEMORY_DEK`. **C1 (RESOLVED 2026-06-16 — DROP, §6 Q7):** the webhook secret is **app-level** (one for all installs), verified before body-parse — NOT the per-install `webhook_secret_enc` column, which is **DROPPED via a new additive forward migration** (§2.5(d); `0060` unedited; re-added at v1.1 for the OAuth-app / GitLab per-install fallback). `apps/api/SECRETS.md`'s table + `apps/api/secrets.nonprod.cue` must gain `GitHubApp*` + `GitHubAppWebhookSecret` placeholders (neither exists today — the registry holds OAuth-app secrets only). OAuth-app fallback set (client-id/secret/redirect-uri + per-install secret) deferred unless the App path is rejected. | infra-supervisor (Olive) |
| E-3 | AR-13 free-tier ceiling measurement (Pub/Sub rate, connection cap, cold-start) + AR-16 warmer. **R-P02-12/C5 (RESOLVED 2026-06-16 — documented outlier, §6 Q8):** free-tier cron is **hourly-min**, so an in-Encore sub-hourly `mcp-warmer` is not deliverable — v1.0 disposition is **document cold-start as an accepted launch-period outlier** (AR-16(a) / SPEC §13 AR-16 option ii); **no external pinger ships at v1.0** (external sub-hourly pinger on `POST /mcp` deferred to v1.x, option i). E-3 publishes the measured cold-start number on staging. Non-gating (Q5). | infra-supervisor (Olive) |
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

### 5.5 Research verdicts and contradiction resolutions (reconciled 2026-06-16)

Source: [`docs/research/02-research-backend-complete.md`](../research/02-research-backend-complete.md)
(Smith, 2026-06-16). **Net: 9 CONFIRMED, 3 PARTIAL, 1 CONTRADICTED**; five
contradictions C1–C5 + two risks resolved below. None blocks the phase; all
are spec-shape decisions. R-P02 verdicts:

| ID | Verdict | Reconciliation into this plan |
|---|---|---|
| R-P02-1 | CONFIRMED | `X-Hub-Signature-256` = hex(HMAC-SHA256(secret, raw_body)); `hmac.Equal` constant-time over unparsed bytes. `X-GitHub-Delivery` GUID = the AR-12 dedup key (stable across redeliveries). v1.0 sub set = `issues` + `pull_request`, action in body, `event_type='<event>.<action>'`. No change. |
| R-P02-2 | PARTIAL | REST (go-github) covers the full v1.0 read+write field map; defer githubv4. Unmapped at v1.0 (02-spec decides degradation): GitHub has no dependency graph (we own it), `state_reason` has no canonical column, the three pipeline columns are unblock-only (NOT inferred from issue state), GitHub-login ↔ `claimed_by_id` ULID needs identity resolution or stays unmapped. Field map is an 02-spec contract. |
| R-P02-3 | CONFIRMED | Loop suppressors all available: App-bot `sender.login` allowlist (fast path) + content-idempotent normalise (structural backstop) + `last_synced_at` echo window (columns exist in `0060`). go-github surfaces rate-limit headers. See R1. |
| R-P02-3b | CONFIRMED | Pin `github.com/google/go-github` (REST, latest major **v88.0.0**, 2026-05-21) + `bradleyfalzon/ghinstallation/v2` (App installation transport). `golang-jwt/jwt/v5 v5.3.1` already in `go.sum`. githubv4 only if a GraphQL-only field forces it (not expected v1.0). Exact major pinned in 02-spec. |
| R-P02-4 | CONFIRMED (App) — **C1** | GitHub App wins: one app-level webhook URL + one app-level webhook secret for all installs (disambiguated by `installation.id`); installation access tokens (JWT-signed, 1-hour) for autonomous server-to-server issue writes; rate budget ≥ OAuth. **C1 resolved below** — webhook secret is app-level (Encore secret), not per-install. |
| R-P02-4b | CONFIRMED | Failure-status contract: bad/absent HMAC → 401 (no `events` insert, no normalise); unknown `installation.id` → 404; malformed JSON → 400; oversized → 413; duplicate `X-GitHub-Delivery` → 200; transient our-side → 503. All 4xx-final classes are non-retryable. 02-spec pins (C-1/F-1). |
| R-P02-5 | CONFIRMED | `errmap.go` already returns `*sdkjsonrpc.Error{Code:-32000, Data:<§7 envelope>}` forwarded verbatim → the exact mechanism for `PIPELINE_PRECONDITION_NOT_MET`. Map onto a NEW `envelopeKind*` constant distinct from the existing `PRECONDITION_NOT_MET` (I-1..I-5). See C3. |
| R-P02-6 | **CONTRADICTED — C2** | go-sdk v1.6.0 has **no "registered-but-unlisted tool"**. SPEC §5.2.2 patched 2026-06-16. **C2 resolved below.** |
| R-P02-7 | PARTIAL — **C3** | `GetState.recent_kinds` is `DISTINCT ON (kind)` (latest-per-kind) — cannot serve global `last_comment_*` nor history-aware `any_comment_*=ever`. The §2.1 "no new RPC" assumption is **WRONG**. **C3 resolved below.** |
| R-P02-8 | CONFIRMED | pgcrypto `pgp_sym_encrypt`/`pgp_sym_decrypt` + `MEMORY_DEK` proven in P01 (`auth/auth.go:487`, `secrets.go:68`, `secrets.nonprod.cue:62`). The `memory` package needs its own `var secrets struct { MemoryDEK string }` (Encore secrets are package-scoped); CI drift-check unions packages. Pin the explicit aes256 form for parity (02-spec). DEK rotation (`MEMORY_DEK_NEXT`, AR-7) inert in P02. No latency gate (Q6). |
| R-P02-9 | PARTIAL | Ordering CONFIRMED structural (`0090` comments: sanitise → tokenise(ts_doc) → encrypt) but it is a **service-code invariant** the 02-spec must pin (DDL enforces nothing). **No v1.0 pattern set exists yet** — the regex baseline is an 02-spec deliverable (research recommends AWS key id, generic secret/token/password/api-key, GitHub PAT prefixes, PEM markers, JWT, bearer, email). `memory.sanitiser_events` shape is a net-new 02-spec contract (additive migration). See R2. |
| R-P02-10 | CONFIRMED — **C4** | Genuine DDL gap (`memory.entries` has no `deleted_at`); partial-on-not-deleted index rewrite required. **C4 resolved below** (B-4, §2.5). |
| R-P02-11 | CONFIRMED | Async webhook→normalise viable + idiomatic; `provider.events` topic does not yet exist; AR-11 publisher-ULID `EventID` idempotency required, distinct from AR-12. Executable template = `insertCascadeEventRow` `ON CONFLICT … DO NOTHING` at `deps/cascade_subscriber.go:742`/`:769-779` + `CascadeRequested.EventID` at `cascade.go:53-57` (NOT the `cascade.go:158-162` doc comment). Recorded in §2.5. Cron min interval = hourly (→ C5). |
| R-P02-12 | CONFIRMED — **C5** | Free-tier: 100k req/day, 100k Pub/Sub msgs/day, 1 GB DB, **cron hourly-min**, 2 cloud envs, no preview, no log retention. mcp-warmer not deliverable sub-hourly in-Encore. AR-13 pooled bindings already compliant (`db/db.go`). **C5 resolved below.** |
| R-P02-13 | CONFIRMED (inert) | `expires_at` ships in `0090`, no code references it. No DDL change for any option. Read-filter couples with the C4 `deleted_at IS NULL` filter; write-surface (wire vs read-filter-only vs inert) is the 02-spec decision (B-6, Q4 open). Precedent: `mcp.api_keys.expires_at` read-time-honour-no-sweeper (`0070`). |

**Contradiction resolutions (C1–C5):**

- **C1 [providers webhook secret — app-level, RESOLVED 2026-06-16 — DROP].**
  Under the confirmed GitHub-App path, HMAC verifies against an **app-level
  Encore secret** (`GITHUB_APP_WEBHOOK_SECRET`, added to Track E-2 / §3.5
  secrets registry of the 02-spec + `SECRETS.md` + `secrets.nonprod.cue`)
  **only**. The per-install `installations.webhook_secret_enc` column is
  **DROPPED via a new additive forward migration** (a forward
  `ALTER TABLE … DROP COLUMN`; `0060` itself is **NOT** edited per
  `feedback_migration_edit_drift`; the stub table is empty pre-production so
  the drop is safe), and **re-added** by a future migration when the
  OAuth-app / GitLab per-install fallback ships (v1.1). SPEC §9.4.5 C1 note +
  changelog patched 2026-06-16. Recorded in §2.4 + §2.5(d) + §6 Q7. The exact
  migration file is an 02-spec deliverable.
- **C2 [unlisted-tool mechanism — SPEC drift, RESOLVED].** SPEC §5.2.2
  patched 2026-06-16: `meta_catalogue` → MCP **Resource** (`AddResource`,
  `resources/list`); `verify_can_transition` → **custom JSON-RPC method** via
  `AddReceivingMiddleware`; the stale "same SSE channel" wording corrected to
  Streamable HTTP. 27-tool count unchanged. Recorded in §2.3.
- **C3 [validator read surface — 02-spec contract, NOT a root-SPEC edit].**
  SPEC §7.5.1 already *defines* `last_comment_*`/`any_comment_*` correctly
  and §6.2 Tool 14 describes `get_state` as "most recent per kind" without
  claiming it serves those predicates — **no literal root-SPEC
  contradiction**, so this is recorded as an **02-spec contract** (new/
  extended `workitems` read RPC), not a root-SPEC patch. The 02-spec defines
  the RPC (global-latest tuple + per-`(kind,status)` EXISTS) and pins the
  PRECONDITION ordering **gate-first** (02-spec OQ-A RESOLVED 2026-06-16): the
  pipeline comment-trail gate runs in the MCP handler **before**
  `SetStateColumns`, so the pipeline gate's `PIPELINE_PRECONDITION_NOT_MET`
  wins when both would fail; the column invariants are the structural backstop;
  one error wins. Recorded in §2.1 + §2.3 + R-P02-7 above + RP02-2 (§7).
- **C4 [memory deleted_at gap — RESOLVED, additive migration].** NEW
  additive forward migration adds `deleted_at timestamptz` to
  `memory.entries` AND rewrites the three per-scope unique indexes to
  partial-on-not-deleted (`… AND deleted_at IS NULL`). `recall`/`memories`
  filter `deleted_at IS NULL`. **No edit to `0090`.** Recorded in §2.2 +
  §2.5 + B-4. The exact migration file is an 02-spec deliverable.
- **C5 [mcp-warmer cron — infra, RESOLVED 2026-06-16 — documented outlier].**
  Free-tier cron is hourly-min → the AR-16 every-N-minutes in-Encore warmer
  is not deliverable. Non-gating (Q5). Disposition: **document cold-start as
  an accepted launch-period outlier** (AR-16(a) "measured warm only"); **no
  external pinger ships at v1.0** — an external sub-hourly pinger on
  `POST /mcp` remains a v1.x option if cold-start proves painful. This is the
  v1.0 disposition of SPEC §13 AR-16 option (ii); the external pinger
  (option i) is deferred to v1.x. E-3 publishes the measured cold-start
  number on staging. SPEC §13 AR-16 patched 2026-06-16. Recorded in §2.5 + §7
  RP02-7 + §8.5 + §6 Q8.

**Remaining research open questions (Q4 `expires_at`, Q5 sanitiser pattern
set) — dispositioned to the 02-spec (no DDL change, non-flagging):**

- **`expires_at` (research OQ4, R-P02-13, B-6).** Disposition: **couple the
  read-time filter** (`recall`/`memories` filter `WHERE expires_at IS NULL OR
  expires_at > now()`) with the C4 `deleted_at IS NULL` filter as one
  combined predicate; the **write-surface decision** (wire `remember`'s
  optional `expires_at` vs read-filter-only vs inert/reserved) is pinned by
  the 02-spec. No DDL change either way (column already in `0090`). No expiry
  *sweeper* cron in scope — read-time filtering only, matching the
  `mcp.api_keys.expires_at` no-sweeper precedent (`0070`).
- **Sanitiser pattern set (research OQ5, R-P02-9, B-1/B-2).** Disposition:
  the v1.0 regex **baseline is a net-new 02-spec deliverable** (no code
  exists today). The research-recommended starting set — AWS access-key id,
  generic `secret|token|password|api-key`, GitHub PAT prefixes
  (`ghp_/gho_/ghu_/ghs_/ghr_`), PEM block markers, JWT, bearer headers,
  email addresses — is the **starting point, not a locked contract**; the
  02-spec authors and pins the final list. Posture: redact-not-reject,
  audit-on-hit (best-effort per NFR-7/PRD-R6).

### 5.6 Code-grounded constraints for the 02-spec

A code-grounded adversarial review (2026-06-16) confronted these reconciled
docs against the live `apps/api/` code and confirmed 23 findings. The
following are **hard constraints the existing code already imposes** —
things `/spec` → `/tasks` → `/do` would otherwise get wrong. This is a
**constraints checklist for the 02-spec**, not the spec itself: no SQL,
JSON schemas, or migration files are authored here. Each bullet carries its
`file:symbol` anchor and the imperative for the 02-spec author.

1. **[D6-02, HIGH] P01 catalogue guards to mutate.** The P01 catalogue
   codegen has hard guards that FAIL the moment P02 does exactly what
   Track A requires. **The 02-spec MUST** enumerate these guards to mutate:
   `apps/api/cmd/gen-catalogue/main.go:60` `const expectedToolCount = 23`
   (bump → 27 for the four memory tools); `main.go:109-111`
   `die("transitions[] must be empty in P01 …")` (relax/replace with the
   §7.5.1 typed-schema validation so an authored `block_conditions` set
   passes); and the catalogue tests `apps/api/mcp/catalogue_test.go:48-72`
   (`expectedP01ToolNames` 23-name slice → extend to 27 + rename),
   `:129-134` (`TestCatalogueTransitionsEmpty` → invert to assert
   `transitions[]` matches the PRD §6.7 row count), `:114-125`
   (`schema_version` `v0.1` pin → decide the `v0.1→v1` bump). Also note the
   4 new tools need 4 `AddTool` registrations or
   `TestCatalogueNamesMatchToolRegistrars` (`:193-214`) fails.

2. **[D6-01, HIGH] BindDB late-bind for the two net-new services.** **The
   02-spec MUST** require that each of `providers` and `memory` ships its own
   `apps/api/<svc>/db.go` with `var db *sqldb.Database` +
   `func BindDB(d *sqldb.Database) { db = d }` (mirror
   `apps/api/workitems/db.go`), AND that `apps/api/db/db.go` registers both —
   add `encore.app/providers` + `encore.app/memory` to the import block
   (`db.go:182-190`) and `providers.BindDB(DB)` + `memory.BindDB(DB)` to
   `init()` (`db.go:273-280`). `apps/api/db/db.go:33-34` mandates this "no
   exceptions"; without it every providers/memory RPC reads a nil handle and
   panics on first query. (Domain services MUST NOT call `sqldb.Named` /
   `sqldb.NewDatabase` at package init.)

3. **[D6-03, HIGH] providers RBAC — per-table org-scoping classification.**
   Only `providers.installations` has `org_id`
   (`0060_providers.up.sql:10`); `providers.events` (`0060:28-43`) and
   `providers.mappings` (`0060:70-87`) carry only an `installation_id` FK —
   **NO `org_id`**. So the D-3 extension CANNOT use `rbac.For` on
   events/mappings (`rbac.For` emits `<table>.org_id = $1`, which Postgres
   rejects on org_id-less tables — `matrix.go:136-139`). **The 02-spec MUST**
   add three `org.go` Resource constants (`resourceProvidersInstallations` /
   `resourceProvidersEvents` / `resourceProvidersMappings`, all in
   `resourceAllowed`, NONE in `agentReadWriteResources`) and classify:
   `installations` → `rbac.For` (`KindOrgScoped`, carries `org_id`);
   `events` + `mappings` → **Authorize-only** (`KindAuthorizeOnly`), scoped
   via the parent installation's `org_id` FK join (mirroring
   `workitems.comments` → `items`). Add the matching `rbactest` matrix rows
   + a `case "providers.installations":` arm in `selectScopedOrgIDs`.

4. **[D6-04, MED] memory RBAC — B-5 EXTENDS, not net-new.** `memory.entries`
   is ALREADY in the `rbactest` matrix as dual `KindOrgScoped` +
   `KindAuthorizeOnly` (`matrix.go:218-219`) with a `scope='org'` seed row
   and an existing `selectScopedOrgIDs` case (`rbactest_test.go:632`). The
   in-code P02 note at `matrix.go:182-188` spells out the gap. **The 02-spec
   MUST** frame B-5 as EXTENDING the existing entry: add project- and
   user-scope seed rows (`org_id` NULL, `project_id`/`user_id` set per
   `entries_scope_target_chk`, `0090:36-40`) and supply a **non-`rbac.For`**
   isolation predicate for project/user reads (because `rbac.For` emits
   `memory.entries.org_id = $1` — `rbac.go:314` — which passes VACUOUSLY for
   NULL-org_id rows and proves nothing). It is NOT net-new scaffolding.

5. **[D2-3, MED] Layer-1 validator wires INTO the existing handler.** **The
   02-spec MUST** state that the comment-trail gate is invoked inside the
   EXISTING `set_state` path (`apps/api/mcp/handler_set_state.go`, which
   today reads "Layer-1 BLOCK conditions … ship in P02 — NOT enforced here"
   at `:32-34`), gating via the generated `catalogue.gen.go` validator —
   **NOT a new MCP tool**.

6. **[D1-3, MED] intent_comment is post-commit; the gate reads PRE-EXISTING
   comments.** `set_state`'s `intent_comment` is appended best-effort AFTER
   `SetStateColumns` commits, non-atomically
   (`handler_set_state.go:43-44`, `:85-88`), and `SetStateColumns` never
   reads `workitems.comments` (`workitems.go:1782-1790`). **The 02-spec
   MUST** therefore pin: (a) the comment-trail gate evaluates comments
   COMMITTED BEFORE the `set_state` call — the required trail must PRE-EXIST;
   (b) `set_state`'s own `intent_comment` does NOT satisfy its own gate;
   (c) the agent must do **two calls** — append the `(kind=qa,
   status=success)` comment first, THEN call `set_state(qa_state=passed)`;
   (d) the in-call order: identity/validation → Layer-1 comment-trail gate
   (reads pre-existing trail) → column-value invariants inside
   `SetStateColumns` → state write → post-commit `intent_comment` append;
   F-2 asserts this ordering.

7. **[D1-2, LOW] errmap.go discriminator branch for the new error.** The new
   `PIPELINE_PRECONDITION_NOT_MET` needs a dedicated discriminator branch in
   `apps/api/mcp/errmap.go` — within the `errs.FailedPrecondition` arm
   (`:237-297`), route a Meta tag onto a new
   `envelopeKindPipelinePreconditionNotMet` constant BEFORE the terminal
   `envelopeKindPreconditionNotMet` return at `errmap.go:297` (analogous to
   the existing `CYCLE_DETECTED` branch at `:241-260`). Add the constant to
   `errenvelope.go`'s const block, distinct from `envelopeKindPreconditionNotMet`
   (`errenvelope.go:70`). Without it, the gate's rejection silently collapses
   into `PRECONDITION_NOT_MET`. **The 02-spec MUST** pin the discriminator key.

8. **[D6-05, MED] every new secret needs a boot-time fail-fast guard.** **The
   02-spec MUST** require that each new secret (GitHub App ID, private-key
   PEM, `GITHUB_APP_WEBHOOK_SECRET`) is declared in a `var secrets struct`
   with a paired boot-time fail-fast `init()` panic-on-empty guard (matching
   `apps/api/auth/secrets.go:136-150`), landing in the SAME commit as the
   `secrets.nonprod.cue` placeholders + the `SECRETS.md` table row so the CI
   secrets-SoT drift-check stays green and no deploy boots with an empty
   secret. The plan's Track E-2 adds the `.cue` entries but the guard
   obligation is the spec's. (Owner package: likely a new
   `apps/api/providers/secrets.go`, since Encore secrets are package-scoped.)

9. **[D6-06, LOW] prime.memory_hints — POPULATE the existing struct.**
   `apps/api/mcp/handler_prime.go` already declares
   `type primeMemoryHint struct { … }` (`:114-117`), the response field
   `MemoryHints []primeMemoryHint json:"memory_hints"` (`:58`), and the
   empty-literal site `[]primeMemoryHint{}` (`:232`). **The 02-spec MUST**
   frame B-3 as POPULATING this existing shape (and resolve the projection:
   how a `memory.entries` row collapses into `{source, body}`, or whether
   `primeMemoryHint` widens — a backward-additive change, acceptable
   pre-prod), NOT reinventing the schema.

10. **[D6-07, LOW] B-4 soft-delete index rewrite preserves the scope
    predicate.** The three existing partial unique indexes are
    `… (org_id, key) WHERE scope='org'` (and project/user siblings) —
    `0090_memory.up.sql:47-55`. **The 02-spec MUST** require the recreated
    indexes read `WHERE scope='org' AND deleted_at IS NULL` — the
    `deleted_at IS NULL` clause is **ADDITIVE** to the existing scope
    predicate and `(scope-key)` column tuple, NOT a replacement, so per-scope
    uniqueness semantics (and the `entries_scope_target_chk` NULL columns)
    are preserved.

11. **[D1-5, LOW] AR-11 idempotency template — cite the executable INSERT.**
    **The 02-spec MUST** cite the EXECUTABLE template at
    `apps/api/deps/cascade_subscriber.go:742` (`insertCascadeEventRow`) and
    its SQL `INSERT INTO deps.cascade_events … ON CONFLICT (event_id,
    triggered_by_item_id) DO NOTHING` (`:769-779`), plus the publisher-ULID
    `EventID` field on `CascadeRequested` (`cascade.go:53-57`) — **NOT** the
    topic-declaration doc comment at `cascade.go:158-162`. (Plan §2.5 +
    research R-P02-11 citations corrected 2026-06-16.)

12. **[D2-2/D2-5, LOW] catalogue.json authoring shape is greenfield.** Today
    `catalogue.json` is `[schema_version, tools, transitions(empty), $shared]`
    with **zero** `block_conditions` (`catalogue.json`,
    `catalogue.gen.go:31` embeds `"transitions":[]`). **The 02-spec MUST**
    pin: (a) the exact `transitions[].block_conditions` JSON shape per §7.5.1
    (incl. the predicate vocabulary, resolving the `any_comment_*` EXISTS read
    surface per C3); (b) the custom JSON-RPC method-name string for
    `verify_can_transition` (e.g. `unblock/verifyCanTransition`, via
    `AddReceivingMiddleware`, with manual arg-decode) and the
    `meta_catalogue` Resource URI (e.g. `unblock://catalogue`, MIME
    `application/json`, via `AddResource`) — both verified implementable on
    the pinned go-sdk **v1.6.0** (`server.go:1352` `AddReceivingMiddleware`,
    `:519` `AddResource`); (c) regenerate `catalogue.gen.go` via `go generate`
    or the CI drift guard (`catalogue.go:10-12`) fails.

13. **[D3-2, LOW] forward-only migration convention.** `0090_memory` has no
    `.down.sql` (the entire P01 set is up-only, validating
    `feedback_migration_edit_drift`). **The 02-spec MUST** note that every new
    P02 forward migration (the `memory.entries` soft-delete file, the
    `webhook_secret_enc` DROP file, `memory.sanitiser_events`, the providers
    retention additions) ships **up-only — no paired `.down.sql`** — matching
    the existing migration set.

> Note on the go-sdk version: the review **disconfirmed** any version drift —
> `apps/api/go.mod:8` pins `go-sdk v1.6.0` and every doc cites v1.6.0
> consistently. No version string is changed.

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

### Q7. C1 — webhook-secret model (research-surfaced 2026-06-16) — RESOLVED 2026-06-16 — **DROP**

**Decision (user, 2026-06-16): DROP.** Under the confirmed GitHub-App path
(Q4), HMAC verifies against an **app-level Encore secret**
(`GITHUB_APP_WEBHOOK_SECRET`, Track E-2) **only** — one secret for all
installs, delivery disambiguated by `installation.id` in the payload. The
per-install `providers.installations.webhook_secret_enc` column is
**DROPPED via a NEW additive forward migration** (a forward
`ALTER TABLE providers.installations DROP COLUMN webhook_secret_enc` — the
P01 `0060_providers` migration is **NOT** edited, per
`feedback_migration_edit_drift`; `providers.installations` is an empty stub
pre-production so the drop is safe). The column is **re-added by a future
migration** when the OAuth-app / GitLab per-install fallback ships (v1.1).
SPEC §9.4.5 C1 note + changelog patched 2026-06-16 to record the DROP. The
exact migration file is an 02-spec deliverable; added to the §2.5 enumerated
additive-migration list.

### Q8. C5 — mcp-warmer on free-tier hourly cron (research-surfaced 2026-06-16) — RESOLVED 2026-06-16 — **documented outlier**

**Decision (user, 2026-06-16): document cold-start as an outlier (no external
pinger at v1.0).** The free-tier cron hourly-min makes the AR-16 in-Encore
sub-hourly warmer undeliverable; rather than ship an external pinger at
v1.0, **cold-start is accepted as a launch-period outlier** (AR-16(a)
"measured warm only"). **No external pinger ships at v1.0.** E-3 publishes
the measured cold-start number on staging; an **external sub-hourly pinger
remains a v1.x option** if cold-start proves painful in practice. This is
the v1.0 disposition of SPEC §13 AR-16 option (ii); the external pinger
(option i) is deferred to v1.x. Non-gating (Q5); not a functional release
gate. SPEC §13 AR-16 patched 2026-06-16 to mark which option v1.0 takes.

---

## 7. Risks Specific to P02

P02-level risks (SPEC §13 AR-* covers architecture-wide risks; PRD §12
covers product-wide risks).

| # | Risk | Mitigation |
|---|---|---|
| RP02-1 | **Bidirectional sync loop / GitHub rate-limit exhaustion.** A naive sync writes to GitHub, the echo webhook re-triggers normalisation, which re-writes — a storm that burns the 5000/hr REST budget. | **Research R-P02-3 CONFIRMED all suppressors available** (R1): App-bot `sender.login` allowlist (fast path) + `last_synced_at` echo window (`0060` columns exist) + content-idempotent normalise (structural backstop); go-github surfaces `x-ratelimit-*`/`retry-after` as typed errors so the reconciler honours them. 02-spec combines actor-allowlist + idempotent-normalise. Sync is opt-in per installation, so blast radius is bounded. |
| RP02-2 | **Layer-1 validator double-validates against the P01 column-value invariants with conflicting errors.** P01 already enforces the §6.2 column rules — but **split across two RPCs** (D1-4): `set_state` carries four (I-1,I-2,I-4,I-5) + the structural `impl_done_requires_claim`; `claim` carries I-3. P02 adds comment-trail gates on `set_state`. A clumsy merge yields two rejection codes for one bad transition. | A-3 reconciles the layers into one validator pass; the spec pins the **per-RPC, gate-first** ordering (§2.3 sidebar; 02-spec §4.4.5 / OQ-A RESOLVED 2026-06-16) — the comment-trail gate runs in the MCP handler **before** `SetStateColumns`, so the pipeline gate's `PIPELINE_PRECONDITION_NOT_MET` wins when both would fail (column invariants = structural backstop). F-2 asserts a single, correct rejection. |
| RP02-3 | **Catalogue drift between PRD §6.7, `catalogue.json`, and `catalogue.gen.go`.** Three representations of the same state machine; hand-authoring the JSON from the PRD table invites transcription error. | AR-4 CI drift test (now load-bearing, A-6) diffs the Go-codegen corner against the live `meta_catalogue`; Ada keeps PRD §6.7 ↔ JSON in sync; the spec includes a per-transition test matrix. |
| RP02-4 | **Webhook HMAC / dedup edge cases.** Replayed `X-GitHub-Delivery`, signature-mismatch, oversized payloads, or a payload-sanitiser false-positive that mangles a legit field. | AR-12 unique constraint + 200-on-duplicate; HMAC verified before any processing; sanitiser is redact-not-reject; F-1 covers the happy path + a replay + a bad-signature case. |
| RP02-5 | **Secret sanitiser false negative leaks a credential into `ts_doc` (unencrypted, GIN-indexed).** AR-10/AR-14: `ts_doc` is plaintext-derived and unencrypted by necessity. | **Research R-P02-9 (R2):** no v1.0 pattern set exists yet — the regex baseline is a NET-NEW 02-spec deliverable (best-effort per NFR-7/PRD-R6). The sanitise-before-tokenise **ordering is a service-code invariant** (DDL enforces nothing — B-1 must implement it). Recovery net: `sanitiser_events` audit + periodic re-scan (B-2, AR-14) make a missed pattern recoverable; `ts_doc` is SELECT-locked to the `memory` connection user (AR-10). |
| RP02-6 | **`forget` DDL gap (no `deleted_at`).** SPEC §5.2.2 promises soft-delete but §9.4.8 has no column for it. | R-P02-10 / B-4 resolve the additive column + whether the unique indexes go partial-on-not-deleted before B-3 ships `forget`. |
| RP02-7 | **Encore Cloud free-tier ceiling binds at first real deploy (AR-13); free-tier cron is hourly-min (C5).** Webhook ingestion + three cron jobs + the warmer + MCP traffic may exceed the free-tier Pub/Sub rate or connection cap. | **Research R-P02-12 CONFIRMED concrete ceilings** (100k req/day, 100k Pub/Sub msgs/day, 1 GB DB, **cron hourly-min**, 2 cloud envs) — Pub/Sub volume is ample at v1 scale; pooled DB bindings already compliant (`db/db.go`). **C5 (RESOLVED 2026-06-16 — documented outlier, §6 Q8):** the AR-16 every-N-minutes `mcp-warmer` is **not deliverable in-Encore on the free tier** (hourly-min); the v1.0 disposition is **document cold-start as an accepted launch-period outlier** (SPEC §13 AR-16 option ii) — **no external pinger ships at v1.0** (the external sub-hourly pinger, option i, is a v1.x option if cold-start proves painful). E-3 publishes the measured cold-start number on staging before D goes live; the AR-1 exit path (NATS + standard Postgres) is the documented escape; capacity is a report (Q5), not a launch blocker. |
| RP02-8 | **Migration drift: P02 edits a P01 migration in place.** Per `feedback_migration_edit_drift`, editing an applied migration silently drifts the long-lived staging + local DBs even while CI stays green on a fresh run. | All P02 migrations are new forward files, higher-numbered than `0140`; CI runs migrations fresh AND a staging-replay check confirms forward-only. |

---

## 8. Acceptance Criteria for P02

This phase is **DONE** when all of the following are demonstrably true.

### 8.1 Functional acceptance (PRD §8 + SPEC §11 P02 exit criterion)

- [ ] A GitHub repository can be linked (`providers.LinkRepo` creates an
      installation; under the confirmed App path the webhook secret is the
      app-level Encore secret, not per-install — C1, §2.4).
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
      (E-1) + provider App secret set (App ID + PEM + **app-level**
      `GITHUB_APP_WEBHOOK_SECRET`, C1) + `MEMORY_DEK` provisioned (E-2);
      AR-13 free-tier ceiling report published (E-3). **AR-16 warmer
      (C5, RESOLVED 2026-06-16 — documented outlier, §6 Q8):** the report
      records that free-tier cron is hourly-min so an in-Encore sub-hourly
      warmer is undeliverable; the v1.0 disposition is **cold-start
      documented as an accepted launch-period outlier** (SPEC §13 AR-16
      option ii) with **no external pinger at v1.0** (external sub-hourly
      pinger deferred to v1.x as option i).
      Confirms the required work landed; **not a pass/fail release gate** on
      the P02 functional exit criterion (mirrors Q5 — staging + capacity are
      required *work*, not a functional gate).

---

## 9. Sequencing Notes

- **`/research` is CLOSED (2026-06-16).** R-P02-1 through R-P02-13 (incl.
  the R-P02-3b / R-P02-4b siblings) are verified in
  `docs/research/02-research-backend-complete.md` — 9 CONFIRMED, 3 PARTIAL,
  1 CONTRADICTED. The load-bearing flips are reconciled into §2 / §5.5 and
  the plan is **re-approved**: R-P02-7/C3 flipped the "no new validator RPC"
  assumption (§2.1); R-P02-6/C2 contradicted the hidden-tool primitive
  (SPEC §5.2.2 patched, §2.3); R-P02-4/C1 made the webhook secret app-level —
  **Q7 RESOLVED 2026-06-16 = DROP** the per-install `webhook_secret_enc`
  column via a new additive forward migration (§2.4 + §2.5(d); `0060`
  unedited; re-added at v1.1); R-P02-10/C4 confirmed the `forget` DDL gap
  (§2.5, B-4); R-P02-12/C5 made the in-Encore warmer free-tier-undeliverable —
  **Q8 RESOLVED 2026-06-16 = documented cold-start outlier**, no external
  pinger at v1.0 (§2.5 + §7 RP02-7 + §8.5). The three SPEC patches (C1/C2/C5)
  are spec-first on main; C3
  and C4 are recorded as 02-spec contracts, not root-SPEC edits (§5.5).
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
  AR-13, AR-14, AR-16). **Research-reconciled 2026-06-16:** §5.2.2 (C2 —
  `verify_can_transition` custom JSON-RPC method / `meta_catalogue` Resource;
  SSE→Streamable-HTTP), §9.4.5 (C1 — app-level webhook-secret; **Q7 RESOLVED
  2026-06-16 = DROP** the per-install `webhook_secret_enc` column via a new
  additive forward migration, `0060` unedited), §13 AR-16 (C5 — free-tier
  hourly-cron warmer; **Q8 RESOLVED 2026-06-16 = documented cold-start
  outlier**, option ii, no external pinger at v1.0).
- **Manifesto** Laws L3 (Postgres source of truth / provider events), L8
  (pipeline gates enforced architecturally — Layer 1 in P02).
- **Predecessor:** P01 plan §2.1 (schema-only stubs for providers/memory),
  §3.4 (Layer-1 deferral to P02), Q2 (all 8 schemas migrate in P01), Q6
  (Encore Cloud staging is a P02 ops task).
- **Explicitly excluded:**
  `docs/archive/agentic-research/RFC-REACTIVE-AGENT-ENVIRONMENT.md`
  (P02+/additive, not P02 — §3.2).
