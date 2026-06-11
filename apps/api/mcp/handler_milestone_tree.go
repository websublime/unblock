// handler_milestone_tree.go owns the §6.2 Tool 19 (`milestone_tree`)
// handler — a thin MCP facade over workitems.MilestoneTree (§4.4.1).
// round-16, bead unblock-tv8.74.
//
// Returns the recursive milestone tree (depth bounded at M-INV-6 = 4),
// either rooted at root_milestone_id OR walking all roots within the
// caller's scope. Read-side org scope is pinned to the Bearer-resolved
// Identity: org_id is NOT a client-supplied wire field. When
// root_milestone_id is empty the walk is scoped to identity.OrgID
// (optionally narrowed to project_id when supplied); when
// root_milestone_id is set the scope is derived from that milestone by
// the backing RPC.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 19 + § 4.4.1
// (workitems.MilestoneTree) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	"github.com/google/jsonschema-go/jsonschema"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// milestoneTreeIn mirrors the addressable fields of
// workitems.MilestoneTreeRequest. org_id is NOT carried on the wire —
// the roots walk is pinned to identity.OrgID (see file doc-comment).
// project_id optionally narrows the roots walk; root_milestone_id
// selects a rooted walk instead.
type milestoneTreeIn struct {
	ProjectID        string `json:"project_id,omitempty"`
	RootMilestoneID  string `json:"root_milestone_id,omitempty"`
	IncludeCancelled bool   `json:"include_cancelled,omitempty"`
}

type milestoneTreeOut struct {
	Roots []milestoneNodeWire `json:"roots"`
}

// milestoneTreeOutputSchema is the explicit output schema for
// milestone_tree. It is supplied to AddTool rather than left to
// reflection because milestoneNodeWire is self-referential
// (children []milestoneNodeWire) and the SDK's jsonschema-go inference
// rejects recursive Go types with "cycle detected". A hand-built schema
// expresses the recursion via a $defs/MilestoneNode self-$ref, which the
// SDK resolves cleanly. The shape mirrors the catalogue.json
// $shared/MilestoneNode + $shared/Milestone definitions verbatim.
var milestoneTreeOutputSchema = &jsonschema.Schema{
	Type:     "object",
	Required: []string{"roots"},
	Properties: map[string]*jsonschema.Schema{
		"roots": {
			Type:  "array",
			Items: &jsonschema.Schema{Ref: "#/$defs/MilestoneNode"},
		},
	},
	Defs: map[string]*jsonschema.Schema{
		"Milestone": {
			Type:     "object",
			Required: []string{"id", "name", "start_date", "end_date", "cancelled_at", "created_at", "updated_at"},
			Properties: map[string]*jsonschema.Schema{
				"id":                  {Type: "string"},
				"parent_milestone_id": {Type: "string"},
				"org_id":              {Type: "string"},
				"project_id":          {Type: "string"},
				"name":                {Type: "string"},
				"description":         {Type: "string"},
				"start_date":          {Type: "string"},
				"end_date":            {Type: "string"},
				"cancelled_at":        {Types: []string{"string", "null"}},
				"cancelled_reason":    {Type: "string"},
				"created_at":          {Type: "string"},
				"updated_at":          {Type: "string"},
			},
		},
		"MilestoneNode": {
			Type:     "object",
			Required: []string{"milestone", "depth", "children"},
			Properties: map[string]*jsonschema.Schema{
				"milestone": {Ref: "#/$defs/Milestone"},
				"depth":     {Type: "integer"},
				"children": {
					Type:  "array",
					Items: &jsonschema.Schema{Ref: "#/$defs/MilestoneNode"},
				},
			},
		},
	},
}

// registerHandleMilestoneTree is invoked by transport.go's init — see
// the toolRegistrars rationale there.
func registerHandleMilestoneTree(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "milestone_tree",
		Description: "Return the milestone tree for the caller's org (or a " +
			"project, via project_id), or the subtree rooted at " +
			"root_milestone_id. Depth is bounded at 4. Pass " +
			"include_cancelled=true to include cancelled milestones. " +
			"SPEC § 6.2 Tool 19.",
		OutputSchema: milestoneTreeOutputSchema,
	}, handleMilestoneTree)
}

func handleMilestoneTree(ctx context.Context, req *sdkmcp.CallToolRequest, in milestoneTreeIn) (*sdkmcp.CallToolResult, milestoneTreeOut, error) {
	tool := "milestone_tree"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, milestoneTreeOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, milestoneTreeOut{}, mapError(state, tool, err)
	}

	treeReq := workitems.MilestoneTreeRequest{
		ProjectID:        in.ProjectID,
		RootMilestoneID:  in.RootMilestoneID,
		IncludeCancelled: in.IncludeCancelled,
	}
	// Roots walk (root_milestone_id empty): pin org scope to the caller's
	// identity.OrgID so a client cannot enumerate a foreign org's roots.
	// project_id, when supplied, narrows the walk within that org. The
	// rooted walk (root_milestone_id set) derives its scope from the root
	// milestone inside the RPC, so no org_id is needed there.
	if in.RootMilestoneID == "" {
		treeReq.OrgID = identity.OrgID
	}
	if in.ProjectID != "" && state != nil && state.Call != nil {
		state.Call.ProjectID = in.ProjectID
	}

	resp, err := workitems.MilestoneTree(mcpCtx, &treeReq)
	if err != nil {
		return nil, milestoneTreeOut{}, mapError(state, tool, err)
	}

	roots := make([]milestoneNodeWire, 0, len(resp.Roots))
	for _, r := range resp.Roots {
		roots = append(roots, milestoneNodeToWire(r))
	}

	out := milestoneTreeOut{Roots: roots}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
