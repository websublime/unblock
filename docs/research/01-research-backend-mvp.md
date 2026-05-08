# Research: P01 Backend MVP — Encore Go + Postgres + 14 MCP Tools

**Source PRD:** [docs/PRD.md](../PRD.md) (APPROVED, 2026-05-07)
**Source Plan:** [docs/plans/01-plan-backend-mvp.md](../plans/01-plan-backend-mvp.md) (APPROVED, 2026-05-07; resolutions applied 2026-05-08)
**Source SPEC:** [docs/SPEC.md](../SPEC.md) (APPROVED 2026-05-07)
**Date:** 2026-05-08
**Author:** Smith (feasibility research)

> Closes R-P01-1 through R-P01-10 from `docs/plans/01-plan-backend-mvp.md` §5.
> Six findings classified **CONTRADICTED** are spec-phase blockers: the spec
> author (Ada) MUST resolve them before `/spec` writes the JSON-locked
> contracts. Two **PARTIAL** findings are recoverable inside the spec.
> Two additional plan-level technical assumptions were surfaced beyond the
> ten R-P01-* items — see §"Additional findings".

---

## Dependencies Investigated

### D1 — Encore Go Pub/Sub (`encore.dev/pubsub`)

- **Documentation:** https://encore.dev/docs/go/primitives/pubsub ; https://pkg.go.dev/encore.dev/pubsub
- **Capabilities:** typed topic via `pubsub.NewTopic[T](name, TopicConfig{DeliveryGuarantee: pubsub.AtLeastOnce})`. `Publish(ctx, &T) (messageID string, err error)` returns the ID at publish time.
- **Subscriber surface:** `pubsub.NewSubscription[T](topic, name, SubscriptionConfig[T]{Handler: func(ctx context.Context, msg T) error})`. Handler receives only `(ctx, T)`.
- **Limitations (load-bearing):** the handler signature does **not** expose the message ID, attempt count, or any delivery metadata. pkg.go.dev confirms: "Handler is the function which will be called to process a message" — the only argument is the typed message. The published `messageID` is "also provided to the subscribers when processing the event" per the prose docs, but the Go SDK does not expose it through the handler's parameter list.
- **Evidence quote:** "all subscription handlers should be idempotent" — Encore Pub/Sub docs.
- **Evidence quote:** "Handler func(ctx context.Context, msg T) error" — pkg.go.dev/encore.dev/pubsub `SubscriptionConfig`.

### D2 — Encore Go `sqldb.Database` and migration runner

- **Documentation:** https://encore.dev/docs/go/primitives/databases ; https://encore.dev/docs/go/primitives/share-db-between-services
- **Capabilities:** `sqldb.NewDatabase(name, sqldb.DatabaseConfig{Migrations: "./migrations"})`. Migration files are SQL, must be named `NNNN_description.up.sql`, "must increase sequentially". Migrations run automatically on service deploy/start.
- **Database ownership:** "Each database is defined within a service" — the service that calls `NewDatabase` is the owner. Other services access via `sqldb.Named("name")` (read-only consumer pattern).
- **Multi-schema:** documentation does **not** explicitly forbid multiple `CREATE SCHEMA` statements in one migration file, nor does it document the pattern. Engineering precedent (e.g., Encore community Discord, third-party blog posts) confirms multiple schemas in a single Encore-managed DB are achievable, but **all migrations must live under one owning service** — the plan's "8 services × 8 schemas, all migrations" assumption is structurally compatible only if exactly one service owns the migrations directory.
- **Extensions:** the SPEC's bootstrap pattern `CREATE EXTENSION IF NOT EXISTS pgcrypto; CREATE EXTENSION IF NOT EXISTS pg_trgm;` is plain SQL inside a `.up.sql` file — Encore runs whatever SQL is in the file. No special extension API.
- **Evidence quote:** "Migration files must start with a number followed by an underscore (`_`), and must increase sequentially."
- **Evidence quote:** "Each database is defined within a service, and that service's name becomes the database name."

### D3 — Encore Go SSE / streaming response support

- **Documentation:** https://encore.dev/docs/go/primitives/raw-endpoints
- **Capabilities:** `//encore:api public raw method=GET path=/mcp/sse` exposes the bare `http.ResponseWriter` and `*http.Request`, so SSE is implementable by writing chunked `text/event-stream` responses with explicit `Flusher` calls — exactly the standard Go SSE pattern.
- **Limitations:** Encore's docs do **not** document any edge-proxy / load-balancer timeout for long-lived connections. The free-tier infrastructure terms also do not document an idle-connection timeout. This is a real gap — see CONTRADICTION C4.
- **MCP framing library:** the plan §2.1 D-1 mentions "rmcp Go bindings". This is a misnomer. `rmcp` is the **Rust** MCP SDK (`modelcontextprotocol/rust-sdk`); the Go SDK is a separate project at `modelcontextprotocol/go-sdk`, "maintained in collaboration with Google".
- **Evidence quote:** "An official Rust Model Context Protocol SDK implementation with tokio async runtime" — github.com/modelcontextprotocol/rust-sdk.
- **Evidence quote:** "the official Go software development kit (SDK) for the Model Context Protocol (MCP)" — github.com/modelcontextprotocol/go-sdk.

### D4 — MCP wire protocol (Streamable HTTP, replaces SSE-only)

- **Documentation:** https://modelcontextprotocol.io/specification/2025-06-18/basic/transports
- **Capabilities:** the **2025-06-18 spec** defines two transports: `stdio` and **Streamable HTTP**. Streamable HTTP requires a *single* MCP endpoint that supports **both POST (client → server) and GET (server → client SSE stream)**.
- **Critical change:** "This replaces the [HTTP+SSE transport](.../2024-11-05/basic/transports#http-with-sse) from protocol version 2024-11-05".
- **Old transport (SSE-only):** the deprecated 2024-11-05 transport required **two endpoints** — an SSE endpoint plus a separate POST endpoint, with the server emitting an `endpoint` event on connect to tell the client where to POST. The plan's `GET /mcp/sse` shape matches this deprecated model.
- **Backwards-compat:** modern clients will probe POST first; on 4xx fall back to GET-SSE (old transport). Servers that only ship the old SSE+POST pair still work for clients that implement fall-back.
- **Session management:** Streamable HTTP introduces `Mcp-Session-Id` response header at init time; clients must echo it on subsequent requests.
- **Evidence quote:** "The server **MUST** provide a single HTTP endpoint path … that supports both POST and GET methods."
- **Evidence quote:** "The client **MUST** include an `Accept` header, listing both `application/json` and `text/event-stream` as supported content types."

### D5 — Claude Code MCP transport support

- **Documentation:** https://code.claude.com/docs/en/mcp
- **Capabilities:** `claude mcp add <name> --transport http <url>` (Streamable HTTP), `claude mcp add <name> --transport sse <url>` (legacy SSE), or `--transport stdio`. Both remote transports are supported.
- **Implication:** Claude Code is forward-compatible with **both** the new Streamable HTTP transport and the legacy SSE+POST transport.

### D6 — Cursor MCP transport support

- **Documentation:** https://cursor.com/docs (MCP section)
- **Capabilities:** "Cursor supports three transport methods": stdio, SSE, Streamable HTTP. Both remote transports supported.

### D7 — GitHub Copilot MCP transport support

- **Documentation:** https://docs.github.com/en/copilot/customizing-copilot/extending-copilot-chat-with-mcp
- **Limitation:** the GitHub docs page does not enumerate transport support per host. Visual Studio reportedly supports "both remote and local servers"; per-transport detail is not crisply documented at this URL. **OPEN QUESTION** — see §"Open Questions".

### D8 — PostgreSQL recursive CTE — `LIMIT` semantics

- **Documentation:** https://www.postgresql.org/docs/current/queries-with.html
- **Standard pattern:** **depth counter with `WHERE depth < N`** in the recursive term. PG explicitly recommends the `WHERE depth < N` pattern for production.
- **`LIMIT N` in parent query:** documented, but only "trick for testing" — "Using this trick in production is not recommended."
- **`LIMIT N` *inside* the recursive term:** **not documented** at all in the official PG queries-with page. PG evaluates the recursive term iteratively against a working table; placing `LIMIT N` inside the recursive term has implementation-defined behaviour and depends on the planner.
- **Evidence quote:** "A helpful trick for testing queries when you are not certain if they might loop is to place a `LIMIT` in the parent query."
- **Evidence quote:** "Using this trick in production is not recommended, because other systems might work differently."
- **Implication for SPEC §9.4.9:** the SPEC's "raised as `LIMIT 256` inside the recursive term" is non-standard. The CTE *snippet* in §9.4.9 actually does **not** include `LIMIT 256` in the recursive term; the cap is described in prose only. Spec-phase fix: rewrite as a depth counter (`WHERE depth < 256`).

### D9 — PostgreSQL `SELECT FOR UPDATE` under poolers (PgBouncer ≈ Encore Cloud's pooler)

- **Documentation:** https://www.pgbouncer.org/features.html
- **Capabilities (session pooling):** "When a client connects, a server connection will be assigned to it for the whole duration it stays connected" — full transactional locking semantics preserved.
- **Capabilities (transaction pooling):** "a server connection is assigned to a client only during a transaction" — locks acquired by `SELECT FOR UPDATE` ARE preserved within the transaction (the entire transaction lives on one backend connection by definition); locks DO NOT survive `COMMIT`. Since `claim` is a single-transaction mutation (SPEC §5.5), transaction pooling is safe for the claim path.
- **Encore-specific:** Encore Cloud's pooler mode is **not documented** in the public docs (`https://encore.dev/docs/platform/infrastructure/infra` covers infra provisioning but not pool mode). Almost certainly transaction pooling (industry default for managed Postgres + Go), but **unverified by docs**.
- **Implication:** the atomic-claim transaction (BEGIN; SELECT … FOR UPDATE; UPDATE; COMMIT — SPEC §5.5) is safe under either pool mode because the entire critical section lives inside a single transaction, but the spec should pin the assumption explicitly.

### D10 — PostgreSQL multi-table FTS pattern

- **Documentation:** https://www.postgresql.org/docs/current/textsearch-tables.html
- **Capability:** **GIN indexes are per-table.** "PostgreSQL indexes are table-specific constructs, so a single GIN index cannot span multiple tables." The standard pattern for cross-table FTS is per-table tsvector columns + UNION ALL at query time, OR a materialised view.
- **Implication for SPEC §9.4.3:** `workitems.items` carries `title + body` (per the search tool's intent — see plan §2.2 #9), and `workitems.comments` carries `body`. The current DDL has neither a `tsvector` column nor a GIN FTS index on either of those tables — the SPEC is silent. This is an **assumption gap** beyond the R-P01-* list — see §"Additional findings", AF1.
- **Memory entries (§9.4.8):** `memory.entries` carries both `pg_trgm` GIN on `key` and `tsvector` GIN on `ts_doc`. Per PG docs these are independent index types and coexist freely on the same table.
- **Evidence quote:** "Trigram matching is a very useful tool when used in conjunction with a full text index."

### D11 — Encore Cloud free-tier ceilings

- **Documentation:** https://encore.cloud/pricing ; https://encore.dev/docs/platform/management/usage
- **Documented free-tier ceilings (per app):**
  - **Requests:** 100,000 / day
  - **Database storage:** 1 GB
  - **Pub/Sub messages:** 100,000 / day
  - **Object storage:** 1 GB
  - **Cron:** "once every hour"
  - **Tracing:** 1M events / month
  - **Log/trace retention:** 7 days
  - **Concurrent build:** 1
- **Not documented:** Postgres connection cap, request rate per second, cold-start latency, edge proxy idle timeout, max request duration. The pricing page calls out "Fair use" as the gating concept and explicitly disclaims warranty.
- **Evidence quote:** "Encore Cloud is subject to Fair Use guidelines and comes without warranty, as it's not intended for large-scale business-critical use cases."
- **Implication for M-1 (`prime → ready → claim` p99 < 2 s):** cold-start latency is **unmeasured**. The plan's NFR-1 harness (E-2) must run on a **warm** instance and explicitly carve out cold-start outliers, or the gate is meaningless.

### D12 — GitHub OAuth2 + PKCE

- **Documentation:** https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps ; .../scopes-for-oauth-apps
- **PKCE support:** confirmed for OAuth Apps. "Used to secure the authentication flow with PKCE (Proof Key for Code Exchange). Required if `code_challenge_method` is included." Only `S256` accepted; `plain` not supported.
- **Scopes for v1.0 (read-only at v1.0, no webhook subscription, no write):** `read:user` (read user profile). Repo metadata is **not needed at v1.0** because P01 has no provider integration (plan §3.1) and P02 webhook subscription belongs to the GitHub App path, not the OAuth App path.
- **GitHub App vs OAuth App for webhooks:** "By default, GitHub Apps have a single webhook that receives the events they are configured to receive for every repository they have access to" — GitHub App is the recommended path for webhook subscription. OAuth App alternative requires `write:repo_hook` scope per repo plus manual cleanup.
- **Implication for v1.0 Identity:** scopes `read:user` (and optionally `user:email` if email at signup is required) are sufficient. **`public_repo` / `repo` are NOT needed** at v1.0 — P02 should add a separate GitHub App installation flow (not OAuth scope expansion) for webhook subscription.
- **Evidence quote:** "Required if `code_challenge_method` is included. Must be a 43 character SHA-256 hash …"
- **Evidence quote:** "Webhooks are not automatically disabled if an OAuth app's access token is deleted, and there is no way to clean them up automatically."

### D13 — API key format and storage (high-entropy random tokens)

- **Documentation:** OWASP Password Storage Cheat Sheet; Django auth/passwords docs; widely-cited Filippo Valsorda / Mathias Bynens posts on token storage.
- **Capability — fast hash for high-entropy tokens:** for a 32-byte URL-safe random key (~256 bits of entropy), brute-force is infeasible at any conceivable speed. The slow-hash justification (defending against low-entropy password brute-force) does not apply. Industry consensus (Django, GitHub, Stripe, AWS) is: **HMAC-SHA256 or plain SHA-256 over the random bytes**, with constant-time comparison at verify.
- **Evidence quote (Django):** "PBKDF2 and bcrypt … deliberately slows down attackers, making attacks against hashed passwords harder" — applies to passwords, not high-entropy tokens.
- **Evidence quote (OWASP):** "Use Argon2id with a minimum configuration of 19 MiB of memory, an iteration count of 2, and 1 degree of parallelism" — note the cheat sheet scope is **password storage**, not API token storage.
- **Capability — prefix scheme:** Stripe's `sk_live_<key>`, GitHub's `ghp_<key>` patterns enable (a) fast prefix-indexed lookup at the DB (no full-table scan over hashed values), (b) human-recognisable keys for logs/UI ("last-4 hint"), (c) trivial format validation on ingest. SPEC §9.4.6 already declares `key_prefix text NOT NULL` with a UNIQUE constraint and a `key_prefix` partial index — the schema is right; the hash algorithm choice is the open question.
- **Implication for SPEC §9.4.6:** the comment "argon2id of the actual key" is **technically wrong** for a high-entropy random key. Use `sha256_hmac(server_secret, key)` or `sha256(key)` (with the `key_prefix` doing fast lookup, not the hash). Argon2id over a 32-byte random key burns 19 MiB and ~50 ms per Bearer-auth request for **zero security benefit** — directly attacks NFR-1.
- **Capability — rotation strategy:** issue new key (new prefix), keep old key valid until `revoked_at` is set; agent operator updates Bearer; old row marked `revoked_at`. This is a v1.0-acceptable surface; the SPEC §10.1 defers the lifecycle to "the P01 spec" — confirm in spec.

---

## Assumptions Validated

| # | Assumption (from Plan) | Status | Evidence |
|---|---|---|---|
| **R-P01-1** | Encore Pub/Sub envelope exposes a stable `delivery_id` usable for AR-11 idempotency on `cascade_events` insert. | **CONTRADICTED** | The Go SDK handler signature is `func(ctx context.Context, msg T) error`. No `MessageContext`, no attributes parameter, no delivery metadata exposed. AR-11's claim that "the delivery id is propagated from the Pub/Sub envelope" cannot be implemented with the current SDK surface — see C1. |
| **R-P01-2** | Encore migration runner handles cross-schema FK ordering across the 8 schemas in canonical order per SPEC §9.4.0. | **PARTIAL** | Encore runs migrations sequentially by filename within one service-owned migrations dir. The 8 schemas can ship in one dir if a single service owns the database. Cross-service migration coordination is **not** Encore's design; the plan's "all 8 schemas in P01 §2.1" assumption is implementable only if one service is designated migration-owner — see C2. |
| **R-P01-3** | Encore SSE / streaming for `GET /mcp/sse` integrates with rmcp Go bindings. | **CONTRADICTED** | (a) `rmcp` is Rust-only — the Go SDK is a separate project (`modelcontextprotocol/go-sdk`). (b) Encore raw endpoints support hand-rolled SSE via `http.ResponseWriter` + `Flusher`, **but** Encore Cloud's edge-proxy idle/total-request timeout is not documented — long-lived SSE is a structural risk RP01-4 — see C3 + C4. |
| **R-P01-4** | Encore Cloud free-tier ceilings (Pub/Sub rate, Postgres conn cap, cold-start latency) are compatible with M-1 (`prime → ready → claim` p99 < 2 s). | **PARTIAL** | Documented ceilings: 100k requests/day, 100k Pub/Sub messages/day, 1 GB Postgres storage, 1 concurrent build. **Not documented:** connection cap, request rate, cold-start, idle timeout. The 100k req/day = ~1.16 req/sec average sustained is structurally fine for v1 scale, but the latency numbers are unmeasured — NFR-1 measurement methodology must explicitly carve out cold-start outliers (RP01-1 mitigation in plan §7) — see C4. |
| **R-P01-5** | Postgres recursive-CTE `LIMIT 256` cap on cycle-detection CTE behaves as expected (terminates the recursive term, not the result row count). | **CONTRADICTED** | PG docs only document `LIMIT N` in the **parent query**, and explicitly mark it "not recommended for production". `LIMIT N` inside the recursive term is undocumented. The standard pattern is a depth counter `WHERE depth < N`. SPEC §9.4.9 actually shows a CTE *without* the `LIMIT 256` clause and only describes the cap in prose — the implementation will not match the SPEC's stated intent unless rewritten as a depth counter — see C5. |
| **R-P01-6** | `SELECT FOR UPDATE` lock survives across pooled-connection acquire/release cycles under Encore's pooler. | **PARTIAL** | Within a **single transaction**, both PgBouncer transaction-mode and session-mode preserve `SELECT FOR UPDATE` locks (the transaction owns one backend connection by definition). Encore Cloud's pool mode is undocumented but the SPEC §5.5 atomic claim is single-transaction, so the property holds independent of pool mode. **Spec must pin the assumption.** |
| **R-P01-7** | Multi-table FTS pattern (pg_trgm + tsvector inside one transaction, especially `memory.entries`). | **PARTIAL** | `memory.entries` works as designed: pg_trgm GIN on `key` and tsvector GIN on `ts_doc` are independent and coexist. **Gap:** `workitems.items` and `workitems.comments` have **no** tsvector column declared in §9.4.3, but the `search` MCP tool (plan §2.2 #9) promises FTS over titles + bodies + comment bodies. The SPEC's DDL is missing the FTS infrastructure for `workitems` — see AF1. |
| **R-P01-8** | MCP wire protocol cross-client compat (Claude Code, Cursor, Copilot CLI all accept the same SSE-based MCP wire format). | **CONTRADICTED** | The 2024-11-05 SSE+POST transport was **replaced** by Streamable HTTP in the 2025-06-18 spec. Modern clients (Claude Code, Cursor) support both via fallback, but **the plan's `GET /mcp/sse` is the deprecated transport**. Modern clients negotiate Streamable HTTP first; old-style SSE is a backwards-compat mode. Spec must decide which to ship — see C6. Copilot transport per host is not crisply documented — open question OQ1. |
| **R-P01-9** | GitHub OAuth2 + PKCE scope set required for v1.0 Identity. | **CONFIRMED** | PKCE supported (S256 only); for v1.0 Identity (no webhook subscription, no write), `read:user` is sufficient. P02 webhook subscription should use a **GitHub App** (recommended), not OAuth scope expansion. The plan §6 Q5 default ("`auth.users.primary_provider` value") is correct in §3.6. |
| **R-P01-10** | API key format and storage best-practice (bcrypt vs argon2id for hash, key prefix scheme for fast lookup, rotation strategy). | **CONTRADICTED** | SPEC §9.4.6 declares `key_hash bytea NOT NULL, -- argon2id of the actual key`. For a 32-byte random API key (~256-bit entropy), argon2id is the **wrong** primitive — it adds ~50 ms to every Bearer-auth check (directly attacking NFR-1) for zero security benefit (brute-force on 256-bit entropy is infeasible regardless of hash speed). The correct primitive is `sha256_hmac(server_secret, key)` or plain `sha256(key)`. Prefix scheme (`unblock_pat_<base32>`) and `key_prefix` UNIQUE index are correct in the SPEC — see C7. |

---

## Contradictions and Risks

### C1 — Pub/Sub message ID is not exposed to handlers (BLOCKER)

- **Plan / SPEC says:** AR-11 (SPEC §13) and the cascade subsystem (§5.4) require that "the delivery id is propagated from the Pub/Sub envelope into the audit row to support replay-safe inserts". Plan §2.3 reaffirms: `deps.cascade_events` insert is idempotent on `(delivery_id, triggered_by_item_id)`.
- **Reality:** Encore Go's `pubsub.NewSubscription` handler signature is `func(ctx context.Context, msg T) error`. The Go SDK does **not** surface the message ID, attempt count, or any per-delivery metadata to the handler. The published `messageID` returned from `Publish(ctx, &T)` is not propagated as a parameter the subscriber can read.
- **Impact:** AR-11's `(delivery_id, triggered_by_item_id)` uniqueness constraint cannot be populated with a real Encore-supplied delivery id. Either:
  - **Option A (workaround in payload):** the publisher generates a ULID at publish time, embeds it as a field in the typed message (`type CascadeRequested struct { DeliveryID string; … }`), and the subscriber reads it from the payload. **Caveat:** if Encore re-delivers the same envelope after a transient handler error, the same payload (including the embedded `DeliveryID`) is delivered — so this **does** give idempotency for the re-delivery case.
  - **Option B (drop AR-11's "from-envelope" framing):** the subscriber computes a deterministic key from the payload itself (e.g. `sha256(triggered_by_item_id || closure_at_seqno)`). This works only if the payload uniquely identifies the logical event; for "close item X", `triggered_by_item_id` alone would collide on legitimate re-emission, so the publisher must include something monotonic (the cascade-event sequence number from the same INSERT-emit transaction).
- **Recommendation for Ada:** prefer Option A. The publisher generates `delivery_id` (ULID) when emitting `deps.cascade.requested`; the subscriber treats it as the idempotency key. Update SPEC §5.4 and AR-11 to say "the delivery id is generated by the publisher and embedded in the payload" rather than "propagated from the Pub/Sub envelope".

### C2 — Encore database ownership is per-service; "8 services × 1 DB × 8 schemas" needs an owner (BLOCKER)

- **Plan / SPEC says:** plan §2.4 — "one Postgres database; … 8 schemas"; SPEC §5.2 — "Encore enforces the convention 'one service owns one Postgres schema'. The eight services map 1:1 to the eight schemas".
- **Reality:** Encore's database primitive is **per-service, not per-schema**. "Each database is defined within a service, and that service's name becomes the database name." Other services consume via `sqldb.Named("name")`, but the **migrations directory is owned by the defining service**. There is no documented pattern where 8 services each contribute migrations to one shared DB.
- **Impact:** the plan's task A-2 / A-3 ("eight schemas declared", "Migrations §9.4.1–§9.4.8 in canonical order") cannot map to 8 service-local migrations dirs. Either:
  - **Option A:** designate one service (say `core` or `auth`) as the migration-owner; all 8 schemas' DDL ships under that one service's `migrations/` directory in the canonical §9.4.0 order. Other services consume via `sqldb.Named(...)`.
  - **Option B:** abandon the "1 DB" rule and let each service own its own database (Encore-native pattern). This breaks SPEC FR-1 ("Single Postgres database with 8 schemas").
- **Recommendation for Ada:** Option A. Pin which service owns migrations in the spec (suggest `auth` since it has no incoming FKs, ordering in §9.4.0 step 1). Update plan §2.1 / §4.1 to reflect "one service owns migrations; other services declare `sqldb.Named` references".

### C3 — "rmcp Go bindings" is a misnomer (BLOCKER for D-1 task scope)

- **Plan says:** §2.1 D-1 — "rmcp Go bindings or do we implement the JSON-RPC framing ourselves?" §5 R-P01-3 — "rmcp Go bindings".
- **Reality:** `rmcp` is the **Rust** MCP SDK (`modelcontextprotocol/rust-sdk`). The Go MCP SDK is a separate project at `modelcontextprotocol/go-sdk`, "maintained in collaboration with Google".
- **Impact:** the D-1 task's "use rmcp Go vs. roll our own" decision tree is wrong by name. The actual choice is "use `modelcontextprotocol/go-sdk` vs. roll our own JSON-RPC framing".
- **Recommendation for Ada:** rename the dependency throughout. Spec should pin a version of `github.com/modelcontextprotocol/go-sdk` (verify maturity / API stability before pinning).

### C4 — Encore Cloud edge-proxy timeout for long-lived SSE is undocumented (BLOCKER for M-1 + RP01-4 risk)

- **Plan says:** plan §7 RP01-4 acknowledges the risk: "Free-tier proxies often kill idle connections. Agent reconnect logic must handle this gracefully. … Heartbeat ping every 15s; client reconnect on close."
- **Reality:** Encore Cloud's pricing/usage docs (`https://encore.cloud/pricing`, `https://encore.dev/docs/platform/management/usage`) document **none** of: max request duration, idle timeout, edge-proxy timeout. Free-tier infrastructure terms note "Fair use" without numbers.
- **Impact:** the SSE/Streamable-HTTP MCP transport's behaviour under Encore Cloud free-tier is untestable from docs alone. The 15-second heartbeat mitigation is sensible but the actual proxy timeout (could be 30 s, could be 100 s, could be 5 min) determines reconnect frequency. The exit-criterion harness E-4 must measure this empirically — but P01 acceptance is "local Encore emulator + CI green" (Q6 resolution), so the cloud risk is deferred to P02 deploy.
- **Recommendation for Ada:** spec must document that NFR-1 is measured **on the local emulator**, not on Encore Cloud, and the Cloud SSE behaviour is a P02 ops item owned by Olive. Add an explicit "warm cache" definition: pool established, identity validated, no cold-start in the budget.

### C5 — `LIMIT 256` inside recursive term is undocumented PG behaviour (BLOCKER)

- **Plan says:** §2.3 — "recursive CTE per SPEC §9.4.9 with `LIMIT 256` cap". §5 R-P01-5 — "exact PG semantics — does it bound the working set, the result set, or both?"
- **Reality:** PG docs document `LIMIT N` in the **parent query** ("trick for testing", "not recommended for production"). `LIMIT N` **inside** the recursive term is not in the PG queries-with reference at all. Behaviour is implementation-defined and planner-dependent. The standard production pattern is `WHERE depth < N` with an explicit depth counter column in the CTE.
- **Impact:** SPEC §9.4.9's CTE snippet does **not** include `LIMIT 256` in the recursive term — only the prose mentions it. As written, the SPEC has no actual cap. As intended, the cap mechanism is non-portable and depends on PG version behaviour.
- **Recommendation for Ada:** rewrite SPEC §9.4.9 cycle-prevention CTE with a depth counter:
  ```sql
  WITH RECURSIVE reachable(id, depth) AS (
      SELECT $2::text, 0
      UNION ALL
      SELECT d.to_item, r.depth + 1
        FROM deps.dependencies d
        JOIN reachable r ON d.from_item = r.id
        WHERE d.kind = 'blocks' AND r.depth < 256
  )
  SELECT 1 FROM reachable WHERE id = $1 LIMIT 1;
  ```
  This is portable, terminates deterministically, and gives the same v1 cap.

### C6 — `GET /mcp/sse` is the deprecated MCP transport (BLOCKER)

- **Plan / SPEC says:** plan §2.1, §2.4, §5.3 + SPEC §5.3 — public endpoint `GET /mcp/sse`. SPEC §5.2.2 footnote: "Operational primitives … served via the same SSE channel". The wire shape is **2024-11-05 HTTP+SSE transport** (separate SSE endpoint emitting an `endpoint` event + a separate POST endpoint).
- **Reality:** the **2025-06-18 MCP spec** *replaces* HTTP+SSE with **Streamable HTTP** — a single endpoint supporting both POST (client → server) and GET (server → client SSE). The SSE-only transport is documented as "deprecated".
- **Impact:** the plan's surface as written is the legacy transport. Modern clients (Claude Code, Cursor) support both via fallback negotiation, so it works, but the SPEC inherits a deprecated wire format on day one. Forward work to upgrade is non-trivial (session id management, POST endpoint shape).
- **Recommendation for Ada:** decide explicitly:
  - **Option A — ship Streamable HTTP from P01.** Single endpoint `/mcp` supporting POST + GET. Mcp-Session-Id header. This is forward-compatible and matches the current spec.
  - **Option B — ship legacy SSE+POST + accept the deprecation tax.** Two endpoints (`GET /mcp/sse` + `POST /mcp/messages`). Note: this still expands FR-12's "two public endpoints" to three (the legacy POST is a third public surface).
  Recommend Option A. Update FR-12, plan §2.1, SPEC §5.3 accordingly.

### C7 — `argon2id` over 32-byte API key wastes ~50 ms per Bearer auth (BLOCKER for NFR-1)

- **SPEC says:** §9.4.6 `mcp.api_keys.key_hash bytea NOT NULL, -- argon2id of the actual key`.
- **Reality:** argon2id (per OWASP cheat sheet — "19 MiB memory, t=2, p=1") is designed for **password storage** where entropy is low (~30 bits) and brute-force is the threat. A 32-byte URL-safe random key has ~256 bits entropy; brute-force is mathematically infeasible. Argon2id's CPU+memory cost on every Bearer auth check (industry-tuned ~50–100 ms) directly attacks NFR-1 (`prime → ready → claim` p99 < 2 s).
- **Impact:** every MCP tool call burns 50+ ms before the SQL even starts. With 3 calls in M-1 (prime, ready, claim), that's ~150–300 ms of pure hash overhead — a meaningful fraction of the 2 s budget.
- **Recommendation for Ada:** change SPEC §9.4.6 to:
  - `key_hash bytea NOT NULL, -- HMAC-SHA256(server_secret, raw_key)` OR `key_hash bytea NOT NULL, -- SHA-256(raw_key)`.
  - Bearer-auth lookup: parse prefix from header → `SELECT … WHERE key_prefix = ? AND revoked_at IS NULL` (uses `api_keys_prefix_uniq` index, O(1)) → `subtle.ConstantTimeCompare(stored_hash, computed_hash)`.
  - Document the rationale in §10.1 / §13 AR-X (new): "API keys are high-entropy random; SHA-256 is sufficient and brute-force is infeasible by entropy."

### R1 — One-service migration ownership is fragile under Encore deploy

- **Risk:** if `auth` (or whichever service owns migrations) is deployed before another service that has cross-schema FK declarations, Encore may interleave deploys in a way that violates §9.4.0 ordering.
- **Evidence:** Encore docs do not document deploy ordering for migrations across multi-service consumers of one DB.
- **Mitigation options:** (a) gate all 8 schemas behind one bootstrap migration in the migration-owner service that runs to completion before any service code is reachable; (b) in CI, run a "migrations only" job before any service-test job; (c) document the constraint in plan §6 OPEN QUESTION 8 (new) — "should the migration-owner service be a stub that does nothing but own migrations, to decouple migration ordering from service-deploy ordering?".

### R2 — `workitems` FTS infrastructure is missing from SPEC §9.4.3

- **Risk:** the `search` MCP tool (plan §2.2 #9, "FTS over titles + bodies + comment bodies") has no DDL backing in SPEC §9.4.3. No `tsvector` column on `workitems.items` or `workitems.comments`; no GIN FTS index. The plan's track D-4 ("Tools 9–10: search, comment") implements a tool with no index path.
- **Mitigation:** spec adds `to_tsvector('english', coalesce(title,'') || ' ' || coalesce(body,'')) STORED` generated columns + GIN indexes on both tables, and a `UNION ALL` query in the search RPC. See AF1 below.

---

## Additional Findings (assumptions surfaced beyond R-P01-*)

### AF1 — `workitems` FTS DDL is missing (additional finding to R-P01-7)

- **Plan says:** §2.2 #9 `search` tool — "FTS over titles + bodies + comment bodies". §6 cross-cutting CI gates assume the tool works.
- **SPEC says:** §9.4.3 declares no tsvector column on `workitems.items` or `workitems.comments`. Only `comments_kind_status_idx` exists for the comments hot path.
- **Recommendation:** spec adds `tsv_title_body tsvector GENERATED ALWAYS AS (...) STORED` on `workitems.items` and `tsv_body tsvector GENERATED ALWAYS AS (...) STORED` on `workitems.comments`, plus GIN indexes on each. The `search` RPC issues a `UNION ALL` over both tables (PG GIN cannot span two tables — D10).

### AF2 — `prime`'s "recent cascade events" cap of 50 (Q7) needs an index check

- **Plan says:** §6 Q7 resolution — "`prime` returns the last 50 `deps.cascade_events` rows scoped to the agent's org/project, ordered by `triggered_at desc`".
- **SPEC §9.4.4:** the existing index `cascade_events_org_triggered_idx ON deps.cascade_events (org_id, triggered_at DESC)` covers this query. The partial index `cascade_events_nonzero_idx (org_id, triggered_at DESC) WHERE cascaded_count > 0` covers the M-5 metric query but NOT the `prime` query (which wants all events, including no-op ones, for traceability).
- **Recommendation:** confirm in spec — the existing index is sufficient. Add an integration test: `prime` returns ≤ 50 rows and `EXPLAIN` shows index-only scan on `cascade_events_org_triggered_idx`.

### AF3 — Plan §3.4 close precondition is `claimed_by_id IS NOT NULL` only — but DDL doesn't enforce it

- **Plan says:** §3.4 — "`close` does not require `qa_state=passed` in P01; the precondition is `claimed_by_id IS NOT NULL` only."
- **SPEC §9.4.3:** the items DDL has `items_claim_status_chk` which says claim implies `status IN ('InProgress', 'Done')` — i.e., once claimed, status must be InProgress or Done. There is **no** DDL-level CHECK that says "to set status=Done, claimed_by_id must be NOT NULL" — that's an MCP-layer check.
- **Recommendation:** spec must explicitly write the MCP-layer precondition for `close`: reject if `claimed_by_id IS NULL`. The structural DDL constraint allows `(claimed_by_id NULL, status Done)` if `claimed_at` is also NULL — the plan's deliberate P01 relaxation must be implemented as MCP-layer code, not DDL.

### AF4 — `mcp.api_keys.expires_at` and lifecycle are unspecified for v1.0

- **SPEC §10.1:** "Exact key lifecycle and rotation policy land in the P01 spec."
- **Plan §5 R-P01-10:** "Rotation / revocation approach for v1.0". The plan does not have a Q-resolution for this.
- **Recommendation:** spec pins: keys never expire by default (`expires_at NULL`); rotation is opt-in via the seeder CLI; revocation flips `revoked_at`; no automatic refresh.

### AF5 — Cycle-detection CTE wraps in transaction with `SELECT ... FOR UPDATE` on edges

- **SPEC §9.4.9:** "with `SELECT ... FOR UPDATE` on `deps.dependencies` rows touching either endpoint to prevent racing inserts".
- **Reality (PG docs):** `FOR UPDATE` requires the rows to actually exist; a racing INSERT of a brand-new edge isn't blocked by `FOR UPDATE` on existing rows. The race is between two transactions both inserting cycle-creating edges from different vantage points.
- **Recommendation:** spec uses **advisory locks** at the (org_id, "deps") level OR a serialisable-isolation transaction OR a unique constraint that catches the concurrent insert. The "FOR UPDATE on touching rows" pattern as written is insufficient. This is a P01 spec-phase decision the architect must own.

---

## Open Questions

- **OQ1.** GitHub Copilot's per-host MCP transport support (cloud vs. local vs. CLI) is not crisply documented at `https://docs.github.com/en/copilot/customizing-copilot/extending-copilot-chat-with-mcp`. P01's exit criterion only mentions Claude Code-class agents. **Decision needed:** does P01 acceptance require a Copilot-targeted MCP harness, or is the Claude Code harness sufficient for the M-1 measurement? Recommend Claude Code only for P01; add Copilot to P04 plugin renderer scope.
- **OQ2.** Is `MEMORY_DEK` (SPEC §9.4.10 / §13 AR-7) populated and rotated in P01? PRD §10.1 says Encore Cloud free-tier secret manager. The plan's §3.2 defers `memory` service code to P02, so P01 *could* skip the secret entirely — but `auth.oauth_tokens.access_token_enc` and `providers.installations.installation_id_enc` use the same DEK and are P01 schemas. **Decision needed:** does P01 ship with `MEMORY_DEK` provisioned (even if no rows are written), or is it a P02 ops item?
- **OQ3.** Does Encore Cloud's free-tier Postgres support `pgcrypto` and `pg_trgm`? Both are in the standard PG `contrib` distribution and almost universally available on managed PG, but Encore's docs do not enumerate available extensions. **Decision needed:** confirm via local emulator before spec ships.
- **OQ4.** SPEC §9.4.6 declares `key_hash bytea` — if we move to SHA-256 (per C7), is the column type still `bytea` (32 bytes raw) or `text` (hex / base32-encoded)? **Recommend:** `bytea NOT NULL` (32 bytes, raw — smallest, fastest, no encoding ambiguity).

---

## Recommendation

**RESOLVE CONTRADICTIONS BEFORE SPEC.** Six **CONTRADICTED** findings (C1, C2, C3, C5, C6, C7) and three **PARTIAL** findings (R-P01-2, R-P01-4, R-P01-7) require Ada's resolution before `/spec` can write JSON-locked tool/RPC contracts. C1 and C7 in particular alter the data model (delivery_id source, hash algorithm), and C6 alters the public surface (legacy SSE vs Streamable HTTP).

**OK to proceed with:** R-P01-9 (CONFIRMED) and R-P01-6 (PARTIAL but resolvable in spec by pinning the assumption).

**Deferred to P02 ops:** Encore Cloud edge-proxy timeout (C4 mitigation), GitHub App webhook subscription path (D12).

**Suggested order of resolution in `/spec`:**
1. C2 (migration ownership) — picks the bootstrap service.
2. C6 (Streamable HTTP vs legacy SSE) — picks the public surface shape.
3. C7 (hash algorithm) — picks the auth hot-path performance budget.
4. C1 (Pub/Sub idempotency mechanism) — picks the publisher/subscriber contract.
5. C5 (recursive CTE depth counter) — rewrites the CTE.
6. C3 (Go MCP SDK) — pins the dependency.
7. AF1 (workitems FTS) and AF5 (cycle-detection locking) — augments the DDL.
