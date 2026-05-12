// Package mcpaudittest is the //encore:service-anchored integration
// test harness for the mcp audit writer (apps/api/mcp/recordToolCall)
// and the MCPHandler tracing scaffold (apps/api/mcp/mcp.go).
//
// Why this package exists (mirrors apps/api/shared/rbactest/).
//
// The natural home for these tests is apps/api/mcp/mcphandler_test.go,
// but three Encore / Go invariants force a separate test package:
//
//  1. E1389 — "Raw APIs cannot be called from within an Encore
//     application." That blocks calling MCPHandler in-process; the
//     supported shape is an HTTP request against
//     encore.Meta().APIBaseURL. This is enforced from any code
//     inside the Encore application graph, including the mcp
//     package's own tests.
//
//  2. Import-cycle rule — apps/api/db/db.go imports apps/api/mcp/ to
//     call mcp.BindDB(DB) during init(). The mcp package therefore
//     cannot import apps/api/db/, which is the canonical entry
//     point that triggers BindDB. Without that import the mcp test
//     binary's init chain leaves the mcp package's db pointer nil,
//     and every DB-touching test panics at db.Exec on a nil
//     receiver.
//
//  3. E1810 — "Services cannot be nested." A sub-package under
//     apps/api/mcp/ that declares //encore:service is rejected
//     because mcp is already a service. That forced relocation to
//     a peer path; apps/api/shared/ is the established convention
//     for cross-service test scaffolding (rbactest is the
//     precedent).
//
// mcpaudittest lives at apps/api/shared/mcpaudittest/ so it can
// blank-import apps/api/db without creating an import cycle. It
// also declares its own //encore:service anchor so the parser
// permits HTTP calls and cross-service DB reads from inside test
// bodies (same shape as apps/api/shared/rbactest/service.go).
//
// Constraints (DO NOT VIOLATE):
//
//   - This package MUST NOT declare any //encore:api endpoints. Its
//     only purpose is to drive HTTP requests against MCPHandler and
//     read the mcp.tool_calls audit table; exposing RPCs here would
//     muddle scope and risk a production path landing in a test
//     subtree.
//
//   - This package MUST NOT acquire other responsibilities. It is a
//     test surface. The Service struct + initService below stay
//     empty. The Service struct's identity is purely a parser anchor;
//     no dependency injection, no per-request state, no lifecycle
//     hooks beyond the no-op initService.

package mcpaudittest

// Service is the Encore service anchor for the mcp audit integration
// test suite. Carries no state and exposes no APIs — the
// //encore:service annotation is what makes the cross-service HTTP
// call to /mcp and the mcp.tool_calls / org.organizations queries in
// mcpaudittest_test.go legal under Encore's parser.
//
//encore:service
type Service struct{}

// initService satisfies Encore's lifecycle contract for the service
// struct above. mcpaudittest has no per-request state; the struct
// stays empty. Encore calls this exactly once during service
// bring-up.
func initService() (*Service, error) {
	return &Service{}, nil
}
