// db.go declares the package-level *sqldb.Database handle for the auth
// service and the BindDB hook used by the dedicated apps/api/db/
// service to inject the canonical handle at process start.
//
// History (beads unblock-xuk, unblock-bne).
//
// Round 1 (unblock-xuk): the original shape declared the handle inline
// here as `var db = sqldb.NewDatabase("unblock", ...)`. Encore's
// sqldb.NewDatabase doc-comment requires that call to occur in a
// package-level variable declaration, and the runtime stub panics
// unconditionally if it is reached without the encore CLI's cluster
// bring-up. Combined, those two rules meant any plain
// `go test ./apps/api/auth/...` triggered Go package init, fired
// sqldb.NewDatabase, and panicked with "encore apps must be run using
// the encore command" before any test could run. The same hazard hid
// the unblock-t47 fixture-drift regression (TestPrefixOf) through the
// entire B-1 (unblock-tv8.7) review/QA pipeline because QA could not
// actually load the package locally. unblock-xuk fixed this by
// relocating the NewDatabase call into a sibling sub-package the auth
// root package did not import (auth/dbhandle/) and exposing the
// BindDB hook here so the sub-package's init() could inject the
// constructed handle.
//
// Round 2 (unblock-bne): the dbhandle relocation kept the panic out
// of the auth init path but it left the auth service nominally
// owning the canonical migration set for every schema in the system
// (SPEC §4.1 previously read "Owns: schema auth,
// apps/api/auth/migrations/ — the canonical migrations directory for
// the whole DB"). That was accidental coupling — the auth service's
// surface was tied to the lifecycle of org / workitems / deps / etc.
// DDL. unblock-bne completed the decoupling by extracting BOTH the
// `sqldb.NewDatabase` declaration AND the migration set into a
// dedicated `apps/api/db/` service whose only purpose is to own the
// database resource. The BindDB hook stayed (it is the channel the
// new db service uses to populate this nil *sqldb.Database without
// re-introducing the import that would re-fire the encore-CLI panic
// at this package's init); the difference is which package now owns
// the NewDatabase call.
//
// Why the BindDB shape stays (unblock-xuk invariant preserved) —
// and why it is now the CANONICAL pattern for every domain service.
//
// An alternative shape considered during unblock-bne was to convert
// this file to `var db = sqldb.Named("unblock")` (mirroring the
// then-current apps/api/org/db.go) and have the new apps/api/db/
// service ship only the NewDatabase declaration + migrations.
// Empirical encore-CLI check disproved the premise: `sqldb.Named` is
// NOT a benign runtime lookup — its v1.52.1 implementation calls
// doPanic at package-load time exactly like NewDatabase. Plain
// `go test ./auth/...` against that shape panics at package init
// the same way the original unblock-xuk-broken shape did. See the
// DECISION comment on bead unblock-bne for the full empirical
// trace.
//
// bne's pre-review scope expansion completed the symmetry: the org
// service was converted from the eager `sqldb.Named("unblock")`
// shape to the same BindDB late-bind hook documented here, and
// every future domain service that touches the unblock database
// (workitems, deps, mcp, providers, boards, memory) MUST follow the
// same shape. The dedicated apps/api/db/ service binds every domain
// service's nil handle from its single central init, eliminating
// per-service initbind.go files and standardising the consumer
// pattern across the codebase.
//
// SPEC §3.1 invariant preserved: there is still exactly ONE
// `sqldb.NewDatabase("unblock", ...)` call across the workspace —
// it now lives in apps/api/db/db.go. The registered database name
// `"unblock"` is unchanged, so every cross-service binding via
// `sqldb.Named("unblock")` continues to work transparently.
//
// Hot-path impact: zero. After apps/api/db/db.go's init() runs (once,
// during Encore process bootstrap) `db` is a non-nil *sqldb.Database
// for the lifetime of the process. There is no per-request
// synchronisation, no sync.Once.Do post-completion atomic load, no
// atomic.Pointer load — every Validate / IssueAPIKey / RevokeAPIKey
// / ExchangeOAuthCode invocation reads `db` as a plain Go variable.
// SPEC §4.3.2's <5 ms p99 budget is unaffected.
//
// Test-time consequences (unblock-xuk DB-handle invariant preserved;
// secret invariant DELIBERATELY SUPERSEDED by unblock-tv8.57):
//
// The unblock-xuk fix kept the `sqldb.NewDatabase` panic out of the auth
// root package's init path: the auth root never imports apps/api/db/, so
// the (now relocated) NewDatabase call does not fire during plain-go-test
// bring-up. That DB-handle invariant is still intact — BindDB late-bind is
// unchanged.
//
// However, the auth root package is NO LONGER plain-`go test`-loadable.
// unblock-tv8.57 added a boot-time fail-fast init() in auth/secrets.go that
// panics when secrets.MemoryDEK, GitHubOAuthClientID, or
// GitHubOAuthClientSecret are empty — mirroring the unconditional secret
// guard the mcp service applies in mcp/transport.go's init. Under plain
// `go test ./apps/api/auth/...` those Encore secrets resolve to "" (no
// cluster, no .secrets.local.cue overlay), so auth.init() panics by design.
//
// WHY this trade was made (bead unblock-tv8.57): deploy-time fail-fast is
// worth more than go-test-without-Docker ergonomics. Before the guard, an
// empty MemoryDEK / GitHubOAuth* secret resolved silently and failed deep on
// a hot path — MemoryDEK at the first pgcrypto encrypt of an oauth_tokens
// row (auth.go:459), the GitHubOAuth* pair as an opaque remote 400/401 in
// the code exchange (auth.go:326). Those late, confusing failure modes are
// exactly what the unblock-tv8 exit-criterion OAuth E2E would have hit. The
// init() makes all four required secrets surface uniformly as a startup
// crash with an actionable message, which is the operator experience the
// mcp guard already provides. The go-test-loads convenience for the auth
// ROOT package was the accepted casualty.
//
// Consequences in practice:
//   - `encore test ./auth/...` is the canonical gate for the auth root
//     package. The Encore CLI populates the secrets from
//     apps/api/.secrets.local.cue and brings up the Docker cluster, so the
//     new init() does NOT fire and sqldb.NewDatabase's runtime requirement
//     is satisfied. The apikey, oauth, and AuthHandler input-error subtests
//     run there. (Bearer parsing moved to apps/api/shared/httpauth.)
//   - `go test ./apps/api/auth/...` now panics at package init on the empty
//     secret — this is the intended, accepted outcome, NOT a regression.
//   - The leaf sub-package `apps/api/auth/types/` REMAINS plain-`go test`-
//     clean: it imports neither the auth root nor any encore.dev package,
//     so neither the NewDatabase call nor the new secret-guard init() is in
//     its load path. Consumers needing only auth.Identity import
//     `encore.app/auth/types` and stay Docker-free. Do not weaken that.
//   - `go test ./apps/api/db/...` would panic if apps/api/db/ contained any
//     tests — it does not, by design (pure infrastructure wiring; see its
//     package doc-comment).
//
// Tracked risk: BindDB is not goroutine-safe. The contract is
// identical to rbac.Bind (apps/api/shared/rbac/rbac.go): it runs
// exactly once during process bootstrap, before any handler can
// invoke Validate / IssueAPIKey / etc. Bead unblock-tv8.34 already
// tracks the parallel hardening discussion for rbac.Bind (sync.Once
// / atomic.Pointer) and is the appropriate follow-up if a
// concurrent-init guarantee becomes load-bearing here. Until then,
// the Encore process-init order is the guarantee.

package auth

import "encore.dev/storage/sqldb"

// db is the canonical handle for the `unblock` Postgres database. It is
// a package-level pointer that starts nil and is populated exactly once
// by BindDB during process bootstrap (called from apps/api/db/db.go's
// init function — the dedicated migration-owner service per SPEC §3.1).
// Every RPC body in this package reads `db` directly — there is no
// per-call lookup.
//
//nolint:unused // referenced by RPC bodies starting in beads B-1..D-3.
var db *sqldb.Database

// BindDB installs the unblock-database handle. The companion
// apps/api/db package owns the sqldb.NewDatabase call and invokes
// BindDB from its package init. Called exactly once per process;
// subsequent calls overwrite the handle (test fixtures may rely on
// this for swap-in of fakes — same contract as rbac.Bind).
//
// Concurrency: not goroutine-safe. See the file-level doc-comment for
// the runtime guarantee (Encore's process-bootstrap import order
// guarantees BindDB completes before any //encore:authhandler can run).
//
// This function is intentionally exported so the dedicated db service
// can call it without re-declaring `db` or duplicating the
// infrastructure. The auth package still owns `db`; the db service
// only sources the value.
func BindDB(d *sqldb.Database) { db = d }
