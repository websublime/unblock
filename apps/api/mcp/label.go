// label.go owns the JSON wire shape shared by the §6.2 Tool 20–23 label
// handlers (create_label, list_labels, update_label, delete_label) and the
// converter from the canonical workitems.Label Go struct.
//
// Each handler file owns its own request/response envelope type; the
// nested Label payload is factored here so the handlers that echo a Label
// (create / list / update) agree byte-for-byte on the wire shape and there
// is a single time→RFC3339Nano formatting site.
//
// All exported fields carry explicit snake_case json tags per the SPEC
// §3.6 wire convention (grep -rnE 'json:"[A-Z]' apps/api/ must be empty).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 4.4 (Label shape) + § 6.2
// Tools 20–23.
//
// round-16, bead unblock-tv8.75.

package mcp

import (
	"time"

	"encore.app/workitems"
)

// labelWire is the JSON wire shape of one workitems.labels row. OrgID is
// empty for project-scoped labels and ProjectID is empty for org-scoped
// ones (mirrors the canonical struct's empty-when-other-scope contract).
// Timestamps are rendered RFC3339Nano (UTC).
type labelWire struct {
	ID          string `json:"id"`
	OrgID       string `json:"org_id,omitempty"`
	ProjectID   string `json:"project_id,omitempty"`
	Name        string `json:"name"`
	Color       string `json:"color"`
	Description string `json:"description,omitempty"`
	CreatedAt   string `json:"created_at"`
	UpdatedAt   string `json:"updated_at"`
}

// labelToWire converts a canonical workitems.Label into its wire shape,
// rendering timestamps as RFC3339Nano (UTC).
func labelToWire(l workitems.Label) labelWire {
	return labelWire{
		ID:          l.ID,
		OrgID:       l.OrgID,
		ProjectID:   l.ProjectID,
		Name:        l.Name,
		Color:       l.Color,
		Description: l.Description,
		CreatedAt:   l.CreatedAt.UTC().Format(time.RFC3339Nano),
		UpdatedAt:   l.UpdatedAt.UTC().Format(time.RFC3339Nano),
	}
}
