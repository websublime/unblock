// Package db is the dedicated, zero-API migration-owner service for the
// canonical `unblock` Postgres database. It declares
// `sqldb.NewDatabase("unblock", ...)` exactly once across the workspace,
// ships the canonical migration set under apps/api/db/migrations/, and
// binds the constructed handle into every consumer that needs it at
// package init (the auth service via auth.BindDB; the shared rbac
// builder via rbac.Bind). Every domain service (auth, org, workitems,
// deps, mcp, providers, boards, memory) is an equal database consumer;
// no domain service owns DDL for schemas it does not consume.
//
// Why this package exists (bead unblock-bne).
//
// Through bead unblock-xuk the NewDatabase call lived inside
// apps/api/auth/dbhandle/. That kept the call out of the auth root
// package's init path (so plain `go test ./auth/...` no longer
// panicked) but it left the auth service still owning DDL for every
// schema in the system through the migration set's location. SPEC
// §4.1's surface description literally read "Owns: schema auth,
// apps/api/auth/migrations/ (the canonical migrations directory for
// the whole DB)" — which coupled the auth service's surface to the
// lifecycle of org / workitems / deps / mcp / providers / boards /
// memory DDL. unblock-bne breaks that coupling: the
// `var DB = sqldb.NewDatabase(...)` declaration and all nine migration
// files relocate here, into a service whose only purpose is to own the
// database resource. Every domain service is now an equal consumer of
// the result.
//
// Encore service-shape.
//
// This package has zero //encore:api endpoints, no //encore:service
// struct, and no exported test surface. The Encore parser auto-
// discovers it via the same static-analysis sweep that registers
// every other infrastructure resource: the literal
// `sqldb.NewDatabase("unblock", ...)` call in this file is what makes
// the migration set known to the runtime, regardless of whether any
// //encore:api surface exists in the package. This is the same
// mechanism the now-deleted apps/api/auth/dbhandle/ relied on; only
// the package location and the implied ownership story have changed.
//
// Wiring shape (Shape A from the bead investigation, validated
// empirically against the encore CLI).
//
// db.init() centralises the cross-cutting binds:
//
//   - auth.BindDB(DB) populates the auth service's nil *sqldb.Database
//     handle. Auth keeps the unblock-xuk late-bind shape (a nil
//     package-level pointer + an exported BindDB hook) because the
//     auth root package MUST NOT import any package that calls
//     sqldb.NewDatabase or sqldb.Named at its own package init —
//     either call panics under plain `go test ./auth/...` outside the
//     encore CLI (sqldb.Named is NOT a benign runtime lookup; its
//     v1.52.1 implementation calls doPanic the same way NewDatabase
//     does). The BindDB hook is the only shape that preserves the
//     xuk goal (plain `go test ./auth/...` loads without panic) while
//     moving the NewDatabase declaration out of the auth tree.
//
//   - rbac.Bind(DB) installs the shared rbac builder's handle. Every
//     consumer service also calls rbac.Bind in its own initbind.go
//     (apps/api/org/initbind.go today; future workitems / deps / mcp
//     initbind files when those services land). The double-bind is
//     documented-safe (rbac.go's Bind contract: "Subsequent calls
//     overwrite the handle; tests rely on this for swap-in of
//     fakes") and gives defense-in-depth: db's bind is the single
//     central wiring point; each service's local bind closes the
//     cross-service init-order gap that org/initbind.go's header
//     describes ("Encore guarantees per-service init() runs before
//     any //encore:api dispatch, but cross-service initialisation
//     order is not specified").
//
// Init-order analysis.
//
// Go guarantees imported packages' init runs before the importer's
// init. db imports encore.app/auth and encore.app/shared/rbac;
// auth.init and rbac.init therefore complete before db.init. Encore
// loads infrastructure-resource consumers (db.init) during process
// bootstrap, before any //encore:api handler dispatches. The
// dbhandle.init precedent (shipped to PROD under unblock-tv8.7
// before bead unblock-bne) proves the runtime contract; this package
// only relocates the same init from apps/api/auth/dbhandle/ to
// apps/api/db/ and adopts the same wiring shape.
//
// Constraints (DO NOT VIOLATE):
//
//   - This package MUST NOT contain unit tests (no *_test.go files).
//     Adding tests would reintroduce the same panic shape
//     unblock-xuk was created to avoid: `go test ./apps/api/db/...`
//     would build a test binary that loads this package, hit
//     sqldb.NewDatabase, and panic with "encore apps must be run
//     using the encore command". There is nothing here worth
//     unit-testing in isolation; encore test ./... is the
//     integration gate that confirms the migration set registers
//     from this path.
//
//   - This package MUST NOT acquire other responsibilities. Beyond
//     the NewDatabase declaration and the two bind calls below,
//     nothing belongs here. No domain logic, no //encore:api
//     surface, no exported helpers outside DB. Keep db leaf-shaped
//     so the "every domain service is an equal consumer" invariant
//     is obvious to a reader.
//
//   - The first argument to sqldb.NewDatabase MUST stay the literal
//     `"unblock"`. It is the registered database name every consumer
//     service binds to via `sqldb.Named("unblock")` (org,
//     workitems, deps, mcp, providers, boards, memory; SPEC §3.1).
//     Renaming would silently break every cross-service handle.
//
//   - The Migrations path MUST stay `./migrations` (relative to this
//     package directory). Encore's parser rejects sqldb.NewDatabase
//     calls whose Migrations path is not rooted within the calling
//     package's directory (parser error E1796 documented on
//     unblock-xuk). The migration files therefore live at
//     apps/api/db/migrations/, not at any historical location.
//
//   - This package MAY import encore.app/auth (for BindDB) and
//     encore.app/shared/rbac (for Bind). It MUST NOT import any
//     other domain service. Any extra import would re-couple the
//     migration owner to a domain service it has no business owning
//     and risk a package import cycle if that service ever needs to
//     reach a db-package symbol.
package db

import (
	"encore.app/auth"
	"encore.app/shared/rbac"
	"encore.dev/storage/sqldb"
)

// Service is the Encore service struct for the dedicated db migration-
// owner service. It carries no dependencies and exposes no APIs — its
// sole purpose is to give Encore's parser an //encore:service anchor
// so the package is registered as a top-level service and the
// canonical sqldb.NewDatabase declaration below is allowed to bind
// the shared rbac builder + the auth service's nil-handle via the
// init function. Without the //encore:service annotation, Encore
// rejects the cross-package use of DB with error E1814:
// "Infrastructure resources can only be referenced within services."
//
//encore:service
type Service struct{}

// initService satisfies Encore's lifecycle contract for the service
// struct above. db has no per-request state; the struct stays empty.
//
// The init() function below is the real bootstrap wiring: it fires
// at Go package init (before Encore calls initService) and centralises
// the auth.BindDB / rbac.Bind calls. initService is required by
// Encore's //encore:service form but does no useful work here.
func initService() (*Service, error) {
	return &Service{}, nil
}

// DB is the canonical *sqldb.Database for the `unblock` Postgres
// database. Consumer services receive the handle via auth.BindDB(DB)
// (for the auth service's late-bind path) or via the
// service-local `sqldb.Named("unblock")` lookup at their own
// initbind.go — DB is exported only so future tooling (linters,
// audits) can spot-check that the same handle is registered
// everywhere.
//
// The package-level form is mandated by sqldb.NewDatabase's
// doc-comment: "a call to NewDatabase can only be made when declaring
// a package level variable". The migration directory is co-located at
// apps/api/db/migrations/ because the Encore parser refuses
// Migrations paths that escape the calling package's directory
// (parser error E1796).
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
// apps/api/auth/dbhandle/dbhandle.go (now deleted) and on
// apps/api/shared/rbac/rbac.go; bead unblock-tv8.34 tracks the
// parallel hardening discussion for both setters. Until that lands,
// the runtime guarantee is "Encore process init runs serially and
// completes before the first handler".
//
// init is the central service-bootstrap wiring point for the
// unblock database handle; the pattern mirrors the historical
// apps/api/auth/dbhandle/dbhandle.go init (relocated here as part of
// bead unblock-bne) and is tracked on bead unblock-tv8.34.
//
//nolint:gochecknoinits
func init() {
	auth.BindDB(DB)
	rbac.Bind(DB)
}
