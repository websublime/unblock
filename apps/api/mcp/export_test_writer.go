// export_test_writer.go exposes recordToolCall under an exported
// name (WriteToolCallForTest) so the audittest integration package
// (apps/api/mcp/audittest/) can exercise the writer directly. The
// natural Go pattern — `var WriteToolCallForTest = recordToolCall`
// inside an `export_test.go` file — does not work here because
// `export_test.go` is package-local (mcp_test, not the audittest
// sub-package). The audittest package needs an exported symbol on
// the production import path.
//
// Why an exported symbol on the production path is acceptable here:
//
//   - WriteToolCallForTest is a thin re-export of recordToolCall,
//     not a separate code path. Tests exercising it exercise the
//     real production writer.
//
//   - The name is suffixed `ForTest` so a reader landing on it
//     immediately understands the audience. Production callers
//     SHOULD use recordToolCall (unexported); this hook is for the
//     cross-package integration test in mcp/audittest/ only.
//
//   - The alternative — moving recordToolCall into a separate
//     exported package — would split the writer from MCPHandler
//     (which lives in this package), forcing every MCP tool
//     handler in D-1+ to import a sub-package for what is logically
//     the same module's audit writer. The cost (one named
//     re-export with a clear suffix) is lower than that surface
//     drift.
//
// If a future contributor adds non-test exported callers to
// WriteToolCallForTest, that is a smell — the suffix is the audit
// trail and the package-local lint (apps/api/shared/lint/) is the
// canonical home for a guard if drift becomes a problem.

package mcp

import (
	"context"
	"net/http"

	"encore.app/workitems"
)

// WriteToolCallForTest is the integration-test-only re-export of
// recordToolCall. See the file-level doc-comment for the rationale.
// Production code MUST NOT call this — it MUST call recordToolCall
// directly.
func WriteToolCallForTest(ctx context.Context, call ToolCall) {
	recordToolCall(ctx, call)
}

// ServeMCPForTest is the integration-test-only re-export of
// serveMCP. The natural test path — HTTP against
// encore.Meta().APIBaseURL + "/mcp" — does not work under
// `encore test` (the A-5 DEVIATION recorded in
// apps/api/shared/mcpaudittest/mcpaudittest_test.go:62-75: encore's
// in-process test listener does not route raw //encore:api routes;
// E1387/E1389 forbid in-process references to MCPHandler). The
// test suite wraps ServeMCPForTest in httptest.NewServer to drive
// the transport behaviour end-to-end on a real http.Handler.
//
// Production callers MUST NOT use this — the parsed
// //encore:api public raw path=/mcp is the only public route.
// Lint convention: any non-test reference to ServeMCPForTest is a
// smell — the suffix is the audit trail.
func ServeMCPForTest(w http.ResponseWriter, r *http.Request) {
	serveMCP(w, r)
}

// SetAppendIntentCommentForTest overrides the package-level
// appendIntentComment seam (handler_set_state.go) and returns a restore
// func the caller MUST invoke (typically via defer) to reinstate the
// production binding. It exists so the cross-package mcpaudittest suite
// can force the post-commit AppendComment to fail and exercise the
// §6.2 Tool 13 intent_comment partial-failure path — the §7.1
// warnings[] + §8.1.1 warning_codes dropped-path that AC#3 of
// unblock-tv8.63 requires. There is NO black-box input that makes
// AppendComment fail after SetStateColumns commits (see the seam's
// doc-comment + INVESTIGATION risk R1), so a test double is the only
// honest way to cover this branch.
//
// Production callers MUST NOT use this — the suffix is the audit trail;
// the only caller is the partial-failure integration test.
func SetAppendIntentCommentForTest(fn func(ctx context.Context, req *workitems.AppendCommentRequest) error) (restore func()) {
	prev := appendIntentComment
	appendIntentComment = fn
	return func() { appendIntentComment = prev }
}
