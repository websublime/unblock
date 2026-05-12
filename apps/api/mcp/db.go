// db.go declares the package-level *sqldb.Database handle for the mcp
// service and the BindDB hook used by the dedicated apps/api/db/
// service to inject the canonical handle at process start.
//
// Canonical consumer pattern (SPEC §3.1). Same shape as auth/db.go,
// org/db.go, workitems/db.go, deps/db.go — a nil *sqldb.Database
// pointer + an exported BindDB hook populated centrally by
// apps/api/db/db.go's init. Direct `sqldb.Named("unblock")` at
// package init is forbidden — encore.dev v1.52.1 panics at package
// load outside the encore CLI on every call to either
// sqldb.NewDatabase or sqldb.Named, which breaks plain
// `go test ./apps/api/mcp/...`.
//
// In bead A-5 (unblock-tv8.5) the only consumer of `db` is
// recordToolCall (mcp/recordtoolcall.go) — the audit writer that
// fires at request end and inserts a row into mcp.tool_calls per
// SPEC §8.1. Until D-1 (unblock-tv8.16) plugs the Go MCP SDK into
// MCPHandler, every request falls through to the 405 default branch
// in mcp.go; recordToolCall is invoked from the handler's deferred
// epilogue so every observed request produces one tool_calls row
// even before real tool dispatch lands.
//
// Hot-path impact: zero. After apps/api/db/db.go's init() runs
// (once, during Encore process bootstrap) `db` is a non-nil
// *sqldb.Database for the lifetime of the process. Every recordToolCall
// invocation reads `db` as a plain Go variable — no per-request
// synchronisation.
//
// Tracked risk: BindDB is not goroutine-safe. Same contract as
// auth.BindDB, org.BindDB, workitems.BindDB, deps.BindDB, and
// rbac.Bind: runs exactly once during process bootstrap, before
// any handler can dispatch. Bead unblock-tv8.34 tracks the parallel
// hardening discussion for all setters.

package mcp

import "encore.dev/storage/sqldb"

// db is the canonical handle for the `unblock` Postgres database.
// Starts nil; populated exactly once by BindDB during process
// bootstrap (called from apps/api/db/db.go's init function — the
// dedicated migration-owner service per SPEC §3.1). recordToolCall
// is the only A-5 consumer; D-1+ tool handlers will read `db` for
// future mcp-schema reads/writes.
//
//nolint:unused // referenced by recordToolCall (A-5) and by apps/api/db's init.
var db *sqldb.Database

// BindDB installs the unblock-database handle. The companion
// apps/api/db package owns the sqldb.NewDatabase call and invokes
// BindDB from its package init. Called exactly once per process;
// subsequent calls overwrite the handle (test fixtures may rely on
// this for swap-in of fakes — same contract as auth.BindDB,
// org.BindDB, workitems.BindDB, deps.BindDB, and rbac.Bind).
//
// Concurrency: not goroutine-safe. See the file-level doc-comment
// for the runtime guarantee (Encore's process-bootstrap import
// order guarantees BindDB completes before any //encore:api
// dispatch).
func BindDB(d *sqldb.Database) { db = d }
