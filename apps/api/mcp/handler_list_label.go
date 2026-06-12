// handler_list_label.go owns the §6.2 Tool 21 (`list_labels`) handler —
// a thin MCP facade over workitems.ListLabels (§4.4). round-16, bead
// unblock-tv8.75.
//
// Read-side org scope is pinned to the Bearer-resolved Identity: org_id is
// NOT a client-supplied wire field — it always comes from identity.OrgID.
// When project_id is supplied the backing RPC returns the project's labels
// PLUS the inherited org labels, applying PRD §6.4 "project wins on
// identical name"; otherwise it returns the caller's org labels. The
// backing RPC's org_id = $caller_org predicate is a hard tenant gate
// (matching the rbac.For read-side surface, auth-model doc-comment at
// workitems.go:28-66).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 21 + § 4.4
// (workitems.ListLabels) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// listLabelsIn is the JSON wire shape for list_labels. org_id is NOT
// carried on the wire — the read RPC scopes to identity.OrgID. project_id
// optionally narrows to a project (returning inherited org labels too).
type listLabelsIn struct {
	ProjectID string `json:"project_id,omitempty"`
}

type listLabelsOut struct {
	Labels []labelWire `json:"labels"`
}

// registerHandleListLabel is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleListLabel(s *sdkmcp.Server) {
	registerValidatedTool(s, "list_labels",
		"List labels visible to the caller within a scope. Without "+
			"project_id, returns the caller's org labels. With project_id, "+
			"returns the project's labels plus the inherited org labels "+
			"(project wins on identical name). SPEC § 6.2 Tool 21.",
		nil, handleListLabel)
}

func handleListLabel(ctx context.Context, req *sdkmcp.CallToolRequest, in listLabelsIn) (*sdkmcp.CallToolResult, listLabelsOut, error) {
	tool := "list_labels"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, listLabelsOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, listLabelsOut{}, mapError(state, tool, err)
	}

	// Org scope is always the caller's identity.OrgID — never wire-supplied.
	// project_id narrows within that org.
	listReq := workitems.ListLabelsRequest{
		OrgID:     identity.OrgID,
		ProjectID: in.ProjectID,
	}
	if in.ProjectID != "" && state != nil && state.Call != nil {
		state.Call.ProjectID = in.ProjectID
	}

	resp, err := workitems.ListLabels(mcpCtx, &listReq)
	if err != nil {
		return nil, listLabelsOut{}, mapError(state, tool, err)
	}

	labels := make([]labelWire, 0, len(resp.Labels))
	for _, l := range resp.Labels {
		labels = append(labels, labelToWire(l))
	}

	out := listLabelsOut{Labels: labels}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
