// Package db is the dedicated, zero-API migration-owner service for the
// canonical `unblock` Postgres database. It declares
// `sqldb.NewDatabase("unblock", ...)` exactly once across the workspace,
// ships the canonical migration set under apps/api/db/migrations/, and
// is the SOLE binding authority for every domain-service database
// handle. Its package init() invokes:
//
//   - auth.BindDB(DB)  — populates the auth service's nil handle
//   - org.BindDB(DB)   — populates the org service's nil handle
//   - rbac.Bind(DB)    — installs the shared rbac builder's handle
//
// Every domain service (auth, org, workitems, deps, mcp, providers,
// boards, memory) is an equal database consumer; no domain service
// owns DDL for schemas it does not consume, and no domain service
// declares `sqldb.Named("unblock")` at package init (such a call
// panics outside the encore CLI; see CONSUMER PATTERN below).
//
// CONSUMER PATTERN — required for every domain service that touches
// the unblock database (canonical post bead unblock-bne pre-review):
//
//	package <service>
//
//	import "encore.dev/storage/sqldb"
//
//	var db *sqldb.Database
//
//	func BindDB(d *sqldb.Database) { db = d }
//
// Then add the corresponding `<service>.BindDB(DB)` call to this
// package's init() below. The workitems and deps services already
// carry their skeleton db.go + BindDB hook ahead of DB-touching code
// landing in B-1+ / C-1+ — they are pre-wired here. Future P01/P0n
// services (mcp, providers, boards, memory) that touch the database
// MUST follow this pattern and MUST be registered here — no exceptions.
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
// empirically against the encore CLI; pre-review scope expansion
// extended the same shape to org and made it the canonical pattern
// for every future service).
//
// db.init() centralises ALL cross-cutting binds. Domain services do
// NOT carry their own initbind.go anymore; the single central wiring
// point in this package is now the sole binding authority.
//
//   - auth.BindDB(DB) populates the auth service's nil *sqldb.Database
//     handle. The auth root package MUST NOT import any package that
//     calls sqldb.NewDatabase or sqldb.Named at its own package init —
//     either call panics under plain `go test ./auth/...` outside the
//     encore CLI (sqldb.Named is NOT a benign runtime lookup; its
//     v1.52.1 implementation calls doPanic the same way NewDatabase
//     does). The BindDB late-bind hook is the only shape that
//     preserves the xuk goal (plain `go test ./auth/...` loads
//     without panic) while moving the NewDatabase declaration out of
//     the auth tree.
//
//   - org.BindDB(DB) populates the org service's nil handle. The org
//     service was converted from the eager `var db =
//     sqldb.Named("unblock")` shape to the same BindDB late-bind
//     pattern as part of bead unblock-bne's pre-review scope
//     expansion. The motivation is identical: sqldb.Named panics at
//     package init outside the encore CLI, so the eager form broke
//     `go test ./apps/api/org/...` at load time. The BindDB shape
//     fixes that and standardises every domain service on the same
//     consumer pattern.
//
//   - workitems.BindDB(DB) and deps.BindDB(DB) populate the workitems
//     and deps services' nil handles. In P01 A-1 their RPC bodies
//     return errNotImplemented and do not yet read `db`; the
//     pre-wiring lands now so beads B-1+ (workitems bodies, FTS,
//     milestones, claim transaction) and C-1+ (cycle CTE, advisory
//     locks, cascade Pub/Sub publisher) inherit a non-nil handle the
//     instant they start touching the database. The skeleton db.go
//     in each service mirrors auth/db.go and org/db.go verbatim.
//
//   - rbac.Bind(DB) installs the shared rbac builder's handle. The
//     previous defense-in-depth pattern (each service called
//     rbac.Bind from its own initbind.go) is gone: with apps/api/db/
//     as the single binding authority, the central Bind here is
//     sufficient and the cross-service init-order race that
//     org/initbind.go was originally created to close is now
//     subsumed by Go's import-order guarantee. db imports every
//     consumer service, so every consumer service's init runs before
//     db's init, and rbac.Bind fires before any handler dispatches.
//
// Init-order analysis.
//
// Go guarantees imported packages' init runs before the importer's
// init. db imports encore.app/auth, encore.app/org,
// encore.app/workitems, encore.app/deps, and encore.app/shared/rbac;
// auth.init, org.init, workitems.init, deps.init, and rbac.init
// therefore all complete before db.init. Encore loads
// infrastructure-resource consumers (db.init) during process
// bootstrap, before any //encore:api handler dispatches. The
// dbhandle.init precedent (shipped to PROD under unblock-tv8.7
// before bead unblock-bne) proves the runtime contract; this package
// relocates the same init from apps/api/auth/dbhandle/ to
// apps/api/db/ and (post-bne pre-review) extends it to bind every
// domain service's handle from a single central wiring point.
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
//     the NewDatabase declaration and the bind calls in init()
//     below, nothing belongs here. No domain logic, no //encore:api
//     surface, no exported helpers outside DB. Keep db leaf-shaped
//     so the "every domain service is an equal consumer" invariant
//     is obvious to a reader. When a new domain service lands
//     DB-touching code, add ONLY a new `<service>.BindDB(DB)` line
//     to init() — nothing else.
//
//   - The first argument to sqldb.NewDatabase MUST stay the literal
//     `"unblock"`. It is the registered database name; Encore's
//     parser uses it to wire this NewDatabase declaration to every
//     consumer service's per-service DB binding (SPEC §3.1).
//     Renaming would silently break every cross-service handle and
//     leave every BindDB hook bound to a nil pointer.
//
//   - The Migrations path MUST stay `./migrations` (relative to this
//     package directory). Encore's parser rejects sqldb.NewDatabase
//     calls whose Migrations path is not rooted within the calling
//     package's directory (parser error E1796 documented on
//     unblock-xuk). The migration files therefore live at
//     apps/api/db/migrations/, not at any historical location.
//
//   - This package MAY import every domain service whose handle it
//     binds (encore.app/auth, encore.app/org, encore.app/workitems,
//     encore.app/deps today; future mcp / providers / boards / memory
//     when those services land DB-touching code) and
//     encore.app/shared/rbac (for rbac.Bind). It MUST NOT import any
//     other package. This keeps the migration owner tightly scoped to
//     "owns NewDatabase + migrations, binds every consumer's handle,
//     period". A new service that needs the DB MUST add its own
//     BindDB hook (see CONSUMER PATTERN above) and add the
//     corresponding bind line to init() below — no shortcuts.
package db

import (
	"encore.app/auth"
	"encore.app/deps"
	"encore.app/mcp"
	"encore.app/org"
	"encore.app/shared/rbac"
	"encore.app/workitems"
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
// every consumer's BindDB call plus rbac.Bind. initService is required
// by Encore's //encore:service form but does no useful work here.
func initService() (*Service, error) {
	return &Service{}, nil
}

// DB is the canonical *sqldb.Database for the `unblock` Postgres
// database. Consumer services receive the handle exclusively via
// the canonical BindDB late-bind hook (auth.BindDB(DB),
// org.BindDB(DB), workitems.BindDB(DB), deps.BindDB(DB), and future
// <service>.BindDB(DB) entries) plus the rbac.Bind(DB) cross-cutting
// call, all invoked from this package's init() below. DB is exported
// only so future tooling (linters, audits) can spot-check that the
// same handle is registered everywhere.
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

// init binds the unblock database handle into every consumer that
// needs it. After bead unblock-bne's pre-review scope expansion this
// is the SOLE binding authority for the auth.db, org.db,
// workitems.db, deps.db, and rbac.db handles — no domain service
// carries its own initbind.go.
//
// Per-service binds (one BindDB call per domain service that touches
// the database; add new entries here when new services land
// DB-touching code):
//
//   - auth.BindDB(DB)
//   - org.BindDB(DB)
//   - workitems.BindDB(DB)  — pre-wired ahead of B-1+ bodies
//   - deps.BindDB(DB)       — pre-wired ahead of C-1+ bodies
//   - mcp.BindDB(DB)        — consumed by recordToolCall (A-5)
//
// Plus the cross-cutting shared-rbac bind:
//
//   - rbac.Bind(DB)
//
// Encore's process-bootstrap import order guarantees this init runs
// before any //encore:api handler dispatches; Go's import-order
// guarantee additionally guarantees auth.init, org.init,
// workitems.init, deps.init, and rbac.init all complete before
// db.init fires, so each BindDB sets a package-level pointer that is
// then read by every subsequent RPC invocation without further
// synchronisation.
//
// Concurrency: auth.BindDB, org.BindDB, workitems.BindDB,
// deps.BindDB, and rbac.Bind are not goroutine-safe. The single-init
// contract is shared across all five setters; bead unblock-tv8.34
// tracks the parallel hardening discussion. Until that lands, the
// runtime guarantee is "Encore process init runs serially and
// completes before the first handler".
//
//nolint:gochecknoinits
func init() {
	auth.BindDB(DB)
	org.BindDB(DB)
	workitems.BindDB(DB)
	deps.BindDB(DB)
	mcp.BindDB(DB)
	rbac.Bind(DB)
}
