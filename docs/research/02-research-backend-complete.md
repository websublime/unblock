# Research: P02 — Backend Complete (providers + memory tools + Layer-1 pipeline enforcement)

**Source PRD:** [docs/PRD.md](../PRD.md) (APPROVED 2026-05-07)
**Source Plan:** [docs/plans/02-plan-backend-complete.md](../plans/02-plan-backend-complete.md) (APPROVED 2026-06-16)
**Source SPEC:** [docs/SPEC.md](../SPEC.md) (APPROVED 2026-05-07, round-6 2026-05-12)
**Date:** 2026-06-16
**Author:** Smith (feasibility research)

> Closes R-P02-1 … R-P02-13 (incl. siblings R-P02-3b, R-P02-4b) per plan §5.
> Gates the P02 spec. Each item is validated against real external
> documentation and the existing `apps/api/` P01 codebase. Findings that flip
> a load-bearing plan assumption are flagged in **Contradictions and Risks**.

---

## Executive summary

| Item | Topic | Status |
|---|---|---|
| R-P02-1 | GitHub webhook schema + X-Hub-Signature-256 HMAC | CONFIRMED |
| R-P02-2 | GitHub Issue ↔ canonical workitems field map | PARTIALLY CONFIRMED |
| R-P02-3 | Bidirectional sync loop prevention + rate limits | CONFIRMED |
| R-P02-3b | Go GitHub client library pin | CONFIRMED |
| R-P02-4 | GitHub App vs OAuth app for webhook-sub + write | CONFIRMED — **contradicts a P01 DDL assumption** (C1) |
| R-P02-4b | Webhook failure-response status contract | CONFIRMED |
| R-P02-5 | go-sdk structured-error for PIPELINE_PRECONDITION_NOT_MET | CONFIRMED |
| R-P02-6 | verify_can_transition hook-facing-not-listed primitive | CONTRADICTED — **no "hidden tool" in go-sdk** (C2) |
| R-P02-7 | Comment-trail read for the validator (get_state/GetTrail) | PARTIALLY CONFIRMED — **last_comment_* gap** (C3) |
| R-P02-8 | pgcrypto pgp_sym_encrypt / DEK via Encore secret | CONFIRMED |
| R-P02-9 | Sanitiser pattern set + sanitise-before-tokenise ordering | PARTIALLY CONFIRMED (no v1.0 pattern set exists yet) |
| R-P02-10 | forget soft-delete deleted_at additive column | CONFIRMED — genuine DDL gap, partial-index required (C4) |
| R-P02-11 | Encore Cron + Pub/Sub for reconciler/digest/sanitiser; sync sync vs async | CONFIRMED — async path viable |
| R-P02-12 | Encore Cloud free-tier ceilings + mcp-warmer | CONFIRMED — **free-tier cron is hourly-min** (C5) |
| R-P02-13 | memory.entries.expires_at read+write semantics | CONFIRMED — column inert today (decision needed) |

**Net:** 9 CONFIRMED, 3 PARTIALLY CONFIRMED, 1 CONTRADICTED. Five
contradictions/risks (C1–C5) must be resolved in the spec. None block the
phase; all are spec-shape decisions.

---

## Dependencies Investigated

### GitHub webhook delivery (REST + webhooks)
- **Documentation:** docs.github.com/webhooks (events-and-payloads,
  validating-webhook-deliveries, best-practices), apps authentication.
- **Capabilities:**
  - Headers on every delivery: `X-GitHub-Delivery` (GUID — the AR-12 dedup
    key), `X-GitHub-Event` (e.g. `issues`), `X-Hub-Signature-256`
    (`sha256=<hex>` HMAC-SHA256 of the **raw** body), `X-Hub-Signature`
    (legacy SHA-1 — ignore), `X-GitHub-Hook-ID`,
    `X-GitHub-Hook-Installation-Target-Type`,
    `X-GitHub-Hook-Installation-Target-ID`, `User-Agent: GitHub-Hookshot/*`.
  - The action lives in the JSON body's `action` field (NOT the header).
    `issues` actions: `opened`, `edited`, `closed`, `reopened`, `deleted`,
    `assigned`, `unassigned`, `labeled`, `unlabeled`, `milestoned`,
    `demilestoned`, `locked`, `unlocked`, `pinned`, `unpinned`,
    `transferred`, plus newer `typed`/`field_*`. `pull_request` actions:
    `opened`/`edited`/`closed`/`reopened`/`synchronize`/`assigned`/… .
  - GitHub-App deliveries carry a top-level `installation.id` (maps to our
    `providers.installations`) and a `sender.login` (the actor — the
    loop-prevention allowlist anchor, R-P02-3).
- **Limitations:**
  - **10-second response budget.** GitHub terminates the connection and
    marks the delivery a failure after 10 s — mandates ack-fast (Q3 async
    working assumption is correct, R-P02-11).
  - Payload size cap is 25 MB (GitHub-side); our oversized-payload class
    (R-P02-4b) is about *our* read limit, not GitHub's.
- **Evidence:** signature scheme test vector — secret `It's a Secret to
  Everybody`, body `Hello, World!` → `sha256=757107ea0eb2509fc211221cce98
  4b8a37570b6d7586c22c46f4379c8b043e17`. Constant-time compare mandated
  ("never use a plain `==`"; Go: `hmac.Equal`).

### GitHub REST/GraphQL rate limits
- **Documentation:** docs.github.com/rest rate-limits.
- **Capabilities/limits:** authenticated user / OAuth token = **5,000
  req/hr**; GitHub-App installation = **5,000/hr baseline**, scaling
  +50/hr per repo and per user above 20, ceiling **12,500/hr**
  (Enterprise 15,000). Secondary limits: ≤100 concurrent requests, REST
  900 points/min, content-creation 80/min & 500/hr, OAuth token requests
  2,000/hr. GraphQL: separate primary limit, 1 point per read query, 5 per
  mutation. Headers: `x-ratelimit-limit/-remaining/-used/-reset`,
  `retry-after` (seconds) on secondary-limit hits.
- **Evidence:** the GitHub-App path gives an **equal-or-higher** rate
  budget than OAuth (baseline parity, scales up) — reinforces R-P02-4's
  App working assumption on the rate-limit axis.

### `github.com/google/go-github` (REST client)
- **Documentation:** github.com/google/go-github + proxy.golang.org.
- **Capabilities:** current **v88.0.0** (2026-05-21), targets REST API
  `2022-11-28`. Services map 1:1 to API areas (`Issues.Create/Edit/
  ListByRepo`). Native rate-limit support: `*RateLimitError` (primary),
  `*AbuseRateLimitError{RetryAfter}` (secondary),
  `SleepUntilPrimaryRateLimitResetWhenRateLimited`. Pagination via
  `ListOptions`/`ListCursorOptions` + Go-1.23 iterators. Auth via
  `WithAuthToken()` (OAuth) or the **`bradleyfalzon/ghinstallation`**
  RoundTripper (GitHub-App installation tokens).
- **GraphQL:** go-github does not cover v4; the repo itself recommends
  **`shurcooL/githubv4`** for GraphQL.
- **Evidence:** `golang-jwt/jwt/v5 v5.3.1` is already in `apps/api/go.sum`
  (transitive) — the JWT primitive the App path needs is already vendored.

### `github.com/modelcontextprotocol/go-sdk` (MCP transport)
- **Documentation:** pkg.go.dev + the installed module cache
  (`go-sdk@v1.6.0`, pinned in `apps/api/go.mod`).
- **Capabilities (verified against installed source `mcp/server.go`):**
  `Server.AddTool` (always advertised in `tools/list`), `RemoveTools`,
  `AddResource`/`AddResourceTemplate`/`readResource` (MCP resources),
  `AddReceivingMiddleware` (intercept raw JSON-RPC by method name).
  `ServerOptions` has **no** tool-listing filter. Tool-handler errors:
  returning a `*jsonrpc.Error` → JSON-RPC error response; returning a
  plain error → `isError` tool-result frame.
- **Limitation (load-bearing):** there is **no built-in "registered but
  unlisted" tool** concept (C2). A tool added via `AddTool` is in
  `tools/list` for every session.
- **Evidence:** `apps/api/mcp/errmap.go` already returns `*sdkjsonrpc.Error{
  Code:-32000, Data:<§7 envelope>}` and the SDK forwards it verbatim — the
  exact mechanism R-P02-5 needs for `PIPELINE_PRECONDITION_NOT_MET`.

### Encore Cloud (Cron + Pub/Sub + free tier)
- **Documentation:** encore.dev/docs cron-jobs, platform usage/billing.
- **Capabilities:** `cron.NewJob(id, cron.JobConfig{Title, Every|Schedule,
  Endpoint})`; target endpoint must be parameter-free
  (`func(context.Context) error`) and idempotent; cron jobs **do not run
  locally or in Preview envs** — only in deployed Cloud envs. Pub/Sub
  proven in P01 (`deps.CascadeRequestedTopic`, `AtLeastOnce`).
- **Free-tier ceilings (Fair Use):** 100,000 requests/day; **100,000
  Pub/Sub messages/day**; 1 GB DB storage; 1 GB object storage; **cron
  minimum interval = once per hour**; max 2 cloud environments; no PR
  preview envs; single concurrent build; no guaranteed log/trace
  retention. Over-limit = Encore reaches out, no auto-termination.
- **Evidence:** `apps/api/deps/cascade.go` lines 165/174 already declare
  `pubsub.NewTopic[...](..., pubsub.TopicConfig{DeliveryGuarantee:
  pubsub.AtLeastOnce})`; `cascade_subscriber.go` proves the
  `ON CONFLICT DO NOTHING` + publisher-ULID idempotency (AR-11) pattern.

---

## Assumptions Validated (R-P02-1 … R-P02-13)

### R-P02-1 — GitHub webhook event schema + HMAC — **CONFIRMED**
- `X-Hub-Signature-256` = `sha256=` + hex(HMAC-SHA256(secret, raw_body)).
  Verify with Go stdlib `hmac.New(sha256.New, secret)` + `hmac.Equal`
  (constant-time) over the **unparsed** body bytes.
- `X-GitHub-Delivery` is the per-delivery GUID → the AR-12
  `events_delivery_uniq (provider, delivery_id)` key already in
  `0060_providers.up.sql`. Confirmed it is unique and stable across
  redeliveries (a manual redelivery reuses the same GUID — so the dedup
  constraint correctly suppresses replays).
- v1.0 subscription set: `issues` + `pull_request` (action discriminated in
  the body). `event_type` column should store `<event>.<action>` (e.g.
  `issues.opened`) — matches the `0060` comment `-- e.g. 'issues.opened'`.
- **Evidence:** validating-webhook-deliveries doc + the 0060 DDL comments.

### R-P02-2 — GitHub Issue ↔ canonical workitems field map — **PARTIALLY CONFIRMED**
- GitHub `issue` object fields that map cleanly: `number`→provider id /
  `provider_url`; `title`→`title`; `body`→`body`; `state`
  (`open`/`closed`)→canonical `Status` (closed→`Done`, open→`Backlog`/
  `Ready` per deps); `labels[]`→`workitems.labels` (name match);
  `assignees[]`/`assignee`; `milestone`; `locked`.
- **No clean counterpart (spec must decide degradation):**
  - GitHub has **no dependency graph** → our `deps` edges have no GitHub
    source; one-directional (we own the graph).
  - GitHub `state_reason` (`completed`/`not_planned`/`reopened`) has no
    direct canonical column.
  - Our three orthogonal pipeline columns (`impl/review/qa_state`) and
    `pipeline_state` have **no** GitHub equivalent — they are unblock-only
    and must NOT be inferred from issue state.
  - GitHub assignee = a GitHub login; our `claimed_by_id` = an
    `auth.users` ULID. Mapping requires an identity resolution step or is
    left unmapped at v1.0 (spec decision).
- **REST vs GraphQL:** REST `Issues` endpoints cover the full v1.0
  read+write field map; GraphQL is only needed for issue-level
  relationships we do not consume at v1.0. **Recommendation:** REST
  (go-github) for both normalise-read echo and outbound write; defer
  githubv4 unless a specific field forces it. Confirmed go-github v88
  exposes every field above.
- **Status PARTIALLY** because the exact column-by-column map + the
  unmapped-field disposition is a spec contract, not researchable to a
  single answer; research confirms field *availability* and the REST
  sufficiency.

### R-P02-3 — Bidirectional sync loop prevention — **CONFIRMED**
- Loop mechanism confirmed: a `://unblock`→GitHub write fires an echo
  webhook back. The payload carries `sender.login` (the App's bot identity
  on our own writes) and the App carries a known bot login.
- **Viable suppression strategies (all evidence-backed):**
  1. **Actor allowlist** — drop/no-op normalisation when `sender.login` ==
     our App's bot account (the App acts as `<app-name>[bot]`). Cleanest;
     GitHub-App path makes our writes attributable.
  2. **`last_synced_at` echo window** — on outbound write, stamp
     `providers.mappings.last_synced_at = now()`; an inbound webhook whose
     content matches and arrives within ε of `last_synced_at` is an echo.
     Columns already exist in `0060`.
  3. **Content-hash compare** — normalise then diff against current
     canonical state; a no-op diff suppresses the re-write (idempotent
     normaliser, defends regardless of actor).
  Spec should combine (1)+(3): actor allowlist as the fast path,
  content-idempotent normalise as the structural backstop.
- **Rate-limit safety:** reconciler honours `x-ratelimit-remaining`/`-reset`
  and `retry-after`; go-github surfaces these as typed errors. 5,000/hr
  REST budget is ample for opt-in-per-installation sync at v1 scale.
- **Evidence:** webhook payload `sender`/`installation` objects (confirmed);
  `mappings.last_synced_at`/`drift_detected_at` columns exist.

### R-P02-3b — Go GitHub client library — **CONFIRMED**
- **Pin: `github.com/google/go-github/vNN` (REST) + `bradleyfalzon/
  ghinstallation/v2` (App installation transport).** Add `shurcooL/
  githubv4` ONLY if R-P02-2 forces a GraphQL-only field (not expected at
  v1.0). Do NOT hand-roll `net/http` — go-github gives rate-limit header
  parsing, secondary-limit `RetryAfter`, and pagination for free (the exact
  surfaces D-1/D-2 need).
- Latest major is **v88.0.0**; spec pins the exact major. `golang-jwt/jwt/
  v5` (the App-JWT dep that ghinstallation needs) is **already** in
  `go.sum`.
- **Evidence:** go-github README (rate-limit + ghinstallation auth +
  GraphQL deferral); `apps/api/go.sum` already carries jwt/v5 v5.3.1.

### R-P02-4 — GitHub App vs OAuth app — **CONFIRMED (App), contradicts a DDL assumption → C1**
- **GitHub App wins** for the v1.0 webhook-subscription + issue-write job:
  - One **app-level** webhook URL + **one app-level webhook secret** fires
    for **all** installations; each delivery carries `installation.id`. No
    per-repo webhook wiring needed.
  - Server-to-server **installation access tokens** (JWT signed with the
    App private-key PEM + App ID → `POST /app/installations/{id}/access_
    tokens`, **1-hour** lifetime) let the App write issues with no user
    present — exactly the autonomous bidirectional-sync model.
  - Rate budget ≥ OAuth (R-P02-3 evidence).
- **Secret model the spec must adopt:** App ID + private-key PEM + the
  single app-level webhook secret. OAuth-app fallback would need
  client-id/secret/redirect-uri + per-user tokens and per-repo webhook
  subscription — strictly worse for autonomous sync.
- **Contradiction (C1):** the `0060_providers.up.sql` DDL models a
  **per-install** `webhook_secret_enc` column. Under the GitHub-App model
  the webhook secret is **app-level (one secret for all installs)**, not
  per-install. See C1.
- **Evidence:** using-webhooks-with-github-apps (single app-level URL +
  secret confirmed); about-authentication-with-a-github-app (installation
  tokens, server-to-server, no user).

### R-P02-4b — Webhook failure-response status contract — **CONFIRMED**
- GitHub marks any non-2xx (or >10 s) as a failed delivery; redelivery is
  **manual** (no automatic retry storm by default), but a misconfigured
  health-checker / GitHub's own internal retry on transient transport
  failures means the status classification still matters. Contract the spec
  must pin:
  - bad/absent HMAC signature → **4xx-final** (`401`); GitHub must not treat
    it as our transient fault. **No `providers.events` insert, no
    normalise.** (RP02-4 mitigation: HMAC verified before any processing.)
  - unknown/unregistered `installation.id` → **4xx-final** (`404`); not
    retryable.
  - malformed JSON / unparseable body → **4xx-final** (`400`).
  - oversized payload (above our read cap) → **4xx-final** (`413`).
  - recognised **duplicate** `X-GitHub-Delivery` → **`200`** (so GitHub
    stops; the AR-12 constraint already suppresses the double-create).
  - transient our-side failure (DB down on the ack-insert) → **5xx**
    (`503`) so a redelivery can succeed.
- **Evidence:** best-practices doc (2xx-within-10 s, async-recommended);
  `0060` `events_delivery_uniq` for the duplicate→200 path.

### R-P02-5 — go-sdk structured error for PIPELINE_PRECONDITION_NOT_MET — **CONFIRMED**
- The exit-criterion rejection is wire-legible TODAY via the existing
  `apps/api/mcp/errmap.go` path: a tool handler returns
  `*sdkjsonrpc.Error{Code:-32000, Message:<human>, Data:<§7 envelope>}` and
  the SDK forwards it verbatim as a JSON-RPC **error** (not an `isError`
  tool-result). Agent clients render JSON-RPC errors.
- The §7 envelope already carries `data.kind` (machine code),
  `data.trace_id`, `data.tool`, `data.details`. SPEC §7.5.1 adds
  `error_code="PIPELINE_PRECONDITION_NOT_MET"`, `error_message`,
  `rejection_reason`. **Spec decision:** map `error_code` onto a new
  `envelopeKind*` constant (`PIPELINE_PRECONDITION_NOT_MET`) distinct from
  the existing `PRECONDITION_NOT_MET` (which `set_state` I-1..I-5 already
  uses) — see C3/RP02-2. `rejection_reason` already flows into
  `mcp.tool_calls.rejection_reason` via `mapError` for the
  `envelopeKindPreconditionNotMet` branch.
- **Evidence:** `errmap.go` lines 40-104; `errenvelope.go` kind constants;
  `mcp.tool_calls.rejection_reason`/`error_code` columns in `0070`.

### R-P02-6 — verify_can_transition hook-facing-but-not-listed — **CONTRADICTED → C2**
- SPEC §5.2.2 says `verify_can_transition` and `meta_catalogue` are "**not**
  a separate top-level MCP tool … exposed via the same SSE channel" and do
  not count toward the 27. The **go-sdk v1.6.0 has no "registered but
  unlisted tool" concept** — `Server.AddTool` always advertises in
  `tools/list`; `ServerOptions` has no listing filter.
- **Three viable mechanisms the spec must choose among (C2):**
  1. **Custom JSON-RPC method via `AddReceivingMiddleware`** — register a
     non-`tools/call` method name (e.g. `unblock/verifyCanTransition`,
     `unblock/metaCatalogue`); intercept it in receiving middleware before
     the SDK's method dispatch. True "not in tools/list", matches the SPEC
     intent most literally. Cost: bypasses the typed `AddTool` plumbing
     (manual arg-decode + the §7 envelope path).
  2. **MCP Resource** (`AddResource`) — `meta_catalogue` fits a read-only
     resource (URI like `unblock://catalogue`) cleanly; it surfaces under
     `resources/list`, not `tools/list`. `verify_can_transition` is a
     parameterised call, less natural as a resource.
  3. **A normal `AddTool`** that simply *is* listed — pragmatic, but
     violates the "does not count toward the 27 / not advertised" SPEC
     promise and would need the §5.2.2 wording reconciled.
- **Recommendation to Ada:** `meta_catalogue` → MCP Resource (option 2);
  `verify_can_transition` → custom JSON-RPC method via middleware
  (option 1). Both keep the agent-facing `tools/list` at exactly 27. The
  "same SSE channel" SPEC phrasing predates the Streamable-HTTP transport
  (the channel is now `POST /mcp`); flag the stale wording.
- **Evidence:** installed `go-sdk@v1.6.0/mcp/server.go` — `AddTool`
  (always-listed), `RemoveTools`, `AddResource`, `AddReceivingMiddleware`;
  no listing-filter option.

### R-P02-7 — Comment-trail read for the validator — **PARTIALLY CONFIRMED → C3**
- `workitems.GetState` (P01, `workitems/workitems.go:1333`) returns
  `recent_kinds`: `SELECT DISTINCT ON (kind) kind, status, id, created_at
  FROM workitems.comments WHERE item_id=$1 ORDER BY kind ASC, created_at
  DESC` — i.e. the **most-recent (status,id,ts) per distinct kind**, one
  row per kind present on the item.
- **`any_comment_kind`/`any_comment_status` predicates (§7.5.1): FULLY
  covered.** "A `(kind=qa,status=success)` comment exists" = scan
  `recent_kinds` for the `qa` row and check its status is `success` —
  because DISTINCT ON returns the *latest* per kind, but for the
  any-of-kind existence predicate the latest is sufficient ONLY when the
  latest carries the required status. **Caveat:** if the latest `qa`
  comment is `error` but an earlier one was `success`, `recent_kinds`
  shows only the `error` — so a strict `any_comment(kind=qa,
  status=success)` "ever existed" predicate is **NOT** answerable from
  `recent_kinds` alone (it collapses history to one row per kind).
- **`last_comment_kind`/`last_comment_status` predicates: NOT directly
  covered.** `recent_kinds` is keyed/ordered by kind, not by global
  recency — it does not expose "the single most recent comment across all
  kinds". §7.5.1's `last_comment_*` needs `ORDER BY created_at DESC LIMIT
  1` over all comments.
- **Conclusion (C3):** the P01 `get_state`/`GetState` surface is
  **insufficient** for (a) the global `last_comment_*` predicate and (b) a
  history-aware `any_comment_*=success ever` predicate. P02 needs either a
  new `workitems` read RPC (e.g. `GetCommentTrailPredicates(item_id)`
  returning both the global-latest tuple and per-(kind,status) existence
  booleans) or an extension of `GetState`. The plan's §2.1 assumption
  ("confirm the P01 GetState/get_state RPCs expose everything the validator
  needs without a new private RPC") is **flagged**: a new/extended RPC is
  required.
- **Note:** PRD §6.7's actual preconditions are all phrased as
  `any_comment` existence (`comment trail includes kind=review,
  status=success`), and the SPEC §7.5.3 example uses `any_comment_kind/
  status`. If the spec restricts itself to existence predicates AND defines
  existence as "a comment with this (kind,status) exists" (history-aware),
  the validator needs an EXISTS query per (kind,status), which
  `recent_kinds` cannot serve when a later same-kind comment overwrote it.
  The `last_comment_*` fields in §7.5.1 are present in the schema but
  unused by any PRD §6.7 row — spec should either populate them from a new
  query or mark them reserved.

### R-P02-8 — pgcrypto pgp_sym_encrypt / DEK via Encore secret — **CONFIRMED**
- The pattern is **already proven in P01**: `auth/auth.go:487`
  `pgp_sym_encrypt($3, $5)` with `secrets.MemoryDEK`; decrypt
  `pgp_sym_decrypt(value_enc, $dek)::text` (SPEC §9.4.10). pgcrypto
  extension is created in `0010_bootstrap`.
- **Secret wiring CONFIRMED:** `MEMORY_DEK` → Go field `MemoryDEK` in
  `var secrets struct` (`auth/secrets.go:68`); placeholder in
  `secrets.nonprod.cue:62` (`MemoryDEK: "nonprod-…"`); copied to
  `.secrets.local.cue`. The memory service in P02 reads the **same**
  `MemoryDEK` (SPEC §9.4.10 says "the same DEK" for all `_enc` columns).
  **P02 spec note:** the `memory` service package needs its own
  `var secrets struct { MemoryDEK string }` (Encore secrets are
  package-scoped) — the CI drift-check unions all packages, so this is
  already covered by `secrets.nonprod.cue`.
- **Encrypt args:** SPEC §9.4.10 uses
  `pgp_sym_encrypt($plaintext,$dek,'cipher-algo=aes256,compress-algo=2')`
  (the P01 oauth path omits the options arg — both valid; spec should pin
  the memory path to the explicit aes256 form for parity).
- **Latency (Q6):** memory tools carry **no PRD latency gate** (plan Q6
  CONFIRMED — not on the `prime→ready→claim` hot path). `recall` decrypts
  per row; acceptable as a spec-level NFR only.
- **DEK rotation (`MEMORY_DEK_NEXT`, AR-7):** documented, **not exercised
  in P02** — confirmed inert for this phase.
- **Evidence:** `auth/auth.go:476-499`, `auth/secrets.go:63-96`,
  `secrets.nonprod.cue`, SPEC §9.4.10.

### R-P02-9 — Sanitiser pattern set + ts_doc ordering — **PARTIALLY CONFIRMED**
- **Ordering CONFIRMED structural:** `0090_memory.up.sql` comments lock
  "sanitisation runs *before* encryption" and `ts_doc` "built over the
  *plaintext* before encryption" — so sanitise → tokenise(ts_doc) →
  encrypt(value_enc). The DDL enforces nothing here; the **ordering is a
  service-code invariant** the spec must pin and B-1/B-2 must implement.
  RP02-5 risk (a missed pattern leaking into the GIN-indexed `ts_doc`) is
  real and is why `sanitiser_events` + periodic re-scan exist.
- **Pattern set: NOT YET DEFINED.** No sanitiser code or regex set exists
  in `apps/api/` today (grep: zero matches). NFR-7/PRD R-6 make detection
  best-effort and the warning mandatory. **Spec must author the v1.0
  baseline regex set** — candidates the research recommends: AWS access
  key id (`AKIA[0-9A-Z]{16}`), generic `(?i)(secret|token|password|api[_-]?
  key)\s*[:=]\s*\S+`, GitHub PAT (`ghp_/gho_/ghu_/ghs_/ghr_[A-Za-z0-9]{36,}`),
  PEM block markers, JWT (`eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+`),
  bearer headers, and email addresses (the providers-payload sanitiser also
  redacts emails per `0060`). The exact set is a spec contract; research
  pins the *posture* (redact-not-reject, audit on hit) not the final list.
- **`memory.sanitiser_events` shape: NOT YET MIGRATED** (AR-14 says "added
  in P02"). Additive migration required (entry_id?, scope, pattern_id/
  category, matched_at, redaction_count). Shape is a spec contract.
- **Status PARTIALLY:** ordering + audit-table + re-scan-job *requirements*
  confirmed; the pattern set and the audit-table DDL are net-new spec
  contracts with no existing artifact to validate against.

### R-P02-10 — forget soft-delete vs hard-delete — **CONFIRMED (genuine DDL gap) → C4**
- `0090_memory.up.sql` `memory.entries` has **no `deleted_at` column**
  (verified — only `expires_at`). SPEC §5.2.2 Tool 27 promises soft-delete
  "via `deleted_at`-equivalent". **Genuine gap; additive migration
  required.**
- **Partial-index consequence (C4):** the three per-scope uniqueness
  indexes are `entries_org_key_uniq … WHERE scope='org'` (and project/
  user). Soft-delete means a deleted `(scope,key)` must NOT block
  re-`remember`ing the same key. The indexes **must become partial-on-
  not-deleted**: `WHERE scope='org' AND deleted_at IS NULL` (and the two
  siblings). Without this, `forget` then re-`remember` of the same key
  fails the unique constraint. This is an additive migration that DROPs +
  recreates the three partial indexes (a forward migration, never an edit
  to `0090` — per `feedback_migration_edit_drift`).
- **Read-path consequence:** `recall`/`memories` must filter
  `deleted_at IS NULL` (couples with R-P02-13's `expires_at` filter).
- **Evidence:** `0090` lines 47-55 (the three partial unique indexes, no
  `deleted_at` clause); SPEC §5.2.2 line 528.

### R-P02-11 — Encore Cron + Pub/Sub for the jobs; sync-vs-async — **CONFIRMED**
- **Cron CONFIRMED:** `cron.NewJob` with parameter-free idempotent
  endpoints; **free-tier minimum interval = hourly** (see R-P02-12/C5).
  Cron does NOT run locally / in preview — so the reconciler, payload-
  digest, and sanitiser-rescan jobs are testable only via their
  underlying endpoint directly under `encore test` (call the endpoint, not
  the schedule). The four P02 crons: reconcile, payload-digest (daily,
  §9.4.5), sanitiser re-scan (AR-14), `mcp-warmer` (AR-16).
- **Pub/Sub CONFIRMED real:** the `provider.events` topic named in
  CLAUDE.md does **not yet exist** (grep: only `deps-cascade-*` topics
  exist). The async webhook→normalise path (Q3 working assumption) is
  **viable and idiomatic** — `pubsub.NewTopic[*ProviderEvent](...,
  {DeliveryGuarantee: AtLeastOnce})` + `pubsub.NewSubscription`, mirroring
  `deps/cascade.go`.
- **Async path requirement (plan §5 R-P02-11, AR-11) CONFIRMED:** if async,
  the subscriber payload MUST carry a **publisher-generated ULID
  `EventID`** as a typed field; at-least-once replay dedup uses
  `ON CONFLICT DO NOTHING` keyed on it — **distinct** from the handler-side
  `(provider, delivery_id)` AR-12 dedup. P01's `deps.cascade_events`
  `(event_id, triggered_by_item_id) UNIQUE` + `ON CONFLICT DO NOTHING`
  (`cascade.go:158-162`) is the proven template.
- **Sync-vs-async recommendation:** **async** — the 10-s GitHub budget +
  Law-3 spirit (a normaliser bug must never make GitHub retry-storm) both
  favour ack-fast/normalise-async. The handler does HMAC-verify → dedup-
  insert `providers.events` → publish → 200; the subscriber normalises.
  This is the plan's working assumption and research confirms it.
- **Pub/Sub volume budget:** 100k messages/day free-tier (R-P02-12) is
  ample — one message per webhook delivery + cascade fan-out is far under
  that at v1 scale.
- **Evidence:** `deps/cascade.go`, `deps/cascade_subscriber.go`; encore
  cron-jobs doc; Fair-Use limits.

### R-P02-12 — Encore Cloud free-tier ceilings + mcp-warmer — **CONFIRMED → C5**
- **Concrete Fair-Use numbers:** 100,000 requests/day; **100,000 Pub/Sub
  msgs/day**; 1 GB DB; cron **once-per-hour minimum**; max 2 cloud envs;
  no PR-preview envs; no guaranteed log/trace retention; over-limit →
  Encore contacts you (no auto-kill).
- **mcp-warmer viability (C5):** AR-16 specifies the warmer hits
  `mcp.meta_catalogue` "every N minutes" to defeat scale-to-zero cold
  start. **On the free tier, cron cannot run more often than hourly** — an
  every-N-minutes warmer is **not deliverable as an Encore cron on the free
  tier.** Options: (a) accept hourly warm-pings (insufficient to keep a
  scale-to-zero service warm between pings); (b) an external warmer
  (off-Encore uptime pinger hitting `POST /mcp`); (c) treat cold-start as
  the documented outlier class per Q5 (capacity is a *report*, not a gate)
  and AR-16(a) (NFR-1 measured warm only). Per Q5 this is non-gating, but
  the spec must record that the free-tier hourly cron makes the AR-16
  in-Encore warmer ineffective.
- **Connection cap:** not published numerically (Fair-Use doc silent). AR-13
  mandates pooled DB bindings (already the P01 pattern — single
  `sqldb.NewDatabase` owner in `apps/api/db/db.go`, BindDB late-bind). No
  per-call fresh connections; confirmed compliant.
- **Cold-start:** Fair-Use doc does not quantify scale-to-zero latency;
  AR-16 treats it as a launch-period outlier; the E-3 measurement (Q5
  non-gating) publishes the real number on staging.
- **Evidence:** encore usage Fair-Use limits; SPEC AR-13/AR-16; `apps/api/
  db/db.go` pooled-binding pattern.

### R-P02-13 — memory.entries.expires_at semantics — **CONFIRMED (inert today; decision needed)**
- `expires_at timestamptz` (nullable) ships in `0090` but **no tool/code
  references it** (grep: only the schema line + auth's unrelated
  `sessions.expires_at`). Mirror of the `forget`/`deleted_at` gap: a
  shipped column with no defined behaviour.
- **Three options (plan B-6):** (a) wire it — `remember` accepts optional
  `expires_at`; `recall`/`memories` filter `WHERE expires_at IS NULL OR
  expires_at > now()` at read time; (b) inert/reserved for v1.0, no DDL
  change, documented; (c) read-filter only (no write surface). No DDL
  change needed for any option (column already exists).
- **Recommendation:** couple the read-time filter with R-P02-10's
  `deleted_at IS NULL` filter (one combined predicate in `recall`/
  `memories`), and decide write-surface (a vs c) at spec time. There is no
  expiry *sweeper* cron in scope — read-time filtering only (matches the
  `mcp.api_keys.expires_at` precedent in `0070`: "honoured if set … no
  scheduler in v1").
- **Evidence:** `0090` line 32; `0070` lines 41-44 (the api-keys
  read-time-honour-no-sweeper precedent).

---

## Contradictions and Risks

### C1 — GitHub-App webhook secret is app-level, but the DDL models it per-install
- **Plan/DDL says:** `providers.installations.webhook_secret_enc` — a
  per-installation encrypted webhook secret (`0060_providers.up.sql:16`).
- **Reality:** under the GitHub-App model (R-P02-4 working assumption,
  confirmed best path), the webhook URL **and secret are configured once at
  the App level** and fire for **all** installations; deliveries are
  disambiguated by `installation.id` in the payload, not by a per-install
  secret.
- **Impact:** HMAC verification cannot key off a per-install secret because
  the request must be verified **before** the body is parsed to learn the
  installation. The single app-level secret must be available at the
  transport edge (an Encore secret, e.g. `GitHubAppWebhookSecret`), not
  read from `installations.webhook_secret_enc`.
- **Recommendation (Ada):** Either (a) verify HMAC with an app-level
  Encore secret and repurpose/retire `webhook_secret_enc` (additive
  migration to drop or leave-nullable — pre-prod, no migration tax,
  `feedback_pre_production`); or (b) keep `webhook_secret_enc` only for a
  future OAuth-app/GitLab per-install model and document it as unused under
  the v1.0 App path. The OAuth-app fallback (R-P02-4) is the only world
  where `webhook_secret_enc` is load-bearing. Track-E secrets (E-2) must add
  `GitHubApp*` placeholders (App ID, private-key PEM, app-level webhook
  secret) to `secrets.nonprod.cue` + `SECRETS.md` — none exist today (the
  registry holds OAuth-app secrets only), exactly as plan E-2 notes.

### C2 — go-sdk has no "registered-but-unlisted tool"; verify_can_transition/meta_catalogue need a different mechanism
- **Plan/SPEC says:** `verify_can_transition` and `meta_catalogue` are
  "not a separate top-level MCP tool … exposed via the same SSE channel"
  and excluded from the 27 (SPEC §5.2.2).
- **Reality:** `go-sdk@v1.6.0` `Server.AddTool` always advertises in
  `tools/list`; no `ServerOptions` listing filter; no hidden-tool concept.
- **Impact:** if both are added as `AddTool`, the agent-facing `tools/list`
  becomes 29, contradicting the "exactly 27, final" SPEC/plan §2.2 claim.
- **Recommendation (Ada):** `meta_catalogue` → MCP **Resource**
  (`AddResource`, surfaces under `resources/list`); `verify_can_transition`
  → **custom JSON-RPC method** via `AddReceivingMiddleware` (e.g.
  `unblock/verifyCanTransition`). Both keep `tools/list` at 27. Also fix the
  stale "same SSE channel" wording — the live transport is Streamable HTTP
  (`POST /mcp`), not the deprecated SSE+POST shape. This is the only
  CONTRADICTED item; it is a mechanism choice, not a scope change.

### C3 — Layer-1 validator needs a comment-trail read the P01 surface does not fully provide; and two PRECONDITION error codes now coexist
- **Plan §2.1 says:** "confirm the P01 GetTrail/get_state RPCs expose
  everything the validator needs without a new private RPC."
- **Reality:** `workitems.GetState.recent_kinds` is `DISTINCT ON (kind)` —
  latest-per-kind. It cannot answer the global `last_comment_*` predicate
  (§7.5.1) nor a history-aware "a `(kind=qa,status=success)` ever existed"
  predicate when a later same-kind comment overwrote the status.
- **Impact:** P02 must add/extend a `workitems` read RPC for the validator
  (e.g. `GetCommentTrailPredicates`), OR the spec must restrict §6.7
  preconditions to predicates `recent_kinds` *can* serve (latest-per-kind
  status equality — which actually matches every PRD §6.7 row as written,
  since each gate checks the most recent review/qa outcome). Additionally,
  P01's `set_state` already enforces column-value invariants I-1..I-5 with
  `PRECONDITION_NOT_MET` + `data.invariant`; P02 adds comment-trail gates
  with `PIPELINE_PRECONDITION_NOT_MET`. **RP02-2 risk:** a single bad
  transition must not yield two conflicting rejection codes.
- **Recommendation (Ada):** (1) decide whether §6.7 gates are
  "latest-per-kind" (servable by `recent_kinds`, no new RPC) or
  "ever-existed" (needs a new EXISTS-per-(kind,status) RPC) — the PRD §6.7
  wording ("comment trail includes …") reads as ever-existed, so plan for a
  new/extended RPC; (2) pin the order: column-value invariants (I-1..I-5,
  `PRECONDITION_NOT_MET`) run first, comment-trail gates
  (`PIPELINE_PRECONDITION_NOT_MET`) second, one error wins (A-3 / RP02-2).

### C4 — forget soft-delete forces the per-scope unique indexes to go partial-on-not-deleted
- **Plan/SPEC says:** `forget` is a soft-delete preserving audit trail
  (SPEC §5.2.2).
- **Reality:** `memory.entries` has no `deleted_at`; the three per-scope
  unique indexes (`entries_{org,project,user}_key_uniq`) are partial on
  `scope` only.
- **Impact:** after `forget`, re-`remember`ing the same `(scope,key)` would
  violate the live unique index — soft-delete silently breaks key reuse.
- **Recommendation (Ada):** additive forward migration: add `deleted_at
  timestamptz` to `memory.entries`; DROP + recreate the three unique
  indexes with `… AND deleted_at IS NULL`; `recall`/`memories` filter
  `deleted_at IS NULL`. Never edit `0090` in place.

### C5 — Encore free-tier cron is hourly-minimum; the AR-16 every-N-minutes mcp-warmer is not deliverable in-Encore on the free tier
- **SPEC AR-16 says:** an Encore cron `mcp-warmer` hits `mcp.meta_catalogue`
  "every N minutes" to defeat scale-to-zero cold start.
- **Reality:** Encore Cloud **free tier caps cron at once-per-hour**; cron
  also does not run locally / in preview.
- **Impact:** an hourly warm-ping cannot keep a scale-to-zero MCP service
  warm between requests; the in-Encore warmer as specified is ineffective
  on the launch (free) tier.
- **Recommendation (Ada/Olive):** per Q5 this is **non-gating** (capacity
  is a report, NFR-1 measured warm). Document that the in-Encore warmer is
  hourly-bounded on the free tier; if a real warmer is wanted, use an
  external uptime pinger hitting `POST /mcp`, or accept cold-start as the
  documented outlier class (AR-16(a)). The E-3 staging measurement
  publishes the real cold-start number.

### R1 — Bidirectional sync loop / rate-limit exhaustion (RP02-1)
- **Risk:** echo-webhook storm burning the 5,000/hr REST budget.
- **Evidence:** confirmed loop mechanism; `sender.login` + `last_synced_at`
  + idempotent-normalise are all available suppressors (R-P02-3).
- **Mitigation:** actor allowlist (App bot login) + content-idempotent
  normalise; reconciler honours `x-ratelimit-*`/`retry-after` (go-github
  surfaces these); sync is opt-in per install (bounded blast radius).

### R2 — Sanitiser false-negative leaks a credential into the unencrypted GIN-indexed ts_doc (RP02-5)
- **Risk:** a missed pattern tokenises into `ts_doc` (plaintext-derived,
  GIN-indexed, unencrypted by necessity).
- **Evidence:** `0090` ordering comments; no pattern set exists yet
  (R-P02-9).
- **Mitigation:** structural sanitise-before-tokenise (B-1);
  `sanitiser_events` audit + periodic re-scan (B-2, AR-14) make a missed
  pattern recoverable; `ts_doc` SELECT-locked to the memory connection user
  (AR-10). The v1.0 pattern set is best-effort by NFR-7/PRD-R6.

---

## Open Questions (need human / Ada decision — not researchable)

1. **C1 disposition:** under the GitHub-App path, drop/retire
   `installations.webhook_secret_enc`, or keep it nullable-and-unused for a
   future OAuth-app/GitLab per-install model? (App ID + PEM + one app-level
   webhook secret is the confirmed v1.0 set.)
2. **C2 mechanism:** `meta_catalogue` as MCP Resource and
   `verify_can_transition` as a custom JSON-RPC method (recommended), or
   relax the SPEC §5.2.2 "exactly 27 / not listed" wording and ship both as
   normal tools? Either way the §5.2.2 "same SSE channel" phrasing needs a
   Streamable-HTTP correction.
3. **C3 predicate semantics:** are PRD §6.7 comment-trail gates
   "latest-per-kind" (no new RPC) or "ever-existed" (new EXISTS RPC)? The
   PRD wording reads as ever-existed → plan a new/extended `workitems` read
   RPC. Confirm before A-3/A-4.
4. **R-P02-13 expires_at:** wire a `remember` write surface + read filter,
   or read-filter-only, or inert/reserved? (No DDL change either way.)
5. **R-P02-9 pattern set:** confirm the v1.0 sanitiser regex baseline (the
   research-recommended set is a starting point, not a locked contract).
