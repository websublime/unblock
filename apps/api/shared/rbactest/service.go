// service.go declares the //encore:service anchor that lets the
// rbactest package call other services' private RPCs (specifically
// org.Authorize) under `encore test ./shared/rbactest/...`.
//
// Why a service annotation on a "shared/" package.
//
// The Encore parser enforces invariant E1388 ("APIs can only be
// called from within a service"). Without a //encore:service anchor
// here, every call to org.Authorize, auth.IssueAPIKey, etc. inside
// rbactest_test.go fails the parser check before any test runs.
// rbactest is, by design, a cross-service integration suite that
// invokes the canonical RBAC predicate on the production code path —
// it MUST be a parser-visible service for those calls to be legal.
//
// The package still lives under `apps/api/shared/` because the bead
// (unblock-tv8.9) and SPEC §10.1 both lock that path. The shared/
// prefix is a layout convention, not a parser constraint — Encore
// treats any directory with a //encore:service annotation as a
// service, regardless of its file-system parent. The auth/types,
// ulid, rbac, and lint siblings are NOT services because they own no
// state and call no APIs; rbactest is a service because it calls
// APIs.
//
// Constraints (mirrors apps/api/db/db.go's tighter version of the
// same shape):
//
//   - This package MUST NOT declare any //encore:api endpoints. The
//     suite's only purpose is to drive cross-service calls; exposing
//     RPCs here would muddle the suite's scope and risk a real
//     production path landing in a /shared/ subtree.
//   - This package MUST NOT acquire other responsibilities. It is a
//     test surface. The Service struct + initService below stay
//     empty.
//   - The Service struct's identity is purely a parser anchor; no
//     dependency injection, no per-request state, no lifecycle
//     hooks beyond the no-op initService.

package rbactest

// Service is the Encore service anchor for the rbactest cross-service
// integration suite. Carries no state and exposes no APIs — the
// //encore:service annotation is what makes the cross-service calls
// in rbactest_test.go (org.Authorize, future auth/workitems/deps/mcp
// extensions in C-6 and E-3) legal under Encore's parser.
//
//encore:service
type Service struct{}

// initService satisfies Encore's lifecycle contract for the service
// struct above. rbactest has no per-request state; the struct stays
// empty. Encore calls this exactly once during service bring-up.
func initService() (*Service, error) {
	return &Service{}, nil
}
