// handler_assign_item.go owns the §6.2 Tool 18 (`assign_item`) handler —
// a thin MCP facade over workitems.AssignItem (§4.4.1). round-16, bead
// unblock-tv8.74.
//
// Assigns a work item to a milestone, or UNASSIGNS it when milestone_id
// is the empty string (clears milestone_id + milestone_assigned_at +
// milestone_assigned_by). The backing RPC returns error-only, so this
// handler synthesises the structuredContent
// { assigned, item_id, milestone_id } per §6.2 Tool 18: milestone_id
// echoes the assigned milestone on the assign path and is null on the
// unassign path.
//
// assigned_by_user is taken from the caller's resolved Identity
// (identity.UserID) — it is NOT a client-supplied argument. Org scope
// flows through the Bearer-resolved Identity (withIdentityFromReq); the
// handler pins CallerOrgID from identity.OrgID and the backing write RPC
// self-gates the target item on a row-level tenant predicate (a foreign
// item_id yields NOT_FOUND) — round-16 / bead unblock-tv8.77, §10.1.1
// (workitems.go auth-model doc-comment).
//
// M-INV-7 (milestone scope reachable in the item's project) violations
// surface from the RPC as FailedPrecondition with Meta["invariant"]=
// "M-INV-7"; mapError projects that into §7 PRECONDITION_NOT_MET with
// data.invariant. An unknown item / milestone surfaces NOT_FOUND.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 18 + § 4.4.1
// (workitems.AssignItem) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// assignItemIn mirrors the assignable fields of
// workitems.AssignItemRequest. assigned_by_user is intentionally absent
// — it is resolved server-side from identity.UserID, never the wire.
type assignItemIn struct {
	ItemID      string `json:"item_id"`
	MilestoneID string `json:"milestone_id"`
}

// assignItemOut is the synthesised structuredContent for assign_item.
// MilestoneID is a *string so the unassign path renders
// milestone_id:null (a non-pointer empty string would render "").
type assignItemOut struct {
	Assigned    bool    `json:"assigned"`
	ItemID      string  `json:"item_id"`
	MilestoneID *string `json:"milestone_id"`
}

// registerHandleAssignItem is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleAssignItem(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "assign_item",
		Description: "Assign a work item to a milestone, or unassign it by " +
			"passing milestone_id as the empty string. The actor is the " +
			"calling identity (not a wire argument). A milestone not reachable " +
			"in the item's project rejects with PRECONDITION_NOT_MET " +
			"(data.invariant=M-INV-7). SPEC § 6.2 Tool 18.",
	}, handleAssignItem)
}

func handleAssignItem(ctx context.Context, req *sdkmcp.CallToolRequest, in assignItemIn) (*sdkmcp.CallToolResult, assignItemOut, error) {
	tool := "assign_item"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, assignItemOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, assignItemOut{}, mapError(state, tool, err)
	}

	if in.ItemID == "" {
		return nil, assignItemOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing item_id",
			Meta:    errs.Metadata{"field": "item_id"},
		})
	}
	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	// assigned_by_user comes from the resolved Identity (§6.2 Tool 18),
	// never the wire. On the unassign path (MilestoneID == "") the RPC
	// ignores AssignedByUser and sets milestone_assigned_by to NULL
	// unconditionally, so passing it is harmless. CallerOrgID is pinned to
	// identity.OrgID (never the wire) so the backing RPC's row-level tenant
	// gate on the target item rejects a foreign item_id as NOT_FOUND rather
	// than acting cross-tenant (§10.1.1).
	if err := workitems.AssignItem(mcpCtx, &workitems.AssignItemRequest{
		ItemID:         in.ItemID,
		CallerOrgID:    identity.OrgID,
		MilestoneID:    in.MilestoneID,
		AssignedByUser: identity.UserID,
	}); err != nil {
		return nil, assignItemOut{}, mapError(state, tool, err)
	}

	// The RPC returns error-only; synthesise the §6.2 Tool 18 result.
	// milestone_id is null on the unassign path (empty input), else the
	// assigned milestone id.
	out := assignItemOut{
		Assigned: true,
		ItemID:   in.ItemID,
	}
	if in.MilestoneID != "" {
		mid := in.MilestoneID
		out.MilestoneID = &mid
	}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
