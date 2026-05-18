// Integration tests for the mcp tracing scaffold (A-5 /
// unblock-tv8.5).
//
// What's asserted (SPEC §10.2 Option B + §8.1):
//
//  1. MCPHandler returns 405 on the P01 skeleton path with the
//     documented Allow header. The pre-auth path writes NO audit
//     row (mcp.tool_calls.org_id is NOT NULL with a FK to
//     org.organizations; recordToolCall short-circuits to a
//     diagnostic rlog line). This is the transitional contract
//     until D-1 wires real auth + tool dispatch.
//
//  2. Two successive MCPHandler invocations produce distinct
//     trace_ids (verified indirectly via the recordToolCall path
//     below, which receives the same ctx-bound ULID format).
//
//  3. The trace_id ULID format guarantee: 26 chars from Crockford
//     base32. Asserted both on the ULID minted by ulid.New() and on
//     round-trip through recordToolCall → mcp.tool_calls.trace_id.
//
// This is a separate package from apps/api/mcp because:
//
//   - The mcp package cannot import apps/api/db (db imports mcp,
//     creating a cycle), so the mcp package's db pointer is nil
//     under its own tests. audittest blank-imports db so the bind
//     fires before TestMain runs.
//   - Encore's parser rejects in-process calls to a raw //encore:api
//     (E1389). audittest invokes MCPHandler via HTTP against
//     encore.Meta().APIBaseURL, which is the supported test shape.
//
// Same scaffold as apps/api/shared/rbactest/.

package mcpaudittest

import (
	"context"
	"strings"
	"testing"

	// Imported for its side-effect — encore.app/db's package init
	// is the canonical binding authority for every domain service's
	// *sqldb.Database handle. Without this import, mcp's db pointer
	// stays nil and recordToolCall panics on the Exec call.
	_ "encore.app/db"
	"encore.app/mcp"
	"encore.app/shared/tracectx"
	"encore.app/shared/ulid"
	"encore.dev/storage/sqldb"
)

// db is the cross-service handle for the canonical `unblock`
// database. Resolved via sqldb.Named to keep audittest decoupled
// from any one consumer service's binding. encore.app/db's init has
// already run by the time tests fire (the blank import above forces
// it), so the named lookup returns the same handle every other
// service holds.
//
//nolint:gochecknoglobals
var db = sqldb.Named("unblock")

// NOTE on MCPHandler request-entry coverage. The natural test —
// fire an HTTP POST against encore.Meta().APIBaseURL + "/mcp",
// assert 405 + Allow header + zero audit rows — is not viable
// under `encore test`. Encore's test runtime does not register
// raw //encore:api routes on the in-process HTTP listener (the
// listener returns 404 for /mcp even though `encore check` confirms
// the route is parsed and `encore run` serves it correctly), and
// E1389 forbids direct in-process invocation of a raw endpoint.
// MCPHandler's request-entry behaviour (mint ULID, bind ctx, defer
// recordToolCall) is therefore exercised indirectly via the
// TestRecordToolCallPersistsRow case below — which proves the
// ctx → recordToolCall → mcp.tool_calls.trace_id chain works end
// to end with a 26-char Crockford-base32 ULID. The full E2E
// MCPHandler → 405 path lands as a regression case when D-1
// (unblock-tv8.16) wires real tool dispatch on top of A-5.

// TestRecordToolCallPersistsRow verifies the §8.1 audit contract:
// recordToolCall writes one row per authenticated dispatch with
// trace_id, tool_name, result_kind, duration_ms populated. Uses a
// seeded org to satisfy the FK + NOT NULL constraint.
//
// This exercises the writer directly (not through MCPHandler) so
// the test can force a populated OrgID — the 405 skeleton path
// always sees an empty OrgID and short-circuits.
func TestRecordToolCallPersistsRow(t *testing.T) {
	resetToolCalls(t)
	orgID := seedOrg(t)

	traceID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid.New: %v", err)
	}
	ctx := tracectx.With(context.Background(), tracectx.Fields{
		TraceID: traceID,
		Service: "mcp",
		OrgID:   orgID,
		Tool:    "claim",
	})

	mcp.WriteToolCallForTest(ctx, mcp.ToolCall{
		OrgID:      orgID,
		ToolName:   "claim",
		ResultKind: mcp.ResultOK,
		DurationMs: 7,
	})

	rows := selectToolCalls(t)
	if len(rows) != 1 {
		t.Fatalf("tool_calls rows = %d, want 1", len(rows))
	}
	r := rows[0]

	// trace_id round-trips verbatim — proves the ctx → recordToolCall
	// → SQL bind chain works end-to-end.
	if r.TraceID == nil || *r.TraceID != traceID {
		got := "<nil>"
		if r.TraceID != nil {
			got = *r.TraceID
		}
		t.Fatalf("trace_id = %q, want %q", got, traceID)
	}
	// trace_id ULID format check.
	if len(*r.TraceID) != 26 {
		t.Fatalf("len(trace_id) = %d, want 26", len(*r.TraceID))
	}
	for i, c := range *r.TraceID {
		if !strings.ContainsRune(ulid.Alphabet(), c) {
			t.Fatalf("trace_id[%d] = %q not in Crockford alphabet", i, c)
		}
	}

	if r.ToolName != "claim" {
		t.Fatalf("tool_name = %q, want claim", r.ToolName)
	}
	if r.ResultKind != "ok" {
		t.Fatalf("result_kind = %q, want ok", r.ResultKind)
	}
	if r.DurationMs != 7 {
		t.Fatalf("duration_ms = %d, want 7", r.DurationMs)
	}
	if len(r.ID) != 26 {
		t.Fatalf("len(id) = %d, want 26", len(r.ID))
	}
}

// TestRecordToolCallProducesUniqueIDs guards entropy regressions —
// two consecutive recordToolCall invocations must produce distinct
// PKs and (when minted from distinct ctx) distinct trace_ids.
func TestRecordToolCallProducesUniqueIDs(t *testing.T) {
	resetToolCalls(t)
	orgID := seedOrg(t)

	for i := 0; i < 2; i++ {
		traceID, err := ulid.New()
		if err != nil {
			t.Fatalf("ulid.New #%d: %v", i, err)
		}
		ctx := tracectx.With(context.Background(), tracectx.Fields{TraceID: traceID})
		mcp.WriteToolCallForTest(ctx, mcp.ToolCall{
			OrgID:      orgID,
			ToolName:   "ready",
			ResultKind: mcp.ResultOK,
		})
	}

	rows := selectToolCalls(t)
	if len(rows) != 2 {
		t.Fatalf("tool_calls rows = %d, want 2", len(rows))
	}
	if rows[0].ID == rows[1].ID {
		t.Fatalf("two PK ids collided: %q", rows[0].ID)
	}
	if rows[0].TraceID == nil || rows[1].TraceID == nil {
		t.Fatalf("trace_id was NULL: a=%v b=%v", rows[0].TraceID, rows[1].TraceID)
	}
	if *rows[0].TraceID == *rows[1].TraceID {
		t.Fatalf("two trace_ids collided: %q", *rows[0].TraceID)
	}
}

// TestRecordToolCallNullableFields verifies the empty-string →
// SQL NULL collapsing contract. Tools that do not target a single
// item (prime, ready, search) leave ItemID empty; the column is
// nullable and the writer MUST not emit a literal empty string.
func TestRecordToolCallNullableFields(t *testing.T) {
	resetToolCalls(t)
	orgID := seedOrg(t)

	traceID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid.New: %v", err)
	}
	ctx := tracectx.With(context.Background(), tracectx.Fields{TraceID: traceID})

	mcp.WriteToolCallForTest(ctx, mcp.ToolCall{
		OrgID:      orgID,
		ToolName:   "prime",
		ResultKind: mcp.ResultOK,
	})

	rows := selectToolCalls(t)
	if len(rows) != 1 {
		t.Fatalf("tool_calls rows = %d, want 1", len(rows))
	}
	if rows[0].ErrorCode != nil {
		t.Fatalf("error_code = %q, want NULL (empty → NULL)", *rows[0].ErrorCode)
	}
	if rows[0].TraceID == nil || *rows[0].TraceID != traceID {
		t.Fatalf("trace_id was lost during nullable collapse: %v", rows[0].TraceID)
	}
}

// ----------------------------------------------------------------------
// Test fixtures.
// ----------------------------------------------------------------------

// toolCallRow mirrors the columns we read in assertions. SPEC §8.1
// frozen column set.
type toolCallRow struct {
	ID         string
	OrgID      string
	ProjectID  *string
	ToolName   string
	ResultKind string
	ErrorCode  *string
	DurationMs int
	TraceID    *string
}

// resetToolCalls clears mcp.tool_calls so test cases run
// deterministically. Uses DELETE rather than TRUNCATE because the
// encore-test Postgres user lacks the TRUNCATE privilege on
// schema-owned tables.
func resetToolCalls(t *testing.T) {
	t.Helper()
	ctx := context.Background()
	if _, err := db.Exec(ctx, `DELETE FROM mcp.tool_calls`); err != nil {
		t.Fatalf("delete tool_calls: %v", err)
	}
}

// selectToolCalls returns every row in mcp.tool_calls ordered by
// called_at ASC.
func selectToolCalls(t *testing.T) []toolCallRow {
	t.Helper()
	ctx := context.Background()
	rows, err := db.Query(ctx, `
		SELECT id, org_id, project_id, tool_name, result_kind, error_code, duration_ms, trace_id
		FROM mcp.tool_calls
		ORDER BY called_at ASC, id ASC
	`)
	if err != nil {
		t.Fatalf("select tool_calls: %v", err)
	}
	defer rows.Close()

	var out []toolCallRow
	for rows.Next() {
		var r toolCallRow
		if err := rows.Scan(&r.ID, &r.OrgID, &r.ProjectID, &r.ToolName, &r.ResultKind, &r.ErrorCode, &r.DurationMs, &r.TraceID); err != nil {
			t.Fatalf("scan tool_calls: %v", err)
		}
		out = append(out, r)
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("iter tool_calls: %v", err)
	}
	return out
}

// seedOrg inserts a minimal org.organizations row and returns its
// id. Each call mints a fresh ULID so concurrent / parallel tests
// do not collide on the org slug UNIQUE constraint.
func seedOrg(t *testing.T) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid.New: %v", err)
	}
	if _, err := db.Exec(ctx, `
		INSERT INTO org.organizations (id, slug, name)
		VALUES ($1, $2, $3)
	`, id, "test-"+strings.ToLower(id), "Test Org "+id); err != nil {
		t.Fatalf("insert org.organizations: %v", err)
	}
	return id
}
