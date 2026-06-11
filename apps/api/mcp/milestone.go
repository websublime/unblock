// milestone.go owns the JSON wire shapes shared by the §6.2 Tool 16–19
// milestone handlers (create_milestone, update_milestone, assign_item,
// milestone_tree) and the converters from the canonical
// workitems.Milestone / workitems.MilestoneNode Go structs.
//
// Each handler file owns its own request/response envelope type; the
// nested Milestone + MilestoneNode payloads are factored here so the
// three handlers that echo a Milestone (create / update / milestone_tree)
// agree byte-for-byte on the wire shape and there is a single
// time→RFC3339Nano formatting site.
//
// All exported fields carry explicit snake_case json tags per the
// SPEC §3.6 wire convention (grep -rnE 'json:"[A-Z]' apps/api/ must be
// empty).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 4.4.1 (Milestone /
// MilestoneNode shapes) + § 6.2 Tools 16–19.

package mcp

import (
	"time"

	"encore.app/workitems"
)

// milestoneWire is the JSON wire shape of one workitems.Milestone row.
// Timestamps are rendered RFC3339Nano (UTC); CancelledAt is null when
// the milestone is not cancelled. OrgID is empty for project-scoped
// milestones and ProjectID is empty for org-scoped ones (mirrors the
// canonical struct's empty-when-other-scope contract).
type milestoneWire struct {
	ID                string  `json:"id"`
	ParentMilestoneID string  `json:"parent_milestone_id,omitempty"`
	OrgID             string  `json:"org_id,omitempty"`
	ProjectID         string  `json:"project_id,omitempty"`
	Name              string  `json:"name"`
	Description       string  `json:"description,omitempty"`
	StartDate         string  `json:"start_date"`
	EndDate           string  `json:"end_date"`
	CancelledAt       *string `json:"cancelled_at"`
	CancelledReason   string  `json:"cancelled_reason,omitempty"`
	CreatedAt         string  `json:"created_at"`
	UpdatedAt         string  `json:"updated_at"`
}

// milestoneNodeWire is one node of the recursive milestone_tree
// response: a Milestone plus its depth (0 at a root of the requested
// walk) and its nested children.
type milestoneNodeWire struct {
	Milestone milestoneWire       `json:"milestone"`
	Depth     int                 `json:"depth"`
	Children  []milestoneNodeWire `json:"children"`
}

// milestoneToWire converts a canonical workitems.Milestone into its
// wire shape, rendering timestamps as RFC3339Nano (UTC). CancelledAt is
// nil-preserving: a non-cancelled milestone serialises cancelled_at:null.
func milestoneToWire(m workitems.Milestone) milestoneWire {
	w := milestoneWire{
		ID:                m.ID,
		ParentMilestoneID: m.ParentMilestoneID,
		OrgID:             m.OrgID,
		ProjectID:         m.ProjectID,
		Name:              m.Name,
		Description:       m.Description,
		StartDate:         m.StartDate,
		EndDate:           m.EndDate,
		CancelledReason:   m.CancelledReason,
		CreatedAt:         m.CreatedAt.UTC().Format(time.RFC3339Nano),
		UpdatedAt:         m.UpdatedAt.UTC().Format(time.RFC3339Nano),
	}
	if m.CancelledAt != nil {
		s := m.CancelledAt.UTC().Format(time.RFC3339Nano)
		w.CancelledAt = &s
	}
	return w
}

// milestoneNodeToWire recursively converts a workitems.MilestoneNode
// (and its subtree) into the wire shape. The children slice is always
// non-nil (empty for a leaf) so the JSON renders `"children": []`
// rather than `"children": null`, matching the §4.4.1 MilestoneNode
// contract.
func milestoneNodeToWire(n workitems.MilestoneNode) milestoneNodeWire {
	children := make([]milestoneNodeWire, 0, len(n.Children))
	for _, c := range n.Children {
		children = append(children, milestoneNodeToWire(c))
	}
	return milestoneNodeWire{
		Milestone: milestoneToWire(n.Milestone),
		Depth:     n.Depth,
		Children:  children,
	}
}
