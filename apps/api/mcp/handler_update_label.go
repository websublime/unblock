// handler_update_label.go owns the §6.2 Tool 22 (`update_label`) handler
// — a thin MCP facade over workitems.UpdateLabel (§4.4). round-16, bead
// unblock-tv8.75.
//
// Renames and/or recolors an existing label. The label's scope (org_id /
// project_id) is immutable — a scope change is a delete-then-create. Org
// scoping is enforced by the Bearer-resolved Identity (the handler resolves
// the caller via withIdentityFromReq and passes identity.OrgID as
// CallerOrgID); the backing RPC applies a row-level tenant predicate (the
// targeted label's org_id = CallerOrgID OR its project_id belongs to a
// project in the caller's org) so a foreign label_id yields NOT_FOUND
// rather than a cross-tenant write (DRIFT-3b). A successful write bumps
// workitems.labels.updated_at (the column added by migration 0130, §3.2).
//
// A rename colliding with an existing label in the same scope
// (case-insensitive) surfaces from the backing RPC as AlreadyExists with
// Meta["constraint"]; mapError projects it into the §7 CONFLICT envelope.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 22 + § 4.4
// (workitems.UpdateLabel) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// updateLabelIn is the JSON wire shape for update_label. Optional fields
// are pointers so an omitted field (nil) is distinguishable from an
// explicit empty string — only supplied fields are applied.
type updateLabelIn struct {
	LabelID     string  `json:"label_id"`
	Name        *string `json:"name,omitempty"`
	Color       *string `json:"color,omitempty"`
	Description *string `json:"description,omitempty"`
}

type updateLabelOut struct {
	Label labelWire `json:"label"`
}

// registerHandleUpdateLabel is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleUpdateLabel(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "update_label",
		Description: "Rename and/or recolor an existing label. Only the " +
			"supplied fields change; the label's scope is immutable. A rename " +
			"that collides with an existing label in the same scope rejects " +
			"with CONFLICT. SPEC § 6.2 Tool 22.",
	}, handleUpdateLabel)
}

func handleUpdateLabel(ctx context.Context, req *sdkmcp.CallToolRequest, in updateLabelIn) (*sdkmcp.CallToolResult, updateLabelOut, error) {
	tool := "update_label"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, updateLabelOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, updateLabelOut{}, mapError(state, tool, err)
	}

	if in.LabelID == "" {
		return nil, updateLabelOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing label_id",
			Meta:    errs.Metadata{"field": "label_id"},
		})
	}

	// CallerOrgID is pinned to identity.OrgID (never the wire) so the backing
	// RPC's row-level tenant predicate rejects a foreign label_id as
	// NOT_FOUND rather than acting cross-tenant (DRIFT-3b).
	label, err := workitems.UpdateLabel(mcpCtx, &workitems.UpdateLabelRequest{
		LabelID:     in.LabelID,
		CallerOrgID: identity.OrgID,
		Name:        in.Name,
		Color:       in.Color,
		Description: in.Description,
	})
	if err != nil {
		return nil, updateLabelOut{}, mapError(state, tool, err)
	}

	out := updateLabelOut{Label: labelToWire(*label)}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		if label.ProjectID != "" {
			state.Call.ProjectID = label.ProjectID
		}
	}
	return nil, out, nil
}
