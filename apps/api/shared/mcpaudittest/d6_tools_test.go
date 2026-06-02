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

	"encore.app/mcp"
	"encore.app/shared/ulid"
	"encore.app/workitems"
	"encore.dev/beta/errs"
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
// structuredContent JSON object carrying { "item": Item } plus the
// optional §7.1 success-side `warnings` array (omitted entirely when
// no warning is present). The intent_comment partial-failure path
// carries exactly one {code:intent_comment_dropped, ...} entry.
type setStateWireOut struct {
	Item struct {
		ID            string `json:"id"`
		ImplState     string `json:"impl_state"`
		ReviewState   string `json:"review_state"`
		QAState       string `json:"qa_state"`
		PipelineState string `json:"pipeline_state"`
	} `json:"item"`
	Warnings []struct {
		Code    string         `json:"code"`
		Message string         `json:"message"`
		Details map[string]any `json:"details"`
	} `json:"warnings"`
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
// pipeline_stage / claimed_by_id / claimed_at are *string because the
// wire contract (post-review DECISION 2026-05-18, S1) emits explicit
// JSON `null` when the underlying field is unset — a value-typed
// `string` with `omitempty` would silently swallow that distinction.
type getStateWireOut struct {
	ProjectID     string  `json:"project_id"`
	ImplState     string  `json:"impl_state"`
	ReviewState   string  `json:"review_state"`
	QAState       string  `json:"qa_state"`
	PipelineState string  `json:"pipeline_state"`
	PipelineStage *string `json:"pipeline_stage"`
	IsReady       bool    `json:"is_ready"`
	ClaimedByID   *string `json:"claimed_by_id"`
	ClaimedAt     *string `json:"claimed_at"`
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

// TestD6_SetStateSuccessOmitsWarningsKey pins SPEC §7.1's omitempty
// contract + the R2 schema-validation concern: a successful set_state
// with NO dropped intent_comment MUST omit the `warnings` key from
// structuredContent entirely (not emit `null` or `[]`), and the result
// MUST still pass the SDK's additionalProperties:false output-schema
// validation that the embedded WithWarnings field introduces. We
// inspect the raw structuredContent map (not the typed decode) so the
// "key absent" mode is caught — a typed slice + omitempty would
// silently mask an unexpectedly-present `null`.
func TestD6_SetStateSuccessOmitsWarningsKey(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	// Happy path WITH an intent_comment that succeeds: still no warning.
	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "needs_rework",
		"intent_comment": map[string]any{
			"kind":   "review",
			"status": "warning",
			"body":   "no warning expected on the success path",
		},
	})
	res := assertStructuredEchoesText(t, env)

	var raw map[string]json.RawMessage
	if err := json.Unmarshal(res.StructuredContent, &raw); err != nil {
		t.Fatalf("unmarshal raw structuredContent: %v; raw=%s", err, string(res.StructuredContent))
	}
	if _, ok := raw["warnings"]; ok {
		t.Fatalf("success structuredContent carried a 'warnings' key; want it omitted (omitempty); raw=%s",
			string(res.StructuredContent))
	}

	// Audit row: result_kind='ok' and warning_codes is the empty array.
	rows := selectToolCalls(t, fx.OrgID)
	var setStateRow *toolCallRow
	for i := range rows {
		if rows[i].ToolName == "set_state" {
			setStateRow = &rows[i]
		}
	}
	if setStateRow == nil {
		t.Fatalf("no set_state audit row found")
	}
	if setStateRow.ResultKind != "ok" {
		t.Fatalf("audit result_kind = %q, want ok", setStateRow.ResultKind)
	}
	if setStateRow.WarningCodes != "[]" {
		t.Fatalf("audit warning_codes = %q, want [] on the no-warning path", setStateRow.WarningCodes)
	}
}

// TestD6_SetStateIntentCommentDroppedEmitsWarning is the AC#3 test: on
// the intent_comment partial-failure path (state mutation committed,
// AppendComment then fails), set_state returns SUCCESS carrying exactly
// one §7.1 warning {code:intent_comment_dropped, message, details} on
// the wire AND the §8.1.1 mcp.tool_calls.warning_codes audit column
// records ["intent_comment_dropped"], with result_kind staying 'ok'.
//
// There is no black-box input that makes AppendComment fail after
// SetStateColumns commits (validation is caught at the MCP boundary
// first; the item provably exists post-commit) — see the
// appendIntentComment seam doc-comment + unblock-tv8.63 INVESTIGATION
// risk R1. We force the failure by overriding the production seam with
// a stub that returns an error AFTER asserting SetStateColumns already
// committed (it reads the live state row), then restore it.
func TestD6_SetStateIntentCommentDroppedEmitsWarning(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	// Override the post-commit AppendComment seam to force a failure.
	// The stub returns an Internal error WITHOUT writing any comment,
	// simulating a DB blip on the second RPC after the state mutation
	// has already committed.
	var stubCalled bool
	restore := mcp.SetAppendIntentCommentForTest(
		func(_ context.Context, req *workitems.AppendCommentRequest) error {
			stubCalled = true
			if req.ItemID != itemID {
				t.Errorf("seam received item_id %q, want %q", req.ItemID, itemID)
			}
			return &errs.Error{Code: errs.Internal, Message: "simulated AppendComment failure"}
		},
	)
	defer restore()

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "needs_rework",
		"intent_comment": map[string]any{
			"kind":   "completed",
			"status": "success",
			"body":   "this append is forced to fail post-commit",
		},
	})
	res := assertStructuredEchoesText(t, env)

	if !stubCalled {
		t.Fatalf("appendIntentComment seam was never invoked")
	}

	// The primary state mutation committed: item.review_state changed
	// and (I-1) qa_state auto-reset to pending — the call succeeded.
	got := decodeSetStateOut(t, res.StructuredContent)
	if got.Item.ReviewState != "needs_rework" {
		t.Fatalf("item.review_state = %q, want needs_rework (state committed)", got.Item.ReviewState)
	}
	// DB read-back confirms the commit landed despite the dropped comment.
	_, review, qa, _ := readItemStateColumns(t, itemID)
	if review != "needs_rework" {
		t.Fatalf("DB review_state = %q, want needs_rework", review)
	}
	if qa != "pending" {
		t.Fatalf("DB qa_state = %q, want pending (I-1 auto-reset)", qa)
	}

	// §7.1 caller-visible signal: exactly one warning entry.
	if len(got.Warnings) != 1 {
		t.Fatalf("warnings len = %d, want 1; raw=%s", len(got.Warnings), string(res.StructuredContent))
	}
	w := got.Warnings[0]
	if w.Code != "intent_comment_dropped" {
		t.Fatalf("warning.code = %q, want intent_comment_dropped", w.Code)
	}
	if w.Message == "" {
		t.Fatalf("warning.message is empty; want a human-readable summary")
	}
	if got, _ := w.Details["intent_comment_kind"].(string); got != "completed" {
		t.Fatalf("warning.details.intent_comment_kind = %q, want completed", got)
	}
	if got, _ := w.Details["intent_comment_status"].(string); got != "success" {
		t.Fatalf("warning.details.intent_comment_status = %q, want success", got)
	}
	// The comment body MUST NOT leak into details (SPEC §6.2/§7.1).
	for k, v := range w.Details {
		if s, ok := v.(string); ok && s == "this append is forced to fail post-commit" {
			t.Fatalf("warning.details[%q] echoed the comment body; want body excluded", k)
		}
	}

	// No comment row landed (the append failed).
	ctx := context.Background()
	var count int
	if err := db.QueryRow(ctx,
		`SELECT count(*) FROM workitems.comments WHERE item_id = $1`, itemID,
	).Scan(&count); err != nil {
		t.Fatalf("count comments: %v", err)
	}
	if count != 0 {
		t.Fatalf("comment row count = %d, want 0 (append failed)", count)
	}

	// §8.1.1 operator-visible signal: the audit row keeps result_kind
	// 'ok' and records warning_codes ["intent_comment_dropped"].
	rows := selectToolCalls(t, fx.OrgID)
	var setStateRow *toolCallRow
	for i := range rows {
		if rows[i].ToolName == "set_state" {
			setStateRow = &rows[i]
		}
	}
	if setStateRow == nil {
		t.Fatalf("no set_state audit row found")
	}
	if setStateRow.ResultKind != "ok" {
		t.Fatalf("audit result_kind = %q, want ok (call succeeded)", setStateRow.ResultKind)
	}
	if setStateRow.WarningCodes != `["intent_comment_dropped"]` {
		t.Fatalf("audit warning_codes = %q, want [\"intent_comment_dropped\"]", setStateRow.WarningCodes)
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
	if got.ClaimedByID == nil || *got.ClaimedByID != fx.UserID {
		t.Fatalf("claimed_by_id = %v, want %q", got.ClaimedByID, fx.UserID)
	}
	if got.ClaimedAt == nil || *got.ClaimedAt == "" {
		t.Fatalf("claimed_at = %v, want non-empty on claimed item", got.ClaimedAt)
	}
	if got.ProjectID != fx.ProjectID {
		t.Fatalf("project_id = %q, want %q", got.ProjectID, fx.ProjectID)
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

// TestD6_GetStateUnclaimedItemEmitsExplicitNulls asserts the
// post-review wire-shape contract (DECISION 2026-05-18, S1): for an
// unclaimed item, the §6.2 Tool 14 structuredContent envelope MUST
// contain the keys `claimed_by_id` and `claimed_at` with literal JSON
// `null` value — NOT omit them. pipeline_stage is also pointer-
// encoded; since the DDL default 'Investigation' means it's never
// empty in practice, we only assert the key is present at the wire
// (`null` would require a corrupted row).
//
// The check is done on the raw structuredContent JSON map so we catch
// the "key absent" failure mode that a value-typed `string` +
// `omitempty` decoding would silently mask.
func TestD6_GetStateUnclaimedItemEmitsExplicitNulls(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	// Unclaimed item: empty claimedBy → claimed_by_id and claimed_at
	// both must surface as JSON `null` on the wire envelope.
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"pending", "pending", "pending", "running", "")

	env := callTool(t, fx.RawKey, "get_state", map[string]any{
		"item_id": itemID,
	})
	res := assertStructuredEchoesText(t, env)

	// Decode into a generic map so we can inspect KEY PRESENCE
	// independently of Go zero-value semantics — a missing key is
	// indistinguishable from a JSON null after typed unmarshalling,
	// but `_, ok := m[k]` discriminates the two.
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(res.StructuredContent, &raw); err != nil {
		t.Fatalf("unmarshal raw structuredContent: %v; raw=%s", err, string(res.StructuredContent))
	}

	for _, key := range []string{"claimed_by_id", "claimed_at", "pipeline_stage"} {
		v, ok := raw[key]
		if !ok {
			t.Fatalf("structuredContent missing key %q; want present (with null for unclaimed); raw=%s",
				key, string(res.StructuredContent))
		}
		// For the two claim fields we expect literal null; for
		// pipeline_stage the DDL default means it's a non-null
		// string. Either way the key MUST be present.
		if key == "pipeline_stage" {
			continue
		}
		if string(v) != "null" {
			t.Fatalf("structuredContent[%q] = %s, want literal null on unclaimed item", key, string(v))
		}
	}
}

// =============================================================================
// audit rows
// =============================================================================

// TestD6_SetStateIntentCommentInvalidKindRejected asserts the
// post-review enum-validation contract (DECISION 2026-05-18, S2): an
// intent_comment.kind outside SPEC §6.5's allow-list surfaces §7
// VALIDATION with `details.field="intent_comment.kind"` BEFORE the
// state mutation runs — DB state columns unchanged.
func TestD6_SetStateIntentCommentInvalidKindRejected(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "needs_rework",
		"intent_comment": map[string]any{
			"kind":   "not-a-real-kind", // not in §6.5 allow-list
			"status": "warning",
			"body":   "should never land",
		},
	})
	if env.Error == nil {
		t.Fatalf("expected VALIDATION on invalid intent_comment.kind; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "intent_comment.kind" {
		t.Fatalf("error.data.details.field = %q, want intent_comment.kind", got)
	}

	// DB read-back: state columns unchanged — SetStateColumns was
	// never called because validation ran first.
	_, review, qa, _ := readItemStateColumns(t, itemID)
	if review != "approved" {
		t.Fatalf("DB review_state = %q after rejection, want approved (unchanged)", review)
	}
	if qa != "passed" {
		t.Fatalf("DB qa_state = %q after rejection, want passed (unchanged)", qa)
	}
}

// TestD6_SetStateIntentCommentInvalidStatusRejected: same contract as
// the invalid-kind test, applied to intent_comment.status.
func TestD6_SetStateIntentCommentInvalidStatusRejected(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedItemForState(t, fx.OrgID, fx.ProjectID,
		"done", "approved", "passed", "running", fx.UserID)

	env := callTool(t, fx.RawKey, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": "needs_rework",
		"intent_comment": map[string]any{
			"kind":   "review",
			"status": "not-a-real-status", // not in §6.5 allow-list
			"body":   "should never land",
		},
	})
	if env.Error == nil {
		t.Fatalf("expected VALIDATION on invalid intent_comment.status; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "intent_comment.status" {
		t.Fatalf("error.data.details.field = %q, want intent_comment.status", got)
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

// TestD6_AuditRowsCarryToolName: each set_state + get_state dispatch
// writes one mcp.tool_calls row with the matching tool_name. SPEC
// §8.1 — completes the audit coverage matrix alongside D-2, D-3,
// D-4, and D-5.
//
// Post-review extension (DECISION 2026-05-18, S3): the get_state
// audit row's project_id MUST be populated from the item's resolved
// project scope (sourced from the same row already loaded by the
// rbac.For org gate inside workitems.GetState). This pins the
// audit-dashboard filterability contract: every tool call that
// resolves an item knows its project.
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

	rows := selectToolCalls(t, fx.OrgID)
	have := map[string]int{}
	getStateProjectID := ""
	for _, r := range rows {
		have[r.ToolName]++
		if r.ToolName == "get_state" && r.ProjectID != nil {
			getStateProjectID = *r.ProjectID
		}
	}
	for _, want := range []string{"set_state", "get_state"} {
		if have[want] < 1 {
			t.Fatalf("audit row for tool_name=%q: count=%d, want >=1; rows=%+v", want, have[want], rows)
		}
	}
	// S3 contract: the get_state audit row carries the resolved
	// project_id (sourced from the item's row via the rbac.For org
	// gate). Non-empty + matches the seeded project.
	if getStateProjectID == "" {
		t.Fatalf("get_state audit row project_id is empty; want %q", fx.ProjectID)
	}
	if getStateProjectID != fx.ProjectID {
		t.Fatalf("get_state audit row project_id = %q, want %q", getStateProjectID, fx.ProjectID)
	}
}
