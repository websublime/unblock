// handler_remove_dependency.go owns the §6.2 Tool 12
// (`remove_dependency`) handler — a thin wire+identity-bridge+errmap
// wrapper around the private deps.RemoveEdge RPC.
//
// All business logic — the EdgeID XOR (FromItem+ToItem+Kind) selection
// gate, edge_id minting BEFORE BEGIN so the inline audit row and the
// post-commit publish share the SAME event_id (round-6 tension #1),
// inline DELETE + inline is_ready recompute on the direct to_item
// (Regime A — single-hop), inline deps.cascade_events row with
// kind='edge_removed', and the post-commit
// CascadeRequested{Reason:"edge_removed"} publish reusing the same
// event_id so the subscriber's ON CONFLICT (event_id,
// triggered_by_item_id) DO NOTHING collapses to no-op — all live in
// deps.RemoveEdge. This handler MUST NOT duplicate any of it.
//
// Wire shape selection: SPEC §6.2 Tool 12 line 1528 declares the
// arguments as edge_id OR (from_item_id + to_item_id + kind). The
// JSON-schema description carries the XOR rule; runtime enforcement
// is owned by deps.RemoveEdge (deps/deps.go:443-457) which surfaces
// the XOR violation as InvalidArgument → §7 VALIDATION via errmap.
// The handler accepts both shapes verbatim and forwards.
//
// Audit row convention: the handler stamps state.Call.ItemID =
// resp.ToItemID and state.Call.ProjectID = resp.ProjectID after the
// deps call succeeds (deps.RemoveEdgeResponse carries both the resolved
// to_item identifier and its project regardless of which selection
// shape the caller used). The to_item is the natural single-item
// handle — it is the only item whose is_ready flag is mutated inline.
// RemoveEdge resolves project_id from the to_item row it already locks
// (deps/deps.go), so surfacing it on the response costs no extra
// round-trip and mirrors the add_dependency symmetry (review cleanup
// unblock-tv8.62).
//
// to_item_now_ready is the SINGLE-HOP view per SPEC §6.2 Tool 12
// lines 1578-1586: the boolean reflects ONLY the direct to_item's
// post-DELETE is_ready value. Transitive pipeline_stage recompute
// on downstream items is eventually consistent (driven by the
// post-commit publish — Regime B).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 12 (lines
// 1523-1604) + § 6.3.0 (symmetric writer model) + § 6.5 (cycle CTE
// shared with AddEdge — reused by recomputeReady) + § 7 (error
// envelope).

package mcp

import (
	"context"

	"encore.app/deps"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

type removeDependencyIn struct {
	EdgeID     string `json:"edge_id,omitempty"`
	FromItemID string `json:"from_item_id,omitempty"`
	ToItemID   string `json:"to_item_id,omitempty"`
	Kind       string `json:"kind,omitempty"`
}

type removeDependencyOut struct {
	Removed        bool `json:"removed"`
	ToItemNowReady bool `json:"to_item_now_ready"`
}

// registerHandleRemoveDependency is invoked by transport.go's init —
// see the toolRegistrars rationale there.
func registerHandleRemoveDependency(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "remove_dependency",
		Description: "Delete a dependency edge. Accepts edge_id OR the " +
			"composite (from_item_id, to_item_id, kind) — exactly one " +
			"shape. Recomputes is_ready inline for the direct to_item " +
			"(single-hop) and publishes CascadeRequested{Reason:" +
			"\"edge_removed\"} post-commit for the multi-hop " +
			"pipeline_stage recompute. SPEC § 6.2 Tool 12.",
	}, handleRemoveDependency)
}

func handleRemoveDependency(ctx context.Context, req *sdkmcp.CallToolRequest, in removeDependencyIn) (*sdkmcp.CallToolResult, removeDependencyOut, error) {
	tool := "remove_dependency"
	state := bindTool(req, tool)

	if _, ok := identityFromReq(req); !ok {
		return nil, removeDependencyOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, removeDependencyOut{}, mapError(state, tool, err)
	}

	// XOR selection enforcement is owned by deps.RemoveEdge
	// (deps/deps.go:443-457) — passing both / neither surfaces
	// InvalidArgument with no Meta.field, which errmap maps to §7
	// VALIDATION with details.reason carrying the deps-layer message.
	// We forward the inputs verbatim.
	resp, err := deps.RemoveEdge(mcpCtx, &deps.RemoveEdgeRequest{
		EdgeID:   in.EdgeID,
		FromItem: in.FromItemID,
		ToItem:   in.ToItemID,
		Kind:     in.Kind,
	})
	if err != nil {
		return nil, removeDependencyOut{}, mapError(state, tool, err)
	}

	out := removeDependencyOut{
		Removed:        resp.Removed,
		ToItemNowReady: resp.ToItemNowReady,
	}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		state.Call.ItemID = resp.ToItemID
		state.Call.ProjectID = resp.ProjectID
	}
	return nil, out, nil
}
