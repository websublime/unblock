// d6_tools_test.go covers the D-6 (bead unblock-tv8.21) acceptance
// matrix for MCP tools 13 (`set_state`) and 14 (`get_state`).
//
// Reuses the d2Fixture / callTool / assertStructuredEchoesText
// harness — full Bearer-auth roundtrip through MCPHandler. Each test
// seeds an isolated org+user+project+api_key tuple so the §7
// envelopes reach the wire with real Identity propagation through
// withIdentity.
//
// Coverage matrix (bead unblock-tv8.21 AC):
//
//   - setState_I1AutoResetsQA (AC #1): writing review_state=
//     needs_rework on a (done, approved, passed) item auto-resets
//     qa_state to pending in the same write.
//   - setState_I2QAFailedRequiresReviewApproved (AC #2): qa_state=
//     failed with review_state != approved returns §7
//     PRECONDITION_NOT_MET with data.invariant=
//     qa_failed_requires_review_approved.
//   - setState_I4ReviewChangeRequiresImplDone (AC #3): review_state
//     change with impl_state=pending returns §7
//     PRECONDITION_NOT_MET with data.invariant=
//     review_change_requires_impl_done.
//   - setState_I5ImplDoneToPendingRequiresReworkPath (AC #4):
//     impl_state=pending request on an (impl=done, review=approved,
//     qa=passed) item returns §7 PRECONDITION_NOT_MET with
//     data.invariant=impl_done_to_pending_requires_rework_path.
//   - setState_IntentCommentWrittenAtomically: optional
//     intent_comment is persisted alongside a successful state
//     mutation (best-effort atomic per orchestrator DECISION
//     2026-05-18 on bead unblock-tv8.21).
//   - setState_IntentCommentValidationFailsBeforeStateWrite: an
//     invalid intent_comment (empty body) returns §7 VALIDATION
//     BEFORE the state mutation runs — the item's state columns are
//     unchanged in the DB.
//   - getState_ReturnsAllStateDimensions: every state column +
//     materialised pipeline_stage + claim columns surface on the
//     wire.
//   - getState_RecentKindsOneRowPerKind (AC #5): get_state returns
//     recent_kinds with one row per kind, picking the most recent
//     comment per kind by created_at desc.
//   - d6_AuditRowsCarryToolName: set_state + get_state dispatches
//     each write one mcp.tool_calls row with the matching tool_name.

package mcpaudittest

import (
	"context"
	"encoding/json"
	"fmt"
	"testing"
	"time"

	"encore.app/shared/ulid"
)

// =============================================================================
// helpers
// =============================================================================

// seedItemForState inserts a workitems.items row with the requested
// state columns + claim binding. The caller controls every
// dimension so each invariant test sets up the precise precondition
// the rule was designed to gate.
//
// claimedBy may be empty — the item lands unclaimed and impl_state=
// done is then forbidden by the structural impl_done_requires_claim
// pre-check.
//
// Returns the ULID.
func seedItemForState(t *testing.T, orgID, projectID string, impl, review, qa, pipe, claimedBy string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	// The items_claim_status_chk CHECK constraint requires status =
	// 'InProgress' (or 'Done') whenever claimed_by_id IS NOT NULL.
	// Pick the row status to satisfy that gate based on the test
	// fixture intent.
	status := "Backlog"
	if claimedBy != "" {
		status = "InProgress"
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, priority,
		    impl_state, review_state, qa_state, pipeline_state,
		    claimed_by_id, claimed_at, claimed_by_agent,
		    created_at, updated_at)
		 VALUES ($1, $2, $3, 'task', $4, $5, 'P2',
		         $6, $7, $8, $9,
		         NULLIF($10, ''),
		         CASE WHEN $10 = '' THEN NULL ELSE now() END,
		         CASE WHEN $10 = '' THEN NULL ELSE 'claude-code' END,
		         now(), now())`,
		id, orgID, projectID,
		"d6-state-"+id[len(id)-6:], status,
		impl, review, qa, pipe,
		claimedBy,
	); err != nil {
		t.Fatalf("insert state-fixture item: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.items WHERE id = $1`, id) })
	return id
}

// seedCommentDirect inserts a workitems.comments row directly via
// SQL. Used by the get_state recent_kinds test to land multiple
// comments per kind with explicit created_at offsets so the
// DISTINCT ON (kind) ORDER BY kind, created_at DESC contract is
// verifiable.
func seedCommentDirect(t *testing.T, itemID, kind, status, body string, createdAtOffset time.Duration) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid comment: %v", err)
	}
	// comments_author_chk requires author_id OR author_agent populated;
	// the get_state test only cares about the (kind, status,
	// comment_id, created_at) tuple so any author stub satisfies the
	// constraint without affecting the recent_kinds projection.
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, author_agent, kind, status, body, created_at, updated_at)
		 VALUES ($1, $2, 'claude-code', $3, $4, $5,
		         now() + ($6 || ' microseconds')::interval,
		         now() + ($6 || ' microseconds')::interval)`,
		id, itemID, kind, status, body,
		fmt.Sprintf("%d", createdAtOffset.Microseconds()),
	); err != nil {
		t.Fatalf("insert comment: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.comments WHERE id = $1`, id) })
	return id
}

// setStateWireOut models the §6.2 Tool 13 wire shape — the
// structuredContent JSON object carrying { "item": Item }.
type setStateWireOut struct {
	Item struct {
		ID            string `json:"id"`
		ImplState     string `json:"impl_state"`
		ReviewState   string `json:"review_state"`
		QAState       string `json:"qa_state"`
		PipelineState string `json:"pipeline_state"`
	} `json:"item"`
}

func decodeSetStateOut(t *testing.T, raw json.RawMessage) setStateWireOut {
	t.Helper()
	var out setStateWireOut
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("decodeSetStateOut: %v; raw=%s", err, string(raw))
	}
	return out
}

// getStateWireOut models the §6.2 Tool 14 structuredContent shape.
type getStateWireOut struct {
	ImplState     string `json:"impl_state"`
	ReviewState   string `json:"review_state"`
	QAState       string `json:"qa_state"`
	PipelineState string `json:"pipeline_state"`
	PipelineStage string `json:"pipeline_stage,omitempty"`
	IsReady       bool   `json:"is_ready"`
	ClaimedByID   string `json:"claimed_by_id,omitempty"`
	ClaimedAt     string `json:"claimed_at,omitempty"`
	RecentKinds   []struct {
		Kind      string `json:"kind"`
		Status    string `json:"status"`
		CommentID string `json:"comment_id"`
		CreatedAt string `json:"created_at"`
	} `json:"recent_kinds"`
}

func decodeGetStateOut(t *testing.T, raw json.RawMessage) getStateWireOut {
	t.Helper()
	var out getStateWireOut
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("decodeGetStateOut: %v; raw=%s", err, string(raw))
	}
	return out
}

// readItemStateColumns reads the live state columns from
// workitems.items for post-write assertions.
func readItemStateColumns(t *testing.T, itemID string) (impl, review, qa, pipe string) {
	t.Helper()
	ctx := context.Background()
	if err := db.QueryRow(ctx,
		`SELECT impl_state, review_state, qa_state, pipeline_state
		   FROM workitems.items WHERE id = $1`,
		itemID,
	).Scan(&impl, &review, &qa, &pipe); err != nil {
		t.Fatalf("read state columns: %v", err)
	}
	return
}

// =============================================================================
// set_state — invariants
// =============================================================================

// TestD6_SetStateI1AutoResetsQA covers AC #1: writing review_state=
// needs_rework on a (done, approved, passed) item auto-resets
// qa_state to pending in the same write (I-1 is auto-applied, not a
// rejection).
func TestD6_SetStateI1AutoResetsQA(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "needs_rework",
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeSetStateOut(t, res.StructuredContent)

	if got.Item.ReviewState != "needs_rework" {
		t.Fatalf("item.review_state = %q, want needs_rework", got.Item.ReviewState)
	}
	if got.Item.QAState != "pending" {
		t.Fatalf("I-1: item.qa_state = %q, want pending (auto-reset)", got.Item.QAState)
	}

	// DB read-back: both columns reflect the auto-reset.
	impl, review, qa, _ := readItemStateColumns(t, itemID)
	if impl != "done" {
		t.Fatalf("DB impl_state = %q, want done (unchanged)", impl)
	}
	if review != "needs_rework" {
		t.Fatalf("DB review_state = %q, want needs_rework", review)
	}
	if qa != "pending" {
		t.Fatalf("DB qa_state = %q, want pending (I-1 auto-reset)", qa)
	}
}

// TestD6_SetStateI2QAFailedRequiresReviewApproved covers AC #2:
// qa_state=failed with review_state != approved returns §7
// PRECONDITION_NOT_MET with data.invariant=
// qa_failed_requires_review_approved.
func TestD6_SetStateI2QAFailedRequiresReviewApproved(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	// Start from (done, pending, pending) so I-4 doesn't fire on
	// review_state interactions — we only want to trigger I-2 by
	// raising qa_state to failed while review stays at pending.
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "pending", "pending", "running", fx.UserID)

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":  itemID,
		"qa_state": "failed",
	})
	if env.Error == nil {
		t.Fatalf("expected PRECONDITION_NOT_MET I-2; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("error.data.kind = %q, want PRECONDITION_NOT_MET", data.Kind)
	}
	if got, _ := data.Details["invariant"].(string); got != "qa_failed_requires_review_approved" {
		t.Fatalf("error.data.details.invariant = %q, want qa_failed_requires_review_approved", got)
	}

	// DB read-back: state columns unchanged.
	_, _, qa, _ := readItemStateColumns(t, itemID)
	if qa != "pending" {
		t.Fatalf("DB qa_state = %q after rejection, want pending (unchanged)", qa)
	}
}

// TestD6_SetStateI4ReviewChangeRequiresImplDone covers AC #3:
// review_state change to approved/needs_rework with impl_state=
// pending returns §7 PRECONDITION_NOT_MET with data.invariant=
// review_change_requires_impl_done.
func TestD6_SetStateI4ReviewChangeRequiresImplDone(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	// (pending, pending, pending), unclaimed — review change to
	// approved must reject by I-4 because impl is not done.
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"pending", "pending", "pending", "running", "")

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "approved",
	})
	if env.Error == nil {
		t.Fatalf("expected PRECONDITION_NOT_MET I-4; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("error.data.kind = %q, want PRECONDITION_NOT_MET", data.Kind)
	}
	if got, _ := data.Details["invariant"].(string); got != "review_change_requires_impl_done" {
		t.Fatalf("error.data.details.invariant = %q, want review_change_requires_impl_done", got)
	}

	// DB read-back: review column unchanged.
	_, review, _, _ := readItemStateColumns(t, itemID)
	if review != "pending" {
		t.Fatalf("DB review_state = %q after rejection, want pending (unchanged)", review)
	}
}

// TestD6_SetStateI5ImplDoneToPendingRequiresReworkPath covers AC #4:
// a bare impl=pending request on an (impl=done, review=approved,
// qa=passed) item returns §7 PRECONDITION_NOT_MET with
// data.invariant=impl_done_to_pending_requires_rework_path. The
// rework path (review=needs_rework or qa=failed) is the only allowed
// impl_state transition off `done` per PRD §6.2.
func TestD6_SetStateI5ImplDoneToPendingRequiresReworkPath(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":    itemID,
		"impl_state": "pending",
	})
	if env.Error == nil {
		t.Fatalf("expected PRECONDITION_NOT_MET I-5; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("error.data.kind = %q, want PRECONDITION_NOT_MET", data.Kind)
	}
	if got, _ := data.Details["invariant"].(string); got != "impl_done_to_pending_requires_rework_path" {
		t.Fatalf("error.data.details.invariant = %q, want impl_done_to_pending_requires_rework_path", got)
	}

	// DB read-back: impl column unchanged.
	impl, _, _, _ := readItemStateColumns(t, itemID)
	if impl != "done" {
		t.Fatalf("DB impl_state = %q after rejection, want done (unchanged)", impl)
	}
}

// =============================================================================
// set_state — intent_comment atomicity & validation
// =============================================================================

// TestD6_SetStateIntentCommentWrittenAtomically asserts the optional
// intent_comment block is persisted as a workitems.comments row
// alongside a successful state mutation. Per orchestrator DECISION
// 2026-05-18 on bead unblock-tv8.21 the contract is "best-effort
// atomic" (Encore RPC boundaries prevent a single Postgres
// transaction spanning both writes); this test pins the happy path.
func TestD6_SetStateIntentCommentWrittenAtomically(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "needs_rework",
		"intent_comment": map[string]any{
			"kind":   "review",
			"status": "warning",
			"body":   "found a regression in the migration",
		},
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeSetStateOut(t, res.StructuredContent)
	if got.Item.ReviewState != "needs_rework" {
		t.Fatalf("item.review_state = %q, want needs_rework", got.Item.ReviewState)
	}

	// DB read-back: exactly one workitems.comments row landed with
	// the requested kind/status/body for this item.
	ctx := context.Background()
	var count int
	if err := db.QueryRow(ctx,
		`SELECT count(*) FROM workitems.comments
		  WHERE item_id = $1 AND kind = 'review' AND status = 'warning'
		    AND body = 'found a regression in the migration'`,
		itemID,
	).Scan(&count); err != nil {
		t.Fatalf("count comments: %v", err)
	}
	if count != 1 {
		t.Fatalf("intent_comment row count = %d, want 1", count)
	}
}

// TestD6_SetStateIntentCommentValidationFailsBeforeStateWrite
// asserts the boundary-validation contract: an invalid
// intent_comment (empty body) surfaces §7 VALIDATION with
// data.field="intent_comment.body" BEFORE the state mutation runs —
// the item's state columns remain unchanged in the DB.
//
// This is the safety property that makes the "validate at the MCP
// boundary first" pattern meaningful: without it, a malformed
// intent_comment would leave a stale state-only mutation behind on
// every retry attempt.
func TestD6_SetStateIntentCommentValidationFailsBeforeStateWrite(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "needs_rework",
		"intent_comment": map[string]any{
			"kind":   "review",
			"status": "warning",
			"body":   "", // empty body — must reject
		},
	})
	if env.Error == nil {
		t.Fatalf("expected VALIDATION on empty intent_comment.body; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "intent_comment.body" {
		t.Fatalf("error.data.details.field = %q, want intent_comment.body", got)
	}

	// DB read-back: state columns unchanged.
	_, review, qa, _ := readItemStateColumns(t, itemID)
	if review != "approved" {
		t.Fatalf("DB review_state = %q after rejection, want approved (unchanged)", review)
	}
	if qa != "passed" {
		t.Fatalf("DB qa_state = %q after rejection, want passed (unchanged)", qa)
	}
}

// =============================================================================
// get_state
// =============================================================================

// TestD6_GetStateReturnsAllStateDimensions asserts the §6.2 Tool 14
// happy path: every state column + materialised pipeline_stage +
// is_ready + claim columns surface on the wire. For an item with no
// comments, recent_kinds is the empty array.
func TestD6_GetStateReturnsAllStateDimensions(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	env := callTool(t, fx.RawKey, "get_state", map[string]any{
		"item_id": itemID,
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeGetStateOut(t, res.StructuredContent)

	if got.ImplState != "done" {
		t.Fatalf("impl_state = %q, want done", got.ImplState)
	}
	if got.ReviewState != "approved" {
		t.Fatalf("review_state = %q, want approved", got.ReviewState)
	}
	if got.QAState != "passed" {
		t.Fatalf("qa_state = %q, want passed", got.QAState)
	}
	if got.PipelineState != "running" {
		t.Fatalf("pipeline_state = %q, want running", got.PipelineState)
	}
	if got.ClaimedByID != fx.UserID {
		t.Fatalf("claimed_by_id = %q, want %q", got.ClaimedByID, fx.UserID)
	}
	if got.ClaimedAt == "" {
		t.Fatalf("claimed_at empty on claimed item")
	}
	if got.RecentKinds == nil {
		t.Fatalf("recent_kinds nil (must be [] when no comments)")
	}
	if len(got.RecentKinds) != 0 {
		t.Fatalf("recent_kinds len = %d, want 0 (no comments seeded)", len(got.RecentKinds))
	}
}

// TestD6_GetStateRecentKindsOneRowPerKind covers AC #5: get_state
// returns recent_kinds with one row per kind, picking the most
// recent comment per kind by created_at desc. We seed two kinds with
// multiple comments each; the response MUST contain exactly two rows
// — one per kind — each pointing at the latest comment_id for that
// kind.
func TestD6_GetStateRecentKindsOneRowPerKind(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"pending", "pending", "pending", "running", "")

	// Seed two kinds with two comments each, offsetting created_at
	// so the "most recent per kind" picker has a real choice to make.
	_ = seedCommentDirect(t, itemID, "investigation", "info", "first investigation", -2*time.Second)
	latestInvestigation := seedCommentDirect(t, itemID, "investigation", "info", "second investigation", 0)
	_ = seedCommentDirect(t, itemID, "decision", "info", "first decision", -2*time.Second)
	latestDecision := seedCommentDirect(t, itemID, "decision", "info", "second decision", 0)

	env := callTool(t, fx.RawKey, "get_state", map[string]any{
		"item_id": itemID,
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeGetStateOut(t, res.StructuredContent)

	if len(got.RecentKinds) != 2 {
		t.Fatalf("recent_kinds len = %d, want 2 (one row per kind); rows=%+v", len(got.RecentKinds), got.RecentKinds)
	}

	byKind := map[string]string{} // kind → comment_id
	for _, rk := range got.RecentKinds {
		byKind[rk.Kind] = rk.CommentID
	}
	if byKind["investigation"] != latestInvestigation {
		t.Fatalf("recent_kinds[investigation].comment_id = %q, want %q (most recent)",
			byKind["investigation"], latestInvestigation)
	}
	if byKind["decision"] != latestDecision {
		t.Fatalf("recent_kinds[decision].comment_id = %q, want %q (most recent)",
			byKind["decision"], latestDecision)
	}
}

// =============================================================================
// audit rows
// =============================================================================

// TestD6_AuditRowsCarryToolName: each set_state + get_state dispatch
// writes one mcp.tool_calls row with the matching tool_name. SPEC
// §8.1 — completes the audit coverage matrix alongside D-2, D-3,
// D-4, and D-5.
func TestD6_AuditRowsCarryToolName(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	_ = callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "needs_rework",
	})
	_ = callTool(t, fx.RawKey, "get_state", map[string]any{
		"item_id": itemID,
	})

	rows := selectToolCalls(t)
	have := map[string]int{}
	for _, r := range rows {
		have[r.ToolName]++
	}
	for _, want := range []string{"set_state", "get_state"} {
		if have[want] < 1 {
			t.Fatalf("audit row for tool_name=%q: count=%d, want >=1; rows=%+v", want, have[want], rows)
		}
	}
}
