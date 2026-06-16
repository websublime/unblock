// handler_update_milestone_test.go locks the §7.3 data.field contract for
// update_milestone's cancelled_at argument (bead unblock-tv8.88).
//
// The bug it guards against: cancelled_at used to be a *time.Time on the
// input struct, so a non-RFC3339 value (the empty string, a date-only
// "2026-06-15", "abc", "2026-06-15 00:00:00") passed the shared
// validateArgs pass (which advertises cancelled_at as a plain {type:string})
// and then failed the typed json.Unmarshal INSIDE registerValidatedTool,
// hitting the generic catch-all that mints VALIDATION with the MISLEADING
// data.field='arguments' / reason='arguments must be a JSON object'. §7.3
// requires the VALIDATION envelope to NAME the offending argument.
//
// These tests invoke handleUpdateMilestone directly with a registered
// request identity and a valid milestone_id, so the cancelled_at parse runs
// and rejects BEFORE the backing workitems RPC (no DB needed). They assert
// the rejection is a §7 VALIDATION envelope whose data.field is 'cancelled_at'
// (not 'arguments') with a meaningful reason. A separate case pins that the
// nil (omitted) cancelled_at path is NOT rejected at the parse boundary.

package mcp

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"

	sdkjsonrpc "github.com/modelcontextprotocol/go-sdk/jsonrpc"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// reqWithIdentity builds a CallToolRequest carrying a registered request
// identity (OrgID/UserID non-empty so identityFromReq succeeds), returning
// the request and a release func the caller MUST defer. The trace_id is a
// fixed test sentinel; the registry is keyed by it.
func reqWithIdentity(t *testing.T, tool, traceID string) (*sdkmcp.CallToolRequest, func()) {
	t.Helper()
	state := &requestState{
		Call:    &ToolCall{},
		TraceID: traceID,
		Identity: requestIdentity{
			UserID:    "usr_test_update_milestone",
			OrgID:     "org_test_update_milestone",
			AgentKind: "test",
		},
	}
	release := registerRequestState(traceID, state)
	req := &sdkmcp.CallToolRequest{
		Params: &sdkmcp.CallToolParamsRaw{Name: tool},
		Extra: &sdkmcp.RequestExtra{
			Header: http.Header{traceIDHeader: []string{traceID}},
		},
	}
	return req, release
}

// envelopeFieldOf extracts data.field + data.kind from a mapError-produced
// *sdkjsonrpc.Error (the §7 envelope payload). Fails the test if err is not a
// jsonrpc envelope error.
func envelopeFieldOf(t *testing.T, err error) (kind, field, reason string) {
	t.Helper()
	if err == nil {
		t.Fatalf("expected a VALIDATION envelope error, got nil")
	}
	je, ok := err.(*sdkjsonrpc.Error)
	if !ok {
		t.Fatalf("expected *sdkjsonrpc.Error, got %T: %v", err, err)
	}
	var payload struct {
		Kind    string `json:"kind"`
		Details struct {
			Field  string `json:"field"`
			Reason string `json:"reason"`
		} `json:"details"`
	}
	if uerr := json.Unmarshal(je.Data, &payload); uerr != nil {
		t.Fatalf("envelope Data does not unmarshal: %v (raw=%s)", uerr, string(je.Data))
	}
	return payload.Kind, payload.Details.Field, payload.Details.Reason
}

// TestUpdateMilestone_BadCancelledAt_FieldIsCancelledAt is the bead
// unblock-tv8.88 repro: every non-RFC3339 cancelled_at must surface as a §7
// VALIDATION envelope naming data.field='cancelled_at' (NOT 'arguments').
func TestUpdateMilestone_BadCancelledAt_FieldIsCancelledAt(t *testing.T) {
	// Each case is a cancelled_at value the live sweep flagged as
	// mis-fielded: the empty string (the reporter's "natural uncancel"),
	// date-only (the format start/end_date accept), free text, and a
	// space-separated near-timestamp.
	cases := []struct {
		name        string
		cancelledAt string
	}{
		{"empty string", ""},
		{"date only", "2026-06-15"},
		{"free text", "abc"},
		{"space separated", "2026-06-15 00:00:00"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			traceID := "trace_tv8_88_" + c.name
			req, release := reqWithIdentity(t, "update_milestone", traceID)
			defer release()

			in := updateMilestoneIn{
				MilestoneID: "mst_01J0000000000000000000000",
				CancelledAt: &c.cancelledAt,
			}
			_, _, err := handleUpdateMilestone(context.Background(), req, in)

			kind, field, reason := envelopeFieldOf(t, err)
			if kind != "VALIDATION" {
				t.Fatalf("kind = %q, want VALIDATION", kind)
			}
			if field != "cancelled_at" {
				t.Fatalf("data.field = %q, want cancelled_at (the §7.3 mis-field bug)", field)
			}
			if field == "arguments" {
				t.Fatalf("data.field regressed to the mis-fielded 'arguments' (bead unblock-tv8.88)")
			}
			if reason == "" {
				t.Fatalf("data.reason is empty; §7.3 requires a meaningful reason")
			}
		})
	}
}

// TestUpdateMilestone_ValidRFC3339_PassesParse pins that a valid RFC3339
// cancelled_at is NOT rejected at the parse boundary — it proceeds past the
// parse into the backing RPC. We can't assert success here (no DB), but we
// CAN assert the error, if any, is no longer the cancelled_at parse
// VALIDATION (i.e. the parse accepted the value).
func TestUpdateMilestone_ValidRFC3339_PassesParse(t *testing.T) {
	traceID := "trace_tv8_88_valid"
	req, release := reqWithIdentity(t, "update_milestone", traceID)
	defer release()

	valid := "2026-06-15T00:00:00Z"
	in := updateMilestoneIn{
		MilestoneID: "mst_01J0000000000000000000000",
		CancelledAt: &valid,
	}
	_, _, err := handleUpdateMilestone(context.Background(), req, in)
	if err == nil {
		// No DB in this unit path normally means the backing RPC errors;
		// a nil error would only happen with a live cluster + seeded row,
		// which is fine too — the parse certainly passed.
		return
	}
	je, ok := err.(*sdkjsonrpc.Error)
	if !ok {
		// A non-envelope error (e.g. a runtime/db error) still proves the
		// parse boundary was passed — the cancelled_at parse mints an
		// envelope, not a bare error.
		return
	}
	var payload struct {
		Details struct {
			Field string `json:"field"`
		} `json:"details"`
	}
	_ = json.Unmarshal(je.Data, &payload)
	if payload.Details.Field == "cancelled_at" {
		t.Fatalf("a valid RFC3339 cancelled_at was rejected at the parse boundary (field=cancelled_at); it must pass")
	}
}

// TestUpdateMilestone_NilCancelledAt_NoParseReject pins that an omitted
// cancelled_at (nil pointer) is never rejected by the parse step — the
// nil-is-unchanged contract (§4.4.1) is preserved.
func TestUpdateMilestone_NilCancelledAt_NoParseReject(t *testing.T) {
	traceID := "trace_tv8_88_nil"
	req, release := reqWithIdentity(t, "update_milestone", traceID)
	defer release()

	in := updateMilestoneIn{
		MilestoneID: "mst_01J0000000000000000000000",
		CancelledAt: nil,
	}
	_, _, err := handleUpdateMilestone(context.Background(), req, in)
	if err == nil {
		return
	}
	je, ok := err.(*sdkjsonrpc.Error)
	if !ok {
		return
	}
	var payload struct {
		Details struct {
			Field string `json:"field"`
		} `json:"details"`
	}
	_ = json.Unmarshal(je.Data, &payload)
	if payload.Details.Field == "cancelled_at" {
		t.Fatalf("an omitted cancelled_at was rejected at the parse boundary; nil must mean unchanged")
	}
}
