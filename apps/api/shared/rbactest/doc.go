// Package rbactest is the exhaustive RBAC regression suite for the
// `://unblock` backend. It fires one Go subtest per
// (caller-org × target-org × caller-role × table × action) tuple across
// every P01-exposed schema this bead owns, and asserts that the
// canonical isolation gates (org.Authorize predicate and rbac.For[T]
// scoped reads) never leak a single row or a single PERMIT across the
// tenant boundary. CI gates release on zero cross-tenant leaks
// (SPEC §10.1, §11.2 NFR-2).
//
// Bead unblock-tv8.9 (B-3) lays down the suite scaffolding and covers
// the auth + org schemas: auth.users, auth.oauth_tokens, auth.sessions,
// org.organizations, org.projects, org.members, org.project_members.
// Bead unblock-tv8.15 (C-6) extends the matrix to workitems.items,
// workitems.comments, workitems.trail, deps.dependencies as the
// C-group RPCs land. Bead unblock-tv8.25 (E-3) closes the final
// release-gate matrix across mcp.api_keys, mcp.tool_calls,
// memory.entries, boards.boards. The package shape, matrix vocabulary,
// and seed/cleanup helpers below are deliberately additive so the two
// follow-up beads only add fixture rows and table classifications
// rather than re-architecting the driver.
//
// Two assertion shapes, picked by table characteristics
// (see matrix.go for the classification table):
//
//  1. Tables that carry an `org_id` column (org.projects, org.members
//     today; workitems.items, workitems.comments, workitems.trail,
//     deps.dependencies, mcp.api_keys, mcp.tool_calls, memory.entries,
//     boards.boards in C-6/E-3): driven through rbac.For[T]. The
//     assertion is row-level — the suite seeds rows in both orgs and
//     asserts that a caller from org_a reading `<table>` produces zero
//     rows whose org_id == org_b.id, for every (caller-role × action)
//     pairing. The reads themselves are read-only (read-action only);
//     write/delete are exercised via the Authorize-predicate shape
//     below because the rbac builder is a read-only surface.
//
//  2. Tables that do NOT carry an `org_id` column (auth.users,
//     auth.oauth_tokens, auth.sessions, org.organizations,
//     org.project_members): driven through org.Authorize directly.
//     The cross-tenant gate for these tables is the Authorize
//     predicate (step 1 cross-tenant short-circuit), not a
//     `WHERE … org_id = $1` SQL filter. The assertion is permit-level
//     — for every (caller-role × action) tuple, the suite asserts
//     Authorize denies when the request's OrgID differs from the
//     caller's Identity.OrgID, and permits or denies per the policy
//     matrix when same-org. The two shapes converge on the same
//     promise: zero cross-tenant leaks at the gate or in the rows.
//
// Roles axis (first-class — SPEC §10.1):
//
// The matrix iterates {owner, admin, member, viewer, agent}. The
// synthetic "agent" role (Identity.Role = "agent") is the API-key
// runtime identity per SPEC §4.3.2 step 8 — never a member-table row,
// constructed in-memory only by the test fixtures. owner, admin,
// member, viewer are seeded as org.members rows so the effective-role
// derivation (max(org_role, project_role)) inside Authorize reads a
// real row. The five-role axis matches the org.Authorize
// implementation's branches (apps/api/org/org.go ~line 506 onward) and
// is what makes "one test per tuple" exhaustive against the actual
// policy code path, not a paraphrase of it.
//
// Encore-runtime requirement.
//
// This package MUST be executed under `encore test
// ./apps/api/shared/rbactest/...`. Plain `go test` does NOT bring up
// the Encore-managed Postgres cluster, does NOT fire the dedicated
// apps/api/db/ service's init() (which is what calls auth.BindDB(DB),
// org.BindDB(DB), workitems.BindDB(DB), deps.BindDB(DB), and
// rbac.Bind(DB) per apps/api/db/db.go's package doc), and therefore
// leaves every service's *sqldb.Database pointer nil. Run on a nil
// handle panics inside encore.dev/storage/sqldb. Any future contributor
// tempted to add a `go:build` constraint or a runtime fast-path that
// allows `go test` here MUST first read the CLAUDE.md
// `feedback_encore_test_not_go_test` memory and the package-doc on
// apps/api/db/db.go — the constraint is empirical, not stylistic.
//
// CI wiring (out of scope for this bead).
//
// The CI gate that runs this suite and blocks merge on any failure is
// owned by bead unblock-tv8.6 (A-6) — that bead is the consumer of
// this package, not its sibling. This package's responsibility ends at
// being discoverable as `encore test
// ./apps/api/shared/rbactest/...` and producing a subtest tree whose
// name pinpoints any failing tuple. SPEC §10.1 line 2001-2004 — the
// "CI gates release on zero cross-tenant leaks" sentence — is the
// product invariant; the wiring lives in A-6.
//
// Concurrency.
//
// The suite does NOT call t.Parallel anywhere. rbac.Bind (and the
// per-service BindDB hooks) are not goroutine-safe; bead
// unblock-tv8.34 tracks the hardening discussion. The single-binding
// contract is shared with the wider domain-service tree (auth, org,
// workitems, deps); rbactest takes the same guarantee Encore's process
// init provides ("BindDB completes before any handler dispatches") and
// runs the matrix sequentially. The suite has a few-thousand subtests
// in P01; sequential execution is fast enough on the Encore-managed
// local cluster (single-digit-second budget per encore test invocation
// on developer hardware).
//
// Cleanup discipline.
//
// Every subtest runs against the same seeded fixture (two orgs, all
// roles per side, one project per org, one user per role per org,
// plus a small set of cross-side leak-bait rows in auth.users,
// auth.oauth_tokens, auth.sessions, mcp.api_keys). TestMain seeds
// once at startup and tears down once at exit via a leaf-first
// TRUNCATE ... CASCADE on org.organizations (the FK chain fans out
// to org.members, org.projects, org.project_members,
// auth.users-by-membership-fan, auth.oauth_tokens-by-user-cascade,
// auth.sessions-by-user-cascade, mcp.api_keys-by-org-cascade, …).
// Per-subtest cleanup is deliberately avoided — the matrix subtests
// are read-only on the fixture and never mutate state, so a single
// global seed/teardown pair is both correct and fastest. If a future
// contributor adds a mutating subtest, it MUST t.Cleanup its own
// rollback or be excluded from the read-only sweep.
package rbactest
