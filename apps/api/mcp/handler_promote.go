// handler_promote.go owns the §6.2 Tool 15 (`promote`) handler — the
// Backlog→Ready transition backed by workitems.Promote (SPEC §6.6
// status transition map). round-16, bead unblock-tv8.71.
//
// promote is the canonical Ready writer that round-12 DRIFT-2 observed
// was missing: before it, nothing moved an item into Ready via RPC, so
// the ready queue and claim (both requiring status='Ready') were inert
// for any item created through the create tool.
//
// Success path: structuredContent = { item: Item } with status='Ready'.
// Rejection paths (mapError → §7 envelope):
//   - not in Backlog OR not ready → PRECONDITION_NOT_MET with the §7.2
//     {status, required} extension (and missing="is_ready" when the item
//     is Backlog but still blocked).
//   - item not found / not visible → NOT_FOUND.
//
// Side-effects: none on the cascade subsystem (§6.2 Tool 15) — promote
// publishes no CascadeRequested.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 15 + § 6.6
// (status transition map) + § 7.2 ({status, required} extension).

package mcp

import (
	"context"

	"encore.app/workitems"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

type promoteIn struct {
	ItemID string `json:"item_id"`
}

type promoteOut struct {
	Item primeItem `json:"item"`
}

// registerHandlePromote is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandlePromote(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "promote",
		Description: "Promote a Backlog item to Ready so it enters the ready " +
			"queue and becomes claimable. Precondition: status='Backlog' AND " +
			"is_ready=true (no open blockers). A still-blocked or wrong-status " +
			"item is rejected with PRECONDITION_NOT_MET carrying {status, " +
			"required}. SPEC § 6.2 Tool 15 + § 6.6.",
	}, handlePromote)
}

func handlePromote(ctx context.Context, req *sdkmcp.CallToolRequest, in promoteIn) (*sdkmcp.CallToolResult, promoteOut, error) {
	tool := "promote"
	state := bindTool(req, tool)

	if _, ok := identityFromReq(req); !ok {
		return nil, promoteOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, promoteOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	item, err := workitems.Promote(mcpCtx, &workitems.PromoteRequest{
		ItemID: in.ItemID,
	})
	if err != nil {
		return nil, promoteOut{}, mapError(state, tool, err)
	}

	out := promoteOut{Item: itemToPrime(*item)}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		state.Call.ProjectID = item.ProjectID
	}
	return nil, out, nil
}
