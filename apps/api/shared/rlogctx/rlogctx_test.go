// Unit tests for the rlogctx binder.
//
// Test-runtime constraint (same as auth/authhandler_test.go): every
// top-level function in encore.dev/rlog (Info, Error, With, …) is a
// runtime stub that calls doPanic when invoked outside the Encore
// CLI. We therefore exercise only fieldsToKV — the pure-value
// flattener — under plain `go test`. Bind itself ends in a
// rlog.With call and is exercised under `encore test` via the MCP
// handler tests (which run the whole Encore runtime).

package rlogctx

import (
	"reflect"
	"testing"

	"encore.app/shared/tracectx"
)

func TestFieldsToKV(t *testing.T) {
	t.Run("all fields populated emits canonical order", func(t *testing.T) {
		got := fieldsToKV(tracectx.Fields{
			TraceID:   "01HABC",
			OrgID:     "O",
			ProjectID: "P",
			UserID:    "U",
			AgentKind: "claude-code",
			Tool:      "claim",
			Service:   "mcp",
		})
		want := []any{
			"trace_id", "01HABC",
			"org_id", "O",
			"project_id", "P",
			"user_id", "U",
			"agent_kind", "claude-code",
			"tool", "claim",
			"service", "mcp",
		}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("fieldsToKV = %+v, want %+v", got, want)
		}
	})

	t.Run("empty Fields produces empty slice", func(t *testing.T) {
		got := fieldsToKV(tracectx.Fields{})
		if len(got) != 0 {
			t.Fatalf("fieldsToKV(zero) = %+v, want empty", got)
		}
	})

	t.Run("partial fields elides empties", func(t *testing.T) {
		got := fieldsToKV(tracectx.Fields{
			TraceID: "T",
			Tool:    "ready",
		})
		want := []any{"trace_id", "T", "tool", "ready"}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("fieldsToKV = %+v, want %+v", got, want)
		}
	})

	t.Run("snake_case keys are normative per SPEC §8.2", func(t *testing.T) {
		// Lock the literal key spellings. A code change that
		// flips trace_id → traceID (or similar) breaks the
		// SPEC §8.2 contract and must fail loudly.
		if keyTraceID != "trace_id" {
			t.Fatalf("keyTraceID = %q, want trace_id", keyTraceID)
		}
		if keyOrgID != "org_id" {
			t.Fatalf("keyOrgID = %q, want org_id", keyOrgID)
		}
		if keyProjectID != "project_id" {
			t.Fatalf("keyProjectID = %q, want project_id", keyProjectID)
		}
		if keyUserID != "user_id" {
			t.Fatalf("keyUserID = %q, want user_id", keyUserID)
		}
		if keyAgentKind != "agent_kind" {
			t.Fatalf("keyAgentKind = %q, want agent_kind", keyAgentKind)
		}
		if keyTool != "tool" {
			t.Fatalf("keyTool = %q, want tool", keyTool)
		}
		if keyService != "service" {
			t.Fatalf("keyService = %q, want service", keyService)
		}
	})
}
