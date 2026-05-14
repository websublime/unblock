// handler_list.go owns the §6.2 Tool 8 (`list`) handler — the
// general-purpose paginated read across workitems.items.
//
// Wraps workitems.List which orders by `id ASC` only (SPEC §4.4) and
// emits a NextCursor on `id > $1` keyset semantics. The MCP layer
// wraps that bare ULID in the §6.2.0 opaque, HMAC-signed cursor
// envelope (cursor.go::listCursor with V="l1").
//
// Filter semantics:
//
//   - project_id, milestone_id, claimed_by: scalar equality
//   - status[], pipeline_stage[]: validated enum-array filter
//     (workitems.List rejects unknown members with VALIDATION)
//   - labels[]: post-fetch intersection — see "labels filter note"
//     below.
//
// Labels filter note (intentional, per round-7 §6.2 + workitems.List
// implementation): labels[] is applied AFTER the keyset window is
// materialised because labels live in workitems.item_labels (junction
// table). A page with limit+1 rows can therefore return FEWER than
// `limit` items when labels[] narrows the set — but the cursor still
// advances past the last fetched row. `next_cursor` signals "more rows
// past the anchor" (which is the spec's pagination contract), NOT
// "more rows matching the labels filter". A caller hunting for a
// specific labelset may need to consume multiple cursored pages to
// see all matches. Do NOT "fix" this by lifting the labels filter
// into SQL — the spec is silent on per-page completeness and the
// junction-table join would defeat the items_ready_partial_idx
// keyset scan that Tool 2 / Tool 8 share for the hot path.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 8 (lines
// 1381-1398) + § 6.2.0 (cursor contract) + § 4.4 (workitems.List).

package mcp

import (
	"context"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// listLimitDefault / listLimitMax mirror SPEC §6.2 Tool 8 line 1389
// (1..200; default 50). Aligns with workitems.listDefaultLimit /
// listMaxLimit so the wire ceiling matches the downstream RPC.
// Two local consts keep this handler's behaviour readable without
// chasing constants across package boundaries.
const (
	listLimitDefault = 50
	listLimitMax     = 200
)

type listIn struct {
	ProjectID     string   `json:"project_id,omitempty"`
	MilestoneID   string   `json:"milestone_id,omitempty"`
	Status        []string `json:"status,omitempty"`
	PipelineStage []string `json:"pipeline_stage,omitempty"`
	ClaimedBy     string   `json:"claimed_by,omitempty"`
	Labels        []string `json:"labels,omitempty"`
	Limit         int      `json:"limit,omitempty"`
	Cursor        string   `json:"cursor,omitempty"`
}

type listOut struct {
	Items []primeItem `json:"items"`
	// NextCursor follows the same round-2 W1 contract as readyOut /
	// searchOut: *string with NO `omitempty` so the final page emits
	// an explicit `"next_cursor": null` rather than omitting the field.
	NextCursor *string `json:"next_cursor"`
}

// registerHandleList is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleList(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "list",
		Description: "Paginate workitems with scalar + array filters " +
			"(project_id, milestone_id, status, pipeline_stage, " +
			"claimed_by, labels). Ordered by id ASC. SPEC § 6.2 Tool 8.",
	}, handleList)
}

func handleList(ctx context.Context, req *sdkmcp.CallToolRequest, in listIn) (*sdkmcp.CallToolResult, listOut, error) {
	tool := "list"
	state := bindTool(req, tool)

	if _, ok := identityFromReq(req); !ok {
		return nil, listOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, listOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil && in.ProjectID != "" {
		state.Call.ProjectID = in.ProjectID
	}

	// Limit bounds — same shape as handler_ready.go (rework S4): explicit
	// default coercion + explicit range rejection. limit=0/negative
	// coerces to listLimitDefault (50); limit > listLimitMax is a
	// contract violation surfaced as VALIDATION.
	if in.Limit <= 0 {
		in.Limit = listLimitDefault
	}
	if in.Limit > listLimitMax {
		return nil, listOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "limit out of range (1..200)",
			Meta:    errs.Metadata{"field": "limit"},
		})
	}

	// §6.2.0 cursor decode — the opaque token is verified and the {id}
	// tuple unpacked into ListRequest.Cursor (workitems.List accepts a
	// plain ULID for its `id > $1` keyset predicate). An empty cursor
	// is the first-page signal; cursorVersionList="l1" rejects replays
	// from other tools (Tool 2 / Tool 9) at the decoder boundary.
	var c listCursor
	if in.Cursor != "" {
		if err := decodeCursor(in.Cursor, cursorVersionList, &c); err != nil {
			return nil, listOut{}, mapError(state, tool, err)
		}
	}

	rpcReq := &workitems.ListRequest{
		ProjectID:     in.ProjectID,
		MilestoneID:   in.MilestoneID,
		Status:        in.Status,
		PipelineStage: in.PipelineStage,
		ClaimedBy:     in.ClaimedBy,
		Labels:        in.Labels,
		Limit:         in.Limit,
	}
	if in.Cursor != "" {
		rpcReq.Cursor = c.ID
	}

	resp, err := workitems.List(mcpCtx, rpcReq)
	if err != nil {
		return nil, listOut{}, mapError(state, tool, err)
	}

	// Encode next_cursor only when the underlying RPC produced one.
	// Empty NextCursor means end-of-stream and the *string stays nil
	// so it marshals to literal JSON `null`.
	var nextCursor *string
	if resp.NextCursor != "" {
		tok, err := encodeCursor(listCursor{
			V:  cursorVersionList,
			ID: resp.NextCursor,
		})
		if err != nil {
			return nil, listOut{}, mapError(state, tool, &errs.Error{
				Code:    errs.Internal,
				Message: "cursor encode failed",
			})
		}
		nextCursor = &tok
	}

	out := listOut{
		Items:      itemsToPrime(resp.Items),
		NextCursor: nextCursor,
	}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}
