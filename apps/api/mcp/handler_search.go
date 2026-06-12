// handler_search.go owns the §6.2 Tool 9 (`search`) handler — full
// text search across workitems.items + workitems.comments via UNION
// ALL over items_fts_idx and comments_fts_idx (SPEC §3.4 FTS DDL +
// §4.4 Search RPC).
//
// Wraps workitems.Search which over-fetches LIMIT+1 and surfaces a
// typed `(rank, item_id, comment_id)` triple as the next-cursor
// anchor. The MCP layer encodes that triple into the opaque,
// HMAC-signed §6.2.0 cursor envelope (cursor.go::searchCursor with
// V="s1"). Empty NextCursor* anchor means end-of-stream — the *string
// stays nil so the wire emits literal `"next_cursor": null` (round-2
// W1 contract shared with handler_ready.go and handler_list.go).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 9 (lines
// 1421-1452) + § 6.2.0 (cursor contract) + § 4.4 (workitems.Search) +
// § 7 (error envelope).

package mcp

import (
	"context"
	"strings"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// searchLimitDefault is the per-tool default applied when `limit` is
// OMITTED (SPEC §6.2 Tool 9 line 1428: default 25). The advertised
// inclusive range (1..100) lives in catalogue.gen.go and is ENFORCED by
// the shared validateArgs boundary pass (§7.3.1) — a supplied
// out-of-range value rejects with VALIDATION, never clamps.
const searchLimitDefault = 25

type searchIn struct {
	ProjectID string `json:"project_id,omitempty"`
	Query     string `json:"query"`
	Limit     int    `json:"limit,omitempty"`
	Cursor    string `json:"cursor,omitempty"`
}

// searchHit is the JSON wire shape for one workitems.SearchHit row.
// Mirrors SPEC §6.2 Tool 9 lines 1434-1441. CommentID surfaces as a
// pointer-to-string so source="item" rows can emit literal
// `"comment_id": null` rather than an empty string (the spec calls
// out "<ULID|null>" on line 1438).
type searchHit struct {
	ItemID    string  `json:"item_id"`
	Source    string  `json:"source"`
	CommentID *string `json:"comment_id"`
	Rank      float64 `json:"rank"`
	Snippet   string  `json:"snippet"`
}

type searchOut struct {
	Hits []searchHit `json:"hits"`
	// NextCursor follows the round-2 W1 wire contract shared with
	// readyOut / listOut: *string with NO `omitempty` so the final
	// page emits explicit `"next_cursor": null` rather than omitting
	// the field. Per SPEC §6.2.0 + §6.2 Tool 9 line 1443.
	NextCursor *string `json:"next_cursor"`
}

// registerHandleSearch is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleSearch(s *sdkmcp.Server) {
	registerValidatedTool(s, "search",
		"Full-text search over items + comments. UNION ALL "+
			"over items_fts_idx and comments_fts_idx; ranked by "+
			"ts_rank_cd DESC; snippet via ts_headline (≤ 200 chars). "+
			"Paginates via opaque cursor (§6.2.0). SPEC § 6.2 Tool 9.",
		nil, handleSearch)
}

func handleSearch(ctx context.Context, req *sdkmcp.CallToolRequest, in searchIn) (*sdkmcp.CallToolResult, searchOut, error) {
	tool := "search"
	state := bindTool(req, tool)

	if _, ok := identityFromReq(req); !ok {
		return nil, searchOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, searchOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil && in.ProjectID != "" {
		state.Call.ProjectID = in.ProjectID
	}

	// SPEC §6.2 Tool 9 line 1427 marks `query` as required (no
	// `<optional>` marker). Reject empty/whitespace at the wire
	// boundary so the contract is symmetric with the JSON schema
	// description; workitems.Search's empty-string short-circuit
	// remains the defensive floor.
	if strings.TrimSpace(in.Query) == "" {
		return nil, searchOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "query is required",
			Meta:    errs.Metadata{"field": "query"},
		})
	}

	// §7.3.1: a SUPPLIED limit outside 1..100 was already rejected with
	// VALIDATION by the shared validateArgs boundary pass (§7.3.2). A
	// zero here can only mean the argument was OMITTED, so we apply the
	// per-tool default on omission only — no clamp, no coerce.
	if in.Limit == 0 {
		in.Limit = searchLimitDefault
	}

	// §6.2.0 cursor decode — verifies HMAC + version discriminator,
	// unpacks the (rank, item_id, comment_id) tuple. Empty cursor =
	// first-page signal; cursorVersionSearch="s1" rejects cross-tool
	// replays at the decoder boundary.
	var c searchCursor
	if in.Cursor != "" {
		if err := decodeCursor(in.Cursor, cursorVersionSearch, &c); err != nil {
			return nil, searchOut{}, mapError(state, tool, err)
		}
	}

	rpcReq := &workitems.SearchRequest{
		ProjectID: in.ProjectID,
		Query:     in.Query,
		Limit:     in.Limit,
	}
	if in.Cursor != "" {
		rpcReq.CursorRank = c.Rank
		rpcReq.CursorItemID = c.ItemID
		rpcReq.CursorCommentID = c.CommentID
	}

	resp, err := workitems.Search(mcpCtx, rpcReq)
	if err != nil {
		return nil, searchOut{}, mapError(state, tool, err)
	}

	// Encode next_cursor only when the underlying RPC produced an
	// anchor (i.e. when there is at least one more row past the
	// current page). End-of-stream → nextCursor stays nil → JSON null.
	var nextCursor *string
	if resp.NextCursorItemID != "" {
		tok, err := encodeCursor(searchCursor{
			V:         cursorVersionSearch,
			Rank:      resp.NextCursorRank,
			ItemID:    resp.NextCursorItemID,
			CommentID: resp.NextCursorCommentID,
		})
		if err != nil {
			return nil, searchOut{}, mapError(state, tool, &errs.Error{
				Code:    errs.Internal,
				Message: "cursor encode failed",
			})
		}
		nextCursor = &tok
	}

	out := searchOut{
		Hits:       hitsToWire(resp.Hits),
		NextCursor: nextCursor,
	}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
	}
	return nil, out, nil
}

// hitsToWire converts a []workitems.SearchHit into the §6.2 Tool 9
// wire shape. Always returns a non-nil slice so the JSON encodes as
// `[]` rather than `null` on the empty case. source="item" rows emit
// `"comment_id": null` (pointer-to-string nil); source="comment" rows
// emit the ULID literally.
func hitsToWire(in []workitems.SearchHit) []searchHit {
	if len(in) == 0 {
		return []searchHit{}
	}
	out := make([]searchHit, 0, len(in))
	for _, h := range in {
		row := searchHit{
			ItemID:  h.ItemID,
			Source:  h.Source,
			Rank:    h.Rank,
			Snippet: h.Snippet,
		}
		if h.CommentID != "" {
			cid := h.CommentID
			row.CommentID = &cid
		}
		out = append(out, row)
	}
	return out
}
