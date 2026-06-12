// handler_close.go owns the §6.2 Tool 6 (`close`) handler — wraps
// workitems.Close which flips status=Done, runs the inline Regime-A
// is_ready recompute on direct blocks downstream, and publishes
// CascadeRequested{Reason:"close"} post-commit for the multi-hop
// pipeline_stage recompute (Regime B).
//
// AF3 precondition path: when claimed_by_id IS NULL on the locked row,
// workitems.Close returns FailedPrecondition with
// Meta["invariant"]="claimed_by_id_required" AND
// Meta["missing"]="claimed_by_id" (see workitems.go's
// preconditionErrorMissing helper). The errmap's classifyEnvelopeError
// projects those into the §7 PRECONDITION_NOT_MET envelope:
// details.rejection_reason="claimed_by_id_required" +
// details.missing="claimed_by_id". The bead AC ("close with
// claimed_by_id IS NULL returns PRECONDITION_NOT_MET with
// data.missing=claimed_by_id") is satisfied at the workitems-error-
// shape layer; this handler only translates the resulting *errs.Error
// to the wire envelope.
//
// Cascade event publication is the responsibility of workitems.Close
// (post-commit publish on deps.cascade.requested with Reason="close").
// The subscriber lands a deps.cascade_events row with kind='close';
// the handler does not observe that side-effect synchronously.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 6 (lines 1322-1361)
// + § 6.3.0 / § 6.3.2 (cascade regimes) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

type closeIn struct {
	ItemID string `json:"item_id"`
	Reason string `json:"reason,omitempty"`
}

type closeOut struct {
	Item primeItem `json:"item"`
}

// registerHandleClose is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleClose(s *sdkmcp.Server) {
	registerValidatedTool(s, "close",
		"Flip an item to Done. Rejects with PRECONDITION_NOT_MET "+
			"(data.missing=claimed_by_id) when the item is not currently "+
			"claimed. Emits CascadeRequested{Reason:\"close\"} post-commit. "+
			"SPEC § 6.2 Tool 6.",
		nil, handleClose)
}

func handleClose(ctx context.Context, req *sdkmcp.CallToolRequest, in closeIn) (*sdkmcp.CallToolResult, closeOut, error) {
	tool := "close"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, closeOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, closeOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	// CallerOrgID is pinned to identity.OrgID (never the wire) so the backing
	// RPC's row-level tenant predicate rejects a foreign item_id as NOT_FOUND
	// rather than acting cross-tenant (§10.1.1).
	item, err := workitems.Close(mcpCtx, &workitems.CloseRequest{
		ItemID:      in.ItemID,
		CallerOrgID: identity.OrgID,
		Reason:      in.Reason,
	})
	if err != nil {
		return nil, closeOut{}, mapError(state, tool, err)
	}

	out := closeOut{Item: itemToPrime(*item)}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		state.Call.ProjectID = item.ProjectID
	}
	return nil, out, nil
}
