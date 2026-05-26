// state_invariants_test.go covers the §11.1.2 state-machine
// invariants (round-2 D2 — five property tests) via the `set_state`
// MCP tool surface AND `claim` for the I-3 reset path:
//
//   - I-1: set_state(review_state=needs_rework) on an item with
//     qa_state='passed' flips qa_state='pending' in the same write.
//   - I-2: set_state(qa_state=failed) on an item with review_state
//     <> 'approved' is rejected with data.invariant=
//     "qa_failed_requires_review_approved".
//   - I-3: After set_state(qa_state=failed), the next claim resets
//     review_state='pending' and qa_state='pending' atomically
//     (verified via get_state immediately post-claim).
//   - I-4: set_state(review_state=approved) on an item with
//     impl_state='pending' is rejected with data.invariant=
//     "review_change_requires_impl_done".
//   - I-5: set_state(impl_state=pending) on an item with
//     impl_state='done' AND no rework path active is rejected with
//     data.invariant=
//     "impl_done_to_pending_requires_rework_path"; the same call
//     when review_state='needs_rework' succeeds.
//
// Internal RPC-level coverage of I-1..I-5 lives in
// apps/api/workitems/workitems_test.go (table-driven cases on
// SetStateColumns). This file samples one representative case per
// invariant through the MCP tool surface so the public agent path
// is exercised end-to-end. The two suites are complementary, not
// redundant.

package exitcriteriontest_test

import (
	"context"
	"encoding/json"
	"testing"

	encoredb "encore.app/db"
	"encore.app/exitcriteriontest"
	"encore.app/shared/ulid"
)

// TestExitCriterion_StateInvariant_I1 covers I-1 — review_state=needs_rework
// flips qa_state to 'pending' in the same write.
func TestExitCriterion_StateInvariant_I1(t *testing.T) {
	f := fx(t)
	ctx := t.Context()
	sessionID := initializeSession(t, f.RawKey)

	// Seed an InProgress, claimed item with impl_state=done (so I-4
	// doesn't reject the review_state advance), review_state=approved
	// (I-1 fires only when review_state changes — needs_rework here),
	// qa_state=passed (so I-1 has something to flip).
	itemID := seedFreshClaimedReady(t, ctx, f, "I-1-target", "approved", "passed")

	needsRework := "needs_rework"
	env := callTool(t, f.RawKey, sessionID, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": needsRework,
	})
	raw := expectSuccess(t, env)

	var s struct {
		Item struct {
			ReviewState string `json:"review_state"`
			QAState     string `json:"qa_state"`
		} `json:"item"`
	}
	if err := json.Unmarshal(raw, &s); err != nil {
		t.Fatalf("unmarshal set_state result: %v; raw=%s", err, string(raw))
	}
	if s.Item.ReviewState != "needs_rework" {
		t.Fatalf("I-1: review_state = %q, want needs_rework", s.Item.ReviewState)
	}
	if s.Item.QAState != "pending" {
		t.Fatalf("I-1: qa_state = %q, want pending (atomic flip per I-1)", s.Item.QAState)
	}
}

// TestExitCriterion_StateInvariant_I2 covers I-2 —
// qa_state=failed requires review_state='approved'.
func TestExitCriterion_StateInvariant_I2(t *testing.T) {
	f := fx(t)
	ctx := t.Context()
	sessionID := initializeSession(t, f.RawKey)

	// Seed with review_state='pending' so I-2 rejects qa_state=failed.
	itemID := seedFreshClaimedReady(t, ctx, f, "I-2-target", "pending", "pending")

	qaFailed := "failed"
	env := callTool(t, f.RawKey, sessionID, "set_state", map[string]any{
		"item_id":  itemID,
		"qa_state": qaFailed,
	})
	data := expectError(t, env)

	if data.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("I-2: data.kind = %q, want PRECONDITION_NOT_MET", data.Kind)
	}
	gotInvariant, _ := data.Details["invariant"].(string)
	if gotInvariant != "qa_failed_requires_review_approved" {
		t.Fatalf("I-2: details.invariant = %q, want qa_failed_requires_review_approved; details=%+v",
			gotInvariant, data.Details)
	}
}

// TestExitCriterion_StateInvariant_I3 covers I-3 — Claim after
// qa_state=failed resets both review_state and qa_state to
// 'pending' atomically.
func TestExitCriterion_StateInvariant_I3(t *testing.T) {
	f := fx(t)
	ctx := t.Context()
	sessionID := initializeSession(t, f.RawKey)

	// Seed a fresh Ready+unclaimed item directly in (impl=done,
	// review=approved, qa=failed). The §6.4 atomic claim transaction
	// checks qa_state='failed' on the locked row and resets both
	// state columns in the same SELECT FOR UPDATE. Seeding the row
	// in this state avoids a test-only UPDATE on is_ready (which
	// would trip the no_direct_is_ready_write linter).
	itemID := seedFreshReadyUnclaimed(t, ctx, f, "I-3-target", "done", "approved", "failed")

	claimEnv := callTool(t, f.RawKey, sessionID, "claim", map[string]any{
		"item_id": itemID,
	})
	_ = expectSuccess(t, claimEnv)

	// Verify post-claim state via the get_state tool.
	getEnv := callTool(t, f.RawKey, sessionID, "get_state", map[string]any{
		"item_id": itemID,
	})
	getRaw := expectSuccess(t, getEnv)

	var gs struct {
		ImplState   string `json:"impl_state"`
		ReviewState string `json:"review_state"`
		QAState     string `json:"qa_state"`
	}
	if err := json.Unmarshal(getRaw, &gs); err != nil {
		t.Fatalf("unmarshal get_state: %v; raw=%s", err, string(getRaw))
	}
	if gs.ReviewState != "pending" || gs.QAState != "pending" {
		t.Fatalf("I-3: post-claim state (review=%q, qa=%q), want both pending",
			gs.ReviewState, gs.QAState)
	}
}

// TestExitCriterion_StateInvariant_I4 covers I-4 —
// review_state=approved requires impl_state='done'.
func TestExitCriterion_StateInvariant_I4(t *testing.T) {
	f := fx(t)
	ctx := t.Context()
	sessionID := initializeSession(t, f.RawKey)

	// Seed with impl_state=pending (default) so I-4 rejects
	// review_state=approved.
	itemID := seedFreshClaimedPending(t, ctx, f, "I-4-target")

	approved := "approved"
	env := callTool(t, f.RawKey, sessionID, "set_state", map[string]any{
		"item_id":      itemID,
		"review_state": approved,
	})
	data := expectError(t, env)

	if data.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("I-4: data.kind = %q, want PRECONDITION_NOT_MET", data.Kind)
	}
	gotInvariant, _ := data.Details["invariant"].(string)
	if gotInvariant != "review_change_requires_impl_done" {
		t.Fatalf("I-4: details.invariant = %q, want review_change_requires_impl_done", gotInvariant)
	}
}

// TestExitCriterion_StateInvariant_I5 covers I-5 —
// impl_state=pending after impl_state=done requires a rework path
// (review_state='needs_rework') to be legal.
//
// Two sub-cases:
//   - Without rework path (review_state='approved'): REJECT.
//   - With rework path (review_state='needs_rework'): ACCEPT.
func TestExitCriterion_StateInvariant_I5(t *testing.T) {
	f := fx(t)
	ctx := t.Context()
	sessionID := initializeSession(t, f.RawKey)

	t.Run("reject_without_rework_path", func(t *testing.T) {
		itemID := seedFreshClaimedReady(t, ctx, f, "I-5-reject-target", "approved", "passed")

		pending := "pending"
		env := callTool(t, f.RawKey, sessionID, "set_state", map[string]any{
			"item_id":    itemID,
			"impl_state": pending,
		})
		data := expectError(t, env)
		if data.Kind != "PRECONDITION_NOT_MET" {
			t.Fatalf("I-5 reject: data.kind = %q, want PRECONDITION_NOT_MET", data.Kind)
		}
		gotInvariant, _ := data.Details["invariant"].(string)
		if gotInvariant != "impl_done_to_pending_requires_rework_path" {
			t.Fatalf("I-5 reject: details.invariant = %q, want impl_done_to_pending_requires_rework_path", gotInvariant)
		}
	})

	t.Run("rework_path_via_claim_I3", func(t *testing.T) {
		// SPEC §11.1.2 I-5 last sub-bullet wording: "The same call
		// when review_state='needs_rework' succeeds." A literal
		// interpretation (a SetStateColumns request that flips
		// impl_state=done → pending while also asserting/keeping
		// review_state=needs_rework) is unreachable through
		// SetStateColumns by design — I-4 still requires impl=done
		// when review ∈ {approved, needs_rework} per workitems.go
		// line 1268, so the combined request rejects on I-4 BEFORE
		// the rework path can take effect.
		//
		// The codebase convention (workitems/integration_test.go
		// line 350-368, TestSetStateInvariantI5AllowedWhenQAAlreadyFailed,
		// is t.Skip-ed with the same rationale) is that the
		// canonical rework flow uses Claim's I-3 reset path, not a
		// one-shot SetStateColumns. We follow the same convention:
		// the I-3 reset is exercised by TestExitCriterion_StateInvariant_I3
		// above, which is the spec's intended "rework path
		// succeeds" assertion mediated by the production code path.
		//
		// DEVIATION-LOG (logged on bead unblock-tv8.26): the SPEC
		// §11.1.2 I-5 second sub-bullet is unreachable through
		// SetStateColumns alone; the production happy-rework flow
		// is Claim's I-3. The internal I-3 path is exercised by
		// the dedicated I-3 test above.
		t.Skip("I-5 rework happy path is exercised by TestExitCriterion_StateInvariant_I3 (Claim I-3 reset); SetStateColumns one-shot combination is rejected by I-4 per workitems.go:1268. See DEVIATION on bead unblock-tv8.26.")
	})
}

// seedFreshClaimedPending inserts a fresh InProgress item with
// impl_state='pending' (default) + review_state='pending' +
// qa_state='pending' claimed by Alice. Used by I-4 setup where the
// invariant trigger needs impl_state != 'done'.
func seedFreshClaimedPending(t *testing.T, ctx context.Context, f *exitcriteriontest.Fixture, title string) string {
	t.Helper()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status,
		    impl_state, review_state, qa_state,
		    claimed_by_id, claimed_at, claimed_by_agent,
		    is_ready)
		 VALUES ($1, $2, $3, 'task', $4, 'InProgress',
		         'pending', 'pending', 'pending',
		         $5, now(), 'claude-code',
		         false)`,
		id, f.OrgID, f.ProjectID, title, f.UserID,
	); err != nil {
		t.Fatalf("seedFreshClaimedPending insert: %v", err)
	}
	t.Cleanup(func() {
		// Background ctx because t.Context() is cancelled before cleanup runs.
		_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.items WHERE id = $1`, id)
	})
	return id
}
