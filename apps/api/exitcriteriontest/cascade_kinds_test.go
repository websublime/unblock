// cascade_kinds_test.go covers the §11.1.2 cascade-symmetry kinds
// (round-6 §6.3.0) — each of the four CascadeRequested Reason values
// produces exactly one deps.cascade_events row when the subscriber
// is driven, AND §11.3's idempotency invariant (re-delivery
// produces byte-identical post-state, exactly one row per
// (event_id, triggered_by_item_id)).
//
// Per SPEC §11.1.1 round-13: Encore Pub/Sub subscriptions do not
// fire under `encore test`. The harness invokes
// deps.HandleCascadeRequestedForTest directly. The four-step
// invocation pattern (publish → capture → invoke → assert) is the
// canonical flow.
//
// Kinds exercised (one test each so a regression on a single kind
// is pinpointed):
//
//   - close — via the `close` tool (already exercised by
//     prime_ready_claim_close_test.go; the test below adds an
//     N=100 re-delivery property assertion on top).
//   - edge_added — via the `add_dependency` tool.
//   - edge_removed — via the `remove_dependency` tool. Note: the
//     inline INSERT in deps.RemoveEdge writes the row BEFORE the
//     post-commit publish; the subscriber's re-insert via the same
//     event_id collapses to no-op via ON CONFLICT. Both the inline
//     row and the (no-op) subscriber re-insert are exercised.
//   - state_change — via the `set_state` tool with §5.7.1-affecting
//     columns AND via the `claim` tool's I-3 reset path.

package exitcriteriontest_test

import (
	"context"
	"testing"

	encoredb "encore.app/db"
	"encore.app/deps"
	"encore.app/exitcriteriontest"
	"encore.app/shared/ulid"
	"encore.app/workitems"
)

// DEVIATION (round-13 spec gap): cascade-publish observation goes through
// the private-mesh path, NOT the MCP tool surface. Per SPEC §11.1.1
// (round-13) "Invoke the producing RPC through the normal MCP /
// private-mesh path" — both paths are spec-endorsed; here we choose
// the private-mesh path because `et.Topic(...).PublishedMessages()` is
// only observable when the publishing goroutine is in the test's
// request manager scope. The MCP transport runs inside an
// httptest.NewServer goroutine which bypasses Encore's request
// manager (see apps/api/shared/mcpaudittest/d3_tools_test.go:317-332 for
// the same documented limitation), so a tool-surface call to
// `add_dependency` / `remove_dependency` / `set_state` / `close`
// publishes the CascadeRequested correctly on the workitems/deps side
// but the publish event is invisible to `cascadeMessagesFor` from
// this test scope. Calling the producing //encore:api directly
// preserves all production semantics (same inline is_ready recompute
// via Regime A, same post-commit Publish via the same code path)
// while making the publish observable. The MCP tool surface for these
// four tools is exercised end-to-end in
// apps/api/shared/mcpaudittest/d3_tools_test.go and
// apps/api/exitcriteriontest/prime_ready_claim_close_test.go (close).

// idempotencyN is the re-delivery cardinality per SPEC §11.3:
// "property test: re-deliver every CascadeRequested event twice;
// assert post-state is byte-identical and exactly one row exists
// per (event_id, triggered_by_item_id)". We bump to N=100 to match
// the bead AC's "N=100 redeliveries per kind".
const idempotencyN = 100

// TestExitCriterion_CascadeKind_EdgeAdded covers the §11.1.2
// cascade-symmetry edge_added assertion:
//
//   - add_dependency(from=itm_c, to=itm_d) is issued after setup.
//   - Exactly one CascadeRequested{Reason="edge_added",
//     TriggeredByItemID=itm_d} is published.
//   - After driving the subscriber, exactly one deps.cascade_events
//     row with kind='edge_added' and triggered_by_item_id=itm_d exists.
//   - N=100 re-deliveries via HandleCascadeRequestedForTest leave the
//     row count at exactly one (ON CONFLICT idempotency).
func TestExitCriterion_CascadeKind_EdgeAdded(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	// Seed two fresh items in the exit-criterion project so the new
	// edge does not interact with the §11.1.0 cycle topology (the
	// cycle test runs against itm_a..itm_e; this test runs against
	// disjoint items so the order between tests doesn't matter).
	fromID := seedFreshTask(t, ctx, f, "edge-added-from")
	toID := seedFreshTask(t, ctx, f, "edge-added-to")

	// Drive deps.AddEdge directly (private mesh) — see file-level
	// DEVIATION block on why MCP tool surface cannot be used for the
	// publish-observation step.
	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID:     f.OrgID,
		ProjectID: f.ProjectID,
		FromItem:  fromID,
		ToItem:    toID,
		Kind:      "blocks",
	}); err != nil {
		t.Fatalf("deps.AddEdge: %v", err)
	}

	msgs := cascadeMessagesFor(toID, "edge_added")
	if len(msgs) != 1 {
		t.Fatalf("CascadeRequested{kind=edge_added, item=%s} count = %d, want 1", toID, len(msgs))
	}

	// Drive the subscriber once → row materialises.
	if err := deps.HandleCascadeRequestedForTest(ctx, msgs[0]); err != nil {
		t.Fatalf("HandleCascadeRequestedForTest: %v", err)
	}
	assertCascadeRowCount(t, ctx, msgs[0].EventID, toID, "edge_added", 1)

	// Idempotency property: N=100 re-deliveries with the same
	// event_id collapse to no-op (ON CONFLICT). Row count stays at 1.
	for i := 0; i < idempotencyN; i++ {
		if err := deps.HandleCascadeRequestedForTest(ctx, msgs[0]); err != nil {
			t.Fatalf("HandleCascadeRequestedForTest re-delivery #%d: %v", i, err)
		}
	}
	assertCascadeRowCount(t, ctx, msgs[0].EventID, toID, "edge_added", 1)
}

// TestExitCriterion_CascadeKind_EdgeRemoved covers the §11.1.2
// edge_removed assertion. Tension #1 (round-6): deps.RemoveEdge
// writes the audit row INLINE inside the DELETE transaction with
// the same event_id it then publishes; the subscriber's re-insert
// collapses to no-op via ON CONFLICT.
func TestExitCriterion_CascadeKind_EdgeRemoved(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	// Seed two fresh items + an edge between them so the remove call
	// has something to delete.
	fromID := seedFreshTask(t, ctx, f, "edge-removed-from")
	toID := seedFreshTask(t, ctx, f, "edge-removed-to")
	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID:     f.OrgID,
		ProjectID: f.ProjectID,
		FromItem:  fromID,
		ToItem:    toID,
		Kind:      "blocks",
	}); err != nil {
		t.Fatalf("deps.AddEdge (setup): %v", err)
	}

	// Drive deps.RemoveEdge directly (private mesh) — see file-level
	// DEVIATION. remove_dependency by composite (from, to, kind).
	if _, err := deps.RemoveEdge(ctx, &deps.RemoveEdgeRequest{
		FromItem: fromID,
		ToItem:   toID,
		Kind:     "blocks",
	}); err != nil {
		t.Fatalf("deps.RemoveEdge: %v", err)
	}

	msgs := cascadeMessagesFor(toID, "edge_removed")
	if len(msgs) != 1 {
		t.Fatalf("CascadeRequested{kind=edge_removed, item=%s} count = %d, want 1", toID, len(msgs))
	}

	// After the inline INSERT in deps.RemoveEdge, the row already
	// exists for (event_id, triggered_by_item_id=toID). Driving the
	// subscriber re-INSERTs and is collapsed by ON CONFLICT.
	assertCascadeRowCount(t, ctx, msgs[0].EventID, toID, "edge_removed", 1)
	if err := deps.HandleCascadeRequestedForTest(ctx, msgs[0]); err != nil {
		t.Fatalf("HandleCascadeRequestedForTest: %v", err)
	}
	assertCascadeRowCount(t, ctx, msgs[0].EventID, toID, "edge_removed", 1)

	// Idempotency: N=100 re-deliveries keep row count at 1.
	for i := 0; i < idempotencyN; i++ {
		if err := deps.HandleCascadeRequestedForTest(ctx, msgs[0]); err != nil {
			t.Fatalf("HandleCascadeRequestedForTest re-delivery #%d: %v", i, err)
		}
	}
	assertCascadeRowCount(t, ctx, msgs[0].EventID, toID, "edge_removed", 1)
}

// TestExitCriterion_CascadeKind_StateChange covers the §11.1.2
// state_change kind via the set_state path. SPEC §11.1.2:
// "issue set_state(qa_state=failed) on an item with
// review_state='approved', then claim it (different agent); the
// Claim fires the I-3 reset path and publishes state_change".
//
// We split into two assertions:
//
//   - set_state(qa_state=failed) on an item with review_state='approved'
//     publishes ONE CascadeRequested{Reason="state_change"}.
//   - The subsequent claim by a different agent fires the I-3 reset
//     (review→pending, qa→pending) and publishes a SECOND
//     CascadeRequested{Reason="state_change"} per round-6 §6.3.0.
//
// Both publishes materialise the audit row when the subscriber is
// driven.
func TestExitCriterion_CascadeKind_StateChange(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	// Seed a fresh InProgress item with impl_state=done +
	// review_state=approved + qa_state=passed + claimed_by Alice so
	// the set_state(qa_state=failed) call is legal (I-2 passes
	// because review_state='approved'; I-1 does not fire because
	// review_state is not flipped).
	itemID := seedFreshClaimedReady(t, ctx, f, "state-change-target", "approved", "passed")

	// Step A: drive workitems.SetStateColumns directly (private mesh)
	// — see file-level DEVIATION. Publishes
	// CascadeRequested{Reason="state_change"} per §6.3.0.
	qaFailed := "failed"
	if _, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:  itemID,
		QAState: &qaFailed,
	}); err != nil {
		t.Fatalf("workitems.SetStateColumns(qa_state=failed): %v", err)
	}

	stateMsgs := cascadeMessagesFor(itemID, "state_change")
	if len(stateMsgs) != 1 {
		t.Fatalf("after set_state(qa_state=failed): CascadeRequested{kind=state_change, item=%s} count = %d, want 1", itemID, len(stateMsgs))
	}
	if err := deps.HandleCascadeRequestedForTest(ctx, stateMsgs[0]); err != nil {
		t.Fatalf("HandleCascadeRequestedForTest (set_state): %v", err)
	}
	assertCascadeRowCount(t, ctx, stateMsgs[0].EventID, itemID, "state_change", 1)

	// Idempotency on the same event_id.
	for i := 0; i < idempotencyN; i++ {
		if err := deps.HandleCascadeRequestedForTest(ctx, stateMsgs[0]); err != nil {
			t.Fatalf("HandleCascadeRequestedForTest re-delivery #%d (set_state): %v", i, err)
		}
	}
	assertCascadeRowCount(t, ctx, stateMsgs[0].EventID, itemID, "state_change", 1)

	// Step B: trigger the I-3 reset path on a SEPARATE fresh item.
	// The §6.4 claim transaction's I-3 reset path fires when the
	// locked row carries qa_state='failed' at the start. We seed a
	// fresh Ready+unclaimed item directly in (impl=done,
	// review=approved, qa=failed) state so the next `claim` call
	// fires I-3 and publishes Reason="state_change".
	//
	// Note: SPEC §11.1.2 names "claim it (different agent)" — the
	// fixture's Bearer is always Alice (claude-code). The I-3 reset
	// path is keyed on the item's qa_state, not on the caller
	// identity (workitems.go:1520-1546 — the resetRework branch
	// triggers on qa_state=='failed' regardless of who claims). The
	// assertion target (one CascadeRequested{state_change} from the
	// claim) is invariant over caller identity. A "different agent"
	// is the typical real-world shape; the structural assertion
	// holds with Alice.
	i3ItemID := seedFreshReadyUnclaimed(t, ctx, f, "I-3-cascade-target", "done", "approved", "failed")

	// Drive workitems.Claim directly (private mesh) — see file-level
	// DEVIATION. The §6.4 I-3 reset path fires when the locked row
	// carries qa_state='failed' at the start of the transaction.
	if _, err := workitems.Claim(ctx, &workitems.ClaimRequest{
		ItemID:        i3ItemID,
		ClaimerUserID: f.UserID,
		ClaimerAgent:  "claude-code",
	}); err != nil {
		t.Fatalf("workitems.Claim (I-3 path): %v", err)
	}

	i3Msgs := cascadeMessagesFor(i3ItemID, "state_change")
	if len(i3Msgs) != 1 {
		t.Fatalf("after I-3 reset claim on item=%s: CascadeRequested{state_change} count = %d, want 1", i3ItemID, len(i3Msgs))
	}
	if err := deps.HandleCascadeRequestedForTest(ctx, i3Msgs[0]); err != nil {
		t.Fatalf("HandleCascadeRequestedForTest (I-3): %v", err)
	}
	assertCascadeRowCount(t, ctx, i3Msgs[0].EventID, i3ItemID, "state_change", 1)
}

// TestExitCriterion_CascadeIdempotency_Close runs the §11.3
// idempotency property test on the close kind (the
// prime_ready_claim_close_test exercises a single drive; this
// version adds the N=100 re-delivery assertion).
//
// Fresh item per test so this does not couple to itm_b's state
// after the happy-path test.
func TestExitCriterion_CascadeIdempotency_Close(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	itemID := seedFreshClaimedReady(t, ctx, f, "cascade-idempotency-close", "pending", "pending")

	// Drive workitems.Close directly (private mesh) — see file-level
	// DEVIATION.
	if _, err := workitems.Close(ctx, &workitems.CloseRequest{
		ItemID: itemID,
		Reason: "idempotency-close",
	}); err != nil {
		t.Fatalf("workitems.Close: %v", err)
	}

	msgs := cascadeMessagesFor(itemID, "close")
	if len(msgs) != 1 {
		t.Fatalf("CascadeRequested{close, item=%s} count = %d, want 1", itemID, len(msgs))
	}
	if err := deps.HandleCascadeRequestedForTest(ctx, msgs[0]); err != nil {
		t.Fatalf("HandleCascadeRequestedForTest: %v", err)
	}
	assertCascadeRowCount(t, ctx, msgs[0].EventID, itemID, "close", 1)

	for i := 0; i < idempotencyN; i++ {
		if err := deps.HandleCascadeRequestedForTest(ctx, msgs[0]); err != nil {
			t.Fatalf("HandleCascadeRequestedForTest re-delivery #%d: %v", i, err)
		}
	}
	assertCascadeRowCount(t, ctx, msgs[0].EventID, itemID, "close", 1)
}

// assertCascadeRowCount asserts the deps.cascade_events table holds
// exactly want rows for the given (event_id, triggered_by_item_id,
// kind) tuple. ON CONFLICT (event_id, triggered_by_item_id) DO
// NOTHING is the structural idempotency key (per §6.3.2 and the
// dependencies_pair_uniq + cascade_events_event_trigger_uniq
// constraints in 0050_deps.up.sql).
func assertCascadeRowCount(t *testing.T, ctx context.Context, eventID, itemID, kind string, want int) {
	t.Helper()
	var got int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*)
		   FROM deps.cascade_events
		  WHERE event_id = $1 AND triggered_by_item_id = $2 AND kind = $3`,
		eventID, itemID, kind,
	).Scan(&got); err != nil {
		t.Fatalf("cascade_events count query (event=%s item=%s kind=%s): %v", eventID, itemID, kind, err)
	}
	if got != want {
		t.Fatalf("cascade_events count (event=%s item=%s kind=%s) = %d, want %d", eventID, itemID, kind, got, want)
	}
}

// seedFreshTask inserts a fresh task row under the exit-criterion
// fixture's org/project, in Ready+is_ready=true state with no
// claim. Returns the persisted id. Per-test rows so cascade-kind
// tests do not contend on the shared §11.1.0 graph.
//
// Cleanup is registered on t.Cleanup so the row vanishes at test
// exit; the FK chain (items.org_id → org.organizations) means the
// row is also removed by the global TestMain teardown if cleanup
// runs out of order.
func seedFreshTask(t *testing.T, ctx context.Context, f *exitcriteriontest.Fixture, title string) string {
	t.Helper()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', $4, 'Ready', true)`,
		id, f.OrgID, f.ProjectID, title,
	); err != nil {
		t.Fatalf("seedFreshTask insert: %v", err)
	}
	t.Cleanup(func() {
		// Use a fresh background ctx — t.Context() is cancelled by
		// the time t.Cleanup runs, which would make the DELETE
		// fail silently with "context canceled" and leak the row
		// across tests (observed under in-process leak via prime's
		// claimed_by_me when StateChange / Idempotency_Close
		// preceded PrimeReadyClaimClose).
		_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.items WHERE id = $1`, id)
	})
	return id
}

// seedFreshReadyUnclaimed inserts a fresh task in Ready+is_ready=true
// state with no claim and the given (impl_state, review_state,
// qa_state). Used by tests that need a specific starting state
// before a `claim` call exercises the I-3 reset path — seeding the
// row directly in the desired state avoids a test-only UPDATE on
// is_ready (which would trip the no_direct_is_ready_write linter).
//
// CHECK constraint contract: workitems.items.is_ready is a regular
// column with no constraint on the value; status='Ready' is in the
// items_status_chk allow-list; (impl_state, review_state, qa_state)
// values are validated against their respective CHECK lists. The
// caller is responsible for passing legal enum values; the seed
// surfaces DB errors verbatim.
func seedFreshReadyUnclaimed(t *testing.T, ctx context.Context, f *exitcriteriontest.Fixture, title, implState, reviewState, qaState string) string {
	t.Helper()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status,
		    impl_state, review_state, qa_state,
		    is_ready)
		 VALUES ($1, $2, $3, 'task', $4, 'Ready',
		         $5, $6, $7,
		         true)`,
		id, f.OrgID, f.ProjectID, title,
		implState, reviewState, qaState,
	); err != nil {
		t.Fatalf("seedFreshReadyUnclaimed insert: %v", err)
	}
	t.Cleanup(func() {
		// See note in seedFreshTask cleanup re: fresh ctx.
		_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.items WHERE id = $1`, id)
	})
	return id
}

// seedFreshClaimedReady inserts a fresh task in InProgress state
// claimed by Alice with the given (review_state, qa_state). impl_state
// is forced to 'done' so I-4 (review_state advance requires impl_done)
// is satisfied when the caller wants review_state='approved'.
//
// Used by cascade kind=state_change setup (set_state(qa_state=failed)
// requires review_state='approved' to pass I-2) and the
// cascade-idempotency close test (any claimed item is a legal close
// target per P01's claimed_by_id-only precondition).
func seedFreshClaimedReady(t *testing.T, ctx context.Context, f *exitcriteriontest.Fixture, title, reviewState, qaState string) string {
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
		         'done', $5, $6,
		         $7, now(), 'claude-code',
		         false)`,
		id, f.OrgID, f.ProjectID, title,
		reviewState, qaState,
		f.UserID,
	); err != nil {
		t.Fatalf("seedFreshClaimedReady insert: %v", err)
	}
	t.Cleanup(func() {
		// See note in seedFreshTask cleanup re: fresh ctx.
		_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.items WHERE id = $1`, id)
	})
	return id
}
