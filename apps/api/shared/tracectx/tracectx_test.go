// Unit tests for the shared trace-context carrier. Runs under plain
// `go test` (no Encore runtime required) — the whole point of the
// package being Encore-free.

package tracectx

import (
	"context"
	"testing"
)

func TestWithAndFrom(t *testing.T) {
	t.Run("zero ctx returns no binding", func(t *testing.T) {
		f, ok := From(context.Background())
		if ok {
			t.Fatalf("From(empty) ok = true, want false")
		}
		if f != (Fields{}) {
			t.Fatalf("From(empty) fields = %+v, want zero", f)
		}
	})

	t.Run("With round-trips Fields", func(t *testing.T) {
		want := Fields{
			TraceID:   "01HZZZZZZZZZZZZZZZZZZZZZZZ",
			OrgID:     "org_abc",
			ProjectID: "prj_def",
			UserID:    "usr_ghi",
			AgentKind: "claude-code",
			Tool:      "claim",
			Service:   "mcp",
		}
		ctx := With(context.Background(), want)
		got, ok := From(ctx)
		if !ok {
			t.Fatalf("From(ctx) ok = false, want true")
		}
		if got != want {
			t.Fatalf("From(ctx) = %+v, want %+v", got, want)
		}
	})

	t.Run("With replaces, does not merge", func(t *testing.T) {
		ctx := With(context.Background(), Fields{TraceID: "T1", OrgID: "O1"})
		ctx = With(ctx, Fields{TraceID: "T2"}) // OrgID intentionally dropped
		got, _ := From(ctx)
		if got.TraceID != "T2" {
			t.Fatalf("TraceID = %q, want T2", got.TraceID)
		}
		if got.OrgID != "" {
			t.Fatalf("OrgID = %q, want empty after replace", got.OrgID)
		}
	})

	t.Run("nil ctx is safe on With and From", func(t *testing.T) {
		//nolint:staticcheck // SA1012: explicit nil-ctx test on purpose.
		ctx := With(nil, Fields{TraceID: "T"})
		if ctx == nil {
			t.Fatalf("With(nil, …) returned nil ctx")
		}
		got, ok := From(ctx)
		if !ok || got.TraceID != "T" {
			t.Fatalf("From(With(nil, …)) = (%+v, %v), want T,true", got, ok)
		}
		//nolint:staticcheck // SA1012: explicit nil-ctx test on purpose.
		f, ok := From(nil)
		if ok || f != (Fields{}) {
			t.Fatalf("From(nil) = (%+v, %v), want zero,false", f, ok)
		}
	})

	t.Run("TraceID helper returns bound id", func(t *testing.T) {
		ctx := With(context.Background(), Fields{TraceID: "01HABC"})
		if got := TraceID(ctx); got != "01HABC" {
			t.Fatalf("TraceID = %q, want 01HABC", got)
		}
	})

	t.Run("TraceID returns empty when no binding", func(t *testing.T) {
		if got := TraceID(context.Background()); got != "" {
			t.Fatalf("TraceID(empty) = %q, want empty", got)
		}
	})
}
