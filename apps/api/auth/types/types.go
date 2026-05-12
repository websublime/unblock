// Package types holds the pure value types of the auth service that are
// safe to import from non-Encore Go test runners (plain `go test`, godoc,
// static analysis). It is a leaf sub-package: it imports nothing from
// encore.dev, declares no APIs, and registers no infrastructure.
//
// Why it exists (bead unblock-tv8.30): the parent `auth` package declares
// the canonical `unblock` Postgres handle via
// `var db = sqldb.NewDatabase("unblock", ...)` at package scope. Encore's
// runtime panics if NewDatabase executes outside the encore CLI's
// cluster bring-up — and cluster bring-up itself requires Docker. Any
// transitive import of `encore.app/auth` from a non-Encore test path
// (e.g. `go test ./shared/rbac/...`) triggers `auth.init()` and panics
// with "encore apps must be run using the encore command". That made
// every consumer of `auth.Identity` (org, rbac, future B-1..D-3, E-*)
// require Docker just to compile a unit test that never touches the DB.
//
// The fix: extract the pure value types (currently just Identity) into
// this leaf sub-package. The parent `auth` package re-exports them via
// `type Identity = types.Identity` so SPEC §10.1's literal spelling
// `auth.Identity` is preserved at every call site. Consumers that only
// need the type — like the rbac builder under `apps/api/shared/rbac/` —
// import `encore.app/auth/types` directly and avoid the auth-package
// init-time hazards entirely (NewDatabase, secrets binding,
// //encore:api endpoints).
//
// Constraints (DO NOT VIOLATE):
//
//   - This package MUST NOT import any encore.dev/* package, directly or
//     transitively. The whole point of the package is to be Encore-free.
//   - This package MUST NOT declare //encore:api endpoints, //encore:service
//     annotations, or any infrastructure resource (sqldb.NewDatabase,
//     pubsub.NewTopic, cache.NewCluster, secrets, ...). It is a pure
//     value-type package.
//   - SPEC §3.1 invariant: the auth service remains the sole
//     migration-owner; sqldb.NewDatabase("unblock", ...) is still called
//     exactly once across the workspace, in
//     `apps/api/auth/dbhandle/dbhandle.go`. Bead unblock-xuk relocated
//     the call from the historical apps/api/auth/db.go (where
//     unblock-tv8.30 originally pointed) into the dbhandle leaf
//     sub-package so plain `go test ./auth/...` no longer panics on
//     package init — see dbhandle's package doc-comment for the full
//     reasoning. The auth root package retains the late-bound
//     *sqldb.Database handle (`var db *sqldb.Database` in
//     apps/api/auth/db.go); dbhandle.init populates it via
//     auth.BindDB.
//   - SPEC §10.1 invariant: consumer call sites continue to spell the
//     type as `auth.Identity` (literal). They MUST NOT be re-spelled to
//     `types.Identity` or `authtypes.Identity`. The parent `auth`
//     package re-exports via type alias.
package types

// Identity is the resolved caller record carried inside the Encore mesh.
// Locked shape per SPEC §4.1.
//
// Field semantics:
//
//   - UserID:    ULID — the auth.users row this Identity was minted for.
//   - OrgID:     ULID — the primary org binding for this auth event;
//     used by the rbac scope predicate (`<table>.org_id = $1`).
//   - Role:      one of "owner" | "admin" | "member" | "viewer" for
//     human sessions (org.members.role CHECK enforces the four-role
//     enum); plus the synthetic "agent" runtime role minted by auth's
//     API-key Bearer hot path (auth.go const roleAgent — never a
//     member-table value, never accepted by org.AddMember). See SPEC
//     §4.2 RBAC matrix and §4.3.2 step 8. Unspecified values are
//     rejected at org.Authorize.
//   - AgentKind: empty for human sessions; an AgentKind enum value
//     (SPEC §4.3) for API-key callers (Bearer-key auth path).
//
// This type is a pure value record: no methods, no pointers to runtime
// state, no Encore infrastructure dependencies. Safe to construct from
// any package, including plain `go test` consumers under
// `apps/api/shared/*`.
type Identity struct {
	UserID    string // ULID
	OrgID     string // ULID — primary org binding for this auth event
	Role      string // "owner" | "admin" | "member" | "viewer"
	AgentKind string // empty for human sessions; AgentKind value for API-key callers
}
