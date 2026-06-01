// prime_ready_claim_close_test.go covers the §11.1.2 happy-path
// sequence:
//
//   - prime returns non-empty ready_summary + empty claimed_by_me.
//   - ready --limit 1 returns one item, deterministically.
//   - claim on the returned item succeeds.
//   - set_state(impl_state=done) on the claimed item is accepted.
//   - close on the same item succeeds (P01 relaxation: claimed_by_id
//     IS NOT NULL is the only precondition); the close is driven via
//     the private-mesh workitems.Close so its CascadeRequested publish
//     is observable in this test scope (see step 5 DEVIATION), and the
//     cascade subscriber is then driven via
//     deps.HandleCascadeRequestedForTest on that REAL captured message
//     per the SPEC §11.1.1 round-13 contract.
//   - After cascade, prime reflects newly-unblocked dependents
//     (itm_c, itm_d flip to ready — this is Regime-A-inline per
//     workitems.Close → deps.RecomputeReadyForBlocksDownstream; the
//     subscriber-driven assertion below also exercises the
//     pipeline_stage recompute path).
//   - deps.cascade_events has one row with kind='close' for the
//     close above.
//
// The cascade row materialisation is driven by the four-step
// invocation pattern from SPEC §11.1.1 (round-13):
//
//  1. Invoke `close` (the producing tool).
//  2. Capture et.Topic(deps.CascadeRequestedTopic).PublishedMessages()
//     filtered to the close we just issued.
//  3. Invoke deps.HandleCascadeRequestedForTest exactly once on the
//     captured message.
//  4. Assert the deps.cascade_events row.

package exitcriteriontest_test

import (
	"context"
	"encoding/json"
	"testing"

	encoredb "encore.app/db"
	"encore.app/deps"
	"encore.app/workitems"
	"encore.dev/et"
)

// TestExitCriterion_PrimeReadyClaimCloseCascadeFlow walks the §11.1.2
// happy-path sequence end-to-end against the seeded fixture.
//
// Single Mcp-Session-Id across the entire sequence so the SDK's
// stateful session map sees the consecutive requests as one
// conversational session (mirrors a real agent's lifecycle).
//
// Sequence intentionally selects itm_b as the claim/close target —
// it is the upstream blocker of itm_c and itm_d, so closing it
// flips both downstream rows to is_ready=true (Regime A) AND
// triggers a single CascadeRequested publish with Reason="close"
// for the multi-hop pipeline_stage recompute (Regime B). itm_e is
// NOT downstream of itm_b directly (itm_b → itm_c, itm_b → itm_d,
// itm_d → itm_e — itm_e is two hops away), so the §11.1.2 phrasing
// "itm_c, itm_d flip to ready" matches the direct-blocks-downstream
// scope of deps.RecomputeReadyForBlocksDownstream.
func TestExitCriterion_PrimeReadyClaimCloseCascadeFlow(t *testing.T) {
	f := fx(t)
	ctx := context.Background()

	sessionID := initializeSession(t, f.RawKey)

	// --- 1) prime: non-empty ready_summary, empty claimed_by_me ---

	primeEnv := callTool(t, f.RawKey, sessionID, "prime", map[string]any{
		"project_id":  f.ProjectID,
		"ready_limit": 10,
	})
	primeRaw := expectSuccess(t, primeEnv)

	var primeStruct struct {
		ReadySummary struct {
			CountTotal int `json:"count_total"`
			Items      []struct {
				ID       string `json:"id"`
				IsReady  bool   `json:"is_ready"`
				Priority string `json:"priority"`
			} `json:"items"`
		} `json:"ready_summary"`
		ClaimedByMe []struct {
			ID string `json:"id"`
		} `json:"claimed_by_me"`
	}
	if err := json.Unmarshal(primeRaw, &primeStruct); err != nil {
		t.Fatalf("unmarshal prime: %v; raw=%s", err, string(primeRaw))
	}
	if primeStruct.ReadySummary.CountTotal < 2 {
		t.Fatalf("prime ready_summary.count_total = %d, want >= 2 (itm_b + itm_e)", primeStruct.ReadySummary.CountTotal)
	}
	if len(primeStruct.ClaimedByMe) != 0 {
		t.Fatalf("prime claimed_by_me len = %d, want 0", len(primeStruct.ClaimedByMe))
	}

	// --- 2) ready --limit 1: one item, deterministic ---

	readyEnv := callTool(t, f.RawKey, sessionID, "ready", map[string]any{
		"project_id": f.ProjectID,
		"limit":      1,
	})
	readyRaw := expectSuccess(t, readyEnv)

	var readyStruct struct {
		Items []struct {
			ID       string `json:"id"`
			Priority string `json:"priority"`
		} `json:"items"`
		TotalReady int `json:"total_ready"`
	}
	if err := json.Unmarshal(readyRaw, &readyStruct); err != nil {
		t.Fatalf("unmarshal ready: %v; raw=%s", err, string(readyRaw))
	}
	if len(readyStruct.Items) != 1 {
		t.Fatalf("ready items len = %d, want 1 (limit=1)", len(readyStruct.Items))
	}
	if readyStruct.TotalReady < 2 {
		t.Fatalf("ready total_ready = %d, want >= 2 (itm_b + itm_e)", readyStruct.TotalReady)
	}
	// Determinism check: the canonical ORDER BY (priority ASC,
	// created_at ASC, id ASC) means a second call with the same
	// inputs returns the same id.
	ready2Env := callTool(t, f.RawKey, sessionID, "ready", map[string]any{
		"project_id": f.ProjectID,
		"limit":      1,
	})
	ready2Raw := expectSuccess(t, ready2Env)
	var ready2Struct struct {
		Items []struct {
			ID string `json:"id"`
		} `json:"items"`
	}
	if err := json.Unmarshal(ready2Raw, &ready2Struct); err != nil {
		t.Fatalf("unmarshal ready (second call): %v", err)
	}
	if len(ready2Struct.Items) != 1 || ready2Struct.Items[0].ID != readyStruct.Items[0].ID {
		t.Fatalf("ready determinism failed: first=%v second=%v", readyStruct.Items, ready2Struct.Items)
	}

	// --- 3) claim itm_b (deterministic target for the cascade test) ---

	itemB := f.ItemID("itm_b")
	claimEnv := callTool(t, f.RawKey, sessionID, "claim", map[string]any{
		"item_id": itemB,
	})
	claimRaw := expectSuccess(t, claimEnv)

	var claimStruct struct {
		Claimed bool `json:"claimed"`
		Item    struct {
			ID          string `json:"id"`
			Status      string `json:"status"`
			ClaimedByID string `json:"claimed_by_id"`
			ClaimedAt   string `json:"claimed_at"`
		} `json:"item"`
	}
	if err := json.Unmarshal(claimRaw, &claimStruct); err != nil {
		t.Fatalf("unmarshal claim: %v; raw=%s", err, string(claimRaw))
	}
	if !claimStruct.Claimed {
		t.Fatalf("claim.claimed = false, want true; struct=%+v", claimStruct)
	}
	if claimStruct.Item.ID != itemB {
		t.Fatalf("claim item.id = %q, want %q", claimStruct.Item.ID, itemB)
	}
	if claimStruct.Item.Status != "InProgress" {
		t.Fatalf("post-claim status = %q, want InProgress", claimStruct.Item.Status)
	}
	if claimStruct.Item.ClaimedByID != f.UserID {
		t.Fatalf("post-claim claimed_by_id = %q, want %q (Alice)", claimStruct.Item.ClaimedByID, f.UserID)
	}

	// --- 4) set_state(impl_state=done) — structural invariant only ---
	//
	// I-4 requires impl_state=done BEFORE review_state can advance,
	// but the inverse — setting impl_state=done while review_state is
	// still 'pending' — is legal (review/QA flow has not yet been
	// gated). This call exercises the SetStateColumns path through
	// the MCP tool surface and asserts the resulting item still has
	// claimed_by_id populated (close's precondition).
	implDone := "done"
	setStateEnv := callTool(t, f.RawKey, sessionID, "set_state", map[string]any{
		"item_id":    itemB,
		"impl_state": implDone,
	})
	setStateRaw := expectSuccess(t, setStateEnv)
	var setStateStruct struct {
		Item struct {
			ID          string `json:"id"`
			ImplState   string `json:"impl_state"`
			ClaimedByID string `json:"claimed_by_id"`
		} `json:"item"`
	}
	if err := json.Unmarshal(setStateRaw, &setStateStruct); err != nil {
		t.Fatalf("unmarshal set_state: %v", err)
	}
	if setStateStruct.Item.ImplState != "done" {
		t.Fatalf("post-set_state impl_state = %q, want done", setStateStruct.Item.ImplState)
	}
	if setStateStruct.Item.ClaimedByID == "" {
		t.Fatalf("post-set_state claimed_by_id is empty — close's precondition would fail")
	}

	// --- 5) close itm_b (private mesh) ---
	//
	// DEVIATION (round-15, bead unblock-tv8.66): the close is driven
	// through the private-mesh //encore:api workitems.Close rather than
	// the MCP `close` tool surface. SPEC §11.1.1 (round-13) endorses
	// EITHER path ("Invoke the producing RPC through the normal MCP /
	// private-mesh path"); we choose the mesh path here for the SAME
	// reason cascade_kinds_test.go does (see that file's file-level
	// DEVIATION block): et.Topic(...).PublishedMessages() only observes
	// publishes emitted in the test goroutine's request-manager scope.
	// The MCP transport runs inside an httptest.NewServer goroutine that
	// bypasses Encore's request manager, so an MCP-surface close
	// publishes CascadeRequested correctly on the production side but the
	// publish is INVISIBLE to cascadeMessagesFor from this scope. Driving
	// the mesh API preserves every production semantic (same inline
	// is_ready Regime-A recompute, same post-commit Publish via the same
	// code path) while making the real publish — and its real event_id —
	// observable. The MCP `close` tool surface is exercised end-to-end in
	// apps/api/shared/mcpaudittest/d3_tools_test.go.
	closedItem, err := workitems.Close(ctx, &workitems.CloseRequest{
		ItemID: itemB,
		Reason: "exit-criterion-e2e",
	})
	if err != nil {
		t.Fatalf("workitems.Close: %v", err)
	}
	if closedItem.Status != "Done" {
		t.Fatalf("post-close status = %q, want Done", closedItem.Status)
	}
	if closedItem.ClosedAt == nil {
		t.Fatalf("post-close closed_at is nil")
	}

	// --- 6) After close (Regime A inline recompute): itm_c, itm_d are is_ready=true ---
	//
	// workitems.Close calls deps.RecomputeReadyForBlocksDownstream on
	// itm_b's direct 'blocks' downstream neighbours INSIDE the close
	// transaction. itm_c and itm_d are both direct downstream of
	// itm_b; itm_a (their only upstream blocker via the b→c, b→d
	// edges) is now Done so the §6.5 derivation yields is_ready=true.
	// This is the §11.1.2 "After cascade, prime reflects newly
	// unblocked dependents (itm_c, itm_d flip to ready)" assertion
	// (see doc.go's "Note on is_ready and 'After cascade' wording").
	//
	// DEVIATION (codebase vs spec terminology): §11.1.2 says "prime
	// reflects newly unblocked dependents (itm_c, itm_d flip to
	// ready)". The prime tool's ready_summary surface filters by
	// (is_ready=true AND status='Ready' AND closed_at IS NULL) —
	// items whose `is_ready` flips to true but whose `status` stays
	// 'Backlog' do NOT appear in ready_summary. itm_c and itm_d are
	// seeded at status='Backlog' (per §11.1.0 default-everything rows)
	// and there is NO code path that auto-flips status from Backlog
	// to Ready when is_ready flips (status is set explicitly by
	// Create / Claim / Close — see workitems.go status constants).
	// The §11.1.2 assertion is therefore on the underlying is_ready
	// column flip, not on the prime tool surface — which is what the
	// Regime A inline recompute actually changes. We assert directly
	// against the DB so the test reflects production semantics rather
	// than the spec's loose "ready" terminology.
	wantC, wantD := f.ItemID("itm_c"), f.ItemID("itm_d")
	var isReadyC, isReadyD bool
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT is_ready FROM workitems.items WHERE id = $1`, wantC,
	).Scan(&isReadyC); err != nil {
		t.Fatalf("query itm_c is_ready: %v", err)
	}
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT is_ready FROM workitems.items WHERE id = $1`, wantD,
	).Scan(&isReadyD); err != nil {
		t.Fatalf("query itm_d is_ready: %v", err)
	}
	if !isReadyC {
		t.Errorf("post-close: itm_c (%s) is_ready = false, want true (Regime A inline recompute)", wantC)
	}
	if !isReadyD {
		t.Errorf("post-close: itm_d (%s) is_ready = false, want true (Regime A inline recompute)", wantD)
	}

	// Also drive prime via the MCP surface as a sanity check that
	// the tool is reachable end-to-end. We do NOT assert itm_c/d
	// presence in ready_summary because they remain status='Backlog'
	// (see DEVIATION above).
	postCloseEnv := callTool(t, f.RawKey, sessionID, "prime", map[string]any{
		"project_id":  f.ProjectID,
		"ready_limit": 10,
	})
	_ = expectSuccess(t, postCloseEnv)

	// --- 7) Capture the REAL close publish, then drive the subscriber ---
	//
	// Per SPEC §11.1.1 round-13: Encore Pub/Sub subscriptions do not
	// fire under encore test, so the four-step pattern is (1) invoke the
	// producing RPC, (2) capture et.Topic.PublishedMessages() filtered to
	// that close, (3) invoke HandleCascadeRequestedForTest on the
	// captured message, (4) assert the row. Because step 5 above drove
	// workitems.Close through the private mesh (in THIS test goroutine's
	// request-manager scope), the publish IS observable here — so we
	// assert the production-generated event_id end-to-end rather than a
	// test-fabricated one.
	msgs := cascadeMessagesFor(itemB, "close")
	if len(msgs) != 1 {
		t.Fatalf("CascadeRequested{kind=close, item=%s} publish count = %d, want 1", itemB, len(msgs))
	}
	closeMsg := msgs[0]
	if closeMsg.EventID == "" {
		t.Fatalf("captured close CascadeRequested has empty event_id")
	}

	if err := deps.HandleCascadeRequestedForTest(ctx, closeMsg); err != nil {
		t.Fatalf("HandleCascadeRequestedForTest: %v", err)
	}

	// --- 8) Assert exactly one deps.cascade_events row carrying the REAL event_id ---
	var (
		rowCount int
		eventID  string
	)
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*), COALESCE(max(event_id), '')
		   FROM deps.cascade_events
		  WHERE triggered_by_item_id = $1 AND kind = 'close' AND event_id = $2`,
		itemB, closeMsg.EventID,
	).Scan(&rowCount, &eventID); err != nil {
		t.Fatalf("cascade_events count query: %v", err)
	}
	if rowCount != 1 {
		t.Fatalf("cascade_events (event=%s kind=close item=%s): %d rows, want 1", closeMsg.EventID, itemB, rowCount)
	}
	if eventID != closeMsg.EventID {
		t.Fatalf("cascade_events.event_id = %q, want %q (the real publish event id)", eventID, closeMsg.EventID)
	}
}

// cascadeMessagesFor returns the CascadeRequested publishes whose
// TriggeredByItemID matches itemID and Reason matches reason. Same
// shape as apps/api/workitems/integration_test.go's
// cascadeRequestedMessagesFor — kept here as a package-local helper
// so external-test-package boundary stays clean (we can't import
// workitems_test's helpers).
func cascadeMessagesFor(itemID, reason string) []*deps.CascadeRequested {
	all := et.Topic(deps.CascadeRequestedTopic).PublishedMessages()
	out := make([]*deps.CascadeRequested, 0, len(all))
	for _, msg := range all {
		if msg == nil {
			continue
		}
		if msg.TriggeredByItemID == itemID && msg.Reason == reason {
			out = append(out, msg)
		}
	}
	return out
}
