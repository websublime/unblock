# SPEC: P02 — Backend Complete (providers + memory tools + Layer-1 pipeline enforcement)

**Status:** APPROVED *(2026-06-16 — all five open questions resolved by the
user, each to this spec's documented default: **OQ-A = gate-first** (pipeline
comment-trail gate in the MCP handler before `SetStateColumns`; column
invariants are the structural backstop; the pipeline gate's
`PIPELINE_PRECONDITION_NOT_MET` wins when both would fail — plan §2.3 + §5.5 C3
row + §7 RP02-2 reconciled to gate-first the same day); **OQ-B = wire the write
surface** (`remember` optional `expires_at`; `recall`/`memories` filter
`expires_at IS NULL OR expires_at > now()`; no sweeper cron); **OQ-C = keep the
7-pattern sanitiser baseline** (no SaaS additions at v1.0; registry extensible
post-v1.0); **OQ-D = confirmed** (migration slots 0150/0160/0170, bump-to-next-
free if taken, ordering load-bearing); **OQ-E = confirmed** (method
`unblock/verifyCanTransition`, Resource URI `unblock://catalogue`). §11 records
each as RESOLVED 2026-06-16.)*
**Author:** Ada (architect)
**Date:** 2026-06-16
**Source PRD:** [docs/PRD.md](../PRD.md) (APPROVED 2026-05-07) — §5.1 (FR-8/9/10/11/12/13), §5.2 (FR-14), §6.2 (state invariants), §6.5 (comment trail), §6.7 (pipeline state machine), §8 (P02 exit criterion), §9.2 (GitLab v1.1), §12 (R-2/R-6/R-7)
**Source SPEC:** [docs/SPEC.md](../SPEC.md) (APPROVED 2026-05-07, round-6 2026-05-12; research patches C1/C2/C5 applied 2026-06-16) — §5.2.2, §5.3, §5.6, §5.7/§5.7.1, §7.4, §7.5/§7.5.1/§7.5.2/§7.5.3, §9.4.5, §9.4.8, §9.4.10, §11, §13 (AR-1/4/7/10/11/12/13/14/16)
**Source Plan:** [docs/plans/02-plan-backend-complete.md](../plans/02-plan-backend-complete.md) (APPROVED 2026-06-16, research-reconciled + code-grounded)
**Source Research:** [docs/research/02-research-backend-complete.md](../research/02-research-backend-complete.md) (Smith, 2026-06-16; 9 CONFIRMED / 3 PARTIAL / 1 CONTRADICTED; C1–C5 resolved)
**Predecessor spec:** [docs/specs/01-spec-backend-mvp.md](./01-spec-backend-mvp.md) (APPROVED)

> Stage 2 implementation contract. This document turns the APPROVED P02 plan
> into the exact, machine-checkable artifacts `/tasks` → `/do` consume: the
> migration files, the RPC signatures, the MCP tool JSON schemas, the
> `catalogue.json` `block_conditions` section + per-transition matrix, the
> error-envelope discriminators, the GitHub field map, the Pub/Sub topic
> shapes, and the sanitiser pattern set. It deviates from no SPEC §9.4 DDL
> column type, constraint, or relationship (the §9.4 DDL exception); it only
> **adds** via new forward migrations (§3.1).
>
> Every binding in this spec carries a `file:symbol` or `file:line` anchor to
> the live `apps/api/` code or the SPEC DDL. The plan §5.6 enumerated 13
> code-grounded constraints; each is discharged below and cross-referenced
> as **CG-N**.

---

## 1. Overview

P02 completes the agent-facing backend across three pillars, all in
`apps/api/` Go + infra (no `crates/` code — NFR-9):

1. **Providers (`providers` service, FR-11, Law 3)** — link a GitHub repo,
   ingest HMAC-verified webhooks, normalise GitHub issue/PR events into
   canonical `workitems.items`, push `://unblock` mutations back to GitHub,
   reconcile missed deliveries on a cron.
2. **Memory (`memory` service, FR-13)** — the four memory MCP tools
   (`remember`/`recall`/`memories`/`forget`) over an always-on secret
   sanitiser + `pgcrypto` encrypt-at-rest, raising the MCP tool surface
   **23 → 27**.
3. **Pipeline enforcement Layer 1 (FR-14, Law 8 layer 1)** — the MCP
   state-transition validator: every PRD §6.7 transition encoded as
   `catalogue.json` `block_conditions`, codegen-compiled into
   `catalogue.gen.go`, gating `set_state` / `claim` / `close`, re-validated
   via the `verify_can_transition` primitive, served via the
   `meta_catalogue` Resource.

**Exit criterion (PRD §8 + SPEC §11):** a GitHub repo can be linked, webhooks
normalise events into canonical work items, and an attempt to mark a work
item `done` without the required comment trail is rejected at the MCP boundary
(`PIPELINE_PRECONDITION_NOT_MET`); `meta_catalogue` serves the live catalogue;
`verify_can_transition` agrees with the Layer-1 validator.

**Out of scope (plan §3):** boards service code (P05), Reactive Agent
Environment (P02+/additive), GitLab provider (v1.1), Layer 2/3 enforcement
(P04), Astro web client (P05), AST CLI + plugin renderer (P03/P04), any new
persistent store.

### 1.1 Resolved decisions carried into this spec (binding)

| Ref | Decision | Spec consequence |
|---|---|---|
| Q1 | boards → P05 | no `apps/api/boards/*.go` in P02; `boards/.gitkeep` corrected in-place (§3.6) |
| Q2 | P01 Layer-1 deferral lands here | `close` tightened to require full trail / `qa_state=passed` (§4.4) |
| Q3 / R-P02-11 | webhook→normalise **async** via `provider.events` | §6.4 topic + subscriber; AR-11 ULID `EventID` idempotency |
| Q4 / R-P02-4 | **GitHub App** (installation tokens) | §6.2 secret model; §3.2 client pins |
| Q5 | staging ships, **not** the functional gate | §9 acceptance: local-emulator + CI green is the bar; staging is required *work* |
| Q6 | no memory latency PRD gate | §5 carries a spec-level NFR only |
| Q7 / C1 | **DROP** `webhook_secret_enc` | §3.1(d) forward DROP migration; HMAC verifies the app-level Encore secret only |
| Q8 / C5 | **documented cold-start outlier**, no external pinger v1.0 | §8 ops; no in-Encore sub-hourly warmer cron |

---

## 2. Research findings carried into the design (verified facts)

Every binding below is grounded in [02-research](../research/02-research-backend-complete.md).
This section records where the design follows a **verified** fact rather than
the original plan assumption.

| Research item | Verdict | Design consequence in this spec |
|---|---|---|
| R-P02-1 | CONFIRMED | §6.1 HMAC = `hex(HMAC-SHA256(secret, raw_body))`, `hmac.Equal` over unparsed bytes; `X-GitHub-Delivery` = AR-12 dedup key; v1.0 sub set = `issues` + `pull_request`. |
| R-P02-2 | PARTIAL | §6.3 GitHub field map pinned at REST (go-github); §6.3.3 unmapped-field degradation pinned. githubv4 deferred. |
| R-P02-3 | CONFIRMED | §6.5 loop suppression = App-bot `sender.login` allowlist (fast) + content-idempotent normalise (backstop) + `last_synced_at` echo window. |
| R-P02-3b | CONFIRMED | §3.2 pins `google/go-github/v88` + `bradleyfalzon/ghinstallation/v2`; `golang-jwt/jwt/v5` already vendored. |
| R-P02-4 / **C1** | CONFIRMED (App) | §6.2 App credential set; §6.1 HMAC against app-level Encore secret; §3.1(d) DROP `webhook_secret_enc`. |
| R-P02-4b | CONFIRMED | §6.1.2 failure-status contract (401/404/400/413/200-dup/503). |
| R-P02-5 | CONFIRMED | §4.6 maps `PIPELINE_PRECONDITION_NOT_MET` via existing `errmap.go` `*sdkjsonrpc.Error{Code:-32000}` path. |
| R-P02-6 / **C2** | CONTRADICTED | §4.7 `meta_catalogue` = MCP Resource (`AddResource`); §4.8 `verify_can_transition` = custom JSON-RPC method (`AddReceivingMiddleware`). Neither via `AddTool`. 27-tool count unchanged. |
| R-P02-7 / **C3** | PARTIAL | §4.2 new `workitems.GetCommentTrailPredicates` RPC (P01 `GetState.recent_kinds` is `DISTINCT ON (kind)` — cannot serve EXISTS-ever); §4.3 predicate semantics = **ever-existed** (EXISTS-per-`(kind,status)`). |
| R-P02-8 | CONFIRMED | §5.3 `pgp_sym_encrypt(..,$dek,'cipher-algo=aes256,compress-algo=2')`; `memory` package gets own `var secrets struct { MemoryDEK string }`. |
| R-P02-9 | PARTIAL | §5.4 sanitiser pattern set authored (no v1.0 set existed); sanitise→tokenise→encrypt ordering pinned as service-code invariant. |
| R-P02-10 / **C4** | CONFIRMED | §3.1(b) `deleted_at` + partial-index rewrite migration; §5.2 `recall`/`memories` filter `deleted_at IS NULL`. |
| R-P02-11 | CONFIRMED | §6.4 async path; AR-11 publisher-ULID `EventID` idempotency, distinct from AR-12. |
| R-P02-12 / **C5** | CONFIRMED | §8 free-tier ceilings; in-Encore sub-hourly warmer undeliverable → cold-start documented outlier, no pinger v1.0. |
| R-P02-13 | CONFIRMED (inert) | §5.2 `expires_at` read-time filter wired; write surface = optional on `remember` (§5.1.1); no sweeper cron. |

---

## 3. Migrations, repo, and dependency pins

### 3.1 New forward migrations (additive only; up-only; higher-numbered than `0140`)

Per `feedback_migration_edit_drift` and CG-13: **no P01 migration is edited
in place**; every P02 migration is a new forward file under
`apps/api/db/migrations/` (the dedicated zero-API `db` migration-owner
service; the sole `sqldb.NewDatabase("unblock", …)` owner is
`apps/api/db/db.go`). The P01 set ends at `0140_deps_cascade_events_kind_chk_fix`.
None ships a `.down.sql` (the entire P01 set is up-only — CG-13).

| File | Purpose | Source |
|---|---|---|
| `0150_memory_sanitiser_events.up.sql` | AR-14 audit table (§5.4.3) | plan §2.1, AR-14 |
| `0160_memory_entries_soft_delete.up.sql` | add `memory.entries.deleted_at` + rewrite the 3 per-scope unique indexes partial-on-not-deleted (CG-10) | C4, B-4 |
| `0170_providers_drop_webhook_secret.up.sql` | `ALTER TABLE providers.installations DROP COLUMN webhook_secret_enc` (C1, Q7) | C1, plan §2.5(d) |

> File numbers `0150`/`0160`/`0170` are the canonical P02 slots. If a slot is
> taken by a migration merged between approval and `/do`, the implementer
> bumps to the next free slot and updates the §11 traceability row; the
> *ordering* (sanitiser_events before nothing dependent; soft-delete and the
> DROP are independent) is what is load-bearing.

#### 3.1(a) `0150_memory_sanitiser_events.up.sql`

```sql
-- AR-14: audit every secret-sanitiser hit. Added in P02 (no P01 DDL).
-- One row per sanitiser match (a single remember may produce N rows).
CREATE TABLE memory.sanitiser_events (
    id              text         PRIMARY KEY,                 -- ULID
    entry_id        text         REFERENCES memory.entries(id) ON DELETE CASCADE,
                                                              -- nullable: a periodic re-scan hit
                                                              -- (AR-14) or a sanitiser hit on a
                                                              -- providers payload records NULL here
    scope           text,                                    -- 'org'|'project'|'user' for memory hits; NULL for providers hits
    org_id          text         REFERENCES org.organizations(id) ON DELETE CASCADE,
    pattern_id      text         NOT NULL,                    -- stable id from the §5.4.1 registry, e.g. 'github_pat'
    category        text         NOT NULL,                    -- 'credential' | 'pii' (coarse class)
    source          text         NOT NULL,                    -- 'memory_write' | 'memory_rescan' | 'providers_payload'
    redaction_count integer      NOT NULL DEFAULT 1,          -- matches collapsed under one (pattern, field)
    detected_at     timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT sanitiser_events_category_chk
        CHECK (category IN ('credential', 'pii')),
    CONSTRAINT sanitiser_events_source_chk
        CHECK (source IN ('memory_write', 'memory_rescan', 'providers_payload'))
);
CREATE INDEX sanitiser_events_entry_idx ON memory.sanitiser_events (entry_id);
CREATE INDEX sanitiser_events_org_detected_idx
    ON memory.sanitiser_events (org_id, detected_at DESC);
CREATE INDEX sanitiser_events_pattern_idx ON memory.sanitiser_events (pattern_id);
```

> `entry_id` is **nullable** so the table serves both the memory sanitiser
> (entry-scoped) and the providers payload sanitiser (no `memory.entries`
> row), and the AR-14 periodic re-scan (entry exists). `org_id` is recorded
> for RBAC-scoped audit reads even when `entry_id` is NULL (providers hits
> carry the installation's `org_id`).

#### 3.1(b) `0160_memory_entries_soft_delete.up.sql`

CG-10: the `deleted_at IS NULL` clause is **additive** to the existing
`WHERE scope='…'` predicate and the existing `(scope-target, key)` tuple —
NOT a replacement. Per-scope uniqueness and the `entries_scope_target_chk`
NULL columns are preserved.

```sql
-- C4 / B-4: forget is a soft-delete (audit trail preserved). memory.entries
-- has no deleted_at in 0090; add it and rewrite the three per-scope unique
-- indexes partial-on-not-deleted so forget→re-remember of the same
-- (scope,key) does not violate the live unique index. 0090 is NOT edited.
ALTER TABLE memory.entries ADD COLUMN deleted_at timestamptz;

DROP INDEX memory.entries_org_key_uniq;
DROP INDEX memory.entries_project_key_uniq;
DROP INDEX memory.entries_user_key_uniq;

CREATE UNIQUE INDEX entries_org_key_uniq
    ON memory.entries (org_id, key)
    WHERE scope = 'org' AND deleted_at IS NULL;
CREATE UNIQUE INDEX entries_project_key_uniq
    ON memory.entries (project_id, key)
    WHERE scope = 'project' AND deleted_at IS NULL;
CREATE UNIQUE INDEX entries_user_key_uniq
    ON memory.entries (user_id, key)
    WHERE scope = 'user' AND deleted_at IS NULL;
```

#### 3.1(c) — RESERVED (no providers digest DDL needed)

The §9.4.5 providers retention/digest job operates entirely on the existing
`providers.events.payload` / `received_at` / `error` columns (it rewrites
`payload` in place to a metadata-only digest — §6.6). **No additive providers
migration is required** for the digest job. This slot is intentionally empty;
plan §2.5(c) ("anything the digest job requires beyond the shipped DDL")
resolves to **nothing**.

#### 3.1(d) `0170_providers_drop_webhook_secret.up.sql`

```sql
-- C1 / Q7 (RESOLVED 2026-06-16 = DROP). Under the confirmed GitHub-App path
-- the webhook secret is app-level (one secret for all installs); HMAC
-- verifies against the Encore secret GITHUB_APP_WEBHOOK_SECRET, NOT a
-- per-install column. providers.installations is an empty stub
-- pre-production, so the drop is safe. Re-added by a future migration when
-- the OAuth-app / GitLab per-install fallback ships (v1.1). 0060 NOT edited.
ALTER TABLE providers.installations DROP COLUMN webhook_secret_enc;
```

> Consequence for §9.4.5 DDL: `providers.installations.installation_id_enc`
> remains `NOT NULL` (App installation id, encrypted). After `0170` the only
> `*_enc` column on the table is `installation_id_enc` (CG-2).

### 3.2 Dependency pins (R-P02-3b CONFIRMED)

| Module | Pin | Role |
|---|---|---|
| `github.com/google/go-github/v88` | v88.x (latest major, 2026-05-21) | REST client: `Issues.Create/Edit/ListByRepo`, rate-limit typed errors |
| `github.com/bradleyfalzon/ghinstallation/v2` | latest v2 | App installation-token RoundTripper (JWT-signed, 1-hour tokens) |
| `github.com/golang-jwt/jwt/v5` | v5.3.1 (already in `go.sum`, transitive) | App-JWT primitive ghinstallation needs |
| `shurcooL/githubv4` | **NOT added** at v1.0 | deferred unless a GraphQL-only field forces it (§6.3 — not expected) |

The outbound writer (D-1) and reconciler (D-2) build on go-github's
`*RateLimitError` / `*AbuseRateLimitError{RetryAfter}` typed errors +
`x-ratelimit-*` headers; they MUST NOT hand-roll `net/http` (R-P02-3b).

### 3.3 New service packages and BindDB late-bind (CG-2)

Each net-new domain service declares its own `db.go` mirroring
`apps/api/workitems/db.go`:

```go
// apps/api/providers/db.go  (and apps/api/memory/db.go, identical shape)
package providers
import "encore.dev/storage/sqldb"
var db *sqldb.Database
func BindDB(d *sqldb.Database) { db = d }
```

`apps/api/db/db.go` registers both (CG-2; `db.go:33-34` mandates this "no
exceptions"): add `encore.app/providers` + `encore.app/memory` to the import
block (`db.go:182-190`) and `providers.BindDB(DB)` + `memory.BindDB(DB)` to
`init()` (`db.go:273-280`). Domain services MUST NOT call `sqldb.Named` /
`sqldb.NewDatabase` at package init (panics under plain `go test`).

### 3.4 Secrets and boot-time fail-fast guards (CG-8)

Each new secret is declared in a package-scoped `var secrets struct` with a
paired `init()` panic-on-empty guard (mirror `apps/api/auth/secrets.go:136-150`),
landing in the **same commit** as the `secrets.nonprod.cue` placeholder + the
`SECRETS.md` table row so the CI secrets-SoT drift-check stays green.

| Logical secret | Go field | Owner package | Source |
|---|---|---|---|
| `GITHUB_APP_ID` | `GitHubAppID` | `apps/api/providers/secrets.go` (new) | E-2, R-P02-4 |
| `GITHUB_APP_PRIVATE_KEY` | `GitHubAppPrivateKey` | `apps/api/providers/secrets.go` | E-2 (PEM) |
| `GITHUB_APP_WEBHOOK_SECRET` | `GitHubAppWebhookSecret` | `apps/api/providers/secrets.go` | E-2, C1 (app-level HMAC) |
| `MEMORY_DEK` | `MemoryDEK` | `apps/api/memory/secrets.go` (new) | R-P02-8 (same DEK as auth/providers) |

`apps/api/SECRETS.md` + `apps/api/secrets.nonprod.cue` gain
`GitHubAppID` / `GitHubAppPrivateKey` / `GitHubAppWebhookSecret` placeholders
(none exist today — the registry holds OAuth-app secrets only). `MemoryDEK`
already has a `secrets.nonprod.cue:62` placeholder (R-P02-8); the `memory`
package needs its own `var secrets struct { MemoryDEK string }` (Encore
secrets are package-scoped; the CI drift-check unions packages).

### 3.5 JSON wire-tag rule (NFR-10)

Every exported field of every Go struct in `providers` / `memory` and the new
`mcp` structs that may transit JSON declares an explicit `json:"snake_case"`
tag. `grep -rnE 'json:"[A-Z]' apps/api/` returns zero. MCP SDK / JSON-RPC
protocol fields (`jsonrpc`, `protocolVersion`, …) follow the MCP 2025-06-18
spec verbatim; third-party GitHub unmarshal structs (go-github types) are
exempt as third-party wire structs.

### 3.6 Repo hygiene (CG-1 trigger sibling; plan §2.6)

- `apps/api/providers/.gitkeep` + `apps/api/memory/.gitkeep` (both cite the
  stale `apps/api/auth/migrations/…` path) → **deleted** once the service
  `.go` files land in P02.
- `apps/api/boards/.gitkeep` (cites `auth/migrations/0080_boards.up.sql`;
  boards code is P05 so the "service files land" trigger never fires in P02)
  → **corrected in-place** to `apps/api/db/migrations/0080_boards.up.sql`.

---

## 4. Layer-1 pipeline enforcement (Track A)

### 4.1 The transition set (PRD §6.7 → `catalogue.json`)

The PRD §6.7 table is the human-readable counterpart of the
`catalogue.json` `transitions[]`. Each §6.7 row becomes one transition object
with one `block_conditions` entry (typed schema SPEC §7.5.1). Ada keeps the
PRD §6.7 table and the JSON in sync; the CI drift test (A-6) enforces equality.

| # | Transition id | Tool | `required_state` predicate | error_code |
|---|---|---|---|---|
| T1 | `claim` | `claim` | `status="Ready"`, `claimed=false` | `PIPELINE_PRECONDITION_NOT_MET` |
| T2 | `impl_state.pending->done` | `set_state` | `claimed=true`, `any_comment_kind="completed"` (status any) | `PIPELINE_PRECONDITION_NOT_MET` |
| T3 | `review_state.pending->approved` | `set_state` | `impl_state="done"`, `any_comment_kind="review"`, `any_comment_status="success"` | `PIPELINE_PRECONDITION_NOT_MET` |
| T4 | `review_state.pending->needs_rework` | `set_state` | `impl_state="done"`, `any_comment_kind="review"`, `any_comment_status="warning"` | `PIPELINE_PRECONDITION_NOT_MET` |
| T5 | `qa_state.pending->passed` | `set_state` | `review_state="approved"`, `any_comment_kind="qa"`, `any_comment_status="success"` | `PIPELINE_PRECONDITION_NOT_MET` |
| T6 | `qa_state.pending->failed` | `set_state` | `review_state="approved"`, `any_comment_kind="qa"`, `any_comment_status="error"` | `PIPELINE_PRECONDITION_NOT_MET` |
| T7 | `qa_state.failed->passed` (override) | `set_state` | `any_comment_kind="override"`, `any_comment_status="warning"`, `override_body_min=20` | `PIPELINE_PRECONDITION_NOT_MET` |
| T8 | `pipeline_state.running->needs_human` | `set_state` | (disjunctive: 3× needs_rework OR 3× qa_failed OR claim/worktree conflict OR explicit flag) — see §4.5 | `PIPELINE_PRECONDITION_NOT_MET` |
| T9 | `Status.*->Done` | `close` | `qa_state="passed"` | `PIPELINE_PRECONDITION_NOT_MET` |

> **T9 is the exit criterion.** `close` (and `set_state(qa_state=passed)`
> reaching it) without the review-then-QA trail is rejected. T9's
> `required_state` is `qa_state="passed"`; `qa_state` itself only reaches
> `passed` through T5 (which requires the review-approved + qa-success trail)
> or T7 (override). So the trail requirement is enforced transitively at T5/T7
> and structurally at T9.

#### 4.1.1 Predicate vocabulary additions (this spec extends SPEC §7.5.1)

SPEC §7.5.1 `required_state` defines `impl_state` / `review_state` /
`qa_state` / `pipeline_state` / `last_comment_kind` / `last_comment_status` /
`any_comment_kind` / `any_comment_status` / `claimed`. P02 transitions need
two additions, both authored here as 02-spec contracts (additive to the §7.5.1
shape; the CI drift test validates against the typed schema, CG-12):

- `status` (string) — the §6.1 work-item Status enum value that must hold
  (used by T1 `claim`).
- `override_body_min` (integer) — the minimum `body` length on the matched
  override comment (T7). Evaluated by the validator against the matched
  `(kind=override, status=warning)` comment's body length.

`last_comment_*` (single most-recent comment across all kinds) is **declared
in the schema but unused by any P02 transition** — every PRD §6.7 gate is an
existence (`any_comment_*`) predicate (§4.3). The validator populates
`last_comment_*` from the new RPC (§4.2) so the schema field is non-vacuous
and Layer 3 (P04) can render it, but no P02 transition keys on it.

### 4.2 The validator read RPC (C3 — CG-3 sibling; plan §2.1)

P01's `workitems.GetState.recent_kinds` is `DISTINCT ON (kind)` (latest-per-kind,
`workitems/workitems.go:1333`). Per R-P02-7 this **cannot** answer (a) the
global `last_comment_*` predicate, nor (b) a history-aware `any_comment_*=ever`
predicate (a later same-kind comment overwrites an earlier success). P02 adds
a new private RPC:

```go
// apps/api/workitems/workitems.go (new RPC; //encore:api private)
type CommentTrailPredicatesRequest struct {
    ItemID      string `json:"item_id"`
    CallerOrgID string `json:"caller_org_id"` // tenant filter, mirror SetStateColumns
}
type CommentTrailPredicatesResponse struct {
    // Global most-recent comment across all kinds (ORDER BY created_at DESC LIMIT 1).
    LastCommentKind   string `json:"last_comment_kind"`   // "" if no comments
    LastCommentStatus string `json:"last_comment_status"` // "" if no comments
    // Per-(kind,status) ever-existed booleans (EXISTS, history-aware).
    Predicates []CommentTrailPredicate `json:"predicates"`
}
type CommentTrailPredicate struct {
    Kind   string `json:"kind"`   // §6.5 kind enum
    Status string `json:"status"` // §6.5 status enum
    Exists bool   `json:"exists"` // a comment with this exact (kind,status) ever exists
    // For the override gate: the max body length among matching comments.
    MaxBodyLen int `json:"max_body_len"`
}
```

Backing query (single round-trip; both predicate forms in one read):

```sql
-- Global latest comment.
WITH latest AS (
  SELECT kind, status
    FROM workitems.comments
   WHERE item_id = $1
   ORDER BY created_at DESC
   LIMIT 1
),
-- Per-(kind,status) existence + max body length, history-aware.
existence AS (
  SELECT kind, status, count(*) > 0 AS ex, COALESCE(max(length(body)), 0) AS max_body
    FROM workitems.comments
   WHERE item_id = $1
   GROUP BY kind, status
)
SELECT (SELECT kind FROM latest), (SELECT status FROM latest),
       existence.kind, existence.status, existence.ex, existence.max_body
  FROM existence;
```

The generated validator (§4.4) requests only the `(kind,status)` pairs its
transitions reference, so the response is bounded (no N+1 per transition: one
RPC call per `set_state`/`claim`/`close` invocation feeds all candidate
gates). Tenant filtering reuses the `($2='' OR org_id=$2)` shape from
`SetStateColumns` (`workitems.go:1786`) via an item-scoped pre-check.

### 4.3 Predicate semantics — **ever-existed** (C3 resolution)

PRD §6.7 gates read "comment trail **includes** `kind=…, status=…`" and SPEC
§7.5.3 uses `any_comment_kind`/`any_comment_status`. The validator therefore
evaluates `any_comment_*` as **ever-existed** existence: a
`(kind=qa, status=success)` comment that ever existed satisfies T5 even if a
later `(kind=qa, status=error)` comment was appended afterward. This is the
`CommentTrailPredicate.Exists` EXISTS form (§4.2), **not** the latest-per-kind
form `recent_kinds` provides. This resolves Open Question 3.

### 4.4 Codegen, validator wiring, and per-RPC ordering (CG-1, CG-5, CG-6)

#### 4.4.1 Codegen guards to mutate (CG-1)

`apps/api/cmd/gen-catalogue/main.go`:
- `:60` `const expectedToolCount = 23` → **27** (the four memory tools).
- `:109-111` `die("transitions[] must be empty in P01 …")` → **replace** with
  §7.5.1 typed-schema validation so an authored `block_conditions` set passes
  (validate each transition object against the §4.1.1-extended shape; reject
  unknown predicate keys).

`apps/api/mcp/catalogue_test.go`:
- `:48-72` `expectedP01ToolNames` (23-name slice) → extend to 27 (add
  `remember`/`recall`/`memories`/`forget`); rename to drop the P01 framing.
- `:129-134` `TestCatalogueTransitionsEmpty` → **invert** to assert
  `len(transitions) == <PRD §6.7 row count>` (= 9, §4.1) and that each carries
  exactly the §4.1 predicate set.
- `:114-125` `schema_version` `v0.1` pin → **bump to `v1`** (the catalogue now
  carries load-bearing transitions; the bump is the version signal P04's Rust
  `include_str!` corner pins against).
- `:193-214` `TestCatalogueNamesMatchToolRegistrars` → the four new tools need
  four `AddTool` registrations (§5.5) or this test fails.

`catalogue.gen.go` is regenerated via `go generate` and committed; the CI
drift guard (`apps/api/mcp/catalogue.go:10-12`) fails on a `go generate` diff
(CG-12).

#### 4.4.2 Validator wires INTO the existing handlers (CG-5)

The comment-trail gate is invoked **inside** the existing handlers — NOT a new
MCP tool:
- `set_state` → inside `apps/api/mcp/handler_set_state.go` (today reads
  "Layer-1 BLOCK conditions … ship in P02 — NOT enforced here" at `:32-34`).
- `claim` → inside the existing claim handler (T1).
- `close` → inside the existing close handler (T9).

Each calls the generated `catalogue.gen.go` validator with the candidate
transition + the §4.2 RPC result.

#### 4.4.3 The gate reads PRE-EXISTING comments (CG-6)

`set_state`'s own `intent_comment` is appended **best-effort, post-commit,
non-atomically** (`handler_set_state.go:43-44`, `:85-88`) and `SetStateColumns`
never reads `workitems.comments` (`workitems.go:1782-1790`). Therefore:

- (a) the comment-trail gate evaluates comments **committed BEFORE** the
  `set_state` call — the required trail must PRE-EXIST.
- (b) `set_state`'s own `intent_comment` does NOT satisfy its own gate.
- (c) the agent must do **two calls**: append the `(kind=qa, status=success)`
  comment first, THEN call `set_state(qa_state=passed)`.
- (d) **in-call order** (pinned; F-2 asserts it):
  1. identity / arg validation (`intent_comment` validation as today)
  2. **Layer-1 comment-trail gate** (reads the pre-existing trail via §4.2)
  3. column-value invariants inside `SetStateColumns` (I-1 auto-reset; then
     the rejection checks)
  4. state write (commit)
  5. post-commit `intent_comment` append (best-effort)

#### 4.4.4 Per-RPC invariant split + two-error ordering (RP02-2, CG-6)

The PRD §6.2 column-value invariants already ship in P01, **split across two
RPCs** (`workitems.go`):
- **`set_state` (`SetStateColumns`)** carries the structural
  `impl_done_requires_claim` (`:1808-1811`) + I-2 (`:1813-1816`) + I-5
  (`:1818-1832`) + I-4 (`:1842-1844`), with I-1 as an **auto-reset** (NOT a
  rejection, `:1803-1806`). All rejections fire as
  `errs.FailedPrecondition` + `Meta["invariant"]` → `PRECONDITION_NOT_MET`.
- **`claim` (`Claim`)** carries I-3 (post-failure rework reset).

P02's comment-trail gates reject with `PIPELINE_PRECONDITION_NOT_MET` (a
distinct envelope kind, §4.6). The two MUST NOT double-fire. **Per-RPC order:**

- **`set_state`:** Layer-1 comment-trail gate (`PIPELINE_PRECONDITION_NOT_MET`)
  evaluated in step 2 (§4.4.3.d) **BEFORE** entering `SetStateColumns`; the
  column-value invariants (`PRECONDITION_NOT_MET`) are evaluated inside
  `SetStateColumns` in step 3. So the **pipeline gate is checked first**; if it
  rejects, the column-value path is never reached. (This inverts the original
  plan §2.3 prose "I-1..I-5 first / pipeline gates second" — written before
  CG-6 pinned that the gate must read pre-existing comments OUTSIDE the
  `SetStateColumns` round-trip. The **load-bearing invariant** — "exactly one
  error wins per bad transition" — holds either way; **gate-first is the
  RESOLVED ordering (OQ-A, user 2026-06-16)** because the gate runs in the MCP
  handler before the RPC, so the pipeline gate's `PIPELINE_PRECONDITION_NOT_MET`
  wins. The plan §2.3 was reconciled to gate-first on 2026-06-16. See §4.4.5.)
- **`claim`:** T1's status/claimed gate (`PIPELINE_PRECONDITION_NOT_MET`)
  runs in the claim handler; I-3 is internal to `Claim` and orthogonal.

F-2 asserts a single, correct rejection per bad transition.

#### 4.4.5 Ordering rationale (RESOLVED 2026-06-16 = gate-first — OQ-A)

The original plan §2.3 sidebar said "column-value invariants (I-1..I-5) run
first, comment-trail gates second." CG-6 establishes that the comment-trail
gate **must** read comments committed before the call, which the validator
does in the MCP handler **before** `SetStateColumns` runs. Two consistent
orderings exist:

1. **Gate-first (RESOLVED 2026-06-16 — this is the binding ordering):**
   pipeline comment-trail gate in the handler → `SetStateColumns` invariants.
   Simpler (the gate never needs the RPC's column result); the column
   invariants are the structural backstop. When a gate and a column invariant
   would both fail, the **pipeline gate's `PIPELINE_PRECONDITION_NOT_MET`
   wins** — the gate is checked first and the column-value path is never
   reached.
2. **Invariants-first (rejected):** call `SetStateColumns` (column invariants
   fire) → only if it would succeed, run the pipeline gate. Requires a dry-run
   / two-phase split of `SetStateColumns` (it currently writes in one
   round-trip).

Both satisfy "one error wins." **Gate-first is the resolved disposition
(user, 2026-06-16):** the lower-risk implementation that matches CG-6's
pre-existing-comment requirement without re-architecting `SetStateColumns`.
**The plan §2.3 (+ §5.5 C3 row + §7 RP02-2) "invariants-first" prose was
reconciled to gate-first on 2026-06-16** to agree with this spec.

### 4.5 T8 (`pipeline_state.running->needs_human`) — disjunctive gate

T8's precondition is disjunctive and counts history (3× needs_rework, 3×
qa_failed) or external signals (claim/worktree conflict, explicit
`flag_human`). The `block_conditions` `required_state` cannot express counts,
so T8 ships as a `block_conditions` object with `precondition_human` +
`rejection_reason` for Layer-3 rendering, and the **count predicates are
evaluated in handler code** (not the generated validator) via a count query
over `workitems.comments` / the cascade audit. The catalogue entry marks T8
`evaluator: "handler"` (a new `block_conditions` field, additive to §7.5.1)
so the codegen emits a hand-written-evaluator stub rather than a pure-state
matcher. The explicit `flag_human` path (the existing `mcp.flag_human` call)
always satisfies T8.

> T8 is the one transition where the typed `required_state` matcher is
> insufficient. The `evaluator:"handler"` escape hatch keeps T8 in the single
> catalogue source (so Layer-3 still renders it) while routing its evaluation
> to handler code. The exact count query lives in the handler; the catalogue
> records only the human form + the rejection_reason.

### 4.6 Error envelope: `PIPELINE_PRECONDITION_NOT_MET` (R-P02-5, CG-7)

The rejection is wire-legible via the existing `apps/api/mcp/errmap.go` path:
a handler returns `*sdkjsonrpc.Error{Code:-32000, Message:<human>, Data:<§7 envelope>}`,
forwarded verbatim as a JSON-RPC **error** (not an `isError` tool-result).

CG-7 wiring:
- Add `envelopeKindPipelinePreconditionNotMet` to `apps/api/mcp/errenvelope.go`'s
  const block, **distinct** from `envelopeKindPreconditionNotMet`
  (`errenvelope.go:70`).
- In `errmap.go`, within the `errs.FailedPrecondition` arm (`:237-297`), add a
  discriminator branch routing `Meta["kind"]="PIPELINE_PRECONDITION_NOT_MET"`
  onto the new constant **BEFORE** the terminal `envelopeKindPreconditionNotMet`
  return at `:297` (analogous to the `CYCLE_DETECTED` branch at `:241-260`).
- The handler-side gate sets the error `Meta` with:
  `kind="PIPELINE_PRECONDITION_NOT_MET"`, `error_code` (= the catalogue
  `error_code`), `transition` (the catalogue `transition` label),
  `rejection_reason` (the catalogue `rejection_reason`), and `missing` (a
  short human note naming the unmet precondition).
- Envelope `data` carries `kind`, `error_code`, `transition`,
  `rejection_reason`, `details.missing`. `rejection_reason` flows into
  `mcp.tool_calls.rejection_reason` via the existing `mapError` path (R-P02-5).

Without the dedicated discriminator the gate's rejection silently collapses
into `PRECONDITION_NOT_MET` (CG-7).

### 4.7 `meta_catalogue` — MCP Resource (C2)

Ships as an MCP **Resource** via `AddResource` (go-sdk v1.6.0 `server.go:519`),
NOT `AddTool` — so it surfaces under `resources/list`, never `tools/list`,
keeping the agent-facing tool count at 27 (CG-12).

- URI: `unblock://catalogue`
- MIME: `application/json`
- Body: the live `catalogue.json` bytes (the same source `catalogue.gen.go`
  embeds), served over the Streamable HTTP transport (`POST /mcp` — the SPEC
  "same SSE channel" wording was corrected to Streamable HTTP, C2).
- Purpose: the P04 build-time renderer reads it to verify the checked-in copy
  (AR-4 third corner; inert until P04). A-6 makes the Go-codegen ↔ live-Resource
  pair load-bearing now.

### 4.8 `verify_can_transition` — custom JSON-RPC method (C2)

Ships as a **custom JSON-RPC method** intercepted via `AddReceivingMiddleware`
(go-sdk v1.6.0 `server.go:1352`), NOT `AddTool` — so it is never in
`tools/list` (CG-12). Method name: **`unblock/verifyCanTransition`**.

```jsonc
// request params
{
  "item_id":    "01J…",        // ULID
  "tool":       "set_state",   // candidate tool
  "transition": "qa_state.pending->passed"  // candidate transition id from the catalogue
}
// result
{
  "allowed":          true,
  "error_code":       null,    // or "PIPELINE_PRECONDITION_NOT_MET"
  "rejection_reason": null,    // or the catalogue rejection_reason
  "missing":          null     // or a short human note
}
```

The middleware (a) matches the method name, (b) manually decodes the params
(bypassing `AddTool`'s typed plumbing), (c) loads the candidate transition from
the generated validator, (d) reads the §4.2 RPC, (e) runs the **same**
generated validator the `set_state`/`claim`/`close` gates use, (f) returns the
result. Read-only: it never mutates state. This is the machinery P04's
`verify-state` hook (Layer 2) calls. F-4 asserts `verify_can_transition`
agrees with `set_state`'s live gate on the same candidate.

---

## 5. Memory service + four tools (Track B)

### 5.1 Private RPCs

`apps/api/memory/memory.go` (all `//encore:api private`):

```go
type RememberRequest struct {
    Scope       string   `json:"scope"`        // 'org'|'project'|'user'
    OrgID       string   `json:"org_id"`       // exactly one scope target set
    ProjectID   string   `json:"project_id"`
    UserID      string   `json:"user_id"`
    AuthorID    string   `json:"author_id"`    // one of author_id / author_agent
    AuthorAgent string   `json:"author_agent"`
    Key         string   `json:"key"`
    Value       string   `json:"value"`        // plaintext; sanitised then encrypted
    Tags        []string `json:"tags"`
    Refs        []EntryRef `json:"refs"`        // optional cross-references (§9.4.8 entry_refs)
    ExpiresAt   string   `json:"expires_at"`   // RFC3339, optional (§5.1.1)
    CallerOrgID string   `json:"caller_org_id"`
}
type RememberResponse struct {
    EntryID        string `json:"entry_id"`
    Sanitised      bool   `json:"sanitised"`        // true if any pattern hit
    RedactionCount int    `json:"redaction_count"`
}

type RecallRequest struct {
    Scope       string   `json:"scope"`
    OrgID       string   `json:"org_id"`
    ProjectID   string   `json:"project_id"`
    UserID      string   `json:"user_id"`
    Key         string   `json:"key"`          // exact key, optional
    Query       string   `json:"query"`        // ts_doc full-text, optional
    Tags        []string `json:"tags"`         // tag filter, optional
    Limit       int      `json:"limit"`
    CallerOrgID string   `json:"caller_org_id"`
}
type RecallResponse struct { Entries []MemoryEntry `json:"entries"` }

type ListRequest struct {
    Scope       string `json:"scope"`
    OrgID       string `json:"org_id"`
    ProjectID   string `json:"project_id"`
    UserID      string `json:"user_id"`
    Limit       int    `json:"limit"`
    Cursor      string `json:"cursor"`         // keyset pagination on (created_at, id)
    CallerOrgID string `json:"caller_org_id"`
}
type ListResponse struct {
    Entries    []MemoryEntrySummary `json:"entries"`  // no decrypted value (cheap dashboard read)
    NextCursor string               `json:"next_cursor"`
}

type ForgetRequest struct {
    EntryID     string `json:"entry_id"`
    CallerOrgID string `json:"caller_org_id"`
}
type ForgetResponse struct { Forgotten bool `json:"forgotten"` }
```

`MemoryEntry` carries the **decrypted** `value` (decrypt-on-read, §5.3);
`MemoryEntrySummary` carries `key`, `tags`, `scope`, `created_at`, `value_size`
only (no decrypt — `memories` is the cheap read).

#### 5.1.1 `expires_at` write surface (R-P02-13, B-6 → OQ-B RESOLVED 2026-06-16)

`remember` accepts an **optional** `expires_at` (RFC3339). Disposition
(**OQ-B RESOLVED 2026-06-16 = wire the write surface**): the agent may set a
TTL, coupled with the read-time filter (§5.2). No DDL change (column already
in `0090`). No expiry sweeper cron (matches the `mcp.api_keys.expires_at`
no-sweeper precedent, `0070`).

### 5.2 Read-time filtering (C4 + R-P02-13, combined predicate)

`recall` and `memories` filter both in one predicate:

```sql
WHERE deleted_at IS NULL
  AND (expires_at IS NULL OR expires_at > now())
```

`forget` (soft-delete) sets `deleted_at = now()` (CG-13: no hard delete; the
audit row survives). A `forget`→re-`remember` of the same `(scope,key)`
succeeds because the unique indexes are partial-on-not-deleted (§3.1(b)).

### 5.3 Encrypt-at-rest (R-P02-8)

Pinned to the explicit aes256 form (SPEC §9.4.10) for parity:

```sql
-- write
pgp_sym_encrypt($plaintext, $dek, 'cipher-algo=aes256, compress-algo=2')
-- read (after authorisation, service layer)
pgp_sym_decrypt(value_enc, $dek)::text
```

`$dek` = `secrets.MemoryDEK` (the `memory` package's own `var secrets struct`,
§3.4). DEK rotation (`MEMORY_DEK_NEXT`, AR-7) is **inert in P02** (R-P02-8).
`recall` decrypts per row; no PRD latency gate (Q6) — spec-level NFR only.

### 5.4 Always-on secret sanitiser (NFR-7, AR-10, AR-14)

#### 5.4.1 Ordering invariant (service-code, DDL enforces nothing — R-P02-9)

Per `remember`: **sanitise → build `ts_doc` (tokenise the sanitised
plaintext) → encrypt(`value_enc`)**. The DDL enforces nothing here (the
`0090` comments document the intent); B-1/B-2 implement the order. A missed
pattern must never reach the unencrypted GIN-indexed `ts_doc` (R2). Posture:
**redact-not-reject** — a matched pattern is replaced with a `[REDACTED:<category>]`
marker; the call still succeeds; a `sanitiser_events` row is written (best-effort
detection per NFR-7/PRD-R6).

#### 5.4.2 v1.0 pattern set (authored here — no v1.0 set existed; R-P02-9)

| pattern_id | category | regex (Go RE2) | notes |
|---|---|---|---|
| `aws_access_key_id` | credential | `AKIA[0-9A-Z]{16}` | AWS access key id |
| `github_pat` | credential | `gh[posru]_[A-Za-z0-9]{36,}` | GitHub PAT prefixes (ghp/gho/ghu/ghs/ghr) |
| `pem_block` | credential | `-----BEGIN [A-Z ]+PRIVATE KEY-----` | PEM private-key marker |
| `jwt` | credential | `eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+` | JWT |
| `bearer_header` | credential | `(?i)bearer\s+[A-Za-z0-9._-]{16,}` | bearer auth header |
| `generic_secret_assign` | credential | `(?i)(secret\|token\|password\|api[_-]?key)\s*[:=]\s*\S{6,}` | generic secret assignment |
| `email` | pii | `[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}` | email (also the providers-payload redactor, §6.6) |

> This is the **v1.0 baseline**, not a frozen contract (best-effort per
> NFR-7). The list is open to extension; the registry is keyed by
> `pattern_id` so `sanitiser_events` rows stay stable across additions.
> **OQ-C RESOLVED 2026-06-16 = keep the 7-pattern baseline** (no SaaS
> additions at v1.0; the registry stays extensible post-v1.0).

The same registry feeds three callers: the memory write sanitiser (B-1/B-2),
the AR-14 periodic re-scan job (§6.7), and the providers payload sanitiser
(§6.6, the `email` + credential subset on insert).

#### 5.4.3 Audit (`memory.sanitiser_events`, §3.1(a))

One row per `(pattern_id, field)` hit, collapsing N matches of the same
pattern in the same value into `redaction_count`. `source='memory_write'` for
`remember`, `'memory_rescan'` for the cron, `'providers_payload'` for §6.6.

### 5.5 MCP tools 24–27 (CG-1, CG-9)

Four `AddTool` registrations in `apps/api/mcp/` (facades over `memory.*` RPCs),
each with `input_schema` / `output_schema` in `catalogue.json` (so
`TestCatalogueNamesMatchToolRegistrars` passes, CG-1):

| # | Tool | Maps to | Notes |
|---|---|---|---|
| 24 | `remember` | `memory.Remember` | 8 KB `value_size` cap (DDL CHECK); per-(scope,key) uniqueness; sanitiser; encrypt; `sanitiser_events` on hit |
| 25 | `recall` | `memory.Recall` | scope+key / ts_doc / tag filters; decrypt-on-read; filters `deleted_at IS NULL` + `expires_at` (§5.2) |
| 26 | `memories` | `memory.List` | keyset pagination; cheap (no decrypt); filters as §5.2; powers `prime.memory_hints` (CG-9) |
| 27 | `forget` | `memory.Forget` | soft-delete (`deleted_at=now()`); audit-trail preserved |

#### 5.5.1 `prime.memory_hints` (CG-9 — POPULATE the existing shape)

`apps/api/mcp/handler_prime.go` already declares `primeMemoryHint{Source, Body}`
(`:114-117`), the response field `MemoryHints []primeMemoryHint` (`:58`), and
the empty literal `[]primeMemoryHint{}` (`:232`). B-3 **populates** this
existing shape — it does NOT reinvent the schema. Projection: a `memory.entries`
row collapses to `{source: "<scope>:<key>", body: <decrypted value, truncated>}`
via a `memory.List`-style cheap read scoped to the caller's org/project. If
`primeMemoryHint` needs more fields, widening it is backward-additive
(acceptable pre-prod) — but the default is the existing two-field shape.

### 5.6 RBAC for `memory` (B-5 EXTENDS — CG-4)

`memory.entries` is ALREADY in the `rbactest` matrix as dual `KindOrgScoped` +
`KindAuthorizeOnly` (`matrix.go:218-219`) with a `scope='org'` seed row and an
existing `selectScopedOrgIDs` case (`rbactest_test.go:632`); the in-code P02
note at `matrix.go:182-188` spells out the gap. B-5 **EXTENDS** this:

- add project- and user-scope seed rows (`org_id` NULL, `project_id`/`user_id`
  set per `entries_scope_target_chk`, `0090:36-40`).
- supply a **non-`rbac.For`** isolation predicate for project/user reads —
  `rbac.For` emits `memory.entries.org_id = $1` (`rbac.go:314`) which passes
  **vacuously** for NULL-org_id rows and proves nothing. The project/user
  isolation predicate keys on `project_id = $1` / `user_id = $1` respectively,
  scoped through the caller's org membership.

**`tsvector` projection constraint (round-18, bead `unblock-8xb.8`).** Any
`rbac.For[T]` read of `memory.entries` — whether the production memory-service
org-scope read (B-1/B-5) or the `rbactest` matrix read — MUST pass an explicit
`.Columns(...)` projection that EXCLUDES the `ts_doc tsvector NOT NULL` column
(`0090_memory.up.sql:29`). This is the general rule pinned in
`01-spec-backend-mvp.md` §3.4 / §10.1 (round-17/18): `ts_doc` is delivered by
the Encore pgx v5.7.6 runtime in BINARY format (OID 3614) with NO registered
scan-plan into any Go scalar (`[]byte` included), so `SELECT *` over
`memory.entries` fails `rows.Scan` on any populated result set on the Encore
platform (the local emulator delivers text format and masks the defect — exactly
the latent failure mode that blocked the P01 `workitems.items` reads). `ts_doc`
is query-side only (the `recall` / `prime.memory_hints` `@@`/GIN predicate,
§5.4.1/§5.5.1); no read path scans its value into Go. The `rbactest`
`memory.entries` read at `rbactest_test.go:633` is corrected for this on the
`unblock-8xb.8` branch (its phantom `memoryEntriesRow.TSDoc []byte` field is
removed); the production memory-service reads introduced by B-1/B-5 MUST follow
the same pattern from the outset, or they reintroduce this exact bug in
production. The same rule applies to any future `rbac.For` read over
`workitems.comments` (its `fts tsvector`).

It is NOT net-new scaffolding (CG-4).

---

## 6. Providers service (Tracks C + D)

### 6.1 Webhook ingestion — `POST /webhooks/github` (FR-12, CG / plan §2.6)

The raw public endpoint is declared **inside the `providers` service**
(`//encore:api public raw path=/webhooks/github`), mirroring `/mcp` inside the
`mcp` service — **not** an `apps/api/public/` package (no such package exists).

#### 6.1.1 Handler sequence (ack-fast, R-P02-1, R-P02-11)

1. Read the raw body bytes (capped at the §6.1.2 oversize limit) WITHOUT
   parsing.
2. **HMAC verify** `X-Hub-Signature-256` = `sha256=` + hex(HMAC-SHA256(
   `secrets.GitHubAppWebhookSecret`, raw_body)) via `hmac.New(sha256.New, …)`
   + `hmac.Equal` (constant-time) over the **unparsed** bytes. The secret is
   the **app-level** Encore secret (C1) — there is no per-install secret to
   key on (the column is dropped, §3.1(d)), and verification must precede
   parsing (we cannot learn `installation.id` until after parse).
3. Parse the body; read `installation.id`, `X-GitHub-Delivery`,
   `X-GitHub-Event`, body `action`.
4. Resolve `installation_id` → `providers.installations` row. Unknown → 404
   (§6.1.2), no `events` insert.
5. **Sanitise the payload** (§6.6) before insert.
6. **Dedup insert** into `providers.events` with the
   `(provider, delivery_id)` UNIQUE constraint (AR-12). `ON CONFLICT
   (provider, delivery_id) DO NOTHING`. A conflict (recognised duplicate) →
   return 200 (GitHub stops retrying), no publish.
7. **Publish** to the `provider.events` Pub/Sub topic (§6.4) with a
   publisher-generated ULID `EventID` (AR-11).
8. Return **200**. Normalisation happens async in the subscriber (§6.4).

`event_type` stored = `<event>.<action>` (e.g. `issues.opened`), matching the
`0060` comment.

#### 6.1.2 Failure-status contract (R-P02-4b)

| Class | Status | `events` insert? | normalise? |
|---|---|---|---|
| bad/absent HMAC signature | **401** (4xx-final) | no | no |
| unknown `installation.id` | **404** (4xx-final) | no | no |
| malformed / unparseable JSON | **400** (4xx-final) | no | no |
| oversized payload (> read cap) | **413** (4xx-final) | no | no |
| recognised duplicate `X-GitHub-Delivery` | **200** | no (ON CONFLICT) | no |
| transient our-side (DB down on ack-insert) | **503** (5xx-retryable) | — | — |

All 4xx classes are non-retryable; the single 5xx is for transient our-side
failure so a redelivery can succeed. Oversize read cap: pin to **2 MB** (well
under GitHub's 25 MB delivery cap; `issues`/`pull_request` payloads are far
smaller). F-1 covers happy + replay + bad-signature.

### 6.2 `LinkRepo` RPC and the App credential model (C2/CG-2 — Q4/C1)

```go
type LinkRepoRequest struct {
    OrgID            string `json:"org_id"`
    ProjectID        string `json:"project_id"`
    Provider         string `json:"provider"`          // 'github' at v1.0
    ProviderAccount  string `json:"provider_account"`  // e.g. 'websublime'
    ProviderRepo     string `json:"provider_repo"`     // nullable for org-level
    InstallationID   string `json:"installation_id"`   // GitHub App installation id
    SyncEnabled      bool   `json:"sync_enabled"`      // opt-in bidirectional sync
    CallerOrgID      string `json:"caller_org_id"`
    CallerUserID     string `json:"caller_user_id"`
}
type LinkRepoResponse struct { InstallationRowID string `json:"installation_row_id"` }
```

Stores `installation_id_enc` (encrypted via the shared DEK, §5.3) **only** —
**NO per-install webhook secret** (the `webhook_secret_enc` column is dropped,
§3.1(d), CG-2). HMAC verifies against `secrets.GitHubAppWebhookSecret`
(app-level). Outbound writes (D-1) mint installation access tokens via
`ghinstallation` (App ID + PEM → `/app/installations/{id}/access_tokens`,
1-hour lifetime).

### 6.3 Normalisation field map (R-P02-2, REST/go-github)

GitHub `issues.*` / `pull_request.*` event → canonical `workitems.items` via
the **existing** `workitems.Create` / `workitems.Update` RPCs (no direct
cross-schema writes — Law 6 / RBAC). Mapping recorded in `providers.mappings`
(`provider_kind ∈ {issue, pull_request}`, external-id uniqueness).

#### 6.3.1 Inbound map (GitHub → canonical)

| GitHub field | Canonical | Notes |
|---|---|---|
| `issue.number` | `providers.mappings.provider_id` + `provider_url` | string for portability |
| `issue.title` | `items.title` | |
| `issue.body` | `items.body` | sanitised (§6.6) |
| `issue.state` (`open`/`closed`) | `items.status` | closed→`Done`; open→`Backlog`/`Ready` per deps (§6.3.3) |
| `issue.labels[].name` | `items.labels` (name match against `workitems.labels`) | unknown label → skip (do not auto-create at v1.0) |
| `issue.milestone` | `items.milestone_id` | resolve by title; unmatched → unmapped (§6.3.3) |
| `issue.assignees[]` / `assignee` | (unmapped at v1.0) | GitHub login ↔ `claimed_by_id` ULID needs identity resolution (§6.3.3) |
| `issue.locked` | (unmapped) | no canonical column |

#### 6.3.2 Outbound map (canonical → GitHub, D-1)

| Canonical | GitHub REST call | Notes |
|---|---|---|
| `items.title` | `Issues.Edit{Title}` | |
| `items.body` | `Issues.Edit{Body}` | |
| `items.status=Done` | `Issues.Edit{State:"closed"}` | |
| `items.status≠Done` (was Done) | `Issues.Edit{State:"open"}` | |
| `items.labels` | `Issues.Edit{Labels}` | full-set replace (name match) |

The three pipeline columns (`impl/review/qa_state`), `pipeline_state`, and the
dependency graph are **unblock-only** — they are NEVER written to GitHub and
NEVER inferred from GitHub state (R-P02-2).

#### 6.3.3 Unmapped-field degradation (R-P02-2, spec decision)

- **dependency graph:** one-directional — `://unblock` owns it; GitHub has no
  source. Never synced.
- **`state_reason`** (`completed`/`not_planned`/`reopened`): no canonical
  column → dropped (audit-only via the raw `payload`).
- **pipeline columns:** unblock-only; NOT inferred from `issue.state`.
- **assignee identity:** unmapped at v1.0 — `claimed_by_id` is never set from a
  GitHub login. (A future identity-resolution step maps GitHub login →
  `auth.users` ULID; out of P02 scope.)
- **milestone:** resolve by title against `workitems.milestones`; unmatched →
  leave `milestone_id` NULL (do not auto-create).
- **labels:** name-match only; unknown labels are skipped (no auto-create).

### 6.4 Async Pub/Sub path (R-P02-11, AR-11, CG-11)

```go
// apps/api/providers/events.go
type ProviderEvent struct {
    EventID        string `json:"event_id"`        // publisher-generated ULID (AR-11 idempotency key)
    InstallationID string `json:"installation_id"`
    Provider       string `json:"provider"`
    DeliveryID     string `json:"delivery_id"`     // X-GitHub-Delivery (AR-12, distinct from EventID)
    EventType      string `json:"event_type"`      // e.g. 'issues.opened'
    EventRowID     string `json:"event_row_id"`    // providers.events.id (the persisted audit row)
    OrgID          string `json:"org_id"`
    TraceID        string `json:"trace_id"`        // §10.5 propagation (NFR-12)
}
var ProviderEventsTopic = pubsub.NewTopic[*ProviderEvent]("provider-events",
    pubsub.TopicConfig{DeliveryGuarantee: pubsub.AtLeastOnce})
```

The subscriber (mirroring `deps/cascade.go` + `deps/cascade_subscriber.go`)
reads `providers.events` by `EventRowID`, normalises (§6.3) via `workitems`
RPCs, and stamps `providers.events.processed_at`. **At-least-once replay
dedup** uses the publisher-generated ULID `EventID` with `ON CONFLICT DO
NOTHING` — **distinct** from the handler-side `(provider, delivery_id)` AR-12
dedup. The executable template is `insertCascadeEventRow`
(`apps/api/deps/cascade_subscriber.go:742`, SQL `:769-779`
`ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING`) + the
publisher-ULID `EventID` field on `CascadeRequested` (`deps/cascade.go:53-57`)
— **NOT** the topic-declaration doc comment at `cascade.go:158-162` (CG-11).

Dedup landing: the subscriber's idempotency key is `(EventID, EventRowID)`;
since `providers.events` already carries the AR-12 constraint, replay safety
at the subscriber is achieved by a guarded `UPDATE … WHERE processed_at IS
NULL` on `providers.events` (the row is the natural idempotency record) rather
than a second audit table. The implementer MUST confirm the guarded-update
form is replay-safe against double-normalisation (F-1's replay case asserts
no double-create).

### 6.5 Bidirectional sync + loop prevention (D-1, R-P02-3, R1)

Opt-in per installation (`LinkRepo.SyncEnabled`). A `://unblock` mutation on a
mapped item propagates back via go-github `Issues.Edit` (§6.3.2). **Loop
suppression combines two suppressors (R-P02-3):**

1. **Actor allowlist (fast path):** on inbound webhook, if `sender.login` ==
   the App's bot account (`<app-name>[bot]`), the normaliser no-ops — this is
   our own echo write.
2. **Content-idempotent normalise (structural backstop):** the normaliser
   diffs the candidate against current canonical state; a no-op diff suppresses
   the re-write regardless of actor.

Additionally, `providers.mappings.last_synced_at` is stamped `= now()` on each
outbound write (echo-window context for the reconciler). Rate safety: the
writer honours go-github's `*RateLimitError` / `*AbuseRateLimitError{RetryAfter}`
+ `x-ratelimit-remaining`/`-reset`. F-5 asserts the echo webhook does NOT
re-trigger a normalise→re-write storm.

### 6.6 Payload sanitiser + 90-day digest cron (§9.4.5 retention)

#### 6.6.1 On-insert sanitiser (first layer)

Before the `providers.events` insert (§6.1.1 step 5), redact emails + the
credential subset (§5.4.2 registry) from `payload`. A hit writes a
`sanitiser_events` row with `source='providers_payload'`, `org_id` = the
installation's org, `entry_id` NULL.

#### 6.6.2 90-day digest cron (second layer)

Daily Encore cron (`cron.NewJob`, parameter-free idempotent endpoint). For
each `providers.events` row with `received_at < now() - interval '90 days'`
and a non-digested `payload`, replace `payload` with the metadata-only digest:

```jsonc
{ "event_type": "<string>", "actor_login": "<sha256 hash>",
  "repo": "<sha256 hash>", "delivery_id": "<string>", "digest_at": "<ts>" }
```

A test asserts a payload older than 90 days is digested (§9.4.5 mandate).

### 6.7 Reconciliation cron (Law 3, D-2)

Scheduled Encore cron detecting drift (`providers.mappings.drift_detected_at`)
when webhooks were missed or GitHub was offline. For each opt-in installation,
list GitHub issues changed since the mapping's `last_synced_at` (go-github,
rate-limit-honouring), diff against canonical state, and reconcile via the
normaliser (content-idempotent, §6.5). Sets/clears `drift_detected_at`.
**Cadence:** the smallest cron the free tier permits = **hourly** (R-P02-12).
F-6 simulates a missed webhook → drift → cron repair, and asserts MCP is still
served while the provider is offline (NFR-3 / Law 3).

> Cron jobs do **not** run locally / in preview (R-P02-11). The reconciler,
> digest, and re-scan jobs are tested via their underlying parameter-free
> endpoints directly under `encore test` (call the endpoint, not the schedule).

### 6.8 RBAC for `providers` (D-3 — CG-3, per-table classification)

Only `providers.installations` has `org_id` (`0060:10`); `providers.events`
(`0060:28-43`) and `providers.mappings` (`0060:70-87`) carry only an
`installation_id` FK — **NO `org_id`**. So D-3 CANNOT use `rbac.For` on
events/mappings (`rbac.For` emits `<table>.org_id = $1`, which Postgres rejects
on org_id-less tables — `matrix.go:136-139`). Add three `org.go` Resource
constants — `resourceProvidersInstallations` / `resourceProvidersEvents` /
`resourceProvidersMappings`, all in `resourceAllowed`, **NONE** in
`agentReadWriteResources` (providers tables are not agent-writable via MCP) —
and classify:

- `installations` → `rbac.For` (`KindOrgScoped`, carries `org_id`).
- `events` + `mappings` → **Authorize-only** (`KindAuthorizeOnly`), scoped via
  the parent installation's `org_id` FK join (mirroring
  `workitems.comments` → `items`).

Add the matching `rbactest` matrix rows + a `case "providers.installations":`
arm in `selectScopedOrgIDs` (CG-3).

---

## 7. Pub/Sub, Cron, and trace propagation summary

| Mechanism | Name | Trigger | Idempotency |
|---|---|---|---|
| Pub/Sub topic | `provider-events` | webhook handler publish (§6.4) | publisher ULID `EventID` (AR-11) + AR-12 `(provider, delivery_id)` |
| Cron (hourly) | `providers-reconcile` | drift repair (§6.7) | content-idempotent normalise |
| Cron (daily) | `providers-payload-digest` | 90-day retention (§6.6.2) | `received_at` predicate + digest marker |
| Cron (hourly) | `memory-sanitiser-rescan` | AR-14 re-scan (§5.4) | re-scan is read-then-conditional-write |

**No `mcp-warmer` cron** ships (C5/Q8 — free-tier hourly cron can't keep a
scale-to-zero service warm; cold-start is a documented outlier; no external
pinger at v1.0). **Trace propagation (NFR-12):** the Encore `trace_id` (§10.5)
propagates from `POST /webhooks/github` through the async subscriber (carried
as `ProviderEvent.TraceID`) into the downstream `workitems.Create/Update` RPCs
and into the cron jobs; ≥1 test asserts a single webhook delivery and its async
normalisation share one trace tree.

---

## 8. Infra (Track E, Olive) — non-gating per Q5

- **E-1:** Encore Cloud staging deploy (first real deploy).
- **E-2:** secrets — `GitHubAppID` + `GitHubAppPrivateKey` (PEM) +
  `GitHubAppWebhookSecret` (app-level, C1) + `MemoryDEK` (§3.4); each with the
  boot-time fail-fast guard (CG-8) + `SECRETS.md` + `secrets.nonprod.cue`
  placeholders in the same commit.
- **E-3:** AR-13 free-tier ceiling report (100k req/day, 100k Pub/Sub msgs/day,
  1 GB DB, cron hourly-min, 2 cloud envs, no preview) + measured cold-start
  number on staging. Pooled DB bindings already compliant (`db/db.go`).
- **E-4:** cron schedules wired (reconcile, payload-digest, sanitiser-rescan).
  **No warmer cron** (C5).
- **A-6 (Olive):** catalogue-drift CI made load-bearing for the Go-codegen ↔
  live-`meta_catalogue`-Resource pair (Rust corner inert until P04).

Per Q5 the functional acceptance bar (§9.1) is local-emulator + CI green;
staging + capacity are required *work*, not a pass/fail release gate.

---

## 9. Acceptance criteria (the spec is satisfied when…)

### 9.1 Functional (PRD §8 + SPEC §11 exit criterion)

- [ ] `LinkRepo` creates an installation (App path, app-level webhook secret).
- [ ] A synthetic HMAC-signed `issues.opened` webhook to `POST /webhooks/github`
      is signature-verified, deduplicated, and normalised into a canonical
      `workitems.items` row mapped via `providers.mappings`; a duplicate
      `X-GitHub-Delivery` returns 200 and does not double-create (F-1).
- [ ] A `://unblock` mutation on a mapped item propagates to the GitHub Issue
      without a sync loop (F-5).
- [ ] The reconciliation cron detects + repairs a missed-webhook drift (F-6).
- [ ] The four memory tools work end-to-end (`remember` with sanitiser +
      encrypt, `recall`, `memories`, `forget`); MCP tool surface = **27** (F-3).
- [ ] `done` (via `close` / `set_state(qa_state=passed)`) without the
      `kind=review,status=success` then `kind=qa,status=success` trail is
      rejected with `PIPELINE_PRECONDITION_NOT_MET` (F-2).
- [ ] `meta_catalogue` Resource serves the live catalogue;
      `verify_can_transition` agrees with `set_state`'s gate (F-4).

### 9.2 Non-functional + architectural

- [ ] **NFR-2:** RBAC suite extended to `providers` (per-table classification,
      §6.8) + `memory` (project/user scope isolation, §5.6); zero cross-tenant
      leaks. Release-blocking.
- [ ] **NFR-3 (Law 3):** provider outage / missed webhook does not stop MCP;
      reconciler repairs drift (F-6).
- [ ] **NFR-6 (Layer 1):** every PRD §6.7 transition's precondition enforced
      (per-transition matrix, §9.3); Layers 2/3 close at P04.
- [ ] **NFR-7:** sanitiser always-on; credential sanitised before encrypt +
      before `ts_doc`; `sanitiser_events` row written.
- [ ] **NFR-10:** Greta's Go gate green (`encore test ./...`, `go vet`,
      `go fmt`, `encore check`); JSON-tag lint zero on the new structs.
- [ ] **NFR-12:** JSON-Lines logs on STDERR; envelopes on STDOUT; DEK / webhook
      secret / App PEM / installation token never logged; trace tree shared
      webhook→subscriber (§7).
- [ ] **Additive migrations only:** all P02 migrations new, > `0140`, up-only;
      forward-only replay check green.
- [ ] **No new persistent store** (FR-1); **catalogue single-source** (Go ↔
      live Resource agree, A-6); **webhook dedup structural** (AR-12); **no
      `crates/` code** (NFR-9).

### 9.3 Per-transition test matrix (NFR-6, RP02-3)

One test per PRD §6.7 row (T1–T9, §4.1), each asserting (a) the gate ACCEPTS
when the precondition holds, (b) the gate REJECTS with exactly one
`PIPELINE_PRECONDITION_NOT_MET` (no double-fire with `PRECONDITION_NOT_MET`,
§4.4.4) when it does not, (c) `verify_can_transition` returns the same verdict
as the live `set_state`/`claim`/`close` gate. The matrix is the mechanical
proof of "every §6.7 transition enforced."

---

## 10. Implementation tasks (track → supervisor)

The plan §4 tracks map 1:1 to bd beads emitted by `/tasks`. All Go → **Greta**
(go-supervisor); all infra/CI → **Olive** (infra-supervisor). Every bead must
require the worker to read THIS spec + the plan (per
`feedback_bead_description_not_spec`).

| Track | Tasks (this spec §) | Owner | Depends on |
|---|---|---|---|
| A — Layer-1 | A-1 catalogue `block_conditions` (§4.1); A-2 codegen (§4.4.1); A-3 gate set_state/claim/close + per-RPC order (§4.4); A-4 `verify_can_transition` (§4.8); A-5 `meta_catalogue` Resource (§4.7); + validator RPC (§4.2) + error envelope (§4.6) | Greta | P01 mcp + catalogue scaffold |
| A-6 — drift CI | catalogue-drift CI load-bearing (Go ↔ live Resource) | Olive | A-2 + A-5 |
| B — memory | B-1 RPCs + encrypt + ts_doc (§5.1/5.3/5.4.1); B-2 sanitiser + audit + re-scan (§5.4); B-3 tools 24–27 + prime_hints (§5.5); B-4 soft-delete migration (§3.1(b)); B-5 RBAC extend (§5.6); B-6 expires_at (§5.1.1/5.2) | Greta | P01 (independent of A/C) |
| C — providers ingest | C-1 webhook handler (§6.1); C-2 LinkRepo (§6.2); C-3 normaliser + field map (§6.3); C-4 payload digest (§6.6); + async topic/subscriber (§6.4) | Greta | P01 workitems |
| D — providers sync | D-1 sync writer + loop prevention (§6.5); D-2 reconcile cron (§6.7); D-3 RBAC extend (§6.8) | Greta | C-1..C-3 |
| E — infra | E-1 staging; E-2 secrets + guards (§3.4); E-3 capacity report (§8); E-4 cron schedules (§7) | Olive | alongside C/D |
| F — exit harness | F-1..F-6 (§9.1) | Greta | A + B + C (+D for F-5/F-6) |

Migrations (§3.1) land in `apps/api/db/migrations/`; BindDB registration
(§3.3) and the `0150`/`0160`/`0170` files must be in the same logical change as
the service code that depends on them.

---

## 11. Open questions — ALL RESOLVED 2026-06-16

All five open questions were resolved by the user on 2026-06-16, each to THIS
spec's documented default. No alternative was selected; the spec body already
reflects every disposition below.

1. **OQ-A — Layer-1 vs column-value ordering (§4.4.5). RESOLVED 2026-06-16 =
   gate-first.** The pipeline comment-trail gate runs in the MCP handler,
   **before** `SetStateColumns`; the column-value invariants (I-1..I-5 +
   `impl_done_requires_claim`) are the structural backstop. When a pipeline
   gate and a column invariant would both fail, the **pipeline gate's
   `PIPELINE_PRECONDITION_NOT_MET` wins** (it is checked first; the column path
   is never reached). "One error wins" holds. This inverts the original plan
   §2.3 "invariants-first" prose; **plan §2.3 (+ §5.5 C3 row + §7 RP02-2) were
   reconciled to gate-first on 2026-06-16** to agree with this spec. See §4.4.4
   / §4.4.5.
2. **OQ-B — `expires_at` write surface (§5.1.1). RESOLVED 2026-06-16 = wire the
   write surface.** `remember` accepts an optional `expires_at`;
   `recall`/`memories` filter `expires_at IS NULL OR expires_at > now()`
   (combined with `deleted_at IS NULL`, §5.2). No sweeper cron (matches the
   `mcp.api_keys.expires_at` no-sweeper precedent). No DDL change.
3. **OQ-C — sanitiser v1.0 pattern set (§5.4.2). RESOLVED 2026-06-16 = keep the
   7-pattern baseline.** The seven patterns in §5.4.2 are the v1.0 set; no SaaS
   additions at v1.0. The registry stays extensible post-v1.0 (best-effort per
   NFR-7); `pattern_id` keys keep `sanitiser_events` rows stable across future
   additions.
4. **OQ-D — migration slot numbers (§3.1). RESOLVED 2026-06-16 = confirmed.**
   Slots `0150`/`0160`/`0170` are canonical; if a slot is taken by an
   intervening merge the implementer bumps to the next free slot and updates
   the §11-equivalent traceability. The **ordering** (not the exact number) is
   load-bearing.
5. **OQ-E — `verify_can_transition` method name (§4.8). RESOLVED 2026-06-16 =
   confirmed.** Custom JSON-RPC method `unblock/verifyCanTransition`;
   `meta_catalogue` Resource URI `unblock://catalogue`. Both strings stand.

---

## 12. Reference anchors

- **PRD:** §5.1, §5.2, §6.2, §6.5, §6.7 (state machine — the BLOCK source),
  §8 (exit criterion), §9.2, §12.
- **SPEC:** §5.2.2 (27-tool inventory + operational primitives, C2-patched),
  §5.3 (`POST /webhooks/github`), §5.6 (RBAC), §5.7/§5.7.1, §7.4/§7.5/§7.5.1/
  §7.5.2/§7.5.3 (BLOCK schema), §9.4.5 (providers DDL + retention, C1-patched),
  §9.4.8 (memory DDL), §9.4.10 (DEK), §11 (P02 traceability, boards→P05),
  §13 (AR-1/4/7/10/11/12/13/14/16, AR-16 C5-patched).
- **Research:** R-P02-1..13 (incl. 3b/4b) + C1–C5 + R1/R2.
- **Live code anchors (CG-1..13):** `cmd/gen-catalogue/main.go:60,109-111`;
  `mcp/catalogue_test.go:48-72,114-125,129-134,193-214`; `db/db.go:33-34,182-190,273-280`;
  `org.go` Resource constants + `matrix.go:136-139,182-188,218-219` +
  `rbactest_test.go:632` + `rbac.go:314`; `mcp/handler_set_state.go:32-34,43-44,85-88`;
  `workitems/workitems.go:1782-1790,1803-1844`; `mcp/errmap.go:237-297` +
  `errenvelope.go:70`; `auth/secrets.go:136-150`; `mcp/handler_prime.go:58,114-117,232`;
  `deps/cascade_subscriber.go:742,769-779` + `deps/cascade.go:53-57`;
  `mcp/catalogue.go:10-12`; go-sdk v1.6.0 `server.go:519,1352`.
- **Excluded:** `docs/archive/agentic-research/RFC-REACTIVE-AGENT-ENVIRONMENT.md`
  (P02+/additive, not P02).
