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
// The wire shape mirrors SPEC §6.2 Tool 7 (round-16 / bead
// unblock-tv8.76 — parent + dependency targets resolved to
// {id,title,status,kind} ResolvedRefs, not bare IDs):
//
//	{
//	  "item":             <Item>,
//	  "parent":           <ResolvedRef> | null,  // resolved parent; null when no parent
//	  "comments":         [Comment],
//	  "dependencies_in":  [ResolvedRef],  // FAR target of edges where to_item = item_id
//	  "dependencies_out": [ResolvedRef],  // FAR target of edges where from_item = item_id
//	  "findings":         [Item]          // children with type=finding
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

// showRef is the JSON wire shape for one resolved neighbour — the parent
// or a dependency target — surfaced as {id,title,status,kind} (SPEC §6.2
// Tool 7 lines 1819-1825, round-16 / bead unblock-tv8.76). The bare-Edge
// shape (id/from_item/to_item/created_by) is intentionally dropped: an
// agent rendering the neighbourhood needs the FAR target's identity +
// display fields, not the edge row. `kind` carries the edge kind
// ("blocks" | "related") so edge semantics survive; it is EMPTY for the
// parent ref (§4.4 line 831).
type showRef struct {
	ID     string `json:"id"`
	Title  string `json:"title"`
	Status string `json:"status"`
	Kind   string `json:"kind"`
}

type showOut struct {
	Item            primeItem     `json:"item"`
	Parent          *showRef      `json:"parent"` // pointer (NOT omitempty) so it serialises null when absent — SPEC §6.2 line 1812
	Comments        []showComment `json:"comments"`
	DependenciesIn  []showRef     `json:"dependencies_in"`
	DependenciesOut []showRef     `json:"dependencies_out"`
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
		Parent:          refToShow(trail.Parent),
		Comments:        []showComment{},
		DependenciesIn:  []showRef{},
		DependenciesOut: []showRef{},
		Findings:        []primeItem{},
	}

	if includeComments {
		out.Comments = commentsToShow(trail.Comments)
	}
	if includeDependencies {
		out.DependenciesIn = refsToShow(trail.DependenciesIn)
		out.DependenciesOut = refsToShow(trail.DependenciesOut)
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

// refsToShow converts a []workitems.ResolvedRef (the resolved dependency
// targets) into the §6.2 wire shape. Always returns a non-nil slice so
// the JSON encodes as `[]` rather than `null` on the empty case
// (round-16 / bead unblock-tv8.76).
func refsToShow(in []workitems.ResolvedRef) []showRef {
	if len(in) == 0 {
		return []showRef{}
	}
	out := make([]showRef, 0, len(in))
	for _, r := range in {
		out = append(out, showRef{
			ID:     r.ID,
			Title:  r.Title,
			Status: r.Status,
			Kind:   r.Kind,
		})
	}
	return out
}

// refToShow converts the resolved parent ResolvedRef into the §6.2 wire
// shape. Returns nil when there is no parent (or it was omitted as a
// cross-tenant neighbour) so showOut.Parent serialises as JSON `null`
// rather than an empty object — SPEC §6.2 line 1812 (round-16 / bead
// unblock-tv8.76). The parent ref's Kind is empty by design (§4.4 line
// 831) and is carried through verbatim.
func refToShow(in *workitems.ResolvedRef) *showRef {
	if in == nil {
		return nil
	}
	return &showRef{
		ID:     in.ID,
		Title:  in.Title,
		Status: in.Status,
		Kind:   in.Kind,
	}
}
