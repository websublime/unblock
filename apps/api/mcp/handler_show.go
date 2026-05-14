// handler_show.go owns the §6.2 Tool 7 (`show`) handler — composes
// the item, its comments, incoming/outgoing edges, and finding
// children into a single read.
//
// Delegates to workitems.GetTrail which fan-outs the four collections
// inside the workitems service (single auth gate, single org scope).
// The include_* flags (defaults true per SPEC §6.2 Tool 7 line 1364)
// are honoured at the wire shape: the RPC always materialises the
// full Trail; the handler drops collections the caller opted out of
// so wire size is bounded by the request.
//
// The wire shape mirrors SPEC §6.2 Tool 7 lines 1370-1378:
//
//	{
//	  "item":             <Item>,
//	  "comments":         [Comment],
//	  "dependencies_in":  [Edge],   // edges where to_item = item_id
//	  "dependencies_out": [Edge],   // edges where from_item = item_id
//	  "findings":         [Item]    // children with type=finding
//	}
//
// Each collection always serialises as an array (never null) so
// downstream consumers see a stable shape; null-out behaviour for
// include_*=false would surface as a missing key here, which is a
// breaking contract change. We instead emit `[]` when the caller
// opted out — symmetric with how primeItem.Labels / similar arrays
// behave throughout the §6.2 surface.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 7 (lines
// 1361-1379) + § 4.4 (workitems.GetTrail) + § 7 (error envelope).

package mcp

import (
	"context"
	"time"

	"encore.app/deps"
	"encore.app/workitems"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// showIn mirrors SPEC §6.2 Tool 7 lines 1363-1367. All three include_*
// flags default to true; the SDK does not interpret the `omitempty` JSON
// tag for booleans (a missing field unmarshals to false), so the
// handler reads the raw arguments to distinguish "absent (default
// true)" from "explicitly false" — see handleShow's flag normalisation
// below.
//
// Implementation note: we keep the typed shape (`*bool`) so the SDK's
// auto-inferred schema reports the right types to clients; the
// handler reads the pointer for the actual value with a nil-means-
// default treatment.
type showIn struct {
	ItemID              string `json:"item_id"`
	IncludeComments     *bool  `json:"include_comments,omitempty"`
	IncludeDependencies *bool  `json:"include_dependencies,omitempty"`
	IncludeFindings     *bool  `json:"include_findings,omitempty"`
}

// showComment is the JSON wire shape for one workitems.Comment row.
// Mirrors the §4.4 Comment fields, ISO-8601 timestamps. Omits
// updated_at and parent_id when zero so empty fields don't pollute
// the wire (SPEC §6.2 Tool 7 line 1373 says "Comment[]" — the
// canonical shape lives at the workitems service layer).
type showComment struct {
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

// showEdge is the JSON wire shape for one deps.Edge row. The §4.5
// Edge struct carries created_by which we surface as an optional
// string (omitted when empty so the wire stays compact for
// system-generated edges).
type showEdge struct {
	ID        string `json:"id"`
	FromItem  string `json:"from_item"`
	ToItem    string `json:"to_item"`
	Kind      string `json:"kind"`
	CreatedAt string `json:"created_at"`
	CreatedBy string `json:"created_by,omitempty"`
}

type showOut struct {
	Item            primeItem     `json:"item"`
	Comments        []showComment `json:"comments"`
	DependenciesIn  []showEdge    `json:"dependencies_in"`
	DependenciesOut []showEdge    `json:"dependencies_out"`
	Findings        []primeItem   `json:"findings"`
}

// registerHandleShow is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleShow(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "show",
		Description: "Read an item with its comments, incoming + outgoing " +
			"dependencies, and finding children. include_comments / " +
			"include_dependencies / include_findings default to true. " +
			"SPEC § 6.2 Tool 7.",
	}, handleShow)
}

func handleShow(ctx context.Context, req *sdkmcp.CallToolRequest, in showIn) (*sdkmcp.CallToolResult, showOut, error) {
	tool := "show"
	state := bindTool(req, tool)

	if _, ok := identityFromReq(req); !ok {
		return nil, showOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, showOut{}, mapError(state, tool, err)
	}

	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	// Flag defaults: nil pointer (field absent on the wire) means TRUE
	// per SPEC §6.2 Tool 7 line 1364 ("default true"). An explicit
	// `false` on the wire opts the caller out.
	includeComments := in.IncludeComments == nil || *in.IncludeComments
	includeDependencies := in.IncludeDependencies == nil || *in.IncludeDependencies
	includeFindings := in.IncludeFindings == nil || *in.IncludeFindings

	trail, err := workitems.GetTrail(mcpCtx, &workitems.GetTrailRequest{
		ItemID: in.ItemID,
	})
	if err != nil {
		return nil, showOut{}, mapError(state, tool, err)
	}

	out := showOut{
		Item:            itemToPrime(*trail.Item),
		Comments:        []showComment{},
		DependenciesIn:  []showEdge{},
		DependenciesOut: []showEdge{},
		Findings:        []primeItem{},
	}

	if includeComments {
		out.Comments = commentsToShow(trail.Comments)
	}
	if includeDependencies {
		out.DependenciesIn = edgesToShow(trail.DependenciesIn)
		out.DependenciesOut = edgesToShow(trail.DependenciesOut)
	}
	if includeFindings {
		out.Findings = itemsToPrime(trail.Findings)
	}

	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		state.Call.ProjectID = trail.Item.ProjectID
	}
	return nil, out, nil
}

// commentsToShow converts a []workitems.Comment into the §6.2 wire
// shape. Always returns a non-nil slice so the JSON encodes as `[]`
// rather than `null` on the empty case.
func commentsToShow(in []workitems.Comment) []showComment {
	if len(in) == 0 {
		return []showComment{}
	}
	out := make([]showComment, 0, len(in))
	for _, c := range in {
		out = append(out, showComment{
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
		})
	}
	return out
}

// edgesToShow converts a []deps.Edge into the §6.2 wire shape. Always
// returns a non-nil slice.
func edgesToShow(in []deps.Edge) []showEdge {
	if len(in) == 0 {
		return []showEdge{}
	}
	out := make([]showEdge, 0, len(in))
	for _, e := range in {
		out = append(out, showEdge{
			ID:        e.ID,
			FromItem:  e.FromItem,
			ToItem:    e.ToItem,
			Kind:      e.Kind,
			CreatedAt: e.CreatedAt.UTC().Format(time.RFC3339Nano),
			CreatedBy: e.CreatedBy,
		})
	}
	return out
}
