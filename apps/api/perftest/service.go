// service.go declares the //encore:service anchor for the perftest
// package. Same shape as apps/api/exitcriteriontest/service.go.
//
// Why a service annotation on a top-level package without endpoints.
//
// The Encore parser enforces invariant E1388 ("APIs can only be called
// from within a service"). The perftest harness drives the production
// code path through the MCP httptest transport rather than calling
// private RPCs directly, but the //encore:service anchor keeps the
// package shape identical to exitcriteriontest and pre-empts any
// future direct RPC call from tripping the parser before the test runs.
//
// Constraints:
//
//   - This package MUST NOT declare any //encore:api endpoints. The
//     suite's only purpose is to measure prime → ready → claim latency
//     and exercise the negative auth paths; exposing RPCs here would
//     muddle the suite's scope.
//   - The Service struct's identity is purely a parser anchor; no
//     dependency injection, no per-request state, no lifecycle hooks
//     beyond the no-op initService.

package perftest

// Service is the Encore service anchor for the NFR-1 latency harness.
// Carries no state and exposes no APIs — the //encore:service
// annotation is what keeps the package's shape parser-consistent with
// the rest of the integration-test surfaces.
//
//encore:service
type Service struct{}

// initService satisfies Encore's lifecycle contract for the service
// struct above. perftest has no per-request state; the struct stays
// empty. Encore calls this exactly once during service bring-up.
func initService() (*Service, error) {
	return &Service{}, nil
}
