// labels_mcp_test.go covers the §11.4 / §14-approval-checklist round-16
// (bead unblock-tv8.75) label-registry assertions THROUGH the MCP tool
// surface — the four new Tools 20–23 (create_label / list_labels /
// update_label / delete_label) as agents actually call them.
//
// The §14 approval checklist (spec line 3467) requires driving
// create_label → list_labels → update_label → delete_label across the
// Bearer / JSON-RPC boundary and asserting that the registry round-trips
// and delete_label detaches the label from any items it was applied to:
//
//   - create_label: an org-scoped label (no project_id) and a
//     project-scoped label (project_id wire path). The structuredContent
//     echoes the persisted Label; org scope is pinned to the
//     Bearer-resolved Identity (no org_id on the wire — DECISION on this
//     bead).
//   - list_labels: the org-scoped list returns the org label; the
//     project-scoped list returns the project label PLUS the inherited org
//     label, with "project wins on identical name" (PRD §6.4) applied.
//   - update_label: rename + recolor; only the supplied fields change and
//     updated_at advances past created_at (migration 0130).
//   - delete_label: after attaching the org label to a fixture item
//     directly via the junction table, delete_label reports
//     detached_item_count = 1 and the attachment is gone (the FK cascade).
//
// A duplicate-name create on the same scope asserts the §7 CONFLICT
// envelope carries data.constraint naming the violated UNIQUE index
// (case-insensitive: "Bug" collides with "bug").
//
// All tool calls share one Mcp-Session-Id (the SDK's stateful session
// model).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tools 20–23 + § 4.4
// (label RPCs) + § 7 (envelope) + § 11.4.

package exitcriteriontest_test

import (
	"context"
	"encoding/json"
	"testing"

	encoredb "encore.app/db"
)

// mcpLabel is the typed shape of the structuredContent { "label": { … } }
// payload returned by create_label / update_label.
type mcpLabel struct {
	Label struct {
		ID          string `json:"id"`
		OrgID       string `json:"org_id"`
		ProjectID   string `json:"project_id"`
		Name        string `json:"name"`
		Color       string `json:"color"`
		Description string `json:"description"`
		CreatedAt   string `json:"created_at"`
		UpdatedAt   string `json:"updated_at"`
	} `json:"label"`
}

// mcpLabelRow is one element of the list_labels structuredContent.
type mcpLabelRow struct {
	ID        string `json:"id"`
	OrgID     string `json:"org_id"`
	ProjectID string `json:"project_id"`
	Name      string `json:"name"`
	Color     string `json:"color"`
}

// mcpLabelList is the list_labels structuredContent { "labels": [ … ] }.
type mcpLabelList struct {
	Labels []mcpLabelRow `json:"labels"`
}

// mcpDeleteLabel is the delete_label structuredContent.
type mcpDeleteLabel struct {
	Deleted           bool   `json:"deleted"`
	LabelID           string `json:"label_id"`
	DetachedItemCount int    `json:"detached_item_count"`
}

// TestExitCriterion_LabelTools_MCPBoundary walks the round-16 label
// management tools across the MCP wire on a single session.
func TestExitCriterion_LabelTools_MCPBoundary(t *testing.T) {
	f := fx(t)
	sessionID := initializeSession(t, f.RawKey)
	ctx := t.Context()

	// --- 1) create_label: org-scoped (no project_id) -------------------
	orgEnv := callTool(t, f.RawKey, sessionID, "create_label", map[string]any{
		"name":        "bug",
		"color":       "#d73a4a",
		"description": "Something is broken",
	})
	var orgLabel mcpLabel
	if err := json.Unmarshal(expectSuccess(t, orgEnv), &orgLabel); err != nil {
		t.Fatalf("unmarshal create_label org: %v", err)
	}
	if orgLabel.Label.ID == "" {
		t.Fatalf("create_label org returned empty id")
	}
	if orgLabel.Label.OrgID != f.OrgID {
		t.Fatalf("org label.org_id = %q, want identity org %q (org scope pinned to Bearer identity)", orgLabel.Label.OrgID, f.OrgID)
	}
	if orgLabel.Label.ProjectID != "" {
		t.Fatalf("org label.project_id = %q, want empty (org-scoped)", orgLabel.Label.ProjectID)
	}
	if orgLabel.Label.Name != "bug" || orgLabel.Label.Color != "#d73a4a" {
		t.Fatalf("org label name/color = %q/%q, want bug/#d73a4a", orgLabel.Label.Name, orgLabel.Label.Color)
	}

	// --- 2) create_label: project-scoped (project_id wire path) --------
	//
	// Same name as the org label ("bug"), but project-scoped. PRD §6.4
	// "project wins on identical name" makes the project label shadow the
	// org one in a project-scoped list. The two live in different UNIQUE
	// scopes so this is NOT a CONFLICT.
	projEnv := callTool(t, f.RawKey, sessionID, "create_label", map[string]any{
		"project_id": f.ProjectID,
		"name":       "bug",
		"color":      "#00ff00",
	})
	var projLabel mcpLabel
	if err := json.Unmarshal(expectSuccess(t, projEnv), &projLabel); err != nil {
		t.Fatalf("unmarshal create_label project: %v", err)
	}
	if projLabel.Label.ProjectID != f.ProjectID {
		t.Fatalf("project label.project_id = %q, want %q", projLabel.Label.ProjectID, f.ProjectID)
	}
	if projLabel.Label.OrgID != "" {
		t.Fatalf("project label.org_id = %q, want empty (project-scoped)", projLabel.Label.OrgID)
	}

	// --- 3) create_label: duplicate name in org scope ⇒ CONFLICT -------
	//
	// Case-insensitive uniqueness: "Bug" collides with the org "bug" via
	// the labels_org_name_uniq lower(name) index. The §7 envelope carries
	// data.constraint naming the violated index.
	dupEnv := callTool(t, f.RawKey, sessionID, "create_label", map[string]any{
		"name":  "Bug",
		"color": "#123456",
	})
	dupErr := expectError(t, dupEnv)
	if dupErr.Kind != "CONFLICT" {
		t.Fatalf("duplicate create_label: kind = %q, want CONFLICT", dupErr.Kind)
	}
	if got, _ := dupErr.Details["constraint"].(string); got != "labels_org_name_uniq" {
		t.Fatalf("duplicate create_label: data.constraint = %q, want labels_org_name_uniq", got)
	}

	// --- 4) list_labels: org scope returns the org label ---------------
	orgListEnv := callTool(t, f.RawKey, sessionID, "list_labels", map[string]any{})
	var orgList mcpLabelList
	if err := json.Unmarshal(expectSuccess(t, orgListEnv), &orgList); err != nil {
		t.Fatalf("unmarshal list_labels org: %v", err)
	}
	if findLabel(orgList.Labels, orgLabel.Label.ID) == nil {
		t.Fatalf("org list_labels did not contain org label %s; labels=%+v", orgLabel.Label.ID, orgList.Labels)
	}
	// The project-scoped label must NOT appear in the org-only list.
	if findLabel(orgList.Labels, projLabel.Label.ID) != nil {
		t.Fatalf("org list_labels leaked project label %s", projLabel.Label.ID)
	}

	// --- 5) list_labels: project scope, "project wins on identical name"
	projListEnv := callTool(t, f.RawKey, sessionID, "list_labels", map[string]any{
		"project_id": f.ProjectID,
	})
	var projList mcpLabelList
	if err := json.Unmarshal(expectSuccess(t, projListEnv), &projList); err != nil {
		t.Fatalf("unmarshal list_labels project: %v", err)
	}
	// The project label is present; the org "bug" is SHADOWED (suppressed)
	// because the project defines a same-name label (PRD §6.4).
	if findLabel(projList.Labels, projLabel.Label.ID) == nil {
		t.Fatalf("project list_labels did not contain project label %s; labels=%+v", projLabel.Label.ID, projList.Labels)
	}
	if findLabel(projList.Labels, orgLabel.Label.ID) != nil {
		t.Fatalf("project list_labels returned the SHADOWED org label %s; 'project wins on identical name' (PRD §6.4) must suppress it", orgLabel.Label.ID)
	}
	// Exactly one "bug" survives in the project list (the project's).
	var bugCount int
	for _, l := range projList.Labels {
		if l.Name == "bug" {
			bugCount++
		}
	}
	if bugCount != 1 {
		t.Fatalf("project list has %d labels named 'bug', want exactly 1 (project shadows org)", bugCount)
	}

	// --- 6) update_label: rename + recolor the org label ---------------
	updEnv := callTool(t, f.RawKey, sessionID, "update_label", map[string]any{
		"label_id": orgLabel.Label.ID,
		"name":     "defect",
		"color":    "#abcdef",
	})
	var updated mcpLabel
	if err := json.Unmarshal(expectSuccess(t, updEnv), &updated); err != nil {
		t.Fatalf("unmarshal update_label: %v", err)
	}
	if updated.Label.Name != "defect" || updated.Label.Color != "#abcdef" {
		t.Fatalf("update_label: name/color = %q/%q, want defect/#abcdef", updated.Label.Name, updated.Label.Color)
	}
	// Description was not supplied — it must be unchanged.
	if updated.Label.Description != "Something is broken" {
		t.Fatalf("update_label changed unspecified description: %q", updated.Label.Description)
	}
	// updated_at must advance past created_at (migration 0130 + the now()
	// bump on every write).
	if updated.Label.UpdatedAt <= updated.Label.CreatedAt {
		t.Fatalf("update_label: updated_at %q did not advance past created_at %q", updated.Label.UpdatedAt, updated.Label.CreatedAt)
	}

	// --- 7) Attach the org label to a fixture item, then delete_label --
	//
	// Attach directly via the junction table (the test goroutine has no
	// Identity for the create-tool label path; the milestones harness seeds
	// foreign rows the same way). After delete_label the attachment must be
	// gone via the ON DELETE CASCADE FK, and detached_item_count = 1.
	target := f.ItemID("itm_d")
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.item_labels (item_id, label_id) VALUES ($1, $2)`,
		target, orgLabel.Label.ID,
	); err != nil {
		t.Fatalf("attach label to item: %v", err)
	}

	delEnv := callTool(t, f.RawKey, sessionID, "delete_label", map[string]any{
		"label_id": orgLabel.Label.ID,
	})
	var deleted mcpDeleteLabel
	if err := json.Unmarshal(expectSuccess(t, delEnv), &deleted); err != nil {
		t.Fatalf("unmarshal delete_label: %v", err)
	}
	if !deleted.Deleted {
		t.Fatalf("delete_label: deleted = false, want true")
	}
	if deleted.LabelID != orgLabel.Label.ID {
		t.Fatalf("delete_label: label_id = %q, want %q", deleted.LabelID, orgLabel.Label.ID)
	}
	if deleted.DetachedItemCount != 1 {
		t.Fatalf("delete_label: detached_item_count = %d, want 1 (the one attached item)", deleted.DetachedItemCount)
	}
	// The junction row is gone.
	assertLabelDetached(t, ctx, target, orgLabel.Label.ID)
	// The label is gone from the registry.
	postEnv := callTool(t, f.RawKey, sessionID, "list_labels", map[string]any{})
	var postList mcpLabelList
	if err := json.Unmarshal(expectSuccess(t, postEnv), &postList); err != nil {
		t.Fatalf("unmarshal post-delete list_labels: %v", err)
	}
	if findLabel(postList.Labels, orgLabel.Label.ID) != nil {
		t.Fatalf("deleted label %s still present in list_labels", orgLabel.Label.ID)
	}

	// --- 8) delete_label on a missing label ⇒ NOT_FOUND ----------------
	missEnv := callTool(t, f.RawKey, sessionID, "delete_label", map[string]any{
		"label_id": "01HZZZZZZZZZZZZZZZZZZZZZZZZ",
	})
	missErr := expectError(t, missEnv)
	if missErr.Kind != "NOT_FOUND" {
		t.Fatalf("delete_label missing: kind = %q, want NOT_FOUND", missErr.Kind)
	}
}

// findLabel returns the label row with the given id within the slice, or
// nil (non-recursive — a flat search over the list).
func findLabel(labels []mcpLabelRow, id string) *mcpLabelRow {
	for i := range labels {
		if labels[i].ID == id {
			return &labels[i]
		}
	}
	return nil
}

// assertLabelDetached asserts the (item, label) junction row is gone.
func assertLabelDetached(t *testing.T, ctx context.Context, itemID, labelID string) {
	t.Helper()
	var n int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT COUNT(*) FROM workitems.item_labels WHERE item_id = $1 AND label_id = $2`,
		itemID, labelID,
	).Scan(&n); err != nil {
		t.Fatalf("read item_labels for %s/%s: %v", itemID, labelID, err)
	}
	if n != 0 {
		t.Fatalf("item_labels(%s,%s) count = %d after delete_label, want 0 (ON DELETE CASCADE)", itemID, labelID, n)
	}
}
