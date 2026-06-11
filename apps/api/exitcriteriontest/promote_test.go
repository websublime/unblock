// promote_test.go covers the §11.1.2 round-16 (bead unblock-tv8.71)
// assertions for the Backlog→Ready lifecycle:
//
//   - is_ready-on-create: a fresh create with no inline dependencies
//     returns an item whose is_ready=true and status='Backlog' (the
//     §6.6 inline create-path write, NOT subscriber materialisation),
//     so the item is immediately promote-able.
//   - promote success: promote(item) on a Backlog+is_ready item flips
//     status='Ready'; a subsequent `ready` lists it; claim then close
//     complete the create → promote → ready → claim → close lifecycle
//     the 2026-06-03 demo could not reach (round-12 DRIFT-2 closure).
//   - promote rejection — already Ready: promote on an already-Ready
//     item is rejected PRECONDITION_NOT_MET {status:Ready, required:
//     Ready}, with NO `missing` (the block is the wrong status, not an
//     unmet readiness precondition).
//   - promote rejection — still blocked: promote on a Backlog item with
//     an open incoming 'blocks' edge (is_ready=false) is rejected
//     PRECONDITION_NOT_MET {status:Backlog, required:Ready, missing:
//     is_ready} per §7.2 — the agent disambiguates "blocked" from
//     "wrong status" via data.details.missing.
//
// All §7.2 status-extension fields are asserted INSIDE data.details
// (the locked §7 base-table shape; errmap surfaces them there), per the
// orchestrator DECISION on unblock-tv8.71.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 15 (promote) +
// § 6.6 (status transition map + is_ready-on-create rule) + § 7.2
// ({status, required} extension) + § 11.1.2 (functional assertions).

package exitcriteriontest_test

import (
	"context"
	"encoding/json"
	"testing"

	encoredb "encore.app/db"
)

// createOutItem is the typed shape of a create / promote / claim
// structuredContent { "item": { … } } payload — only the fields these
// assertions read.
type createOutItem struct {
	Item struct {
		ID      string `json:"id"`
		Status  string `json:"status"`
		IsReady bool   `json:"is_ready"`
	} `json:"item"`
}

// TestExitCriterion_PromoteLifecycle walks the §11.1.2 round-16
// create → promote → ready → claim → close lifecycle plus the two
// promote rejection paths. Single Mcp-Session-Id across the sequence.
func TestExitCriterion_PromoteLifecycle(t *testing.T) {
	f := fx(t)
	ctx := t.Context()
	sessionID := initializeSession(t, f.RawKey)

	// --- 1) is_ready-on-create (§6.6 inline write) -------------------
	//
	// A fresh create with no inline dependencies must come back
	// is_ready=true AND status='Backlog'. Before round-16 nothing on
	// the create path set is_ready and a no-blocker item stranded at
	// false forever (the sole recomputeReady writer only ran from
	// add/remove edge + close), so it was never promote-able.
	createEnv := callTool(t, f.RawKey, sessionID, "create", map[string]any{
		"project_id": f.ProjectID,
		"type":       "task",
		"title":      "promote-lifecycle-target",
	})
	var created createOutItem
	if err := json.Unmarshal(expectSuccess(t, createEnv), &created); err != nil {
		t.Fatalf("unmarshal create result: %v", err)
	}
	if created.Item.ID == "" {
		t.Fatalf("create returned empty item id")
	}
	if created.Item.Status != "Backlog" {
		t.Fatalf("create: status = %q, want Backlog", created.Item.Status)
	}
	if !created.Item.IsReady {
		t.Fatalf("create: is_ready = false, want true (§6.6 is_ready-on-create inline write)")
	}
	// Belt-and-suspenders: assert the column directly, not just the
	// tool surface, so a future regression in readItem cannot mask a
	// missing inline write.
	assertDBStatusReady(t, ctx, created.Item.ID, "Backlog", true)

	target := created.Item.ID

	// --- 2) promote success: Backlog → Ready -------------------------
	promoteEnv := callTool(t, f.RawKey, sessionID, "promote", map[string]any{
		"item_id": target,
	})
	var promoted createOutItem
	if err := json.Unmarshal(expectSuccess(t, promoteEnv), &promoted); err != nil {
		t.Fatalf("unmarshal promote result: %v", err)
	}
	if promoted.Item.Status != "Ready" {
		t.Fatalf("promote: status = %q, want Ready", promoted.Item.Status)
	}
	if !promoted.Item.IsReady {
		t.Fatalf("promote: is_ready = false, want true (promote reads, never recomputes is_ready)")
	}
	assertDBStatusReady(t, ctx, target, "Ready", true)

	// --- 3) ready lists the promoted item ----------------------------
	//
	// The promoted item is now (status='Ready' AND is_ready=true AND
	// closed_at IS NULL) so it appears in the ready queue. We page a
	// generous limit and scan for our id rather than asserting position
	// (the seeded fixture also contributes ready items).
	readyEnv := callTool(t, f.RawKey, sessionID, "ready", map[string]any{
		"project_id": f.ProjectID,
		"limit":      200,
	})
	if !readyContainsID(t, expectSuccess(t, readyEnv), target) {
		t.Fatalf("ready did not list the promoted item %s", target)
	}

	// --- 4) claim → close: the lifecycle the demo could not reach ----
	claimEnv := callTool(t, f.RawKey, sessionID, "claim", map[string]any{
		"item_id": target,
	})
	var claimed struct {
		Claimed bool `json:"claimed"`
	}
	if err := json.Unmarshal(expectSuccess(t, claimEnv), &claimed); err != nil {
		t.Fatalf("unmarshal claim result: %v", err)
	}
	if !claimed.Claimed {
		t.Fatalf("claim: claimed = false, want true")
	}
	// close requires claimed_by_id IS NOT NULL (P01 AF3) — satisfied by
	// the claim above.
	closeEnv := callTool(t, f.RawKey, sessionID, "close", map[string]any{
		"item_id": target,
		"reason":  "promote-lifecycle complete",
	})
	_ = expectSuccess(t, closeEnv)
	assertDBStatus(t, ctx, target, "Done")

	// --- 5) promote rejection — already Ready ------------------------
	//
	// Promote a SECOND fresh item, then attempt to promote it again
	// once it is Ready. The second promote must reject with
	// PRECONDITION_NOT_MET {status:Ready, required:Ready} and NO
	// `missing` (wrong status, not an unmet readiness precondition).
	secondEnv := callTool(t, f.RawKey, sessionID, "create", map[string]any{
		"project_id": f.ProjectID,
		"type":       "task",
		"title":      "promote-already-ready",
	})
	var second createOutItem
	if err := json.Unmarshal(expectSuccess(t, secondEnv), &second); err != nil {
		t.Fatalf("unmarshal second create: %v", err)
	}
	_ = expectSuccess(t, callTool(t, f.RawKey, sessionID, "promote", map[string]any{
		"item_id": second.Item.ID,
	}))
	reEnv := callTool(t, f.RawKey, sessionID, "promote", map[string]any{
		"item_id": second.Item.ID,
	})
	reData := expectError(t, reEnv)
	if reData.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("promote-already-Ready: kind = %q, want PRECONDITION_NOT_MET", reData.Kind)
	}
	if got, _ := reData.Details["status"].(string); got != "Ready" {
		t.Fatalf("promote-already-Ready: details.status = %q, want Ready; details=%+v", got, reData.Details)
	}
	if got, _ := reData.Details["required"].(string); got != "Ready" {
		t.Fatalf("promote-already-Ready: details.required = %q, want Ready; details=%+v", got, reData.Details)
	}
	if _, present := reData.Details["missing"]; present {
		t.Fatalf("promote-already-Ready: details.missing present (%v), want absent (the block is wrong status, not unmet readiness)", reData.Details["missing"])
	}

	// --- 6) promote rejection — still blocked ------------------------
	//
	// Create a blocker (a fresh, non-Done item) and a dependent item
	// that lists it as an incoming 'blocks' edge. The dependent is
	// Backlog with is_ready=false (the §6.5 inline recompute in the
	// create edge loop flips it false because the blocker is not Done).
	// promote on it rejects PRECONDITION_NOT_MET {status:Backlog,
	// required:Ready, missing:is_ready}.
	blockerEnv := callTool(t, f.RawKey, sessionID, "create", map[string]any{
		"project_id": f.ProjectID,
		"type":       "task",
		"title":      "promote-blocker",
	})
	var blocker createOutItem
	if err := json.Unmarshal(expectSuccess(t, blockerEnv), &blocker); err != nil {
		t.Fatalf("unmarshal blocker create: %v", err)
	}
	blockedEnv := callTool(t, f.RawKey, sessionID, "create", map[string]any{
		"project_id": f.ProjectID,
		"type":       "task",
		"title":      "promote-blocked-dependent",
		"dependencies": []map[string]any{
			{"blocker_item_id": blocker.Item.ID, "kind": "blocks"},
		},
	})
	var blocked createOutItem
	if err := json.Unmarshal(expectSuccess(t, blockedEnv), &blocked); err != nil {
		t.Fatalf("unmarshal blocked create: %v", err)
	}
	// The dependent has an open incoming blocker → is_ready=false at
	// create (the edge loop recompute corrected the initial true).
	if blocked.Item.IsReady {
		t.Fatalf("blocked dependent: is_ready = true, want false (open incoming blocker)")
	}
	assertDBStatusReady(t, ctx, blocked.Item.ID, "Backlog", false)

	blockedPromoteEnv := callTool(t, f.RawKey, sessionID, "promote", map[string]any{
		"item_id": blocked.Item.ID,
	})
	bData := expectError(t, blockedPromoteEnv)
	if bData.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("promote-blocked: kind = %q, want PRECONDITION_NOT_MET", bData.Kind)
	}
	if got, _ := bData.Details["status"].(string); got != "Backlog" {
		t.Fatalf("promote-blocked: details.status = %q, want Backlog; details=%+v", got, bData.Details)
	}
	if got, _ := bData.Details["required"].(string); got != "Ready" {
		t.Fatalf("promote-blocked: details.required = %q, want Ready; details=%+v", got, bData.Details)
	}
	if got, _ := bData.Details["missing"].(string); got != "is_ready" {
		t.Fatalf("promote-blocked: details.missing = %q, want is_ready; details=%+v", got, bData.Details)
	}
}

// assertDBStatus reads workitems.items.status for id and fails if it
// does not equal want. Asserts against the column directly so the test
// reflects production semantics, not just the tool surface.
func assertDBStatus(t *testing.T, ctx context.Context, id, want string) {
	t.Helper()
	var got string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT status FROM workitems.items WHERE id = $1`, id,
	).Scan(&got); err != nil {
		t.Fatalf("query status for %s: %v", id, err)
	}
	if got != want {
		t.Fatalf("item %s: status = %q, want %q", id, got, want)
	}
}

// readyContainsID reports whether the `ready` structuredContent
// items[] array contains an item with the given id.
func readyContainsID(t *testing.T, raw []byte, id string) bool {
	t.Helper()
	var s struct {
		Items []struct {
			ID string `json:"id"`
		} `json:"items"`
	}
	if err := json.Unmarshal(raw, &s); err != nil {
		t.Fatalf("unmarshal ready result: %v; raw=%s", err, string(raw))
	}
	for _, it := range s.Items {
		if it.ID == id {
			return true
		}
	}
	return false
}

// assertDBStatusReady asserts (status, is_ready) on the persisted row.
func assertDBStatusReady(t *testing.T, ctx context.Context, id, wantStatus string, wantReady bool) {
	t.Helper()
	var gotStatus string
	var gotReady bool
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT status, is_ready FROM workitems.items WHERE id = $1`, id,
	).Scan(&gotStatus, &gotReady); err != nil {
		t.Fatalf("query status/is_ready for %s: %v", id, err)
	}
	if gotStatus != wantStatus {
		t.Fatalf("item %s: status = %q, want %q", id, gotStatus, wantStatus)
	}
	if gotReady != wantReady {
		t.Fatalf("item %s: is_ready = %v, want %v", id, gotReady, wantReady)
	}
}
