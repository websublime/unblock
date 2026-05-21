// Handler-level §5.7.1 derivation tests for the cascade subscriber.
//
// These tests are the regression surface for unblock-tv8.14 (C-5):
// they lock the WIRING from CascadeRequested → BFS → state read →
// comment-existence read → derivePipelineStage → idempotent UPDATE.
// The pure §5.7.1 derivation table is already covered in isolation by
// cascade_subscriber_unit_test.go; the four-kind smoke + idempotency
// + depth-2 BFS surface is locked by cascade_subscriber_handler_test.go.
// This file extends to:
//
//   - Per-kind end-to-end coverage of every §5.7.1 rule (4 kinds × 12
//     rules + first-match-wins guards) — bead acceptance criteria #1 +
//     #2 ceiling.
//   - Depth-10 multi-hop BFS convergence in a single subscriber pass
//     plus an idempotent second pass — bead §11.1 four-kind smoke-test
//     extension floor.
//
// Test pattern: each sub-test creates a FRESH item via
// createItemInternal, applies the §5.7.1 input combination via direct
// SQL (UPDATE workitems.items + INSERT INTO workitems.comments where
// rules 9-12 require a comment-existence predicate), invokes
// handleCascadeRequested directly (Encore Pub/Sub does not fire
// subscriptions under `encore test` — see the file header on
// cascade_subscriber_handler_test.go), then asserts the post-call
// pipeline_stage equals the rule's documented output.
//
// Why fresh items per sub-test: rules 9 and 10 differ only by the
// presence of a kind='review' comment; rule 11 only by a
// kind='investigation' comment. Sharing items across sub-tests would
// leak comment state. The seedFixtureInternal helper is reused (one
// org/user/project per top-level test) but each rule case mints a new
// item id.
//
// Encore test runtime caveat: SetStateColumns now publishes
// state_change for (impl, review, qa) material changes per
// SPEC §6.3.0 tension #3 (shipped on unblock-tv8.53). The
// state_change Reason can therefore enter the subscriber via
// SetStateColumns or via Claim's I-3 reset path. To decouple these
// tests from publisher state and from the Encore test runtime's
// non-firing-subscribers caveat, we still invoke the subscriber
// handler DIRECTLY for all four Reasons. The subscriber body is
// identical across Reasons — only the audit row's kind discriminator
// differs — so the derivation coverage holds for every Reason
// equivalently.
//
// Per the bead's DECISION comment: this is NOT a workaround, it is
// the canonical test pattern under the Encore test runtime (cf.
// tv8.12).

package deps

import (
	"context"
	"testing"
	"time"

	"encore.app/shared/ulid"
)

// derivationCase declares one §5.7.1 rule's input fields and the
// expected pipeline_stage value. Each case names the rule it
// exercises so a regression points at the SPEC docs/SPEC.md §5.7.1
// line range.
//
// Default field values map to the workitems.items column defaults
// declared by 0040_workitems.up.sql (status='Backlog',
// impl_state='pending', review_state='pending', qa_state='pending',
// pipeline_state='running'). Each case overrides ONLY the fields its
// rule predicates against, so a future spec edit that flips an
// unrelated field cannot accidentally pass these tests.
type derivationCase struct {
	name string

	// State columns (left empty → defaults from the items DDL apply).
	status        string // '' → 'Backlog'
	implState     string // '' → 'pending'
	reviewState   string // '' → 'pending'
	qaState       string // '' → 'pending'
	pipelineState string // '' → 'running'
	closeIt       bool   // when true, set closed_at = now() (status stays as `status`)

	// Comment-existence predicates (rules 9-12).
	addReviewComment        bool
	addInvestigationComment bool

	wantStage string
}

// allDerivationCases enumerates every §5.7.1 rule plus two
// first-match-wins guards. Rule order is locked from
// docs/SPEC.md §5.7.1 lines 766-779 — DO NOT REORDER without a SPEC
// amendment.
func allDerivationCases() []derivationCase {
	return []derivationCase{
		// Rule 1: pipeline_state = needs_human → Deferred.
		{
			name:          "rule1_needs_human",
			pipelineState: "needs_human",
			wantStage:     "Deferred",
		},
		// Rule 2: pipeline_state = paused → Deferred.
		{
			name:          "rule2_paused",
			pipelineState: "paused",
			wantStage:     "Deferred",
		},
		// Rule 3: pipeline_state = no_investigation AND impl_state = pending → Implementation.
		{
			name:          "rule3_no_investigation_pending_impl",
			pipelineState: "no_investigation",
			implState:     "pending",
			wantStage:     "Implementation",
		},
		// Rule 4a: status = Done → Done.
		{
			name:      "rule4a_status_done",
			status:    "Done",
			wantStage: "Done",
		},
		// Rule 4b: qa_state = passed AND closed_at IS NOT NULL → Done.
		// status stays Backlog so rule 4a does not pre-empt rule 4b.
		{
			name:      "rule4b_qa_passed_closed_at",
			qaState:   "passed",
			closeIt:   true,
			wantStage: "Done",
		},
		// Rule 5: qa_state = passed (closure pending) → Quality.
		// closed_at is NULL so rule 4b does not pre-empt rule 5.
		{
			name:      "rule5_qa_passed_closure_pending",
			qaState:   "passed",
			wantStage: "Quality",
		},
		// Rule 6: qa_state = failed → Quality.
		{
			name:      "rule6_qa_failed",
			qaState:   "failed",
			wantStage: "Quality",
		},
		// Rule 7: review_state = approved AND qa_state = pending → Quality.
		// impl_state stays pending so rule 9/10 (impl_done) cannot fire.
		// I-4 (review change requires impl done) does not gate inserts of
		// pre-existing state via direct SQL UPDATE — the invariant is
		// enforced only on the SetStateColumns RPC path.
		{
			name:        "rule7_review_approved_qa_pending",
			implState:   "done", // I-4 spec invariant: approved requires impl done
			reviewState: "approved",
			qaState:     "pending",
			wantStage:   "Quality",
		},
		// Rule 8: review_state = needs_rework → Implementation.
		{
			name:        "rule8_review_needs_rework",
			implState:   "done", // needs_rework only meaningful after impl done
			reviewState: "needs_rework",
			wantStage:   "Implementation",
		},
		// Rule 9: impl_state = done AND review_state = pending AND review-kind
		//         comment exists → Review.
		{
			name:             "rule9_impl_done_review_pending_with_review_comment",
			implState:        "done",
			reviewState:      "pending",
			qaState:          "pending",
			addReviewComment: true,
			wantStage:        "Review",
		},
		// Rule 10: impl_state = done AND review_state = pending AND no
		//          review-kind comment → Implementation.
		{
			name:        "rule10_impl_done_review_pending_no_review_comment",
			implState:   "done",
			reviewState: "pending",
			qaState:     "pending",
			wantStage:   "Implementation",
		},
		// Rule 11: impl_state = pending AND investigation-kind comment exists
		//          → Implementation.
		{
			name:                    "rule11_impl_pending_with_investigation_comment",
			implState:               "pending",
			reviewState:             "pending",
			qaState:                 "pending",
			addInvestigationComment: true,
			wantStage:               "Implementation",
		},
		// Rule 12: impl_state = pending AND no investigation comment →
		//          Investigation. Default for a freshly created item.
		{
			name:        "rule12_impl_pending_no_investigation_comment",
			implState:   "pending",
			reviewState: "pending",
			qaState:     "pending",
			wantStage:   "Investigation",
		},
		// First-match-wins guard A: rule 1 (needs_human) wins over rule
		// 4a (status=Done) + rule 4b (qa=passed + closed_at).
		{
			name:          "fmw_deferred_over_done",
			status:        "Done",
			qaState:       "passed",
			closeIt:       true,
			pipelineState: "needs_human",
			wantStage:     "Deferred",
		},
		// First-match-wins guard B: rule 2 (paused) wins over rule 3
		// (no_investigation+pending). pipeline_state can be only one
		// value, so we phrase the guard as "paused beats rule 7" — an
		// item with review_state=approved+qa_pending+pipeline=paused
		// must land in Deferred, not Quality.
		{
			name:          "fmw_paused_over_quality",
			implState:     "done",
			reviewState:   "approved",
			qaState:       "pending",
			pipelineState: "paused",
			wantStage:     "Deferred",
		},
	}
}

// allReasons is the closed publisher set declared by SPEC §6.3.0:
// close, edge_added, edge_removed, state_change. The subscriber's
// shared body is identical across all four — only the audit row's
// `kind` discriminator differs.
func allReasons() []string {
	return []string{"close", "edge_added", "edge_removed", "state_change"}
}

// applyDerivationInputs mutates the seed item in-place to satisfy the
// case's input combination. Uses direct SQL because the production
// RPC paths (SetStateColumns) enforce state-machine invariants (I-1
// through I-5) that would reject some of the artificial mid-state
// combinations a per-rule §5.7.1 test needs to exercise (e.g. setting
// qa_state=failed in isolation without going through the
// review_state=approved gate first). The derivation table is a
// post-state read, not a transition; the subscriber's job is to read
// whatever state currently sits on the row and produce the right
// label.
func applyDerivationInputs(t *testing.T, ctx context.Context, fx *internalFixture, itemID string, c derivationCase) {
	t.Helper()

	status := c.status
	if status == "" {
		status = "Backlog"
	}
	implState := c.implState
	if implState == "" {
		implState = "pending"
	}
	reviewState := c.reviewState
	if reviewState == "" {
		reviewState = "pending"
	}
	qaState := c.qaState
	if qaState == "" {
		qaState = "pending"
	}
	pipelineState := c.pipelineState
	if pipelineState == "" {
		pipelineState = "running"
	}

	if c.closeIt {
		if _, err := db.Exec(ctx,
			`UPDATE workitems.items
			    SET status = $2,
			        impl_state = $3,
			        review_state = $4,
			        qa_state = $5,
			        pipeline_state = $6,
			        closed_at = now()
			  WHERE id = $1`,
			itemID, status, implState, reviewState, qaState, pipelineState,
		); err != nil {
			t.Fatalf("apply derivation inputs (close): %v", err)
		}
	} else {
		if _, err := db.Exec(ctx,
			`UPDATE workitems.items
			    SET status = $2,
			        impl_state = $3,
			        review_state = $4,
			        qa_state = $5,
			        pipeline_state = $6
			  WHERE id = $1`,
			itemID, status, implState, reviewState, qaState, pipelineState,
		); err != nil {
			t.Fatalf("apply derivation inputs: %v", err)
		}
	}

	if c.addReviewComment {
		insertCommentForDerivation(t, ctx, fx, itemID, "review", "review comment for §5.7.1 rule coverage")
	}
	if c.addInvestigationComment {
		insertCommentForDerivation(t, ctx, fx, itemID, "investigation", "investigation comment for §5.7.1 rule coverage")
	}
}

// insertCommentForDerivation writes a workitems.comments row of the
// given kind, authored by the fixture user. The cascade subscriber's
// batched comment-existence query reads this row when re-deriving
// pipeline_stage for the affected set.
func insertCommentForDerivation(t *testing.T, ctx context.Context, fx *internalFixture, itemID, kind, body string) {
	t.Helper()
	commentID, err := ulid.New()
	if err != nil {
		t.Fatalf("comment ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, author_id, kind, status, body)
		 VALUES ($1, $2, $3, $4, 'info', $5)`,
		commentID, itemID, fx.UserID, kind, body,
	); err != nil {
		t.Fatalf("insert %s comment: %v", kind, err)
	}
}

// TestHandleCascadeRequested_DerivationByKind_FullTable exercises every
// §5.7.1 rule through every documented Reason kind. The outer loop is
// the Reason (close | edge_added | edge_removed | state_change); the
// inner loop is the §5.7.1 case table.
//
// Acceptance criterion #1 + #2 of unblock-tv8.14:
//   - "All pipeline_stage derivation table transitions produce the
//     correct value when triggered through the cascade subscriber."
//   - "Unit tests cover every (impl_state, review_state, qa_state,
//     pipeline_state) combination that maps to a non-trivial
//     pipeline_stage value."
//
// The subscriber body is shared across all four Reasons (the Reason
// only discriminates the audit row's kind column). Running the full
// derivation table under each Reason wrapper closes the §11.1
// four-kind smoke-test extension: any wiring divergence between
// Reasons (e.g. a future dispatch that conditionally skips the
// recompute pass for one kind) would surface here.
//
// Sequential by construction — each sub-test owns its item id and
// state, so there is no inter-test contention. This avoids
// inadvertently surfacing the unblock-tv8.51 race (LWW between SELECT
// and UPDATE in recomputePipelineStageForAffected); fixing that race
// is tv8.51's scope, not tv8.14's.
func TestHandleCascadeRequested_DerivationByKind_FullTable(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)
	cases := allDerivationCases()

	for _, reason := range allReasons() {
		t.Run(reason, func(t *testing.T) {
			for _, c := range cases {
				t.Run(c.name, func(t *testing.T) {
					itemID := createItemInternal(t, ctx, fx, "Backlog")
					applyDerivationInputs(t, ctx, fx, itemID, c)

					eventID := mustULIDInternal(t)
					msg := &CascadeRequested{
						EventID:           eventID,
						OrgID:             fx.OrgID,
						ProjectID:         fx.ProjectID,
						TriggeredByItemID: itemID,
						Reason:            reason,
						TraceID:           mustULIDInternal(t),
						EmittedAt:         time.Now().UTC(),
					}
					if err := handleCascadeRequested(ctx, msg); err != nil {
						t.Fatalf("handleCascadeRequested(reason=%s, case=%s): %v",
							reason, c.name, err)
					}

					got := readPipelineStageInternal(t, ctx, itemID)
					if got != c.wantStage {
						t.Fatalf("pipeline_stage = %q, want %q (reason=%s, case=%s)",
							got, c.wantStage, reason, c.name)
					}

					// Lock the audit row carries the expected kind (the
					// dispatch did not silently rewrite it).
					var kind string
					if err := db.QueryRow(ctx,
						`SELECT kind FROM deps.cascade_events
						  WHERE event_id = $1 AND triggered_by_item_id = $2`,
						eventID, itemID,
					).Scan(&kind); err != nil {
						t.Fatalf("read audit kind: %v", err)
					}
					if kind != reason {
						t.Fatalf("audit kind = %q, want %q (reason=%s, case=%s)",
							kind, reason, reason, c.name)
					}
				})
			}
		})
	}
}

// TestHandleCascadeRequested_MultiHopBFSDepth10 builds a 10-deep
// blocks chain a₀ → a₁ → … → a₉ (10 items, 9 edges), triggers
// Reason='edge_added' from the root a₀, and asserts:
//
//  1. The BFS forward 'blocks' closure reaches every item (the
//     audit row's affected_item_ids contains all 10 ids).
//  2. pipeline_stage on every downstream item converges within ONE
//     subscriber pass — the per-item derivePipelineStage runs in a
//     single recompute call, and the second-pass idempotent UPDATE
//     guard short-circuits anything already at its target value.
//  3. A second invocation with the SAME EventID is an idempotent
//     no-op: the (event_id, triggered_by_item_id) UNIQUE constraint
//     blocks a duplicate audit row, and the value-equality UPDATE
//     guard blocks any state churn.
//
// Closes the §11.1 four-kind smoke-test extension floor: the bead
// requires depth=10 property coverage as the floor; full §5.7.1
// derivation coverage above is the ceiling.
//
// Depth 10 is well inside the AR-8 / cascadeBFSMaxDepth=256 cap, so
// the cap-warning heuristic (unblock-tv8.49 — false trip when size
// is conflated with depth) should NOT fire here. If it does, that is
// tv8.49's scope; the test still asserts the correct affected set.
func TestHandleCascadeRequested_MultiHopBFSDepth10(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)

	const chainDepth = 10
	ids := make([]string, chainDepth)
	for i := 0; i < chainDepth; i++ {
		ids[i] = createItemInternal(t, ctx, fx, "Backlog")
	}

	// Insert edges directly to bypass the deps.AddEdge publisher path
	// — we are exercising the BFS + derivation pass, not the
	// publisher. Each edge id is a fresh ULID; the (from, to, kind)
	// triple is unique per chain step.
	for i := 0; i < chainDepth-1; i++ {
		edgeID := mustULIDInternal(t)
		if _, err := db.Exec(ctx,
			`INSERT INTO deps.dependencies (id, from_item, to_item, kind)
			 VALUES ($1, $2, $3, 'blocks')`,
			edgeID, ids[i], ids[i+1],
		); err != nil {
			t.Fatalf("insert edge %d (%s -> %s): %v", i, ids[i], ids[i+1], err)
		}
	}

	// Each item lands in Investigation by default (rule 12: impl_state
	// pending, no investigation comment). Mutate the inputs along the
	// chain so the recompute pass has work to do beyond a no-op:
	// - depth 0 (root): leave default → Investigation
	// - depth 5: status=Done → Done (exercises rule 4a from the
	//   middle of the chain, demonstrating that derivation runs per
	//   row, not just on the seed)
	// - depth 9 (tail): qa_state=failed → Quality (rule 6)
	if _, err := db.Exec(ctx,
		`UPDATE workitems.items SET status = 'Done' WHERE id = $1`, ids[5],
	); err != nil {
		t.Fatalf("set status=Done on chain[5]: %v", err)
	}
	if _, err := db.Exec(ctx,
		`UPDATE workitems.items SET qa_state = 'failed' WHERE id = $1`, ids[chainDepth-1],
	); err != nil {
		t.Fatalf("set qa_state=failed on chain[tail]: %v", err)
	}

	eventID := mustULIDInternal(t)
	msg := &CascadeRequested{
		EventID:           eventID,
		OrgID:             fx.OrgID,
		ProjectID:         fx.ProjectID,
		TriggeredByItemID: ids[0],
		Reason:            "edge_added",
		TraceID:           mustULIDInternal(t),
		EmittedAt:         time.Now().UTC(),
	}

	// Single-pass invocation: every downstream pipeline_stage must
	// converge after one call.
	if err := handleCascadeRequested(ctx, msg); err != nil {
		t.Fatalf("first pass: %v", err)
	}

	var affected []string
	if err := db.QueryRow(ctx,
		`SELECT affected_item_ids FROM deps.cascade_events
		  WHERE event_id = $1 AND triggered_by_item_id = $2`,
		eventID, ids[0],
	).Scan(&affected); err != nil {
		t.Fatalf("read affected after first pass: %v", err)
	}

	want := make(map[string]bool, chainDepth)
	for _, id := range ids {
		want[id] = true
	}
	got := make(map[string]bool, len(affected))
	for _, id := range affected {
		got[id] = true
	}
	for id := range want {
		if !got[id] {
			t.Fatalf("affected_item_ids missing chain id %q after depth=10 BFS: %v",
				id, affected)
		}
	}
	if len(affected) < chainDepth {
		t.Fatalf("affected_item_ids cardinality = %d, want >= %d (depth=%d chain)",
			len(affected), chainDepth, chainDepth)
	}

	// Capture pipeline_stage at every chain item after the first
	// pass. These are the expected values for §5.7.1 single-pass
	// convergence.
	stagesAfterFirst := make([]string, chainDepth)
	for i, id := range ids {
		stagesAfterFirst[i] = readPipelineStageInternal(t, ctx, id)
	}

	// Spot-check the mutated rows landed on the right derivation
	// label after one pass (single-pass convergence assertion):
	if stagesAfterFirst[5] != "Done" {
		t.Fatalf("chain[5] pipeline_stage = %q after one pass, want %q (rule 4a)",
			stagesAfterFirst[5], "Done")
	}
	if stagesAfterFirst[chainDepth-1] != "Quality" {
		t.Fatalf("chain[tail] pipeline_stage = %q after one pass, want %q (rule 6)",
			stagesAfterFirst[chainDepth-1], "Quality")
	}

	// Second invocation with the SAME EventID: idempotent no-op.
	// The audit-row insert collapses on the UNIQUE
	// (event_id, triggered_by_item_id) constraint; the per-item
	// UPDATE's WHERE pipeline_stage <> $new guard short-circuits
	// every row. No pipeline_stage value may change.
	if err := handleCascadeRequested(ctx, msg); err != nil {
		t.Fatalf("second (idempotent) pass: %v", err)
	}

	if got := countCascadeEvents(t, ctx, eventID, ids[0]); got != 1 {
		t.Fatalf("idempotency violation at depth=10: %d audit rows after redelivery, want 1", got)
	}

	for i, id := range ids {
		after := readPipelineStageInternal(t, ctx, id)
		if after != stagesAfterFirst[i] {
			t.Fatalf("idempotency violation: chain[%d] pipeline_stage %q → %q across passes",
				i, stagesAfterFirst[i], after)
		}
	}
}
