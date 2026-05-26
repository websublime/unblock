// Package exitcriteriontest is the end-to-end exit-criterion test
// harness for the ://unblock Backend MVP (phase P01).
//
// Scope (SPEC §11.1):
//
//   - §11.1.0 Fixture: the canonical 5-item dependency graph
//     (itm_a..itm_e, project prj_exit, org org_exit_criterion, user
//     usr_alice) is materialised in TestMain via direct
//     `encore.dev/storage/sqldb` writes. Identifiers in §11.1.0 are
//     illustrative labels — the seed mints fresh ULIDs at runtime
//     (same constraint as `apps/api/shared/rbactest/seed.go`).
//   - §11.1.1 Seed ownership: the test owns its own seed via TestMain
//     and direct INSERTs (NOT through auth/org RPCs). The
//     `mcp.api_keys` row is inserted via direct SQL with `key_hash`
//     computed using `secrets.APIKeyHMACSecret` per the production
//     hashing in `apps/api/auth/apikey.go:103-111` (HMAC-SHA256 over
//     the raw key, 32-byte digest stored as bytea). The raw key value
//     is held in memory by the test goroutine and used as the
//     `Bearer` token in the RPC assertions. The test never calls
//     `auth.IssueAPIKey` — direct INSERT is the seed contract.
//   - §11.1.2 Functional assertions: the harness drives the 14 MCP
//     tools via JSON-RPC over Streamable HTTP, asserts state
//     transitions, cascade kinds, cycle detection, milestone
//     invariants, and the five state-machine invariants (I-1..I-5).
//   - §11.3 Architectural invariants: single-writer is_ready /
//     pipeline_stage (Regime A vs Regime B), cascade idempotency
//     under N=100 re-deliveries, atomic claim under N=100 concurrent
//     attempts, cycle detection under N=100 random graph mutations,
//     and the Manifesto Laws coverage check.
//
// Why this is an Encore service (//encore:service anchor in service.go).
//
// The Encore parser enforces invariant E1388 ("APIs can only be
// called from within a service"). Without a //encore:service anchor
// here, calls to workitems.CreateMilestone / workitems.AssignItem /
// workitems.MilestoneTree (which the §11.1.2 milestones checkpoint
// requires going through Encore's private mesh per SPEC §11.1.1) fail
// the parser check before any test runs. The exit-criterion harness
// is a cross-service integration suite that invokes the canonical
// milestone RPCs on the production code path — it MUST be a
// parser-visible service for those calls to be legal. Same shape as
// `apps/api/shared/rbactest/service.go`.
//
// The package still produces no //encore:api endpoints. The Service
// struct + initService are pure parser anchors. No state, no
// lifecycle hooks beyond the no-op initService.
//
// Bearer auth + tool dispatch transport.
//
// Tool surface assertions drive the 14 MCP tools via JSON-RPC POSTs
// against `httptest.NewServer(http.HandlerFunc(mcp.ServeMCPForTest))`.
// The natural test path — HTTP against `encore.Meta().APIBaseURL +
// "/mcp"` — does not work under `encore test`: Encore's in-process
// listener does not route raw //encore:api routes (the A-5 DEVIATION
// recorded in `apps/api/shared/mcpaudittest/d1_transport_test.go`
// lines 17-27, still present in encore.dev v1.52.1). The
// `mcp.ServeMCPForTest` hook re-exports the production `serveMCP`
// implementation under a `ForTest` suffix; the wrapped handler
// exercises the identical code path that production traffic hits.
//
// Cascade subscriber test invocation (round-13 contract).
//
// Encore Pub/Sub subscriptions DO NOT fire under `encore test` (the
// test harness records published messages on
// `et.Topic(...).PublishedMessages()` but never delivers them to the
// subscriber goroutine). To make the §11.1.2 / §11.3 row-level
// assertions on `deps.cascade_events` reachable, the harness drives
// the subscriber directly via `deps.HandleCascadeRequestedForTest`
// (apps/api/deps/export_test_handler.go) — a thin pass-through to the
// production `handleCascadeRequested`. Same convention as
// `mcp.ServeMCPForTest`. Per the four-step invocation pattern in SPEC
// §11.1.1:
//
//  1. Invoke the producing RPC through the normal MCP / private-mesh path.
//  2. Capture `et.Topic(deps.CascadeRequestedTopic).PublishedMessages()`.
//  3. For each captured message invoke `HandleCascadeRequestedForTest`
//     exactly once to materialise the audit row(s) and apply the
//     pipeline_stage updates.
//  4. Assert the row(s) per §11.1.2.
//
// Note on is_ready and "After cascade" wording in §11.1.2.
//
// SPEC §11.1.2 says "After cascade, prime reflects newly unblocked
// dependents (itm_c, itm_d flip to ready)". In the codebase the
// is_ready flip is INLINE in `workitems.Close` via
// `deps.RecomputeReadyForBlocksDownstream` (workitems.go:1424; SPEC
// §6.3.0 lines 1691-1692; §11.3 bullet (b)). The cascade subscriber
// does NOT write is_ready (it only writes pipeline_stage — the
// fractured single-writer invariant per round-6 §6.3.0). Therefore
// `prime` reflects itm_c/itm_d as ready immediately after
// `workitems.Close` commits, without any subscriber firing. The
// §11.1.2 phrasing "After cascade" is Regime-A-inline and
// self-satisfying — NOT a drift, but easily misread. Future readers
// landing on the relevant test body should NOT try to "fix" the
// apparent ordering by gating the assertion behind the subscriber.
//
// Encore-runtime requirement.
//
// This package MUST be executed under `encore test
// ./apps/api/exitcriteriontest/...`. Plain `go test` does NOT bring
// up the Encore-managed Postgres cluster, does NOT fire the dedicated
// `apps/api/db/` service's init() (which is what calls the BindDB
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
// per-service BindDB hooks) are not goroutine-safe; bead unblock-tv8.34
// tracks the hardening discussion. The N=100 property tests use
// goroutines internally where the assertion requires concurrent
// execution (concurrent claim, cycle property test); the goroutine
// fan-out is scoped to a single subtest and is the documented shape
// in `apps/api/workitems/integration_test.go` /
// `apps/api/deps/integration_test.go`.
//
// `-race` is intentionally NOT enabled on the gate set (SPEC §11.2
// NFR-10 round-11 changelog: `encore test ... -race` reproducibly
// SIGSEGVs inside encore-go's `lazyTraceInit.initStream` goroutine
// spawn — encoredev/encore#1943, open ~1 year). Leaf-package race
// coverage lives in `apps/api/shared/ulid/`,
// `apps/api/shared/rbac/`, `apps/api/shared/lint/`.
package exitcriteriontest
