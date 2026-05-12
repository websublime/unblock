// db.go declares the package-level *sqldb.Database handle for the auth
// service and the BindDB hook used by the sibling apps/api/auth/dbhandle
// package to inject the canonical handle at process start.
//
// History (bead unblock-xuk). The previous shape declared the handle
// inline as `var db = sqldb.NewDatabase("unblock", ...)`. Encore's
// sqldb.NewDatabase doc-comment requires that call to occur in a
// package-level variable declaration (the runtime stub panics
// unconditionally if the call is reached without the encore CLI's
// cluster bring-up; the parser also rejects the call inside a function
// body — see the doc-comment on NewDatabase in encore.dev/storage/sqldb
// for both constraints). Combined, these two rules meant any plain
// `go test ./apps/api/auth/...` triggered Go package init, fired
// sqldb.NewDatabase, and panicked with "encore apps must be run using
// the encore command" before any test could run. The same hazard hid a
// real fixture-drift regression (TestPrefixOf, bead unblock-t47)
// through the entire B-1 (unblock-tv8.7) review/QA pipeline because
// QA could not actually load the package locally.
//
// The fix relocates the NewDatabase call into the leaf sub-package
// `apps/api/auth/dbhandle/` (alongside the migration set), and exposes
// BindDB here so dbhandle.init() can inject the constructed handle.
// Plain `go test ./auth/...` builds a test binary for the auth package
// only — it does not import dbhandle, so no NewDatabase call fires at
// init and the leaf-package tests load and execute. The Encore
// runtime, by contrast, auto-discovers the dbhandle package via
// `sqldb.NewDatabase` static analysis (the same mechanism that makes
// Encore aware of every other infrastructure resource) and loads it
// before any //encore:authhandler dispatches, which is what makes
// BindDB fire before the first handler reads `db`.
//
// SPEC §3.1 invariant preserved: the auth service remains the sole
// migration-owner; sqldb.NewDatabase("unblock", ...) is still called
// exactly once across the workspace, now from
// apps/api/auth/dbhandle/dbhandle.go (was apps/api/auth/db.go before
// unblock-xuk). The migration set lives at
// apps/api/auth/dbhandle/migrations/ — the directory move is the
// trade-off Encore's "migration path must be local to the package"
// constraint forces (parser error E1796 if migrations stays at
// apps/api/auth/migrations/ while NewDatabase moves into the
// sub-package). Encore's per-service DB binding via sqldb.Named
// continues to work for every consumer service (org, workitems, deps,
// mcp, ...) because the registered database name `"unblock"` is
// unchanged.
//
// Hot-path impact: zero. After dbhandle.init() runs (once, during
// Encore process bootstrap) `db` is a non-nil *sqldb.Database for the
// lifetime of the process. There is no per-request synchronisation,
// no sync.Once.Do post-completion atomic load, no atomic.Pointer
// load — every Validate / IssueAPIKey / RevokeAPIKey / ExchangeOAuthCode
// invocation reads `db` as a plain Go variable. SPEC §4.3.2's <5 ms
// p99 budget is unaffected.
//
// Test-time consequences (bead unblock-xuk closure):
//   - `go test ./apps/api/auth/...` loads the package without panicking
//     and runs the leaf-package tests (apikey, oauth, ulid, parseBearer
//     subtests of authhandler_test.go).
//   - `go test ./apps/api/auth/dbhandle/...` would panic if dbhandle
//     contained any tests — it does not, by design (dbhandle is pure
//     infrastructure wiring with no logic worth unit-testing in
//     isolation).
//   - `encore test ./auth/...` continues to work under the Encore
//     CLI's Docker-backed test runner (the cluster bring-up satisfies
//     sqldb.NewDatabase's runtime requirement).
//
// Tracked risk: BindDB is not goroutine-safe. The contract is identical
// to rbac.Bind (apps/api/shared/rbac/rbac.go): it runs exactly once
// during process bootstrap, before any handler can invoke Validate /
// IssueAPIKey / etc. Bead unblock-tv8.34 already tracks the parallel
// hardening discussion for rbac.Bind (sync.Once / atomic.Pointer) and
// is the appropriate follow-up if a concurrent-init guarantee becomes
// load-bearing here. Until then, the Encore process-init order is the
// guarantee.

package auth

import "encore.dev/storage/sqldb"

// db is the canonical handle for the `unblock` Postgres database. It is
// a package-level pointer that starts nil and is populated exactly once
// by BindDB during process bootstrap (called from
// apps/api/auth/dbhandle/dbhandle.go's init function). Every RPC body
// in this package reads `db` directly — there is no per-call lookup.
//
//nolint:unused // referenced by RPC bodies starting in beads B-1..D-3.
var db *sqldb.Database

// BindDB installs the unblock-database handle. The companion
// apps/api/auth/dbhandle package owns the sqldb.NewDatabase call and
// invokes BindDB from its package init. Called exactly once per
// process; subsequent calls overwrite the handle (test fixtures may
// rely on this for swap-in of fakes — same contract as rbac.Bind).
//
// Concurrency: not goroutine-safe. See the file-level doc-comment for
// the runtime guarantee (Encore's process-bootstrap import order
// guarantees BindDB completes before any //encore:authhandler can run).
//
// This function is intentionally exported so the wiring sub-package
// can call it without re-declaring `db` or duplicating the
// infrastructure. The parent `auth` package still owns `db`; dbhandle
// only sources the value.
func BindDB(d *sqldb.Database) { db = d }
