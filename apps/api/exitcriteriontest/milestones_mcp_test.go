// milestones_mcp_test.go covers the §11.1.2 round-16 (bead
// unblock-tv8.74) milestone-management assertions THROUGH the MCP tool
// surface — the complement to milestones_test.go, which drives the
// backing workitems RPCs directly via Encore's private mesh.
//
// The §14 approval checklist (spec line 3383) requires driving
// create_milestone → assign_item → milestone_tree across the Bearer /
// JSON-RPC boundary and asserting the tree shape, exercising the four
// new Tools 16–19 as agents actually call them:
//
//   - create_milestone: org-scoped (no project_id) parent + an
//     org-scoped nested child (M-INV-5 requires child scope to match the
//     parent's, so the nested child shares the parent's org scope); a
//     separate project-scoped create exercises the project_id wire path.
//     The structuredContent echoes the persisted Milestone.
//   - update_milestone: rename the parent; only the supplied field
//     changes.
//   - assign_item: assign a fixture item to the child milestone, then
//     unassign it (milestone_id="" ⇒ structuredContent milestone_id:null).
//   - milestone_tree: project-scoped roots walk returns the parent with
//     the child nested at depth 1, plus a rooted walk from the parent.
//
// All four tools share one Mcp-Session-Id (the SDK's stateful session
// model). Org scope is pinned to the Bearer-resolved Identity — no
// org_id is sent on the wire (DECISION on this bead).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tools 16–19 + § 4.4.1
// (milestone RPCs) + § 11.1.2 (functional assertions) + § 7 (envelope).

package exitcriteriontest_test

import (
	"context"
	"encoding/json"
	"testing"

	encoredb "encore.app/db"
)

// mcpMilestone is the typed shape of the structuredContent
// { "milestone": { … } } payload returned by create_milestone /
// update_milestone — only the fields these assertions read.
type mcpMilestone struct {
	Milestone struct {
		ID                string  `json:"id"`
		ParentMilestoneID string  `json:"parent_milestone_id"`
		OrgID             string  `json:"org_id"`
		ProjectID         string  `json:"project_id"`
		Name              string  `json:"name"`
		StartDate         string  `json:"start_date"`
		EndDate           string  `json:"end_date"`
		CancelledAt       *string `json:"cancelled_at"`
	} `json:"milestone"`
}

// mcpAssignResult is the synthesised assign_item structuredContent.
// MilestoneID is a *string so the unassign path's null is distinguishable
// from an empty string.
type mcpAssignResult struct {
	Assigned    bool    `json:"assigned"`
	ItemID      string  `json:"item_id"`
	MilestoneID *string `json:"milestone_id"`
}

// mcpMilestoneNode mirrors the milestone_tree structuredContent node
// shape (recursive). Only id + the nested children are read.
type mcpMilestoneNode struct {
	Milestone struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	} `json:"milestone"`
	Depth    int                `json:"depth"`
	Children []mcpMilestoneNode `json:"children"`
}

type mcpMilestoneTree struct {
	Roots []mcpMilestoneNode `json:"roots"`
}

// TestExitCriterion_MilestoneTools_MCPBoundary walks the round-16
// milestone-management tools across the MCP wire on a single session.
func TestExitCriterion_MilestoneTools_MCPBoundary(t *testing.T) {
	f := fx(t)
	sessionID := initializeSession(t, f.RawKey)

	// --- 1) create_milestone: org-scoped parent (no project_id) --------
	parentEnv := callTool(t, f.RawKey, sessionID, "create_milestone", map[string]any{
		"name":       "MCP Org Roadmap",
		"start_date": "2026-01-01",
		"end_date":   "2026-12-31",
	})
	var parent mcpMilestone
	if err := json.Unmarshal(expectSuccess(t, parentEnv), &parent); err != nil {
		t.Fatalf("unmarshal create_milestone parent: %v", err)
	}
	if parent.Milestone.ID == "" {
		t.Fatalf("create_milestone parent returned empty id")
	}
	if parent.Milestone.OrgID != f.OrgID {
		t.Fatalf("parent.org_id = %q, want identity org %q (org scope pinned to Bearer identity)", parent.Milestone.OrgID, f.OrgID)
	}
	if parent.Milestone.ProjectID != "" {
		t.Fatalf("parent.project_id = %q, want empty (org-scoped)", parent.Milestone.ProjectID)
	}

	// --- 2) create_milestone: project-scoped nested child --------------
	//
	// Child is scoped to the fixture project and nested under the parent.
	// M-INV-5 (child scope matches parent) is NOT violated: the parent is
	// org-scoped (org_id=identity.OrgID) and the child is project-scoped;
	// they share no scope axis, so this would FAIL M-INV-5. Use an
	// org-scoped child instead to keep the nesting valid and exercise the
	// tree shape — the project-scoped path is covered separately below.
	childEnv := callTool(t, f.RawKey, sessionID, "create_milestone", map[string]any{
		"parent_milestone_id": parent.Milestone.ID,
		"name":                "MCP Org Sprint 1",
		"start_date":          "2026-01-01",
		"end_date":            "2026-01-14",
	})
	var child mcpMilestone
	if err := json.Unmarshal(expectSuccess(t, childEnv), &child); err != nil {
		t.Fatalf("unmarshal create_milestone child: %v", err)
	}
	if child.Milestone.ParentMilestoneID != parent.Milestone.ID {
		t.Fatalf("child.parent_milestone_id = %q, want %q", child.Milestone.ParentMilestoneID, parent.Milestone.ID)
	}
	if child.Milestone.OrgID != f.OrgID {
		t.Fatalf("child.org_id = %q, want %q", child.Milestone.OrgID, f.OrgID)
	}

	// --- 2b) create_milestone: project-scoped (project_id wire path) ---
	//
	// Exercises the project_id argument: the milestone is project-scoped
	// (org_id empty, project_id set) — the alternate XOR branch the
	// org-scoped parent above does not cover.
	projEnv := callTool(t, f.RawKey, sessionID, "create_milestone", map[string]any{
		"project_id": f.ProjectID,
		"name":       "MCP Project Milestone",
		"start_date": "2026-02-01",
		"end_date":   "2026-02-28",
	})
	var projMS mcpMilestone
	if err := json.Unmarshal(expectSuccess(t, projEnv), &projMS); err != nil {
		t.Fatalf("unmarshal project-scoped create_milestone: %v", err)
	}
	if projMS.Milestone.ProjectID != f.ProjectID {
		t.Fatalf("project-scoped milestone: project_id = %q, want %q", projMS.Milestone.ProjectID, f.ProjectID)
	}
	if projMS.Milestone.OrgID != "" {
		t.Fatalf("project-scoped milestone: org_id = %q, want empty", projMS.Milestone.OrgID)
	}

	// --- 3) update_milestone: rename the parent ------------------------
	renamed := "MCP Org Roadmap (renamed)"
	updEnv := callTool(t, f.RawKey, sessionID, "update_milestone", map[string]any{
		"milestone_id": parent.Milestone.ID,
		"name":         renamed,
	})
	var updated mcpMilestone
	if err := json.Unmarshal(expectSuccess(t, updEnv), &updated); err != nil {
		t.Fatalf("unmarshal update_milestone: %v", err)
	}
	if updated.Milestone.Name != renamed {
		t.Fatalf("update_milestone: name = %q, want %q", updated.Milestone.Name, renamed)
	}
	// Dates were not supplied — they must be unchanged.
	if updated.Milestone.StartDate != "2026-01-01" || updated.Milestone.EndDate != "2026-12-31" {
		t.Fatalf("update_milestone changed unspecified dates: start=%q end=%q", updated.Milestone.StartDate, updated.Milestone.EndDate)
	}

	// --- 4) assign_item: assign a fixture item to the child ------------
	//
	// itm_d is an org/project item in the fixture. The child milestone is
	// org-scoped (org_id=identity.OrgID), so M-INV-7 reachability holds:
	// milestone.org_id == item.org_id.
	target := f.ItemID("itm_d")
	assignEnv := callTool(t, f.RawKey, sessionID, "assign_item", map[string]any{
		"item_id":      target,
		"milestone_id": child.Milestone.ID,
	})
	var assigned mcpAssignResult
	if err := json.Unmarshal(expectSuccess(t, assignEnv), &assigned); err != nil {
		t.Fatalf("unmarshal assign_item: %v", err)
	}
	if !assigned.Assigned {
		t.Fatalf("assign_item: assigned = false, want true")
	}
	if assigned.MilestoneID == nil || *assigned.MilestoneID != child.Milestone.ID {
		t.Fatalf("assign_item: milestone_id = %v, want %q", assigned.MilestoneID, child.Milestone.ID)
	}
	// Belt-and-suspenders: the column reflects the assignment.
	assertItemMilestone(t, t.Context(), target, child.Milestone.ID)

	// --- 5) milestone_tree: org roots walk, parent with nested child ---
	treeEnv := callTool(t, f.RawKey, sessionID, "milestone_tree", map[string]any{})
	var tree mcpMilestoneTree
	if err := json.Unmarshal(expectSuccess(t, treeEnv), &tree); err != nil {
		t.Fatalf("unmarshal milestone_tree: %v", err)
	}
	node := findMilestoneNode(tree.Roots, parent.Milestone.ID)
	if node == nil {
		t.Fatalf("milestone_tree roots did not contain parent %s; roots=%+v", parent.Milestone.ID, tree.Roots)
	}
	if node.Depth != 0 {
		t.Fatalf("parent node depth = %d, want 0 (root of the walk)", node.Depth)
	}
	childNode := findMilestoneNode(node.Children, child.Milestone.ID)
	if childNode == nil {
		t.Fatalf("parent node did not nest child %s; children=%+v", child.Milestone.ID, node.Children)
	}
	if childNode.Depth != 1 {
		t.Fatalf("child node depth = %d, want 1", childNode.Depth)
	}

	// --- 6) milestone_tree: rooted walk from the parent ----------------
	rootedEnv := callTool(t, f.RawKey, sessionID, "milestone_tree", map[string]any{
		"root_milestone_id": parent.Milestone.ID,
	})
	var rooted mcpMilestoneTree
	if err := json.Unmarshal(expectSuccess(t, rootedEnv), &rooted); err != nil {
		t.Fatalf("unmarshal rooted milestone_tree: %v", err)
	}
	if len(rooted.Roots) != 1 || rooted.Roots[0].Milestone.ID != parent.Milestone.ID {
		t.Fatalf("rooted milestone_tree roots = %+v, want single parent root", rooted.Roots)
	}
	if findMilestoneNode(rooted.Roots[0].Children, child.Milestone.ID) == nil {
		t.Fatalf("rooted walk did not nest the child under the parent")
	}

	// --- 7) assign_item unassign: milestone_id:null --------------------
	unassignEnv := callTool(t, f.RawKey, sessionID, "assign_item", map[string]any{
		"item_id":      target,
		"milestone_id": "",
	})
	var unassigned mcpAssignResult
	if err := json.Unmarshal(expectSuccess(t, unassignEnv), &unassigned); err != nil {
		t.Fatalf("unmarshal assign_item unassign: %v", err)
	}
	if !unassigned.Assigned {
		t.Fatalf("unassign: assigned = false, want true")
	}
	if unassigned.MilestoneID != nil {
		t.Fatalf("unassign: milestone_id = %v, want null", *unassigned.MilestoneID)
	}
	assertItemMilestoneCleared(t, t.Context(), target)
}

// findMilestoneNode returns the node with the given milestone id within
// the slice (non-recursive — searches one level), or nil. Callers walk
// the tree level-by-level (roots, then a node's children) so a flat
// search per level keeps the assertions explicit about depth.
func findMilestoneNode(nodes []mcpMilestoneNode, id string) *mcpMilestoneNode {
	for i := range nodes {
		if nodes[i].Milestone.ID == id {
			return &nodes[i]
		}
	}
	return nil
}

// assertItemMilestone asserts the item's milestone_id column equals want.
// Reads the column directly (the test goroutine has no Identity for the
// rbac.For read path) — same pattern as milestones_test.go.
func assertItemMilestone(t *testing.T, ctx context.Context, itemID, want string) {
	t.Helper()
	var got *string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT milestone_id FROM workitems.items WHERE id = $1`, itemID,
	).Scan(&got); err != nil {
		t.Fatalf("read milestone_id for %s: %v", itemID, err)
	}
	if got == nil || *got != want {
		t.Fatalf("item %s: milestone_id = %v, want %q", itemID, got, want)
	}
}

// assertItemMilestoneCleared asserts the item's milestone columns are all
// NULL after an unassign.
func assertItemMilestoneCleared(t *testing.T, ctx context.Context, itemID string) {
	t.Helper()
	var milestoneID, assignedBy *string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT milestone_id, milestone_assigned_by FROM workitems.items WHERE id = $1`, itemID,
	).Scan(&milestoneID, &assignedBy); err != nil {
		t.Fatalf("read milestone columns for %s: %v", itemID, err)
	}
	if milestoneID != nil {
		t.Fatalf("item %s: milestone_id = %q after unassign, want NULL", itemID, *milestoneID)
	}
	if assignedBy != nil {
		t.Fatalf("item %s: milestone_assigned_by = %q after unassign, want NULL", itemID, *assignedBy)
	}
}
