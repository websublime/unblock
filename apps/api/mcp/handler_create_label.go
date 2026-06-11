// handler_create_label.go owns the §6.2 Tool 20 (`create_label`) handler
// — a thin MCP facade over workitems.CreateLabel (§4.4). round-16, bead
// unblock-tv8.75.
//
// Scope (org_id XOR project_id) is resolved server-side from the
// Bearer-resolved Identity, NOT a client-supplied org_id: when the caller
// passes project_id the label is project-scoped; otherwise it is
// org-scoped using identity.OrgID. This matches the rest of the write
// surface (handler_create_milestone pins OrgID=identity.OrgID). The
// handler ALWAYS pins CallerOrgID=identity.OrgID; the backing
// workitems.CreateLabel RPC self-gates on it (round-16 / bead
// unblock-tv8.77, §10.1.1): an empty CallerOrgID is HARD-REJECTED with
// VALIDATION (CreateLabel is MCP-only, no trusted-internal no-op), and on
// the project-scoped branch the insert proceeds only when the project
// belongs to CallerOrgID, so a foreign project_id yields NOT_FOUND rather
// than a cross-tenant write (DRIFT-2c locked decision).
//
// A duplicate name within the same scope (case-insensitive per the
// lower(name) UNIQUE indexes) surfaces from the backing RPC as
// AlreadyExists with Meta["constraint"]; mapError projects it into the §7
// CONFLICT envelope with data.constraint. Malformed color/name surface as
// §7 VALIDATION.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 20 + § 4.4
// (workitems.CreateLabel) + § 7 (error envelope).

package mcp

import (
	"context"

	"encore.app/workitems"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// createLabelIn is the JSON wire shape for create_label. org_id is NOT
// carried on the wire — org scope is pinned to identity.OrgID (see file
// doc-comment). project_id selects project-scoping; its absence selects
// org-scoping.
type createLabelIn struct {
	ProjectID   string `json:"project_id,omitempty"`
	Name        string `json:"name"`
	Color       string `json:"color"`
	Description string `json:"description,omitempty"`
}

type createLabelOut struct {
	Label labelWire `json:"label"`
}

// registerHandleCreateLabel is invoked by transport.go's init — see the
// toolRegistrars rationale there.
func registerHandleCreateLabel(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "create_label",
		Description: "Create a label in the registry, scoped to the caller's " +
			"org or to a project (pass project_id to project-scope; omit it to " +
			"org-scope). name is 1..64 chars and unique within scope " +
			"(case-insensitive); color is #RRGGBB. A duplicate name rejects " +
			"with CONFLICT carrying data.constraint. SPEC § 6.2 Tool 20.",
	}, handleCreateLabel)
}

func handleCreateLabel(ctx context.Context, req *sdkmcp.CallToolRequest, in createLabelIn) (*sdkmcp.CallToolResult, createLabelOut, error) {
	tool := "create_label"
	state := bindTool(req, tool)

	identity, ok := identityFromReq(req)
	if !ok {
		return nil, createLabelOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, createLabelOut{}, mapError(state, tool, err)
	}

	// Scope resolution: project_id ⇒ project-scoped; otherwise org-scoped
	// on the caller's identity.OrgID. Exactly one of (OrgID, ProjectID) is
	// passed to the backing RPC, satisfying the XOR the RPC + DDL CHECK
	// enforce. A client cannot name a foreign org because OrgID is never
	// read from the wire.
	//
	// CallerOrgID is ALWAYS pinned to identity.OrgID (never the wire). On the
	// project-scoped branch the backing RPC uses it to gate the insert on the
	// project belonging to the caller's org — a foreign project ULID yields
	// NOT_FOUND, never a cross-tenant write (DRIFT-2c locked decision).
	scope := workitems.CreateLabelRequest{
		CallerOrgID: identity.OrgID,
		Name:        in.Name,
		Color:       in.Color,
		Description: in.Description,
	}
	if in.ProjectID != "" {
		scope.ProjectID = in.ProjectID
		if state != nil && state.Call != nil {
			state.Call.ProjectID = in.ProjectID
		}
	} else {
		scope.OrgID = identity.OrgID
	}

	label, err := workitems.CreateLabel(mcpCtx, &scope)
	if err != nil {
		return nil, createLabelOut{}, mapError(state, tool, err)
	}

	out := createLabelOut{Label: labelToWire(*label)}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		if label.ProjectID != "" {
			state.Call.ProjectID = label.ProjectID
		}
	}
	return nil, out, nil
}
