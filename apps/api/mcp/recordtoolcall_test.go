// Tests for the mcp.tool_calls audit writer.
//
// Scope split:
//
//   - Pure-value tests (this file) — exercise the constants and the
//     nullable helper under plain `go test ./mcp/...`. The mcp
//     package root is loadable without the Encore runtime because
//     mcp/db.go uses the canonical BindDB late-bind shape
//     (a nil *sqldb.Database pointer + exported BindDB hook) and
//     mcp/cascade.go-style infrastructure resources do not live in
//     this package (cascade topics are on the deps service).
//
//   - Integration tests for the real DB INSERT path live in
//     mcphandler_test.go and run under `encore test ./mcp/...` —
//     they exercise MCPHandler end-to-end, which is the
//     load-bearing assertion for A-5 (one row per request, with
//     the correct trace_id).
//
// recordToolCall failure-path semantics: this writer is
// fire-and-forget by SPEC §8.1 contract. Unit tests therefore
// assert behavioural shape (constants, NULL-collapsing helper)
// rather than error propagation.

package mcp

import "testing"

func TestResultKindConstants(t *testing.T) {
	// Lock the literal spellings against the DDL CHECK constraint
	// (apps/api/db/migrations/0070_mcp.up.sql:67 — 'ok', 'rejected',
	// 'error'). A rename here that drifts from the DDL would
	// silently produce 23514 constraint violations at runtime.
	cases := []struct {
		got  ResultKind
		want string
	}{
		{ResultOK, "ok"},
		{ResultRejected, "rejected"},
		{ResultError, "error"},
	}
	for _, c := range cases {
		if string(c.got) != c.want {
			t.Fatalf("ResultKind = %q, want %q", c.got, c.want)
		}
	}
}

func TestNullable(t *testing.T) {
	t.Run("empty string returns nil pointer", func(t *testing.T) {
		if got := nullable(""); got != nil {
			t.Fatalf("nullable(empty) = %v, want nil", got)
		}
	})
	t.Run("non-empty string round-trips", func(t *testing.T) {
		got := nullable("hello")
		if got == nil {
			t.Fatalf("nullable(hello) = nil, want pointer")
		}
		if *got != "hello" {
			t.Fatalf("*nullable = %q, want hello", *got)
		}
	})
}
