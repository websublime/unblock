// handler_delete_label.go owns the §6.2 Tool 23 (`delete_label`) handler
// — a thin MCP facade over workitems.DeleteLabel (§4.4). round-16, bead
// unblock-tv8.75.
//
// Deletes a label from the registry. The workitems.item_labels junction
// rows referencing it are removed in the same transaction (the FK is ON
// DELETE CASCADE per SPEC §9.4.3) — deleting a label detaches it from every
// item without deleting the items. The structuredContent's
// detached_item_count reports how many item attachments were removed. Org
// scoping is enforced by the Bearer-resolved Identity (the handler resolves
// the caller via withIdentityFromReq and passes identity.OrgID as
// CallerOrgID); the backing RPC applies a row-level tenant predicate (the
// targeted label's org_id = CallerOrgID OR its project_id belongs to a
// project in the caller's org) so a foreign label_id yields NOT_FOUND
// rather than a cross-tenant delete (DRIFT-3b). A missing OR foreign label
// → §7 NOT_FOUND.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 23 + § 4.4
// (workitems.DeleteLabel) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// deleteLabelIn is the JSON wire shape for delete_label.
type deleteLabelIn struct {
	LabelID string `json:"label_id"`
}

type deleteLabelOut struct {
	Deleted           bool   `json:"deleted"`
	LabelID           string `json:"label_id"`
	DetachedItemCount int    `json:"detached_item_count"`
}

// registerHandleDeleteLabel is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleDeleteLabel(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "delete_label",
		Description: "Delete a label from the registry. Detaches it from every " +
			"item it was applied to (the items are not deleted); " +
			"detached_item_count reports how many attachments were removed. " +
			"A missing label rejects with NOT_FOUND. SPEC § 6.2 Tool 23.",
	}, handleDeleteLabel)
}

func handleDeleteLabel(ctx context.Context, req *sdkmcp.CallToolRequest, in deleteLabelIn) (*sdkmcp.CallToolResult, deleteLabelOut, error) {
	tool := "delete_label"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, deleteLabelOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, deleteLabelOut{}, mapError(state, tool, err)
	}

	if in.LabelID == "" {
		return nil, deleteLabelOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing label_id",
			Meta:    errs.Metadata{"field": "label_id"},
		})
	}

	// CallerOrgID is pinned to identity.OrgID (never the wire) so the backing
	// RPC's row-level tenant predicate rejects a foreign label_id as
	// NOT_FOUND rather than acting cross-tenant (DRIFT-3b).
	resp, err := workitems.DeleteLabel(mcpCtx, &workitems.DeleteLabelRequest{
		LabelID:     in.LabelID,
		CallerOrgID: identity.OrgID,
	})
	if err != nil {
		return nil, deleteLabelOut{}, mapError(state, tool, err)
	}

	out := deleteLabelOut{
		Deleted:           resp.Deleted,
		LabelID:           resp.LabelID,
		DetachedItemCount: resp.DetachedItemCount,
	}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
