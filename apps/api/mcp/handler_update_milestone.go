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
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 17 + § 4.4.1
// (workitems.UpdateMilestone) + § 7 (error envelope).

package mcp

import (
	"context"
	"time"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// updateMilestoneIn mirrors workitems.UpdateMilestoneRequest. Mutable
// fields are pointers (nil = unchanged); cancelled_at is a *time.Time so
// the wire carries an RFC3339 timestamp and a non-nil value sets the
// cancellation. No parent_milestone_id field — reparenting is rejected
// in P01 (§4.4.1).
type updateMilestoneIn struct {
	MilestoneID     string     `json:"milestone_id"`
	Name            *string    `json:"name,omitempty"`
	Description     *string    `json:"description,omitempty"`
	StartDate       *string    `json:"start_date,omitempty"`
	EndDate         *string    `json:"end_date,omitempty"`
	CancelledAt     *time.Time `json:"cancelled_at,omitempty"`
	CancelledReason *string    `json:"cancelled_reason,omitempty"`
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
		CancelledAt:     in.CancelledAt,
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
