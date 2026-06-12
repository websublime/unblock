// uniform_validation_test.go is the end-to-end matrix for the
// unblock-tv8.82 uniform §7 argument-validation contract (SPEC §7.3 /
// §7.3.1 / §7.3.2 + §6.2.0a). It drives the real Streamable HTTP MCP
// tools/call surface and asserts that EVERY argument-shape violation —
// missing required, invalid enum, wrong JSON type, out-of-range numeric
// bound, and unknown argument — surfaces the §7 VALIDATION envelope
// (kind=VALIDATION, trace_id, data.field; data.bound on a range
// violation), uniformly, with NO bare isError text frame. A happy-path
// control proves valid in-range arguments still pass.
//
// expectError() (transport_test.go) already unwraps error.data into
// envelopeData{Kind, Tool, TraceID, Details}; we assert on those fields.

package exitcriteriontest_test

import (
	"testing"
)

// TestExitCriterion_UniformValidation_Matrix exercises the four
// violation classes (+ unknown-key + happy control) across a
// representative slice of the 23-tool surface.
func TestExitCriterion_UniformValidation_Matrix(t *testing.T) {
	f := fx(t)
	sessionID := initializeSession(t, f.RawKey)

	// validationCase is one row of the matrix: a tool + arguments that
	// MUST reject with §7 VALIDATION naming wantField (and wantBound when
	// the violation is a numeric range).
	type validationCase struct {
		name      string
		tool      string
		args      map[string]any
		wantField string
		wantBound string // "" → not a range violation; data.bound not asserted
	}

	cases := []validationCase{
		// --- missing required ---
		{
			name:      "claim_missing_item_id",
			tool:      "claim",
			args:      map[string]any{},
			wantField: "item_id",
		},
		{
			name:      "search_missing_query",
			tool:      "search",
			args:      map[string]any{"project_id": f.ProjectID},
			wantField: "query",
		},
		{
			name:      "create_missing_title",
			tool:      "create",
			args:      map[string]any{"project_id": f.ProjectID, "type": "task"},
			wantField: "title",
		},

		// --- invalid enum ---
		{
			name:      "create_invalid_type_enum",
			tool:      "create",
			args:      map[string]any{"project_id": f.ProjectID, "title": "x", "type": "bogus"},
			wantField: "type",
		},
		{
			name:      "create_invalid_priority_enum",
			tool:      "create",
			args:      map[string]any{"project_id": f.ProjectID, "title": "x", "priority": "P9"},
			wantField: "priority",
		},
		{
			name:      "set_state_invalid_impl_state_enum",
			tool:      "set_state",
			args:      map[string]any{"item_id": f.ItemID("itm_a"), "impl_state": "bogus"},
			wantField: "impl_state",
		},
		{
			name:      "ready_invalid_priority_min_enum",
			tool:      "ready",
			args:      map[string]any{"project_id": f.ProjectID, "priority_min": "P9"},
			wantField: "priority_min",
		},
		{
			name:      "comment_invalid_kind_enum",
			tool:      "comment",
			args:      map[string]any{"item_id": f.ItemID("itm_a"), "body": "b", "kind": "bogus"},
			wantField: "kind",
		},
		{
			name:      "add_dependency_invalid_kind_enum",
			tool:      "add_dependency",
			args:      map[string]any{"from_item_id": f.ItemID("itm_a"), "to_item_id": f.ItemID("itm_b"), "kind": "bogus"},
			wantField: "kind",
		},

		// --- wrong type ---
		{
			name:      "ready_limit_wrong_type_string",
			tool:      "ready",
			args:      map[string]any{"project_id": f.ProjectID, "limit": "ten"},
			wantField: "limit",
		},
		{
			name:      "list_status_wrong_type_string",
			tool:      "list",
			args:      map[string]any{"project_id": f.ProjectID, "status": "Ready"},
			wantField: "status",
		},

		// --- out-of-range numeric bound (data.bound asserted) ---
		{
			name:      "prime_ready_limit_zero",
			tool:      "prime",
			args:      map[string]any{"project_id": f.ProjectID, "ready_limit": 0},
			wantField: "ready_limit",
			wantBound: "1..50",
		},
		{
			name:      "prime_ready_limit_above_max",
			tool:      "prime",
			args:      map[string]any{"project_id": f.ProjectID, "ready_limit": 51},
			wantField: "ready_limit",
			wantBound: "1..50",
		},
		{
			name:      "ready_limit_zero",
			tool:      "ready",
			args:      map[string]any{"project_id": f.ProjectID, "limit": 0},
			wantField: "limit",
			wantBound: "1..200",
		},
		{
			name:      "ready_limit_above_max",
			tool:      "ready",
			args:      map[string]any{"project_id": f.ProjectID, "limit": 201},
			wantField: "limit",
			wantBound: "1..200",
		},
		{
			name:      "list_limit_above_max",
			tool:      "list",
			args:      map[string]any{"project_id": f.ProjectID, "limit": 201},
			wantField: "limit",
			wantBound: "1..200",
		},
		{
			name:      "search_limit_zero",
			tool:      "search",
			args:      map[string]any{"project_id": f.ProjectID, "query": "x", "limit": 0},
			wantField: "limit",
			wantBound: "1..100",
		},
		{
			name:      "search_limit_above_max",
			tool:      "search",
			args:      map[string]any{"project_id": f.ProjectID, "query": "x", "limit": 101},
			wantField: "limit",
			wantBound: "1..100",
		},

		// --- unknown argument (additionalProperties:false) ---
		{
			name:      "ready_unknown_argument",
			tool:      "ready",
			args:      map[string]any{"project_id": f.ProjectID, "bogus_field": "v"},
			wantField: "bogus_field",
		},
	}

	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			env := callTool(t, f.RawKey, sessionID, tc.tool, tc.args)
			data := expectError(t, env)
			if data.Kind != "VALIDATION" {
				t.Fatalf("%s: kind = %q, want VALIDATION", tc.tool, data.Kind)
			}
			if data.TraceID == "" {
				t.Fatalf("%s: §7 envelope requires a non-empty trace_id", tc.tool)
			}
			if data.Tool != tc.tool {
				t.Fatalf("%s: data.tool = %q, want %q", tc.tool, data.Tool, tc.tool)
			}
			if got, _ := data.Details["field"].(string); got != tc.wantField {
				t.Fatalf("%s: data.field = %q, want %q (details=%v)", tc.tool, got, tc.wantField, data.Details)
			}
			if tc.wantBound != "" {
				if got, _ := data.Details["bound"].(string); got != tc.wantBound {
					t.Fatalf("%s: data.bound = %q, want %q", tc.tool, got, tc.wantBound)
				}
			}
		})
	}
}

// TestExitCriterion_UniformValidation_HappyControl proves the validation
// layer does NOT reject valid in-range arguments: an OMITTED limit takes
// the per-tool default (not a rejected zero), and an in-range supplied
// limit passes. This is the control that distinguishes "rejects
// out-of-range" from "rejects everything".
func TestExitCriterion_UniformValidation_HappyControl(t *testing.T) {
	f := fx(t)
	sessionID := initializeSession(t, f.RawKey)

	// Omitted limit → default applied, success.
	omittedEnv := callTool(t, f.RawKey, sessionID, "ready", map[string]any{
		"project_id": f.ProjectID,
	})
	_ = expectSuccess(t, omittedEnv)

	// In-range supplied limit → success.
	inRangeEnv := callTool(t, f.RawKey, sessionID, "ready", map[string]any{
		"project_id": f.ProjectID,
		"limit":      1,
	})
	_ = expectSuccess(t, inRangeEnv)

	// In-range supplied prime.ready_limit at the boundary (50) → success.
	primeEnv := callTool(t, f.RawKey, sessionID, "prime", map[string]any{
		"project_id":  f.ProjectID,
		"ready_limit": 50,
	})
	_ = expectSuccess(t, primeEnv)
}
