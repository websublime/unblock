// handler_add_dependency.go owns the §6.2 Tool 11 (`add_dependency`)
// handler — a thin wire+identity-bridge+errmap wrapper around the
// private deps.AddEdge RPC.
//
// All business logic — cycle detection (per-project advisory lock +
// depth-counter CTE per §6.5), cross-project rejection, inline
// is_ready recompute on to_item, default kind='blocks' substitution,
// and the post-commit CascadeRequested{Reason:"edge_added"} publish —
// lives in deps.AddEdgeInTx / deps.AddEdge. This handler MUST NOT
// duplicate any of it.
//
// project_id derivation at the boundary (orchestrator DECISION on
// bead unblock-tv8.20, 2026-05-18 — DRIFT-2 resolved with option (a)):
// the spec wording at §6.2 Tool 11 line 1496 says project_id is
// "looked up in workitems.items at the start of the transaction",
// while deps.AddEdge's request struct REQUIRES non-empty ProjectID at
// the validation gate. To keep tv8.11's RPC contract stable, this
// handler does an explicit workitems.Get(to_item_id) lookup, reads
// the ProjectID from the returned Item, and forwards it into
// deps.AddEdge. Cross-project rejection still fires inside
// deps.AddEdge — its own (org_id, project_id) re-derivation from the
// DB row remains the trust boundary; this handler only solves the
// non-empty validation gate.
//
// Audit row convention (orchestrator INVESTIGATION 2026-05-18): the
// handler stamps state.Call.ItemID = in.ToItemID (the blocked item is
// the natural single-item handle — the to_item's is_ready is the only
// flag mutated inline) and state.Call.ProjectID = resolved
// project_id from the workitems.Get lookup.
//
// Default Kind: SPEC §6.2 Tool 11 line 1484 sets default kind='blocks'.
// deps.AddEdgeInTx:192-195 substitutes 'blocks' when req.Kind is
// empty. We pass empty strings through verbatim — the in-tx helper is
// the single source of truth (round-2 review S1 finding, see
// handler_create.go:91-98).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 11 (lines
// 1477-1521) + § 6.5 (cycle detection CTE) + § 6.3.0 (symmetric writer
// model) + § 7 (error envelope).

package mcp

import (
	"context"
	"time"

	"encore.app/deps"
	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

type addDependencyIn struct {
	FromItemID string `json:"from_item_id"`
	ToItemID   string `json:"to_item_id"`
	Kind       string `json:"kind,omitempty"`
}

// edgeWire mirrors deps.Edge on the JSON wire. RFC3339Nano timestamps
// are the canonical MCP wire format (see itemToPrime, commentWire,
// etc.). created_by may be empty when the caller identity is not
// recorded; we keep the field with `omitempty` so the wire form is
// minimal.
type edgeWire struct {
	ID        string `json:"id"`
	FromItem  string `json:"from_item"`
	ToItem    string `json:"to_item"`
	Kind      string `json:"kind"`
	CreatedAt string `json:"created_at"`
	CreatedBy string `json:"created_by,omitempty"`
}

type addDependencyOut struct {
	Edge edgeWire `json:"edge"`
}

// registerHandleAddDependency is invoked by transport.go's init — see
// the toolRegistrars rationale there.
func registerHandleAddDependency(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "add_dependency",
		Description: "Add a dependency edge between two work items in the " +
			"same project. Cycle-detected inline (per-project advisory " +
			"lock + depth-counter CTE per §6.5). Cross-project edges are " +
			"rejected with VALIDATION. kind defaults to \"blocks\". " +
			"SPEC § 6.2 Tool 11.",
	}, handleAddDependency)
}

func handleAddDependency(ctx context.Context, req *sdkmcp.CallToolRequest, in addDependencyIn) (*sdkmcp.CallToolResult, addDependencyOut, error) {
	tool := "add_dependency"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, addDependencyOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, addDependencyOut{}, mapError(state, tool, err)
	}

	// Boundary validation: surface a clearer VALIDATION envelope than
	// the deps-layer 'missing' message. Same pattern as
	// handler_create.go:80-86 and handler_comment.go:101-107.
	if in.FromItemID == "" {
		return nil, addDependencyOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing from_item_id",
			Meta:    errs.Metadata{"field": "from_item_id"},
		})
	}
	if in.ToItemID == "" {
		return nil, addDependencyOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing to_item_id",
			Meta:    errs.Metadata{"field": "to_item_id"},
		})
	}

	// Audit-row item handle: the blocked item is the natural single-
	// item handle for the audit row (the to_item's is_ready is the
	// only flag mutated inline by deps.AddEdge). Set BEFORE the
	// deps call so a downstream failure still leaves the audit row
	// pointing at the correct item.
	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ToItemID
	}

	// project_id derivation per orchestrator DECISION 2026-05-18
	// (DRIFT-2 option (a)). workitems.Get enforces the org-scope
	// predicate via rbac.For and returns NOT_FOUND when the to_item
	// does not exist or is outside the caller's org — same envelope
	// the cross-project rejection inside deps.AddEdge would surface
	// for inter-org slips. Empty ProjectID on the returned Item is
	// rejected by deps.AddEdge with VALIDATION data.field="to_item_id"
	// — we don't need a separate check here.
	toItem, err := workitems.Get(mcpCtx, in.ToItemID)
	if err != nil {
		// workitems.Get returns NotFound with no Meta — adapt the
		// envelope to NOT_FOUND data.kind="item" / id=to_item_id so
		// the wire payload is self-describing.
		if errsErr, ok := err.(*errs.Error); ok && errsErr.Code == errs.NotFound {
			return nil, addDependencyOut{}, mapError(state, tool, &errs.Error{
				Code:    errs.NotFound,
				Message: "to_item not found",
				Meta:    errs.Metadata{"kind": "item", "id": in.ToItemID, "field": "to_item_id"},
			})
		}
		return nil, addDependencyOut{}, mapError(state, tool, err)
	}
	if state != nil && state.Call != nil {
		state.Call.ProjectID = toItem.ProjectID
	}

	edge, err := deps.AddEdge(mcpCtx, &deps.AddEdgeRequest{
		OrgID:     identity.OrgID,
		ProjectID: toItem.ProjectID,
		FromItem:  in.FromItemID,
		ToItem:    in.ToItemID,
		Kind:      in.Kind, // pass empty through; deps.AddEdgeInTx owns the "blocks" default.
	})
	if err != nil {
		return nil, addDependencyOut{}, mapError(state, tool, err)
	}

	out := addDependencyOut{Edge: edgeWire{
		ID:        edge.ID,
		FromItem:  edge.FromItem,
		ToItem:    edge.ToItem,
		Kind:      edge.Kind,
		CreatedAt: edge.CreatedAt.UTC().Format(time.RFC3339Nano),
		CreatedBy: edge.CreatedBy,
	}}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
