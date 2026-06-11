// Tests for the workitems service (C-1 / bead unblock-tv8.10).
//
// Scope:
//
//   - Pure-helper unit tests: validateTitle, validateStateEnums,
//     coalesceState, ptrToString, nilString, derefString.
//     Reachable under plain `go test ./apps/api/workitems/...` after
//     the BindDB late-bind shape converted workitems/db.go to a nil
//     pointer (loads without panic outside encore CLI).
//
//   - Pre-DB request validation: Create / Update / AppendComment /
//     SetStateColumns / Claim / CreateMilestone / AssignItem /
//     MilestoneTree input shape rejections. These hit the
//     validation gates BEFORE any sqldb call, so they run under plain
//     `go test` without DB.
//
//   - Integration tests for the SQL bodies (Create insert, Get,
//     SetStateColumns invariants I-1..I-5, Claim race, Close cascade
//     publish, milestone DDL invariants) require the Encore runtime
//     and live under encore test. They are gated by the
//     skipIfNoDB helper which checks the package-level db pointer at
//     test start.

package workitems

import (
	"context"
	"strings"
	"testing"

	"encore.dev/beta/errs"
)

// -----------------------------------------------------------------------------
// Pure-helper tests.
// -----------------------------------------------------------------------------

func TestValidateTitle(t *testing.T) {
	cases := []struct {
		name    string
		input   string
		wantErr bool
	}{
		{"empty rejected", "", true},
		{"single char accepted", "a", false},
		{"200 chars accepted", strings.Repeat("a", titleMaxLen), false},
		{"201 chars rejected", strings.Repeat("a", titleMaxLen+1), true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			err := validateTitle(tc.input)
			if (err != nil) != tc.wantErr {
				t.Fatalf("validateTitle(%q) err=%v wantErr=%v", tc.input, err, tc.wantErr)
			}
		})
	}
}

func TestValidateStateEnumsRejectsUnknownValues(t *testing.T) {
	bogus := "not-a-state"
	cases := []struct {
		name string
		req  *SetStateRequest
	}{
		{"unknown impl_state", &SetStateRequest{ImplState: &bogus}},
		{"unknown review_state", &SetStateRequest{ReviewState: &bogus}},
		{"unknown qa_state", &SetStateRequest{QAState: &bogus}},
		{"unknown pipeline_state", &SetStateRequest{PipelineState: &bogus}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			err := validateStateEnums(tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestValidateStateEnumsAcceptsValidValues(t *testing.T) {
	impl := implDone
	review := reviewApproved
	qa := qaPassed
	pipeline := pipelineStateRunning
	req := &SetStateRequest{
		ImplState:     &impl,
		ReviewState:   &review,
		QAState:       &qa,
		PipelineState: &pipeline,
	}
	if err := validateStateEnums(req); err != nil {
		t.Fatalf("validateStateEnums err = %v, want nil", err)
	}
}

func TestCoalesceState(t *testing.T) {
	cur := "current"
	override := "override"
	if got := coalesceState(nil, cur); got != cur {
		t.Fatalf("coalesceState(nil, %q) = %q, want %q", cur, got, cur)
	}
	if got := coalesceState(&override, cur); got != override {
		t.Fatalf("coalesceState(&override, %q) = %q, want %q", cur, got, override)
	}
}

func TestPtrToString(t *testing.T) {
	v := "hello"
	if got := ptrToString(&v); got != v {
		t.Fatalf("ptrToString(&%q) = %q, want %q", v, got, v)
	}
	if got := ptrToString(nil); got != "" {
		t.Fatalf("ptrToString(nil) = %q, want \"\"", got)
	}
}

func TestNilString(t *testing.T) {
	v := "x"
	if got := nilString(&v); got != v {
		t.Fatalf("nilString(&%q) = %q", v, got)
	}
	if got := nilString(nil); got != "" {
		t.Fatalf("nilString(nil) = %q, want \"\"", got)
	}
}

// -----------------------------------------------------------------------------
// Request-validation tests (pre-DB).
// -----------------------------------------------------------------------------

func TestCreateRejectsBadInput(t *testing.T) {
	cases := []struct {
		name string
		req  *CreateRequest
	}{
		{"nil request", nil},
		{"empty org_id", &CreateRequest{OrgID: "", Title: "Test"}},
		{"empty title", &CreateRequest{OrgID: "org_a", Title: ""}},
		{"invalid type", &CreateRequest{OrgID: "org_a", Type: "bogus", Title: "Test"}},
		{"invalid priority", &CreateRequest{OrgID: "org_a", Title: "Test", Priority: "P9"}},
		{"finding missing parent_id", &CreateRequest{OrgID: "org_a", Type: "finding", Title: "T", DiscoveredFromID: "d", Severity: "minor", KindOfFinding: "review"}},
		{"finding missing severity", &CreateRequest{OrgID: "org_a", Type: "finding", Title: "T", ParentID: "p", DiscoveredFromID: "d", KindOfFinding: "review"}},
		{"finding missing kind_of_finding", &CreateRequest{OrgID: "org_a", Type: "finding", Title: "T", ParentID: "p", DiscoveredFromID: "d", Severity: "minor"}},
		{"finding bad severity", &CreateRequest{OrgID: "org_a", Type: "finding", Title: "T", ParentID: "p", DiscoveredFromID: "d", Severity: "BOGUS", KindOfFinding: "review"}},
		{"finding bad kind_of_finding", &CreateRequest{OrgID: "org_a", Type: "finding", Title: "T", ParentID: "p", DiscoveredFromID: "d", Severity: "minor", KindOfFinding: "BOGUS"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := Create(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestAppendCommentRejectsBadInput(t *testing.T) {
	cases := []struct {
		name string
		req  *AppendCommentRequest
	}{
		{"nil request", nil},
		{"empty item_id", &AppendCommentRequest{Body: "hi"}},
		{"missing author", &AppendCommentRequest{ItemID: "i", Body: "hi"}},
		{"empty body", &AppendCommentRequest{ItemID: "i", AuthorID: "u", Body: "  "}},
		{"bad kind", &AppendCommentRequest{ItemID: "i", AuthorID: "u", Kind: "BOGUS", Body: "hi"}},
		{"bad status", &AppendCommentRequest{ItemID: "i", AuthorID: "u", Status: "BOGUS", Body: "hi"}},
		{"bad agent kind", &AppendCommentRequest{ItemID: "i", AuthorAgent: "BOGUS", Body: "hi"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := AppendComment(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestClaimRejectsBadInput(t *testing.T) {
	cases := []struct {
		name string
		req  *ClaimRequest
	}{
		{"nil", nil},
		{"empty item_id", &ClaimRequest{ClaimerUserID: "u"}},
		{"empty claimer_user_id", &ClaimRequest{ItemID: "i"}},
		{"bad agent", &ClaimRequest{ItemID: "i", ClaimerUserID: "u", ClaimerAgent: "BOGUS"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := Claim(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestSetStateColumnsRejectsBadInput(t *testing.T) {
	bogus := "BOGUS"
	cases := []struct {
		name string
		req  *SetStateRequest
	}{
		{"nil", nil},
		{"empty item_id", &SetStateRequest{}},
		{"bad impl", &SetStateRequest{ItemID: "i", ImplState: &bogus}},
		{"bad review", &SetStateRequest{ItemID: "i", ReviewState: &bogus}},
		{"bad qa", &SetStateRequest{ItemID: "i", QAState: &bogus}},
		{"bad pipeline", &SetStateRequest{ItemID: "i", PipelineState: &bogus}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := SetStateColumns(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestCreateMilestoneRejectsBadInput(t *testing.T) {
	cases := []struct {
		name string
		req  *CreateMilestoneRequest
	}{
		{"nil", nil},
		{"missing scope", &CreateMilestoneRequest{Name: "Q1", StartDate: "2026-01-01", EndDate: "2026-03-31"}},
		{"both scope", &CreateMilestoneRequest{OrgID: "o", ProjectID: "p", Name: "Q1", StartDate: "2026-01-01", EndDate: "2026-03-31"}},
		{"empty name", &CreateMilestoneRequest{OrgID: "o", Name: "", StartDate: "2026-01-01", EndDate: "2026-03-31"}},
		{"bad start date", &CreateMilestoneRequest{OrgID: "o", Name: "Q1", StartDate: "2026/01/01", EndDate: "2026-03-31"}},
		{"bad end date", &CreateMilestoneRequest{OrgID: "o", Name: "Q1", StartDate: "2026-01-01", EndDate: "bogus"}},
		{"end before start", &CreateMilestoneRequest{OrgID: "o", Name: "Q1", StartDate: "2026-03-31", EndDate: "2026-01-01"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := CreateMilestone(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestMilestoneTreeRequiresScope(t *testing.T) {
	_, err := MilestoneTree(context.Background(), &MilestoneTreeRequest{})
	if err == nil {
		t.Fatalf("expected InvalidArgument, got nil")
	}
	if errs.Code(err) != errs.InvalidArgument {
		t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
	}
}

// -----------------------------------------------------------------------------
// Label-registry RPC input rejection (round-16, bead unblock-tv8.75).
// These hit the pre-DB validation paths only — every case returns before
// any db.Exec, so they run under plain `go test` without the Encore
// runtime. Persistence + CONFLICT + cascade-detach are covered by the
// §11.4 E2E harness (exitcriteriontest/labels_mcp_test.go).
// -----------------------------------------------------------------------------

func TestCreateLabelRejectsBadInput(t *testing.T) {
	longName := strings.Repeat("x", labelNameMaxLen+1)
	cases := []struct {
		name string
		req  *CreateLabelRequest
	}{
		{"nil", nil},
		{"missing scope", &CreateLabelRequest{Name: "bug", Color: "#d73a4a"}},
		{"both scope", &CreateLabelRequest{OrgID: "o", ProjectID: "p", Name: "bug", Color: "#d73a4a"}},
		{"empty name", &CreateLabelRequest{OrgID: "o", Name: "", Color: "#d73a4a"}},
		{"name too long", &CreateLabelRequest{OrgID: "o", Name: longName, Color: "#d73a4a"}},
		{"bad color no hash", &CreateLabelRequest{OrgID: "o", Name: "bug", Color: "d73a4a"}},
		{"bad color short", &CreateLabelRequest{OrgID: "o", Name: "bug", Color: "#fff"}},
		{"bad color non-hex", &CreateLabelRequest{OrgID: "o", Name: "bug", Color: "#gggggg"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := CreateLabel(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestListLabelsRequiresOrgScope(t *testing.T) {
	cases := []struct {
		name string
		req  *ListLabelsRequest
	}{
		{"nil", nil},
		{"empty org", &ListLabelsRequest{}},
		{"project without org", &ListLabelsRequest{ProjectID: "p"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := ListLabels(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestUpdateLabelRejectsBadInput(t *testing.T) {
	longName := strings.Repeat("x", labelNameMaxLen+1)
	badName := longName
	badColor := "not-a-color"
	cases := []struct {
		name string
		req  *UpdateLabelRequest
	}{
		{"nil", nil},
		{"empty label_id", &UpdateLabelRequest{}},
		{"empty caller org", &UpdateLabelRequest{LabelID: "l"}},
		{"name too long", &UpdateLabelRequest{LabelID: "l", CallerOrgID: "o", Name: &badName}},
		{"bad color", &UpdateLabelRequest{LabelID: "l", CallerOrgID: "o", Color: &badColor}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := UpdateLabel(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

func TestDeleteLabelRejectsBadInput(t *testing.T) {
	cases := []struct {
		name string
		req  *DeleteLabelRequest
	}{
		{"nil", nil},
		{"empty label_id", &DeleteLabelRequest{}},
		{"empty caller org", &DeleteLabelRequest{LabelID: "l"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := DeleteLabel(context.Background(), tc.req)
			if err == nil {
				t.Fatalf("expected InvalidArgument, got nil")
			}
			if errs.Code(err) != errs.InvalidArgument {
				t.Fatalf("err code = %v, want InvalidArgument", errs.Code(err))
			}
		})
	}
}

// TestLabelColorPatternMatchesDDL pins the Go-side color guard to the
// labels_color_chk DDL regex (#RRGGBB) so the early VALIDATION matches the
// DB's last-line-of-defence CHECK exactly.
func TestLabelColorPatternMatchesDDL(t *testing.T) {
	good := []string{"#d73a4a", "#FFFFFF", "#000000", "#AbCdEf"}
	bad := []string{"", "d73a4a", "#fff", "#1234567", "#gggggg", "#12 456"}
	for _, c := range good {
		if !labelColorPattern.MatchString(c) {
			t.Errorf("color %q should match #RRGGBB", c)
		}
	}
	for _, c := range bad {
		if labelColorPattern.MatchString(c) {
			t.Errorf("color %q should NOT match #RRGGBB", c)
		}
	}
}

// -----------------------------------------------------------------------------
// preconditionError shape.
// -----------------------------------------------------------------------------

func TestPreconditionErrorCarriesInvariantInMeta(t *testing.T) {
	err := preconditionError("M-INV-7", "scope unreachable")
	e, ok := err.(*errs.Error)
	if !ok {
		t.Fatalf("expected *errs.Error, got %T", err)
	}
	if e.Code != errs.FailedPrecondition {
		t.Fatalf("err code = %v, want FailedPrecondition", e.Code)
	}
	if e.Meta["invariant"] != "M-INV-7" {
		t.Fatalf("meta[invariant] = %v, want M-INV-7", e.Meta["invariant"])
	}
}
