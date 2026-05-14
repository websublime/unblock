// handler_ready.go owns the §6.2 Tool 2 (`ready`) handler — returns
// the ready queue ordered by (priority asc, created_at asc, id asc),
// with §6.2.0 cursor keyset pagination.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 2 (lines
// 1177-1212 post-round-7) + § 6.2.0 (cursor contract) + § 7 (error
// envelope).
//
// Wraps workitems.Ready which serves the partial index extended
// under migration 0100. After round-7 the wire `limit` accepts the
// full spec range 1..200 (no silent truncation downstream); cursors
// are HMAC-signed opaque tokens minted by mcp/cursor.go.

package mcp

import (
	"context"
	"time"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// readyLimitDefault / readyLimitMax mirror SPEC §6.2 Tool 2 line 1183
// (1..200; default 10). After round-7 the rework removed the
// downstream 50-cap (Linus REVIEW S2) so the wire range is honoured
// end-to-end without silent truncation.
const (
	readyLimitDefault = 10
	readyLimitMax     = 200
)

type readyIn struct {
	ProjectID   string `json:"project_id,omitempty"`
	Limit       int    `json:"limit,omitempty"`
	PriorityMin string `json:"priority_min,omitempty"`
	Cursor      string `json:"cursor,omitempty"`
}

type readyOut struct {
	Items      []primeItem `json:"items"`
	TotalReady int         `json:"total_ready"`
	// NextCursor is an opaque, server-signed token (§6.2.0). The wire
	// contract per SPEC §6.2.0 line 1150 and §6.2 Tool 2 line 1231 is
	// "string OR null" — clients MUST be able to distinguish "more
	// pages" from "end-of-stream" without inspecting the string value.
	//
	// Round-2 review rework W1 (Linus): a pointer-to-string lets us
	// marshal nil as JSON `null` and an encoded token as a JSON string,
	// matching the spec literally. The JSON tag has NO `omitempty` so
	// the final page's response always carries an explicit
	// `"next_cursor": null` rather than omitting the field — strict
	// schema validators on the client side would reject the missing
	// key. Same shape will be inherited by Tools 8 (list) and 9
	// (search) when D-3/D-4 land.
	NextCursor *string `json:"next_cursor"`
}

// registerHandleReady is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleReady(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "ready",
		Description: "Items currently Ready for any agent to claim, " +
			"ordered by (priority asc, created_at asc, id asc). " +
			"Paginates via opaque cursor (§6.2.0). SPEC § 6.2 Tool 2.",
	}, handleReady)
}

func handleReady(ctx context.Context, req *sdkmcp.CallToolRequest, in readyIn) (*sdkmcp.CallToolResult, readyOut, error) {
	tool := "ready"
	state := bindTool(req, tool)

	if _, ok := identityFromReq(req); !ok {
		return nil, readyOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, readyOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil && in.ProjectID != "" {
		state.Call.ProjectID = in.ProjectID
	}

	// Limit bounds per SPEC §6.2 Tool 2 line 1183 (1..200; default 10).
	// Two explicit lines that read straight off the spec (rework S4 —
	// no magic, no chained defaults). limit > 200 is a contract
	// violation and surfaces as VALIDATION; limit <= 0 coerces to the
	// spec default.
	if in.Limit <= 0 {
		in.Limit = readyLimitDefault
	}
	if in.Limit > readyLimitMax {
		return nil, readyOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "limit out of range (1..200)",
			Meta:    errs.Metadata{"field": "limit"},
		})
	}

	// §6.2.0 cursor decode. The opaque token is verified and the
	// (priority, created_at, id) tuple is unpacked into typed fields
	// on the downstream request. Empty cursor = first page.
	var c readyCursor
	if in.Cursor != "" {
		if err := decodeCursor(in.Cursor, cursorVersionReady, &c); err != nil {
			return nil, readyOut{}, mapError(state, tool, err)
		}
	}
	// OrgID is NOT carried on the request — workitems.Ready pins
	// scope to identity.OrgID via rbac.For (rework S1). The identity
	// is propagated through mcpCtx by withIdentityFromReq above.
	rpcReq := &workitems.ReadyRequest{
		ProjectID:   in.ProjectID,
		Limit:       in.Limit,
		PriorityMin: in.PriorityMin,
	}
	if in.Cursor != "" {
		rpcReq.CursorPriority = c.Priority
		rpcReq.CursorCreatedAt = time.UnixMicro(c.CreatedAtUnixUS).UTC()
		rpcReq.CursorID = c.ID
	}

	resp, err := workitems.Ready(mcpCtx, rpcReq)
	if err != nil {
		return nil, readyOut{}, mapError(state, tool, err)
	}

	// Encode next_cursor when the underlying RPC produced one
	// (i.e. when there is at least one more row past the current
	// page). On end-of-stream `nextCursor` stays nil so the field
	// marshals to JSON `null` per the §6.2.0 wire contract (round-2
	// W1).
	var nextCursor *string
	if resp.NextCursorID != "" {
		tok, err := encodeCursor(readyCursor{
			V:               cursorVersionReady,
			Priority:        resp.NextCursorPriority,
			CreatedAtUnixUS: resp.NextCursorCreatedAt.UnixMicro(),
			ID:              resp.NextCursorID,
		})
		if err != nil {
			return nil, readyOut{}, mapError(state, tool, &errs.Error{
				Code:    errs.Internal,
				Message: "cursor encode failed",
			})
		}
		nextCursor = &tok
	}

	out := readyOut{
		Items:      itemsToPrime(resp.Items),
		TotalReady: resp.TotalReady,
		NextCursor: nextCursor,
	}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
