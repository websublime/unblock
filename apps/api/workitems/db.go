// db.go declares the package-level *sqldb.Database handle for the
// workitems service and the BindDB hook used by the dedicated
// apps/api/db/ service to inject the canonical handle at process start.
//
// Canonical consumer pattern (SPEC §3.1, post bead unblock-bne rework).
//
// Every domain service that touches the unblock database uses this exact
// shape — a nil *sqldb.Database pointer plus an exported BindDB hook.
// The companion apps/api/db package owns the sqldb.NewDatabase call and
// invokes BindDB from its package init. No per-service initbind.go; the
// central bind in apps/api/db/ is the sole binding authority.
//
// Why the BindDB shape — empirical encore-CLI invariant.
//
// The naive consumer pattern is `var db = sqldb.Named("unblock")` at
// package init. Empirical check against encore.dev/storage/sqldb v1.52.1
// disproved the assumption that sqldb.Named is a benign runtime lookup:
// the v1.52.1 implementation calls doPanic at package-load time the same
// way sqldb.NewDatabase does (pkgfn.go:182-192). So
// `var db = sqldb.Named("unblock")` at package init panics any plain
// `go test ./apps/api/workitems/...` outside the encore CLI with the
// "encore apps must be run using the encore command" error.
//
// The BindDB late-bind pattern (a nil package-level pointer + an
// exported BindDB hook populated by apps/api/db/db.go's init) is the
// only shape that preserves the xuk goal (plain `go test ./<service>/...`
// loads without panic) while keeping the canonical NewDatabase call
// centralised in apps/api/db/. See the DECISION trail on bead
// unblock-bne for the empirical trace.
//
// db is declared here (tagged //nolint:unused for the rare build that
// excludes the RPC bodies) and BindDB is exported. The handle is
// registered in apps/api/db/db.go's init so the wiring is live from
// day one; the RPC bodies in workitems.go read `db` directly now that
// the B-1+ DB-touching code has landed.
//
// Hot-path impact: zero. After apps/api/db/db.go's init() runs (once,
// during Encore process bootstrap) `db` is a non-nil *sqldb.Database
// for the lifetime of the process. Every RPC body reads `db` as a
// plain Go variable — no per-request synchronisation.
//
// Tracked risk: BindDB is not goroutine-safe. Identical contract to
// auth.BindDB, org.BindDB, and rbac.Bind: runs exactly once during
// process bootstrap, before any handler can dispatch. Bead
// unblock-tv8.34 tracks the parallel hardening discussion for all
// setters.

package workitems

import "encore.dev/storage/sqldb"

// db is the canonical handle for the `unblock` Postgres database. It is
// a package-level pointer that starts nil and is populated exactly once
// by BindDB during process bootstrap (called from apps/api/db/db.go's
// init function — the dedicated migration-owner service per SPEC §3.1).
// RPC bodies in workitems.go read `db` directly now that the B-1+
// DB-touching code (workitems bodies, FTS, milestones, claim
// transaction) has landed.
//
//nolint:unused // referenced by RPC bodies starting in beads B-1+ and by apps/api/db's init.
var db *sqldb.Database

// BindDB installs the unblock-database handle. The companion
// apps/api/db package owns the sqldb.NewDatabase call and invokes
// BindDB from its package init. Called exactly once per process;
// subsequent calls overwrite the handle (test fixtures may rely on
// this for swap-in of fakes — same contract as auth.BindDB,
// org.BindDB, and rbac.Bind).
//
// Concurrency: not goroutine-safe. See the file-level doc-comment for
// the runtime guarantee (Encore's process-bootstrap import order
// guarantees BindDB completes before any //encore:api dispatch).
//
// This function is intentionally exported so the dedicated db service
// can call it without re-declaring `db` or duplicating the
// infrastructure. The workitems package still owns `db`; the db
// service only sources the value.
func BindDB(d *sqldb.Database) { db = d }
