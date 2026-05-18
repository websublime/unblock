// handler_get_state.go owns the §6.2 Tool 14 (`get_state`) handler —
// the read-side complement to set_state. Returns every state
// dimension materialised on the item (impl_state, review_state,
// qa_state, pipeline_state, pipeline_stage, is_ready, claimed_by_id,
// claimed_at) plus the per-kind `recent_kinds` aggregate from
// workitems.comments (one row per distinct kind, holding the most
// recent (status, comment_id, created_at) for that kind).
//
// Delegation: 100% to workitems.GetState (added in bead
// unblock-tv8.21 alongside this handler). No inline SQL — preserves
// the symmetric delegation pattern shared by every other D-2..D-5
// handler (orchestrator DECISION 2026-05-18 on bead unblock-tv8.21).
//
// Read-side org gate: workitems.GetState uses rbac.For for the item
// lookup, so a cross-org item_id surfaces as §7 NOT_FOUND at the
// wire (mapError projects errs.NotFound → envelopeKindNotFound). The
// recent_kinds query is scoped to the resolved item_id; cross-org
// leakage is impossible because the item lookup gate runs first.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 14 (lines
// 1727-1754) + § 4.4 (workitems.GetState) + § 7 (error envelope).

package mcp

import (
	"context"
	"time"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

type getStateIn struct {
	ItemID string `json:"item_id"`
}

// getStateRecentKind mirrors SPEC §6.2 Tool 14 lines 1745-1750: one
// row per distinct comment kind on the item with the most recent
// status, comment_id, created_at for that kind.
type getStateRecentKind struct {
	Kind      string `json:"kind"`
	Status    string `json:"status"`
	CommentID string `json:"comment_id"`
	CreatedAt string `json:"created_at"`
}

// getStateOut mirrors SPEC §6.2 Tool 14 lines 1735-1751. Every state
// dimension is serialised verbatim (no omitempty on the four state
// columns + pipeline_stage + is_ready — they are always present per
// the schema defaults). claimed_by_id and claimed_at are omitempty so
// an unclaimed item produces a compact wire response.
type getStateOut struct {
	ImplState     string               `json:"impl_state"`
	ReviewState   string               `json:"review_state"`
	QAState       string               `json:"qa_state"`
	PipelineState string               `json:"pipeline_state"`
	PipelineStage string               `json:"pipeline_stage,omitempty"`
	IsReady       bool                 `json:"is_ready"`
	ClaimedByID   string               `json:"claimed_by_id,omitempty"`
	ClaimedAt     string               `json:"claimed_at,omitempty"`
	RecentKinds   []getStateRecentKind `json:"recent_kinds"`
}

// registerHandleGetState is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleGetState(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "get_state",
		Description: "Read the four state dimensions + materialised " +
			"pipeline_stage + is_ready + claim columns + the per-kind " +
			"recent_kinds aggregate from workitems.comments (one row per " +
			"distinct kind, most recent first). SPEC § 6.2 Tool 14.",
	}, handleGetState)
}

func handleGetState(ctx context.Context, req *sdkmcp.CallToolRequest, in getStateIn) (*sdkmcp.CallToolResult, getStateOut, error) {
	tool := "get_state"
	state := bindTool(req, tool)

	if _, ok := identityFromReq(req); !ok {
		return nil, getStateOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, getStateOut{}, mapError(state, tool, err)
	}

	if in.ItemID == "" {
		return nil, getStateOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing item_id",
			Meta:    errs.Metadata{"field": "item_id"},
		})
	}
	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	resp, err := workitems.GetState(mcpCtx, &workitems.GetStateRequest{ItemID: in.ItemID})
	if err != nil {
		return nil, getStateOut{}, mapError(state, tool, err)
	}

	out := getStateOut{
		ImplState:     resp.ImplState,
		ReviewState:   resp.ReviewState,
		QAState:       resp.QAState,
		PipelineState: resp.PipelineState,
		PipelineStage: resp.PipelineStage,
		IsReady:       resp.IsReady,
		ClaimedByID:   resp.ClaimedByID,
		RecentKinds:   recentKindsToWire(resp.RecentKinds),
	}
	if resp.ClaimedAt != nil {
		out.ClaimedAt = resp.ClaimedAt.UTC().Format(time.RFC3339Nano)
	}

	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		// ProjectID is not directly exposed by GetState's narrow
		// response (the spec contract is the state surface, not the
		// full Item). Leaving it unset on the audit row matches the
		// SetStateColumns RPC's own behaviour for tools that don't
		// resolve project scope at the boundary — non-fatal for the
		// audit dashboard (per-tool tool_name + item_id remain the
		// load-bearing fields).
	}
	return nil, out, nil
}

// recentKindsToWire converts a workitems-layer []RecentKindRow into
// the §6.2 Tool 14 wire shape. Always returns a non-nil slice so the
// JSON encodes as `[]` rather than `null` for items with no comments.
func recentKindsToWire(in []workitems.RecentKindRow) []getStateRecentKind {
	if len(in) == 0 {
		return []getStateRecentKind{}
	}
	out := make([]getStateRecentKind, 0, len(in))
	for _, r := range in {
		out = append(out, getStateRecentKind{
			Kind:      r.Kind,
			Status:    r.Status,
			CommentID: r.CommentID,
			CreatedAt: r.CreatedAt.UTC().Format(time.RFC3339Nano),
		})
	}
	return out
}
