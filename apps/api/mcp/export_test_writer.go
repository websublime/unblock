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

import "context"

// WriteToolCallForTest is the integration-test-only re-export of
// recordToolCall. See the file-level doc-comment for the rationale.
// Production code MUST NOT call this — it MUST call recordToolCall
// directly.
func WriteToolCallForTest(ctx context.Context, call ToolCall) {
	recordToolCall(ctx, call)
}
