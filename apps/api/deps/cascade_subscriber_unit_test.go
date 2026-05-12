// Unit tests for the pure-function §5.7.1 derivation table in
// derivePipelineStage. Internal-package test so the unexported helper
// is visible.
//
// These tests do NOT touch the DB or the pubsub topics — they are
// table-driven against the locked SPEC §5.7.1 rule order (rules 1..12,
// first match wins). Tests still run under `encore test` because the
// `deps` package itself imports encore.dev/pubsub at package init —
// plain `go test` panics on the package-level NewTopic call (see
// cascade.go) even for files that do not exercise pub/sub.

package deps

import "testing"

// TestDerivePipelineStage_Rules covers every rule in the SPEC §5.7.1
// derivation table. Order matches the spec (rule 1 first → rule 12
// last). Each subtest names the rule it exercises so a regression
// points at the SPEC line range.
//
// Pure function — no DB, no fixtures. Defaults for unset fields use
// the zero value of itemDerivationInputs (empty strings, false bools);
// each test sets only the fields its rule predicates against, so a
// future spec edit that flips an unrelated field cannot accidentally
// pass these tests.
func TestDerivePipelineStage_Rules(t *testing.T) {
	tests := []struct {
		name string
		in   itemDerivationInputs
		want string
	}{
		// Rule 1: pipeline_state = needs_human → Deferred.
		{
			name: "rule1_needs_human",
			in:   itemDerivationInputs{pipelineState: "needs_human"},
			want: "Deferred",
		},
		// Rule 2: pipeline_state = paused → Deferred.
		{
			name: "rule2_paused",
			in:   itemDerivationInputs{pipelineState: "paused"},
			want: "Deferred",
		},
		// Rule 3: pipeline_state = no_investigation AND impl_state = pending → Implementation.
		{
			name: "rule3_no_investigation_pending_impl",
			in: itemDerivationInputs{
				pipelineState: "no_investigation",
				implState:     "pending",
			},
			want: "Implementation",
		},
		// Rule 4a: status = Done → Done.
		{
			name: "rule4a_status_done",
			in:   itemDerivationInputs{status: "Done"},
			want: "Done",
		},
		// Rule 4b: qa_state = passed AND closed_at IS NOT NULL → Done.
		{
			name: "rule4b_qa_passed_closed_at",
			in: itemDerivationInputs{
				qaState:         "passed",
				closedAtNotNull: true,
			},
			want: "Done",
		},
		// Rule 5: qa_state = passed (closure pending) → Quality.
		{
			name: "rule5_qa_passed_closure_pending",
			in:   itemDerivationInputs{qaState: "passed"},
			want: "Quality",
		},
		// Rule 6: qa_state = failed → Quality.
		{
			name: "rule6_qa_failed",
			in:   itemDerivationInputs{qaState: "failed"},
			want: "Quality",
		},
		// Rule 7: review_state = approved AND qa_state = pending → Quality.
		{
			name: "rule7_review_approved_qa_pending",
			in: itemDerivationInputs{
				reviewState: "approved",
				qaState:     "pending",
			},
			want: "Quality",
		},
		// Rule 8: review_state = needs_rework → Implementation.
		{
			name: "rule8_review_needs_rework",
			in:   itemDerivationInputs{reviewState: "needs_rework"},
			want: "Implementation",
		},
		// Rule 9: impl_state = done AND review_state = pending AND review comment exists → Review.
		{
			name: "rule9_impl_done_review_pending_with_review_comment",
			in: itemDerivationInputs{
				implState:        "done",
				reviewState:      "pending",
				qaState:          "pending",
				hasReviewComment: true,
			},
			want: "Review",
		},
		// Rule 10: impl_state = done AND review_state = pending AND no review comment → Implementation.
		{
			name: "rule10_impl_done_review_pending_no_review_comment",
			in: itemDerivationInputs{
				implState:        "done",
				reviewState:      "pending",
				qaState:          "pending",
				hasReviewComment: false,
			},
			want: "Implementation",
		},
		// Rule 11: impl_state = pending AND investigation comment exists → Implementation.
		{
			name: "rule11_impl_pending_with_investigation_comment",
			in: itemDerivationInputs{
				implState:               "pending",
				reviewState:             "pending",
				qaState:                 "pending",
				hasInvestigationComment: true,
			},
			want: "Implementation",
		},
		// Rule 12: impl_state = pending AND no investigation comment → Investigation.
		{
			name: "rule12_impl_pending_no_investigation_comment",
			in: itemDerivationInputs{
				implState:               "pending",
				reviewState:             "pending",
				qaState:                 "pending",
				hasInvestigationComment: false,
			},
			want: "Investigation",
		},
		// First-match-wins guard: needs_human + qa_passed + status=Done
		// must still return Deferred (rule 1 wins over rule 4).
		{
			name: "first_match_wins_deferred_over_done",
			in: itemDerivationInputs{
				pipelineState:   "needs_human",
				status:          "Done",
				qaState:         "passed",
				closedAtNotNull: true,
			},
			want: "Deferred",
		},
		// First-match-wins guard: paused (rule 2) wins over
		// rule-3's no_investigation + impl_state=pending.
		{
			name: "first_match_wins_paused_over_no_investigation",
			in: itemDerivationInputs{
				pipelineState: "paused",
				implState:     "pending",
			},
			want: "Deferred",
		},
		// Default: a freshly created item (all zeros + impl_state=pending,
		// pipeline_state=running) lands in Investigation via rule 12.
		{
			name: "default_fresh_item_investigation",
			in: itemDerivationInputs{
				status:        "Backlog",
				pipelineState: "running",
				implState:     "pending",
				reviewState:   "pending",
				qaState:       "pending",
			},
			want: "Investigation",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := derivePipelineStage(&tc.in)
			if got != tc.want {
				t.Fatalf("derivePipelineStage(%+v) = %q, want %q", tc.in, got, tc.want)
			}
		})
	}
}
