// handler_update_milestone.go owns the §6.2 Tool 17 (`update_milestone`)
// handler — a thin MCP facade over workitems.UpdateMilestone (§4.4.1).
// round-16, bead unblock-tv8.74.
//
// Updates name, description, start/end dates, and cancellation. Every
// mutable field is a pointer so the handler faithfully distinguishes
// "unchanged" (nil → field omitted from the wire) from "explicit value",
// matching workitems.UpdateMilestoneRequest's nil-is-unchanged
// convention. Reparenting is NOT exposed (no parent_milestone_id field)
// — it is rejected in P01 per §4.4.1 and deferred to P02.
//
// Org scope flows through the Bearer-resolved Identity (identity.OrgID
// via withIdentityFromReq); the milestone is addressed by its ULID. The
// handler pins CallerOrgID from identity.OrgID and the backing write RPC
// self-gates on a row-level tenant predicate (a foreign milestone_id
// yields NOT_FOUND) — round-16 / bead unblock-tv8.77, §10.1.1
// (workitems.go auth-model doc-comment).
//
// Invariant / validation rejections surface from the backing RPC and are
// projected by mapError into the §7 envelope (PRECONDITION_NOT_MET with
// data.invariant for M-INV violations, VALIDATION for date/scope CHECK
// failures, NOT_FOUND for an unknown milestone_id).
//
// # cancelled_at is an RFC3339 wire string, parsed at the handler boundary
// (bead unblock-tv8.88)
//
// cancelled_at is the milestone-cancellation TIMESTAMP (§6.2 Tool 17 types
// it `<ts; optional>`, distinct from start_date/end_date which are
// `<ISO date>`). It is carried on the wire as a *string and parsed here with
// time.RFC3339, NOT as a *time.Time on the input struct. The reason is the
// §7.3 data.field contract: the shared validateArgs pass (validate.go)
// advertises and accepts cancelled_at as a plain {type:string} (the catalogue
// schema), so a non-RFC3339 value passes argument-shape validation — and a
// *time.Time input field would then fail the typed json.Unmarshal INSIDE
// registerValidatedTool, hitting the generic catch-all that mis-reports
// field='arguments' / "arguments must be a JSON object". That violated §7.3
// (the envelope must name the offending argument). By keeping the field a
// *string and parsing it here, a bad cancelled_at mints a VALIDATION envelope
// with data.field='cancelled_at' and a meaningful reason, exactly as a bad
// start_date already does via the backing RPC. A valid RFC3339 timestamp is
// converted to *time.Time before the backing RPC, so the workitems contract
// (CancelledAt *time.Time, nil = unchanged) is unchanged. cancelled_at is the
// ONLY *time.Time wire field on the 23-tool surface, so this is the only site
// that needed the fix.
//
// Cancellation semantics are SET-only, matching workitems.UpdateMilestone
// (§4.4.1): cancelled_at omitted or null → nil → unchanged; a valid RFC3339
// timestamp → set the cancellation. There is no uncancel path in P01 — the
// spec types cancelled_at as a set-cancellation `<ts>`, the backing RPC's
// `CASE WHEN $6::boolean` only writes when the value is non-nil, and neither
// defines a clear-to-NULL contract. An empty-string cancelled_at is therefore
// not "uncancel"; it is simply not a valid RFC3339 timestamp and rejects with
// VALIDATION data.field='cancelled_at' (no longer mis-fielded as 'arguments').
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 17 + § 4.4.1
// (workitems.UpdateMilestone) + § 7 (error envelope) + § 7.3 (data.field).

package mcp

import (
	"context"
	"time"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// updateMilestoneIn mirrors workitems.UpdateMilestoneRequest. Mutable
// fields are pointers (nil = unchanged). cancelled_at is a *string carrying
// an RFC3339 timestamp on the wire — it is parsed to *time.Time at the
// handler boundary (handleUpdateMilestone) so a non-RFC3339 value yields a
// §7 VALIDATION envelope with data.field='cancelled_at' rather than the
// mis-fielded generic catch-all (§7.3, bead unblock-tv8.88; see the package
// doc-comment above). No parent_milestone_id field — reparenting is rejected
// in P01 (§4.4.1).
type updateMilestoneIn struct {
	MilestoneID     string  `json:"milestone_id"`
	Name            *string `json:"name,omitempty"`
	Description     *string `json:"description,omitempty"`
	StartDate       *string `json:"start_date,omitempty"`
	EndDate         *string `json:"end_date,omitempty"`
	CancelledAt     *string `json:"cancelled_at,omitempty"`
	CancelledReason *string `json:"cancelled_reason,omitempty"`
}

type updateMilestoneOut struct {
	Milestone milestoneWire `json:"milestone"`
}

// registerHandleUpdateMilestone is invoked by transport.go's init — see
// the toolRegistrars rationale there.
func registerHandleUpdateMilestone(s *sdkmcp.Server) {
	registerValidatedTool(s, "update_milestone",
		"Update a milestone's name, description, start/end "+
			"dates, or cancellation. Only the supplied fields change. "+
			"Reparenting is not supported in P01 (rejected with VALIDATION). "+
			"Date changes that violate a parent's or child's range reject "+
			"with PRECONDITION_NOT_MET. SPEC § 6.2 Tool 17.",
		nil, handleUpdateMilestone)
}

func handleUpdateMilestone(ctx context.Context, req *sdkmcp.CallToolRequest, in updateMilestoneIn) (*sdkmcp.CallToolResult, updateMilestoneOut, error) {
	tool := "update_milestone"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, updateMilestoneOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, updateMilestoneOut{}, mapError(state, tool, err)
	}

	if in.MilestoneID == "" {
		return nil, updateMilestoneOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing milestone_id",
			Meta:    errs.Metadata{"field": "milestone_id"},
		})
	}

	// Parse the RFC3339 cancelled_at wire string into *time.Time for the
	// backing RPC. nil (omitted/null) stays nil → unchanged. A non-RFC3339
	// value (incl. the empty string, a date-only "2026-06-15", or "abc")
	// mints a §7 VALIDATION envelope NAMING the offending argument
	// (data.field='cancelled_at') — never the mis-fielded generic
	// "arguments must be a JSON object" (§7.3, bead unblock-tv8.88).
	var cancelledAt *time.Time
	if in.CancelledAt != nil {
		t, perr := time.Parse(time.RFC3339, *in.CancelledAt)
		if perr != nil {
			return nil, updateMilestoneOut{}, mapError(state, tool, &errs.Error{
				Code:    errs.InvalidArgument,
				Message: "cancelled_at must be an RFC3339 timestamp",
				Meta: errs.Metadata{
					"field":  "cancelled_at",
					"reason": "cancelled_at must be an RFC3339 timestamp",
				},
			})
		}
		cancelledAt = &t
	}

	// CallerOrgID is pinned to identity.OrgID (never the wire) so the backing
	// RPC's row-level tenant predicate rejects a foreign milestone_id as
	// NOT_FOUND rather than acting cross-tenant (§10.1.1).
	ms, err := workitems.UpdateMilestone(mcpCtx, &workitems.UpdateMilestoneRequest{
		MilestoneID:     in.MilestoneID,
		CallerOrgID:     identity.OrgID,
		Name:            in.Name,
		Description:     in.Description,
		StartDate:       in.StartDate,
		EndDate:         in.EndDate,
		CancelledAt:     cancelledAt,
		CancelledReason: in.CancelledReason,
	})
	if err != nil {
		return nil, updateMilestoneOut{}, mapError(state, tool, err)
	}

	out := updateMilestoneOut{Milestone: milestoneToWire(*ms)}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		if ms.ProjectID != "" {
			state.Call.ProjectID = ms.ProjectID
		}
	}
	return nil, out, nil
}
