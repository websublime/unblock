// cascade_pipeline_stage_trigger_test.go closes the two live-confirmed
// pipeline_stage cascade-trigger reproes from bead unblock-tv8.87: the
// cascade-publish trigger set was NARROWER than the §5.7.1 (root
// docs/SPEC.md "authoritative") pipeline_stage derivation-input set, so
// a derived pipeline_stage went STALE until an unrelated impl/review/qa
// write re-fired the subscriber.
//
// Two reproes, now closed:
//
//   REPRO 1 (pure pipeline_state write): set_state(pipeline_state=
//     needs_human) on a claimed item now publishes
//     CascadeRequested{state_change}; driving the subscriber recomputes
//     pipeline_stage to 'Deferred' (§5.7.1 row 1) with NO unrelated
//     impl/review/qa write. Same for 'paused' (→ Deferred) and
//     'no_investigation' with impl=pending (→ Implementation).
//
//   REPRO 2 (investigation/review comment append): comment(kind=
//     investigation) while impl=pending now publishes; the subscriber
//     derives 'Implementation' (§5.7.1 comment-existence row). A
//     comment(kind=review) after impl=done derives 'Review'.
//
// Per SPEC §11.1.1 (round-13): Encore Pub/Sub subscriptions do not fire
// under `encore test`. The harness drives the producing RPC directly
// (private mesh — see the file-level DEVIATION block in
// cascade_kinds_test.go on why the MCP tool surface cannot be used for
// the publish-observation step), captures the publish via
// et.Topic(...).PublishedMessages(), then invokes
// deps.HandleCascadeRequestedForTest directly. The post-drive
// pipeline_stage column is the observable §5.7.1 outcome.
//
// These tests assert BOTH the trigger (a state_change publish fires)
// AND the downstream derivation (the subscriber writes the §5.7.1
// pipeline_stage), so a regression on EITHER half (the new publisher OR
// the unchanged subscriber) is pinpointed.

package exitcriteriontest_test

import (
	"context"
	"testing"

	encoredb "encore.app/db"
	"encore.app/deps"
	"encore.app/exitcriteriontest"
	"encore.app/workitems"
)

// readPipelineStage fetches the subscriber-maintained pipeline_stage
// column by item id. This is the observable §5.7.1 derivation outcome
// the reproes assert on.
func readPipelineStage(t *testing.T, ctx context.Context, itemID string) string {
	t.Helper()
	var stage string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT pipeline_stage FROM workitems.items WHERE id = $1`,
		itemID,
	).Scan(&stage); err != nil {
		t.Fatalf("readPipelineStage %s: %v", itemID, err)
	}
	return stage
}

// driveCascadeOnce captures the single state_change publish for itemID,
// asserts exactly one fired, then drives the subscriber once so the
// pipeline_stage recompute lands. Returns the captured message so the
// caller can assert on its fields if needed.
func driveCascadeOnce(t *testing.T, ctx context.Context, itemID string) *deps.CascadeRequested {
	t.Helper()
	msgs := cascadeMessagesFor(itemID, "state_change")
	if len(msgs) != 1 {
		t.Fatalf("expected exactly 1 state_change publish for item=%s, got %d (the §5.7.1 trigger must fire)", itemID, len(msgs))
	}
	if err := deps.HandleCascadeRequestedForTest(ctx, msgs[0]); err != nil {
		t.Fatalf("HandleCascadeRequestedForTest: %v", err)
	}
	return msgs[0]
}

// seedFreshClaimedPosture inserts a fresh InProgress item claimed by the
// fixture user with the given (impl_state, review_state, qa_state) and
// pipeline_state='running'. A claimed item is the precondition for both
// reproes (set_state(pipeline_state=…) and the comment appends both run
// against a claimed, in-progress item). impl_state is the caller's
// choice (REPRO 1c needs impl=pending, REPRO 2b needs impl=done), so it
// is a parameter unlike seedFreshClaimedReady which forces impl=done.
func seedFreshClaimedPosture(t *testing.T, ctx context.Context, f *exitcriteriontest.Fixture, title, implState, reviewState, qaState string) string {
	t.Helper()
	id := mustULID(t, title)
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status,
		    impl_state, review_state, qa_state, pipeline_state,
		    claimed_by_id, claimed_at, claimed_by_agent,
		    is_ready)
		 VALUES ($1, $2, $3, 'task', $4, 'InProgress',
		         $5, $6, $7, 'running',
		         $8, now(), 'claude-code',
		         false)`,
		id, f.OrgID, f.ProjectID, title,
		implState, reviewState, qaState,
		f.UserID,
	); err != nil {
		t.Fatalf("seedFreshClaimedPosture insert: %v", err)
	}
	t.Cleanup(func() {
		// Fresh background ctx — t.Context() is cancelled by cleanup time.
		_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.items WHERE id = $1`, id)
	})
	return id
}

// TestExitCriterion_PipelineStage_PureNeedsHumanRecomputes — REPRO 1a
// (bead AC #2). A pure set_state(pipeline_state=needs_human) on a
// claimed item (no impl/review/qa change) now publishes state_change;
// driving the subscriber recomputes pipeline_stage from 'Investigation'
// (the impl=pending+no-investigation-comment default, §5.7.1 last row)
// to 'Deferred' (§5.7.1 row 1) with NO unrelated write.
func TestExitCriterion_PipelineStage_PureNeedsHumanRecomputes(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	// impl=pending, review=pending, qa=pending, pipeline=running →
	// §5.7.1 derives 'Investigation' (impl=pending, no investigation
	// comment). Confirm the starting stage so the recompute is a real
	// transition, not a no-op.
	itemID := seedFreshClaimedPosture(t, ctx, f, "repro1a-needs-human", "pending", "pending", "pending")

	needsHuman := "needs_human"
	if _, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:        itemID,
		PipelineState: &needsHuman,
	}); err != nil {
		t.Fatalf("SetStateColumns(pipeline_state=needs_human): %v", err)
	}

	driveCascadeOnce(t, ctx, itemID)

	if got := readPipelineStage(t, ctx, itemID); got != "Deferred" {
		t.Fatalf("REPRO 1a: pipeline_stage = %q after pure pipeline_state=needs_human, want Deferred (§5.7.1 row 1)", got)
	}
}

// TestExitCriterion_PipelineStage_PurePausedRecomputes — REPRO 1b
// (bead AC #2). Pure set_state(pipeline_state=paused) → Deferred
// (§5.7.1 row 2).
func TestExitCriterion_PipelineStage_PurePausedRecomputes(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	itemID := seedFreshClaimedPosture(t, ctx, f, "repro1b-paused", "pending", "pending", "pending")

	paused := "paused"
	if _, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:        itemID,
		PipelineState: &paused,
	}); err != nil {
		t.Fatalf("SetStateColumns(pipeline_state=paused): %v", err)
	}

	driveCascadeOnce(t, ctx, itemID)

	if got := readPipelineStage(t, ctx, itemID); got != "Deferred" {
		t.Fatalf("REPRO 1b: pipeline_stage = %q after pure pipeline_state=paused, want Deferred (§5.7.1 row 2)", got)
	}
}

// TestExitCriterion_PipelineStage_PureNoInvestigationRecomputes — REPRO
// 1c (bead AC #2). Pure set_state(pipeline_state=no_investigation) on an
// impl=pending item → Implementation (§5.7.1 row 3:
// no_investigation AND impl=pending). The starting stage is
// 'Investigation' (running + impl=pending + no investigation comment),
// so this is a real transition driven solely by the pipeline_state
// short-circuit.
func TestExitCriterion_PipelineStage_PureNoInvestigationRecomputes(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	itemID := seedFreshClaimedPosture(t, ctx, f, "repro1c-no-investigation", "pending", "pending", "pending")

	noInvestigation := "no_investigation"
	if _, err := workitems.SetStateColumns(ctx, &workitems.SetStateRequest{
		ItemID:        itemID,
		PipelineState: &noInvestigation,
	}); err != nil {
		t.Fatalf("SetStateColumns(pipeline_state=no_investigation): %v", err)
	}

	driveCascadeOnce(t, ctx, itemID)

	if got := readPipelineStage(t, ctx, itemID); got != "Implementation" {
		t.Fatalf("REPRO 1c: pipeline_stage = %q after pure pipeline_state=no_investigation (impl=pending), want Implementation (§5.7.1 row 3)", got)
	}
}

// TestExitCriterion_PipelineStage_InvestigationCommentRecomputes —
// REPRO 2a (bead AC #3). comment(kind=investigation) while impl=pending
// now publishes state_change; the subscriber derives 'Implementation'
// (§5.7.1 comment-existence row: impl=pending AND a kind=investigation
// comment exists). Starting stage is 'Investigation' (impl=pending, no
// investigation comment yet).
func TestExitCriterion_PipelineStage_InvestigationCommentRecomputes(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	itemID := seedFreshClaimedPosture(t, ctx, f, "repro2a-investigation-comment", "pending", "pending", "pending")

	if _, err := workitems.AppendComment(ctx, &workitems.AppendCommentRequest{
		ItemID:   itemID,
		AuthorID: f.UserID,
		Kind:     "investigation",
		Body:     "investigation finding: root cause traced",
	}); err != nil {
		t.Fatalf("AppendComment(kind=investigation): %v", err)
	}

	driveCascadeOnce(t, ctx, itemID)

	if got := readPipelineStage(t, ctx, itemID); got != "Implementation" {
		t.Fatalf("REPRO 2a: pipeline_stage = %q after kind=investigation comment (impl=pending), want Implementation (§5.7.1 comment-existence row)", got)
	}
}

// TestExitCriterion_PipelineStage_ReviewCommentRecomputes — REPRO 2b
// (bead AC #3). comment(kind=review) after impl=done now publishes
// state_change; the subscriber derives 'Review' (§5.7.1:
// impl=done AND review=pending AND a kind=review comment exists).
// Starting stage is 'Implementation' (impl=done, review=pending, no
// review comment yet — §5.7.1 row "impl=done AND review=pending AND no
// kind=review comment yet").
func TestExitCriterion_PipelineStage_ReviewCommentRecomputes(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	// impl=done requires a claim (structural invariant) — seedFreshClaimedPosture
	// claims the item, so impl=done is legal.
	itemID := seedFreshClaimedPosture(t, ctx, f, "repro2b-review-comment", "done", "pending", "pending")

	if _, err := workitems.AppendComment(ctx, &workitems.AppendCommentRequest{
		ItemID:   itemID,
		AuthorID: f.UserID,
		Kind:     "review",
		Body:     "review finding: needs a second look at error handling",
	}); err != nil {
		t.Fatalf("AppendComment(kind=review): %v", err)
	}

	driveCascadeOnce(t, ctx, itemID)

	if got := readPipelineStage(t, ctx, itemID); got != "Review" {
		t.Fatalf("REPRO 2b: pipeline_stage = %q after kind=review comment (impl=done, review=pending), want Review (§5.7.1 comment-existence row)", got)
	}
}

// TestExitCriterion_PipelineStage_NonDerivationCommentDoesNotPublish —
// negative path (bead AC #4 / SPEC §6.2 Tool 10 Side-effects): a comment
// kind that is NOT a §5.7.1 derivation input (e.g. kind=general) MUST
// NOT publish state_change. Confirms the publish-trigger set EQUALS the
// §5.7.1 derivation-input set — no wider.
func TestExitCriterion_PipelineStage_NonDerivationCommentDoesNotPublish(t *testing.T) {
	f := fx(t)
	ctx := t.Context()

	itemID := seedFreshClaimedPosture(t, ctx, f, "repro-general-comment", "pending", "pending", "pending")

	if _, err := workitems.AppendComment(ctx, &workitems.AppendCommentRequest{
		ItemID:   itemID,
		AuthorID: f.UserID,
		Kind:     "general",
		Body:     "general note, not a §5.7.1 derivation input",
	}); err != nil {
		t.Fatalf("AppendComment(kind=general): %v", err)
	}

	if msgs := cascadeMessagesFor(itemID, "state_change"); len(msgs) != 0 {
		t.Fatalf("kind=general comment must NOT publish state_change for item=%s: got %d (want 0) — only investigation/review kinds are §5.7.1 inputs", itemID, len(msgs))
	}
}
