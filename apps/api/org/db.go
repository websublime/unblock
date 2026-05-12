// db.go declares the package-level *sqldb.Database handle for the org
// service and the BindDB hook used by the dedicated apps/api/db/ service
// to inject the canonical handle at process start.
//
// Canonical consumer pattern (post bead unblock-bne rework).
//
// Every domain service that touches the unblock database (auth, org,
// and every future P01/P0n service: workitems, deps, mcp, providers,
// boards, memory) uses this exact shape:
//
//	package <service>
//
//	import "encore.dev/storage/sqldb"
//
//	//nolint:unused // referenced by RPC bodies and apps/api/db's init.
//	var db *sqldb.Database
//
//	// BindDB installs the unblock-database handle. The companion
//	// apps/api/db package owns the sqldb.NewDatabase call and invokes
//	// BindDB from its package init.
//	func BindDB(d *sqldb.Database) { db = d }
//
// Why this shape — empirical encore-CLI invariant.
//
// The naive consumer pattern is `var db = sqldb.Named("unblock")` at
// package init. Empirical check against encore.dev/storage/sqldb v1.52.1
// disproved the assumption that sqldb.Named is a benign runtime lookup:
// the v1.52.1 implementation calls doPanic at package-load time the
// same way sqldb.NewDatabase does (pkgfn.go:182-192). So
// `var db = sqldb.Named("unblock")` at package init panics any plain
// `go test ./apps/api/<service>/...` outside the encore CLI with the
// same "encore apps must be run using the encore command" error as the
// pre-unblock-xuk auth-tree shape.
//
// The dbhandle-style late-bind pattern preserved on the auth service
// (a nil package-level pointer + an exported BindDB hook) is the only
// shape that preserves the xuk goal (plain `go test ./<service>/...`
// loads without panic) while keeping the canonical NewDatabase call
// centralised in apps/api/db/. After bead unblock-bne's pre-review
// scope expansion, this shape is the canonical consumer pattern for
// every domain service — no service declares `sqldb.Named("unblock")`
// at package init anywhere in the workspace. See the DECISION comment
// on bead unblock-bne for the empirical trace.
//
// Round 1 (the original B-2 / unblock-tv8.8 shape, now superseded)
// declared the handle here inline as `var db = sqldb.Named("unblock")`
// and used a companion initbind.go to call rbac.Bind(db) at service
// init. That shape was authored on the (incorrect) assumption that
// sqldb.Named did not panic at package init. The bne pre-review scope
// expansion replaced it with the BindDB late-bind shape below and
// deleted initbind.go: the centralised apps/api/db/db.go init now
// drives both BindDB(DB) and rbac.Bind(DB) for every service that
// needs them.
//
// Hot-path impact: zero. After apps/api/db/db.go's init() runs (once,
// during Encore process bootstrap) `db` is a non-nil *sqldb.Database
// for the lifetime of the process. Every RPC body reads `db` as a
// plain Go variable — no per-request synchronisation, no sync.Once
// post-completion atomic load.
//
// Test-time consequences:
//   - `go test ./apps/api/org/...` loads the package without
//     panicking. The org root package no longer imports anything that
//     fires sqldb.NewDatabase or sqldb.Named at its own package init.
//   - `encore test ./org/...` continues to work under the Encore CLI's
//     Docker-backed test runner.
//
// Tracked risk: BindDB is not goroutine-safe. Identical contract to
// auth.BindDB and rbac.Bind: runs exactly once during process
// bootstrap, before any handler can dispatch. Bead unblock-tv8.34
// tracks the parallel hardening discussion for all three setters.

package org

import "encore.dev/storage/sqldb"

// db is the canonical handle for the `unblock` Postgres database. It is
// a package-level pointer that starts nil and is populated exactly once
// by BindDB during process bootstrap (called from apps/api/db/db.go's
// init function — the dedicated migration-owner service per SPEC §3.1).
// Every RPC body in this package reads `db` directly — there is no
// per-call lookup.
//
//nolint:unused // referenced by RPC bodies starting in beads B-2.
var db *sqldb.Database

// BindDB installs the unblock-database handle. The companion
// apps/api/db package owns the sqldb.NewDatabase call and invokes
// BindDB from its package init. Called exactly once per process;
// subsequent calls overwrite the handle (test fixtures may rely on
// this for swap-in of fakes — same contract as auth.BindDB and
// rbac.Bind).
//
// Concurrency: not goroutine-safe. See the file-level doc-comment for
// the runtime guarantee (Encore's process-bootstrap import order
// guarantees BindDB completes before any //encore:api dispatch).
//
// This function is intentionally exported so the dedicated db service
// can call it without re-declaring `db` or duplicating the
// infrastructure. The org package still owns `db`; the db service
// only sources the value.
func BindDB(d *sqldb.Database) { db = d }
