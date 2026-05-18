// d5_tools_test.go covers the D-5 (unblock-tv8.20) acceptance matrix
// for MCP tools 11 (`add_dependency`) and 12 (`remove_dependency`).
//
// Reuses the d2Fixture / callTool / assertStructuredEchoesText
// harness — same package, full Bearer-auth roundtrip through
// MCPHandler. Each test seeds an isolated org+user+project+api_key
// tuple so the §7 envelopes reach the wire with real Identity
// propagation through withIdentity.
//
// Coverage matrix (bead unblock-tv8.20 AC, rewritten per orchestrator
// DECISION 2026-05-18):
//
//   - addDependency_HappyPath: from_item + to_item in same project
//     succeed; structuredContent.edge populated with id/from_item/
//     to_item/kind/created_at; deps.dependencies row landed.
//   - addDependency_DefaultKindBlocks: omitting kind defaults to
//     "blocks" (substitution owned by deps.AddEdgeInTx, NOT this
//     handler — the test asserts the boundary contract).
//   - addDependency_CycleDetected (AC #1): seed a→b→c via direct SQL
//     then add_dependency(c→a); response is §7 CYCLE_DETECTED with
//     data.cycle_path populated as a typed []string (the
//     deps.AddEdgeInTx dual-encoding of cycle_path/cycle_path_list
//     survives the gob round-trip).
//   - addDependency_CrossProjectRejected (AC #2): two projects in the
//     same org, one item in each; add_dependency rejected with
//     VALIDATION data.field="to_item_id".
//   - addDependency_MissingFromItemID / MissingToItemID: empty
//     boundary inputs surface §7 VALIDATION with the matching
//     data.field — clearer than the deps-layer 'missing' message.
//   - removeDependency_HappyPath_ByEdgeID: delete by edge_id;
//     structuredContent.{removed,to_item_now_ready} populated; row
//     gone from deps.dependencies.
//   - removeDependency_HappyPath_ByComposite: delete by (from, to,
//     kind); same shape as above.
//   - removeDependency_ToItemNowReady (AC #3): seed item_a (Done,
//     so the closure CTE counts it as a satisfied blocker) blocking
//     item_b (Backlog, is_ready=false because there is also a second
//     unsatisfied blocker); call remove_dependency on the OTHER edge;
//     structuredContent.to_item_now_ready=true and item_b.is_ready=
//     true in workitems.items by direct SQL read.
//   - removeDependency_WritesCascadeEvent (AC #5): exactly one
//     deps.cascade_events row with kind='edge_removed' lands after
//     a successful remove_dependency call (the inline INSERT inside
//     deps.RemoveEdge writes it; the subscriber's attempted INSERT
//     collapses via ON CONFLICT — exactly-one).
//   - d5_AuditRowsCarryToolName: add_dependency + remove_dependency
//     dispatches each write one mcp.tool_calls row with the matching
//     tool_name.

package mcpaudittest

import (
	"context"
	"encoding/json"
	"testing"

	"encore.app/shared/ulid"
)

// =============================================================================
// helpers
// =============================================================================

// seedItemForEdge inserts a Backlog/unclaimed task with explicit
// is_ready value. Edge tests need both ready and non-ready endpoints
// so the recomputeReady result is observable post-remove. Caller owns
// the cleanup via t.Cleanup chained from the helper.
func seedItemForEdge(t *testing.T, orgID, projectID, status string, isReady bool) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, priority,
		    is_ready, created_at, updated_at)
		 VALUES ($1, $2, $3, 'task', $4, $5, 'P2', $6, now(), now())`,
		id, orgID, projectID, "d5-edge-"+id[len(id)-6:], status, isReady,
	); err != nil {
		t.Fatalf("insert edge item: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.items WHERE id = $1`, id) })
	return id
}

// seedItemForEdgeInProject is identical to seedItemForEdge but takes
// an explicit projectID so the cross-project test can place items in
// a sibling project.
func seedItemForEdgeInProject(t *testing.T, orgID, projectID string) string {
	t.Helper()
	return seedItemForEdge(t, orgID, projectID, "Backlog", false)
}

// seedSiblingProject inserts a second project in the same org so the
// cross-project rejection path has a real foreign key target. Cleanup
// runs at test end via t.Cleanup.
func seedSiblingProject(t *testing.T, orgID string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		id, orgID, "p-"+id[len(id)-8:], "d5 sibling project",
	); err != nil {
		t.Fatalf("insert sibling project: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM org.projects WHERE id = $1`, id) })
	return id
}

// seedEdgeDirect inserts a deps.dependencies row directly via SQL —
// bypasses deps.AddEdge so the cycle test can pre-seed a chain
// without firing the publisher path under encore test (et.Topic is
// invisible from this harness anyway, but the direct insert is
// simpler and equivalent for read-side fixtures). Caller owns the
// cleanup.
func seedEdgeDirect(t *testing.T, fromItem, toItem, kind string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid edge: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO deps.dependencies (id, from_item, to_item, kind)
		 VALUES ($1, $2, $3, $4)`,
		id, fromItem, toItem, kind,
	); err != nil {
		t.Fatalf("insert edge: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM deps.dependencies WHERE id = $1`, id) })
	return id
}

// edgeWireOut models the §6.2 Tool 11 wire shape — the
// structuredContent JSON object carrying { "edge": Edge }.
type edgeWireOut struct {
	Edge struct {
		ID        string `json:"id"`
		FromItem  string `json:"from_item"`
		ToItem    string `json:"to_item"`
		Kind      string `json:"kind"`
		CreatedAt string `json:"created_at"`
		CreatedBy string `json:"created_by,omitempty"`
	} `json:"edge"`
}

func decodeEdgeOut(t *testing.T, raw json.RawMessage) edgeWireOut {
	t.Helper()
	var out edgeWireOut
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("decodeEdgeOut: %v; raw=%s", err, string(raw))
	}
	return out
}

// removeWireOut models the §6.2 Tool 12 wire shape —
// { "removed": bool, "to_item_now_ready": bool }.
type removeWireOut struct {
	Removed        bool `json:"removed"`
	ToItemNowReady bool `json:"to_item_now_ready"`
}

func decodeRemoveOut(t *testing.T, raw json.RawMessage) removeWireOut {
	t.Helper()
	var out removeWireOut
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("decodeRemoveOut: %v; raw=%s", err, string(raw))
	}
	return out
}

// =============================================================================
// add_dependency
// =============================================================================

// TestD5_AddDependencyHappyPath asserts the §6.2 Tool 11 happy path:
// from_item + to_item in same project, kind="blocks"; response carries
// the structured Edge; deps.dependencies row landed.
func TestD5_AddDependencyHappyPath(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	from := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	to := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)

	env := callTool(t, fx.RawKey, "add_dependency", map[string]any{
		"from_item_id": from,
		"to_item_id":   to,
		"kind":         "blocks",
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeEdgeOut(t, res.StructuredContent)

	if got.Edge.ID == "" {
		t.Fatalf("edge.id empty")
	}
	if got.Edge.FromItem != from {
		t.Fatalf("edge.from_item = %q, want %q", got.Edge.FromItem, from)
	}
	if got.Edge.ToItem != to {
		t.Fatalf("edge.to_item = %q, want %q", got.Edge.ToItem, to)
	}
	if got.Edge.Kind != "blocks" {
		t.Fatalf("edge.kind = %q, want blocks", got.Edge.Kind)
	}
	if got.Edge.CreatedAt == "" {
		t.Fatalf("edge.created_at empty")
	}

	// DB read-back: row persisted.
	ctx := context.Background()
	var dbID string
	if err := db.QueryRow(ctx, `SELECT id FROM deps.dependencies WHERE id = $1`, got.Edge.ID).Scan(&dbID); err != nil {
		t.Fatalf("edge not persisted: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM deps.dependencies WHERE id = $1`, got.Edge.ID) })
}

// TestD5_AddDependencyDefaultKindBlocks asserts that omitting the
// optional kind argument substitutes "blocks" per SPEC §6.2 Tool 11
// line 1484. The substitution is owned by deps.AddEdgeInTx
// (deps/deps.go:192-195), NOT the MCP handler — the handler passes
// empty strings through verbatim. This test pins the boundary
// contract: the wire-observable kind on the returned Edge is "blocks"
// when the input omits the field.
func TestD5_AddDependencyDefaultKindBlocks(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	from := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	to := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)

	env := callTool(t, fx.RawKey, "add_dependency", map[string]any{
		"from_item_id": from,
		"to_item_id":   to,
		// kind deliberately omitted
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeEdgeOut(t, res.StructuredContent)
	if got.Edge.Kind != "blocks" {
		t.Fatalf("default kind: edge.kind = %q, want blocks", got.Edge.Kind)
	}
	ctx := context.Background()
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM deps.dependencies WHERE id = $1`, got.Edge.ID) })
}

// TestD5_AddDependencyCycleDetected covers AC #1: forming a cycle
// surfaces §7 CYCLE_DETECTED with data.cycle_path populated as a
// typed []string. Seed chain a→b→c via direct SQL (the publisher path
// is invisible to this harness anyway) and then call add_dependency
// (c→a) which closes the cycle.
//
// The dual-encoding of cycle_path / cycle_path_list at the deps
// layer (deps/deps.go:329-330) survives the gob round-trip across the
// //encore:api boundary; errmap (errmap.go:227-231) prefers the typed
// slice. We assert the JSON wire form carries a non-empty cycle_path
// array.
func TestD5_AddDependencyCycleDetected(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	a := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	b := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	c := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)

	// Pre-seed a→b and b→c via direct SQL so the chain exists for the
	// cycle CTE to discover when we attempt to close it with c→a.
	_ = seedEdgeDirect(t, a, b, "blocks")
	_ = seedEdgeDirect(t, b, c, "blocks")

	env := callTool(t, fx.RawKey, "add_dependency", map[string]any{
		"from_item_id": c,
		"to_item_id":   a,
		"kind":         "blocks",
	})
	if env.Error == nil {
		t.Fatalf("expected CYCLE_DETECTED; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "CYCLE_DETECTED" {
		t.Fatalf("error.data.kind = %q, want CYCLE_DETECTED", data.Kind)
	}
	// cycle_path is a typed []string on the wire (errmap projects the
	// dual-encoded Meta into a JSON array). json.Unmarshal lands it as
	// []interface{}; assert at least one element so the path is real.
	rawPath, ok := data.Details["cycle_path"]
	if !ok {
		t.Fatalf("error.data.details.cycle_path missing; details=%+v", data.Details)
	}
	pathSlice, ok := rawPath.([]interface{})
	if !ok {
		t.Fatalf("cycle_path type = %T, want []interface{} (JSON array); raw=%+v", rawPath, rawPath)
	}
	if len(pathSlice) == 0 {
		t.Fatalf("cycle_path is empty; want at least one element")
	}
	// Sanity: from and to surface on the envelope per errmap.go:215-220.
	if got, _ := data.Details["from"].(string); got != c {
		t.Fatalf("error.data.details.from = %q, want %q", got, c)
	}
	if got, _ := data.Details["to"].(string); got != a {
		t.Fatalf("error.data.details.to = %q, want %q", got, a)
	}
}

// TestD5_AddDependencyCrossProjectRejected covers AC #2: from_item and
// to_item in DIFFERENT projects in the same org are rejected with §7
// VALIDATION data.field="to_item_id". The rejection lives in
// deps.AddEdge (deps/deps.go:258-272) which sets Meta.field=
// "to_item_id"; errmap projects it to the §7 envelope without changes.
func TestD5_AddDependencyCrossProjectRejected(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	otherProject := seedSiblingProject(t, fx.OrgID)

	fromInP1 := seedItemForEdgeInProject(t, fx.OrgID, fx.ProjectID)
	toInP2 := seedItemForEdgeInProject(t, fx.OrgID, otherProject)

	env := callTool(t, fx.RawKey, "add_dependency", map[string]any{
		"from_item_id": fromInP1,
		"to_item_id":   toInP2,
		"kind":         "blocks",
	})
	if env.Error == nil {
		t.Fatalf("expected VALIDATION on cross-project edge; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "to_item_id" {
		t.Fatalf("error.data.details.field = %q, want \"to_item_id\"", got)
	}
}

// TestD5_AddDependencyMissingFromItemID asserts the MCP-boundary
// guard: empty from_item_id surfaces §7 VALIDATION with the matching
// data.field. The handler's own pre-check is clearer than the
// deps-layer 'missing' message (the deps layer would still reject,
// but the MCP-level message is intentional per handler_create.go:80-86
// pattern).
//
// Defense-in-depth: an EMPTY field (present but zero string) bypasses
// the SDK's required-field check — the handler's guard is what makes
// the error envelope precise.
func TestD5_AddDependencyMissingFromItemID(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	to := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)

	env := callTool(t, fx.RawKey, "add_dependency", map[string]any{
		"from_item_id": "",
		"to_item_id":   to,
		"kind":         "blocks",
	})
	if env.Error == nil {
		t.Fatalf("expected VALIDATION on empty from_item_id; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "from_item_id" {
		t.Fatalf("error.data.details.field = %q, want \"from_item_id\"", got)
	}
}

// TestD5_AddDependencyMissingToItemID: symmetric guard for the
// to_item_id field.
func TestD5_AddDependencyMissingToItemID(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	from := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)

	env := callTool(t, fx.RawKey, "add_dependency", map[string]any{
		"from_item_id": from,
		"to_item_id":   "",
		"kind":         "blocks",
	})
	if env.Error == nil {
		t.Fatalf("expected VALIDATION on empty to_item_id; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "to_item_id" {
		t.Fatalf("error.data.details.field = %q, want \"to_item_id\"", got)
	}
}

// =============================================================================
// remove_dependency
// =============================================================================

// TestD5_RemoveDependencyHappyPathByEdgeID exercises the §6.2 Tool 12
// selection-by-edge_id path. Seed an edge via direct SQL (decoupled
// from the publisher path), call remove_dependency with edge_id, and
// assert structuredContent + DB read-back.
func TestD5_RemoveDependencyHappyPathByEdgeID(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	from := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Done", true)
	to := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	edgeID := seedEdgeDirect(t, from, to, "blocks")

	env := callTool(t, fx.RawKey, "remove_dependency", map[string]any{
		"edge_id": edgeID,
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeRemoveOut(t, res.StructuredContent)

	if !got.Removed {
		t.Fatalf("removed = false, want true")
	}

	// DB read-back: edge is gone.
	ctx := context.Background()
	var count int
	if err := db.QueryRow(ctx,
		`SELECT count(*) FROM deps.dependencies WHERE id = $1`, edgeID,
	).Scan(&count); err != nil {
		t.Fatalf("count edges: %v", err)
	}
	if count != 0 {
		t.Fatalf("edge still present after remove: count = %d, want 0", count)
	}
}

// TestD5_RemoveDependencyHappyPathByComposite exercises the §6.2
// Tool 12 selection-by-(from,to,kind) path. Equivalent to the
// edge_id path but the input shape differs.
func TestD5_RemoveDependencyHappyPathByComposite(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	from := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Done", true)
	to := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	_ = seedEdgeDirect(t, from, to, "blocks")

	env := callTool(t, fx.RawKey, "remove_dependency", map[string]any{
		"from_item_id": from,
		"to_item_id":   to,
		"kind":         "blocks",
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeRemoveOut(t, res.StructuredContent)
	if !got.Removed {
		t.Fatalf("removed = false, want true")
	}

	ctx := context.Background()
	var count int
	if err := db.QueryRow(ctx,
		`SELECT count(*) FROM deps.dependencies
		 WHERE from_item = $1 AND to_item = $2 AND kind = 'blocks'`,
		from, to,
	).Scan(&count); err != nil {
		t.Fatalf("count edges: %v", err)
	}
	if count != 0 {
		t.Fatalf("edge still present after composite remove: count = %d, want 0", count)
	}
}

// TestD5_RemoveDependencyToItemNowReady covers AC #3: the inline
// is_ready recompute on the direct to_item flips correctly when the
// removed edge was the LAST unsatisfied blocker.
//
// Fixture: item_a (Done, satisfied blocker) and item_blocker
// (Backlog, unsatisfied blocker) both block item_b (Backlog,
// is_ready=false). Removing the item_blocker → item_b edge leaves
// only the satisfied a → b edge — item_b's is_ready flips to true.
// to_item_now_ready in structuredContent reflects the single-hop
// view per SPEC §6.2 Tool 12 lines 1578-1586.
func TestD5_RemoveDependencyToItemNowReady(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	ctx := context.Background()

	itemA := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Done", true)
	itemBlocker := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	itemB := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)

	// Both A and blocker block B; A is Done (counts as satisfied),
	// blocker is not (so B is_ready stays false).
	_ = seedEdgeDirect(t, itemA, itemB, "blocks")
	blockerEdge := seedEdgeDirect(t, itemBlocker, itemB, "blocks")

	env := callTool(t, fx.RawKey, "remove_dependency", map[string]any{
		"edge_id": blockerEdge,
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeRemoveOut(t, res.StructuredContent)
	if !got.Removed {
		t.Fatalf("removed = false, want true")
	}
	if !got.ToItemNowReady {
		t.Fatalf("to_item_now_ready = false, want true (only satisfied blocker remains)")
	}

	// Direct SQL read-back: item_b.is_ready = true.
	var isReady bool
	if err := db.QueryRow(ctx,
		`SELECT is_ready FROM workitems.items WHERE id = $1`, itemB,
	).Scan(&isReady); err != nil {
		t.Fatalf("query is_ready: %v", err)
	}
	if !isReady {
		t.Fatalf("workitems.items[%s].is_ready = false, want true after Regime-A recompute", itemB)
	}
}

// TestD5_RemoveDependencyWritesCascadeEvent covers AC #5: exactly one
// deps.cascade_events row with kind='edge_removed' lands per logical
// remove_dependency call. The inline INSERT inside deps.RemoveEdge
// writes it (round-6 tension #1); the subscriber's attempted INSERT
// collapses via ON CONFLICT (event_id, triggered_by_item_id) DO
// NOTHING — exactly-one is the invariant.
//
// We assert on the directly-observable side-effect: read the
// cascade_events table by triggered_by_item_id AFTER the call and
// count rows with kind='edge_removed'. The subscriber path is
// invisible from this harness (httptest goroutine — see
// d3_tools_test.go:319-326 comment), so we are observing only the
// inline insert; the no-op subscriber collapse would not add a row
// anyway.
func TestD5_RemoveDependencyWritesCascadeEvent(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	ctx := context.Background()

	from := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Done", true)
	to := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	edgeID := seedEdgeDirect(t, from, to, "blocks")

	env := callTool(t, fx.RawKey, "remove_dependency", map[string]any{
		"edge_id": edgeID,
	})
	res := assertStructuredEchoesText(t, env)
	_ = decodeRemoveOut(t, res.StructuredContent)

	var count int
	if err := db.QueryRow(ctx,
		`SELECT count(*) FROM deps.cascade_events
		  WHERE triggered_by_item_id = $1 AND kind = 'edge_removed'`,
		to,
	).Scan(&count); err != nil {
		t.Fatalf("count cascade_events: %v", err)
	}
	if count != 1 {
		t.Fatalf("cascade_events kind='edge_removed' rows for to=%s: count = %d, want 1", to, count)
	}
	// Clean up the audit row so cross-test isolation is preserved.
	t.Cleanup(func() {
		_, _ = db.Exec(ctx,
			`DELETE FROM deps.cascade_events WHERE triggered_by_item_id = $1 AND kind = 'edge_removed'`,
			to,
		)
	})
}

// =============================================================================
// audit rows
// =============================================================================

// TestD5_AuditRowsCarryToolName: each add_dependency + remove_dependency
// dispatch writes one mcp.tool_calls row with the matching tool_name.
// SPEC §8.1 — completes the audit coverage matrix alongside D-2, D-3,
// and D-4.
func TestD5_AuditRowsCarryToolName(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	from := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)
	to := seedItemForEdge(t, fx.OrgID, fx.ProjectID, "Backlog", false)

	envAdd := callTool(t, fx.RawKey, "add_dependency", map[string]any{
		"from_item_id": from,
		"to_item_id":   to,
		"kind":         "blocks",
	})
	res := assertStructuredEchoesText(t, envAdd)
	addOut := decodeEdgeOut(t, res.StructuredContent)
	ctx := context.Background()
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM deps.dependencies WHERE id = $1`, addOut.Edge.ID) })

	_ = callTool(t, fx.RawKey, "remove_dependency", map[string]any{
		"edge_id": addOut.Edge.ID,
	})
	// Clean the cascade_events row written by RemoveEdge.
	t.Cleanup(func() {
		_, _ = db.Exec(ctx,
			`DELETE FROM deps.cascade_events WHERE triggered_by_item_id = $1 AND kind = 'edge_removed'`,
			to,
		)
	})

	rows := selectToolCalls(t)
	have := map[string]int{}
	for _, r := range rows {
		have[r.ToolName]++
	}
	for _, want := range []string{"add_dependency", "remove_dependency"} {
		if have[want] < 1 {
			t.Fatalf("audit row for tool_name=%q: count=%d, want >=1; rows=%+v", want, have[want], rows)
		}
	}
}
