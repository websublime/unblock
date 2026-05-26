// service.go declares the //encore:service anchor that lets the
// exitcriteriontest package call other services' private RPCs
// (workitems.CreateMilestone, workitems.AssignItem, workitems.MilestoneTree,
// and the rest of the §11.1.2 milestone assertions) under
// `encore test ./apps/api/exitcriteriontest/...`.
//
// Why a service annotation on a top-level package without endpoints.
//
// The Encore parser enforces invariant E1388 ("APIs can only be
// called from within a service"). Without a //encore:service anchor
// here, every call to a private //encore:api from this package would
// fail the parser check before any test runs. The exit-criterion
// harness is a cross-service integration suite that invokes the
// canonical milestone RPCs (and indirectly, via the MCP transport,
// every tool RPC) on the production code path — it MUST be a
// parser-visible service for those calls to be legal.
//
// Same shape as `apps/api/shared/rbactest/service.go`. The path under
// `apps/api/exitcriteriontest/` (rather than under `apps/api/shared/`)
// is deliberate: the test package owns its own seed and a §11.1.0
// fixture topology that is product-domain (not RBAC fixture data),
// so collocating with the eight production services keeps the audit
// trail unambiguous.
//
// Constraints:
//
//   - This package MUST NOT declare any //encore:api endpoints. The
//     suite's only purpose is to drive end-to-end exit-criterion
//     assertions; exposing RPCs here would muddle the suite's scope
//     and risk a real production path landing in a test-only package.
//   - This package MUST NOT acquire other responsibilities. It is a
//     test surface. The Service struct + initService below stay empty.
//   - The Service struct's identity is purely a parser anchor; no
//     dependency injection, no per-request state, no lifecycle hooks
//     beyond the no-op initService.

package exitcriteriontest

// Service is the Encore service anchor for the exit-criterion
// end-to-end test harness. Carries no state and exposes no APIs —
// the //encore:service annotation is what makes the cross-service
// calls in the *_test.go files legal under Encore's parser.
//
//encore:service
type Service struct{}

// initService satisfies Encore's lifecycle contract for the service
// struct above. exitcriteriontest has no per-request state; the
// struct stays empty. Encore calls this exactly once during service
// bring-up.
func initService() (*Service, error) {
	return &Service{}, nil
}
