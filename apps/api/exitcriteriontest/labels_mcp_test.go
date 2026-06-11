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
	"encore.app/shared/ulid"
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

// TestExitCriterion_LabelTools_CrossTenantRejected pins the round-16
// rework (bead unblock-tv8.75) cross-tenant write fixes across the MCP
// boundary:
//
//   - create_label with a FOREIGN project_id (a project in another org)
//     must be rejected NOT_FOUND, never create a label inside that
//     project — the project-scoped create gate validates the project
//     belongs to the caller's identity.OrgID (DRIFT-2c locked decision).
//   - list_labels scoped to a FOREIGN project_id must return zero rows
//     (the gated project branch yields no project rows AND no org leak) —
//     exercising the gated project branch with a project outside the
//     caller's org.
//   - update_label and delete_label targeting a FOREIGN label_id (a label
//     in another org/project) must return NOT_FOUND, never mutate or
//     delete it — the row-level tenant predicate makes a foreign label
//     indistinguishable from a missing one (DRIFT-3b).
//
// All four are driven as the fixture identity (Bearer bound to f.OrgID);
// the foreign org / project / labels are seeded directly via the DB so the
// caller's identity is never involved in their creation. Before the fixes
// the create/update/delete paths would have SUCCEEDED cross-tenant — these
// assertions are the regression guard.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 4.4 (label RPC row-level
// tenant predicates) + § 6.2 Tools 20/21/22/23 + § 7 (NOT_FOUND envelope).
func TestExitCriterion_LabelTools_CrossTenantRejected(t *testing.T) {
	f := fx(t)
	sessionID := initializeSession(t, f.RawKey)
	ctx := t.Context()

	// --- Seed a FOREIGN org + project (no membership, no API key for the
	// caller; the caller's Bearer is bound to f.OrgID only). ------------
	foreignOrgID := mustULID(t, "foreign org")
	foreignSlug := "foreign-" + foreignOrgID[len(foreignOrgID)-8:]
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		foreignOrgID, foreignSlug, "Foreign Org",
	); err != nil {
		t.Fatalf("insert foreign org: %v", err)
	}
	t.Cleanup(func() {
		// Background ctx because t.Context() is cancelled before cleanup
		// runs. ON DELETE CASCADE clears the foreign project + labels.
		_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM org.organizations WHERE id = $1`, foreignOrgID)
	})

	foreignProjID := mustULID(t, "foreign project")
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		foreignProjID, foreignOrgID, "foreign-proj", "Foreign Project",
	); err != nil {
		t.Fatalf("insert foreign project: %v", err)
	}

	// A foreign org-scoped label and a foreign project-scoped label,
	// inserted directly so the caller's identity never touches them.
	foreignOrgLabelID := mustULID(t, "foreign org label")
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.labels (id, org_id, name, color) VALUES ($1, $2, $3, $4)`,
		foreignOrgLabelID, foreignOrgID, "foreign-org-label", "#abcdef",
	); err != nil {
		t.Fatalf("insert foreign org label: %v", err)
	}
	foreignProjLabelID := mustULID(t, "foreign project label")
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.labels (id, project_id, name, color) VALUES ($1, $2, $3, $4)`,
		foreignProjLabelID, foreignProjID, "foreign-proj-label", "#fedcba",
	); err != nil {
		t.Fatalf("insert foreign project label: %v", err)
	}

	// --- 1) create_label into a FOREIGN project ⇒ NOT_FOUND ------------
	// A Bearer for f.OrgID passing a project ULID owned by foreignOrgID
	// must NOT create a label in that project (the cross-tenant write hole
	// the CRITICAL flagged). The gate makes it indistinguishable from a
	// missing project.
	createEnv := callTool(t, f.RawKey, sessionID, "create_label", map[string]any{
		"project_id": foreignProjID,
		"name":       "intruder",
		"color":      "#123456",
	})
	createErr := expectError(t, createEnv)
	if createErr.Kind != "NOT_FOUND" {
		t.Fatalf("cross-tenant create_label: kind = %q, want NOT_FOUND (foreign project must not be writable)", createErr.Kind)
	}
	// Assert nothing was persisted into the foreign project.
	var intruderCount int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT COUNT(*) FROM workitems.labels WHERE project_id = $1 AND lower(name) = 'intruder'`,
		foreignProjID,
	).Scan(&intruderCount); err != nil {
		t.Fatalf("count intruder labels: %v", err)
	}
	if intruderCount != 0 {
		t.Fatalf("cross-tenant create_label persisted %d label(s) into foreign project %s, want 0", intruderCount, foreignProjID)
	}

	// --- 2) list_labels scoped to a FOREIGN project ⇒ zero rows --------
	// Exercises the gated project branch with a project outside the
	// caller's org. The project branch yields no rows (project_id not in
	// the caller's org's projects) AND the org-inheritance branch is gated
	// on the caller's org_id, so no foreign rows leak. The result must be
	// empty — in particular it must NOT contain the foreign project label.
	listEnv := callTool(t, f.RawKey, sessionID, "list_labels", map[string]any{
		"project_id": foreignProjID,
	})
	var foreignList mcpLabelList
	if err := json.Unmarshal(expectSuccess(t, listEnv), &foreignList); err != nil {
		t.Fatalf("unmarshal cross-tenant list_labels: %v", err)
	}
	if findLabel(foreignList.Labels, foreignProjLabelID) != nil {
		t.Fatalf("cross-tenant list_labels leaked foreign project label %s", foreignProjLabelID)
	}
	if findLabel(foreignList.Labels, foreignOrgLabelID) != nil {
		t.Fatalf("cross-tenant list_labels leaked foreign org label %s", foreignOrgLabelID)
	}
	if len(foreignList.Labels) != 0 {
		t.Fatalf("cross-tenant list_labels returned %d row(s): %+v; want empty (foreign project not in caller's org)", len(foreignList.Labels), foreignList.Labels)
	}

	// --- 3) update_label on a FOREIGN label ⇒ NOT_FOUND ----------------
	// Both the foreign org-scoped and foreign project-scoped labels must be
	// unreachable by id from the caller's identity (the row-level tenant
	// predicate). A rename attempt must fail and leave the row untouched.
	for _, tc := range []struct {
		name    string
		labelID string
	}{
		{"foreign org label", foreignOrgLabelID},
		{"foreign project label", foreignProjLabelID},
	} {
		t.Run("update_"+tc.name, func(t *testing.T) {
			updEnv := callTool(t, f.RawKey, sessionID, "update_label", map[string]any{
				"label_id": tc.labelID,
				"name":     "hijacked",
			})
			updErr := expectError(t, updEnv)
			if updErr.Kind != "NOT_FOUND" {
				t.Fatalf("cross-tenant update_label (%s): kind = %q, want NOT_FOUND", tc.name, updErr.Kind)
			}
			assertLabelName(t, ctx, tc.labelID, tc.labelID == foreignOrgLabelID, "foreign")
		})
	}

	// --- 4) delete_label on a FOREIGN label ⇒ NOT_FOUND ----------------
	// The foreign labels must survive a delete attempt by the caller.
	for _, tc := range []struct {
		name    string
		labelID string
	}{
		{"foreign org label", foreignOrgLabelID},
		{"foreign project label", foreignProjLabelID},
	} {
		t.Run("delete_"+tc.name, func(t *testing.T) {
			delEnv := callTool(t, f.RawKey, sessionID, "delete_label", map[string]any{
				"label_id": tc.labelID,
			})
			delErr := expectError(t, delEnv)
			if delErr.Kind != "NOT_FOUND" {
				t.Fatalf("cross-tenant delete_label (%s): kind = %q, want NOT_FOUND", tc.name, delErr.Kind)
			}
			assertLabelExists(t, ctx, tc.labelID)
		})
	}
}

// mustULID mints a fresh ULID or fails the test with the given context
// label. Shared by the cross-tenant seed steps.
func mustULID(t *testing.T, what string) string {
	t.Helper()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid %s: %v", what, err)
	}
	return id
}

// assertLabelName asserts the label row still carries its seeded name
// (i.e. a cross-tenant update_label did NOT mutate it). The `name` we
// expect is the seed value; wantPrefix is a readable marker only.
func assertLabelName(t *testing.T, ctx context.Context, labelID string, isOrgLabel bool, wantPrefix string) {
	t.Helper()
	want := "foreign-proj-label"
	if isOrgLabel {
		want = "foreign-org-label"
	}
	var got string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT name FROM workitems.labels WHERE id = $1`, labelID,
	).Scan(&got); err != nil {
		t.Fatalf("read label name for %s: %v", labelID, err)
	}
	if got != want {
		t.Fatalf("foreign label %s name = %q after cross-tenant update_label, want %q (must be untouched)", labelID, got, want)
	}
}

// assertLabelExists asserts the label row is still present (i.e. a
// cross-tenant delete_label did NOT remove it).
func assertLabelExists(t *testing.T, ctx context.Context, labelID string) {
	t.Helper()
	var n int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT COUNT(*) FROM workitems.labels WHERE id = $1`, labelID,
	).Scan(&n); err != nil {
		t.Fatalf("count label %s: %v", labelID, err)
	}
	if n != 1 {
		t.Fatalf("foreign label %s count = %d after cross-tenant delete_label, want 1 (must survive)", labelID, n)
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
