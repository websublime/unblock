// handler_comment.go owns the §6.2 Tool 10 (`comment`) handler —
// append-only comment insertion against workitems.AppendComment.
//
// Append-only by construction: NO update_comment or delete_comment
// tool ships in P01 (SPEC §6.2 Tool 10 line 1472). This handler's
// existence in toolRegistrars is the only path that mutates the
// workitems.comments table from the MCP wire surface.
//
// Body length contract (SPEC §6.2 Tool 10 lines 1474-1475, round-8):
// the handler enforces 1..16384 chars at the MCP boundary; the
// downstream workitems.AppendComment RPC enforces the non-empty floor
// only. The split exists so the wire contract is symmetric with the
// JSON-schema description while internal callers (e.g. workitems.Close
// which lands a "completed" comment transactionally) retain the
// looser per-RPC bound.
//
// Identity propagation: author_id = identity.UserID (always set;
// withIdentityFromReq rejects when missing), author_agent =
// identity.AgentKind (empty for human callers — but the MCP path is
// API-key-only per SPEC §4.3.2, so AgentKind is always populated
// here). The workitems.AppendComment RPC requires `author_id OR
// author_agent` to be non-empty; passing both keeps the audit
// trail explicit.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 10 (lines
// 1454-1475) + § 4.4 (workitems.AppendComment) + § 6.5 (Comment kind
// + status enums) + § 7 (error envelope).

package mcp

import (
	"context"
	"time"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// commentBodyMax is the SPEC §6.2 Tool 10 line 1463 upper bound on
// the body payload (1..16384 chars). Lower bound (non-empty) is
// enforced by workitems.AppendComment; the upper bound is enforced
// here per the round-8 boundary clarification.
const commentBodyMax = 16384

type commentIn struct {
	ItemID   string `json:"item_id"`
	ParentID string `json:"parent_id,omitempty"`
	Kind     string `json:"kind,omitempty"`
	Status   string `json:"status,omitempty"`
	Body     string `json:"body"`
}

// commentWire is the JSON wire shape for one workitems.Comment row.
// Same shape as handler_show.go's showComment — kept local here to
// avoid a layering coupling to that file (each handler owns its own
// wire types; the workitems service owns the canonical Go struct).
type commentWire struct {
	ID          string `json:"id"`
	ItemID      string `json:"item_id"`
	ParentID    string `json:"parent_id,omitempty"`
	AuthorID    string `json:"author_id,omitempty"`
	AuthorAgent string `json:"author_agent,omitempty"`
	Kind        string `json:"kind"`
	Status      string `json:"status"`
	Body        string `json:"body"`
	CreatedAt   string `json:"created_at"`
	UpdatedAt   string `json:"updated_at"`
}

type commentOut struct {
	Comment commentWire `json:"comment"`
}

// registerHandleComment is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleComment(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "comment",
		Description: "Append a comment to a work item. Append-only — no " +
			"update or delete tool ships in P01. Validates kind/status " +
			"against §6.5 enums; body length 1..16384 chars. SPEC § 6.2 " +
			"Tool 10.",
	}, handleComment)
}

func handleComment(ctx context.Context, req *sdkmcp.CallToolRequest, in commentIn) (*sdkmcp.CallToolResult, commentOut, error) {
	tool := "comment"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, commentOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, commentOut{}, mapError(state, tool, err)
	}

	if in.ItemID == "" {
		return nil, commentOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing item_id",
			Meta:    errs.Metadata{"field": "item_id"},
		})
	}
	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	// Body length contract (SPEC §6.2 Tool 10 lines 1474-1475, round-8).
	// Upper bound is enforced here at the wire boundary; the non-empty
	// floor falls through to workitems.AppendComment (which surfaces it
	// as InvalidArgument/Meta.field="body" — mapError translates to §7
	// VALIDATION data.field="body" without changes).
	if len(in.Body) > commentBodyMax {
		return nil, commentOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "body exceeds 16384 chars",
			Meta:    errs.Metadata{"field": "body"},
		})
	}

	c, err := workitems.AppendComment(mcpCtx, &workitems.AppendCommentRequest{
		ItemID:      in.ItemID,
		AuthorID:    identity.UserID,
		AuthorAgent: identity.AgentKind,
		ParentID:    in.ParentID,
		Kind:        in.Kind,
		Status:      in.Status,
		Body:        in.Body,
	})
	if err != nil {
		return nil, commentOut{}, mapError(state, tool, err)
	}

	out := commentOut{Comment: commentWire{
		ID:          c.ID,
		ItemID:      c.ItemID,
		ParentID:    c.ParentID,
		AuthorID:    c.AuthorID,
		AuthorAgent: c.AuthorAgent,
		Kind:        c.Kind,
		Status:      c.Status,
		Body:        c.Body,
		CreatedAt:   c.CreatedAt.UTC().Format(time.RFC3339Nano),
		UpdatedAt:   c.UpdatedAt.UTC().Format(time.RFC3339Nano),
	}}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
