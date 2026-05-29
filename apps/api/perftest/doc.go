// Package perftest is the NFR-1 latency harness for the ://unblock
// Backend MVP (phase P01, bead unblock-tv8.24 / E-2).
//
// Scope (SPEC §11.2 NFR-1, round-14).
//
// The harness drives the warm-cache `prime → ready → claim` sequence
// end-to-end against the local Encore emulator and computes the p99
// wall-clock latency of one full sequence. The contract:
//
//   - p99 < 2 s on the local Encore emulator with a warm cache.
//   - Warm cache means: (a) the Postgres connection pool is
//     established, (b) the API key is validated once before the timer
//     starts, and (c) no first-request cold-start outliers — M ≥ 10
//     warm-up iterations are discarded before measurement begins.
//   - The harness ALWAYS logs the per-call latency samples, the
//     computed p99, and the goroutine deltas as JSON-Lines via
//     t.Logf (informative on every run — aligns with the AC verb
//     "reports").
//   - A hard-fail (t.Fatalf on p99 ≥ 2 s OR drained-baseline > 20) is
//     gated by the UNBLOCK_PERF_GATE=1 environment variable so CI
//     stays advisory on slow shared GH Actions runners. Release-
//     blocking pipeline wiring is a P02 ops item owned by Olive.
//
// Cloud measurement is explicitly a P02 ops item — this harness only
// covers the local emulator.
//
// Seeding doctrine (SPEC §11.2 + §11.1.1 round-12).
//
// The harness owns its fixture via direct `encore.dev/storage/sqldb`
// writes — no auth/org RPCs in the seed path. The org/project slug is
// salted with a shortULID to avoid dev-cluster collision (the suite
// runs in the same dev cluster as exitcriteriontest under
// `encore test ./...`). In-test key issuance uses a direct
// `INSERT INTO mcp.api_keys` with `key_hash` computed against
// `secrets.APIKeyHMACSecret` per the production hashing in
// `apps/api/auth/apikey.go`. Precedent: `apps/api/exitcriteriontest/seed.go`.
//
// Ready-item consumption (R4 — exhaustion mitigation).
//
// Each measured `claim` consumes one ready row (claim flips
// status='Ready' → 'InProgress' and sets claimed_by_id, so the row
// leaves the ready set permanently). The harness therefore seeds
// N = 2 × iterations ready rows so every measured claim — across the
// warm-up loop AND the measurement loop — consumes a fresh row without
// re-using or un-claiming. We deliberately do NOT un-claim rows
// between iterations: an UPDATE that resets claimed_by_id would touch
// the production single-writer claim surface and is harder to reason
// about than simply over-seeding. The factor of 2 covers the warm-up
// iterations (M ≥ 10) plus the measurement iterations with headroom.
//
// W3 closure (negative auth paths) — auth_negative_test.go.
//
// The B-1 auth-service review (closed, cross-linked on bead
// unblock-tv8.24) left the four DB-bound auth RPC bodies verified only
// by inspection. auth_negative_test.go closes that gap: it exercises
// the §4.3.2 negative paths against the same httptest server — revoked
// key, expired key, unknown prefix, bad HMAC, and missing
// `unblock_pat_` prefix.
//
// Wire-signal note (DEVIATION on bead unblock-tv8.24). The bead AC and
// spec §11.2 phrase the negative-path assertion as "401 /
// errs.Unauthenticated". The MCP Streamable HTTP transport NEVER
// returns HTTP 401 — auth failures always return HTTP 200 with a
// JSON-RPC 2.0 error envelope whose error.code == -32000 and
// data.kind == "UNAUTHENTICATED" (apps/api/mcp/errenvelope.go:27-30,
// 179-182; apps/api/mcp/mcp.go:178-214). The d1 transport precedent
// (apps/api/shared/mcpaudittest/d1_transport_test.go:351-427) already
// establishes this as the canonical auth-rejection signal at this
// transport. auth_negative_test.go asserts the UNAUTHENTICATED
// envelope — the faithful realisation of "errs.Unauthenticated" at the
// HTTP/MCP edge.
//
// W4 closure (goroutine drain check).
//
// The Bearer hot path fires `go touchLastUsedAt(id)` per request with
// a 1 s context cap (apps/api/auth/auth.go:217,248-256); under load
// this can pile up. The harness samples `runtime.NumGoroutine` three
// times — `baseline` (before warm-up), `peak` (immediately after the
// measurement loop), and `drained` (after a 2 s post-loop sleep,
// giving the 1 s cap two cycles to expire). Assertion:
// drained - baseline ≤ 20 (absolute margin for runtime/SDK overhead).
// The harness is the leak ALARM; the RS01-4 LRU-cache mitigation
// remains the fix and is tracked separately.
//
// Cascade / Pub/Sub note (R3).
//
// The measured `prime → ready → claim` sequence does NOT exercise
// `workitems.Close`, so no `CascadeRequested` message is published and
// no cascade subscriber goroutine is involved. The goroutine drain
// check therefore observes only the touchLastUsedAt fire-and-forget
// population, which is exactly the W4 leak vector under test.
//
// Why this is an Encore service (//encore:service anchor in service.go).
//
// The Encore parser enforces invariant E1388 ("APIs can only be called
// from within a service"). This package does not call private RPCs
// directly (it drives everything through the MCP httptest transport),
// but the //encore:service anchor keeps the package shape identical to
// exitcriteriontest and guards against future direct RPC calls
// tripping the parser. The package produces NO //encore:api endpoints;
// the Service struct + initService are pure parser anchors.
//
// Encore-runtime requirement.
//
// This package MUST be executed under
// `encore test ./apps/api/perftest/...`. Plain `go test` does NOT
// bring up the Encore-managed Postgres cluster, does NOT fire the
// dedicated `apps/api/db/` service's init() (which calls the BindDB
// hook chain), and therefore leaves every service's *sqldb.Database
// pointer nil. The Encore secret `APIKeyHMACSecret` (declared on the
// `secrets` struct in secrets.go) is read at the same Encore-bootstrap
// step; under plain `go test` the secret resolves to the empty string
// and the HMAC computed by the seed will never match the production
// hot-path comparison.
//
// Concurrency.
//
// The suite does NOT call t.Parallel anywhere. rbac.Bind (and the
// per-service BindDB hooks) are not goroutine-safe. `-race` is
// intentionally NOT enabled on the gate set (SPEC §11.2 NFR-10
// round-11 changelog: `encore test ... -race` reproducibly SIGSEGVs
// inside encore-go's lazyTraceInit.initStream goroutine spawn —
// encoredev/encore#1943).
package perftest
