// handler_create.go owns the §6.2 Tool 4 (`create`) handler.
//
// Translates the JSON wire shape to workitems.CreateRequest +
// deps.Edge dependencies. The downstream RPC is now atomic across
// item+labels+edges per the orchestrator's DECISION on bead
// unblock-tv8.17 (D-2, 2026-05-14, decision #1) — see
// workitems/workitems.go::Create.
//
// Cycle check runs inline inside the workitems.Create transaction
// via deps.AddEdgeInTx; on the cycle path the entire create rolls
// back and the error returned here carries Meta.kind="CYCLE_DETECTED"
// which mapError translates to the §7 CYCLE_DETECTED envelope.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 4 (lines
// 1227-1255) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/deps"
	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// createDependencyIn is the JSON shape of one entry in
// dependencies[]. Mirrors SPEC §6.2 Tool 4 line 1242.
type createDependencyIn struct {
	BlockerItemID string `json:"blocker_item_id"`
	Kind          string `json:"kind,omitempty"`
}

type createIn struct {
	ProjectID        string               `json:"project_id"`
	ParentID         string               `json:"parent_id,omitempty"`
	DiscoveredFromID string               `json:"discovered_from_id,omitempty"`
	Type             string               `json:"type,omitempty"`
	Title            string               `json:"title"`
	Body             string               `json:"body,omitempty"`
	Priority         string               `json:"priority,omitempty"`
	MilestoneID      string               `json:"milestone_id,omitempty"`
	Labels           []string             `json:"labels,omitempty"`
	Dependencies     []createDependencyIn `json:"dependencies,omitempty"`
	Severity         string               `json:"severity,omitempty"`
	KindOfFinding    string               `json:"kind_of_finding,omitempty"`
}

type createOut struct {
	Item primeItem `json:"item"`
}

// registerHandleCreate is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleCreate(s *sdkmcp.Server) {
	registerValidatedTool(s, "create",
		"Create a new work item (epic | task | finding). "+
			"Optional dependencies[] entries are cycle-checked inline "+
			"inside the same transaction as the item insert; on any "+
			"failure the entire create is rejected. SPEC § 6.2 Tool 4.",
		nil, handleCreate)
}

func handleCreate(ctx context.Context, req *sdkmcp.CallToolRequest, in createIn) (*sdkmcp.CallToolResult, createOut, error) {
	tool := "create"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, createOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, createOut{}, mapError(state, tool, err)
	}

	if in.ProjectID == "" {
		return nil, createOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing project_id",
			Meta:    errs.Metadata{"field": "project_id"},
		})
	}
	if state != nil && state.Call != nil {
		state.Call.ProjectID = in.ProjectID
	}

	// dependencies[].blocker_item_id → deps.Edge.FromItem (the new
	// item is the to_item). Kind defaulting to "blocks" per SPEC
	// §6.2 Tool 4 line 1243 is enforced by deps.AddEdgeInTx
	// (deps/deps.go:192-195) — single source of truth. Round-2
	// review S1 (Linus): the MCP layer used to substitute "blocks"
	// itself, which duplicated the helper's logic and invited drift
	// if a future spec amendment changed the canonical default.
	// Passing `d.Kind` through verbatim — including the empty string
	// — lets the in-tx helper own the default.
	depEdges := make([]deps.Edge, 0, len(in.Dependencies))
	for _, d := range in.Dependencies {
		if d.BlockerItemID == "" {
			return nil, createOut{}, mapError(state, tool, &errs.Error{
				Code:    errs.InvalidArgument,
				Message: "dependencies[].blocker_item_id is required",
				Meta:    errs.Metadata{"field": "dependencies[].blocker_item_id"},
			})
		}
		depEdges = append(depEdges, deps.Edge{
			FromItem: d.BlockerItemID,
			Kind:     d.Kind,
		})
	}

	item, err := workitems.Create(mcpCtx, &workitems.CreateRequest{
		OrgID:            identity.OrgID,
		ProjectID:        in.ProjectID,
		ParentID:         in.ParentID,
		DiscoveredFromID: in.DiscoveredFromID,
		Type:             in.Type,
		Title:            in.Title,
		Body:             in.Body,
		Priority:         in.Priority,
		MilestoneID:      in.MilestoneID,
		Labels:           in.Labels,
		Dependencies:     depEdges,
		Severity:         in.Severity,
		KindOfFinding:    in.KindOfFinding,
	})
	if err != nil {
		return nil, createOut{}, mapError(state, tool, err)
	}

	out := createOut{Item: itemToPrime(*item)}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		state.Call.ItemID = item.ID
	}
	return nil, out, nil
}
