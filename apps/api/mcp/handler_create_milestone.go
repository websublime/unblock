// handler_create_milestone.go owns the §6.2 Tool 16 (`create_milestone`)
// handler — a thin MCP facade over workitems.CreateMilestone (§4.4.1).
// round-16, bead unblock-tv8.74.
//
// Scope (org_id XOR project_id) is resolved server-side from the
// Bearer-resolved Identity, NOT a client-supplied org_id: when the
// caller passes project_id the milestone is project-scoped; otherwise
// it is org-scoped using identity.OrgID. This matches the rest of the
// write surface (handler_create pins OrgID=identity.OrgID; handler_prime
// / handler_ready dropped the request-side org_id in rework S1 to close
// confused-deputy seams). The backing workitems write RPC does not
// self-gate — the MCP handler is the authoritative write gate
// (workitems.go auth-model doc-comment).
//
// Invariant violations (M-INV-2/3/5/6) surface from the backing RPC as
// FailedPrecondition with Meta["invariant"]; mapError projects them into
// the §7 PRECONDITION_NOT_MET envelope with data.invariant. Scope / date
// CHECK failures surface as §7 VALIDATION.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 16 + § 4.4.1
// (workitems.CreateMilestone) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// createMilestoneIn is the JSON wire shape for create_milestone.
// org_id is NOT carried on the wire — org scope is pinned to
// identity.OrgID (see file doc-comment). project_id selects
// project-scoping; its absence selects org-scoping.
type createMilestoneIn struct {
	ProjectID         string `json:"project_id,omitempty"`
	ParentMilestoneID string `json:"parent_milestone_id,omitempty"`
	Name              string `json:"name"`
	Description       string `json:"description,omitempty"`
	StartDate         string `json:"start_date"`
	EndDate           string `json:"end_date"`
}

type createMilestoneOut struct {
	Milestone milestoneWire `json:"milestone"`
}

// registerHandleCreateMilestone is invoked by transport.go's init — see
// the toolRegistrars rationale there.
func registerHandleCreateMilestone(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "create_milestone",
		Description: "Create a milestone scoped to the caller's org or to a " +
			"project (pass project_id to project-scope; omit it to org-scope). " +
			"Optional parent_milestone_id nests the milestone (depth ≤ 4); the " +
			"child date range must be within the parent's. Invariant violations " +
			"(M-INV-2/3/5/6) reject with PRECONDITION_NOT_MET carrying " +
			"data.invariant. SPEC § 6.2 Tool 16.",
	}, handleCreateMilestone)
}

func handleCreateMilestone(ctx context.Context, req *sdkmcp.CallToolRequest, in createMilestoneIn) (*sdkmcp.CallToolResult, createMilestoneOut, error) {
	tool := "create_milestone"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, createMilestoneOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, createMilestoneOut{}, mapError(state, tool, err)
	}

	// Scope resolution: project_id ⇒ project-scoped; otherwise org-scoped
	// on the caller's identity.OrgID. Exactly one of (OrgID, ProjectID) is
	// passed to the backing RPC, satisfying the XOR the RPC + DDL CHECK
	// enforce. A client cannot name a foreign org because OrgID is never
	// read from the wire.
	// CallerOrgID is pinned to identity.OrgID (never the wire) so the backing
	// RPC's parent-read seam gates a foreign parent_milestone_id as NOT_FOUND
	// rather than leaking a cross-tenant parent's scope/dates (§10.1.1).
	scope := workitems.CreateMilestoneRequest{
		CallerOrgID:       identity.OrgID,
		ParentMilestoneID: in.ParentMilestoneID,
		Name:              in.Name,
		Description:       in.Description,
		StartDate:         in.StartDate,
		EndDate:           in.EndDate,
	}
	if in.ProjectID != "" {
		scope.ProjectID = in.ProjectID
		if state != nil && state.Call != nil {
			state.Call.ProjectID = in.ProjectID
		}
	} else {
		scope.OrgID = identity.OrgID
	}

	ms, err := workitems.CreateMilestone(mcpCtx, &scope)
	if err != nil {
		return nil, createMilestoneOut{}, mapError(state, tool, err)
	}

	out := createMilestoneOut{Milestone: milestoneToWire(*ms)}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		if ms.ProjectID != "" {
			state.Call.ProjectID = ms.ProjectID
		}
	}
	return nil, out, nil
}
