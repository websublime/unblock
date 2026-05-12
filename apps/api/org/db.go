package org

import "encore.dev/storage/sqldb"

// db is the consumer-side handle for the canonical `unblock` Postgres
// database. The dedicated apps/api/db/ service is the sole
// migration-owner per SPEC §3.1 (apps/api/db/db.go calls
// sqldb.NewDatabase exactly once across the workspace) — every domain
// service, including org, obtains its handle via sqldb.Named and never
// declares migration directories.
//
// The handle is created at package init and used by:
//   - org.go RPC bodies (CreateOrganization, CreateProject, AddMember,
//     GetOrganization, GetProject, Authorize) for direct SQL.
//   - initbind.go which wires this handle into the shared rbac builder
//     so the org service's own RBAC reads do not depend on auth's init
//     ordering.
//
//nolint:unused // referenced by RPC bodies and initbind.go.
var db = sqldb.Named("unblock")
