# SPEC: P01 — Backend MVP Implementation Contract

**Status:** APPROVED (round-6 cascade-symmetry applied 2026-05-12; round-5 tracing applied 2026-05-12; round-4 auth applied 2026-05-11; DRIFT-1/-2 applied 2026-05-08; round-2 applied 2026-05-08; round-3 research applied 2026-05-08; original APPROVED 2026-05-07)
**Changelog:**
- 2026-05-08 — DRIFT-1 (naming): clarified §3.5 that the four logical secret names are spec-level identifiers; added logical-name ↔ Go-field mapping table for the Encore Go secrets manifest.
- 2026-05-08 — DRIFT-2 (format): corrected the local-secrets file path/format from `.encore/local-secrets.toml` (TOML) to `apps/api/.secrets.local.cue` (CUE) per Encore official docs (https://encore.dev/docs/go/primitives/secrets); updated syntax examples and gitignore guidance.
- 2026-05-11 — round-4 (auth drift fixes from Sherlock's investigation on bead unblock-tv8.7): §4.3.2 step 2 aligned with locked key-format note (DRIFT-A); §4.3.3 AuthHandler signature corrected to Encore structured-params form for header dispatch (DRIFT-B); §12 task table cell for B-1 corrected from §6.4 to §4.3.3 (DRIFT-C); session-path P01 contract pinned (returns errs.Unimplemented; multi-org disambiguation deferred to BFF phase).
- 2026-05-12 — round-5 (tracing contract from Sherlock's investigation on bead `unblock-tv8.5`): §10.2 picks Option B — ULID minted at MCP entry, propagated via `context.Context` only; removed the spurious `X-Unblock-Trace-Id` outgoing-RPC header (Encore's generated client carries `ctx` across private RPCs for free, and Pub/Sub embeds the id in the payload). §7, §8.1, §8.2, §4.5, and §6.3.1 reworded to reference Option B. Encore's runtime `req.Trace.TraceID` is observability-only and not persisted. DDL frozen — no schema change.
- 2026-05-14 — round-7 (cursor keyset pagination, P01): Tools 2 (`ready`), 8 (`list`), and 9 (`search`) now share a cursor keyset pagination contract pinned in the new §6.2.0. Tool 2's "No pagination at v1.0" paragraph is removed; the §6.2 contracts for Tools 2/9 gain `cursor` argument + `next_cursor` result (Tool 8 already carried both). Cursors are opaque base64url-encoded JSON tuples HMAC-signed with `API_KEY_HMAC_SECRET`; the per-tool tuples are `{priority, created_at_unix_us, id}` (Tool 2), `{id}` (Tool 8), `{rank, item_id, comment_id}` (Tool 9). Invalid cursors → §7 `VALIDATION` envelope with `data.field = "cursor"`. No schema change — migration `0100` already covers the Tool 2 ORDER BY.
- 2026-05-12 — round-6 (cascade-symmetry): the cascade subsystem is split into two regimes (new §6.3.0). `is_ready` is maintained inline by the writer that mutated the row/edge (single-hop); `pipeline_stage` is maintained exclusively by the cascade subscriber (multi-hop). `deps.cascade_events.kind` CHECK is extended from 2 values (`'close' | 'edge_removed'`) to 4 values (`'close' | 'edge_added' | 'edge_removed' | 'state_change'`). Tools 6 (close), 11 (add_dependency), 12 (remove_dependency), 13 (set_state — narrow rule for §5.7.1-affecting writes), and `workitems.Claim` (only on the I-3 reset path) post-commit publish `CascadeRequested` with the matching `Reason`. Tool 12 reuses the inline audit row's `event_id` on its post-commit publish; the subscriber's `ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING` collapses the second insert to no-op. The §11.3 single-writer invariant is fractured: (a) `pipeline_stage` single-writer = cascade subscriber; (b) `is_ready` single-writer = the mutating call site (Tool 6 close, §6.5 add_edge, Tool 12 remove_edge, internal helper `deps.recomputeReady`); the linter rule scope tightens to `pipeline_stage` only with an explicit allowlist for `is_ready`. DDL migration `0050_deps.up.sql` updated in lockstep (CHECK list + doc comments).

**Author:** Ada (architect)
**Date:** 2026-05-08
**Source PRD:** [docs/PRD.md](../PRD.md) (APPROVED, 2026-05-07)
**Source SPEC:** [docs/SPEC.md](../SPEC.md) (APPROVED 2026-05-07, round-3 research applied 2026-05-08; cascade_events.kind column added 2026-05-08)
**Source Plan:** [docs/plans/01-plan-backend-mvp.md](../plans/01-plan-backend-mvp.md) (APPROVED 2026-05-07; resolutions applied 2026-05-08)
**Source Research:** [docs/research/01-research-backend-mvp.md](../research/01-research-backend-mvp.md) (closed 2026-05-08; 6× CONTRADICTED, 3× PARTIAL, 1× CONFIRMED)
**Companion:** [docs/MANIFESTO.md](../MANIFESTO.md) (APPROVED, 2026-05-07)

**Round-2 review iterations (2026-05-08).** Architecturally-significant
findings closed in this round:
- D1 — Milestone CRUD private RPCs added in P01 (§4.4); MCP tools deferred
  to P02 (option (c) — preserves FR-8 "18 tools at v1.0").
- D2 — PRD §6.2 five structural state-machine invariants enforced at MCP
  layer in P01 (§6.2 Tool 13, §4.4 SetStateColumns + Claim).
- L7-W2 — `MCPHandler` raw endpoint pinned to a single `//encore:api`
  annotation with the elided-method form (raw-endpoint default per
  ENCORE.md §raw_endpoints; functionally equivalent to a conceptual
  `method=*` wildcard, which Encore v1.52.1 rejects with E1371);
  HTTP-method dispatch lives inside the handler.
- `deps.cascade_events.kind` column added (CHECK enum extended to 4
  values per §6.3.0); used by every `CascadeRequested` consumer.
  Reflected in SPEC §9.4.4 + §3.2 + §6.3.

> Stage 2 deliverable. This document is the **JSON-locked, RPC-locked,
> migration-locked implementation contract** for P01. Every field type,
> every signature, every error envelope, every migration filename is pinned
> here. Phase 02 may extend the surfaces named here, but P01 implementation
> may not deviate from them — deviations are flagged via the `DEVIATION`
> comment trail per Manifesto Law 8.
>
> **Research alignment.** This spec is grounded in the seven contradictions
> closed by Smith's research (C1, C2, C3, C5, C6, C7, plus AF1/AF5). Each
> design choice below references the research finding it honours; assumptions
> the research left as PARTIAL (R-P01-2, R-P01-4, R-P01-7) are pinned with
> explicit values here so implementation has no remaining ambiguity.

---

## 1. Overview

P01 ships the agent-facing core of `://unblock`:

- Five live Encore Go services (`auth`, `org`, `workitems`, `deps`, `mcp`).
- Three schema-only services (`providers`, `boards`, `memory` — DDL ships,
  service code ships in P02 / P05).
- Single Postgres database with **all eight schemas** migrating from a
  single migration-owner directory at `apps/api/db/migrations/`, owned
  by a dedicated zero-API `db` service.
- Streamable HTTP MCP transport (per MCP spec 2025-06-18) at `POST /mcp` +
  `GET /mcp` exposing **14 tools** with Bearer API key auth.
- Cascade subsystem on Encore Pub/Sub maintaining `is_ready` and
  `pipeline_stage` materialised columns.
- Atomic claim transaction (`SELECT FOR UPDATE`).
- Cycle detection at write time using a depth-counter recursive CTE
  guarded by a per-project advisory lock.
- One-shot Go CLI seeder under `apps/api/cmd/unblock-seed/` that
  bootstraps the exit-criterion fixture.
- **Milestones (round-2 D1).** Recursive milestones (PRD §6.3 + SPEC
  §9.4.3) ship in P01 as **private RPCs** (`workitems.CreateMilestone`,
  `UpdateMilestone`, `AssignItem`, `MilestoneTree` — §4.4); the four
  M-INV-2 / M-INV-3 / M-INV-6 / M-INV-7 invariants are enforced in app
  code per the SPEC §9.4.3 DDL note. **Milestone MCP tools defer to P02**
  alongside the memory tools (preserves FR-8 "18 tools at v1.0"); P01
  agents do not see milestone tools.

P01 explicitly **defers** Layer-1 BLOCK conditions (P02), the four memory
tools (P02), the four milestone MCP tools (P02 — see D1 above), GitHub
webhook ingestion (P02), the Astro frontend (P05), the plugin renderer
(P04), and `unblock-code` (P03) — see Plan §3.

**P01 Exit criterion (PRD §8 verbatim):** an agent authenticates via
Bearer API key and completes `prime → ready → claim → close` against a
manually-seeded graph; cascade fires; cycle detection rejects offending
edges.

---

## 2. Research Findings Resolution

This spec embodies the closure of all ten R-P01-* items and the five
additional findings (AF1–AF5) surfaced by Smith. Every contradiction has a
binding design decision below.

| Research finding | Status in research | How this spec resolves it |
|---|---|---|
| **C1 — Pub/Sub envelope `delivery_id`** | CONTRADICTED | §6.4 — publisher generates `event_id` (ULID) at emit; subscriber reads it from typed payload; idempotency key `(event_id, triggered_by_item_id)` enforced by DDL UNIQUE on `deps.cascade_events`. |
| **C2 — Encore DB ownership / multi-schema** | CONTRADICTED | §3.1, §5.1 — a dedicated `apps/api/db/` service is the sole migration-owner AND single binding authority; all eight schemas' migrations live under `apps/api/db/migrations/`. Every domain service receives its `*sqldb.Database` handle via the canonical BindDB late-bind pattern (each service declares `var db *sqldb.Database` + `func BindDB(d *sqldb.Database) { db = d }`; `apps/api/db/db.go::init` calls `<service>.BindDB(DB)` for every consumer). Domain services MUST NOT declare `sqldb.Named("unblock")` at package init — it panics outside the Encore runtime and breaks plain `go test` for leaf packages. |
| **C3 — "rmcp Go bindings" misnomer** | CONTRADICTED | §6.1 — pinned dependency: `github.com/modelcontextprotocol/go-sdk` v0.5.0 (or latest stable at implementation start; pinned by Greta in `go.mod` under task D-1). |
| **C4 — Encore Cloud edge-proxy timeout** | PARTIAL | §11.2 — NFR-1 measurement methodology declares "warm cache, local emulator only"; cloud SSE behaviour is a P02 ops item owned by Olive. P01 spec does not target Cloud. |
| **C5 — Recursive CTE `LIMIT 256` semantics** | CONTRADICTED | §6.5 — cycle CTE uses an explicit `depth` counter with `WHERE depth < 256`. The exact CTE is reproduced verbatim from SPEC §9.4.9. |
| **C6 — `GET /mcp/sse` is deprecated transport** | CONTRADICTED | §5 — Streamable HTTP per MCP 2025-06-18: single endpoint at `/mcp` supporting both `POST` (client → server, may stream) and `GET` (server-initiated SSE). No legacy SSE+POST fallback. |
| **C7 — argon2id over 32-byte API key** | CONTRADICTED | §4.3 — API key hash is `HMAC-SHA256(server_secret, raw_key)` stored as `bytea` (32 bytes raw). Lookup by `key_prefix` (UNIQUE), then constant-time HMAC compare. |
| **R-P01-2 — Encore migration runner** | PARTIAL | §3.1 — single-owner pattern (C2 resolution); migrations sequential per Encore convention; bootstrap migration declares both extensions. |
| **R-P01-4 — Free-tier ceilings vs NFR-1** | PARTIAL | §11.2 — NFR-1 measured on local emulator; warm-cache definition pinned (pool established + identity validated, no cold-start). |
| **R-P01-7 — Multi-table FTS** | PARTIAL | §3.4 — `tsvector` `GENERATED` columns on both `workitems.items` and `workitems.comments`; per-table GIN indexes; `search` RPC issues `UNION ALL` over both. |
| **R-P01-9 — GitHub OAuth scopes** | CONFIRMED | §4.1 — `read:user` (and `user:email` if needed); no `repo` scope at v1.0; PKCE S256 mandatory. |
| **AF1 — `workitems` FTS DDL** | NEW | §3.4 — same as R-P01-7. DDL is already in SPEC §9.4.3 (research-applied). |
| **AF2 — `prime`'s "recent cascade events" cap** | NEW | §6.2 / Tool 1 — last 50 rows scoped to org/project; uses existing `cascade_events_org_triggered_idx`. |
| **AF3 — Close precondition without DDL CHECK** | NEW | §6.2 / Tool 6 — MCP-layer precondition `claimed_by_id IS NOT NULL` enforced in handler; structured error envelope on violation. |
| **AF4 — API key lifecycle for v1.0** | NEW | §4.3 — keys default `expires_at = NULL`; rotation = manual "issue new + revoke old"; no auto-rotate; revocation flips `revoked_at`. |
| **AF5 — Cycle-detection write race** | NEW | §6.5 — `pg_advisory_xact_lock(hashtext('deps.add_dependency:' || $project_id))` acquired at transaction start; serialises racing inserts within a project. |
| **OQ1 — Copilot transport coverage** | OPEN | §11.4 — P01 acceptance harness uses Claude Code only; Copilot coverage is P04 plugin renderer scope. |
| **OQ2 — `MEMORY_DEK` provisioning** | OPEN | §3.5 — `MEMORY_DEK` is provisioned in P01 by Olive via Encore secret manager (the bootstrap migration would fail without it because `auth.oauth_tokens.*_enc` columns are exercised by integration tests). Local emulator uses dev DEK from `apps/api/.secrets.local.cue` (CUE format, per Encore docs). |
| **OQ3 — pgcrypto / pg_trgm availability** | OPEN | §3.1 — bootstrap migration `CREATE EXTENSION IF NOT EXISTS` for both; smoke-tested against the local Encore emulator's bundled Postgres in CI before any other migration runs. |
| **OQ4 — `key_hash` column type** | OPEN | §4.3 — `bytea NOT NULL` (32 bytes raw HMAC output). No hex/base32 encoding ambiguity. |

---

## 3. Database Migrations (canonical filenames)

### 3.1 Migration owner and ordering

Per SPEC §5.2 / research C2: **a dedicated `apps/api/db/` service is the
sole migration-owner AND the sole binding authority for every domain
service's database handle**. It is a zero-API Encore service whose only
responsibilities are:

- declare `sqldb.NewDatabase("unblock", ...)` exactly once across the
  workspace,
- ship the canonical migration set under `apps/api/db/migrations/`,
- bind every consumer's nil `*sqldb.Database` handle from its single
  central `init()`.

It owns no business logic, no RPCs, and no domain schema. The directory
`apps/api/db/migrations/` holds the migration set for the entire
`unblock` database.

Every domain service (`auth`, `org`, `workitems`, `deps`, `mcp`,
`providers`, `boards`, `memory`) follows the **canonical BindDB
late-bind consumer pattern** described below; no domain service writes
migration files, and no domain service declares
`sqldb.Named("unblock")` at package init.

Rationale (decoupling): this decouples `auth` from owning DDL for
schemas it does not consume. Every domain service is an equal consumer
of the database, and the migration-owner role lives in a single piece
of infrastructure wiring rather than being grafted onto a domain
service. The historical auth-as-owner pattern (lifted from the C2
closure at spec approval time) accidentally coupled the auth service's
surface to the lifecycle of org / workitems / deps / etc. DDL — bead
`unblock-bne` corrected that. The `var db = sqldb.NewDatabase(...)`
call still happens in exactly one place across the workspace; only its
location changed.

Rationale (consumer pattern, BindDB late-bind): empirical check
against `encore.dev/storage/sqldb` v1.52.1 disproved the assumption
that `sqldb.Named("unblock")` is a benign runtime lookup — its
implementation calls `doPanic` at package-load time exactly like
`sqldb.NewDatabase` (`pkgfn.go:182-192`). Every domain service that
declared `var db = sqldb.Named("unblock")` at package init therefore
panicked any plain `go test ./apps/api/<service>/...` invocation with
"encore apps must be run using the encore command", breaking the
unblock-xuk goal (plain-`go test`-loads-without-panic) for that
service. The BindDB late-bind shape — a nil `*sqldb.Database` pointer
populated by `apps/api/db/db.go`'s `init` — is the only shape that
preserves the xuk goal across every domain service. Bead
`unblock-bne`'s pre-review scope expansion converted the org service
from the eager `sqldb.Named` shape to BindDB and made BindDB the
canonical pattern for every current and future domain service.

Canonical consumer pattern (mandatory for every domain service that
touches the unblock database):

```go
package <service>

import "encore.dev/storage/sqldb"

// db is populated exactly once at process start by
// apps/api/db/db.go's init via <service>.BindDB(DB).
var db *sqldb.Database

// BindDB installs the unblock-database handle. The companion
// apps/api/db package owns the sqldb.NewDatabase call and invokes
// BindDB from its package init.
func BindDB(d *sqldb.Database) { db = d }
```

Then add a `<service>.BindDB(DB)` line to `apps/api/db/db.go`'s
`init()` so the dedicated db service binds the new handle at process
bootstrap. There is no per-service `initbind.go`; the central bind in
`apps/api/db/` is the sole binding authority.

### 3.2 Migration files (locked filenames)

Filename convention: `NNNN_<descr>.up.sql` with `NNNN` strictly increasing
in steps of 10. Step numbering matches §9.4.0 ordering:

| File | Content |
|---|---|
| `0010_bootstrap.up.sql` | `CREATE EXTENSION IF NOT EXISTS pgcrypto;` and `CREATE EXTENSION IF NOT EXISTS pg_trgm;` |
| `0020_auth.up.sql` | Schema `auth` per SPEC §9.4.1 (tables `users`, `oauth_tokens`, `sessions` + indexes) |
| `0030_org.up.sql` | Schema `org` per SPEC §9.4.2 (tables `organizations`, `members`, `projects`, `project_members` + indexes) |
| `0040_workitems.up.sql` | Schema `workitems` per SPEC §9.4.3 (tables `milestones` (recursive, self-referential `parent_milestone_id`, scope-XOR + date-range CHECK constraints; M-INV-2/3/5/6/7 enforced in app code per SPEC §9.4.3 note), `items`, `labels`, `item_labels`, `comments` + all indexes including FTS GIN per AF1) |
| `0050_deps.up.sql` | Schema `deps` per SPEC §9.4.4 (tables `dependencies`, `cycles`, `cascade_events` + indexes; `cascade_events_event_trigger_uniq` for AR-11 idempotency; `cascade_events.kind` column with CHECK `IN ('close','edge_added','edge_removed','state_change')` — see §6.3.0 for the symmetric writer model that introduced the 4-value enum) |
| `0060_providers.up.sql` | Schema `providers` per SPEC §9.4.5 (tables `installations`, `events`, `mappings` + indexes). **Schema-only in P01** — no service code consumes it until P02. |
| `0070_mcp.up.sql` | Schema `mcp` per SPEC §9.4.6 (tables `api_keys`, `tool_calls` + indexes; `key_hash bytea`, `key_prefix UNIQUE` per C7) |
| `0080_boards.up.sql` | Schema `boards` per SPEC §9.4.7 (tables `boards`, `columns` + indexes). **Schema-only in P01** — no service code until P05. |
| `0090_memory.up.sql` | Schema `memory` per SPEC §9.4.8 (tables `entries`, `entry_refs` + indexes). **Schema-only in P01** — no service code until P02. |

**No `down.sql` files in P01.** Pre-prod (no users, no migration tax per
`feedback_pre_production`). Down migrations re-introduce risk without
benefit at this stage.

### 3.3 Migration content rules

- Files contain DDL only. No data migrations in P01.
- Each file is self-contained: a successful run leaves the schema in the
  exact state declared by the matching SPEC §9.4.X subsection.
- `IF NOT EXISTS` on `CREATE SCHEMA` statements; everything else assumes a
  fresh schema (the migration runner refuses to re-run a file).
- Every `CHECK` and `UNIQUE` constraint receives the named identifier
  declared in SPEC §9.4 (e.g. `comments_kind_chk`, `api_keys_prefix_uniq`).
  These names are part of the contract — error messages reference them.

### 3.4 FTS DDL (AF1 closure)

Both FTS additions ship in `0040_workitems.up.sql`:

```sql
ALTER TABLE workitems.items ADD COLUMN fts tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(body,  '')), 'B')
    ) STORED;
CREATE INDEX items_fts_idx ON workitems.items USING GIN (fts);

ALTER TABLE workitems.comments ADD COLUMN fts tsvector
    GENERATED ALWAYS AS (to_tsvector('english', coalesce(body, ''))) STORED;
CREATE INDEX comments_fts_idx ON workitems.comments USING GIN (fts);
```

The `search` MCP tool issues a `UNION ALL` over `items_fts_idx` and
`comments_fts_idx` (PG GIN indexes are per-table per research D10).

### 3.5 Encore secrets required at P01

Owned by Olive, provisioned via the Encore secret manager. Local emulator
reads from a **CUE** file at `apps/api/.secrets.local.cue` (Encore app
root, next to `encore.app`), per Encore official docs
(https://encore.dev/docs/go/primitives/secrets).

> **DRIFT-1 — naming.** The four secret identifiers below
> (`MEMORY_DEK`, `API_KEY_HMAC_SECRET`, `GITHUB_OAUTH_CLIENT_ID`,
> `GITHUB_OAUTH_CLIENT_SECRET`) are **spec-level logical names**, not
> literal manifest field names. The Encore Go secrets manifest declares
> them as Go struct fields in PascalCase, and Encore secret-manager keys
> + CUE-file keys must use those Go field names verbatim.

**Logical-name ↔ Go-field mapping** (binding for both the secret-manager
key and the `.secrets.local.cue` field name):

| Spec-level logical name | Go struct field (manifest key + CUE key) |
|---|---|
| `MEMORY_DEK` | `MemoryDEK` |
| `API_KEY_HMAC_SECRET` | `APIKeyHMACSecret` |
| `GITHUB_OAUTH_CLIENT_ID` | `GitHubOAuthClientID` |
| `GITHUB_OAUTH_CLIENT_SECRET` | `GitHubOAuthClientSecret` |

The Encore Go secrets manifest (declared once in the `auth` service):

```go
var secrets struct {
    MemoryDEK               string
    APIKeyHMACSecret        string
    GitHubOAuthClientID     string
    GitHubOAuthClientSecret string
}
```

Local override via `apps/api/.secrets.local.cue` uses CUE syntax with the
Go field names verbatim:

```cue
MemoryDEK:               "dev-dek-32-bytes-base64..."
APIKeyHMACSecret:        "dev-hmac-secret..."
GitHubOAuthClientID:     "dev-client-id"
GitHubOAuthClientSecret: "dev-client-secret"
```

| Secret (logical) | Purpose | Used by |
|---|---|---|
| `MEMORY_DEK` | pgcrypto symmetric DEK for `*_enc` columns | `auth` (oauth_tokens encryption tests in P01); fully exercised P02 |
| `API_KEY_HMAC_SECRET` | server-side secret for `HMAC-SHA256(secret, raw_key)` per C7 (Bearer auth) AND `HMAC-SHA256(secret, cursor_payload)` per §6.2.0 (paginated cursor signing — re-uses the same key, no new secret) | `auth` (Bearer auth check on every MCP call; API key issuance); `mcp` (paginated cursor encode/decode per §6.2.0) |
| `GITHUB_OAUTH_CLIENT_ID` | OAuth2+PKCE client id (test app at v1.0) | `auth.ExchangeOAuthCode` |
| `GITHUB_OAUTH_CLIENT_SECRET` | OAuth2+PKCE client secret | `auth.ExchangeOAuthCode` |

**Gitignore status (verified 2026-05-08).** The current
`apps/api/.gitignore` ignores `/.encore` and the generated `encore.gen.*`
artefacts but **does not** cover `apps/api/.secrets.local.cue`. Olive
must add an explicit entry (`/.secrets.local.cue`) to `apps/api/.gitignore`
as part of A-2 so the local-override file is never committed. The edit to
`.gitignore` itself is owned by the implementing supervisor (Greta/Olive)
under bead A-2 — this spec only records the requirement.

**P01 exit criterion does not exercise OAuth interactively** — the seeder
CLI inserts `auth.users` rows directly. The OAuth secrets exist so unit
tests that exercise `auth.ExchangeOAuthCode` against a stubbed provider
have a place to read fixtures from.

---

## 4. Service Surfaces

### 4.1 `auth` service

Owns: schema `auth` only. The canonical migrations directory lives at
`apps/api/db/migrations/` and is owned by the dedicated `db` service
(§3.1). The auth service consumes the database via the canonical
BindDB late-bind pattern (§3.1) in `apps/api/auth/db.go`: a nil
`*sqldb.Database` pointer plus an exported `BindDB(d *sqldb.Database)`
hook populated by `apps/api/db/db.go`'s `init`.

Public APIs: **none** (the OAuth callback lives on the Astro origin per
PRD FR-12; in P01 it is exercised only by integration tests).

Private RPCs (locked signatures):

```go
package auth

// Identity is the resolved caller record carried inside the Encore mesh.
type Identity struct {
    UserID    string // ULID
    OrgID     string // ULID — primary org binding for this auth event
    Role      string // "owner" | "admin" | "member" | "viewer"
    AgentKind string // empty for human sessions; AgentKind value for API-key callers
}

// Validate accepts an opaque token (session id OR raw API key) and resolves
// it to an Identity. Returns ErrUnauthenticated on miss / revoked / expired.
//
//encore:api private method=POST path=/auth.Validate
func Validate(ctx context.Context, req ValidateRequest) (*ValidateResponse, error)

type ValidateRequest struct {
    Token     string // either auth.sessions.id (browser BFF) or raw API key
    TokenKind string // "session" | "api_key"
}
type ValidateResponse struct {
    Identity Identity
}

// ExchangeOAuthCode is called by the Astro Action /auth/[provider]/callback
// (P05) and by P01 integration tests. Verifies PKCE, exchanges the code for
// a provider access token, upserts auth.users + auth.oauth_tokens, and
// issues a new auth.sessions row. Returns the opaque session id.
//
//encore:api private method=POST path=/auth.ExchangeOAuthCode
func ExchangeOAuthCode(ctx context.Context, req ExchangeOAuthCodeRequest) (*ExchangeOAuthCodeResponse, error)

type ExchangeOAuthCodeRequest struct {
    Provider     string // "github" | "gitlab"
    Code         string
    PKCEVerifier string
    UserAgent    string
    IPAddress    string
}
type ExchangeOAuthCodeResponse struct {
    SessionID string // ULID; opaque; used as Bearer for private RPCs
    UserID    string // ULID
    ExpiresAt time.Time
}

// IssueAPIKey creates a new mcp.api_keys row. Called by the seeder CLI
// (P01) and by future operator surfaces. Returns the raw key ONCE — the
// caller stores it; subsequent reads return only the prefix and metadata.
//
//encore:api private method=POST path=/auth.IssueAPIKey
func IssueAPIKey(ctx context.Context, req IssueAPIKeyRequest) (*IssueAPIKeyResponse, error)

type IssueAPIKeyRequest struct {
    OrgID         string // ULID
    IssuedToUser  string // ULID; nullable (org-level service key)
    Label         string // human-readable, e.g. "claude-code-laptop"
    AgentKind     string // AgentKind value
    Scopes        []string
    ExpiresAt     *time.Time // nullable; default: never
}
type IssueAPIKeyResponse struct {
    KeyID     string // ULID (mcp.api_keys.id)
    KeyPrefix string // first 8 chars of the raw key
    RawKey    string // FULL raw key — returned ONCE; never persisted in clear
}

// RevokeAPIKey flips revoked_at; idempotent.
//
//encore:api private method=POST path=/auth.RevokeAPIKey
func RevokeAPIKey(ctx context.Context, req RevokeAPIKeyRequest) error

type RevokeAPIKeyRequest struct {
    KeyID string // ULID
}
```

### 4.2 `org` service

Owns: schema `org` only. Consumes the unblock database via the
canonical BindDB late-bind pattern (§3.1) in `apps/api/org/db.go`:
a nil `*sqldb.Database` pointer plus an exported
`BindDB(d *sqldb.Database)` hook populated by `apps/api/db/db.go`'s
`init`. No per-service `initbind.go`.

Public APIs: **none**.

Private RPCs:

```go
package org

//encore:api private method=POST path=/org.CreateOrganization
func CreateOrganization(ctx context.Context, req CreateOrganizationRequest) (*Organization, error)

//encore:api private method=POST path=/org.CreateProject
func CreateProject(ctx context.Context, req CreateProjectRequest) (*Project, error)

//encore:api private method=GET path=/org.GetOrganization/:id
func GetOrganization(ctx context.Context, id string) (*Organization, error)

//encore:api private method=GET path=/org.GetProject/:id
func GetProject(ctx context.Context, id string) (*Project, error)

//encore:api private method=POST path=/org.AddMember
func AddMember(ctx context.Context, req AddMemberRequest) error

// Authorize is the canonical RBAC predicate. Called by every other service
// before reading or writing a resource. Returns nil on permit;
// ErrForbidden on deny. The org_id of the resource is matched against the
// identity's org_id; cross-tenant calls are rejected here.
//
//encore:api private method=POST path=/org.Authorize
func Authorize(ctx context.Context, req AuthorizeRequest) error

type AuthorizeRequest struct {
    Identity   auth.Identity
    Resource   string // "workitems.items" | "deps.dependencies" | etc.
    Action     string // "read" | "write" | "delete"
    OrgID      string
    ProjectID  string // optional
}
```

### 4.3 `mcp` service

Owns: schema `mcp` (writes only), the public Streamable HTTP endpoint, the
14 P01 tool handlers. Consumes the unblock database via the canonical
BindDB late-bind pattern (§3.1) when DB-touching code lands (P01 D-1+):
a nil `*sqldb.Database` pointer + exported `BindDB` hook in
`apps/api/mcp/db.go`, registered in `apps/api/db/db.go`'s `init`. No
per-service `initbind.go`.

#### 4.3.1 Public endpoint (Streamable HTTP per MCP 2025-06-18 spec)

```go
package mcp

// MCPHandler is the single MCP entry point. Both POST and GET hit the same
// handler; HTTP-method dispatch happens inside the function body. Encore's
// raw-endpoint convention is one //encore:api annotation per function
// (https://encore.dev/docs/go/primitives/raw-endpoints): paired
// POST+GET annotations on a single function are NOT supported by the
// Encore parser. P01 uses the elided-method form (no `method=` token)
// so the same handler receives every HTTP method on `/mcp`; this is
// the documented raw-endpoint default (ENCORE.md §raw_endpoints) and
// is functionally equivalent to the conceptual `method=*` wildcard
// (Encore v1.52.1 rejects the literal `method=*` with E1371
// "Invalid endpoint method"). The handler rejects methods other than
// POST/GET with a 405 reply produced via the Go MCP SDK's transport
// adapter.
//
//encore:api public raw path=/mcp
func MCPHandler(w http.ResponseWriter, r *http.Request) {
    switch r.Method {
    case http.MethodPost:
        // delegate to Go MCP SDK Streamable HTTP POST handler
    case http.MethodGet:
        // delegate to Go MCP SDK Streamable HTTP GET handler (server-initiated SSE)
    default:
        w.Header().Set("Allow", "POST, GET")
        http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
    }
}
```

**L7-W2 closure (round-2).** Earlier drafts of this section showed two
`//encore:api public raw` annotations stacked on a single function (one
per HTTP method). Per Encore's documented raw-endpoint syntax that form
is unsupported — the parser binds at most one annotation per function.
The elided-method form (no `method=` token) is the raw-endpoint
default and delegates routing to the function body — functionally
identical to the conceptual `method=*` wildcard but compatible with
the Encore v1.52.1 parser (the literal `method=*` is rejected with
E1371). This matches the MCP 2025-06-18 spec, which intentionally
puts both methods on the same path. (Alternative considered: split into `MCPPostHandler`
+ `MCPGetHandler` with two separate `//encore:api` declarations and let
Encore's path multiplexer route. Rejected because the Go MCP SDK is
designed around a single transport adapter that owns both methods —
splitting forces a session-store seam between the two handlers that
adds nothing.)

Implementation pinned to `github.com/modelcontextprotocol/go-sdk` (version
chosen at task D-1 implementation start; pinned in `go.mod`; documented in
the PR description). Per research C3 the dependency is the canonical Go MCP
SDK; **`rmcp` (Rust SDK) is not used in the Go backend.**

Auth: every `POST /mcp` and `GET /mcp` request must carry
`Authorization: Bearer <api-key>`. `Mcp-Session-Id` header is set on the
`initialize` response and echoed by the client on subsequent requests.

#### 4.3.2 API key Bearer auth hot path (C7 closure)

On every MCP request:

1. Parse `Authorization: Bearer <raw_key>`.
2. Extract `key_prefix = base32_portion[:8]` (i.e. `raw_key[12:20]` for inputs of the form `unblock_pat_<base32>`). *key_prefix is the first 8 chars of the random base32 portion, **after** stripping the literal `unblock_pat_` (12 chars) — see the locked key-format note below.*
3. `SELECT id, org_id, key_hash, agent_kind, revoked_at, expires_at FROM mcp.api_keys WHERE key_prefix = $1` (uses `api_keys_prefix_uniq` UNIQUE index — O(1) lookup).
4. Reject if `revoked_at IS NOT NULL` or `expires_at IS NOT NULL AND expires_at < now()`.
5. Compute `expected = HMAC-SHA256(API_KEY_HMAC_SECRET, raw_key)`.
6. `if !subtle.ConstantTimeCompare(stored_key_hash, expected) { reject }`.
7. `UPDATE mcp.api_keys SET last_used_at = now() WHERE id = $1` (fire-and-forget).
8. Construct `auth.Identity{UserID: issued_to_user, OrgID: org_id, Role: "agent", AgentKind: agent_kind}` and inject via Encore's auth handler.

Total budget for this path: <5 ms p99 on warm cache (no Argon2 cost per C7).

**Raw key format (locked):** `unblock_pat_<base32-32-byte>` — 12-char fixed
prefix + 32 bytes of crypto/rand encoded as base32 (no padding, lowercase),
total 64 characters. The first 8 chars of the encoded portion populate
`key_prefix` (the literal `unblock_pat_` prefix is stripped before
prefixing — `key_prefix` is over the random portion only). This keeps the
prefix UNIQUE across the entire key space without colliding on the literal
brand prefix.

#### 4.3.3 Auth handler

```go
// AuthParams is the structured auth-handler input. The simple-token form
// (`func AuthHandler(ctx, token string)`) cannot read request headers, and
// this handler MUST inspect `X-Unblock-BFF-Origin` to dispatch between the
// MCP API-key path and the BFF session path. Per Encore's documented
// auth-handler contract (ENCORE.md lines 388-398) the structured-params
// form is the only variant that exposes incoming headers via `header:"…"`
// struct tags, so it is required here.
type AuthParams struct {
    Authorization string `header:"Authorization"`
    BFFOrigin     string `header:"X-Unblock-BFF-Origin"`
}

//encore:authhandler
func AuthHandler(ctx context.Context, p *AuthParams) (auth.UID, *AuthData, error) {
    // Parse Bearer from p.Authorization, dispatch on p.BFFOrigin presence:
    //   - p.BFFOrigin == ""  → MCP path: raw API key (handled by §4.3.2 above)
    //   - p.BFFOrigin != ""  → BFF path: session_id (handled by auth.Validate(TokenKind="session"))
}

type AuthData struct {
    Identity auth.Identity
}
```

> **P01 contract (session-path deferral).** `Validate(TokenKind="session", token=...)` returns `errs.Unimplemented` in P01. The session token path is exercised only by the future BFF (Astro Actions) and the multi-org disambiguation rule (because `auth.sessions` has no `org_id` column and `Identity.OrgID` would require a lookup in `org.members` that is undefined when a user belongs to multiple orgs) will be defined as part of the BFF phase. The seeder bypasses OAuth (§3.5) and the MCP transport (D-1) authenticates via API key only, so this deferral does not affect any P01 acceptance criterion.

### 4.4 `workitems` service

Owns: schema `workitems` only. Consumes the unblock database via the
canonical BindDB late-bind pattern (§3.1) in `apps/api/workitems/db.go`:
a nil `*sqldb.Database` pointer + exported `BindDB` hook, registered
in `apps/api/db/db.go`'s `init`. The skeleton hook is pre-wired in
P01 A-1; RPC bodies start reading `db` in beads B-1+ when DB-touching
code lands (bodies, FTS, milestones, claim transaction, state-machine
invariants). No per-service `initbind.go`.

Private RPCs (called by MCP tool handlers; never directly by clients):

```go
package workitems

//encore:api private method=POST path=/workitems.Create
func Create(ctx context.Context, req CreateRequest) (*Item, error)

type CreateRequest struct {
    OrgID            string
    ProjectID        string
    ParentID         string   // optional epic id; required for type=finding
    DiscoveredFromID string   // required for type=finding
    Type             string   // "epic" | "task" | "finding"
    Title            string   // 1..200 chars
    Body             string   // optional, default ""
    Priority         string   // "P0".."P4"; default "P3"
    MilestoneID      string   // optional
    Labels           []string // label IDs to attach
    Dependencies    []Edge   // optional; cycle-checked atomically with the create
                             // (uses the same Edge type as deps.AddEdge — review L15-W2)
    Severity        string   // required when Type="finding"
    KindOfFinding   string   // "review" | "qa"; required when Type="finding"
}

type DependencyEdge struct {
    BlockerItemID string // from_item: must complete first
    Kind          string // "blocks" | "related"; default "blocks"
}

type Item struct {
    ID                  string
    OrgID               string
    ProjectID           string
    MilestoneID         string
    ParentID            string
    DiscoveredFromID    string
    Type                string
    Title               string
    Body                string
    Status              string // §6.1
    Priority            string // §6.1
    PipelineStage       string // §6.1; subscriber-maintained per SPEC §5.7.1
    AgentKind           string
    ImplState           string // "pending" | "done"
    ReviewState         string // "pending" | "approved" | "needs_rework"
    QAState             string // "pending" | "passed" | "failed"
    PipelineState       string // "running" | "needs_human" | "paused" | "no_investigation"
    Severity            string
    KindOfFinding       string
    ClaimedByID         string
    ClaimedByAgent      string
    ClaimedAt           *time.Time
    IsReady             bool
    MilestoneAssignedAt *time.Time
    MilestoneAssignedBy string
    Labels              []string  // label IDs from workitems.labels (review L3-W6)
    CreatedAt           time.Time
    UpdatedAt           time.Time
    ClosedAt            *time.Time
}

//encore:api private method=POST path=/workitems.Update
func Update(ctx context.Context, req UpdateRequest) (*Item, error)

type UpdateRequest struct {
    ItemID      string
    Title       *string
    Body        *string
    Priority    *string
    MilestoneID *string
    Labels      *[]string // nil = no change; pointer to slice = full replace
}

//encore:api private method=GET path=/workitems.Get/:id
func Get(ctx context.Context, id string) (*Item, error)

//encore:api private method=POST path=/workitems.GetTrail
func GetTrail(ctx context.Context, req GetTrailRequest) (*Trail, error)

type GetTrailRequest struct {
    ItemID string
}
type Trail struct {
    Item              *Item
    Comments          []Comment        // ordered by created_at asc
    DependenciesIn    []Edge           // edges where to_item == Item.ID
    DependenciesOut   []Edge           // edges where from_item == Item.ID
    Findings          []Item           // children with type=finding
}

//encore:api private method=POST path=/workitems.AppendComment
func AppendComment(ctx context.Context, req AppendCommentRequest) (*Comment, error)

type AppendCommentRequest struct {
    ItemID       string
    AuthorID     string // user id; nullable if AuthorAgent set
    AuthorAgent  string // AgentKind value; nullable if AuthorID set
    ParentID     string // optional; thread parent
    Kind         string // PRD §6.5 / SPEC §9.4.3 comments_kind_chk:
                        // investigation | decision | deviation | completed |
                        // review | qa | deferred | pr | needs-human |
                        // override | general
    Status       string // "error" | "warning" | "info" | "success"
    Body         string
}

type Comment struct {
    ID          string
    ItemID      string
    ParentID    string
    AuthorID    string
    AuthorAgent string
    Kind        string
    Status      string
    Body        string
    CreatedAt   time.Time
    UpdatedAt   time.Time
}

//encore:api private method=POST path=/workitems.SetStateColumns
// Writes one or more of (impl_state, review_state, qa_state, pipeline_state)
// + recomputes pipeline_stage via the cascade subscriber path.
//
// **P01 enforces:**
//  - structural invariants (e.g. impl_state=done requires claimed_by_id IS NOT NULL);
//  - the **five PRD §6.2 state-machine invariants** (round-2 D2 — see
//    §6.2 Tool 13 for the canonical table). These are pure column-value
//    rules with no comment-trail dependency, so they ship in P01.
//
// Layer-1 BLOCK conditions (comment-trail-driven preconditions) are P02
// (Plan §3.4); they layer on top of the five invariants below.
//
// All five invariants are enforced inside ONE Postgres transaction. The
// implementation uses a CTE / SELECT ... FOR UPDATE / UPDATE chain in a
// single SQL round-trip (preferred over PL/pgSQL for readability — the
// invariants are independent column-value checks, not iterative). The
// CTE shape is documented in §6.2 Tool 13.
func SetStateColumns(ctx context.Context, req SetStateRequest) (*Item, error)

type SetStateRequest struct {
    ItemID        string
    ImplState     *string
    ReviewState   *string
    QAState       *string
    PipelineState *string
    // The MCP layer attaches a (kind, status, body) comment trail entry as
    // part of the same logical mutation when the agent calls set_state with
    // an intent_comment field. workitems.SetStateColumns DOES NOT write
    // comments — that is AppendComment's job. The MCP tool handler
    // composes both calls in one transaction (§6.2 Tool 13).
}

//encore:api private method=POST path=/workitems.Close
// MCP-layer precondition (AF3): rejects if claimed_by_id IS NULL.
// Sets status=Done, closed_at=now(), emits deps.cascade.requested.
func Close(ctx context.Context, req CloseRequest) (*Item, error)

type CloseRequest struct {
    ItemID  string
    Reason  string // optional free-text recorded as a kind=completed comment
}

//encore:api private method=POST path=/workitems.Claim
// Atomic claim per SPEC §5.5. Runs the SELECT FOR UPDATE transaction.
// Returns the loser-side ErrAlreadyClaimed with claimed_by_id and
// claimed_at populated.
//
// **PRD §6.2 invariant #3 (round-2 D2).** When the item being claimed
// has `qa_state='failed'` at the moment the row is locked, this RPC
// resets `review_state='pending'` AND `qa_state='pending'` atomically
// inside the same transaction (callers MUST NOT expect the failed
// states to persist across a re-claim). The reset is the structural
// implementation of the "next supervisor claim after qa_state=failed"
// rule. AR-18 (round-2) discusses the concurrency interaction with
// SetStateColumns racing the same item.
func Claim(ctx context.Context, req ClaimRequest) (*Item, error)

type ClaimRequest struct {
    ItemID         string
    ClaimerUserID  string
    ClaimerAgent   string // AgentKind value
}

//encore:api private method=POST path=/workitems.List
func List(ctx context.Context, req ListRequest) (*ListResponse, error)

type ListRequest struct {
    OrgID        string
    ProjectID    string
    MilestoneID  string
    Status       []string // any of "Backlog","Ready","InProgress","Blocked","Done"
    PipelineStage []string
    ClaimedBy    string
    Labels       []string
    Limit        int    // 1..200; default 50
    Cursor       string // opaque pagination cursor
}
type ListResponse struct {
    Items      []Item
    NextCursor string
}

//encore:api private method=POST path=/workitems.Search
// Multi-table FTS per AF1: UNION ALL over items_fts_idx and comments_fts_idx.
func Search(ctx context.Context, req SearchRequest) (*SearchResponse, error)

type SearchRequest struct {
    OrgID     string
    ProjectID string
    Query     string // websearch_to_tsquery format
    Limit     int    // 1..100; default 25
}
type SearchResponse struct {
    Hits []SearchHit
}
type SearchHit struct {
    ItemID    string
    Source    string // "item" | "comment"
    CommentID string // populated when Source="comment"
    Rank      float64
    Snippet   string // ts_headline output, ≤ 200 chars
}
```

#### 4.4.1 Milestone RPCs (round-2 D1)

Milestones (PRD §6.3 + SPEC §9.4.3) ship in P01 as **private RPCs only**.
Agent-facing MCP tools defer to P02 alongside memory tools (see §1
overview / round-2 D1: option (c) preserves FR-8 "18 tools at v1.0").
The seeder CLI (§9) and the future Astro client (P05) call these RPCs
directly through Encore's private mesh.

```go
package workitems

//encore:api private method=POST path=/workitems.CreateMilestone
// Creates a milestone scoped to org_id XOR project_id. Enforces:
//  - M-INV-1 (no self-loop)         — DB CHECK milestones_no_self_loop_chk
//  - M-INV-2 (no parent-chain cycle) — recursive CTE walks ancestors of
//    parent_milestone_id; rejects with kind=PRECONDITION_NOT_MET if the
//    new id appears in the ancestor set
//  - M-INV-3 (child date range ⊆ parent date range) — when
//    parent_milestone_id is non-null, fetch parent (start_date, end_date)
//    and reject if (start_date < parent.start_date OR end_date > parent.end_date)
//  - M-INV-5 (child scope matches parent scope) — when parent_milestone_id
//    is non-null, the new row's (org_id, project_id) must match the parent's
//  - M-INV-6 (max depth = 4) — same recursive CTE depth-counts ancestors
//    and rejects when depth would exceed 4
//  - DB CHECKs milestones_scope_xor_chk and milestones_date_range_chk
//    fire as the last line of defence
// M-INV-7 is enforced lazily on AssignItem (see below).
func CreateMilestone(ctx context.Context, req CreateMilestoneRequest) (*Milestone, error)

type CreateMilestoneRequest struct {
    OrgID             string  // ULID; XOR with ProjectID
    ProjectID         string  // ULID; XOR with OrgID
    ParentMilestoneID string  // optional ULID
    Name              string  // 1..200 chars
    Description       string  // optional, default ""
    StartDate         string  // ISO date (YYYY-MM-DD)
    EndDate           string  // ISO date (YYYY-MM-DD); end_date >= start_date
}

type Milestone struct {
    ID                string
    ParentMilestoneID string     // empty when root
    OrgID             string     // empty when project-scoped
    ProjectID         string     // empty when org-scoped
    Name              string
    Description       string
    StartDate         string     // ISO date
    EndDate           string     // ISO date
    CancelledAt       *time.Time
    CancelledReason   string
    CreatedAt         time.Time
    UpdatedAt         time.Time
}

//encore:api private method=POST path=/workitems.UpdateMilestone
// Updates name, description, start_date, end_date, cancelled_at,
// cancelled_reason. Re-validates M-INV-3 against the (possibly changed)
// parent range AND against any existing children (a date-range narrowing
// that violates a child's range is rejected). Reparenting is NOT
// supported in P01 — change parent_milestone_id is rejected with
// kind=VALIDATION (deferred to P02 alongside the milestone MCP tools).
func UpdateMilestone(ctx context.Context, req UpdateMilestoneRequest) (*Milestone, error)

type UpdateMilestoneRequest struct {
    MilestoneID     string
    Name            *string
    Description     *string
    StartDate       *string  // ISO date; pointer = optional
    EndDate         *string  // ISO date
    CancelledAt     *time.Time
    CancelledReason *string
}

//encore:api private method=POST path=/workitems.AssignItem
// Sets workitems.items.milestone_id + milestone_assigned_at +
// milestone_assigned_by atomically. Pass MilestoneID="" to UNASSIGN
// (clears all three columns).
//
// M-INV-7 enforcement (item's milestone scope reachable in item's project):
// the target milestone's scope must satisfy
//   (milestone.project_id = item.project_id)
//   OR (milestone.org_id IS NOT NULL AND milestone.org_id = item.org_id)
// Rejects with kind=PRECONDITION_NOT_MET, data.invariant="M-INV-7" otherwise.
func AssignItem(ctx context.Context, req AssignItemRequest) error

type AssignItemRequest struct {
    ItemID         string
    MilestoneID    string  // ULID; empty string = unassign
    AssignedByUser string  // ULID; the actor performing the assignment
}

//encore:api private method=POST path=/workitems.MilestoneTree
// Returns the recursive milestone tree rooted at RootMilestoneID, OR all
// roots within (OrgID, ProjectID) when RootMilestoneID is empty. Depth
// is capped at M-INV-6 (4) — the recursive CTE walks at most 4 levels
// (matches SPEC §9.4.9 milestone-walk pattern, which is bounded by
// M-INV-6 and is the same source-of-truth CTE used by CreateMilestone /
// UpdateMilestone for ancestor / depth checks).
//
// Used by:
//  - the seeder CLI to verify post-seed shape;
//  - P05 Astro roadmap view (when the milestone MCP tools land in P02
//    they delegate to this RPC).
func MilestoneTree(ctx context.Context, req MilestoneTreeRequest) (*MilestoneTree, error)

type MilestoneTreeRequest struct {
    OrgID             string  // required when RootMilestoneID is empty (XOR ProjectID)
    ProjectID         string  // required when RootMilestoneID is empty (XOR OrgID)
    RootMilestoneID   string  // optional; when set, OrgID/ProjectID derived from it
    IncludeCancelled  bool    // default false; when true, cancelled milestones appear
}

type MilestoneTree struct {
    Roots []MilestoneNode
}

type MilestoneNode struct {
    Milestone Milestone
    Depth     int             // 0 for roots, ≤ 3 for leaves (M-INV-6)
    Children  []MilestoneNode // recursive; empty when Depth = 3 (no further descent)
}
```

**AR-17 (new — round-2).** Milestone tree CTE depth bound. The recursive
CTE in `CreateMilestone` / `UpdateMilestone` / `MilestoneTree` is
structurally bounded by M-INV-6 (max depth 4). Unlike the dependency
cycle CTE (AR-8, depth ≤ 256), milestone walks are cheap by construction.
The CTE uses the same depth-counter pattern (`WHERE depth < 4` inside
the recursive term) — `LIMIT` in the recursive term remains undocumented
PG behaviour per research C5. CreateMilestone rejects with
`kind=PRECONDITION_NOT_MET, data.invariant="M-INV-6"` when the chain
would exceed 4 levels.

### 4.5 `deps` service

Owns: schema `deps` only. Consumes the unblock database via the
canonical BindDB late-bind pattern (§3.1) in `apps/api/deps/db.go`:
a nil `*sqldb.Database` pointer + exported `BindDB` hook, registered
in `apps/api/db/db.go`'s `init`. The skeleton hook is pre-wired in
P01 A-1; RPC bodies start reading `db` in beads C-1+ when DB-touching
code lands (cycle CTE, advisory locks, cascade Pub/Sub publisher).
No per-service `initbind.go`.

Private RPCs:

```go
package deps

//encore:api private method=POST path=/deps.AddEdge
// Acquires per-project advisory lock (AF5), runs the depth-counter
// reachability CTE (C5), inserts the edge, emits deps.cascade.requested
// if the to_item's readiness flips.
func AddEdge(ctx context.Context, req AddEdgeRequest) (*Edge, error)

type AddEdgeRequest struct {
    OrgID     string
    ProjectID string
    FromItem  string
    ToItem    string
    Kind      string // "blocks" | "related"; default "blocks"
}

type Edge struct {
    ID        string
    FromItem  string
    ToItem    string
    Kind      string
    CreatedAt time.Time
    CreatedBy string
}

//encore:api private method=POST path=/deps.RemoveEdge
// Removes edge; sync-inline recomputes is_ready for the direct to_item
// via the shared deps.recomputeReady helper; writes a cascade_events
// audit row (kind='edge_removed') in the same transaction. Then, after
// the transaction commits, publishes CascadeRequested{Reason:"edge_removed"}
// reusing the SAME event_id as the inline audit row (round-6 §6.3.0
// tension #1). The subscriber's INSERT ... ON CONFLICT
// (event_id, triggered_by_item_id) DO NOTHING collapses to no-op for
// the audit row, but the subscriber still walks the forward closure to
// recompute pipeline_stage on transitively affected items.
// `ToItemNowReady` in the response is documented as the SINGLE-HOP view —
// the direct to_item's new is_ready value. Transitive pipeline_stage
// updates downstream of that to_item are eventually consistent (driven
// by the post-commit publish, not by this RPC's return value). See §6.2
// Tool 12 and §6.3.0.
func RemoveEdge(ctx context.Context, req RemoveEdgeRequest) (*RemoveEdgeResponse, error)

type RemoveEdgeRequest struct {
    EdgeID    string  // EdgeID OR (FromItem + ToItem + Kind), exactly one path
    FromItem  string  // composite: paired with ToItem + Kind
    ToItem    string  // composite: paired with FromItem + Kind
    Kind      string  // composite: paired with FromItem + ToItem
}

type RemoveEdgeResponse struct {
    Removed         bool
    ToItemNowReady  bool  // computed inline in same transaction
    ToItemID        string  // resolved from EdgeID if composite path not used
}

//encore:api private method=POST path=/deps.IsReady
// Read-side helper: returns the current is_ready value (read from
// workitems.items, NOT recomputed). Used by smoke tests; production
// readers query workitems.items directly.
func IsReady(ctx context.Context, itemID string) (bool, error)

//encore:api private method=POST path=/deps.Closure
// Returns the transitive 'blocks' closure (incoming) for an item.
func Closure(ctx context.Context, req ClosureRequest) (*ClosureResponse, error)

type ClosureRequest struct {
    ItemID    string
    Direction string // "incoming" | "outgoing"
    MaxDepth  int    // 1..256; default 256
}
type ClosureResponse struct {
    ItemIDs []string
}

//encore:api private method=POST path=/deps.RecentCascadeEvents
// AF2: returns the last 50 deps.cascade_events rows for the org/project,
// ordered by triggered_at DESC. Used by the prime tool.
func RecentCascadeEvents(ctx context.Context, req RecentCascadeEventsRequest) (*RecentCascadeEventsResponse, error)

type RecentCascadeEventsRequest struct {
    OrgID     string
    ProjectID string // optional
    Limit     int    // capped at 50; default 50
}
type RecentCascadeEventsResponse struct {
    Events []CascadeEventRow
}
type CascadeEventRow struct {
    ID                  string
    EventID             string
    TriggeredByItemID   string
    AffectedItemIDs     []string
    CascadedCount       int
    TriggeredAt         time.Time
    TraceID             string // ULID minted by the mcp raw endpoint that triggered the cascade; mirrors deps.cascade_events.trace_id (§10.2 Option B).
}
```

### 4.6 `providers`, `boards`, `memory` (schema-only in P01)

These services have **no Go package code in P01**. Their schemas migrate
in P01 (per Plan §2.1 + Q2 resolution) but no `//encore:api` declarations
exist in their directories until P02 (`providers`, `memory`) and P05
(`boards`).

To prevent the canonical BindDB consumer pattern (§3.1) from referencing
non-existent services, P01 leaves these directories empty (no `.go` files
under `apps/api/providers/`, `apps/api/boards/`, `apps/api/memory/`).
Encore treats absent service directories as non-services; the schemas
exist purely as DB-side artifacts maintained by the dedicated
`apps/api/db/` service's migration runner. When P02 lands `providers`
and `memory` and P05 lands `boards`, each will add its own
`BindDB` hook per §3.1 and a corresponding bind line in
`apps/api/db/db.go`'s `init`.

---

## 5. Public Surface (single Streamable HTTP endpoint)

Per FR-12, P01 exposes **one logical public endpoint**: `POST /mcp` +
`GET /mcp` (Streamable HTTP per MCP spec 2025-06-18).

### 5.1 Transport contract

| Aspect | Value |
|---|---|
| Protocol | HTTP/1.1 + HTTP/2 (Encore-default) |
| Methods | `POST /mcp` (client → server JSON-RPC; may return single `application/json` body OR `text/event-stream` for incremental responses); `GET /mcp` (server → client SSE for resumable sessions) |
| `Accept` (client) | `application/json, text/event-stream` |
| `Authorization` | `Bearer <api-key>` — required on **every** request |
| `Mcp-Session-Id` | Returned by server on `initialize`; echoed by client on subsequent requests |
| Heartbeat | Server emits an MCP-protocol-native JSON-RPC `ping` request over the open session every 15s on long-lived `GET /mcp` streams. On SSE streams the ping surfaces as an `event: message\ndata: {…ping…}` frame, which is what the modelcontextprotocol/go-sdk (v1.6.0, pinned by D-1) emits when its `ServerOptions.KeepAlive` is set per MCP spec 2025-06-18. The earlier `:keepalive\n\n` SSE-comment literal was a pre-SDK placeholder; the JSON-RPC `ping` is the protocol-canonical form and produces the same anti-idle effect at the wire level (mitigates Encore Cloud edge-proxy idle close per RP01-4). |
| Error envelope | JSON-RPC 2.0 error object (see §7) |

### 5.2 What is NOT exposed in P01

- `POST /webhooks/github` — P02 (Plan §3.1).
- `POST /webhooks/gitlab` — v1.1.
- OAuth callback — Astro origin (P05); P01 exercises `auth.ExchangeOAuthCode` via private RPC in tests only.
- `mcp.meta_catalogue` MCP tool — P02 (Plan §3.4 / Q4 resolution).
- `verify_can_transition` — P02.

---

## 6. The 14 P01 MCP Tools (JSON-locked)

Tool names match SPEC §5.2.2. Every tool returns either a typed result
object or a JSON-RPC error object per §7.

The arguments and result schemas below are **canonical** for P01. Phase 02
may add fields (additive only); existing fields are immutable.

### 6.1 MCP framing

JSON-RPC 2.0 over Streamable HTTP. Each tool is dispatched via the
standard MCP `tools/call` method:

```jsonc
{
  "jsonrpc": "2.0",
  "id": "<client-supplied id>",
  "method": "tools/call",
  "params": {
    "name": "<tool-name>",
    "arguments": { /* tool-specific */ }
  }
}
```

Tool-call results follow MCP convention:

```jsonc
{
  "jsonrpc": "2.0",
  "id": "<echo>",
  "result": {
    "content": [ { "type": "text", "text": "<JSON-encoded payload>" } ],
    "isError": false,
    "structuredContent": { /* tool-specific typed payload */ }
  }
}
```

P01 uses `structuredContent` for typed payload (introduced in MCP
2025-06-18 spec) and replicates the JSON in `content[0].text` for clients
that have not adopted `structuredContent` parsing.

### 6.2.0 Cursor keyset pagination

Tools 2 (`ready`), 8 (`list`), and 9 (`search`) expose cursor keyset
pagination over their respective canonical sort tuples. The contract is
identical across all three read surfaces:

- **Argument.** `cursor` is an OPTIONAL opaque string. When absent the
  server returns the first page. When present, the server decodes +
  verifies it and returns rows strictly after the encoded anchor.
- **Result.** `next_cursor` is a string OR `null`. When `null` the
  caller has reached the end of the stream — the response is the final
  page. When a string, passing it back as the next request's `cursor`
  yields the next page with zero duplicates and zero skips.
- **Encoding.** Cursors are `base64url`-encoded JSON tuples followed by
  an HMAC-SHA256 tag computed with the deployment's
  `API_KEY_HMAC_SECRET` (re-used; no new secret is introduced in P01).
  Tuple shape per tool:
  - Tool 2 `ready`: `{priority, created_at_unix_us, id}`.
  - Tool 8 `list`: `{id}` (List orders by `id ASC` only — see §4.4).
  - Tool 9 `search`: `{rank, item_id, comment_id}`.
- **Validation errors.** Any of (decode failure, HMAC mismatch, wrong
  tuple shape, tuple field type mismatch) MUST return the §7
  `VALIDATION` envelope with `data.field = "cursor"`. Cursors are NOT
  cross-tool portable — a Tool 2 cursor presented to Tool 8 is a
  shape mismatch and is rejected.
- **Lifetime.** Cursors are NOT persisted server-side. They survive
  process restarts (signed by the secret) but a secret rotation
  invalidates every outstanding cursor — a pre-prod-acceptable
  operational tradeoff identical to the API-key contract (§4.3.2).

Rationale: after migration `0100` (Tool 2's covering index) the keyset
ORDER BY for the three read tools is served from a pure index scan.
Filters (`priority_min`, `project_id`, `claimed_by`, `labels`,
FTS predicate) compose with the cursor predicate as additional WHERE
clauses — they narrow the result set but do NOT substitute for
pagination when an agent legitimately needs to consume the full set
in chunks.

### 6.2 Tool-by-tool contracts

> **Round-2 D1 deferral note.** Milestone CRUD MCP tools
> (`create_milestone`, `update_milestone`, `assign_item`,
> `milestone_tree`) are **NOT** in the P01 14-tool inventory. They ship
> in P02 alongside the four memory tools (option (c) preserves PRD FR-8
> "18 tools at v1.0"). The `workitems.CreateMilestone`,
> `workitems.UpdateMilestone`, `workitems.AssignItem`, and
> `workitems.MilestoneTree` private RPCs (§4.4.1) ARE available in P01
> for the seeder CLI (§9) and the future Astro client (P05). Tool 4
> (`create`) and Tool 5 (`update`) accept a `milestone_id` field that
> references an existing milestone — they do not create or modify
> milestone rows. Tool 8 (`list`) accepts `milestone_id` as a filter.

#### Tool 1 — `prime`

Returns the dashboard for a fresh agent session.

```jsonc
// arguments
{
  "project_id": "<ULID; optional — defaults to caller's primary project>",
  "ready_limit": 10  // 1..50; default 10
}

// structuredContent
{
  "ready_summary": {
    "count_total": 42,
    "items": [ /* up to ready_limit Item objects */ ]
  },
  "claimed_by_me": [ /* Item objects where claimed_by_id = caller's user_id */ ],
  "recent_cascade_events": [ /* last 50 CascadeEventRow per AF2 */ ],
  "memory_hints": []  // empty in P01; populated in P02 once memory ships
}
```

#### Tool 2 — `ready`

```jsonc
// arguments
{
  "project_id": "<ULID; optional>",
  "limit": 10,         // 1..200; default 10
  "priority_min": "P3", // optional; "P0".."P4"
  "cursor": "<opaque>"  // optional; first page when absent
}

// structuredContent
{
  "items": [ /* Item objects ordered by (priority asc, created_at asc, id asc) */ ],
  "total_ready": 0,        // total count for the org/project, may exceed `limit`
  "next_cursor": "<opaque|null>"  // null when this is the last page
}
```

Read implementation: filtered scan of `workitems.items` using
`items_ready_partial_idx` (`WHERE is_ready = true AND status = 'Ready' AND
closed_at IS NULL`). Deterministic ordering is guaranteed by the
`(priority, created_at, id)` composite sort; `id` is a ULID so it serves
as a stable tiebreaker. After migration `0100` the index covers
`(org_id, project_id, priority, created_at, id)` so the ORDER BY +
keyset pagination is served from a pure index scan.

**Cursor keyset pagination.** `cursor` is an opaque, server-signed token
that encodes the last-row position on the canonical sort tuple
`(priority, created_at, id)`. On request, the server decodes + verifies
the token and emits the page strictly after that anchor; on response,
`next_cursor` is the token for the row that would start the next page
(or `null` when no more rows exist). Token shape, signing, and error
contract are pinned in §6.2.0 (Cursor keyset pagination) below. Agents
MUST NOT manufacture or mutate cursor values — invalid cursors are
rejected with the §7 `VALIDATION` envelope (`data.field = "cursor"`).

#### Tool 3 — `claim`

```jsonc
// arguments
{
  "item_id": "<ULID>"
}

// structuredContent (success)
{
  "claimed": true,
  "item": { /* Item with claimed_by_id, claimed_at populated */ }
}
```

Loser receives the structured error envelope (§7) with code
`ALREADY_CLAIMED` and `data.winner_user_id`, `data.winner_agent`,
`data.claimed_at`.

#### Tool 4 — `create`

```jsonc
// arguments — mirrors workitems.CreateRequest
{
  "project_id": "<ULID>",
  "parent_id": "<ULID; optional>",
  "discovered_from_id": "<ULID; optional, required for finding>",
  "type": "task",                    // "epic" | "task" | "finding"
  "title": "Implement /ready handler",
  "body": "...",
  "priority": "P2",
  "milestone_id": "<ULID; optional>",
  "labels": ["<label-ULID>", ...],
  "dependencies": [
    { "blocker_item_id": "<ULID>", "kind": "blocks" }
  ],
  "severity": "...",                 // required when type=finding
  "kind_of_finding": "review"        // required when type=finding
}

// structuredContent
{
  "item": { /* Item */ }
}
```

Cycle check (C5/AF5) runs inline for any `dependencies[]` entries on the
new item; if any would create a cycle, the entire `create` is rejected.

#### Tool 5 — `update`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "title": "<string; optional>",
  "body": "<string; optional>",
  "priority": "<P0..P4; optional>",
  "milestone_id": "<ULID|null; optional>",
  "labels": ["<label-ULID>", ...]    // optional; full replace when present
}
```

Does NOT touch state dimensions — use `set_state` for those.

#### Tool 6 — `close`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "reason": "<string; optional>"  // recorded as a kind=completed comment if present
}

// structuredContent
{
  "item": { /* Item with status=Done, closed_at populated */ }
}
```

**P01-only precondition (AF3, plan §3.4):** rejects with
`PRECONDITION_NOT_MET` and `data.missing = "claimed_by_id"` if
`claimed_by_id IS NULL`. The full Layer-1 BLOCK conditions
(`qa_state=passed` etc.) ship in P02.

Side-effects (round-6 §6.3.0 symmetric writer model):

(a) **Inline (Regime A — `is_ready`).** In the same transaction as the
status flip to Done, the handler recomputes `is_ready` for the closed
item's direct `blocks` neighbours via the shared
`deps.recomputeReady(ctx, tx, neighbour_id)` helper. This covers the
single-hop view — the immediate dependents may now flip ready.

(b) **Post-commit (Regime B — `pipeline_stage`).** After the
transaction commits, the handler publishes
`CascadeRequested{Reason:"close", TriggeredByItemID: item_id, …}` on
`deps.cascade.requested`. The subscriber walks the forward `blocks`
closure and recomputes `pipeline_stage` per §5.7.1 on every
transitively affected item; it writes one `deps.cascade_events` row
with `kind='close'`.

Cross-reference: §6.3.0 (propagation regimes), §6.3.2 (subscriber
dispatch).

#### Tool 7 — `show`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "include_comments": true,         // default true
  "include_dependencies": true,     // default true
  "include_findings": true          // default true
}

// structuredContent
{
  "item": { /* Item */ },
  "comments": [ /* Comment[] ordered by created_at asc */ ],
  "dependencies_in":  [ /* Edge[] where to_item = item_id */ ],
  "dependencies_out": [ /* Edge[] where from_item = item_id */ ],
  "findings":         [ /* Item[] of children with type=finding */ ]
}
```

#### Tool 8 — `list`

```jsonc
// arguments
{
  "project_id": "<ULID; optional>",
  "milestone_id": "<ULID; optional>",
  "status": ["Ready", "InProgress"],  // optional []
  "pipeline_stage": ["Implementation"], // optional []
  "claimed_by": "<user-ULID; optional>",
  "labels": ["<label-ULID>"],         // optional []
  "limit": 50,                        // 1..200; default 50
  "cursor": "<opaque>"
}

// structuredContent
{
  "items": [ /* Item[] */ ],
  "next_cursor": "<opaque|null>"
}
```

#### Tool 9 — `search`

```jsonc
// arguments
{
  "project_id": "<ULID; optional>",
  "query": "ready handler",   // websearch_to_tsquery format
  "limit": 25,                 // 1..100; default 25
  "cursor": "<opaque>"         // optional; first page when absent
}

// structuredContent
{
  "hits": [
    {
      "item_id": "<ULID>",
      "source": "item",            // "item" | "comment"
      "comment_id": "<ULID|null>",
      "rank": 0.87,
      "snippet": "<ts_headline output, ≤ 200 chars>"
    }
  ],
  "next_cursor": "<opaque|null>"  // null when this is the last page
}
```

Query plan: `UNION ALL` over `items_fts_idx` and `comments_fts_idx`
(per AF1 / R-P01-7), filtered by `org_id` (and `project_id` if supplied)
via the RBAC helper, ranked by `ts_rank_cd` desc, limited to N. Keyset
pagination uses the canonical FTS sort tuple `(rank desc, item_id asc,
comment_id asc)` — see §6.2.0 for the cursor contract; `comment_id` is
the empty string for `source="item"` rows so the tiebreaker is total.

#### Tool 10 — `comment`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "parent_id": "<ULID; optional thread parent>",
  "kind": "investigation",         // §6.5 kinds
  "status": "info",                // "error" | "warning" | "info" | "success"
  "body": "..."                    // 1..16384 chars
}

// structuredContent
{
  "comment": { /* Comment */ }
}
```

Append-only by construction (no update/delete tool ships in P01).

#### Tool 11 — `add_dependency`

```jsonc
// arguments
{
  "from_item_id": "<ULID>",   // blocker
  "to_item_id":   "<ULID>",   // blocked
  "kind": "blocks"            // "blocks" | "related"; default "blocks"
}

// structuredContent
{
  "edge": { /* Edge */ }
}
```

Cycle check: per-project advisory lock + depth-counter CTE per §6.5 below.
On rejection, error code `CYCLE_DETECTED` with `data.cycle_path = ["<id>", ...]`.

**`project_id` derivation (review L6-W8).** The advisory lock key is the
`to_item_id`'s `project_id`, looked up in `workitems.items` at the start
of the transaction. **P01 rejects cross-project edges** with
`code: VALIDATION, kind: VALIDATION, data.field = "to_item_id"` if
`workitems.items[from_item_id].project_id != workitems.items[to_item_id].project_id`.
Cross-project dependencies are explicitly out-of-scope at v1.0 (the
single-project advisory lock is the simplest correct concurrency model;
cross-project locking would need org-level coordination that adds
complexity without v1.0 value).

**Required field for type=finding** (review L6-W1, cross-cutting):
when `from_item_id` references a `type='finding'` work item, the edge
is allowed (findings can block other items) but the
`items_finding_required_fields_chk` constraint on the `from_item_id`
row must already be satisfied — the spec relies on the DDL CHECK
having been enforced at finding creation time.

**Side-effects (round-6 §6.3.0 symmetric writer model).** After the
transaction in §6.5 commits, the handler publishes
`CascadeRequested{Reason:"edge_added", TriggeredByItemID: to_item_id,
EventID: ulid.New(), TraceID: tracectx.From(ctx), EmittedAt: time.Now()}`
on `deps.cascade.requested`. The inline §6.5 UPDATE covers the
single-hop `is_ready` recompute for `to_item`; the publish drives the
multi-hop `pipeline_stage` recompute on the forward closure (Regime B).
The subscriber writes one `deps.cascade_events` row with
`kind='edge_added'`.

#### Tool 12 — `remove_dependency`

```jsonc
// arguments
{
  "edge_id": "<ULID>"        // OR (from_item_id + to_item_id + kind)
}

// structuredContent
{
  "removed": true,
  "to_item_now_ready": true   // computed inline; sync within the same transaction
}
```

**Implementation (round-6 §6.3.0 symmetric writer model).** The
single-hop `is_ready` recompute on the direct `to_item` runs inline in
the same Postgres transaction as the `DELETE` (Regime A); the
multi-hop `pipeline_stage` recompute is driven by a post-commit
`CascadeRequested` publish (Regime B). The inline audit row's
`event_id` is **reused** as the publish envelope's `EventID` so the
subscriber's later insert collapses to no-op via the `ON CONFLICT
(event_id, triggered_by_item_id) DO NOTHING` clause (tension #1
ruling — exactly one `deps.cascade_events` row per logical edge
remove). The whole inline flow:

```
event_id := ulid.New()  -- captured before BEGIN so it can be reused
                           -- on the post-commit publish below
BEGIN;
  DELETE FROM deps.dependencies WHERE id = $edge_id (or composite);
  -- Regime A: shared helper deps.recomputeReady(ctx, tx, item_id)
  --   recomputes is_ready for the direct to_item via the closure CTE
  --   (§6.5) and writes UPDATE workitems.items SET is_ready = $new
  to_item_now_ready := deps.recomputeReady(ctx, tx, $to_item_id);
  -- Audit row written inline; event_id is the ULID captured above.
  -- `kind='edge_removed'` is the discriminant (see §9.4.4 + §3.2).
  INSERT INTO deps.cascade_events (id, event_id, kind,
    triggered_by_item_id, affected_item_ids, cascaded_count, ...)
    VALUES (ulid(), event_id, 'edge_removed', $to_item_id,
            ARRAY[$to_item_id], 1, ...);
COMMIT;
-- Regime B: post-commit publish, REUSING event_id.
deps.CascadeRequestedTopic.Publish(ctx, &deps.CascadeRequested{
    EventID:           event_id,                -- reused (tension #1)
    OrgID:             $org_id,
    ProjectID:         $project_id,
    TriggeredByItemID: $to_item_id,
    Reason:            "edge_removed",
    TraceID:           tracectx.From(ctx),
    EmittedAt:         time.Now(),
})
return { removed: true, to_item_now_ready };
```

**`to_item_now_ready` is the single-hop view.** The boolean returned
in the RPC response reflects ONLY the direct `to_item`'s
`is_ready` value after the inline recompute. Transitive
`pipeline_stage` updates on items downstream of `to_item` are
**eventually consistent** — they are applied by the cascade
subscriber after the publish above is delivered. Callers that need
the transitively-consistent view should poll `get_state` or read
`workitems.items.pipeline_stage` after observing the corresponding
`deps.cascade_events` row land.

**Exactly one audit row per logical remove (tension #1).** The inline
INSERT lands first (during the transaction) and the subscriber's
attempted INSERT collapses via the UNIQUE
`(event_id, triggered_by_item_id)` constraint and the
`ON CONFLICT … DO NOTHING` clause in §6.3.2 step 5. The subscriber
still performs its `pipeline_stage` recompute pass; the no-op only
applies to the audit-row insert.

**Why a publish at all (round-6 rationale).** Pre-round-6, this tool
ran fully sync inline and did not publish. The round-6 cascade-symmetry
review observed that removing an edge can flip the `pipeline_stage`
of items transitively downstream of `to_item` per §5.7.1 (the
upstream chain's readiness changes). The single-hop inline UPDATE
covers `is_ready` for `to_item` but cannot cover transitive
`pipeline_stage` changes without doing the subscriber's work twice.
Symmetric writer model (§6.3.0): single-hop inline, multi-hop via
publish — uniform across the four cascade kinds.

#### Tool 13 — `set_state`

```jsonc
// arguments
{
  "item_id": "<ULID>",
  "impl_state":     "<pending|done; optional>",
  "review_state":   "<pending|approved|needs_rework; optional>",
  "qa_state":       "<pending|passed|failed; optional>",
  "pipeline_state": "<running|needs_human|paused|no_investigation; optional>",
  "intent_comment": {                  // optional but recommended
    "kind": "completed",
    "status": "success",
    "body": "Implementation complete; all tests pass"
  }
}

// structuredContent
{
  "item": { /* Item with the new state columns + recomputed pipeline_stage */ }
}
```

**P01 enforcement (round-2 D2 — five PRD §6.2 invariants + structural
checks):**

Writes are gated by:

(a) **Structural invariants:**

- `impl_state=done` requires `claimed_by_id IS NOT NULL` (DB-level CHECK
  via `items_claim_status_chk` once `status` is updated; also enforced
  defensively at the MCP layer with a clearer error).
- The `(impl_state, review_state, qa_state, pipeline_state)` CHECK
  constraints from `0040_workitems.up.sql` reject malformed enum
  combinations (e.g. unknown values).

(b) **The five PRD §6.2 state-machine invariants (round-2 D2).** Each is
enforced inside the same transaction as the column write; on violation,
the RPC returns `kind=PRECONDITION_NOT_MET` with `data.invariant`
populated for machine-readability:

| # | Invariant (PRD §6.2 verbatim) | Enforcement | `data.invariant` |
|---|---|---|---|
| I-1 | Writing `review_state=needs_rework` resets `qa_state=pending` in the same transaction | Atomic UPDATE: when `req.review_state='needs_rework'`, the SQL writes both columns (no error case — invariant is auto-applied) | n/a (no rejection; auto-reset applied) |
| I-2 | Writing `qa_state=failed` requires `review_state=approved` | Pre-check inside the FOR UPDATE: reject if `req.qa_state='failed'` AND current `review_state <> 'approved'` (after applying any concurrent `req.review_state` change in the same call) | `qa_failed_requires_review_approved` |
| I-3 | After `qa_state=failed`, the next supervisor `claim` resets `review_state=pending` + `qa_state=pending` atomically | Enforced in `workitems.Claim`, NOT here. Documented for cross-reference. | n/a (lives in Claim) |
| I-4 | `impl_state=done` is required before `review_state` can leave `pending` | Pre-check inside the FOR UPDATE: reject if `req.review_state IN ('approved','needs_rework')` AND current `impl_state <> 'done'` (after applying any concurrent `req.impl_state` change) | `review_change_requires_impl_done` |
| I-5 | Transitioning `impl_state=done → pending` is allowed only via the rework path (Review NEEDS-REWORK or QA FAIL) | Pre-check: reject if `req.impl_state='pending'` AND current `impl_state='done'` AND NOT (`req.review_state='needs_rework'` OR `req.qa_state='failed'` OR (current `qa_state='failed'` AND `req.qa_state IS NULL`)) | `impl_done_to_pending_requires_rework_path` |

The implementation builds these checks as a CTE chain inside one SQL
round-trip; pseudo-shape:

```sql
WITH locked AS (
  SELECT impl_state, review_state, qa_state, pipeline_state
    FROM workitems.items
   WHERE id = $item_id
   FOR UPDATE
),
new_values AS (
  SELECT COALESCE($req_impl_state,     locked.impl_state)     AS new_impl,
         COALESCE($req_review_state,   locked.review_state)   AS new_review,
         CASE WHEN $req_review_state = 'needs_rework' THEN 'pending'
              ELSE COALESCE($req_qa_state, locked.qa_state)
         END                                                   AS new_qa, -- I-1
         COALESCE($req_pipeline_state, locked.pipeline_state) AS new_pipe
    FROM locked
),
validated AS (
  SELECT *,
         -- I-2
         (new_qa = 'failed' AND new_review <> 'approved')                               AS violates_i2,
         -- I-4
         (new_review IN ('approved','needs_rework') AND new_impl <> 'done')             AS violates_i4,
         -- I-5
         (new_impl = 'pending' AND locked.impl_state = 'done'
          AND NOT (new_review = 'needs_rework' OR new_qa = 'failed'))                   AS violates_i5
    FROM new_values, locked
)
-- the application layer reads `validated`, returns PRECONDITION_NOT_MET
-- with the matching data.invariant if any violation flag is true,
-- otherwise issues the UPDATE with the validated columns.
```

**Layer-1 BLOCK conditions (comment-trail-driven preconditions, e.g.
`qa_state → passed` requires a `(kind=qa, status=success)` comment) ship
in P02** per Plan §3.4 and PRD §8 P02 exit criterion. P01 implementation
of `set_state` writes the `intent_comment` (if present) atomically with
the state mutation but does NOT verify any comment-trail-based precondition.

**Side-effects (round-6 §6.3.0 symmetric writer model — tension #3
narrow rule).** After the validating UPDATE commits, the handler
publishes `CascadeRequested{Reason:"state_change", TriggeredByItemID:
item_id, …}` ONLY when the write changes `(impl_state, review_state,
qa_state)` in a way that materially affects §5.7.1 `pipeline_stage`
derivation. Pure `pipe_state` mutations (with no change to the other
three columns) do NOT publish — §5.7.1 derives `pipeline_stage` from
the upstream chain's readiness/closure, not from a downstream item's
own `pipe_state`. The publish drives the multi-hop `pipeline_stage`
recompute on the forward `blocks` closure (Regime B). Note: this tool
writes no `is_ready`-affecting state directly; Regime A is not invoked
here.

**AR-18 (new — round-2).** State-invariant interaction with concurrent
`Claim`. Invariant I-3 is enforced in `workitems.Claim` (not in
`SetStateColumns`); a racing `SetStateColumns(qa_state=failed)` and
`Claim` on the same item could in principle observe an inconsistent
intermediate (Claim sees `qa_state=passed`, then SetStateColumns flips
it to `failed` after Claim's transaction commits — the next Claim then
sees `failed` and applies I-3). Both RPCs use `SELECT FOR UPDATE` on
the row, so the second transaction always observes the first's commit;
the architecture is correct by serialisation, not by avoidance. The
exit-criterion harness adds a property test: N=100 concurrent
`Claim`/`SetStateColumns(qa_state=failed)` interleavings on the same
item; assert that every Claim winner observes either the pre-failure
or post-failure state, never a torn read, and that whenever a Claim
observes `qa_state='failed'`, it resets both review and qa to
`pending` atomically. This invariant test is part of §11.1 acceptance
(I-1..I-5 below).

#### Tool 14 — `get_state`

```jsonc
// arguments
{
  "item_id": "<ULID>"
}

// structuredContent
{
  "impl_state":     "...",
  "review_state":   "...",
  "qa_state":       "...",
  "pipeline_state": "...",
  "pipeline_stage": "...",   // materialised
  "is_ready":       true,
  "claimed_by_id":  "<ULID|null>",
  "claimed_at":     "<ts|null>",
  "recent_kinds": [
    { "kind": "investigation", "status": "info",    "comment_id": "...", "created_at": "..." },
    { "kind": "decision",      "status": "info",    "comment_id": "...", "created_at": "..." },
    { "kind": "completed",     "status": "success", "comment_id": "...", "created_at": "..." }
  ]
}
```

`recent_kinds[]` returns the most recent `(kind, status)` per `kind`
(grouped, ordered by `created_at desc`, one row per kind).

### 6.3 Cascade subsystem (Manifesto Law 1)

#### 6.3.0 Propagation regimes (round-6 cascade-symmetry)

The cascade subsystem maintains two materialised columns on
`workitems.items`: `is_ready` (single-hop derivation; depends only on
the direct incoming `blocks` edges) and `pipeline_stage` (multi-hop
derivation; depends on the upstream chain's readiness/closure per
§5.7.1). Round-6 splits the writer responsibility along that natural
boundary.

**Regime A — `is_ready` (single-hop, writer-inline).** Every call site
that mutates a row or edge in a way that can flip `is_ready` for the
**directly** affected item recomputes `is_ready` synchronously inside
the same SQL transaction as the mutation, via the shared helper
`deps.recomputeReady(ctx, tx, item_id)`. The cascade subscriber never
writes `is_ready`. Allowed writers:

- `workitems.Close` (Tool 6) — recomputes `is_ready` for the closed
  item's direct `blocks` neighbours inline.
- `deps.AddEdge` (Tool 11 / §6.5 cycle-detect block) — recomputes
  `is_ready` for `to_item` inline (the new edge may now block it).
- `deps.RemoveEdge` (Tool 12) — recomputes `is_ready` for the direct
  `to_item` inline.
- `deps.recomputeReady` — the shared helper itself (internal).

**Regime B — `pipeline_stage` (multi-hop, subscriber-only).** The
cascade subscriber is the **sole writer** of `pipeline_stage`. Every
call site that materially mutates §5.7.1 derivation inputs publishes
`CascadeRequested{Reason:<kind>, TriggeredByItemID:<id>, …}` after its
transaction commits. The subscriber walks the forward `blocks` closure
from `TriggeredByItemID` and recomputes `pipeline_stage` (only) for the
affected items.

**Who emits which Reason, what the subscriber does:**

| `Reason` | Emitted by | Trigger | Subscriber behaviour |
|---|---|---|---|
| `"close"` | `workitems.Close` (Tool 6) post-commit | Status flipped to Done. | Walk forward `blocks` closure from the closed item; recompute `pipeline_stage` on every reachable item per §5.7.1. |
| `"edge_added"` | `deps.AddEdge` (Tool 11 / §6.5) post-commit | New `blocks` edge committed. | Walk forward `blocks` closure from `to_item`; recompute `pipeline_stage` (the new upstream blocker may push downstream stages backward). |
| `"edge_removed"` | `deps.RemoveEdge` (Tool 12) post-commit, with `event_id` REUSED from the inline audit row | Edge deleted. | Walk forward `blocks` closure from `to_item`; recompute `pipeline_stage` on transitively reachable items. The audit-row insert collapses to no-op via `ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING` (the inline path already wrote it). |
| `"state_change"` | `workitems.SetStateColumns` (Tool 13) post-commit when the write changes `(impl_state, review_state, qa_state)` materially per §5.7.1; AND `workitems.Claim` post-commit ONLY on the I-3 reset path (current `qa_state='failed'` triggers the in-transaction reset of `review_state` and `qa_state` to `pending`). | State-column mutation that affects §5.7.1 derivation. | Walk forward `blocks` closure from the mutated item; recompute `pipeline_stage` (downstream items may transition between Implementation / Review / QA stages). |

**Explicit non-publishers (round-6 tensions, resolved).**

- `workitems.SetStateColumns` writes that affect ONLY `pipe_state`
  (with no change to `impl_state`/`review_state`/`qa_state`) do NOT
  publish — §5.7.1 derives `pipeline_stage` from the upstream chain's
  readiness/closure, not from a downstream item's own `pipe_state`
  (tension #3 ruling).
- `workitems.Claim` in the normal Ready→InProgress path (no I-3 reset
  fires) does NOT publish — the claimed item was non-Done before and
  remains non-Done; no §5.7.1 downstream re-derivation is needed
  (tension #2 ruling). Only the I-3 reset path publishes.

**Audit-row kind reuse.** `deps.cascade_events.kind` carries the same
discriminant value as the `Reason` field of the originating
`CascadeRequested`. The CHECK constraint enumerates all four kinds
(see §9.4.4 + the `0050_deps.up.sql` migration). The
`(event_id, triggered_by_item_id)` UNIQUE constraint remains the
AR-11 idempotency mechanism across both regimes.

#### 6.3.1 Pub/Sub topics

```go
package deps

import "encore.dev/pubsub"

type CascadeRequested struct {
    EventID            string // ULID, generated by publisher (C1 closure)
    OrgID              string
    ProjectID          string
    TriggeredByItemID  string
    Reason             string // "close" | "edge_added" | "edge_removed" | "state_change"
    TraceID            string // ULID minted by the mcp raw endpoint, copied from ctx into the payload at publish time (Encore Pub/Sub does not propagate context across the topic boundary). Persisted on deps.cascade_events.trace_id by the subscriber. See §10.2 Option B.
    EmittedAt          time.Time
}

type CascadeCompleted struct {
    EventID             string
    TriggeredByItemID   string
    AffectedItemIDs     []string
    CascadedCount       int
    CompletedAt         time.Time
}

var CascadeRequestedTopic = pubsub.NewTopic[*CascadeRequested]("deps.cascade.requested",
    pubsub.TopicConfig{DeliveryGuarantee: pubsub.AtLeastOnce})

var CascadeCompletedTopic = pubsub.NewTopic[*CascadeCompleted]("deps.cascade.completed",
    pubsub.TopicConfig{DeliveryGuarantee: pubsub.AtLeastOnce})
```

#### 6.3.2 Subscriber (idempotent per AR-11)

```go
var _ = pubsub.NewSubscription(CascadeRequestedTopic, "deps-cascade-subscriber",
    pubsub.SubscriptionConfig[*CascadeRequested]{
        Handler: handleCascadeRequested,
        // No retry override: Encore default backoff applies.
    })

func handleCascadeRequested(ctx context.Context, msg *CascadeRequested) error {
    // Round-6 §6.3.0: the subscriber maintains pipeline_stage ONLY
    // (Regime B). is_ready is writer-inline and never touched here.
    // Dispatch over the four documented Reason kinds — direction is
    // forward (along outgoing 'blocks' edges) in all four branches;
    // only the semantic justification differs per kind.
    switch msg.Reason {
    case "close":
        // Triggered by workitems.Close (Tool 6) post-commit. The closed
        // item's neighbours have already had is_ready flipped inline;
        // walk the forward closure to propagate pipeline_stage per
        // §5.7.1 (downstream items may now move forward stages).
    case "edge_added":
        // Triggered by deps.AddEdge (Tool 11 / §6.5) post-commit. The
        // direct to_item's is_ready has already been recomputed inline;
        // walk the forward closure from to_item — a new upstream
        // blocker can push downstream stages backward per §5.7.1.
    case "edge_removed":
        // Triggered by deps.RemoveEdge (Tool 12) post-commit, with the
        // event_id REUSED from the inline audit row (tension #1). The
        // ON CONFLICT clause on the audit INSERT below collapses the
        // second insert to no-op; the pipeline_stage recompute pass
        // still runs. is_ready was recomputed inline for the direct
        // to_item; walk the forward closure to propagate pipeline_stage.
    case "state_change":
        // Triggered by workitems.SetStateColumns (Tool 13) post-commit
        // when (impl_state, review_state, qa_state) changed materially
        // per §5.7.1, OR by workitems.Claim post-commit ONLY on the
        // I-3 reset path (tension #2 narrow rule). Walk the forward
        // closure; pipeline_stage may transition on downstream items.
    default:
        // Unknown Reason — log + drop (defensive; the publisher set
        // is closed, but a malformed redelivery should not crash the
        // subscriber).
        return nil
    }
    // Shared body across all four Reasons:
    // 1. BFS from msg.TriggeredByItemID forward along 'blocks' edges,
    //    collecting items where pipeline_stage might change. Max depth
    //    256 per AR-8.
    // 2. For each affected item, recompute pipeline_stage per §5.7.1
    //    derivation; UPDATE workitems.items SET pipeline_stage = $new
    //    WHERE id = $id AND pipeline_stage <> $new (idempotent).
    //    The subscriber MUST NOT write is_ready (Regime A invariant
    //    — see §11.3 linter rule).
    // 3. INSERT INTO deps.cascade_events (id, event_id, kind, org_id,
    //    project_id, triggered_by_item_id, affected_item_ids,
    //    cascaded_count, trace_id, ...)
    //    VALUES (..., msg.Reason, ..., msg.TraceID, ...)
    //    ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING.
    //    `kind = msg.Reason` (one of 'close','edge_added',
    //    'edge_removed','state_change' — see §9.4.4 CHECK). The
    //    ON CONFLICT clause is the AR-11 idempotency mechanism (C1)
    //    AND the tension #1 mechanism for edge_removed (the inline
    //    audit row already exists with the same event_id).
    // 4. Publish CascadeCompleted with the affected set (best-effort;
    //    the subscriber's commit is the source of truth).
    return nil
}
```

`affected_item_ids` cardinality bound: in P01 the cascade walks the
forward 'blocks' closure of the triggered item (max depth 256 per AR-8).
The `(event_id, triggered_by_item_id)` UNIQUE constraint guarantees a
duplicate delivery is a no-op insert.

**`cascade_events.kind` enum (round-6).** SPEC §9.4.4 declares the
column with `CHECK (kind IN ('close','edge_added','edge_removed','state_change'))`:

- `'close'` — Tool 6 publish; subscriber writes the audit row during
  its `pipeline_stage` recompute pass.
- `'edge_added'` — Tool 11 / §6.5 publish; subscriber writes the audit
  row during its `pipeline_stage` recompute pass.
- `'edge_removed'` — Tool 12 writes the audit row INLINE in the same
  transaction as `DELETE FROM deps.dependencies`; Tool 12 also
  publishes post-commit with the SAME `event_id`, and the subscriber's
  re-insert collapses to no-op via the ON CONFLICT clause (tension #1).
  The subscriber's `pipeline_stage` recompute pass still runs.
- `'state_change'` — Tool 13 publish on §5.7.1-affecting writes; AND
  `workitems.Claim` publish on the I-3 reset path only (tensions #2
  and #3 narrow rules). Subscriber writes the audit row during its
  `pipeline_stage` recompute pass.

P01 ships all four kinds (round-6 cascade-symmetry). Subsequent phases
that introduce new cascade kinds extend the enum in their own phase
migration — adding a value is an additive CHECK rewrite.

### 6.4 Atomic claim transaction (Manifesto Law 5)

Verbatim from SPEC §5.5:

```sql
BEGIN;
  SELECT id FROM workitems.items
   WHERE id = $1 AND status = 'Ready' AND claimed_by_id IS NULL
   FOR UPDATE;
  -- if zero rows: rollback + return ErrAlreadyClaimed with winner info
  UPDATE workitems.items
     SET claimed_by_id   = $2,
         claimed_by_agent = $3,
         claimed_at      = now(),
         status          = 'InProgress'
   WHERE id = $1;
COMMIT;
```

On the loser path (zero rows from `SELECT FOR UPDATE`):

```sql
SELECT claimed_by_id, claimed_by_agent, claimed_at
  FROM workitems.items
 WHERE id = $1;
```

…and return error `ALREADY_CLAIMED` with `data.winner_user_id`,
`data.winner_agent`, `data.claimed_at`.

Pool-mode safety (R-P01-6 closure): the entire critical section lives in
one transaction, so PgBouncer transaction-mode and session-mode both
preserve the lock. The spec **does not pin** Encore Cloud's pool mode —
both modes work.

**Side-effects on I-3 path (round-6 §6.3.0 — tension #2 narrow rule).**
Normal `Claim` (Ready → InProgress with no I-3 reset) does NOT publish
any `CascadeRequested` — the claimed item was non-Done before the
claim and remains non-Done; downstream `pipeline_stage` is unaffected
per §5.7.1, and a publish would burn one cascade pass against the
NFR-1 budget for no observable effect.

`Claim` publishes `CascadeRequested{Reason:"state_change",
TriggeredByItemID: item_id, …}` **only when the I-3 reset path fires**
— that is, the locked row carried `qa_state='failed'` at the start of
the transaction, and the transaction therefore writes
`(review_state, qa_state) = ('pending', 'pending')` atomically with
the claim. In that case the state-column write materially affects
§5.7.1 derivation and the multi-hop `pipeline_stage` recompute is
required (Regime B). The publish happens after the transaction
commits; the subscriber writes one `deps.cascade_events` row with
`kind='state_change'`.

### 6.5 Cycle detection at write time (NFR-5)

Verbatim from SPEC §9.4.9, applied to every `add_dependency` call and to
every `dependencies[]` entry inside `create`:

```sql
BEGIN;
  -- AF5: per-project advisory lock serialises concurrent edge writes
  SELECT pg_advisory_xact_lock(hashtext('deps.add_dependency:' || $project_id));

  -- C5: depth-counter recursive CTE (LIMIT inside recursive term is
  -- undocumented PG behaviour; depth counter is the standard pattern)
  WITH RECURSIVE reachable(id, depth) AS (
      SELECT $2::text, 0
      UNION ALL
      SELECT d.to_item, r.depth + 1
        FROM deps.dependencies d
        JOIN reachable r ON d.from_item = r.id
       WHERE d.kind = 'blocks'
         AND r.depth < 256
  )
  SELECT 1 FROM reachable WHERE id = $1 LIMIT 1;
  -- If a row is returned: cycle would be created; rollback + reject with
  -- CYCLE_DETECTED. Optionally INSERT INTO deps.cycles for forensics.

  INSERT INTO deps.dependencies (id, from_item, to_item, kind, ...)
  VALUES ($edge_id, $1, $2, $kind, ...);

  -- Re-evaluate readiness of the newly-blocked to_item (it may now be
  -- non-ready if the from_item is not Done):
  UPDATE workitems.items
     SET is_ready = (
       NOT EXISTS (
         SELECT 1 FROM deps.dependencies d2
           JOIN workitems.items i ON i.id = d2.from_item
          WHERE d2.to_item = $2 AND d2.kind = 'blocks' AND i.status <> 'Done'
       )
     )
   WHERE id = $2;

COMMIT;

-- Round-6 §6.3.0: post-commit publish drives the multi-hop
-- pipeline_stage recompute on the forward closure. The inline UPDATE
-- above is single-hop (is_ready on to_item only — Regime A).
deps.CascadeRequestedTopic.Publish(ctx, &deps.CascadeRequested{
    EventID:           ulid.New(),
    OrgID:             $org_id,
    ProjectID:         $project_id,
    TriggeredByItemID: $to_item_id,
    Reason:            "edge_added",
    TraceID:           tracectx.From(ctx),
    EmittedAt:         time.Now(),
})
```

The 256 cap is a v1.0 product constraint (RP01-3 risk in plan §7); error
envelope on overflow includes the offending chain prefix.

---

## 7. Error Envelope (locked)

All MCP tool errors return a JSON-RPC 2.0 error object:

```jsonc
{
  "jsonrpc": "2.0",
  "id": "<echo>",
  "error": {
    "code": -32000,             // JSON-RPC reserved range; we always use -32000 for "tool error"
    "message": "<one-line human-readable>",
    "data": {
      "kind": "<MACHINE_CODE>",   // see table below
      "tool": "claim",
      "trace_id": "<ULID>",
      "details": { /* kind-specific */ }
    }
  }
}
```

| `kind` | Meaning | `details` shape |
|---|---|---|
| `UNAUTHENTICATED` | Bearer missing / invalid / revoked / expired | `{}` |
| `FORBIDDEN` | Authenticated, but `org.Authorize` denies | `{ "resource": "...", "action": "..." }` |
| `NOT_FOUND` | Subject id does not exist or not visible to caller | `{ "kind": "item", "id": "..." }` |
| `VALIDATION` | Argument shape / type / range violation | `{ "field": "title", "reason": "must be 1..200 chars" }` |
| `ALREADY_CLAIMED` | `claim` loser path | `{ "winner_user_id": "...", "winner_agent": "...", "claimed_at": "..." }` |
| `CYCLE_DETECTED` | `add_dependency` / `create` cycle reject | `{ "from": "...", "to": "...", "cycle_path": ["...", "..."] }` |
| `PRECONDITION_NOT_MET` | Structural precondition violated (P01) or BLOCK condition (P02+) | `{ "missing": "claimed_by_id" }` or `{ "rejection_reason": "..." }` |
| `CONFLICT` | Optimistic concurrency or unique constraint violation | `{ "constraint": "<name>" }` |
| `INTERNAL` | Unhandled server error (logged with full trace_id) | `{}` |

`trace_id` is the ULID minted by the `mcp` raw endpoint at request
entry (§10.2 Option B). It is stored verbatim in
`mcp.tool_calls.trace_id`, embedded in the Pub/Sub payload
`CascadeRequested.TraceID` (Encore Pub/Sub does not carry
`context.Context` across the topic boundary — the publisher copies the
id into the message explicitly), and re-emitted as the
`trace_id` structured field on every JSON-Lines log line for
correlation. Encore's runtime trace id is observability-only and is
not surfaced here.

---

## 8. Observability and Audit

### 8.1 `mcp.tool_calls` (ships in `0070_mcp.up.sql`)

Every MCP tool dispatch writes one row at request end:

```go
func recordToolCall(ctx context.Context, call ToolCall) {
    db.Exec(ctx, `
      INSERT INTO mcp.tool_calls
        (id, api_key_id, org_id, project_id, item_id, tool_name,
         arguments, result_kind, rejection_reason, error_code,
         duration_ms, trace_id, called_at)
      VALUES (...)`, ...)
}
```

`result_kind` ∈ `{ok, rejected, error}`; `rejection_reason` populated on
PRECONDITION_NOT_MET (canonically named for analysis). `trace_id` is the
ULID minted by `MCPHandler` at request entry (§10.2 Option B), pulled
from `ctx` by `recordToolCall`. The `mcp.tool_calls.trace_id` DDL column
is `text` (frozen in `0070_mcp.up.sql`) and accepts the ULID string
verbatim — no schema change required.

### 8.2 Logging (NFR-12)

`encore.dev/rlog` emits JSON Lines on STDERR. STDOUT is reserved for MCP
JSON-RPC payloads only (per Manifesto / NFR-12). Mixing is a quality-gate
failure.

Required structured fields per log line:
- `trace_id` — ULID minted by the `mcp` raw endpoint at request
  entry and bound on `context.Context` via `rlog.With` (§10.2
  Option B). This is the audit/business correlation id and is the
  same value persisted on `mcp.tool_calls.trace_id`,
  `deps.cascade_events.trace_id`, and emitted in the §7 error
  envelope.
- `org_id`, `project_id`, `user_id`, `agent_kind` — when known
- `tool` — tool name on MCP-path logs
- `service` — Encore service name

Encore's runtime trace id (`req.Trace.TraceID`) is **not** emitted
on application log lines; Encore Cloud's observability stack records
it separately at the infrastructure layer.

### 8.3 `deps.cascade_events` (ships in `0050_deps.up.sql`)

Every successful cascade subscriber pass writes one row (idempotent on
`(event_id, triggered_by_item_id)` per AR-11). Drives PRD M-5
(cascade-events-per-day metric) without touching observability stack
retention windows.

**Round-6 (cascade-symmetry).** The `kind` column now carries all four
values — `'close'`, `'edge_added'`, `'edge_removed'`, `'state_change'`
— per §6.3.0. `'edge_removed'` rows are written INLINE by Tool 12 in
the same transaction as the `DELETE` (the subscriber's later attempted
INSERT collapses to no-op via the `ON CONFLICT (event_id,
triggered_by_item_id) DO NOTHING` clause, since the publish reuses the
inline event_id — tension #1). The other three kinds are written by
the subscriber during its `pipeline_stage` recompute pass.

---

## 9. Seeder CLI (`apps/api/cmd/unblock-seed/`)

Per Plan §6 Q3 resolution: a one-shot Go CLI that bootstraps the
exit-criterion fixture. Owned by Greta.

### 9.1 Surface

```
unblock-seed [--config <path>] [--exit-criterion-fixture] [--issue-key]
```

| Flag | Purpose |
|---|---|
| `--config <path>` | YAML fixture file describing org/projects/users/items/edges to seed |
| `--exit-criterion-fixture` | Loads the canonical `apps/api/cmd/unblock-seed/fixtures/exit-criterion.yaml` (a 5-item dependency graph used by the E2E test) |
| `--issue-key <args>` | Issues an API key (calls `auth.IssueAPIKey` private RPC); prints the raw key to STDOUT once, never persists it |

### 9.2 Fixture schema (`exit-criterion.yaml`)

```yaml
organizations:
  - id: org_exit_criterion
    slug: exit-criterion
    name: P01 Exit Criterion
projects:
  - id: prj_exit
    org_id: org_exit_criterion
    slug: default
    name: Default
users:
  - id: usr_alice
    primary_provider: github
    primary_provider_id: "1"
    email: alice@example.com
    display_name: Alice
api_keys:
  - issued_to_user: usr_alice
    org_id: org_exit_criterion
    label: alice-claude-code
    agent_kind: claude-code
items:
  - id: itm_a
    project_id: prj_exit
    type: task
    title: Bootstrap (already done)
    status: Done
    impl_state: done
    review_state: approved
    qa_state: passed
    closed_at: now
  - id: itm_b
    project_id: prj_exit
    type: task
    title: Implement core (ready)
    status: Ready
    is_ready: true
  - id: itm_c
    project_id: prj_exit
    type: task
    title: Depends on B
  - id: itm_d
    project_id: prj_exit
    type: task
    title: Depends on B
  - id: itm_e
    project_id: prj_exit
    type: task
    title: Cycle attempt target
    status: Ready
    is_ready: true
dependencies:
  - from: itm_a
    to: itm_b
    kind: blocks
  - from: itm_b
    to: itm_c
    kind: blocks
  - from: itm_b
    to: itm_d
    kind: blocks
  - from: itm_d
    to: itm_e
    kind: blocks    # added so itm_e → itm_a (cycle attempt) closes the chain
                    # itm_a → itm_b → itm_d → itm_e → itm_a (review L10-C1)
```

### 9.3 Behaviour

The seeder operates by calling the documented private RPCs (it does NOT
write to Postgres directly). This guarantees the seed exercises the same
RBAC and validation paths as production, and forces the seeder to evolve
in lockstep with the API.

For `--issue-key`, the raw key is printed once to STDOUT in the form:

```
KEY_ID=01HQ...
KEY_PREFIX=abc123de
RAW_KEY=unblock_pat_abc123de4f5g6h7i8j9k...
```

The raw key is never persisted to disk. Operators capture STDOUT, paste
into their agent's secret store, and discard.

---

## 10. Cross-cutting Machinery

### 10.1 RBAC (`pkg/rbac`, NFR-2)

Located at `apps/api/shared/rbac/` (called `pkg/rbac` colloquially per
plan; the actual import path is `encore.app/shared/rbac`). Exposes:

```go
package rbac

// ScopedQuery is a typed query builder that refuses to compile a query
// against an org/project-scoped table without an explicit scope filter.
type ScopedQuery[T any] struct{ /* internal */ }

func For[T any](identity auth.Identity, table string) *ScopedQuery[T]

// Where appends a WHERE clause. The scope filter is automatically added
// to the WHERE chain — it is not optional, not bypassable.
func (q *ScopedQuery[T]) Where(clause string, args ...any) *ScopedQuery[T]

// Run executes; returns an error if the executing service does not own
// the schema (compile-time check via Encore's per-service DB binding;
// runtime check via the typed query builder enforcing the scope filter).
func (q *ScopedQuery[T]) Run(ctx context.Context) ([]T, error)
```

**Mechanism — typed query builder, not Encore middleware (review
L11-W5).** Plan §2.3 mentions "per-service `//encore:middleware` for
tenant filtering" as a candidate. P01 ships **the typed query builder
above** instead, for three reasons:
- Compile-time safety: an attempt to query an org-scoped table without
  going through `rbac.For[T]` is a code-review smell that the linter at
  §11.3 explicitly catches (no direct `db.Query("SELECT ... FROM
  workitems.items")` anywhere outside `pkg/rbac`).
- Encore middleware can intercept request lifecycle but cannot rewrite
  SQL — middleware would need to push a context-bound filter that the
  data layer reads, which adds an indirect layer that the typed builder
  collapses.
- Single canonical helper means the RBAC regression suite has one
  surface to fuzz, not two (middleware AND query path).

Encore middleware (`//encore:middleware`) IS used elsewhere — auth
handler context propagation, request/response logging, panic recovery —
but tenant filtering specifically uses the typed-query-builder
mechanism. Plan §2.3's wording is non-normative on this; spec §10.1 is
the authoritative pin.

The exhaustive RBAC regression suite under `apps/api/shared/rbactest/`
(NFR-2) fires one test per (caller-org, target-org, caller-role, table,
action) combination across the schemas P01 exposes. The role axis is
first-class — `org.Authorize` branches on `Identity.Role` ({owner,
admin, member, viewer, agent}; the synthetic `agent` is the API-key
runtime role per §4.3.2 step 8) before the role-action matrix, so a
matrix that omits role fails to exercise the actual policy. CI gates
release on zero cross-tenant leaks.

**Suite scope is split across three phase tasks, not delivered in one
bead:**

- **B-3 (this bead, `unblock-tv8.9`)** — auth + org schemas only.
  Concretely: `auth.users`, `auth.oauth_tokens`, `auth.sessions`,
  `org.organizations`, `org.projects`, `org.members`,
  `org.project_members`. Lays down the seed/cleanup scaffolding,
  the matrix-axis vocabulary, and the `t.Run`-per-tuple driver that
  the two follow-up tasks extend. The scope here is the surface the
  two live P01 services (auth, org) actually touch; everything beyond
  is reserved for the tasks below so each bead has independent
  acceptance criteria.
- **C-6 (`unblock-tv8.15`)** — extends the matrix to `workitems.items`,
  `workitems.comments`, `workitems.trail`, `deps.dependencies`. Lands
  after the C-group RPCs that introduce those tables.
- **E-3 (`unblock-tv8.25`)** — final P01 gate. Closes the remaining
  `mcp.tool_calls`, `mcp.api_keys`, `memory.entries`, `boards.boards`
  surfaces and is the release-blocking exhaustive sweep across every
  P01-exposed schema.

**Mechanism within each task.** Tables with an `org_id` column are
driven through `rbac.For[T]` (the read-side scope predicate is what's
under test). Tables without an `org_id` column —
`auth.users`, `auth.oauth_tokens`, `auth.sessions`,
`org.organizations`, `org.project_members` — are driven through
`org.Authorize` directly (their cross-tenant gate is the Authorize
predicate, not a `WHERE … org_id = $1` filter). The suite seeds two
orgs with a full role complement per side, runs without `t.Parallel`
(rbac.Bind is not goroutine-safe; bead `unblock-tv8.34` tracks the
hardening), and is discoverable as `encore test
./apps/api/shared/rbactest/...` so the A-6 CI bead (`unblock-tv8.6`)
can wire the gate without touching the suite.

### 10.2 Tracing (NFR-12)

**Decision (round-5, 2026-05-12 — closes contradiction blocking
`unblock-tv8.5`): Option B — single ULID minted at MCP entry, propagated
via `context.Context` only.**

Rationale: keeps a single id format across the public surface
(matches the §7 error-envelope `trace_id: "<ULID>"` contract,
`mcp.tool_calls.trace_id`, and `CascadeRequested.TraceID`), preserves
ULID consistency with workitem identifiers, avoids leaking
Encore-runtime-format strings into stored audit data, and requires
zero custom inter-service header plumbing because Encore's generated
client carries `context.Context` across private RPCs automatically.
Encore's own `req.Trace.TraceID` (per `encore.CurrentRequest()`)
remains available for runtime observability only and is **not**
persisted.

Contract:

- The `mcp` raw endpoint (`MCPHandler`, §5.3) mints a ULID
  `trace_id` at request entry, before dispatching any tool. This is
  the canonical audit/business correlation id.
- The id is attached to the request `context.Context` (`rlog.With`
  binds it as the `trace_id` structured field on every log line for
  the remainder of the request).
- Cross-service propagation rides the standard Encore generated
  client: handlers call e.g. `workitems.Claim(ctx, params)` and
  the framework carries `ctx` (including the bound `trace_id`)
  into the callee. **No `X-Unblock-Trace-Id` header is set or
  required** — that mechanism is not part of the supported Encore
  surface and is explicitly removed from this spec.
- The id is written to `mcp.tool_calls.trace_id` at request end
  (§8.1; column type is `text`, accepts the ULID string verbatim).
- The id is copied into `CascadeRequested.TraceID` (§4.5 / §6.3.1)
  at publish time so Pub/Sub subscribers can persist it on
  `deps.cascade_events.trace_id` (§8.3) — Encore Pub/Sub does not
  surface context across the topic boundary, so the publisher
  explicitly embeds it in the payload (the same pattern used for
  `EventID` per C1 closure).
- The id is echoed in the JSON-RPC error envelope `data.trace_id`
  (§7).

Encore's runtime trace id (`req.Trace.TraceID`) is **not** stored in
any persisted column or pushed onto the public surface; it is left
to the Encore Cloud observability dashboard as the
infrastructure-level correlation id and is independent of
`trace_id` above.

### 10.3 Catalogue authoring (Plan §2.3 / Q4)

`apps/api/mcp/catalogue.json` is **created in P01** with the 14 P01 tools'
tool definitions, but the `block_conditions[]` arrays are **empty
placeholders** for every transition:

```jsonc
{
  "schema_version": "v0.1",
  "tools": [
    {
      "name": "prime",
      "description": "...",
      "input_schema": { /* JSON Schema */ },
      "output_schema": { /* JSON Schema */ }
    },
    /* ... 13 more ... */
  ],
  "transitions": []  // empty; populated in P02
}
```

`go generate` is wired and emits `apps/api/mcp/catalogue.gen.go`
containing the embedded JSON as a `[]byte` constant + helper getters. CI
fails if `go generate` produces a diff against the committed file.

The catalogue-drift CI workflow (`infra/github/workflows/catalogue-drift.yml`)
is **scaffolded** in P01 but is a no-op (the `unblock-plugin` consumer
does not exist until P04). It activates as load-bearing in P04.

`mcp.meta_catalogue` MCP tool itself is **not** exposed in P01 — it ships
in P02 once the BLOCK conditions are authored.

### 10.4 Security boundary / threat model (D-1 transport addendum)

The public `POST /mcp` + `GET /mcp` surface (§5) is exposed only on
Encore Cloud's public hostname (`api.unblock.websublime.com` per
`apps/api/README.md` and the project CLAUDE.md). The Bearer API-key hot
path (§4.3.2) is the canonical authentication gate; every successful
request resolves to a tenant-scoped `Identity` before any tool body
runs, and every method other than `POST` / `GET` returns `405 Allow:
POST, GET` *before* auth fires (§5.1, AC #3).

The modelcontextprotocol/go-sdk Streamable HTTP handler exposes a
defense-in-depth setting `StreamableHTTPOptions.DisableLocalhostProtection`
that, when left at its default (`false`), refuses requests whose
`Host` header is not loopback while the listener is bound to
loopback — a DNS-rebinding mitigation aimed at single-binary
desktop / dev deployments. P01 sets it to `true` (apps/api/mcp/transport.go).
Rationale:

- Encore Cloud's public hostname is the same as the listening interface
  in production; the SDK's loopback check would refuse every legitimate
  request and is therefore inapplicable to the deployment target.
- The Bearer hot path (§4.3.2) + Encore Cloud's TLS-terminating edge
  proxy are the real security boundary; an unauthenticated cross-origin
  caller cannot reach a tool body regardless of `Host` header value.
- The acceptance test suite (`apps/api/shared/mcpaudittest/`) runs
  against `httptest.NewServer`, which emits `Host: 127.0.0.1:<port>`
  in some cases and other host strings in others; the SDK's localhost
  guard would refuse the very tests that prove the Bearer gate works.
- The trade-off is explicit: P01 does **not** support running the
  Encore binary on a developer laptop as a localhost-only MCP server
  for un-trusted local web pages to call. That deployment shape is out
  of scope; the `unblock-code` Rust binary serves that local-dev role
  via its own stdio MCP transport (Plan §1.2).

If a future phase introduces a localhost-binding deployment shape
(e.g. an on-prem appliance), the setting must be re-evaluated and the
threat model patched accordingly.

---

## 11. Acceptance Criteria

### 11.1 Functional acceptance (PRD §8 P01 exit criterion)

The end-to-end harness in `apps/api/exitcriteriontest/` runs against the
seeded fixture and asserts:

- [ ] `auth_handler` accepts a `Bearer <api-key>` from `unblock-seed --issue-key` and resolves to the correct `Identity`.
- [ ] `prime` returns a non-empty `ready_summary` (the seeder placed `itm_b` and `itm_e` in ready state) and an empty `claimed_by_me`.
- [ ] `ready --limit 1` returns one item, deterministically.
- [ ] `claim` on the returned item succeeds; a second concurrent `claim` from a different agent receives `{ "kind": "ALREADY_CLAIMED", ... }`.
- [ ] `set_state(impl_state=done)` on the claimed item is accepted (structural invariant only — `claimed_by_id` is set).
- [ ] `close` on the same item succeeds (P01 relaxation: `claimed_by_id IS NOT NULL` is the only precondition); cascade subscriber fires.
- [ ] After cascade, `prime` reflects newly unblocked dependents (`itm_c`, `itm_d` flip to ready).
- [ ] `add_dependency(from=itm_e, to=itm_a)` is rejected with `CYCLE_DETECTED` (would form `itm_a → itm_b → … → itm_e → itm_a`; the seeder includes such an edge configuration).
- [ ] `deps.cascade_events` has one row per fired cascade with a populated `event_id` and the affected set; `kind='close'` for the cascade triggered by Tool 6 above.
- [ ] **Cascade-symmetry kinds (round-6 §6.3.0).** The exit-criterion
  harness exercises each of the four `kind` values and asserts exactly
  one `deps.cascade_events` row materialises per logical trigger:
  - `'close'` — from the Tool 6 close above.
  - `'edge_added'` — issue `add_dependency(from=itm_c, to=itm_d)` after
    setup; assert a row with `kind='edge_added'` and
    `triggered_by_item_id=itm_d`.
  - `'edge_removed'` — issue `remove_dependency` on the edge above and
    assert a single row with `kind='edge_removed'` (tension #1: the
    inline INSERT and the post-commit subscriber re-insert collapse
    via the reused `event_id` + `ON CONFLICT` clause).
  - `'state_change'` — issue `set_state(qa_state=failed)` on an item
    with `review_state='approved'`, then `claim` it (different agent);
    the Claim fires the I-3 reset path and publishes
    `state_change`. Assert a row with `kind='state_change'` and
    `triggered_by_item_id` = the claimed item id.
- [ ] **Milestones (round-2 D1).** The seeder calls `workitems.CreateMilestone`
  twice — once for a parent (depth=1) and once for a child whose
  `parent_milestone_id` references the parent (depth=2) — then calls
  `workitems.AssignItem(itm_b, child_milestone_id)`; `MilestoneTree` returns
  the parent with the child nested, and `workitems.Get(itm_b)` returns the
  expected `MilestoneID`. M-INV-7 is exercised: assigning an item to a
  milestone whose `project_id` differs from the item's `project_id` is
  rejected with `kind=PRECONDITION_NOT_MET, data.invariant="M-INV-7"`.
- [ ] **State-machine invariants (round-2 D2 — five property tests).**
  - I-1: `set_state(review_state=needs_rework)` on an item with
    `qa_state='passed'` flips `qa_state='pending'` in the same write.
  - I-2: `set_state(qa_state=failed)` on an item with `review_state <>
    'approved'` is rejected with `data.invariant="qa_failed_requires_review_approved"`.
  - I-3: After `set_state(qa_state=failed)`, the next `claim` resets both
    `review_state='pending'` and `qa_state='pending'` atomically (verified
    via `get_state` immediately post-claim).
  - I-4: `set_state(review_state=approved)` on an item with
    `impl_state='pending'` is rejected with
    `data.invariant="review_change_requires_impl_done"`.
  - I-5: `set_state(impl_state=pending)` on an item with `impl_state='done'`
    AND no rework path active is rejected with
    `data.invariant="impl_done_to_pending_requires_rework_path"`. The
    same call when `review_state='needs_rework'` succeeds.

### 11.2 Non-functional acceptance

- [ ] **NFR-1 — Latency.** `prime → ready → claim` p99 < 2 s on the
  warm-cache harness (`apps/api/perftest/`). **Measurement methodology
  (C4 closure):** harness runs against the **local Encore emulator**;
  warm cache means (a) Postgres connection pool established, (b) API key
  validated once before the timer starts, (c) no first-request cold-start
  outliers. Cloud measurement is a P02 ops item.
- [ ] **NFR-2 — RBAC.** `apps/api/shared/rbactest/` green; zero
  cross-tenant leaks across every P01 read and write surface.
- [ ] **NFR-5 — Cycle integrity.** Cycle creation is rejected at write
  time (write-time enforcement, not read-time detection). Property test
  N=100 random graph mutations: zero cycles ever materialise in the DB.
- [ ] **NFR-9 — Decoupled deliverables.** No Rust code under `crates/`
  ships with P01. `crates/` directory remains as in stage-1 (empty or
  placeholder Cargo.toml).
- [ ] **NFR-10 — Quality gates.** Greta gate set green:
  - `cd apps/api && go fmt ./...` produces zero diffs
  - `go vet ./...` clean
  - `golangci-lint run --max-warnings 0`
  - `go test ./... -race`
  - `encore check` clean
  - Encore-generated TypeScript client diff: zero (regenerate, compare to committed in `apps/web/src/lib/encore.gen.ts` if present in P05; in P01 the generated file ships at `apps/api/encore.gen.ts` as a build artifact).
- [ ] **NFR-12 — Logging (HTTP transport reframe).** P01 ships MCP over
  Streamable HTTP, not stdio — so the original NFR-12 phrasing ("STDOUT
  carries only MCP envelopes") is reframed for the HTTP context: (a) all
  service logs go to STDERR via `encore.dev/rlog` as JSON Lines; (b) MCP
  JSON-RPC envelopes travel exclusively via `http.ResponseWriter`, never
  via STDOUT; (c) acceptance check: harness asserts that no log line
  appears in any HTTP response body, and that STDERR is exclusively
  JSON-Lines (one log object per line, parseable). The "no mixing"
  invariant degenerates to "logs and protocol payloads use disjoint
  channels (STDERR for logs, ResponseWriter for envelopes; STDOUT
  unused)". Verified via integration test that captures both streams
  during the exit-criterion harness run.

### 11.3 Architectural invariants

- [ ] All eight Postgres schemas exist with the canonical SPEC §9.4 DDL
  after running migrations 0010..0090.
- [ ] **Single-writer invariants (round-6 §6.3.0 — fractured).** The
  materialised columns on `workitems.items` have distinct, asymmetric
  writers:
  - (a) **`pipeline_stage` single-writer = the cascade subscriber.**
    Integration test asserts no other code path UPDATEs
    `workitems.items.pipeline_stage`. The
    `apps/api/shared/lint/no_direct_is_ready_write.go` linter rule
    enforces this statically — its scope tightens (round-6) to
    `pipeline_stage` writes only; any UPDATE targeting
    `workitems.items.pipeline_stage` outside
    `apps/api/deps/cascade_subscriber.go` is rejected.
  - (b) **`is_ready` single-writer = the mutating call site
    (Regime A).** `is_ready` is recomputed inline by the call site
    that mutated the row/edge — the cascade subscriber MUST NOT write
    `is_ready`. Explicit allowlist of permitted writers:
    `workitems.Close` (Tool 6 — inline recompute on direct `blocks`
    neighbours), `deps.AddEdge` (Tool 11 / §6.5 — inline recompute on
    `to_item`), `deps.RemoveEdge` (Tool 12 — inline recompute on
    direct `to_item`), and the internal shared helper
    `deps.recomputeReady` (called by the above sites). Integration
    test asserts the cascade subscriber never UPDATEs `is_ready`; an
    additional static check in the linter rule asserts the allowlist
    above is exhaustive.
- [ ] `rbac.For` and `rbac.ScopedQuery.Where` are never called with
  runtime-constructed string arguments — every table identifier (For
  arg 2) AND every clause string (Where arg 1) MUST be a Go string
  literal or untyped string constant. Runtime values destined for
  Where flow through `args...` exclusively; the table identifier of
  For has no runtime channel (SQL identifiers cannot be bound).
  (Static analysis: `golangci-lint` custom linter rule under
  `apps/api/shared/lint/no_rbac_dynamic_clause.go` rejects any
  non-literal call site across the unblock backend for BOTH call
  shapes. Locked SPEC §10.1 surface is unchanged; the analyzer is a
  meta-guard. unblock-tv8.33, unblock-tv8.35.)
- [ ] `deps.cascade_events` insert is idempotent on re-delivery (property
  test: re-deliver every `CascadeRequested` event twice; assert post-state
  is byte-identical and exactly one row exists per `(event_id,
  triggered_by_item_id)`).
- [ ] Atomic claim is a single transaction with `SELECT FOR UPDATE`
  (property test: N=100 concurrent claim attempts on the same item;
  assert exactly one winner and N-1 `ALREADY_CLAIMED` errors).
- [ ] Cycle detection runs inside a transaction holding
  `pg_advisory_xact_lock(hashtext('deps.add_dependency:' || project_id))`
  (integration test: simulate two concurrent `add_dependency` calls that
  would form a cycle from different vantage points; assert at most one
  succeeds).
- [ ] Manifesto Laws covered in P01 (L1 cascade, L2 one graph, L3
  Postgres-truth, L5 atomic claim, L7 < 2s) are structurally present —
  each invariant is backed by at least one regression test.

### 11.4 Documentation

- [ ] `docs/specs/01-spec-backend-mvp.md` is **APPROVED** before
  implementation starts (this document).
- [ ] README.md updated with P01 user surface (MCP Bearer auth, the 14
  tools' one-liners, `unblock-seed` invocation).
- [ ] `apps/api/README.md` documents service decomposition and migration
  ownership (the dedicated `db`-service migration-owner pattern is
  non-obvious — every domain service is an equal consumer).

### 11.5 Open Question carry-overs

- **OQ1 (Copilot transport):** P01 acceptance harness uses Claude Code as
  the reference MCP client. Copilot transport coverage is P04 plugin
  renderer scope; if a P01 reviewer wants Copilot manual-tested, they may
  run it against the same `Bearer + Streamable HTTP` endpoint and report
  findings as a P02 input — it does not block P01 close.

---

## 12. Implementation Tasks (mapped to Plan §4)

This spec is the contract. The plan §4 task breakdown remains
authoritative for sequencing. Below maps each plan task to the spec
section that locks its contract:

| Plan task | Owner | Spec section(s) |
|---|---|---|
| A-1 (Encore app init) | Greta | §3.1 (migration owner), §4 (service skeletons) |
| A-2 (Bootstrap migration) | Greta | §3.2 (`0010_bootstrap.up.sql`), §3.5 (secrets) |
| A-3 (Migrations §9.4.1–§9.4.8) | Greta | §3.2 (migrations 0020..0090), §3.4 (FTS DDL) |
| A-4 (`pkg/rbac`) | Greta | §10.1 |
| A-5 (Tracing scaffold) | Greta | §10.2 |
| A-6 (CI gates) | Olive | §11.2 (NFR-10 commands) |
| B-1 (`auth` service) | Greta | §4.1, §4.3.2 (API key hot path), §4.3.3 |
| B-2 (`org` service) | Greta | §4.2 |
| B-3 (RBAC suite) | Greta | §10.1, §11.2 (NFR-2) |
| C-1 (`workitems` service) | Greta | §4.4, §4.4.1 (milestone RPCs — round-2 D1) |
| C-2 (`deps` service + cycle CTE) | Greta | §4.5, §6.5, §6.3.0 (post-commit publishes for `edge_added` and `edge_removed`) |
| C-3 (Cascade subsystem) | Greta | §6.3, §6.3.0 (four-Reason dispatch; `pipeline_stage`-only writer) |
| C-4 (Atomic claim) | Greta | §6.4 (incl. I-3-reset `state_change` publish per §6.3.0) |
| C-5 (`pipeline_stage` derivation tests) | Greta | §6.3.2, §6.3.0, SPEC §5.7.1 |
| C-6 (RBAC suite extensions) | Greta | §10.1 |
| D-1 (MCP transport skeleton) | Greta | §5, §4.3.1, §4.3.2 |
| D-2 (Tools 1–4) | Greta | §6.2 (tools 1–4) |
| D-3 (Tools 5–8) | Greta | §6.2 (tools 5–8) |
| D-4 (Tools 9–10) | Greta | §6.2 (tools 9–10), §3.4 (FTS), §6.2 #9 |
| D-5 (Tools 11–12) | Greta | §6.2 (tools 11–12), §6.5 |
| D-6 (Tools 13–14) | Greta | §6.2 (tools 13–14) |
| D-7 (Catalogue v0) | Greta | §10.3 |
| E-1 (Seeder CLI) | Greta | §9 |
| E-2 (NFR-1 latency harness) | Greta | §11.2 (warm-cache definition) |
| E-3 (NFR-2 RBAC suite) | Greta | §10.1, §11.2 |
| E-4 (Exit-criterion E2E test) | Greta | §11.1, §9 (fixture), §6.3 (cascade), §6.5 (cycle) |

---

## 13. Risks (P01 spec-level)

Plan §7 tracks phase risks; spec-level risks below reflect what could
break a contract pinned in this document.

| # | Risk | Mitigation |
|---|---|---|
| RS01-1 | Go MCP SDK API changes during P01 (the SDK is "in collaboration with Google" but new) | Pin exact version in `go.mod`; vendor-allowlist in CI (no auto-bump on `go.mod`); update path in P02 if SDK breaks. |
| RS01-2 | Encore adds new constraints on multi-schema use that break the dedicated-`db`-service migration-owner pattern | Smoke test in CI: `encore check` on every PR; if Encore's behaviour ever splits the migration runner, escalate to a `rebase` plan. |
| RS01-3 | The `Mcp-Session-Id` header semantics change between MCP spec revisions before P01 ships | Pin to MCP spec 2025-06-18 (current canonical); migration to a future spec is a P02+ task. |
| RS01-4 | The `subtle.ConstantTimeCompare` cost on the API-key hot path (§4.3.2) is unexpectedly high under Encore's request handler | Benchmark inline as part of E-2 latency harness; if budget pressure emerges, introduce a 1-minute in-process LRU cache keyed by `key_prefix` + `revoked_at,expires_at` snapshot (eviction on revoke event). |
| RS01-5 | Cascade subscriber re-delivery storm on a degenerate fixture | The `(event_id, triggered_by_item_id)` UNIQUE constraint absorbs duplicates structurally; a load test with N=10k re-deliveries asserts zero duplicate inserts, zero `is_ready` flips beyond the first one. |
| RS01-6 | `pg_advisory_xact_lock(hashtext(...))` collisions across unrelated projects | `hashtext` is a 32-bit hash; collision probability at v1 scale (≤ 100 projects per org) is negligible (~10⁻⁸). Documented as acceptable; revisit at v1.1 if scale breaks the assumption. Alternative if needed: switch to `pg_advisory_xact_lock(int4, int4)` two-key form using `(org_seq_id, project_seq_id)`. |
| RS01-7 (round-2 D1) | Milestone tree CTE depth violation under racing inserts — two concurrent `CreateMilestone` calls each see their own parent at depth=3, both attempt to insert at depth=4 (legal), but a third call could push one into depth=5. | The recursive ancestor walk runs inside the same transaction as the insert; the parent row is `SELECT ... FOR UPDATE` to serialise concurrent children. M-INV-6 enforcement is therefore strictly serial per parent. Cross-parent concurrency is fine because milestone trees are scope-bounded and the ancestor walk is parent-id-driven. AR-17 documents the cap. |
| RS01-8 (round-2 D2) | State-machine invariants implemented in app code (CTE) rather than DB CHECK constraints — drift risk if a future migration adds new state values without updating the invariant table. | `(impl_state, review_state, qa_state, pipeline_state)` enum CHECK constraints in `0040_workitems.up.sql` are declared `IMMUTABLE` (Postgres-natural) and named (`items_impl_state_chk`, …); any phase that adds a new value MUST update both the CHECK and the invariant CTE in lockstep, asserted by a documentation cross-link in the migration's commit message. The five property tests in §11.1 form the regression net. AR-18 covers the concurrency dimension. |

---

## 14. Approval Checklist

Before this spec moves from DRAFT to APPROVED, the user (orchestrator)
confirms:

- [ ] All seven research contradictions (C1, C2, C3, C5, C6, C7 + AF1/AF5)
  are honoured in the design above.
- [ ] The 14 MCP tool contracts are signature-locked and no field is
  ambiguous (every "optional" / "default" stated explicitly).
- [ ] Migration filenames and numbering are agreed.
- [ ] Error envelope kinds and `data` shapes cover every failure mode the
  exit-criterion harness exercises.
- [ ] The seeder CLI surface is sufficient for both ops and the E2E test.
- [ ] No simplification has been smuggled in — every plan §2 / §6 / §3
  resolution is preserved.

Post-approval, `/tasks` (Fernando) decomposes this spec into bd beads
under epic P01. Each bead's description references the spec section that
locks its contract; no bead is a self-sufficient document
(`feedback_bead_description_not_spec`).
