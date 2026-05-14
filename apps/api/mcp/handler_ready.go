// handler_ready.go owns the §6.2 Tool 2 (`ready`) handler — returns
// the ready queue ordered by (priority asc, created_at asc, id asc).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 2 (lines
// 1177-1206) + § 7 (error envelope).
//
// Wraps workitems.Ready which serves the partial index extended
// under migration 0100 (D-2 orchestrator DECISION decision #2). No
// pagination — v1.0 caller filters with priority_min / project_id
// to keep the page small.

package mcp

import (
	"context"

	"encore.app/workitems"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// readyLimitMaxTool2 mirrors SPEC §6.2 Tool 2 line 1183 — 1..200
// in the JSON contract, BUT the spec narrative caps at 50 via
// items_ready_partial_idx + the practical ready-set sizing comment.
// We pin the wire cap at 50 for parity with workitems.Ready's
// readyMaxLimit; a forward-compat amendment can lift it without a
// schema change. Spec line 1183 says "1..200; default 10" — accept
// the wider range here and pin to 50 downstream so the contract is
// permissive on the input side (no VALIDATION on a 100-limit caller)
// while the implementation stays at the index-friendly cap.
//
// (Tool 1 / prime constrains its own ready_limit to 1..50 directly
// per spec line 1163 — that is the canonical 50-cap surface.)
const readyLimitMaxTool2 = 200

type readyIn struct {
	ProjectID   string `json:"project_id,omitempty"`
	Limit       int    `json:"limit,omitempty"`
	PriorityMin string `json:"priority_min,omitempty"`
}

type readyOut struct {
	Items      []primeItem `json:"items"`
	TotalReady int         `json:"total_ready"`
}

// registerHandleReady is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleReady(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "ready",
		Description: "Items currently Ready for any agent to claim, " +
			"ordered by (priority asc, created_at asc, id asc). " +
			"SPEC § 6.2 Tool 2.",
	}, handleReady)
}

func handleReady(ctx context.Context, req *sdkmcp.CallToolRequest, in readyIn) (*sdkmcp.CallToolResult, readyOut, error) {
	tool := "ready"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, readyOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, readyOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil && in.ProjectID != "" {
		state.Call.ProjectID = in.ProjectID
	}

	limit := in.Limit
	if limit < 0 {
		limit = 0 // workitems.Ready will apply its own default
	}
	if limit > readyLimitMaxTool2 {
		limit = readyLimitMaxTool2
	}

	resp, err := workitems.Ready(mcpCtx, &workitems.ReadyRequest{
		OrgID:       identity.OrgID,
		ProjectID:   in.ProjectID,
		Limit:       limit,
		PriorityMin: in.PriorityMin,
	})
	if err != nil {
		return nil, readyOut{}, mapError(state, tool, err)
	}

	out := readyOut{
		Items:      itemsToPrime(resp.Items),
		TotalReady: resp.TotalReady,
	}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
