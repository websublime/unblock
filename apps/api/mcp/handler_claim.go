// handler_claim.go owns the §6.2 Tool 3 (`claim`) handler — the
// atomic SELECT FOR UPDATE claim transaction backed by
// workitems.Claim (SPEC § 6.4).
//
// Success path: structuredContent = { claimed: true, item: Item }.
// Loser path: §7 ALREADY_CLAIMED envelope with winner_user_id,
// winner_agent, claimed_at — produced by mapError mapping
// errs.AlreadyExists + Meta.reason="already_claimed".
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 3 (lines
// 1208-1225) + § 6.4 (atomic transaction) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

type claimIn struct {
	ItemID string `json:"item_id"`
}

type claimOut struct {
	Claimed bool      `json:"claimed"`
	Item    primeItem `json:"item"`
}

// registerHandleClaim is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleClaim(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "claim",
		Description: "Atomically claim a Ready item. Loser path returns " +
			"ALREADY_CLAIMED with winner_user_id, winner_agent, " +
			"claimed_at. SPEC § 6.2 Tool 3 + § 6.4.",
	}, handleClaim)
}

func handleClaim(ctx context.Context, req *sdkmcp.CallToolRequest, in claimIn) (*sdkmcp.CallToolResult, claimOut, error) {
	tool := "claim"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, claimOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, claimOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	item, err := workitems.Claim(mcpCtx, &workitems.ClaimRequest{
		ItemID:        in.ItemID,
		ClaimerUserID: identity.UserID,
		ClaimerAgent:  identity.AgentKind,
	})
	if err != nil {
		return nil, claimOut{}, mapError(state, tool, err)
	}

	out := claimOut{
		Claimed: true,
		Item:    itemToPrime(*item),
	}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		state.Call.ProjectID = item.ProjectID
	}
	return nil, out, nil
}
