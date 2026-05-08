package auth

import "encore.dev/storage/sqldb"

// db is the canonical handle for the `unblock` Postgres database. The auth
// service is the sole migration-owner per SPEC §3.1 — this NewDatabase
// declaration registers `apps/api/auth/migrations/` as the migration source
// for every schema (auth, org, workitems, deps, providers, mcp, boards,
// memory). All other services obtain their handle via sqldb.Named("unblock")
// and never write migration files.
//
// Bootstrap migration 0010_bootstrap.up.sql ships alongside this declaration
// (task A-2 / unblock-tv8.2): Encore's parser rejects an empty Migrations
// directory, so the NewDatabase call and the first .up.sql file must land
// in the same commit. The remaining seven schema migrations (0020..0090)
// land in task A-3 (unblock-tv8.3).
//
//nolint:unused // referenced by RPC bodies starting in beads B-1..D-3.
var db = sqldb.NewDatabase("unblock", sqldb.DatabaseConfig{
	Migrations: "./migrations",
})
