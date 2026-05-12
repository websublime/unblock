// Package dbhandle owns the canonical sqldb.NewDatabase declaration
// for the `unblock` Postgres database and binds the constructed
// *sqldb.Database into every consumer that needs it (the auth service
// itself via auth.BindDB; the shared rbac builder via rbac.Bind) at
// package init. It is a leaf wiring sub-package: no business logic, no
// API handlers, no exported test surface — just the infrastructure
// declaration and the single init that wires the handle into both
// consumers.
//
// Why this package exists (bead unblock-xuk).
//
// The auth service is the sole migration-owner per SPEC §3.1, which
// historically meant the literal `var db = sqldb.NewDatabase("unblock",
// sqldb.DatabaseConfig{Migrations: "./migrations"})` lived in
// apps/api/auth/db.go. Encore's sqldb.NewDatabase is a runtime-stub
// function: outside the encore CLI's cluster bring-up it panics
// unconditionally with "encore apps must be run using the encore
// command". That panic fires from Go's package init the moment any
// plain `go test ./apps/api/auth/...` loads the auth package, so even
// the leaf-package helpers that never touch the database (prefixOf,
// hashRawKey, parseBearer, splitScopes, ...) could not be unit-tested
// without Docker. The same hazard hid bead unblock-t47's TestPrefixOf
// regression through the entire B-1 (unblock-tv8.7) review/QA pipeline
// because Quinn could not actually load the package locally.
//
// The fix is structural: relocate the NewDatabase call into a sibling
// sub-package that the parent `auth` package does not import. Plain
// `go test ./auth/...` builds a test binary for the auth root package
// only; it does not pull in dbhandle's init, so the panic never fires
// and the leaf-package tests run. The Encore runtime, by contrast,
// auto-discovers dbhandle through the same static analysis that
// registers every other infrastructure resource (sqldb.NewDatabase
// calls are scanned across the entire module — see Encore's parser at
// the runtime version pinned in apps/api/go.mod), and loads dbhandle's
// init before any //encore:authhandler can dispatch, which is what
// makes BindDB and rbac.Bind fire before the first handler reads the
// auth package's `db` variable or invokes rbac.For.
//
// Init ordering note. Go runs init for an imported package before the
// importer's init. dbhandle imports both `encore.app/auth` and
// `encore.app/shared/rbac`; their inits therefore complete before
// dbhandle.init runs. apps/api/auth/initbind.go was the historical
// home of `rbac.Bind(db)` — that call has moved here because
// auth.initbind ran before dbhandle could populate auth.db, so the
// historical sequence would have bound rbac with nil. Centralising
// both bind sites in dbhandle.init keeps the wiring obvious: one
// place owns NewDatabase, one place hands the resulting handle to
// every consumer that needs it.
//
// Constraint that drove the package layout (parser error E1796):
// Encore's parser rejects sqldb.NewDatabase calls whose Migrations
// path is not rooted within the calling package's directory.
// Migrations therefore live at apps/api/auth/dbhandle/migrations/, not
// the historical apps/api/auth/migrations/. The migration filenames
// (0010_bootstrap.up.sql .. 0090_memory.up.sql) and the registered
// database name (`"unblock"`) are unchanged, so SPEC §3.1's
// auth-is-sole-migration-owner invariant and every `sqldb.Named("unblock")`
// consumer continue to bind correctly. The literal `apps/api/auth/migrations/`
// path documented in SPEC §3.1 is the one piece of drift that lands
// with this bead — the directory move is logged as a deviation on the
// bead and SPEC §3.1 will be updated by a follow-up doc patch.
//
// Constraints (DO NOT VIOLATE):
//
//   - This package MUST NOT contain unit tests (no *_test.go files).
//     Adding tests would reintroduce the exact panic this package was
//     created to avoid: `go test ./auth/dbhandle/...` would build a
//     test binary that loads the package, run init, hit
//     sqldb.NewDatabase, and panic. DB-bound tests for the auth
//     service belong under the auth root package and run via
//     `encore test ./auth/...`.
//
//   - This package MUST NOT acquire other responsibilities. Beyond
//     the NewDatabase declaration and the bind calls below, nothing
//     belongs here — keep dbhandle leaf-shaped so the parent `auth`
//     package's no-import-of-dbhandle property is obvious to a reader.
//
//   - The first argument to sqldb.NewDatabase MUST stay the literal
//     `"unblock"`. It is the registered database name every other
//     service binds to via `sqldb.Named("unblock")` (org, workitems,
//     deps, mcp, providers, boards, memory; SPEC §3.1). Renaming
//     would silently break every cross-service handle.
package dbhandle

import (
	"encore.app/auth"
	"encore.app/shared/rbac"
	"encore.dev/storage/sqldb"
)

// DB is the canonical *sqldb.Database for the `unblock` Postgres
// database. The auth root package receives the handle via
// auth.BindDB(DB) in init below; the shared rbac builder receives it
// via rbac.Bind(DB). Consumers outside the auth service continue to
// obtain their own handle via sqldb.Named("unblock") and never import
// this package.
//
// Exported only so future tooling can spot-check the same handle the
// init binds — there is no consumer outside this package today.
//
// The package-level form is mandated by sqldb.NewDatabase's
// doc-comment ("a call to NewDatabase can only be made when declaring
// a package level variable").
//
//nolint:gochecknoglobals
var DB = sqldb.NewDatabase("unblock", sqldb.DatabaseConfig{
	Migrations: "./migrations",
})

// init binds the unblock database handle into both the auth service
// (auth.BindDB) and the shared rbac builder (rbac.Bind) so the auth
// package's `db` variable and the rbac package's internal handle are
// both non-nil before any //encore:authhandler dispatches or any
// rbac.For caller runs. Encore's process-bootstrap import order
// guarantees this init runs before any handler can call Validate /
// IssueAPIKey / RevokeAPIKey / ExchangeOAuthCode or any cross-service
// consumer (org, workitems via apps/api/shared/rbac) reaches a
// rbac.For call site.
//
// Concurrency: BindDB and rbac.Bind are not goroutine-safe. The
// single-init contract was previously documented on
// apps/api/auth/initbind.go and on apps/api/shared/rbac/rbac.go; bead
// unblock-tv8.34 tracks the parallel hardening discussion for both
// setters. Until that lands, the runtime guarantee is "Encore process
// init runs serially and completes before the first handler".
//
// init is the only service-bootstrap wiring point for the unblock
// database handle; the pattern mirrors apps/api/auth/initbind.go's
// historical rbac.Bind(db) call (now relocated here — see the
// package-level doc-comment on the init-ordering reasoning) and is
// tracked on bead unblock-tv8.34.
//
//nolint:gochecknoinits
func init() {
	auth.BindDB(DB)
	rbac.Bind(DB)
}
